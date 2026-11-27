//! The other cores (KERNEL.md "Cores"). The kernel runs on the X3 (x3.rs); this brings up the four Cortex-A715s (MPIDR Aff1 4-7) and the four Cortex-A510s (Aff1 0-3) as workers: each enters through `ferros_core_entry` with the X3's translation and per-core registers from its own hand-over block, sets up its own GIC redistributor (killswitch::arm_core), and runs `idle_loop` — a slice to whatever thread is runnable, `wfe` when none is (an `enqueue` anywhere does `sev`). One kernel lock (process.rs) orders every touch of the shared tables.
//!
//! The A510s (2026-11-18, for Lumis's camera intake) take only threads whose mask names them (`ferros_sys::CORES_LITTLE`): the default mask stays the A715s, so nothing moves there unless it pins. An idle A510 looks for work only when the run queue holds a thread it may run (process.rs `RUNNABLE_LITTLE`), so with nothing pinned there it never takes the kernel lock. CPU0 is among them: the hop parks it with PSCI CPU_OFF (x3.rs `hand_over`), so it is free once the X3 runs the kernel; after the jump from Android the boot core is core 3, parked the same way, and Android's cores 0-2 wait on the hv's mailbox.
//!
//! A RELOAD rewrites the kernel under every core: `stop_all` asks the workers to switch themselves off (PSCI CPU_OFF) and waits until they have, before the copy stub runs; the next generation's `launch_all` turns them on again.
//!
//! `FERROS_SMP=0` at build time keeps everything on the X3; `FERROS_LITTLE=0` leaves the A510s off.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const PSCI_CPU_ON: u64 = 0xC400_0003;
const PSCI_CPU_OFF: u64 = 0x8400_0002;
const PSCI_AFFINITY_INFO: u64 = 0xC400_0004;
/// The workers by MPIDR Aff1 (the core's number; Aff0 is 0 on every core here), in the order they are started: the A715s first, as before the A510s came up.
pub const WORKERS: [usize; 8] = [4, 5, 6, 7, 0, 1, 2, 3];
/// The A715s and the A510s as core masks (bit = Aff1), as process.rs and ferros_sys name them.
pub const BIG_WORKERS: u32 = 0x00F0;
pub const LITTLE: u32 = 0x000F;
/// Each worker's hand-over block, in the non-cacheable result page after the X3's (x3.rs HOP at +G#100, its PARK at +G#800): 128 bytes a core, the X3's layout (mair, tcr, ttbr, sctlr, hcr, vbar, cptr, stack, -, -, -, cnthctl).
/// Core c's block is block c XOR 4: the A715s keep +G#200 to +G#380, where they were, and the A510s take +G#400 to +G#580.
const BLOCKS: usize = crate::x3::RESULT + 0x200;
const BLOCK: usize = 0x80;
fn block_of(c: usize) -> usize {
    BLOCKS + (c ^ 4) * BLOCK
}
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
/// One stack a core, indexed by Aff1 (the X3's own is x3.rs `X3_STACK`).
static mut STACKS: [Stack; 8] = [const { Stack([0; 128 << 10]) }; 8];

/// Times each worker left its idle sleep (DIAG IDLE_WAKES_Cc), by Aff1: the measure of how often an idle core is disturbed.
pub static IDLE_WAKES: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Counter ticks each worker has spent asleep in its idle `wfe` (or `wfi`), added as each sleep ends, by Aff1: the adaptive clock's measure of how busy each cluster is (service.rs `governor`).
pub static IDLE_TICKS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Counter ticks each worker has spent in `run_slice` passes, by Aff1: the A510s' load for their clock (service.rs `gov_share`). An idle A510 sleeps until an event, often for long stretches, and a window with no sleep ended in it would read as all busy by `IDLE_TICKS`; this counts only the time it ran threads.
pub static BUSY_TICKS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// When the `run_slice` pass each worker is in began (0: none), so a run still going is counted as it goes (`busy`): a thread that runs whole slices back to back ends a pass only every slice (2^-7 s and more), and the governor's 2^-6 s windows saw it as a share of about 2/3, under saturation, so the A510s took half a second to leave their floor (the first corebench run, 2026-11-18).
pub static RUN_AT: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Worker c's busy ticks up to now: the passes ended (`BUSY_TICKS`) and the one going on (`RUN_AT`). Read in that order, a pass ending between the two reads is missed until the next read (the caller keeps its marks from going back).
pub fn busy(c: usize, now: u64) -> u64 {
    let done = BUSY_TICKS[c].load(Ordering::Acquire);
    let at = RUN_AT[c].load(Ordering::Acquire);
    // WHY `now > at`: `now` was read before `RUN_AT`, so a pass that began in between has `at` past `now`, and its partial time would wrap; it is counted at the next read instead.
    done + if at != 0 && now > at { now - at } else { 0 }
}
/// Bit c: worker core c (MPIDR Aff1) is up, its idle loop running. `ON_RESULT[c]`: what CPU_ON returned for it (G#FFFF_FFFF_FFFF_FFFC, ALREADY_ON, also for an A510 that never read off: `cpu_on`).
pub static UP: AtomicU32 = AtomicU32::new(0);
pub static ON_RESULT: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// The workers `launch_all` last started (CPU_ON answered 0), for a caller waiting on them to come up.
static LAUNCHED: AtomicU32 = AtomicU32::new(0);
/// Whether `launch_all` starts the A510s (`diag smp little on|off`; on unless built with FERROS_LITTLE=0).
pub static LITTLE_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// Bit c set asks worker c to switch itself off; `OFF` counts the ones that have.
static STOP: AtomicU32 = AtomicU32::new(0);
static OFF: AtomicU32 = AtomicU32::new(0);

core::arch::global_asm!(
    ".section .text.smpentry, \"ax\"",
    ".global ferros_core_entry",
    "ferros_core_entry:",
    // Entry marks in this core's arrival word (G#FD60_0B80 + core * 8, the low byte G#11-G#17; Rust's note_arrival overwrites it with the counter): the eighth jump of 2026-11-03 lost worker core 7 after the hv's branch and before any Rust, with nothing in the hv's trap report. Before the MMU is on the stores go straight to DRAM; after, each is cleaned to it.
    // PSTATE as a fresh CPU_ON leaves it, first: a core parked by the jump from Android keeps the PSTATE it trapped with, PAN set among it (Linux runs with PAN), and once HCR.E2H and TGE are set PAN applies at EL2, so the first load from the hand-over block (in the DMA pool, EL0-accessible) after the MMU is on took a permission fault into vectors that were still the hv's and not executable under these tables (the tenth jump of 2026-11-03: cores 5, 7 and the X3 stopped at mark G#15). An eret to the next instruction at EL2h, D A I F masked, resets PAN, UAO, TCO, SSBS, DIT and the rest.
    "adr x10, 2f",
    "msr elr_el2, x10",
    "mov x10, #0x3C9",
    "msr spsr_el2, x10",
    "eret",
    "2:",
    "mov x19, x0",
    "mrs x9, mpidr_el1",
    "ubfx x9, x9, #8, #4",
    "mov x10, #0x0B80",
    "movk x10, #0xFD60, lsl #16",
    "add x9, x10, x9, lsl #3",
    "mov x10, #0x11",
    "str x10, [x9]",
    "dsb sy",
    "ldr x1, [x19, #32]",
    "msr hcr_el2, x1",
    "isb",
    "mov x10, #0x12",
    "str x10, [x9]",
    "dsb sy",
    "ldr x1, [x19, #48]",
    "msr cptr_el2, x1",
    "ldr x1, [x19, #0]",
    "msr mair_el2, x1",
    "ldr x1, [x19, #8]",
    "msr tcr_el2, x1",
    "ldr x1, [x19, #16]",
    "msr ttbr0_el2, x1",
    "isb",
    "mov x10, #0x13",
    "str x10, [x9]",
    "dsb sy",
    "dsb ish",
    "tlbi alle2",
    "dsb ish",
    "ic iallu",
    "isb",
    "mov x10, #0x14",
    "str x10, [x9]",
    "dsb sy",
    "ldr x1, [x19, #24]",
    "msr sctlr_el2, x1",
    "isb",
    "mov x10, #0x15",
    "str x10, [x9]",
    "dc civac, x9",
    "dsb sy",
    "ldr x1, [x19, #40]",
    "msr vbar_el2, x1",
    "ldr x1, [x19, #88]",
    "msr cnthctl_el2, x1",
    "isb",
    "mov x10, #0x16",
    "str x10, [x9]",
    "dc civac, x9",
    "dsb sy",
    "msr daifset, #0xF",
    "msr spsel, #1",
    "ldr x1, [x19, #56]",
    "mov sp, x1",
    "mov x10, #0x17",
    "str x10, [x9]",
    "dc civac, x9",
    "dsb sy",
    "mov x0, x19",
    "bl ferros_core_main",
    "1:",
    "wfe",
    "b 1b",
);

unsafe extern "C" {
    fn ferros_core_entry();
}

/// PSCI's ALREADY_ON (-4): CPU_ON's answer for a core that is on, and `cpu_on`'s for an A510 that never read off.
const ALREADY_ON: i64 = -4;

/// PSCI AFFINITY_INFO for core `c` (MPIDR Aff1): 0 on, 1 off, 2 on pending.
fn affinity_info(c: usize) -> i64 {
    let st: i64;
    unsafe { core::arch::asm!("smc #0", inout("x0") PSCI_AFFINITY_INFO => st, in("x1") (c as u64) << 8, in("x2") 0u64) };
    st
}

/// Start worker core `c` (MPIDR Aff1) from the core the kernel runs on (its registers are what it copies): the hand-over block, then PSCI CPU_ON (or the hv's mailbox after the jump, platform::start_core). Returns CPU_ON's answer (0: started).
fn cpu_on(c: usize) -> i64 {
    // An A510 not parked on the hv's mailbox is started by PSCI only once EL3 says it is off (AFFINITY_INFO 1), a quarter second at most, else it is left off.
    // WHY the wait: the core the boot ran on (CPU0 cold, core 3 after the jump) switches itself off in x3.rs's park a few instructions after the X3 takes the hand-over, and the service's first `launch_all` can come before that lands. CPU_ON of a core EL3 still holds on answers ALREADY_ON at best, and it hung EL3 on one jump in two when it was an Android core's (2026-11-01/02), so it is never asked of one.
    if c < 4 && !crate::platform::on_mailbox(c) {
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        while affinity_info(c) != 1 {
            if crate::platform::cntpct().wrapping_sub(t0) > f >> 2 {
                return ALREADY_ON;
            }
            core::hint::spin_loop();
        }
    }
    let (mair, tcr, ttbr, sctlr, hcr, vbar, cptr, cnthctl): (u64, u64, u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {a}, mair_el2", "mrs {b}, tcr_el2", "mrs {c}, ttbr0_el2", "mrs {d}, sctlr_el2", "mrs {e}, hcr_el2", "mrs {f}, vbar_el2", "mrs {g}, cptr_el2", "mrs {h}, cnthctl_el2",
            a = out(reg) mair, b = out(reg) tcr, c = out(reg) ttbr, d = out(reg) sctlr, e = out(reg) hcr, f = out(reg) vbar, g = out(reg) cptr, h = out(reg) cnthctl);
    }
    let b = block_of(c);
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
        w(H_STACK, (&raw const STACKS as usize + (c + 1) * (128 << 10)) as u64);
        core::arch::asm!("dsb sy");
        crate::platform::start_core(c, (c as u64) << 8, ferros_core_entry as usize as u64, b as u64)
    }
}

/// A one-shot job on core 7 (an A715) during the boot, before `launch_all` (2026-11-01: KOHI's firmware TRNG words, 329 ms of SMCs, gathered beside the boot core's own work instead of in front of the first process). The core starts with the boot core's tables, runs the job with every interrupt masked, and switches itself off; `launch_all` waits for that before bringing it up as a worker. False when the core did not start: the job never runs, and whoever waits on it does the work itself.
static EARLY: AtomicU64 = AtomicU64::new(0);
/// 0 none, 1 started, 2 the job has run and the core is switching itself off.
static EARLY_STATE: AtomicU32 = AtomicU32::new(0);
const EARLY_CORE: usize = 7;

pub fn early(job: fn()) -> bool {
    if option_env!("FERROS_SMP") == Some("0") {
        return false;
    }
    EARLY.store(job as usize as u64, Ordering::Release);
    EARLY_STATE.store(1, Ordering::Release);
    let ret = cpu_on(EARLY_CORE);
    ON_RESULT[EARLY_CORE].store(ret as u64, Ordering::Relaxed);
    if ret != 0 {
        EARLY.store(0, Ordering::Release);
        EARLY_STATE.store(0, Ordering::Release);
        return false;
    }
    true
}

/// Whether `launch_all` starts the A510s now: not built out, not switched off by hand, and the kernel on the X3 (with the kernel on CPU0, akita's or a FERROS_X3=0 build's, its own cluster's clock is the A510s', and they stay off as before).
fn little_wanted() -> bool {
    option_env!("FERROS_LITTLE") != Some("0") && LITTLE_ON.load(Ordering::Relaxed) && crate::process::core() == 8
}

/// Start every worker that is not up, from the core the kernel runs on (its registers are what they copy). The early job's core first finishes and is off (a second at most each).
pub fn launch_all() {
    if option_env!("FERROS_SMP") == Some("0") {
        return;
    }
    crate::platform::crumb(0xCB);
    STOP.store(0, Ordering::Relaxed);
    OFF.store(0, Ordering::Relaxed);
    if EARLY_STATE.load(Ordering::Acquire) != 0 {
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        while EARLY_STATE.load(Ordering::Acquire) == 1 && crate::platform::cntpct().wrapping_sub(t0) < f {
            core::hint::spin_loop();
        }
        let t1 = crate::platform::cntpct();
        loop {
            if affinity_info(EARLY_CORE) == 1 || crate::platform::cntpct().wrapping_sub(t1) > f {
                break;
            }
            core::hint::spin_loop();
        }
        EARLY_STATE.store(0, Ordering::Release);
    }
    let me = crate::process::core();
    let little = little_wanted();
    let mut launched = UP.load(Ordering::Acquire);
    for &c in WORKERS.iter() {
        if c == me || UP.load(Ordering::Acquire) & (1 << c) != 0 || (c < 4 && !little) {
            continue;
        }
        let ret = cpu_on(c);
        ON_RESULT[c].store(ret as u64, Ordering::Relaxed);
        if ret == 0 {
            launched |= 1 << c;
        }
    }
    LAUNCHED.store(launched, Ordering::Release);
    crate::platform::crumb(0xCC);
}

/// The workers the last `launch_all` left up or started: what a caller waits for to see them all up.
pub fn launched() -> u32 {
    LAUNCHED.load(Ordering::Acquire)
}

/// A worker, up: its own GIC, then slices for as long as there is work; `wfe` otherwise. Leaves through CPU_OFF when `stop` asks.
#[unsafe(no_mangle)]
extern "C" fn ferros_core_main(_block: usize) -> ! {
    crate::platform::note_arrival();
    crate::platform::cpu_inform_clear();
    crate::platform::note_arrival_step(2);
    // This core's number (MPIDR Aff1): the index of everything per core.
    let c = crate::process::core();
    // An A510: it takes only threads whose mask names it (process.rs `RUNNABLE_LITTLE`), wakes for events and not for deadlines, and leaves the background wipe to the A715s.
    let little = c < 4;
    // Started for the boot's one-shot job (`early`): run it, say so, and switch off; `launch_all` starts this core again as a worker.
    if c == EARLY_CORE && EARLY_STATE.load(Ordering::Acquire) == 1 {
        let job = EARLY.swap(0, Ordering::AcqRel);
        if job != 0 {
            let f: fn() = unsafe { core::mem::transmute::<usize, fn()>(job as usize) };
            f();
        }
        EARLY_STATE.store(2, Ordering::Release);
        unsafe {
            core::arch::asm!("msr daifset, #0xF");
            core::arch::asm!("smc #0", inout("x0") PSCI_CPU_OFF => _);
        }
        loop {
            unsafe { core::arch::asm!("wfe") };
        }
    }
    // This core's cache type into the weakest of all (cachemap `CTR_ALL`), before it runs any process code.
    let _ = crate::cachemap::ctr_note();
    crate::platform::worker_step(1);
    crate::killswitch::arm_core();
    crate::platform::worker_step(2);
    crate::pmu::el0_on();
    crate::platform::worker_step(3);
    UP.fetch_or(1 << c, Ordering::AcqRel);
    let mut last_look = 0u64;
    let mut steps = 0u32;
    loop {
        if steps < 3 {
            steps += 1;
            crate::platform::worker_step(3 + steps as u64);
        }
        // A profiler run on: this core's timer started (prof.rs).
        crate::prof::poll();
        // `diag keyreg` asks core 4 to probe its own registers (keyreg.rs).
        crate::keyreg::serve(c);
        // The ira and wairua into this core's registers when a sealed copy is meant for it (keys.rs): before any slice runs here.
        crate::platform::worker_step(9);
        crate::keys::serve();
        crate::platform::worker_step(10);
        // The kernel's core (or the lock's holder) silent past half the watchdog's window: where it is, kept for the post-mortem (platform::stall_check). An idle worker wakes for its own deadlines and kicks at least, so a stall that keeps every worker idle is still seen.
        crate::platform::stall_check();
        // A core stuck in an access to a PCIe link's window ended by that link going down (pcie_guard.rs): from here too, as the stuck core may be the kernel's own.
        crate::pcie_guard::release_stuck();
        if STOP.load(Ordering::Acquire) & (1 << c) != 0 {
            // The keys cleared and this core marked as holding none before it goes off: power-off loses them anyway, and a worker that came back still marked as holding failed its first check every time (four check failures at each dark-to-lit switch, 2026-11-13 on the 8a). The kernel's core holds them meanwhile.
            crate::keys::wipe_for_restart();
            UP.fetch_and(!(1 << c), Ordering::AcqRel);
            OFF.fetch_add(1, Ordering::AcqRel);
            unsafe {
                core::arch::asm!("msr daifset, #0xF");
                core::arch::asm!("smc #0", inout("x0") PSCI_CPU_OFF => _);
            }
            loop {
                unsafe { core::arch::asm!("wfe") };
            }
        }
        // A reason to take the lock — work queued, an interrupt fired, or a sleeper's deadline passed — else sleep in `wfi` until one: the EL2 timer armed for the next deadline (a second at most), and SGI 1 from an `enqueue` or `.Lirq` anywhere (process::kick). No event stream here: an idle worker wakes for events, not for the clock.
        // An A510 looks only for a queued thread it may run: the A715s and the X3 take the interrupts and the deadlines, and the wake they queue kicks this core. With nothing pinned to the A510s they never take the lock, so the A715s' lock and its waits are as before (devsched's measures of the camera's frame thread are made against them).
        unsafe { core::arch::asm!("msr daifset, #2") };
        let now = crate::platform::cntpct();
        let irq = unsafe { core::ptr::read_volatile(&raw const crate::process::__irq_any) } != 0;
        let next = crate::process::NEXT_WAKE.load(Ordering::Acquire);
        let queued = if little { crate::process::RUNNABLE_LITTLE.load(Ordering::Acquire) } else { crate::process::RUNNABLE_N.load(Ordering::Acquire) } != 0;
        // A fruitless look (the only runnable thread is pinned elsewhere) sleeps like any other pass: without this the four workers spun on the lock whenever the X3's render thread was queued (2026-10-23).
        if (queued || (!little && (irq || now >= next))) && {
            last_look = now;
            unsafe { core::arch::asm!("msr daifclr, #2") };
            crate::platform::worker_step(8);
            RUN_AT[c].store(now, Ordering::Release);
            let ran = crate::process::run_slice();
            // Every pass counts, a fruitless one too (its microseconds were the kernel's work on this core), so the count with `RUN_AT` never goes back.
            BUSY_TICKS[c].fetch_add(crate::platform::cntpct().wrapping_sub(now), Ordering::AcqRel);
            RUN_AT[c].store(0, Ordering::Release);
            ran
        } {
            continue;
        }
        // Nothing to run: a frame given back wiped and freed (frames.rs `reclaim_step`, since 2026-11-18: no longer under the kernel lock at a process's end), or a frame of the background wipe (frames.rs, after a jump once a person is in), or a free frame zeroed ahead of the takes (frames.rs `clean_step`), in place of the sleep, IRQs open meanwhile; the loop then looks for work again before the next frame. Not on an A510: the frames stay the A715s' work, as before the A510s came up.
        if !little {
            unsafe { core::arch::asm!("msr daifclr, #2") };
            if crate::frames::reclaim_step() || crate::frames::wipe_step() || crate::frames::clean_step() {
                continue;
            }
            unsafe { core::arch::asm!("msr daifset, #2") };
        }
        // The deadline the sleep is armed for: the next thread's on an A715; on an A510 none (a second, the most it waits), since its threads' deadlines are noticed elsewhere and their wake kicks it.
        let next = if little { u64::MAX } else { next };
        if option_env!("FERROS_IDLE_WFI").is_some() {
            // Under test (FERROS_IDLE_WFI=1 builds). IRQs are masked from the look above to the `wfi`: a kick (SGI 1) that lands in between stays pending and ends the `wfi` at once instead of being taken and forgotten — a touch call once waited the whole 7.8 ms tick that way (2026-10-23). `wfi` wakes on a pending interrupt whatever the mask; it is taken when the mask lifts after.
            let f = crate::platform::cntfrq();
            let wait = next.saturating_sub(now).min(f);
            unsafe {
                core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfi", "msr cnthp_ctl_el2, xzr", "isb", "msr daifclr, #2", t = in(reg) wait, c = out(reg) _);
            }
            IDLE_WAKES[c].fetch_add(1, Ordering::Relaxed);
            IDLE_TICKS[c].fetch_add(crate::platform::cntpct().wrapping_sub(now), Ordering::Relaxed);
        } else {
            // The EL2 timer armed at the next thread deadline, as the kernel's core arms its own `wfe` (service.rs): an idle worker otherwise noticed a sleeper's deadline only at its event stream's next tick (2.7 ms) or a kick, and with the X3 rendering nothing else did (2026-11-17, deadline wakes 590 us late on average during a Photon scroll). Taken at EL2 the tick only stops the timer (.Lirq_tick). WHY the min: no deadline reads u64::MAX, and TVAL is a signed 32-bit count, so a wait past 2^31 ticks would read as one already passed and spin; a second is the most it waits, a kick or an interrupt ends it sooner. WHY saturating: the deadline may have passed since the look above (run_slice found nothing runnable after waking it): the timer then fires at once and the loop looks again.
            let wait = next.saturating_sub(crate::platform::cntpct()).min(crate::platform::cntfrq());
            unsafe { core::arch::asm!("msr daifclr, #2") };
            if crate::process::OLD.load(Ordering::Relaxed) {
                unsafe { core::arch::asm!("wfe") };
            } else {
                unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfe", "msr cnthp_ctl_el2, xzr", "isb", t = in(reg) wait, c = out(reg) _) };
            }
            IDLE_WAKES[c].fetch_add(1, Ordering::Relaxed);
            IDLE_TICKS[c].fetch_add(crate::platform::cntpct().wrapping_sub(now), Ordering::Relaxed);
        }
        let _ = last_look;
    }
}

/// Ask the workers in `mask` (bit = Aff1) that are up to switch themselves off and wait for them (a second at most): every one before a RELOAD rewrites the kernel, the A510s alone for `diag smp little off`.
pub fn stop(mask: u32) {
    let up = UP.load(Ordering::Acquire) & mask;
    if up == 0 {
        return;
    }
    crate::platform::crumb(0xC9);
    // Lines routed to a worker (devsched `follow` and `core`) back to the kernel's core first: one routed to a core that is off is never delivered.
    crate::process::irq_routes_home();
    STOP.fetch_or(up, Ordering::AcqRel);
    // Woken by SGI as well as `sev`: a worker asleep in `wfi` hears only the interrupt (two of four never turned off before a reload and came back ALREADY_ON, 2026-10-23).
    unsafe {
        core::arch::asm!("dsb sy", "sev");
        core::arch::asm!("msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) (1u64 << 40) | (1u64 << 24));
    }
    // Until EL3 says each is off (AFFINITY_INFO = 1), a second at most: the count a worker bumps on its way out says it is leaving, not that it has left.
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    for &c in WORKERS.iter() {
        if up & (1 << c) == 0 {
            continue;
        }
        loop {
            if affinity_info(c) == 1 || crate::platform::cntpct().wrapping_sub(t0) > f {
                break;
            }
            core::hint::spin_loop();
        }
    }
    let _ = OFF.load(Ordering::Acquire);
    // Again with the workers off: a wait made on a worker while it was leaving may have routed its line there.
    crate::process::irq_routes_home();
    crate::platform::crumb(0xCA);
}

/// Every worker off: before a RELOAD rewrites the kernel, a restart, the dark profile.
pub fn stop_all() {
    stop(u32::MAX);
}

/// Which workers are up (bit = Aff1), and each core's CPU_ON result by Aff1, for DIAG.
pub fn report() -> (u32, [u64; 8]) {
    let mut r = [0u64; 8];
    for (c, v) in r.iter_mut().enumerate() {
        *v = ON_RESULT[c].load(Ordering::Relaxed);
    }
    (UP.load(Ordering::Relaxed), r)
}

#[allow(dead_code)]
fn _unused(_: u64) -> u64 {
    unsafe { read_volatile(BLOCKS as *const u64) }
}

/// Which workers have come up (bit = Aff1), for the jump's escape dump.
pub fn up_mask() -> u32 {
    UP.load(Ordering::Acquire)
}
