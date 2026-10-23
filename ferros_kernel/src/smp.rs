//! The other cores (KERNEL.md "Cores"). The kernel runs on the X3 (x3.rs); this brings up the four Cortex-A715s (MPIDR Aff1 4-7) as workers: each enters through `ferros_core_entry` with the X3's translation and per-core registers from its own hand-over block, sets up its own GIC redistributor (killswitch::arm_core), and runs `idle_loop` — a slice to whatever thread is runnable, `wfe` when none is (an `enqueue` anywhere does `sev`). One kernel lock (process.rs) orders every touch of the shared tables.
//!
//! A RELOAD rewrites the kernel under every core: `stop_all` asks the workers to switch themselves off (PSCI CPU_OFF) and waits until they have, before the copy stub runs; the next generation's `launch_all` turns them on again. The A510s (0-3) stay off: CPU0 is parked for good after the hop, and nothing waits on them yet (the plan has the low-rate servers there once pinning exists).
//!
//! `FERROS_SMP=0` at build time keeps everything on the X3.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const PSCI_CPU_ON: u64 = 0xC400_0003;
const PSCI_CPU_OFF: u64 = 0x8400_0002;
/// The workers: their MPIDR affinity values (Aff1 in bits 8-15).
pub const WORKERS: [u64; 4] = [0x400, 0x500, 0x600, 0x700];
/// Each worker's hand-over block, in the non-cacheable result page after the X3's (x3.rs HOP at +G#100, its PARK at +G#800): 128 bytes a core, the X3's layout (mair, tcr, ttbr, sctlr, hcr, vbar, cptr, stack, -, -, -, cnthctl).
const BLOCKS: usize = crate::x3::RESULT + 0x200;
const BLOCK: usize = 0x80;
const H_MAIR: usize = 0;
const H_TCR: usize = 8;
const H_TTBR: usize = 16;
const H_SCTLR: usize = 24;
const H_HCR: usize = 32;
const H_VBAR: usize = 40;
const H_CPTR: usize = 48;
const H_STACK: usize = 56;
const H_CNTHCTL: usize = 88;

#[repr(C, align(4096))]
struct Stack([u8; 128 << 10]);
static mut STACKS: [Stack; 4] = [const { Stack([0; 128 << 10]) }; 4];

/// Bit k: worker k is up (its idle loop running). `ON_RESULT[k]`: what CPU_ON returned.
pub static UP: AtomicU32 = AtomicU32::new(0);
pub static ON_RESULT: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// Set to ask every worker to switch itself off; `OFF` counts the ones that have.
static STOP: AtomicU32 = AtomicU32::new(0);
static OFF: AtomicU32 = AtomicU32::new(0);

core::arch::global_asm!(
    ".section .text.smpentry, \"ax\"",
    ".global ferros_core_entry",
    "ferros_core_entry:",
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
    "bl ferros_core_main",
    "1:",
    "wfe",
    "b 1b",
);

unsafe extern "C" {
    fn ferros_core_entry();
}

/// Start every worker that is not up, from the core the kernel runs on (its registers are what they copy).
pub fn launch_all() {
    if option_env!("FERROS_SMP") == Some("0") {
        return;
    }
    STOP.store(0, Ordering::Relaxed);
    OFF.store(0, Ordering::Relaxed);
    let (mair, tcr, ttbr, sctlr, hcr, vbar, cptr, cnthctl): (u64, u64, u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {a}, mair_el2", "mrs {b}, tcr_el2", "mrs {c}, ttbr0_el2", "mrs {d}, sctlr_el2", "mrs {e}, hcr_el2", "mrs {f}, vbar_el2", "mrs {g}, cptr_el2", "mrs {h}, cnthctl_el2",
            a = out(reg) mair, b = out(reg) tcr, c = out(reg) ttbr, d = out(reg) sctlr, e = out(reg) hcr, f = out(reg) vbar, g = out(reg) cptr, h = out(reg) cnthctl);
    }
    for (k, &aff) in WORKERS.iter().enumerate() {
        if UP.load(Ordering::Acquire) & (1 << k) != 0 {
            continue;
        }
        let b = BLOCKS + k * BLOCK;
        unsafe {
            let w = |o: usize, v: u64| write_volatile((b + o) as *mut u64, v);
            w(H_MAIR, mair);
            w(H_TCR, tcr);
            w(H_TTBR, ttbr);
            w(H_SCTLR, sctlr);
            w(H_HCR, hcr);
            w(H_VBAR, vbar);
            w(H_CPTR, cptr);
            w(H_CNTHCTL, cnthctl);
            w(H_STACK, (&raw const STACKS as usize + (k + 1) * (128 << 10)) as u64);
            core::arch::asm!("dsb sy");
            let ret: i64;
            core::arch::asm!("smc #0", inout("x0") PSCI_CPU_ON => ret, in("x1") aff, in("x2") ferros_core_entry as usize as u64, in("x3") b as u64);
            ON_RESULT[k].store(ret as u64, Ordering::Relaxed);
        }
    }
}

/// A worker, up: its own GIC, then slices for as long as there is work; `wfe` otherwise. Leaves through CPU_OFF when `stop_all` asks.
#[unsafe(no_mangle)]
extern "C" fn ferros_core_main(block: usize) -> ! {
    let k = ((block - BLOCKS) / BLOCK) & 3;
    crate::killswitch::arm_core();
    UP.fetch_or(1 << k, Ordering::AcqRel);
    let mut last_look = 0u64;
    loop {
        if STOP.load(Ordering::Acquire) != 0 {
            UP.fetch_and(!(1 << k), Ordering::AcqRel);
            OFF.fetch_add(1, Ordering::AcqRel);
            unsafe {
                core::arch::asm!("msr daifset, #0xF");
                core::arch::asm!("smc #0", inout("x0") PSCI_CPU_OFF => _);
            }
            loop {
                unsafe { core::arch::asm!("wfe") };
            }
        }
        // Work queued, or a millisecond since the last look (sleepers' deadlines are found under the lock): a slice; else `wfe` — an `enqueue` anywhere does `sev`, and the event stream ticks every 333 µs.
        let f = crate::platform::cntfrq();
        let due = crate::platform::cntpct().wrapping_sub(last_look) > f / 1000;
        if crate::process::RUNNABLE_N.load(Ordering::Acquire) != 0 || due {
            last_look = crate::platform::cntpct();
            if !crate::process::run_slice() {
                unsafe { core::arch::asm!("wfe") };
            }
        } else {
            unsafe { core::arch::asm!("wfe") };
        }
    }
}

/// Ask every worker to switch itself off and wait for them (a quarter second at most): before a RELOAD rewrites the kernel.
pub fn stop_all() {
    let want = UP.load(Ordering::Acquire).count_ones();
    if want == 0 {
        return;
    }
    STOP.store(1, Ordering::Release);
    unsafe { core::arch::asm!("dsb sy", "sev") };
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while OFF.load(Ordering::Acquire) < want && crate::platform::cntpct().wrapping_sub(t0) < f / 4 {
        core::hint::spin_loop();
    }
    // CPU_OFF takes a moment after the count: a little more, so no worker is mid-exit when the image goes.
    let t1 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t1) < f / 100 {
        core::hint::spin_loop();
    }
}

/// How many workers are up, and each one's CPU_ON result, for DIAG.
pub fn report() -> (u32, [u64; 4]) {
    let mut r = [0u64; 4];
    for (k, v) in r.iter_mut().enumerate() {
        *v = ON_RESULT[k].load(Ordering::Relaxed);
    }
    (UP.load(Ordering::Relaxed), r)
}

#[allow(dead_code)]
fn _unused(_: u64) -> u64 {
    unsafe { read_volatile(BLOCKS as *const u64) }
}
