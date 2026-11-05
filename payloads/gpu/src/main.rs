//! The GPU driver (GPU.md): the Mali-G715's CSF microcontroller booted from Arm's firmware, its global interface answering, and a command-stream job run on it — as a process, the kernel holding only the GPU's DMA fence (s2mpu::fence_gpu) and its power.
//!
//! Spawn: `ferros-bridge spawn gpu.bin --name gpu --dma gpu-regs --dma gpu-mem --store gpu --irq gpu-job`. Grant slots: 0 console, 1 the GPU's registers (`gpu-regs`, EL0 Device, mapped at their physical address), 2 the memory the GPU and this process share (`gpu-mem`, Normal non-cacheable), 3 the vault scope `gpu.` (the firmware, `gpu.fw`), 4 the GPU's job line (`gpu-job`).
//!
//! The sequence is kbase's (r52p0, Google's tree), condensed in GPU.md "Bring-up": reset, L2, the MCU's address space and the firmware's sections, the MCU started, the global interface's init and a ping, then one command-stream group with one queue running a job of three instructions that writes a word to memory.
#![no_std]
#![no_main]

// The display's present stage itself (its NEON path and its tables), compiled here so the GPU's version is checked against the code the device runs.
#[path = "../../display/src/neon.rs"]
#[allow(dead_code)]
mod neon;
#[path = "../../display/src/odt.rs"]
#[allow(dead_code)]
mod odt;
mod fw;
mod mem;

use core::fmt::Write;
use ferros_sys::{cap, console, exit, freq, now, window};
use mem::{rd32, wr32, wr64, Mem, Table, PAGE};

const S_REGS: u64 = 1;
const S_MEM: u64 = 2;
const S_STORE: u64 = 3;
const S_IRQ: u64 = 4;

/// A console line in a fixed buffer.
pub struct Line([u8; 200], usize);
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.0.len() - self.1);
        self.0[self.1..self.1 + n].copy_from_slice(&b[..n]);
        self.1 += n;
        Ok(())
    }
}

macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line([0; 200], 0);
        let _ = write!(l, $($t)*);
        console($g, &l.0[..l.1]);
    }};
}

// GPU_CONTROL (GPU.md "Registers").
const GPU_ID: usize = 0x000;
const GPU_IRQ_RAWSTAT: usize = 0x020;
const GPU_IRQ_CLEAR: usize = 0x024;
const GPU_IRQ_MASK: usize = 0x028;
const GPU_COMMAND: usize = 0x030;
const GPU_STATUS: usize = 0x034;
const GPU_FAULTSTATUS: usize = 0x03C;
const L2_CONFIG: usize = 0x048;
const THREAD_MAX_THREADS: usize = 0x0A0;
const SHADER_PRESENT: usize = 0x100;
const TILER_PRESENT: usize = 0x110;
const L2_PRESENT: usize = 0x120;
const L2_READY: usize = 0x160;
const L2_PWRON: usize = 0x1A0;
const L2_PWRTRANS: usize = 0x220;
const ASN_HASH: usize = 0x2C0;
const COHERENCY_ENABLE: usize = 0x304;
const SYSC_ALLOC: usize = 0x340;
const MCU_CONTROL: usize = 0x700;
const MCU_STATUS: usize = 0x704;
const CSF_CONFIG: usize = 0xF00;
const SHADER_CONFIG: usize = 0xF04;
const TILER_CONFIG: usize = 0xF08;
const L2_MMU_CONFIG: usize = 0xF0C;
// JOB_CONTROL.
const JOB_IRQ_RAWSTAT: usize = 0x1000;
const JOB_IRQ_CLEAR: usize = 0x1004;
const JOB_IRQ_MASK: usize = 0x1008;
// MMU_CONTROL, and an address space's registers at G#2400 + n · G#40.
const MMU_IRQ_RAWSTAT: usize = 0x2000;
const MMU_IRQ_CLEAR: usize = 0x2004;
const MMU_IRQ_MASK: usize = 0x2008;
const AS_TRANSTAB: usize = 0x00;
const AS_MEMATTR: usize = 0x08;
const AS_COMMAND: usize = 0x18;
const AS_FAULTSTATUS: usize = 0x1C;
const AS_FAULTADDRESS: usize = 0x20;
const AS_STATUS: usize = 0x28;
const AS_TRANSCFG: usize = 0x30;
/// Doorbell n's page: G#8_0000 + n · G#1_0000; doorbell 0 is the global interface's.
const DOORBELLS: usize = 0x8_0000;

/// GPU_IRQ bits: RESET_COMPLETED, CLEAN_CACHES_COMPLETED; the mask kbase runs with (GPU_FAULT, PROTECTED_FAULT, MCU_STATUS, RESET_COMPLETED, POWER_CHANGED_ALL), and without POWER_CHANGED_ALL once the MCU owns the shader cores' power.
const RESET_COMPLETED: u32 = 1 << 8;
const IRQ_ALL: u32 = 0x8_0503;
const IRQ_MCU_UP: u32 = 0x8_0103;
/// AS_MEMATTR as kbase writes it for every address space: index 0 write-back (implementation alloc), 5 non-cacheable, 6 shared.
const MEMATTR: u64 = 0x0008_4C8D_888D_8F88;
/// AS_TRANSCFG: AArch64 4 KiB tables, page-table walks write-back, read-allocate (non-coherent system: no shareability).
const TRANSCFG: u64 = 0x4200_0006;

/// Page-table entries (GPU.md "MMU"): a table descriptor; a level-3 page with the access flag, its MEMATTR index (bits 4:2), read-only (G#C0) or read-write (G#40), and execute-never (bit 54).
const TABLE: u64 = 0x403;
const PAGE_RO: u64 = 0x4C3;
const PAGE_RW: u64 = 0x443;
const NC: u64 = 5 << 2;
const NX: u64 = 1 << 54;

/// The MCU's view: its shared interface at G#0400_0000, and where this driver maps a group's suspend buffer and a queue's two user-IO pages (the MCU shared zone, after the interface, before any mapping exists, so no live update is needed).
const SHARED_VA: u64 = 0x0400_0000;
const SUSPEND_VA: u64 = 0x0410_0000;
const SUSPEND_PAGES: usize = 64;
const USER_IO_VA: u64 = 0x0420_0000;
/// The group's own address space (1): its ring buffer and the buffer the job writes.
const RING_VA: u64 = 0x1000_0000;
const DST_VA: u64 = 0x2000_0000;
/// The compute job's pages in address space 1: the shader (executable), its descriptors and uniforms, the buffer it writes and its done flag.
const CODE_VA: u64 = 0x3000_0000;
const DESC_VA: u64 = 0x3100_0000;
const OUT_VA: u64 = 0x3200_0000;
const BIG_VA: u64 = 0x3300_0000;
/// The show's picture: its address in the group's address space and its size (the width a power of two: the kernel finds a pixel's column with a shift).
const IMG_VA: u64 = 0x4000_0000;
const IMG_W: usize = 1024;
const IMG_H: usize = 2048;
/// The present-stage bench: 1M pixels (a quarter of a phase frame), its output at BENCH_VA, its input the picture's buffer read as 64-bit pixels.
const BENCH_PX: usize = 1 << 20;
const BENCH_VA: u64 = 0x5000_0000;
/// The snoop test (input bit 3): gpu-mem's CPU-cacheable last 2 MiB (kernel cachemap GPU_SNOOP), one page of it seen by the GPU non-cacheable and again coherent, and a non-cacheable page for what the GPU read.
const SNOOP_NC_VA: u64 = 0x6000_0000;
const SNOOP_CO_VA: u64 = 0x6100_0000;
const RES_VA: u64 = 0x6200_0000;
/// The present-stage test: 512 rows of the panel's width (1344 = 84 x 16, so whole workgroups), and where its canvas, output words and tables sit in the group's address space.
const PRESENT_W: usize = 1344;
const PRESENT_H: usize = 512;
const TAB_BYTES: usize = 131072 + 5 * 1024;
const CANV_VA: u64 = 0x7000_0000;
const WORDS_VA: u64 = 0x7100_0000;
const TAB_VA: u64 = 0x7200_0000;

struct Gpu {
    regs: usize,
    grants: u64,
    f: u64,
}

impl Gpu {
    fn rd(&self, off: usize) -> u32 {
        rd32(self.regs + off)
    }
    fn wr(&self, off: usize, v: u32) {
        wr32(self.regs + off, v)
    }
    fn rd64(&self, off: usize) -> u64 {
        self.rd(off) as u64 | (self.rd(off + 4) as u64) << 32
    }
    fn wr64(&self, off: usize, v: u64) {
        self.wr(off, v as u32);
        self.wr(off + 4, (v >> 32) as u32);
    }
    /// Poll `done` for up to `ms` milliseconds; whether it came true.
    fn wait(&self, ms: u64, done: impl Fn(&Gpu) -> bool) -> bool {
        let t0 = now();
        while now().wrapping_sub(t0) < self.f / 1000 * ms {
            if done(self) {
                return true;
            }
        }
        done(self)
    }
    fn as_reg(n: usize, r: usize) -> usize {
        0x2400 + n * 0x40 + r
    }
    /// Install `root` as address space `n` (kbase_mmu_hw_configure): wait idle, the three registers, UPDATE, wait idle.
    fn install(&self, n: usize, root: usize) -> bool {
        let idle = |g: &Gpu| g.rd(Gpu::as_reg(n, AS_STATUS)) & 1 == 0;
        if !self.wait(64, idle) {
            return false;
        }
        self.wr64(Gpu::as_reg(n, AS_TRANSCFG), TRANSCFG);
        self.wr64(Gpu::as_reg(n, AS_TRANSTAB), root as u64);
        self.wr64(Gpu::as_reg(n, AS_MEMATTR), MEMATTR);
        self.wr(Gpu::as_reg(n, AS_COMMAND), 1);
        self.wait(64, idle)
    }
    fn doorbell(&self, n: usize) {
        self.wr(DOORBELLS + n * 0x1_0000, 1);
    }
    fn fail(&self, what: &str) -> ! {
        say!(self.grants, "gpu: {what}: GPU_STATUS G#{:X} GPU_FAULTSTATUS G#{:X} GPU_IRQ_RAWSTAT G#{:X} JOB_IRQ_RAWSTAT G#{:X} MMU_IRQ_RAWSTAT G#{:X} MCU_STATUS G#{:X}\n", self.rd(GPU_STATUS), self.rd(GPU_FAULTSTATUS), self.rd(GPU_IRQ_RAWSTAT), self.rd(JOB_IRQ_RAWSTAT), self.rd(MMU_IRQ_RAWSTAT), self.rd(MCU_STATUS));
        for n in 0..2 {
            say!(self.grants, "gpu: AS{n} STATUS G#{:X} FAULTSTATUS G#{:X} FAULTADDRESS G#{:X}\n", self.rd(Gpu::as_reg(n, AS_STATUS)), self.rd(Gpu::as_reg(n, AS_FAULTSTATUS)), self.rd64(Gpu::as_reg(n, AS_FAULTADDRESS)));
        }
        exit(1);
    }
}

/// The shared interface as the CPU sees it: the MCU's VA G#0400_0000 lives at `pa` (one section, 64 KiB).
struct Shared {
    pa: usize,
}

impl Shared {
    fn at(&self, va: u32) -> usize {
        self.pa + (va as u64 - SHARED_VA) as usize
    }
    fn rd(&self, va: u32) -> u32 {
        rd32(self.at(va))
    }
    fn wr(&self, va: u32, v: u32) {
        wr32(self.at(va), v)
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let base = ferros_sys::dma(cap(grants, S_MEM)) as usize;
    let g = Gpu { regs, grants, f: freq() };
    // The input's first byte, the run's switches: bit 0 show, 1 the present bench, 2 the bulk buffers GPU-cached, 3 the snoop test, 4 ACE-Lite coherency.
    let in0 = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    let mut m = Mem::new(base);
    say!(grants, "gpu: GPU_ID G#{:X}, MCU_STATUS G#{:X}\n", g.rd(GPU_ID), g.rd(MCU_STATUS));

    // Interrupts masked and cleared, then the soft reset (kbase_pm_do_reset): RESET_COMPLETED polled.
    g.wr(GPU_IRQ_MASK, 0);
    g.wr(GPU_IRQ_CLEAR, IRQ_ALL);
    g.wr(JOB_IRQ_MASK, 0);
    g.wr(JOB_IRQ_CLEAR, 0xFFFF_FFFF);
    g.wr(MMU_IRQ_MASK, 0);
    g.wr(MMU_IRQ_CLEAR, 0xFFFF_FFFF);
    g.wr(GPU_IRQ_MASK, RESET_COMPLETED);
    let t0 = now();
    g.wr(GPU_COMMAND, 0x101);
    if !g.wait(500, |g| g.rd(GPU_IRQ_RAWSTAT) & RESET_COMPLETED != 0) {
        g.fail("soft reset never completed");
    }
    g.wr(GPU_IRQ_CLEAR, RESET_COMPLETED);
    say!(grants, "gpu: soft reset in {} us\n", ferros_sys::us_since(t0));

    // The quirk registers read back and written (kbase_pm_hw_issues_apply), L2_MMU_CONFIG's read and write limits cleared; no coherency; interrupts as kbase runs them.
    for r in [SHADER_CONFIG, TILER_CONFIG, CSF_CONFIG] {
        g.wr(r, g.rd(r));
    }
    g.wr(L2_MMU_CONFIG, g.rd(L2_MMU_CONFIG) & !0x0F00_0000);
    // No coherency (31), as Linux runs it; ACE-Lite I/O coherency (0) for the snoop test (input bit 4): COHERENCY_FEATURES reads G#1, ACE-Lite supported by the GPU, whether zuma's interconnect carries it is what the test asks.
    g.wr(COHERENCY_ENABLE, if in0 & 0x10 != 0 { 0 } else { 31 });
    g.wr(GPU_IRQ_CLEAR, IRQ_ALL);
    g.wr(GPU_IRQ_MASK, IRQ_ALL);
    g.wr(JOB_IRQ_CLEAR, 0xFFFF_FFFF);
    g.wr(JOB_IRQ_MASK, 0xFFFF_FFFF);
    g.wr(MMU_IRQ_CLEAR, 0xFFFF_FFFF);
    g.wr(MMU_IRQ_MASK, 0xFFFF);

    // The L2: Google's slice hash (zuma-gpu.dtsi l2-hash-values) with its enable bit, the allocation overrides zero, then powered (the MCU powers the shader cores itself).
    g.wr(ASN_HASH, 0x0036_DB6D);
    g.wr(ASN_HASH + 4, 0x005B_6DB6);
    g.wr(ASN_HASH + 8, 0);
    g.wr(L2_CONFIG, g.rd(L2_CONFIG) | 1 << 24);
    for k in 0..8 {
        g.wr(SYSC_ALLOC + 4 * k, 0);
    }
    g.wr64(L2_PWRON, g.rd64(L2_PRESENT));
    if !g.wait(64, |g| g.rd64(L2_PWRTRANS) == 0 && g.rd64(L2_READY) == g.rd64(L2_PRESENT)) {
        g.fail("L2 never powered");
    }
    say!(grants, "gpu: L2 ready (L2_CONFIG G#{:X})\n", g.rd(L2_CONFIG));

    // The firmware's sections into the MCU's address space (AS0): every interface-memory entry that is not protected, at its declared VA, with kbase's attributes on a non-coherent system.
    let Some(img) = fw::load(cap(grants, S_STORE), &mut m) else {
        say!(grants, "gpu: no gpu.fw in the vault\n");
        exit(1);
    };
    say!(grants, "gpu: image loaded: {} bytes at G#{:X}\n", img.len, img.at);
    if img.u32_at(0) != 0xC3F1_3A6E || img.bytes()[4] != 3 || img.bytes()[5] != 0 {
        say!(grants, "gpu: gpu.fw is not a CSF firmware image of format 0.3\n");
        exit(1);
    }
    say!(grants, "gpu: image header checked\n");
    let as0 = Table::new(&mut m);
    let mut shared = None;
    let end = img.u32_at(0x10) as usize;
    let mut at = 0x14;
    while at < end {
        let h = img.u32_at(at);
        let (ty, size) = (h & 0xFF, ((h >> 8) & 0xFF) as usize);
        if ty == 0 {
            let flags = img.u32_at(at + 4);
            let (v0, v1) = (img.u32_at(at + 8) as u64, img.u32_at(at + 12) as u64);
            let (d0, d1) = (img.u32_at(at + 16) as usize, img.u32_at(at + 20) as usize);
            // Bit 5 PROTECTED: skipped, as kbase skips it with no protected-memory allocator.
            if flags & 1 << 5 == 0 {
                let pages = ((v1 - v0) as usize).div_ceil(PAGE);
                let pa = m.alloc(pages);
                unsafe { core::ptr::copy_nonoverlapping(img.bytes()[d0..d1].as_ptr(), pa as *mut u8, d1 - d0) };
                // Cache mode bits 4:3: CACHED (1) is index 0; the coherent modes come down to non-cacheable (5) here, as NONE does.
                let attr = if (flags >> 3) & 3 == 1 { 0 } else { NC };
                let leaf = if flags & 2 != 0 { PAGE_RW } else { PAGE_RO } | attr | if flags & 4 != 0 { 0 } else { NX };
                as0.map(&mut m, v0, pa, pages, leaf, TABLE);
                if v0 == SHARED_VA {
                    shared = Some(Shared { pa });
                }
                say!(grants, "gpu: section G#{v0:X}-G#{v1:X} flags G#{flags:X}: {} bytes into {pages} pages at G#{pa:X}\n", d1 - d0);
            }
        }
        at += size;
    }
    let Some(sh) = shared else {
        say!(grants, "gpu: the firmware declares no shared interface at G#0400_0000\n");
        exit(1);
    };

    // A group's suspend buffer and one queue's user-IO pages in the MCU's address space too, before the MCU starts (no live update): the suspend buffer write-back like kbase's, the user-IO pages non-cacheable, input read-only, output read-write.
    let susp = m.alloc(SUSPEND_PAGES);
    as0.map(&mut m, SUSPEND_VA, susp, SUSPEND_PAGES, PAGE_RW | NX, TABLE);
    let uio = m.alloc(2);
    as0.map(&mut m, USER_IO_VA, uio, 1, PAGE_RO | NC | NX, TABLE);
    as0.map(&mut m, USER_IO_VA + PAGE as u64, uio + PAGE, 1, PAGE_RW | NC | NX, TABLE);
    if !g.install(0, as0.root) {
        g.fail("AS0 never went idle");
    }

    // The MCU started (AUTO), and the global interface's interrupt waited for: the firmware has booted.
    let t0 = now();
    g.wr(MCU_CONTROL, 2);
    if !g.wait(500, |g| g.rd(JOB_IRQ_RAWSTAT) & 1 << 31 != 0) {
        g.fail("the MCU never raised its global interface");
    }
    g.wr(JOB_IRQ_CLEAR, 1 << 31);
    let version = sh.rd(0x0400_0000);
    say!(grants, "gpu: MCU booted in {} us: MCU_STATUS G#{:X}, GLB_VERSION G#{version:X}, GLB_FEATURES G#{:X}, {} groups\n", ferros_sys::us_since(t0), g.rd(MCU_STATUS), sh.rd(0x0400_0004), sh.rd(0x0400_0010));
    if version == 0 {
        g.fail("GLB_VERSION is 0");
    }
    g.wr(GPU_IRQ_MASK, IRQ_MCU_UP);

    // The global interface's blocks, as the firmware reports them.
    let (gin, gout) = (sh.rd(0x0400_0008), sh.rd(0x0400_000C));
    let glb_req = |bits: u32| {
        // Toggle `bits` of GLB_REQ against GLB_ACK, then ring doorbell 0.
        let ack = sh.rd(gout);
        let req = sh.rd(gin);
        sh.wr(gin, (req & !bits) | ((ack ^ bits) & bits));
        g.doorbell(0);
    };
    let glb_done = |bits: u32, ms: u64| g.wait(ms, |_| (sh.rd(gin) ^ sh.rd(gout)) & bits == 0);

    // Global init (kbase global_init): the shader cores the firmware may use, the cores' power-off delay (800 us at 24.576 MHz), the progress timer, the eviction timer; the ack interrupts kbase asks for.
    let cores = g.rd64(SHADER_PRESENT);
    sh.wr(gin + 0x18, cores as u32);
    sh.wr(gin + 0x1C, (cores >> 32) as u32);
    sh.wr(gin + 0x14, 0x4CCC);
    sh.wr(gin + 0x88, 1);
    sh.wr(gin + 0x10, 0x27_1000);
    sh.wr(gin + 0x90, 0x05DC_0000);
    sh.wr(gin + 0x94, 1);
    sh.wr(gin + 0x04, 0x4C81_0F1E);
    glb_req(1 << 2 | 1 << 3 | 1 << 1 | 1 << 16);
    if !glb_done(1 << 2 | 1 << 3 | 1 << 1 | 1 << 16, 100) {
        g.fail("global init never acknowledged");
    }
    let t0 = now();
    glb_req(1 << 8);
    if !glb_done(1 << 8, 100) {
        g.fail("ping never answered");
    }
    say!(grants, "gpu: global init acknowledged; ping answered in {} us\n", ferros_sys::us_since(t0));

    // The group's address space (1): a 4 KiB ring and the word the job writes, both non-cacheable so the CPU and the GPU agree without cache maintenance.
    let as1 = Table::new(&mut m);
    let ring = m.alloc(1);
    let dst = m.alloc(1);
    as1.map(&mut m, RING_VA, ring, 1, PAGE_RW | NC | NX, TABLE);
    as1.map(&mut m, DST_VA, dst, 1, PAGE_RW | NC | NX, TABLE);
    // The compute job's pages: the shader non-cacheable and executable (the one page without execute-never), the descriptors and uniforms, the output.
    let code = m.alloc(1);
    let desc = m.alloc(1);
    let out = m.alloc(1);
    as1.map(&mut m, CODE_VA, code, 1, PAGE_RO | NC, TABLE);
    as1.map(&mut m, DESC_VA, desc, 1, PAGE_RW | NC | NX, TABLE);
    as1.map(&mut m, OUT_VA, out, 1, PAGE_RW | NC | NX, TABLE);
    let big = m.alloc(16);
    as1.map(&mut m, BIG_VA, big, 16, PAGE_RW | NC | NX, TABLE);
    // The picture a show renders (input bit 0): 1024 x 2048 escape counts, 8 MiB. Mapped with the rest before the address space is installed: the GPU caches its page-table walks in the L2, so a page mapped after it could meet a stale table line.
    let img = m.alloc(IMG_W * IMG_H * 4 / PAGE);
    // Input bit 2: the picture and the bench's buffers cached on the GPU side (MEMATTR index 0, write-back; every compute stream ends with an L2 clean), else non-cacheable (index 5).
    let bulk = if in0 & 4 != 0 { 0 } else { NC };
    as1.map(&mut m, IMG_VA, img, IMG_W * IMG_H * 4 / PAGE, PAGE_RW | bulk | NX, TABLE);
    // The present-stage bench's output (input bit 1): a 32-bit word for each of the 1M 64-bit pixels the picture's 8 MiB holds as its input.
    let bench = m.alloc(BENCH_PX * 4 / PAGE);
    as1.map(&mut m, BENCH_VA, bench, BENCH_PX * 4 / PAGE, PAGE_RW | bulk | NX, TABLE);
    // The cacheable page: 30 MiB into gpu-mem, above anything the allocator hands out. Seen non-cacheable (index 5) and coherent (index 6, the shared type, shareable both ways: how kbase maps coherent memory).
    let snoop = base + (30 << 20);
    let res = m.alloc(1);
    as1.map(&mut m, SNOOP_NC_VA, snoop, 1, PAGE_RW | NC | NX, TABLE);
    as1.map(&mut m, SNOOP_CO_VA, snoop, 1, PAGE_RW | 6 << 2 | 2 << 8 | NX, TABLE);
    as1.map(&mut m, RES_VA, res, 1, PAGE_RW | NC | NX, TABLE);
    // The present-stage test (input bit 5): 64 rows of the panel's width as a canvas, the words the kernel writes, and its table buffer (the code table, 64 Ki u16, then five 32 x 32 threshold slabs), GPU-cacheable: written once before any job reads it.
    let canvas = m.alloc(PRESENT_W * PRESENT_H * 8 / PAGE);
    let words_out = m.alloc(PRESENT_W * PRESENT_H * 4 / PAGE);
    let tab = m.alloc(TAB_BYTES.div_ceil(PAGE));
    as1.map(&mut m, CANV_VA, canvas, PRESENT_W * PRESENT_H * 8 / PAGE, PAGE_RW | NC | NX, TABLE);
    as1.map(&mut m, WORDS_VA, words_out, PRESENT_W * PRESENT_H * 4 / PAGE, PAGE_RW | NC | NX, TABLE);
    as1.map(&mut m, TAB_VA, tab, TAB_BYTES.div_ceil(PAGE), PAGE_RW | NX, TABLE);
    if !g.install(1, as1.root) {
        g.fail("AS1 never went idle");
    }

    // Group 0, stream 0, its blocks from the interface; the suspend size the firmware wants checked against what was mapped.
    let gctl = 0x0400_1000u32;
    let (cin, cout) = (sh.rd(gctl + 4), sh.rd(gctl + 8));
    let (suspend, streams) = (sh.rd(gctl + 0xC), sh.rd(gctl + 0x14));
    let (sin, sout, sfeat) = (sh.rd(gctl + 0x40 + 4), sh.rd(gctl + 0x40 + 8), sh.rd(gctl + 0x40));
    say!(grants, "gpu: group 0: input G#{cin:X} output G#{cout:X}, suspend {suspend} bytes, {streams} streams; stream 0: input G#{sin:X} output G#{sout:X} features G#{sfeat:X}\n");
    if suspend as usize > SUSPEND_PAGES * PAGE {
        say!(grants, "gpu: the group's suspend buffer needs {suspend} bytes, {} mapped\n", SUSPEND_PAGES * PAGE);
        exit(1);
    }

    // The queue (kbase program_cs): its ring, its user-IO pages, user doorbell 1, priority 0, every ack interrupt; STATE START with the idle notifications.
    let doorbell = 1u32;
    sh.wr(sin + 0x10, RING_VA as u32);
    sh.wr(sin + 0x14, (RING_VA >> 32) as u32);
    sh.wr(sin + 0x18, PAGE as u32);
    sh.wr(sin + 0x30, USER_IO_VA as u32);
    sh.wr(sin + 0x34, (USER_IO_VA >> 32) as u32);
    sh.wr(sin + 0x38, (USER_IO_VA + PAGE as u64) as u32);
    sh.wr(sin + 0x3C, ((USER_IO_VA + PAGE as u64) >> 32) as u32);
    sh.wr(sin + 0x04, doorbell << 8);
    sh.wr(sin + 0x0C, 0xFFFF_FFFF);
    let cs_ack = sh.rd(sout);
    let keep = 1 << 27 | 1 << 30 | 1 << 31;
    sh.wr(sin, (cs_ack & keep) | 1 << 10 | 1 << 8 | 1 << 12 | 1);
    unsafe { core::arch::asm!("dmb osh") };
    sh.wr(cin + 0x08, (sh.rd(cin + 0x08) & !1) | ((sh.rd(cout + 0x08) ^ 1) & 1));

    // The group (kbase program_csg_slot): every core for compute and fragment, the tiler, seven compute endpoints and one tiler endpoint at priority 15, address space 1, its suspend buffer; STATE START, then the group's bit of GLB_DB_REQ toggled and doorbell 0.
    sh.wr(cin + 0x20, cores as u32);
    sh.wr(cin + 0x24, (cores >> 32) as u32);
    sh.wr(cin + 0x28, cores as u32);
    sh.wr(cin + 0x2C, (cores >> 32) as u32);
    sh.wr(cin + 0x30, g.rd64(TILER_PRESENT) as u32);
    sh.wr(cin + 0x34, cores.count_ones() | cores.count_ones() << 8 | 1 << 16 | 15 << 28);
    sh.wr(cin + 0x38, 0);
    sh.wr(cin + 0x50, 1);
    sh.wr(cin + 0x40, SUSPEND_VA as u32);
    sh.wr(cin + 0x44, (SUSPEND_VA >> 32) as u32);
    sh.wr(cin + 0x48, 0);
    sh.wr(cin + 0x4C, 0);
    sh.wr(cin + 0x04, 0xFFFF_FFFF);
    sh.wr(cin, (sh.rd(cin) & !7) | 1);
    unsafe { core::arch::asm!("dmb osh") };
    sh.wr(gin + 0x08, (sh.rd(gin + 0x08) & !1) | ((sh.rd(gout + 0x08) ^ 1) & 1));
    g.doorbell(0);
    if !g.wait(100, |_| sh.rd(cout) & 7 == 1 && sh.rd(sout) & 7 == 1) {
        say!(grants, "gpu: group start: CSG_ACK G#{:X} CS_ACK G#{:X} CS_FAULT G#{:X} CS_FATAL G#{:X}\n", sh.rd(cout), sh.rd(sout), sh.rd(sout + 0x80), sh.rd(sout + 0x84));
        g.fail("the group never started");
    }
    say!(grants, "gpu: group 0 and its queue started\n");

    // The jobs, each appended to the ring after the last and the insert pointer moved on (the ring is 4 KiB: these few jobs never wrap it).
    let q = Queue { ring, uio, insert: core::cell::Cell::new(0), doorbell: doorbell as usize };
    // A job's completion is waited for on the GPU's job line (`gpu-job`): each interrupt is cleared at the GPU, group 0's events acknowledged as kbase acknowledges them (the IDLE, SYNC_UPDATE and progress-timer bits of CSG_ACK copied into CSG_REQ, CSG_IRQ_ACK = CSG_IRQ_REQ), the line unmasked, and the job's own check asked; 100 ms at most.
    let irq = cap(grants, S_IRQ);
    let events = |g: &Gpu| -> u32 {
        let raw = g.rd(JOB_IRQ_RAWSTAT);
        g.wr(JOB_IRQ_CLEAR, raw);
        if raw & 1 != 0 {
            let mask = 1 << 28 | 1 << 29 | 1 << 31;
            sh.wr(cin, (sh.rd(cin) & !mask) | (sh.rd(cout) & mask));
            sh.wr(cin + 0x0C, sh.rd(cout + 0x0C));
        }
        ferros_sys::irq_ack(irq);
        raw
    };
    let run_for = |name: &str, words: &[u64], check: &dyn Fn() -> bool, ms: u64| -> bool {
        let t0 = now();
        q.submit(&g, words);
        let deadline = t0 + g.f / 1000 * ms;
        let mut wakes = 0;
        while !check() && now() < deadline {
            if ferros_sys::irq_wait(irq, deadline) == 0 {
                wakes += 1;
                events(&g);
            }
        }
        let ok = check();
        say!(grants, "gpu: job {name}: {} in {} us after {wakes} interrupts; CS_FAULT G#{:X}, CS_FATAL G#{:X}; CSG_REQ G#{:X} CSG_ACK G#{:X}\n", if ok { "done" } else { "did not finish" }, ferros_sys::us_since(t0), sh.rd(sout + 0x80), sh.rd(sout + 0x84), sh.rd(cin), sh.rd(cout));
        ok
    };
    let run = |name: &str, words: &[u64], check: &dyn Fn() -> bool| run_for(name, words, check, 100);
    let mut ok = true;
    // 1: STORE_MULTIPLE r4..r5 to [r0:r1 + 4] (Mesa genxml v11 CS STORE_MULTIPLE: offset 15:0, register mask 31:16, address pair 47:40, base register 55:48), then WAIT on every scoreboard slot so the stores are done before the queue moves on.
    wr32(dst + 4, 0);
    wr32(dst + 8, 0);
    ok &= run("store_multiple", &[mv48(0, DST_VA), mv32(4, 0x5EED_0001), mv32(5, 0x5EED_0002), 0x15 << 56 | 4 << 48 | 0 << 40 | 0b11 << 16 | 4, 0x03 << 56 | 0xFFFF << 16], &|| rd32(dst + 4) == 0x5EED_0001 && rd32(dst + 8) == 0x5EED_0002);
    // 0: MOVE48 r0:r1 = the destination, MOVE32 r2 = G#CAFE_F00D, SYNC_SET32 [r0:r1] = r2 (encodings kbase itself decodes, mali_kbase_csf_sync.c).
    wr32(dst, 0);
    ok &= run("sync_set", &[mv48(0, DST_VA), mv32(2, 0xCAFE_F00D), 0x26 << 56 | 0 << 40 | 2 << 32], &|| rd32(dst) == 0xCAFE_F00D);
    // 2: compute (GPU.md "Compute"; the words from Mesa's v11 genxml, Valhall ISA.xml and its packer, cross-checked against Mesa's assembler test vectors): sixteen invocations in one workgroup, each storing G#C0FF_EE00 + its global id to out[id]; then the done flag. The shader, in Valhall:
    //   IADD_IMM.i32 r0, r60, #G#C0FFEE00        (r60: the global id X, preloaded)
    //   SHADDX.u64 [r2:r3], u0, [r60:r61].w0, shift:2   (u0: the output's address, the first uniform)
    //   STORE.i32.slot0.end @r0, [r2:r3], offset:0
    let cx = Compute { code, desc, code_va: CODE_VA, desc_va: DESC_VA, max_threads: g.rd(THREAD_MAX_THREADS) };
    let done = OUT_VA + 0x200;
    for i in 0..32 {
        wr32(out + 4 * i, 0);
    }
    let mut words = [0u64; 32];
    let n = cx.prepare(&[0x0110_C0C0_FFEE_003C, 0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002], &[OUT_VA], 1, done, &mut words);
    let all = || (0..16).all(|i| rd32(out + 4 * i) == 0xC0FF_EE00 + i as u32) && rd32(out + 0x200) == 1;
    ok &= run("compute", &words[..n], &all);
    say!(grants, "gpu: compute output: G#{:X} G#{:X} G#{:X} … G#{:X}, done flag {}\n", rd32(out), rd32(out + 4), rd32(out + 8), rd32(out + 60), rd32(out + 0x200));
    // 3: the same shader over 1024 workgroups (16384 invocations, 64 KiB written), the task split along X by the cores' thread limit.
    for i in 0..16384 {
        wr32(big + 4 * i, 0);
    }
    wr32(out + 0x200, 0);
    let n = cx.prepare(&[0x0110_C0C0_FFEE_003C, 0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002], &[BIG_VA], 1024, done, &mut words);
    let all_big = || rd32(out + 0x200) == 1 && (0..16384).all(|i| rd32(big + 4 * i) == 0xC0FF_EE00 + i as u32);
    ok &= run("compute 16384", &words[..n], &all_big);
    say!(grants, "gpu: THREAD_MAX_THREADS {}; the last word G#{:X}\n", cx.max_threads, rd32(big + 4 * 16383));
    // 4: a real kernel, the Mandelbrot set's escape count over a 64 x 64 image, one invocation a pixel (hand-encoded Valhall: shifts, int-to-float, FMA, float and int compares, a divergent loop closed by BRANCHZ.reconverge; GPU.md "Compute"). Uniforms: u0 the output, u1 (log2 width, max_iter), u2 (x0, y0), u3 (step, 0). Checked pixel by pixel against the same arithmetic on the CPU in single precision.
    const MANDEL: [u64; 26] = [
        0x00B4_CD00_11C0_823C, 0x00B4_CC01_11C0_820D, 0x00A8_CC00_0001_0C3C, 0x0090_CC00_0019_000C, 0x0090_CD00_0019_000D, 0x0091_CA00_0000_0086, 0x00B2_C600_0084_0A0C, 0x00B2_C700_0085_0A0D,
        0x0110_C440_8000_00C0, 0x0110_C000_0000_00C0, 0x0110_C100_0000_00C0, 0x5110_C500_0000_00C0, 0x00B2_C804_00C0_0101, 0x00B2_C904_00C0_0505, 0x00A4_CA00_0000_0908, 0x00F0_CB03_80C0_8300,
        0x00F4_CC05_80C0_040A, 0x00B4_CB01_100C_C00B, 0x00A4_CA00_0000_0101, 0x00B2_C500_0007_050A, 0x00A4_CA10_0000_0908, 0x00A4_C100_0000_060A, 0x00A8_C000_0001_0B00, 0x501F_C007_FFFF_F40B,
        0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002,
    ];
    let (shift, max_iter, x0, y0, step) = (6u32, 64u32, -2.0f32, -1.25f32, 0.0390625f32);
    for i in 0..4096 {
        wr32(big + 4 * i, 0xFFFF_FFFF);
    }
    wr32(out + 0x200, 0);
    let u = [BIG_VA, shift as u64 | (max_iter as u64) << 32, x0.to_bits() as u64 | (y0.to_bits() as u64) << 32, step.to_bits() as u64];
    let n = cx.prepare(&MANDEL, &u, 4096 / 16, done, &mut words);
    ok &= run("mandelbrot", &words[..n], &|| rd32(out + 0x200) == 1);
    let reference = |gid: u32| -> u32 {
        let py = gid >> shift;
        let px = gid - (py << shift);
        let (cr, ci) = (px as f32 * step + x0, py as f32 * step + y0);
        let (mut zx, mut zy, mut k) = (0f32, 0f32, 0u32);
        loop {
            let (x2, y2) = (zx * zx, zy * zy);
            if k == max_iter || !(x2 + y2 <= 4.0) {
                return k;
            }
            zy = (zx + zx) * zy + ci;
            zx = (x2 - y2) + cr;
            k += 1;
        }
    };
    let (mut sum, mut full, mut wrong, mut first_wrong) = (0u32, 0u32, 0u32, None);
    for gid in 0..4096u32 {
        let got = rd32(big + 4 * gid as usize);
        sum += got;
        full += (got == max_iter) as u32;
        if got != reference(gid) {
            wrong += 1;
            first_wrong.get_or_insert((gid, got, reference(gid)));
        }
    }
    say!(grants, "gpu: mandelbrot 64x64: sum {sum} (expect 83000), {full} pixels at {max_iter} (expect 1038), {wrong} differ from the CPU{}\n", match first_wrong { Some((g, a, b)) => alloc_free_fmt(g, a, b), None => Tail([0; 48], 0) });
    ok &= wrong == 0;
    say!(grants, "gpu: the target reads G#{:X} G#{:X} G#{:X}; {} KiB of gpu-mem used\n", rd32(dst), rd32(dst + 4), rd32(dst + 8), m.used() >> 10);
    // Input bit 3: does the GPU see the CPU's caches (ACE-Lite I/O coherency, bit 4), so the canvas need not be cleaned or copied? Four cases over 16 words of the CPU-cacheable page, each read back by the CPU from its own caches or from a non-cacheable page.
    if in0 & 8 != 0 {
        let mode = if in0 & 0x10 != 0 { "ACE-Lite" } else { "none" };
        let copy = [0x01A3_C200_2027_3C80u64, 0x0860_8482_1800_0002, 0x01A3_C200_2027_3C82, 0x7861_4402_1800_0002];
        let w = |a: usize, v: u32| unsafe { core::ptr::write_volatile(a as *mut u32, v) };
        let r = |a: usize| unsafe { core::ptr::read_volatile(a as *const u32) };
        let count = |f: &dyn Fn(usize) -> bool| (0..16).filter(|&i| f(i)).count();
        // CPU to GPU: the CPU writes a pattern through its caches (no clean: EL0 cannot), the GPU copies it to a non-cacheable page; how many words arrived.
        for (name, va, base_v) in [("non-cacheable", SNOOP_NC_VA, 0xA000_0000u32), ("coherent", SNOOP_CO_VA, 0xB000_0000)] {
            for i in 0..16 {
                w(snoop + 4 * i, base_v + i as u32);
                wr32(res + 4 * i, 0);
            }
            let _ = (0..16).map(|i| r(snoop + 4 * i)).sum::<u32>();
            wr32(out + 0x200, 0);
            let n = cx.prepare(&copy, &[va, RES_VA], 1, done, &mut words);
            run_for("snoop read", &words[..n], &|| rd32(out + 0x200) == 1, 200);
            say!(grants, "gpu: coherency {mode}: CPU wrote, GPU read through the {name} view: {} of 16 words new (first G#{:X})\n", count(&|i| rd32(res + 4 * i) == base_v + i as u32), rd32(res));
        }
        // GPU to CPU: the CPU reads the page into its caches, the GPU writes it, the CPU reads it again from its caches; how many words it sees new.
        for (name, va, imm_word) in [("non-cacheable", SNOOP_NC_VA, 0x0110_C0D0_D000_003Cu64), ("coherent", SNOOP_CO_VA, 0x0110_C0C0_FFEE_003C)] {
            let base_v = if va == SNOOP_NC_VA { 0xD0D0_0000u32 } else { 0xC0FF_EE00 };
            for i in 0..16 {
                w(snoop + 4 * i, 0x1111_0000 + i as u32);
            }
            let _ = (0..16).map(|i| r(snoop + 4 * i)).sum::<u32>();
            wr32(out + 0x200, 0);
            let n = cx.prepare(&[imm_word, 0x01A3_C200_2027_3C80, 0x7861_4002_1800_0002], &[va], 1, done, &mut words);
            run_for("snoop write", &words[..n], &|| rd32(out + 0x200) == 1, 200);
            say!(grants, "gpu: coherency {mode}: GPU wrote through the {name} view, CPU read from its caches: {} of 16 words new (first G#{:X})\n", count(&|i| r(snoop + 4 * i) == base_v + i as u32), r(snoop));
        }
    }
    // Input bit 5: the present stage on the GPU (GPU.md "The present stage"; the kernel hand-encoded from the display's neon.rs, its words and sources in ferros-present-spec.md on Harbor), checked against the display's own NEON path compiled in here: 64 full rows, the four temporal phases, every word compared (the owner: close is fine, each channel within one code). 512 rows, so the dispatch's fixed cost does not hide the per-pixel one.
    if in0 & 0x20 != 0 {
        const PRESENT: [u64; 92] = [
            0x00A0_C000_000A_883D,
            0x00A0_C000_0000_3C00,
            0x01A3_C200_2037_0080,
            0x0060_84F4_2800_0002,
            0x00B4_C601_108A_C03C,
            0x00B4_C701_148B_C93D,
            0x00B4_C601_1107_C006,
            0x01A3_C800_2007_0686,
            0x1860_81E2_4000_0008,
            0x0090_C600_0014_0004,
            0x0090_C700_1014_0004,
            0x0090_C800_0014_0005,
            0x0090_C600_0019_0006,
            0x00B2_C204_00C0_8C06,
            0x00B2_C304_00C0_0202,
            0x00A4_C400_0000_8E03,
            0x00B2_C400_008F_0403,
            0x00B2_C400_0090_0403,
            0x00B2_C500_0093_9203,
            0x00B2_C500_0091_0503,
            0x00B2_C500_0090_0503,
            0x00B2_C404_00C0_0402,
            0x009C_C300_0000_0005,
            0x00B2_C204_00C0_0304,
            0x00B2_C440_0004_0502,
            0x00B2_C200_0002_0304,
            0x00B2_C204_00C0_0202,
            0x00B2_C204_00C0_8D02,
            0x00A4_4C10_0000_C002,
            0x0090_C700_0019_0007,
            0x00B2_C204_00C0_8C07,
            0x00B2_C304_00C0_0202,
            0x00A4_C400_0000_8E03,
            0x00B2_C400_008F_0403,
            0x00B2_C400_0090_0403,
            0x00B2_C500_0093_9203,
            0x00B2_C500_0091_0503,
            0x00B2_C500_0090_0503,
            0x00B2_C404_00C0_0402,
            0x009C_C300_0000_0005,
            0x00B2_C204_00C0_0304,
            0x00B2_C440_0004_0502,
            0x00B2_C200_0002_0304,
            0x00B2_C204_00C0_0202,
            0x00B2_C204_00C0_8D02,
            0x00A4_4D10_0000_C002,
            0x0090_C800_0019_0008,
            0x00B2_C204_00C0_8C08,
            0x00B2_C304_00C0_0202,
            0x00A4_C400_0000_8E03,
            0x00B2_C400_008F_0403,
            0x00B2_C400_0090_0403,
            0x00B2_C500_0093_9203,
            0x00B2_C500_0091_0503,
            0x00B2_C500_0090_0503,
            0x00B2_C404_00C0_0402,
            0x009C_C300_0000_0005,
            0x00B2_C204_00C0_0304,
            0x00B2_C440_0004_0502,
            0x00B2_C200_0002_0304,
            0x00B2_C204_00C0_0202,
            0x00B2_C204_00C0_8D02,
            0x00A4_4E10_0000_C002,
            0x00A1_C900_0005_0E0C,
            0x00A5_CA10_2000_0D09,
            0x00B3_CB04_2AC0_9409,
            0x00B3_CB00_0A0B_950A,
            0x00B3_CB00_3A0B_960A,
            0x00B3_CC04_0AC0_970D,
            0x00B3_CC00_0A0C_980A,
            0x00B3_CC00_3A0C_990A,
            0x0090_C600_0014_000B,
            0x0090_C800_0014_000C,
            0x0090_CA00_1014_000B,
            0x01A3_C200_2017_0684,
            0x01A3_C400_2017_0884,
            0x01A3_CC00_2017_0A84,
            0x0060_86A2_0800_0002,
            0x0060_88A2_4800_0004,
            0x3860_8AA2_8800_000C,
            0x00A0_C600_0000_0106,
            0x00A0_C800_0000_0108,
            0x00A0_CA00_0000_010A,
            0x00B4_C600_19C0_C906,
            0x00B4_C800_19C0_C908,
            0x00B4_CA00_19C0_C90A,
            0x00B4_CA01_1908_CA0A,
            0x00B4_CA01_1906_CA0A,
            0x00B4_CA01_11C0_C90A,
            0x0110_CA00_0000_030A,
            0x01A3_C200_2027_0082,
            0x7861_4A02_1800_0002,
        ];
        // The display's dither constants (payloads/display main.rs BAYER, TEMPORAL_RANK).
        const BAYER: [[u16; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        const TEMPORAL_RANK: [u32; 4] = [0, 2, 1, 3];
        let tabs = odt::odt_tables(0.5, 1.0);
        let ob = unsafe { core::slice::from_raw_parts(&tabs as *const neon::Odt as *const u8, core::mem::size_of::<neon::Odt>()) };
        let u16_at = |o: usize| ob[o] as u32 | (ob[o + 1] as u32) << 8;
        let f32_bits = |o: usize| u32::from_le_bytes([ob[o], ob[o + 1], ob[o + 2], ob[o + 3]]) as u64;
        // The code table: every f16 bit pattern's code, as code16 computes it, capped at 16320 (the CPU's saturating narrow holds the 8-bit code at 255 there; the shader shifts without saturating, and 16384 would carry into the next channel's bit: proven exact over every code and threshold in the spec).
        for h in 0..65536usize {
            let e = h >> 10;
            let (m6, m4) = ((h >> 4) & 63, (h & 15) as u32);
            let s = if e < 32 { ob[224 + e] as u32 | (ob[192 + e] as u32) << 8 } else { 0 };
            let n = ob[64 + m6] as u32 | (ob[m6] as u32) << 8;
            let f = (n + ((ob[128 + m6] as u32 * m4 + 8) >> 4)) & 0xFFFF;
            let code = ((f * s + 32768) >> 16).min(16320);
            let a = tab + 2 * h;
            unsafe { core::ptr::write_volatile(a as *mut u16, code as u16) };
        }
        // The threshold slabs: phases 0-3, and the spatial-only one.
        for s in 0..5usize {
            for r in 0..32usize {
                for c in 0..32usize {
                    let t = BAYER[r & 3][c & 3] as u32 * 4 + if s < 4 { TEMPORAL_RANK[(s + (c >> 2) + (r >> 2)) & 3] } else { 0 };
                    unsafe { core::ptr::write_volatile((tab + 131072 + 1024 * s + 32 * r + c) as *mut u8, t as u8) };
                }
            }
        }
        // The canvas: a mix of black, white, mid greys and random channels to 4x white, so every part of the curve is crossed.
        let mut rng = 0x2545_F491_4F6C_DD1Du64;
        for i in 0..PRESENT_W * PRESENT_H {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let px = match i % 11 {
                0 => 0,
                1 => 0x4000 | 0x4000 << 16 | 0x4000 << 32,
                2 => 0x2000 | 0x2000 << 16 | 0x2000 << 32,
                _ => rng & 0x0000_FFFF_FFFF_FFFF,
            };
            wr64(canvas + 8 * i, px);
        }
        let (c0, c1) = (|i: usize| u16_at(256 + 2 * i) as u64, u16_at(272) as u64);
        let mut worst = (0u32, 0u32);
        for k in 0..4usize {
            let u = [
                CANV_VA,
                WORDS_VA,
                TAB_VA,
                TAB_VA + 131072 + 1024 * k as u64,
                PRESENT_W as u64 | ((PRESENT_W as f32).to_bits() as u64) << 32,
                0xF | 0x1E0 << 32,
                f32_bits(288) | f32_bits(292) << 32,
                378f32.to_bits() as u64 | (17325f32.to_bits() as u64) << 32,
                135135f32.to_bits() as u64 | (62370f32.to_bits() as u64) << 32,
                28f32.to_bits() as u64 | (3150f32.to_bits() as u64) << 32,
                (c0(0) | c0(2) << 16) | (c0(3) | c0(7) << 16) << 32,
                (c0(4) | c1 << 16) | (c0(1) | c0(1) << 16) << 32,
                (c0(5) | c0(5) << 16) | (c0(6) | c0(6) << 16) << 32,
                0x3800_0000 | 1 << 32,
            ];
            for i in 0..PRESENT_W * PRESENT_H {
                wr32(words_out + 4 * i, 0);
            }
            wr32(out + 0x200, 0);
            let n = cx.prepare_grid(&PRESENT, &u, ((PRESENT_W / 16) as u32, PRESENT_H as u32), 0x3000, done, &mut words);
            let t0 = now();
            let finished = run_for("present", &words[..n], &|| rd32(out + 0x200) == 1, 500);
            let us = ferros_sys::us_since(t0);
            // The NEON path on the same rows, and the comparison.
            let (mut exact, mut close, mut fixed_bad, mut first) = (0usize, 0usize, 0usize, None);
            let mut row = [0u32; PRESENT_W];
            for y in 0..PRESENT_H {
                let th = neon::thresholds(y, k, &BAYER, &TEMPORAL_RANK);
                unsafe {
                    let thv = core::arch::aarch64::vld1q_u8(th.as_ptr());
                    for x in (0..PRESENT_W).step_by(16) {
                        neon::sixteen_canvas(&tabs, (canvas + 8 * (y * PRESENT_W + x)) as *const u64, row.as_mut_ptr().add(x), thv);
                    }
                }
                for x in 0..PRESENT_W {
                    let (g, c) = (rd32(words_out + 4 * (y * PRESENT_W + x)), row[x]);
                    let ch = |w: u32, s: u32| (w >> s) & 255;
                    let d = [4u32, 14, 24].map(|s| ch(g, s).abs_diff(ch(c, s)));
                    let dmax = d[0].max(d[1]).max(d[2]);
                    exact += (g == c) as usize;
                    close += (g & 0x00C0_300F == 3 && dmax <= 1) as usize;
                    fixed_bad += (g & 0x00C0_300F != 3) as usize;
                    if g != c && first.is_none() {
                        first = Some((x, y, g, c));
                    }
                    if dmax > worst.0 {
                        worst = (dmax, g);
                    }
                }
            }
            let total = PRESENT_W * PRESENT_H;
            match first {
                Some((x, y, g, c)) => say!(grants, "gpu: present phase {k}: {} in {us} us; {exact} of {total} exact, {close} within one code, {fixed_bad} with bad fixed bits; first difference at ({x}, {y}): GPU G#{g:X}, NEON G#{c:X}\n", if finished { "done" } else { "not finished" }),
                None => say!(grants, "gpu: present phase {k}: {} in {us} us; all {total} words exact\n", if finished { "done" } else { "not finished" }),
            }
        }
        say!(grants, "gpu: present: the largest channel difference {} (in a word G#{:X})\n", worst.0, worst.1);
    }
    // Input bit 1: the present-stage bench (fluor on the GPU, measured before written): a proxy with the display's present stage's memory shape, a 64-bit pixel in and a 32-bit word out, with 0, 24 or 48 FMAs between, over 1M pixels; and the same at 16 invocations for the fixed cost of a dispatch. The kernel:
    //   SHADDX.u64 [r2:r3], u0, [r60:r61].w0, shift:3      (the pixel's address)
    //   LOAD.i64.unsigned.slot0.wait0 @r4:r5, [r2:r3]   (the wait in the flow field acts after its instruction, before the next: on the consumer it came one too late, 2026-11-05)
    //   U32_TO_F32 r6, r4
    //   FMA.f32 r6, r6, r6, r6   × n
    //   SHADDX.u64 [r2:r3], u1, [r60:r61].w0, shift:2      (the word's address)
    //   STORE.i32.slot0.end @r6, [r2:r3]
    let bench_mode = input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } & 2 == 2;
    if bench_mode {
        for i in 0..BENCH_PX {
            wr64(img + 8 * (i % (IMG_W * IMG_H / 2)), i as u64);
        }
        // The load alone, its word stored back raw: three forms of LOAD.i32 against the test vector's (`LOAD.i32.unsigned.slot0.wait0 @r0, [r0^:r1^]` = G#0860_8082_1800_0040).
        let seqs: [(&str, &[u64]); 4] = [
            ("load, SHADDX.wait0, store r4", &[0x01A3_C200_2037_3C80, 0x0060_84F4_2800_0002, 0x09A3_C200_2027_3C82, 0x7861_4402_1800_0002]),
            ("load, MOV.wait0 r6, store r6", &[0x01A3_C200_2037_3C80, 0x0060_84F4_2800_0002, 0x0891_C600_0000_0004, 0x01A3_C200_2027_3C82, 0x7861_4602_1800_0002]),
            ("load, CVT.wait0 r6, store r6", &[0x01A3_C200_2037_3C80, 0x0060_84F4_2800_0002, 0x0890_C600_0019_0004, 0x01A3_C200_2027_3C82, 0x7861_4602_1800_0002]),
            ("load, SHADDX.wait0, CVT r6, store r6", &[0x01A3_C200_2037_3C80, 0x0060_84F4_2800_0002, 0x09A3_C200_2027_3C82, 0x0090_C600_0019_0004, 0x7861_4602_1800_0002]),
        ];
        for (name, code) in seqs {
            for i in 0..16 {
                wr32(bench + 4 * i, 0xDEAD_0000);
            }
            wr32(out + 0x200, 0);
            let n = cx.prepare(code, &[IMG_VA, BENCH_VA], 1, done, &mut words);
            run_for("raw load", &words[..n], &|| rd32(out + 0x200) == 1, 200);
            say!(grants, "gpu: raw load {name}: G#{:X} G#{:X} G#{:X} (the pixels hold 0, 1, 2); AS1 FAULTSTATUS G#{:X} FAULTADDRESS G#{:X}\n", rd32(bench), rd32(bench + 4), rd32(bench + 8), g.rd(Gpu::as_reg(1, AS_FAULTSTATUS)), g.rd64(Gpu::as_reg(1, AS_FAULTADDRESS)));
        }
        for (fmas, px) in [(0usize, 16usize), (0, BENCH_PX), (24, BENCH_PX), (48, BENCH_PX)] {
            let mut code = [0u64; 64];
            let mut k = 0;
            for w in [0x01A3_C200_2037_3C80u64, 0x0860_84F4_2800_0002, 0x0090_C600_0019_0004] {
                code[k] = w;
                k += 1;
            }
            for _ in 0..fmas {
                code[k] = 0x00B2_C600_0006_0606;
                k += 1;
            }
            code[k] = 0x01A3_C200_2027_3C82;
            code[k + 1] = 0x7861_4602_1800_0002;
            k += 2;
            wr32(out + 0x200, 0);
            let n = cx.prepare(&code[..k], &[IMG_VA, BENCH_VA], (px / 16) as u32, done, &mut words);
            let t0 = now();
            let finished = run_for("bench", &words[..n], &|| rd32(out + 0x200) == 1, 2000);
            let us = ferros_sys::us_since(t0);
            say!(grants, "gpu: bench: {px} pixels, {fmas} FMAs a pixel: {} in {us} us ({} ns a pixel); word 0 G#{:X}\n", if finished { "done" } else { "not finished" }, us * 1000 / px as u64, rd32(bench));
        }
    }
    // Input bit 0: show it (tools/husky-gpu.sh show). The GPU renders the Mandelbrot set at 1024 x 2048 (up to 256 iterations, real -2.2 to 0.8), the CPU turns the counts into grey on a surface (black in the set, brighter the faster a point escapes: gamma 2, the display's working values), and the display shows it at the system level, over everything, for 60 s. Grant slots for it: 5 use display, 6 a surface.
    let show = input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } & 1 == 1;
    if show {
        let display = cap(grants, 5);
        let surf = ferros_sys::dma(cap(grants, 6)) as usize;
        let mut r = [0u8; 256];
        if ferros_sys::call(display, &[5], &mut r) < 4 || surf == 0 || surf == usize::MAX {
            say!(grants, "gpu: show: no display server or no surface\n");
            exit(1);
        }
        let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as usize, u16::from_le_bytes([r[2], r[3]]) as usize);
        let max_iter = 256u32;
        let step = 3.0f32 / IMG_W as f32;
        let (x0, y0) = (-2.2f32, -(IMG_H as f32) * 0.5 * step);
        wr32(out + 0x200, 0);
        let u = [IMG_VA, IMG_W.trailing_zeros() as u64 | (max_iter as u64) << 32, x0.to_bits() as u64 | (y0.to_bits() as u64) << 32, step.to_bits() as u64];
        let n = cx.prepare(&MANDEL, &u, (IMG_W * IMG_H / 16) as u32, done, &mut words);
        let t0 = now();
        let rendered = run_for("mandelbrot 1024x2048", &words[..n], &|| rd32(out + 0x200) == 1, 4000);
        let gpu_us = ferros_sys::us_since(t0);
        let px = surf + window::PORCH as usize;
        let (ox, oy) = ((w - IMG_W) / 2, (h - IMG_H) / 2);
        for y in 0..h {
            for x in 0..w {
                let inside = x >= ox && x < ox + IMG_W && y >= oy && y < oy + IMG_H;
                let k = if inside { rd32(img + 4 * ((y - oy) * IMG_W + x - ox)) } else { max_iter };
                // Linear light k / max_iter, in gamma 2 at Q14: 2^14 · √(k / max_iter).
                let v = if k >= max_iter { 0 } else { ((k as u64) << 28) / max_iter as u64 }.isqrt();
                wr64(px + 8 * (y * w + x), v | v << 16 | v << 32);
            }
        }
        if !rendered || !window::show(display, surf as u64, 0, 0, w as u16, h as u16, window::SYSTEM) {
            say!(grants, "gpu: show: {}\n", if rendered { "the display refused the window" } else { "the render did not finish" });
            exit(1);
        }
        say!(grants, "gpu: show: the Mandelbrot set rendered by the GPU in {gpu_us} us, on the glass for 60 s\n");
        let end = now() + 60 * g.f;
        while now() < end {
            ferros_sys::sleep_until(now() + g.f);
        }
        window::done(display, surf as u64);
    }
    // Left quiet until a resident service keeps it: a soft reset stops the MCU and its jobs and powers the L2 down (the shader cores are the MCU's, off with it), so an ended driver leaves nothing running.
    g.wr(GPU_IRQ_MASK, RESET_COMPLETED);
    g.wr(GPU_COMMAND, 0x101);
    let quiet = g.wait(500, |g| g.rd(GPU_IRQ_RAWSTAT) & RESET_COMPLETED != 0);
    g.wr(GPU_IRQ_CLEAR, IRQ_ALL);
    g.wr(GPU_IRQ_MASK, 0);
    g.wr(JOB_IRQ_MASK, 0);
    g.wr(MMU_IRQ_MASK, 0);
    say!(grants, "gpu: left quiet: reset {}, MCU_STATUS G#{:X}, L2_READY G#{:X}, SHADER_READY G#{:X}\n", if quiet { "done" } else { "never completed" }, g.rd(MCU_STATUS), g.rd64(L2_READY), g.rd64(0x140));
    exit(if ok { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

/// MOVE48: register pair `r` = a 48-bit immediate.
fn mv48(r: u64, v: u64) -> u64 {
    0x01 << 56 | r << 48 | v
}
/// MOVE32: register `r` = a 32-bit immediate.
fn mv32(r: u64, v: u32) -> u64 {
    0x02 << 56 | r << 48 | v as u64
}

/// One queue: its ring and user-IO pages (as the CPU sees them), the bytes inserted so far, its doorbell.
struct Queue {
    ring: usize,
    uio: usize,
    insert: core::cell::Cell<usize>,
    doorbell: usize,
}

impl Queue {
    /// Append `words` to the ring, publish the new insert offset in the user input page, ring the queue's doorbell.
    fn submit(&self, g: &Gpu, words: &[u64]) {
        let at = self.insert.get();
        for (k, &w) in words.iter().enumerate() {
            wr64(self.ring + at + 8 * k, w);
        }
        // Padded with NOPs (zero words) to the next 64 bytes: the command-stream front end fetches the ring a 64-byte line at a time and keeps the line, so a job that began inside the line the last one ended in ran as the NOPs it had fetched there (2026-11-04: the second job's words consumed, CS_EXTRACT moved on, nothing done).
        let end = (at + 8 * words.len()).next_multiple_of(64);
        for p in (at + 8 * words.len()..end).step_by(8) {
            wr64(self.ring + p, 0);
        }
        unsafe { core::arch::asm!("dmb osh") };
        self.insert.set(end);
        wr64(self.uio, self.insert.get() as u64);
        g.doorbell(self.doorbell);
    }
}

/// A compute dispatch's memory: one page for the shader, one for its descriptors and uniforms (as the CPU sees them and at their VAs in the group's address space), and the cores' thread limit (THREAD_MAX_THREADS), which sizes a task.
struct Compute {
    code: usize,
    desc: usize,
    code_va: u64,
    desc_va: u64,
    max_threads: u32,
}

impl Compute {
    /// The shader `code` (Valhall words, at most a page) written and padded, its program descriptor (compute, 32 registers a thread, r60 preloaded with the global id X), no local storage, `uniforms` as 64-bit FAU entries; and into `out` the command stream for `groups` workgroups of 16 invocations in X, then a 32-bit sync object at `done` set to 1. Returns the stream's length in words.
    fn prepare(&self, code: &[u64], uniforms: &[u64], groups: u32, done: u64, out: &mut [u64; 32]) -> usize {
        self.prepare_grid(code, uniforms, (groups, 1), 0x1000, done, out)
    }

    /// As `prepare` over a grid of `groups.0` × `groups.1` workgroups (16 invocations in X each), with the descriptor's preload word `preload` (bit 12 r60 = the global id X, bit 13 r61 = the global id Y). A grid of more than one row runs a row a task (axis Y, increment 1: Mesa's chooser allows any increment of 1 and up).
    fn prepare_grid(&self, code: &[u64], uniforms: &[u64], groups: (u32, u32), preload: u32, done: u64, out: &mut [u64; 32]) -> usize {
        for (k, w) in code.iter().enumerate() {
            wr64(self.code + 8 * k, *w);
        }
        for k in code.len()..PAGE / 8 {
            wr64(self.code + 8 * k, 0);
        }
        let (spd, tsd, fau) = (self.desc_va, self.desc_va + 0x40, self.desc_va + 0x80);
        for (k, w) in [0x8000_0018u32, preload, self.code_va as u32, (self.code_va >> 32) as u32, 0, 0, 0, 0].iter().enumerate() {
            wr32(self.desc + 4 * k, *w);
        }
        for k in 0..8 {
            wr32(self.desc + 0x40 + 4 * k, if k == 1 { 0x1F } else { 0 });
        }
        for (k, &u) in uniforms.iter().enumerate() {
            wr64(self.desc + 0x80 + 8 * k, u);
        }
        // The task's axis and size, as Mesa's gallium chooses them (pan_csf.c): along X while the threads fit the core's limit; a task of every workgroup (axis Z, increment 1) when they all fit.
        let (axis, inc) = if groups.1 > 1 { (1u64, 1u64) } else if 16 * groups.0 >= self.max_threads { (0, (self.max_threads / 16) as u64) } else { (2, 1) };
        let words = [
            0x2200_0000_0000_0001u64,
            0x1C00_0000_0000_0003,
            0x1C00_0001_0000_0000,
            mv48(0, 0),
            mv32(8, fau as u32),
            mv32(9, (fau >> 32) as u32 | (uniforms.len() as u32) << 24),
            mv48(16, spd),
            mv48(24, tsd),
            mv32(32, 0),
            mv32(33, 0x8000_000F),
            mv32(34, 0),
            mv32(35, 0),
            mv32(36, 0),
            mv32(37, groups.0),
            mv32(38, groups.1),
            mv32(39, 1),
            0x04 << 56 | axis << 14 | inc,
            0x0300_0000_0008_0000,
            mv32(74, 0),
            0x2400_4A00_0000_0211,
            0x0300_0000_0001_0000,
            mv48(70, done),
            mv32(72, 1),
            0x2600_4648_0000_0000,
        ];
        out[..words.len()].copy_from_slice(&words);
        words.len()
    }
}

/// A short formatted tail for a console line, without an allocator.
struct Tail([u8; 48], usize);
impl core::fmt::Display for Tail {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(core::str::from_utf8(&self.0[..self.1]).unwrap_or(""))
    }
}
impl Write for Tail {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.0.len() - self.1);
        self.0[self.1..self.1 + n].copy_from_slice(&b[..n]);
        self.1 += n;
        Ok(())
    }
}
/// "; first at gid G: GPU n, CPU n".
fn alloc_free_fmt(g: u32, a: u32, b: u32) -> Tail {
    let mut t = Tail([0; 48], 0);
    let _ = write!(t, "; first at gid {g}: GPU {a}, CPU {b}");
    t
}
