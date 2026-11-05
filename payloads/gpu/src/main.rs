//! The GPU driver (GPU.md): the Mali-G715's CSF microcontroller booted from Arm's firmware, its global interface answering, and a command-stream job run on it — as a process, the kernel holding only the GPU's DMA fence (s2mpu::fence_gpu) and its power.
//!
//! Spawn: `ferros-bridge spawn gpu.bin --name gpu --dma gpu-regs --dma gpu-mem --store gpu --irq gpu-job`. Grant slots: 0 console, 1 the GPU's registers (`gpu-regs`, EL0 Device, mapped at their physical address), 2 the memory the GPU and this process share (`gpu-mem`, Normal non-cacheable), 3 the vault scope `gpu.` (the firmware, `gpu.fw`), 4 the GPU's job line (`gpu-job`).
//!
//! The sequence is kbase's (r52p0, Google's tree), condensed in GPU.md "Bring-up": reset, L2, the MCU's address space and the firmware's sections, the MCU started, the global interface's init and a ping, then one command-stream group with one queue running a job of three instructions that writes a word to memory.
#![no_std]
#![no_main]

mod fw;
mod mem;

use core::fmt::Write;
use ferros_sys::{cap, console, exit, freq, now};
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
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let base = ferros_sys::dma(cap(grants, S_MEM)) as usize;
    let g = Gpu { regs, grants, f: freq() };
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
    g.wr(COHERENCY_ENABLE, 31);
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
    let run = |name: &str, words: &[u64], check: &dyn Fn() -> bool| -> bool {
        let t0 = now();
        q.submit(&g, words);
        let deadline = t0 + g.f / 10;
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
    let mut ok = true;
    // 1: STORE_MULTIPLE r4..r5 to [r0:r1 + 4] (Mesa genxml v11 CS STORE_MULTIPLE: offset 15:0, register mask 31:16, address pair 47:40, base register 55:48), then WAIT on every scoreboard slot so the stores are done before the queue moves on.
    wr32(dst + 4, 0);
    wr32(dst + 8, 0);
    ok &= run("store_multiple", &[mv48(0, DST_VA), mv32(4, 0x5EED_0001), mv32(5, 0x5EED_0002), 0x15 << 56 | 4 << 48 | 0 << 40 | 0b11 << 16 | 4, 0x03 << 56 | 0xFFFF << 16], &|| rd32(dst + 4) == 0x5EED_0001 && rd32(dst + 8) == 0x5EED_0002);
    // 0: MOVE48 r0:r1 = the destination, MOVE32 r2 = G#CAFE_F00D, SYNC_SET32 [r0:r1] = r2 (encodings kbase itself decodes, mali_kbase_csf_sync.c).
    wr32(dst, 0);
    ok &= run("sync_set", &[mv48(0, DST_VA), mv32(2, 0xCAFE_F00D), 0x26 << 56 | 0 << 40 | 2 << 32], &|| rd32(dst) == 0xCAFE_F00D);
    say!(grants, "gpu: the target reads G#{:X} G#{:X} G#{:X}; {} KiB of gpu-mem used\n", rd32(dst), rd32(dst + 4), rd32(dst + 8), m.used() >> 10);
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
