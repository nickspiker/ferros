//! The big core (Cortex-X3, CPU8, affinity G#800 in zuma-cpu.dtsi) — a probe before anything moves there: PSCI CPU_ON starts it on a tiny routine that turns its own I-cache on, times 2^24 dependent adds by the counter, writes what it found to non-cacheable memory, and turns itself off again (PSCI CPU_OFF). `diag x3` (service.rs) runs it and reports the core's clock as measured by work, against CPU0's.
//! Measure first (2026-10-22): the shell's frame on the little core is about 120 ms, ~25 cycles a pixel on the A510 even in NEON; the question is what the X3 does with the same work before any kernel moves to it.

use core::ptr::{read_volatile, write_volatile};

/// Where the probe reports: the last page of the device DMA pool (cachemap DMA_POOL, Normal non-cacheable), unused by anything running tonight.
pub const RESULT: usize = 0x819F_F000;
const MAGIC: u64 = 0x5833_5052_4F42_4521; // "X3PROBE!"
const PSCI_CPU_ON: u64 = 0xC400_0003;

core::arch::global_asm!(
    ".section .text.x3probe, \"ax\"",
    ".global ferros_x3_probe",
    "ferros_x3_probe:",
    // x0 = the result/context block: [0] magic, [8] ticks for the dependent chain, [16] MPIDR, [24] ticks for four independent chains, [32] MAIR, [40] TCR, [48] TTBR0, [56] SCTLR, [64] HCR — CPU0's live translation, so this core runs under the same table, caches on (cachemap.rs's order: HCR, then the table, then SCTLR).
    "ldr x1, [x0, #64]",
    "msr hcr_el2, x1",
    "isb",
    "ldr x1, [x0, #32]",
    "msr mair_el2, x1",
    "ldr x1, [x0, #40]",
    "msr tcr_el2, x1",
    "ldr x1, [x0, #48]",
    "msr ttbr0_el2, x1",
    "isb",
    "dsb ish",
    "tlbi alle2",
    "dsb ish",
    "isb",
    "ldr x1, [x0, #56]",
    "msr sctlr_el2, x1",
    "isb",
    // A dependent chain: 64 adds per pass, 2^18 passes — one add a cycle on any core, so ticks give the clock.
    "mrs x2, cntpct_el0",
    "mov x3, #0",
    "movz x4, #0x4, lsl #16",
    "1:",
    ".rept 64",
    "add x3, x3, #1",
    ".endr",
    "subs x4, x4, #1",
    "b.ne 1b",
    "mrs x5, cntpct_el0",
    "sub x5, x5, x2",
    "str x5, [x0, #8]",
    // Four independent chains, the same number of adds: how wide the core is.
    "mrs x2, cntpct_el0",
    "movz x4, #0x1, lsl #16",
    "2:",
    ".rept 64",
    "add x9, x9, #1",
    "add x10, x10, #1",
    "add x11, x11, #1",
    "add x12, x12, #1",
    ".endr",
    "subs x4, x4, #1",
    "b.ne 2b",
    "mrs x5, cntpct_el0",
    "sub x5, x5, x2",
    "str x5, [x0, #24]",
    "mrs x6, mpidr_el1",
    "str x6, [x0, #16]",
    "movz x7, #0x4521",
    "movk x7, #0x524f, lsl #16",
    "movk x7, #0x5250, lsl #32",
    "movk x7, #0x5833, lsl #48",
    "str x7, [x0]",
    "dsb sy",
    "movz x0, #0x0002",
    "movk x0, #0x8400, lsl #16",
    "smc #0",
    "3:",
    "wfe",
    "b 3b",
);

/// The same two measurements on the calling core (CPU0): (ticks for 2^24 dependent adds, ticks for 2^24 adds in four chains).
pub fn local() -> (u64, u64) {
    let (d, i): (u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {t0}, cntpct_el0",
            "mov {a}, #0",
            "movz {n}, #0x4, lsl #16",
            "1:",
            ".rept 64",
            "add {a}, {a}, #1",
            ".endr",
            "subs {n}, {n}, #1",
            "b.ne 1b",
            "mrs {t1}, cntpct_el0",
            "sub {d}, {t1}, {t0}",
            "mrs {t0}, cntpct_el0",
            "movz {n}, #0x1, lsl #16",
            "2:",
            ".rept 64",
            "add {a}, {a}, #1",
            "add {b}, {b}, #1",
            "add {c}, {c}, #1",
            "add {e}, {e}, #1",
            ".endr",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "mrs {t1}, cntpct_el0",
            "sub {i}, {t1}, {t0}",
            t0 = out(reg) _, t1 = out(reg) _, a = out(reg) _, b = out(reg) _, c = out(reg) _, e = out(reg) _, n = out(reg) _, d = out(reg) d, i = out(reg) i,
        );
    }
    (d, i)
}

unsafe extern "C" {
    fn ferros_x3_probe();
}

/// Start CPU8 on the probe and wait for its report (three seconds at most: powering the cluster up took most of a second the first time). Returns (PSCI's answer, ticks for the dependent chain, ticks for the four chains, its MPIDR) — zeros if it never reported.
pub fn probe() -> (i64, u64, u64, u64) {
    unsafe {
        for i in 0..9 {
            write_volatile((RESULT + i * 8) as *mut u64, 0);
        }
        let r = |n: usize, v: u64| write_volatile((RESULT + n) as *mut u64, v);
        let (mair, tcr, ttbr, sctlr, hcr): (u64, u64, u64, u64, u64);
        core::arch::asm!("mrs {a}, mair_el2", "mrs {b}, tcr_el2", "mrs {c}, ttbr0_el2", "mrs {d}, sctlr_el2", "mrs {e}, hcr_el2", a = out(reg) mair, b = out(reg) tcr, c = out(reg) ttbr, d = out(reg) sctlr, e = out(reg) hcr);
        r(32, mair);
        r(40, tcr);
        r(48, ttbr);
        r(56, sctlr);
        r(64, hcr);
        core::arch::asm!("dsb sy");
        let ret: i64;
        core::arch::asm!("smc #0", inout("x0") PSCI_CPU_ON => ret, in("x1") 0x800u64, in("x2") ferros_x3_probe as usize as u64, in("x3") RESULT as u64);
        if ret != 0 {
            return (ret, 0, 0, 0);
        }
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        while crate::platform::cntpct() - t0 < 3 * f {
            if read_volatile(RESULT as *const u64) == MAGIC {
                return (0, read_volatile((RESULT + 8) as *const u64), read_volatile((RESULT + 24) as *const u64), read_volatile((RESULT + 16) as *const u64));
            }
        }
        (0, 0, 0, 0)
    }
}

// ---------------------------------------------------------------- the hop: the whole (single-core) kernel moves to the X3

/// The hand-over block, in the same non-cacheable page as the probe's (one of the two at a time): CPU0 writes the X3's translation and per-core registers here before CPU_ON (the X3 reads them with its MMU off), the X3 says it is parked, CPU0 says go.
const HOP: usize = RESULT + 0x100;
/// Where CPU0 sleeps once it has handed the service over (see `hand_over`): two instructions in the hand-over page.
const PARK: usize = RESULT + 0x800;
const H_MAIR: usize = 0;
const H_TCR: usize = 8;
const H_TTBR: usize = 16;
const H_SCTLR: usize = 24;
const H_HCR: usize = 32;
const H_VBAR: usize = 40;
const H_CPTR: usize = 48;
const H_STACK: usize = 56;
const H_PARKED: usize = 64;
const H_GO: usize = 72;
const H_STARTED: usize = 80;
/// CNTHCTL_EL2: in VHE, EL0's access to the counters (cachemap.rs sets it on the boot core; a process reading the time on a core without it traps, as the display server's first `now()` did on the X3, 2026-10-22).
const H_CNTHCTL: usize = 88;
const PARKED: u64 = 0x4B52_4150; // "PARK"
const GO: u64 = 0x2121_4F47; // "GO!!"

/// The X3's own stack while it runs the kernel (CPU0 keeps the boot stack, parked on it).
#[repr(C, align(4096))]
struct Stack([u8; 256 << 10]);
static mut X3_STACK: Stack = Stack([0; 256 << 10]);

core::arch::global_asm!(
    ".section .text.x3entry, \"ax\"",
    ".global ferros_x3_entry",
    "ferros_x3_entry:",
    // x0 = the hand-over block. CPU0's translation first (HCR, the table, SCTLR: cachemap.rs's order), then its FP/SIMD trap setting and vectors, then this core's own stack.
    "mov x19, x0",
    "ldr x1, [x19, #32]",
    "msr hcr_el2, x1",
    "isb",
    "ldr x1, [x19, #48]",
    "msr cptr_el2, x1",
    "ldr x1, [x19, #0]",
    "msr mair_el2, x1",
    "ldr x1, [x19, #8]",
    "msr tcr_el2, x1",
    "ldr x1, [x19, #16]",
    "msr ttbr0_el2, x1",
    "isb",
    "dsb ish",
    "tlbi alle2",
    "dsb ish",
    "ic iallu",
    "isb",
    "ldr x1, [x19, #24]",
    "msr sctlr_el2, x1",
    "isb",
    "ldr x1, [x19, #40]",
    "msr vbar_el2, x1",
    "ldr x1, [x19, #88]",
    "msr cnthctl_el2, x1",
    "isb",
    "msr daifset, #0xF",
    "msr spsel, #1",
    "ldr x1, [x19, #56]",
    "mov sp, x1",
    "mov x0, x19",
    "bl ferros_x3_main",
    "1:",
    "wfe",
    "b 1b",
);

unsafe extern "C" {
    fn ferros_x3_entry();
}

/// The boot state the service runs with, carried across the hop (kernel_main moves it in, the X3 moves it out).
static mut HANDOVER: Option<crate::service::Boot> = None;

/// When CPU_ON was asked (counter ticks), and whether it was accepted, for `diag boot`'s record.
pub static LAUNCHED_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Start the X3 early in boot, so its power-up (it has taken most of a second and more) overlaps the rest of the boot: it parks with CPU0's translation and its own stack, and waits for `hand_over`. Nothing else changes if it never answers. Only on CPU0: a kernel already on the X3 (a hot reload there) stays where it is.
pub fn launch() {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    // Whatever an earlier session left in the block is not this boot's: no core is parked until this launch says so.
    unsafe { write_volatile((HOP + H_PARKED) as *mut u64, 0) };
    if mpidr & 0xFFFF != 0 {
        // A reload, already on the X3 (CPU0 is parked for good): the X3's own setup that `ferros_x3_main` would have done.
        crate::killswitch::arm_power_key();
        return;
    }
    unsafe {
        let w = |o: usize, v: u64| write_volatile((HOP + o) as *mut u64, v);
        let (mair, tcr, ttbr, sctlr, hcr, vbar, cptr, cnthctl): (u64, u64, u64, u64, u64, u64, u64, u64);
        core::arch::asm!("mrs {a}, mair_el2", "mrs {b}, tcr_el2", "mrs {c}, ttbr0_el2", "mrs {d}, sctlr_el2", "mrs {e}, hcr_el2", "mrs {f}, vbar_el2", "mrs {g}, cptr_el2", "mrs {h}, cnthctl_el2",
            a = out(reg) mair, b = out(reg) tcr, c = out(reg) ttbr, d = out(reg) sctlr, e = out(reg) hcr, f = out(reg) vbar, g = out(reg) cptr, h = out(reg) cnthctl);
        w(H_CNTHCTL, cnthctl);
        w(H_MAIR, mair);
        w(H_TCR, tcr);
        w(H_TTBR, ttbr);
        w(H_SCTLR, sctlr);
        w(H_HCR, hcr);
        w(H_VBAR, vbar);
        w(H_CPTR, cptr);
        w(H_STACK, (&raw const X3_STACK as usize + (256 << 10)) as u64);
        w(H_PARKED, 0);
        w(H_GO, 0);
        w(H_STARTED, 0);
        core::arch::asm!("dsb sy");
        let ret: i64;
        core::arch::asm!("smc #0", inout("x0") PSCI_CPU_ON => ret, in("x1") 0x800u64, in("x2") ferros_x3_entry as usize as u64, in("x3") HOP as u64);
        if ret == 0 {
            LAUNCHED_AT.store(crate::platform::cntpct(), core::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Whether the X3 is parked and waiting.
pub fn parked() -> bool {
    unsafe { read_volatile((HOP + H_PARKED) as *const u64) == PARKED }
}

/// Give the service to the X3 and park CPU0 for good (interrupts masked, in WFI: its cluster and watchdog stay as they are). Returns the boot state untouched if the X3 is not parked, for CPU0 to run itself.
pub fn hand_over(b: crate::service::Boot) -> crate::service::Boot {
    if !parked() {
        return b;
    }
    unsafe {
        HANDOVER = Some(b);
        core::arch::asm!("dsb sy");
        write_volatile((HOP + H_GO) as *mut u64, GO);
        core::arch::asm!("dsb sy", "sev");
        // Wait for the X3 to take it; if it never does, take the service back.
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        while read_volatile((HOP + H_STARTED) as *const u64) != GO {
            if crate::platform::cntpct() - t0 > f / 4 {
                write_volatile((HOP + H_GO) as *mut u64, 0);
                core::arch::asm!("dsb sy");
                return (*(&raw mut HANDOVER)).take().unwrap();
            }
        }
        core::arch::asm!("msr daifset, #0xF");
        // Parked outside the image, with the MMU off. A RELOAD rewrites every byte of the kernel and zeroes its .bss (the page tables included) while CPU0 sleeps; WFI wakes on any interrupt still aimed at this core whatever the mask, and a core that then fetches through a dead generation's tables and code faults into its old vectors and scribbles on the new kernel (every reload from the X3 hung within seconds, 2026-10-23). So the two instructions it sleeps in live in the hand-over page, which no generation overwrites, and it runs them with no translation under it.
        let park = PARK as *mut u32;
        write_volatile(park, 0xD503_207F); // wfi
        write_volatile(park.add(1), 0x17FF_FFFF); // b .-4
        core::arch::asm!("dsb sy", "ic iallu", "dsb sy", "isb");
        // One asm block from MMU-off to the branch, so nothing touches the stack through a cache the MMU no longer fronts; x9 is scratch (nothing returns to care).
        core::arch::asm!("mrs x9, sctlr_el2", "bic x9, x9, #1", "msr sctlr_el2, x9", "isb", "br {p}", p = in(reg) PARK as u64, options(noreturn));
    }
}

/// The X3, parked: wait for go, then this core's own interrupt setup (the killswitch's redistributor and key routing, the power key) and the service loop — the kernel from here on.
#[unsafe(no_mangle)]
extern "C" fn ferros_x3_main(hop: usize) -> ! {
    unsafe {
        write_volatile((hop + H_PARKED) as *mut u64, PARKED);
        core::arch::asm!("dsb sy");
        loop {
            if read_volatile((hop + H_GO) as *const u64) == GO {
                break;
            }
            core::arch::asm!("wfe");
        }
        let b = (*(&raw mut HANDOVER)).take();
        crate::pmu::el0_on();
        write_volatile((hop + H_STARTED) as *mut u64, GO);
        core::arch::asm!("dsb sy");
        let Some(b) = b else {
            loop {
                core::arch::asm!("wfe");
            }
        };
        crate::killswitch::arm();
        crate::killswitch::arm_power_key();
        // Its clock: the bootloader leaves the X3's cluster at 1164 MHz; 2.91 GHz is its top step (ACPM DVFS, `diag dvfs`). The service loop sleeps in WFI between work, so the clock costs only while something is being done.
        let _ = crate::acpm::dvfs_set(4, 2_910_000);
        crate::checklist::at("service on the X3");
        crate::service::run(b)
    }
}
