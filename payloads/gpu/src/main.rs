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
/// The layer arena (slot 5) and the `gpu` endpoint this process serves (slot 6, init's grants at boot); the apps' surfaces (slot 7, since 2026-11-11: a client's kernel writes its own app surface).
const S_ARENA: u64 = 5;
const S_GPU: u64 = 6;
#[allow(dead_code)]
const S_SURFACES: u64 = 7;

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
const AS_LOCKADDR: usize = 0x10;
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
/// A level-2 block of 2 MiB, read-write (kbase mmu_mode_aarch64.c: an ATE above level 3 ends in 1, ENTRY_IS_ATE_L02, where a level-3 one ends in 3), with the access flag; a client's GPU block's frames (since 2026-11-13).
const BLOCK_RW: u64 = 0x441;
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
/// A client's GPU block in its address space (ferros_sys::gpu `ARENA_VA`), its frames mapped from there in 2 MiB blocks (`Table::map_blocks`) in the order its stamp lists them, so its offsets mean what they mean on the CPU from its block's base. The grant (slot 5, `arena`): the GPU blocks' window since 2026-11-13 (kernel cachemap GPUB_VA, its size from the kernel, `dma_size`), until then the fixed layer arena (128 MiB from 2026-11-11, 64 MiB from a kernel before that, which cannot say).
const ARENA_VA: u64 = 0x8000_0000;
const ARENA_MOST: usize = ferros_sys::GPU_BLOCK_SLOTS * ferros_sys::GPU_BLOCK_MOST;
const ARENA_BEFORE: usize = 64 << 20;
/// The apps' surfaces in the group's address space (coherent as the arena is): a client names its own as SURFACES_VA plus its address less G#CA00_0000 (ferros_sys::gpu `SURFACES_PA`, the four surfaces' physical base until 2026-11-12, the point the view is reckoned from since), its address being its slot's in the surfaces' window (kernel cachemap SURF_VA), so slot k lies at G#50_0000_0000 plus k times 32 MiB here; each of its 16 frames mapped where the kernel's stamp says it is.
const SURFACES_VA: u64 = 0xC000_0000;
/// The whole surfaces' window, as the kernel grants it (slot 7).
const SURFACES_BYTES: usize = ferros_sys::SURFACE_SLOTS * ferros_sys::SURFACE_BYTES;
/// The service's shader code: 32 pages, 16384 words.
const CODE_BIG_VA: u64 = 0x3400_0000;
const CODE_PAGES: usize = 32;
/// The most uniform slots a run passes: 64 (16 until 2026-11-11), from G#80 in the descriptor page to G#280. A kernel reads a constant in a uniform with no move before it; Mahere's map kernel carries over a hundred of the frame's (its light, inks, sun, disk), each a move per pixel before (206 of its 836 words).
const UNIFORMS: usize = 64;
/// The GPU MMU's interrupt (slot 8, since 2026-11-12): a client's job that reaches outside its own memory faults in its own address space, and the service ends that job at once (`mmu_watch`).
const S_MMU: u64 = 8;

/// The resident service's clients at once (GPU-KISTS.md plan step 1, 2026-11-12): a command-stream group each, groups 0 to 6, bound to address spaces 1 to 7 (AS0 is the MCU's; the GPU has 8 address spaces and the firmware 8 groups), user doorbells 1 to 7. An eighth client takes the slot used longest ago (`serve`).
const SLOTS: usize = 7;
/// Each slot's suspend buffer and user-IO pages in the MCU's address space (its shared zone, G#0400_0000 to G#0800_0000, kbase's), mapped before the MCU starts.
const SLOT_UIO_VA: u64 = 0x0430_0000;
const SLOT_SUSPEND_VA: u64 = 0x0440_0000;
/// Each slot's own page-table pages, for its client's address space: a block of the arena of up to 128 MiB is 64 level-3 tables, the most surfaces a stamp carries (four, ferros_sys::gpu `FRAMES`) 64 more and one level-2 table (the view's slots all lie in one GiB), and the service's own pages and the levels above about a dozen.
const TABLE_PAGES: usize = 160;
/// How long a client's job may run before the service ends it (its group terminated): a system process's (wallgen's bands, the mark's fine passes) 2^10 ms, a kist's 2^8 ms, so one kist's job holds the GPU from every other client a quarter second at most. The firmware's progress timer (2.6 G GPU cycles, global init) stops a job that stops progressing on its own.
const GUARD_SYSTEM_MS: u64 = 1 << 10;
const GUARD_KIST_MS: u64 = 1 << 8;
/// CSG_EP_REQ's priority (bits 31:28) of a system client's group and a kist's: the firmware runs the higher first when both have work.
const PRIORITY_SYSTEM: u32 = 15;
const PRIORITY_KIST: u32 = 8;

/// A slot's memory in the MCU's address space (filled before the MCU starts).
#[derive(Clone, Copy, Default)]
struct SlotMem {
    susp_va: u64,
    uio_va: u64,
    uio: usize,
}

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
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
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
    // Started with no input (at boot, by the kernel): the resident service, a group and an address space for each client (`serve`), whose suspend buffers and user-IO pages the MCU's address space must hold before it starts.
    let serving = input_len == 0;
    let mut slot_mem = [SlotMem::default(); SLOTS];
    if serving {
        for (k, sm) in slot_mem.iter_mut().enumerate() {
            sm.susp_va = SLOT_SUSPEND_VA + (k * SUSPEND_PAGES * PAGE) as u64;
            sm.uio_va = SLOT_UIO_VA + (k * 2 * PAGE) as u64;
            let susp = m.alloc(SUSPEND_PAGES);
            as0.map(&mut m, sm.susp_va, susp, SUSPEND_PAGES, PAGE_RW | NX, TABLE);
            sm.uio = m.alloc(2);
            as0.map(&mut m, sm.uio_va, sm.uio, 1, PAGE_RO | NC | NX, TABLE);
            as0.map(&mut m, sm.uio_va + PAGE as u64, sm.uio + PAGE, 1, PAGE_RW | NC | NX, TABLE);
        }
    }
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
    if serving {
        serve(&g, &sh, &mut m, slot_mem, cores, grants);
    }

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
    // The layer arena (input bit 6; grant slot 5, `layers`): all 64 MiB mapped coherent (the shared type, shareable both ways), so whatever the CPU has in its caches the GPU sees, and the other way (GPU.md "Coherency").
    let arena = ferros_sys::dma(cap(grants, S_ARENA)) as usize;
    // The test suite (tools/husky-gpu.sh, started with input bytes): the arena only on input bit 6, and only a fixed arena at its physical address (a kernel before 2026-11-13): since then the grant is the GPU blocks' window, addresses whose frames only a block's stamp names, and gputest covers a client's block.
    let windowed = arena as u64 == ferros_sys::GPU_BLOCKS_WINDOW;
    if in0 & 0x40 != 0 && windowed {
        say!(grants, "gpu: no layer arena test: the arena is the GPU blocks' window, frames given per client (gputest tests a block)\n");
    }
    let arena_ok = in0 & 0x40 != 0 && arena != 0 && arena != usize::MAX && !windowed;
    let arena_bytes = ferros_sys::dma_size(cap(grants, S_ARENA), ARENA_MOST as u64).map_or(ARENA_BEFORE, |n| n as usize);
    if arena_ok {
        as1.map(&mut m, ARENA_VA, arena, arena_bytes / PAGE, PAGE_RW | 6 << 2 | 2 << 8 | NX, TABLE);
    }
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
    // The fault the stream raised in the last job, as the stream's output block holds it: CS_FAULT (exception type bits 7:0, its data 31:8), CS_FAULT_INFO and CS_STATUS_CMD_PTR (kbase mali_kbase_csf_registers.h, output +G#80, +G#88, +G#40); all zero when the job raised none. CS_FAULT itself keeps the last fault's value for ever after, so a fault is told by CS_REQ's FAULT bit (31) against CS_ACK's, never by that register.
    let last_fault = core::cell::Cell::new((0u32, 0u64, 0u64));
    // A recoverable fault noted, then acknowledged as kbase acknowledges one (mali_kbase_csf.c handle_fault_event and kbase_csf_ring_cs_kernel_doorbell): CS_REQ's FAULT bit matched to CS_ACK's, the stream's bit of CSG_DB_REQ toggled against CSG_DB_ACK, the group's bit of GLB_DB_REQ against GLB_DB_ACK, doorbell 0. Without it the stream stays halted at the fault, and every later job waits out its guard with no interrupt (2026-11-11 on the Pro: CS_FAULT G#C51 once, then each serve 1 s with 0 interrupts until `diag "restart gpu"`). Not yet seen on the device: whether the stream then runs the job's tail (its WAIT, FLUSH_CACHE2 and done flag) is the next fault's to show.
    let fault_ack = || -> bool {
        let (req, ack) = (sh.rd(sin), sh.rd(sout));
        if (req ^ ack) & 1 << 31 == 0 {
            return false;
        }
        last_fault.set((sh.rd(sout + 0x80), sh.rd(sout + 0x88) as u64 | (sh.rd(sout + 0x8C) as u64) << 32, sh.rd(sout + 0x40) as u64 | (sh.rd(sout + 0x44) as u64) << 32));
        sh.wr(sin, (req & !(1 << 31)) | (ack & 1 << 31));
        unsafe { core::arch::asm!("dmb osh") };
        sh.wr(cin + 0x08, (sh.rd(cin + 0x08) & !1) | ((sh.rd(cout + 0x08) ^ 1) & 1));
        sh.wr(gin + 0x08, (sh.rd(gin + 0x08) & !1) | ((sh.rd(gout + 0x08) ^ 1) & 1));
        g.doorbell(0);
        true
    };
    // A job that raised a fault failed even when its done flag lands after the acknowledgement lets the stream run on.
    let run_for = |name: &str, words: &[u64], check: &dyn Fn() -> bool, ms: u64| -> bool {
        let t0 = now();
        last_fault.set((0, 0, 0));
        q.submit(&g, words);
        let deadline = t0 + g.f / 1000 * ms;
        let (mut wakes, mut faulted) = (0, false);
        while !check() && now() < deadline {
            if ferros_sys::irq_wait(irq, deadline) == 0 {
                wakes += 1;
                events(&g);
                faulted |= fault_ack();
            }
        }
        faulted |= fault_ack();
        let ok = check() && !faulted;
        // A served job that ran is the client's to say (its own line has its µs); a line for each would be 120 a second from a client drawing every frame (Mahere's map, 2026-11-11). The test suite's jobs and every failure are said.
        if !ok || name != "serve" {
            say!(grants, "gpu: job {name}: {} in {} us after {wakes} interrupts; CS_FAULT G#{:X}, CS_FATAL G#{:X}; CSG_REQ G#{:X} CSG_ACK G#{:X}\n", if ok { "done" } else if faulted { "faulted" } else { "did not finish" }, ferros_sys::us_since(t0), sh.rd(sout + 0x80), sh.rd(sout + 0x84), sh.rd(cin), sh.rd(cout));
        }
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
    // Input bit 6: the layer arena, both ways, at its far end and its start. The CPU writes 16 words through its caches at 60 MiB in and the GPU copies them out; then the GPU renders the 64 x 64 Mandelbrot into the arena's first 16 KiB while the CPU holds those lines stale in its caches, and the CPU checks it against its own arithmetic.
    if arena_ok {
        let far = arena + (60 << 20);
        for i in 0..16 {
            unsafe { core::ptr::write_volatile((far + 4 * i) as *mut u32, 0xA4E0_0000 + i as u32) };
            wr32(res + 4 * i, 0);
        }
        wr32(out + 0x200, 0);
        let copy = [0x01A3_C200_2027_3C80u64, 0x0860_8482_1800_0002, 0x01A3_C200_2027_3C82, 0x7861_4402_1800_0002];
        let n = cx.prepare(&copy, &[ARENA_VA + (60 << 20), RES_VA], 1, done, &mut words);
        run_for("arena read", &words[..n], &|| rd32(out + 0x200) == 1, 200);
        let got = (0..16).filter(|&i| rd32(res + 4 * i) == 0xA4E0_0000 + i as u32).count();
        for i in 0..4096 {
            unsafe { core::ptr::write_volatile((arena + 4 * i) as *mut u32, 0xFFFF_FFFF) };
        }
        let stale: u32 = (0..4096).map(|i| unsafe { core::ptr::read_volatile((arena + 4 * i) as *const u32) } & 1).sum();
        wr32(out + 0x200, 0);
        let (shift, max_iter, x0, y0, step) = (6u32, 64u32, -2.0f32, -1.25f32, 0.0390625f32);
        let u = [ARENA_VA, shift as u64 | (max_iter as u64) << 32, x0.to_bits() as u64 | (y0.to_bits() as u64) << 32, step.to_bits() as u64];
        let n = cx.prepare(&MANDEL, &u, 4096 / 16, done, &mut words);
        run_for("arena mandelbrot", &words[..n], &|| rd32(out + 0x200) == 1, 200);
        let sum: u32 = (0..4096).map(|i| unsafe { core::ptr::read_volatile((arena + 4 * i) as *const u32) }).sum();
        say!(grants, "gpu: layer arena at G#{arena:X}: CPU wrote, GPU read {got} of 16 at 60 MiB in; GPU rendered the Mandelbrot there, CPU read it from its caches: sum {sum} (expect 83000; {stale} stale lines held before)\n");
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
    // Input bit 0: show it (tools/husky-gpu.sh show). The GPU renders the Mandelbrot set at 1024 x 2048 (up to 256 iterations, real -2.2 to 0.8), the CPU turns the counts into grey on a surface (black in the set, brighter the faster a point escapes: gamma 2, the display's working values), and the display shows it at the app level, in front of the map, for 60 s (the system level is the system's alone, WINDOWS.md "Whose level"). Grant slots for it: 5 use display, 6 a surface.
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
        if !rendered || !window::show(display, surf as u64, 0, 0, w as u16, h as u16, window::APP) {
            say!(grants, "gpu: show: {}\n", if rendered { "the display refused the window" } else { "the render did not finish" });
            exit(1);
        }
        say!(grants, "gpu: show: the Mandelbrot set rendered by the GPU in {gpu_us} us, on the glass for 60 s\n");
        let end = now() + 60 * g.f;
        while now() < end {
            ferros_sys::sleep_until(now() + g.f);
        }
    }
    // One shot (owner, 2026-11-08): no reset and no window closed on the way out; the display drops a dead process's window, and the GPU is left as the demo left it until the next driver resets it at its start.
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
        // The ring is one page (CS_RING size, set at the queue's start) and the command stream reads it at the insert offset modulo that size: the offset grows for ever and each word goes in at it modulo the page. Until 2026-11-05 the words went in at the raw offset, past the page after about 21 jobs, while the stream read the page's start again: the 16th fine pass of a session wedged the queue (CS_FATAL G#40). One job at a time, each waited for, so a wrap never overwrites words the stream has not read.
        let at = self.insert.get();
        for (k, &w) in words.iter().enumerate() {
            wr64(self.ring + (at + 8 * k) % PAGE, w);
        }
        // Padded with NOPs (zero words) to the next 64 bytes: the command-stream front end fetches the ring a 64-byte line at a time and keeps the line, so a job that began inside the line the last one ended in ran as the NOPs it had fetched there (2026-11-04: the second job's words consumed, CS_EXTRACT moved on, nothing done).
        let end = (at + 8 * words.len()).next_multiple_of(64);
        for p in (at + 8 * words.len()..end).step_by(8) {
            wr64(self.ring + p % PAGE, 0);
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
        for k in code.len()..(code.len() + 16).max(PAGE / 8) {
            wr64(self.code + 8 * k, 0);
        }
        let (spd, tsd, fau) = (self.desc_va, self.desc_va + 0x40, self.desc_va + 0x80);
        // Bit 15 of the preload word, the service's own: 64 registers a thread (register allocation 0 at bits 31:30 of word 0, v11.xml "Shader Register Allocation"; 2 is 32, and in 32-register mode Valhall has only r0-r15 and r48-r63). The SSA compiler asks for it when it uses more.
        let (spd0, preload) = if preload & 0x8000 != 0 { (0x0000_0018u32, preload & 0x7FFF) } else { (0x8000_0018, preload) };
        for (k, w) in [spd0, preload, self.code_va as u32, (self.code_va >> 32) as u32, 0, 0, 0, 0].iter().enumerate() {
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

/// The GPU MMU's fault, per address space, as `mmu_watch` saw it: AS_FAULTSTATUS with bit 63 set, so a fault whose status reads zero still counts; and the faulting GPU address. Taken (and cleared) by the job's wait.
static MMU_FAULTS: [core::sync::atomic::AtomicU64; 8] = [const { core::sync::atomic::AtomicU64::new(0) }; 8];
static MMU_ADDRS: [core::sync::atomic::AtomicU64; 8] = [const { core::sync::atomic::AtomicU64::new(0) }; 8];
/// The GPU's registers, for `mmu_watch`.
static MMU_REGS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static mut MMU_STACK: [u8; 1 << 14] = [0; 1 << 14];

/// Address space `n` let go: TRANSCFG UNMAPPED (1), TRANSTAB 0, UPDATE (kbase's `kbase_mmu_disable_as`, as `kbase_mmu_report_fault_and_kill` does it). The accesses stalled on its fault end, and the job with them.
fn space_unmap(regs: usize, n: usize) {
    wr32(regs + Gpu::as_reg(n, AS_TRANSCFG), 1);
    wr32(regs + Gpu::as_reg(n, AS_TRANSCFG) + 4, 0);
    wr32(regs + Gpu::as_reg(n, AS_TRANSTAB), 0);
    wr32(regs + Gpu::as_reg(n, AS_TRANSTAB) + 4, 0);
    wr32(regs + Gpu::as_reg(n, AS_COMMAND), 1);
}

/// A fault in address space `n` noted for its job's wait: AS_FAULTSTATUS and AS_FAULTADDRESS read, the space let go (`space_unmap`).
fn space_fault(regs: usize, n: usize) {
    let st = rd32(regs + Gpu::as_reg(n, AS_FAULTSTATUS));
    let addr = rd32(regs + Gpu::as_reg(n, AS_FAULTADDRESS)) as u64 | (rd32(regs + Gpu::as_reg(n, AS_FAULTADDRESS) + 4) as u64) << 32;
    MMU_ADDRS[n].store(addr, core::sync::atomic::Ordering::Relaxed);
    MMU_FAULTS[n].store(1 << 63 | st as u64, core::sync::atomic::Ordering::Release);
    space_unmap(regs, n);
}

/// The GPU MMU's interrupt, on a thread of its own (2026-11-12): a job reaching outside its client's address space faults there, and its accesses stall until the space is let go. Each faulting space is noted and let go (`space_fault`), the fault cleared; the job's wait then finds it and ends that client's group. No other client's space is touched.
extern "C" fn mmu_watch(irq: u64) -> ! {
    let regs = MMU_REGS.load(core::sync::atomic::Ordering::Acquire);
    loop {
        if ferros_sys::irq_wait(irq, u64::MAX) != 0 {
            continue;
        }
        let raw = rd32(regs + MMU_IRQ_RAWSTAT);
        for n in 1..8 {
            // Bit n a page fault in address space n. The bits from 16 are not bus faults on a CSF GPU (kbase's CSF handler takes bits 0-15 alone, mmu/backend/mali_kbase_mmu_csf.c `kbase_mmu_interrupt`; a bus fault comes as a GPU fault): bit 17 rose on akita with no fault in AS1 (2026-11-12) after its LOCK and FLUSH_PT, and is left masked.
            if raw & 1 << n != 0 {
                space_fault(regs, n);
            }
        }
        wr32(regs + MMU_IRQ_CLEAR, raw);
        ferros_sys::irq_ack(irq);
    }
}

/// A client's slot: its own group, address space, ring, descriptors, done flag, code region and page-table pages, all in gpu-mem; and the client bound to it, by the kernel's stamp.
struct Slot {
    mem: SlotMem,
    ring: usize,
    desc: usize,
    flag: usize,
    code: usize,
    pool: usize,
    /// Its address space's root while a client is bound.
    root: usize,
    insert: core::cell::Cell<usize>,
    client: Option<ferros_sys::gpu::Key>,
    /// When it last ran a job (a count of jobs), for choosing the slot used longest ago.
    used: u64,
    /// Its group is running (started, not ended since).
    running: bool,
}

/// Slot `k`'s group's blocks in the shared interface: CSG input and output, stream 0's input and output (GPU.md, ferros-csf-spec.md: group n at G#0400_1000 + n · G#A0).
fn blocks(sh: &Shared, k: usize) -> (u32, u32, u32, u32) {
    let gctl = 0x0400_1000u32 + 0xA0 * k as u32;
    (sh.rd(gctl + 4), sh.rd(gctl + 8), sh.rd(gctl + 0x40 + 4), sh.rd(gctl + 0x40 + 8))
}

/// GLB_DB_REQ's bit `bit` toggled against GLB_DB_ACK, then doorbell 0: the firmware looks at that group's request.
fn glb_doorbell(g: &Gpu, sh: &Shared, bit: u32) {
    let (gin, gout) = (sh.rd(0x0400_0008), sh.rd(0x0400_000C));
    sh.wr(gin + 0x08, (sh.rd(gin + 0x08) & !bit) | ((sh.rd(gout + 0x08) ^ bit) & bit));
    g.doorbell(0);
}

/// The command-stream group of slot `k` (group k, address space k + 1, doorbell k + 1), started as kbase starts one (program_cs, program_csg_slot) and as the test suite's group 0 is: its stream's ring and user-IO pages, every ack interrupt, START; the group's cores, endpoints and `priority`, its address space, its suspend buffer, START. False if the firmware did not acknowledge it in 100 ms.
fn group_start(g: &Gpu, sh: &Shared, k: usize, sl: &Slot, cores: u64, priority: u32) -> bool {
    let (cin, cout, sin, sout) = blocks(sh, k);
    let doorbell = k as u32 + 1;
    // A fresh queue: its user-IO pages zeroed (insert and extract at 0), as kbase gives a new queue.
    for w in 0..2 * PAGE / 8 {
        wr64(sl.mem.uio + 8 * w, 0);
    }
    sl.insert.set(0);
    sh.wr(sin + 0x10, RING_VA as u32);
    sh.wr(sin + 0x14, (RING_VA >> 32) as u32);
    sh.wr(sin + 0x18, PAGE as u32);
    sh.wr(sin + 0x30, sl.mem.uio_va as u32);
    sh.wr(sin + 0x34, (sl.mem.uio_va >> 32) as u32);
    sh.wr(sin + 0x38, (sl.mem.uio_va + PAGE as u64) as u32);
    sh.wr(sin + 0x3C, ((sl.mem.uio_va + PAGE as u64) >> 32) as u32);
    sh.wr(sin + 0x04, doorbell << 8);
    sh.wr(sin + 0x0C, 0xFFFF_FFFF);
    let cs_ack = sh.rd(sout);
    let keep = 1 << 27 | 1 << 30 | 1 << 31;
    sh.wr(sin, (cs_ack & keep) | 1 << 10 | 1 << 8 | 1 << 12 | 1);
    unsafe { core::arch::asm!("dmb osh") };
    sh.wr(cin + 0x08, (sh.rd(cin + 0x08) & !1) | ((sh.rd(cout + 0x08) ^ 1) & 1));
    sh.wr(cin + 0x20, cores as u32);
    sh.wr(cin + 0x24, (cores >> 32) as u32);
    sh.wr(cin + 0x28, cores as u32);
    sh.wr(cin + 0x2C, (cores >> 32) as u32);
    sh.wr(cin + 0x30, g.rd64(TILER_PRESENT) as u32);
    sh.wr(cin + 0x34, cores.count_ones() | cores.count_ones() << 8 | 1 << 16 | priority << 28);
    sh.wr(cin + 0x38, 0);
    sh.wr(cin + 0x50, k as u32 + 1);
    sh.wr(cin + 0x40, sl.mem.susp_va as u32);
    sh.wr(cin + 0x44, (sl.mem.susp_va >> 32) as u32);
    sh.wr(cin + 0x48, 0);
    sh.wr(cin + 0x4C, 0);
    sh.wr(cin + 0x04, 0xFFFF_FFFF);
    sh.wr(cin, (sh.rd(cin) & !7) | 1);
    unsafe { core::arch::asm!("dmb osh") };
    glb_doorbell(g, sh, 1 << k);
    g.wait(100, |_| sh.rd(cout) & 7 == 1 && sh.rd(sout) & 7 == 1)
}

/// Slot `k`'s group ended (CSG_REQ's state TERMINATE, 0): whatever job it runs stops, kbase's way of ending a context's groups. False if the firmware did not acknowledge it in 100 ms.
fn group_end(g: &Gpu, sh: &Shared, k: usize) -> bool {
    let (cin, cout, _, _) = blocks(sh, k);
    sh.wr(cin, sh.rd(cin) & !7);
    unsafe { core::arch::asm!("dmb osh") };
    glb_doorbell(g, sh, 1 << k);
    g.wait(100, |_| sh.rd(cout) & 7 == 0)
}

/// Address space `n` given `root`: the walks and lines of whatever tables it had dropped first (LOCKADDR over the whole 48-bit space, LOCK, FLUSH_PT, which cleans and invalidates the L2 and the MMU's cached walks with it: kbase's sequence for changed mappings on arch 11, ferros-csf-spec.md), then TRANSCFG, TRANSTAB, MEMATTR and UPDATE (`Gpu::install`).
fn space_install(g: &Gpu, n: usize, root: usize) -> bool {
    let idle = |g: &Gpu| g.rd(Gpu::as_reg(n, AS_STATUS)) & 1 == 0;
    if !g.wait(64, idle) {
        return false;
    }
    // LOCKADDR: base 0, SIZE = log2(2^48) - 1.
    g.wr64(Gpu::as_reg(n, AS_LOCKADDR), 47);
    g.wr(Gpu::as_reg(n, AS_COMMAND), 2);
    if !g.wait(64, idle) {
        return false;
    }
    g.wr(Gpu::as_reg(n, AS_COMMAND), 4);
    if !g.wait(64, idle) {
        return false;
    }
    g.install(n, root)
}

/// A group that would not end: its job may still run in its client's address space, and that client may have ended and its memory be another's soon. The service ends, one shot: the GPU's next service soft-resets it before it serves (every job on it stopped), and the kernel holds every block and surface an ended client's job could reach until that service takes its first request (process.rs `gpu_drain`). Clients fall back as they do without a GPU service until it is started again (`diag "restart gpu"`).
fn stuck(grants: u64, k: usize) -> ! {
    say!(grants, "gpu: slot {k}'s group did not end in 100 ms: the service ends, so the GPU is reset before it runs anything more\n");
    exit(1)
}

/// The resident service (GPU.md "The resident service"; GPU-KISTS.md plan step 1, 2026-11-12): every client's jobs in a group and an address space of its own, which map only the memory the kernel's stamp says the client holds (its block of the layer arena at `ARENA_VA`, its surfaces at their place from `SURFACES_VA`) and the service's few pages for that client (its ring and descriptors read-only, its code region read-only and the only executable pages, its done flag). No address a client names reaches another client's memory or the service's: it is in no table of the client's space, the GPU MMU faults the access there, and the kernel's fence (s2mpu::fence_gpu) holds every space inside gpu-mem, the arena and the surfaces.
///
/// A request: the kernel's stamp (ferros_sys::gpu `Stamp`), then [1][preload u16, bit 15 = 64 registers a thread][code at an offset into the client's own block u32][code words u16][groups x u32][groups y u32][uniforms n u8, at most 64][n × u64], then optionally [n u8][the client's name][n u8][the kernel's name] (`label`). The reply: [status][µs u32]: 0 done, 1 a request it cannot take (no stamp, no block of the arena, its code outside its block), 2 the job faulted or did not finish in its guard, 3 its code starts with a zero word, 4 (`OUTSIDE`) it reached outside the client's memory, then [the GPU address u64][AS_FAULTSTATUS u32], 5 (`UNCHECKED`) a kist's job on a production build.
///
/// One job at a time, in the order the kernel queues the calls (each caller's thread holds one call), each ended at its guard (`GUARD_SYSTEM_MS`, `GUARD_KIST_MS`) with its group: a failed job's group is ended, and started again at its client's next job, so a fault costs only that client's job. The service takes its next request only when the last job is over, which is what lets the kernel give an ended client's memory to another (process.rs `gpu_drain`).
fn serve(g: &Gpu, sh: &Shared, m: &mut Mem, slot_mem: [SlotMem; SLOTS], cores: u64, grants: u64) -> ! {
    use ferros_sys::gpu::{is_block, Stamp, OUTSIDE, SURFACES_PA, UNCHECKED};
    let arena = ferros_sys::dma(cap(grants, S_ARENA)) as usize;
    let arena_bytes = ferros_sys::dma_size(cap(grants, S_ARENA), ARENA_MOST as u64).map_or(ARENA_BEFORE, |n| n as usize);
    let arena_ok = arena != 0 && arena != usize::MAX;
    // A client's surfaces are mapped for it only when the kernel fences them in for the GPU (granted here, the whole region, s2mpu::fence_gpu).
    let surfaces = ferros_sys::dma(cap(grants, S_SURFACES));
    let surfaces_ok = surfaces == ferros_sys::SURFACES_WINDOW && ferros_sys::dma_size(cap(grants, S_SURFACES), SURFACES_BYTES as u64) == Some(SURFACES_BYTES as u64);
    let slots: [Slot; SLOTS] = core::array::from_fn(|k| {
        let ring = m.alloc(1);
        let desc = m.alloc(1);
        let flag = m.alloc(1);
        let code = m.alloc(CODE_PAGES);
        let pool = m.alloc(TABLE_PAGES);
        Slot { mem: slot_mem[k], ring, desc, flag, code, pool, root: 0, insert: core::cell::Cell::new(0), client: None, used: 0, running: false }
    });
    let mut slots = slots;
    // Page faults in every client's space, on their own line and thread.
    let mmu = cap(grants, S_MMU);
    // The grants page's count (G#200, ferros_sys::cap) says whether slot 8 was given: the kernel that gives it is the one that stamps every call on `gpu` (2026-11-12). A kernel from before stamps nothing, so a request's first bytes would be the client's own, naming any memory: the service then serves nothing, each request answered 1 at once (its clients draw on the CPU, as with no GPU service), until the kernel is the stamping one.
    let stamped = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) } as u64 > S_MMU;
    if !stamped {
        say!(grants, "gpu: this kernel does not stamp calls on `gpu` (no slot 8): serving nothing\n");
        let mut msg = [0u8; 64];
        let mut token = [0u8; 32];
        loop {
            if ferros_sys::recv(cap(grants, S_GPU), &mut msg, &mut token, false) != ferros_sys::NOTHING {
                ferros_sys::reply(&token, &[1, 0, 0, 0, 0]);
            }
        }
    }
    g.wr(MMU_IRQ_CLEAR, 0xFFFF_FFFF);
    g.wr(MMU_IRQ_MASK, 0xFFFF);
    MMU_REGS.store(g.regs, core::sync::atomic::Ordering::Release);
    ferros_sys::thread_spawn(mmu_watch, unsafe { &mut *(&raw mut MMU_STACK) }, mmu);
    say!(grants, "gpu: serving `gpu`, a group and an address space a client ({} at once): the arena {} ({} MiB), the surfaces {}\n", SLOTS, if arena_ok { "granted" } else { "not granted" }, arena_bytes >> 20, if surfaces_ok { "fenced in" } else { "not granted" });
    let irq = cap(grants, S_IRQ);
    let max_threads = g.rd(THREAD_MAX_THREADS);
    let mut jobs = 0u64;
    let mut msg = [0u8; 1 << 13];
    let mut token = [0u8; 32];
    loop {
        let n = ferros_sys::recv(cap(grants, S_GPU), &mut msg, &mut token, false);
        if n == ferros_sys::NOTHING || n == 0 {
            continue;
        }
        let whole = &msg[..(n as usize).min(msg.len())];
        let mut reply = [1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut reply_len = 5;
        // The kernel's stamp: who calls and what memory it holds. None only from a kernel before the stamp, whose callers' memory the service cannot know: refused.
        let Some((stamp, m)) = Stamp::read(whole) else {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        };
        if stamp.kist && stamp.production {
            reply[0] = UNCHECKED;
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        // The client's block: the first region inside the arena (a client holds one; a holder of the whole arena, a development spawn's `--dma arena`, gets the whole).
        let block = stamp.regions().iter().copied().find(|&(b, sz)| arena_ok && b as usize >= arena && b as usize + sz as usize <= arena + arena_bytes);
        // The request's own lengths checked against what was sent, the code against the client's own block: they come from another process.
        let Some((bbase, bsize)) = block.filter(|_| m.len() >= 18 && m[0] == 1) else {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        };
        let u16_at = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([m[i], m[i + 1], m[i + 2], m[i + 3]]);
        let (preload, code_off, words) = (u16_at(1) as u32, u32_at(3) as usize, u16_at(7) as usize);
        let groups = (u32_at(9), u32_at(13));
        let nu = m[17] as usize;
        let fits = m.len() >= 18 + 8 * nu && nu <= UNIFORMS && words > 0 && words + 16 <= CODE_PAGES * PAGE / 8 && code_off % 8 == 0 && code_off + 8 * words <= bsize as usize && groups.0 > 0 && groups.1 > 0;
        if !fits {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        let code = unsafe { core::slice::from_raw_parts((bbase as usize + code_off) as *const u64, words) };
        let (client, rest) = label(&m[18 + 8 * nu..]);
        let (kernel, _) = label(rest);
        // WHY: a kernel's first word is an instruction, and the zero word is none a client emits (opcode 0 is NOP, which the valhall builder has no form for, and even Mesa's NOP sets bits 47:46: NOP.end is G#7800_C000_0000_0000, assembler-cases.txt). Zeros there mean the client's words never reached the arena (2026-11-11: masked ahau's pointer stores landed in its own arc).
        if code[0] == 0 {
            say!(grants, "gpu: refused {client}'s {kernel} kernel: its code at G#{code_off:X} in its block ({words} words) starts with a zero word, so its words never reached the arena\n");
            reply[0] = 3;
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        let mut uniforms = [0u64; UNIFORMS];
        for (k, u) in uniforms[..nu].iter_mut().enumerate() {
            *u = u64::from_le_bytes(m[18 + 8 * k..26 + 8 * k].try_into().unwrap());
        }
        jobs += 1;
        // The client's slot: its own if bound, else a free one, else the one used longest ago (its group ended, its space given the new client's tables).
        let k = match slots.iter().position(|sl| sl.client == Some(stamp.key())) {
            Some(k) => k,
            None => {
                let k = (0..SLOTS).min_by_key(|&k| (slots[k].client.is_some(), slots[k].used)).unwrap();
                if slots[k].running && !group_end(g, sh, k) {
                    stuck(grants, k);
                }
                let sl = &mut slots[k];
                sl.running = false;
                // The client's address space, from the stamp alone.
                let mut tm = Mem::new(sl.pool);
                let t = Table::new(&mut tm);
                t.map(&mut tm, RING_VA, sl.ring, 1, PAGE_RO | NC | NX, TABLE);
                t.map(&mut tm, DESC_VA, sl.desc, 1, PAGE_RO | NC | NX, TABLE);
                t.map(&mut tm, OUT_VA, sl.flag, 1, PAGE_RW | NC | NX, TABLE);
                t.map(&mut tm, CODE_BIG_VA, sl.code, CODE_PAGES, PAGE_RO | NC, TABLE);
                if is_block(bbase, bsize) {
                    // The block's frames in 2 MiB blocks from `ARENA_VA` (2026-11-13: frames of the pool, wherever it had them), in the stamp's order, which is the CPU's from the block's base.
                    let bi = stamp.regions().iter().position(|&r| r == (bbase, bsize)).unwrap();
                    for (j, pa) in stamp.frames_of(bi).enumerate() {
                        t.map_block(&mut tm, ARENA_VA + j as u64 * ferros_sys::gpu::FRAME, pa as usize, BLOCK_RW | 6 << 2 | 2 << 8 | NX, TABLE);
                    }
                } else {
                    // A kernel from before 2026-11-13: a block of the fixed arena, at its physical address.
                    t.map(&mut tm, ARENA_VA, bbase as usize, bsize as usize / PAGE, PAGE_RW | 6 << 2 | 2 << 8 | NX, TABLE);
                }
                // Each surface frame by frame (the stamp's frames: wherever the pool had them), at its place in the view; a stamp without frames (a kernel from before) maps none.
                for (i, &(b, sz)) in stamp.regions().iter().enumerate() {
                    if is_block(b, sz) {
                        continue;
                    }
                    for (j, pa) in stamp.frames_of(i).enumerate() {
                        if surfaces_ok {
                            t.map(&mut tm, SURFACES_VA + (b - SURFACES_PA) + j as u64 * ferros_sys::gpu::FRAME, pa as usize, ferros_sys::gpu::FRAME as usize / PAGE, PAGE_RW | 6 << 2 | 2 << 8 | NX, TABLE);
                        }
                    }
                }
                // The pool's size is the bound on any stamp the kernel writes (a block of 2 MiB entries in one level-2 table, at most the four surfaces in pages): past it is a bug here, loud.
                assert!(tm.used() <= TABLE_PAGES * PAGE, "gpu: a client's tables outgrew their slot");
                sl.root = t.root;
                sl.client = Some(stamp.key());
                k
            }
        };
        slots[k].used = jobs;
        let n_as = k + 1;
        if !slots[k].running {
            MMU_FAULTS[n_as].store(0, core::sync::atomic::Ordering::Relaxed);
            let ok = space_install(g, n_as, slots[k].root) && group_start(g, sh, k, &slots[k], cores, if stamp.kist { PRIORITY_KIST } else { PRIORITY_SYSTEM });
            if !ok {
                say!(grants, "gpu: slot {k} (address space {n_as}) did not start for {client}\n");
                reply[0] = 2;
                ferros_sys::reply(&token, &reply[..reply_len]);
                continue;
            }
            slots[k].running = true;
        }
        let sl = &slots[k];
        wr32(sl.flag + 0x200, 0);
        let cx = Compute { code: sl.code, desc: sl.desc, code_va: CODE_BIG_VA, desc_va: DESC_VA, max_threads };
        let mut job = [0u64; 32];
        let len = cx.prepare_grid(code, &uniforms[..nu], groups, preload, OUT_VA + 0x200, &mut job);
        let q = Queue { ring: sl.ring, uio: sl.mem.uio, insert: core::cell::Cell::new(sl.insert.get()), doorbell: n_as };
        let (_, _, sin, sout) = blocks(sh, k);
        let t0 = now();
        q.submit(g, &job[..len]);
        sl.insert.set(q.insert.get());
        let guard = if stamp.kist { GUARD_KIST_MS } else { GUARD_SYSTEM_MS };
        let deadline = t0 + g.f / 1000 * guard;
        let done = || rd32(sl.flag + 0x200) == 1;
        let mmu_fault = || MMU_FAULTS[n_as].load(core::sync::atomic::Ordering::Acquire);
        let cs_fault = || (sh.rd(sin) ^ sh.rd(sout)) & 1 << 31 != 0;
        while !done() && mmu_fault() == 0 && !cs_fault() && now() < deadline {
            if ferros_sys::irq_wait(irq, deadline) == 0 {
                // Each group's events acknowledged as kbase does (CSG_ACK's IDLE, SYNC_UPDATE and progress bits into CSG_REQ, CSG_IRQ_ACK = CSG_IRQ_REQ), every group with its bit set.
                let raw = g.rd(JOB_IRQ_RAWSTAT);
                g.wr(JOB_IRQ_CLEAR, raw);
                for j in 0..SLOTS {
                    if raw & 1 << j != 0 {
                        let (ji, jo, _, _) = blocks(sh, j);
                        let mask = 1 << 28 | 1 << 29 | 1 << 31;
                        sh.wr(ji, (sh.rd(ji) & !mask) | (sh.rd(jo) & mask));
                        sh.wr(ji + 0x0C, sh.rd(jo + 0x0C));
                    }
                }
                ferros_sys::irq_ack(irq);
            }
        }
        let us = ferros_sys::us_since(t0) as u32;
        let fault = MMU_FAULTS[n_as].swap(0, core::sync::atomic::Ordering::AcqRel);
        let faulted = cs_fault();
        reply[1..5].copy_from_slice(&us.to_le_bytes());
        if done() && fault == 0 && !faulted {
            reply[0] = 0;
        } else {
            let (cf, info, ptr) = (sh.rd(sout + 0x80), sh.rd(sout + 0x88) as u64 | (sh.rd(sout + 0x8C) as u64) << 32, sh.rd(sout + 0x40) as u64 | (sh.rd(sout + 0x44) as u64) << 32);
            let ran = |w: usize| unsafe { core::ptr::read_volatile((sl.code + 8 * w) as *const u64) };
            let (first, second, last) = (ran(0), ran(1), ran(words - 1));
            // The job ended with its group, whatever state it is in (past its guard, stalled, faulted): no other client's job waits behind it, and its group starts afresh at its client's next job.
            if !group_end(g, sh, k) {
                stuck(grants, k);
            }
            slots[k].running = false;
            if fault != 0 {
                let addr = MMU_ADDRS[n_as].load(core::sync::atomic::Ordering::Relaxed);
                reply[0] = OUTSIDE;
                reply[5..13].copy_from_slice(&addr.to_le_bytes());
                reply[13..17].copy_from_slice(&(fault as u32).to_le_bytes());
                reply_len = 17;
                say!(grants, "gpu: {client}'s {kernel} kernel reached outside its memory: GPU address G#{addr:X}, AS_FAULTSTATUS G#{:X} ({}) in its own address space {n_as}; its job ended\n", fault as u32, exception(fault as u32));
            } else {
                reply[0] = 2;
                say!(grants, "gpu: {client}'s {kernel} kernel {}: {words} words from G#{code_off:X} in its block, first G#{first:X} G#{second:X}, last G#{last:X}; {} x {} groups, preload G#{preload:X}, {nu} uniforms; {us} us\n", if faulted { "faulted" } else { "did not finish in its guard" }, groups.0, groups.1);
                say!(grants, "gpu: {client}'s {kernel} kernel: CS_FAULT G#{cf:X} ({}), CS_FAULT_INFO G#{info:X}, CS_STATUS_CMD_PTR G#{ptr:X}; its group ended\n", exception(cf));
            }
        }
        ferros_sys::reply(&token, &reply[..reply_len]);
    }
}

/// One name from a request's tail: [n u8][n bytes of UTF-8], and the bytes after it. "a client that did not say" when the tail is empty, cut short or not UTF-8 (a client from before the names, or gputest).
fn label(b: &[u8]) -> (&str, &[u8]) {
    // WHY: the tail is another process's bytes; a length past what was sent names nothing.
    match b.split_first() {
        Some((&n, rest)) if rest.len() >= n as usize => (core::str::from_utf8(&rest[..n as usize]).unwrap_or("a client that did not say"), &rest[n as usize..]),
        _ => ("a client that did not say", &[]),
    }
}

/// CS_FAULT's exception type (bits 7:0) by kbase's name for it (mali_kbase_csf_registers.h CS_FAULT_EXCEPTION_TYPE_*), for the line saying why a job failed.
fn exception(fault: u32) -> &'static str {
    match fault & 0xFF {
        0x00 => "none",
        0x05 => "KABOOM",
        0x0F => "CS_RESOURCE_TERMINATED",
        0x48 => "CS_BUS_FAULT",
        0x4B => "CS_INHERIT_FAULT",
        0x50 => "INSTR_INVALID_PC",
        0x51 => "INSTR_INVALID_ENC",
        0x55 => "INSTR_BARRIER_FAULT",
        0x56 => "RT_STACK_OVERFLOW",
        0x58 => "DATA_INVALID_FAULT",
        0x59 => "TILE_RANGE_FAULT",
        0x5A => "ADDR_RANGE_FAULT",
        0x5B => "IMPRECISE_FAULT",
        0x69 => "RESOURCE_EVICTION_TIMEOUT",
        0xC0..=0xC4 => "TRANSLATION_FAULT",
        0xC8..=0xCB => "PERMISSION_FAULT",
        _ => "another type: kbase CS_FAULT_EXCEPTION_TYPE",
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

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
