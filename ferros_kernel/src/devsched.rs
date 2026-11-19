//! Device threads and the kernel lock, measured and served (KERNEL.md "Device threads", 2026-11-16).
//!
//! The owner, 2026-11-16: "Measure the kernel lock, see if we can dedicate it something, would be nice to truly max out the sensor readout."
//! The camera's front end must be answered every frame by the driver's frame thread, woken on SPI 479, or its stream stops for good.
//! This module holds what measures that path (every hold of the kernel lock by its syscall, every granted line's interrupt to the run of its thread) and the switches that serve it (process.rs does the scheduling; this keeps the state and the reports).
//!
//! The switches, each `diag "dev ..."` at run time so one kernel measures with and without:
//! - `first`: a thread woken by its own device's interrupt is picked ahead of every other runnable thread, and when no core it may run on is idle, one running a compute thread is asked to yield (SGI 3) and its thread goes back to the front behind it.
//! - `follow`: a granted line is routed to the core its waiting thread waits from (its pinned core when it has one), so the interrupt lands where the thread will run, preempts what runs there, and needs no wake across cores.
//! - `core <k>`: core k runs device threads only (a thread that has waited on an interrupt), and device threads run only there.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

/// `diag "dev first on|off"`: on by default (the owner, 2026-11-16, after 10 of 10 camera streams at 96 a second died without it and every one with it ran).
/// Only a thread the kernel woke for its own device's interrupt, with front credit left, is made urgent; its run is protected from yields for `FIRST_BUDGET_US`, then runs on without that priority.
pub static FIRST: AtomicBool = AtomicBool::new(true);
/// The urgent run's budget (2^10 us): the camera's answer measured 100 to 340 us, touch and the display's vsync tens.
pub const FIRST_BUDGET_US: u64 = 1 << 10;
/// The flash log has said once this boot that a run went past its budget.
pub static BUDGET_SAID: AtomicBool = AtomicBool::new(false);
/// `diag "dev follow on|off"`.
pub static FOLLOW: AtomicBool = AtomicBool::new(false);
/// `diag "dev core <k>|off"`: the dedicated core as a mask (0: none).
pub static CORE: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

// ---------------------------------------------------------------- the kernel lock

/// What the holder is doing, set right after the take by the syscall path (`ferros_el0_trap`): bit 63 set, the syscall in bits 0-7, the low byte of its x1 (the sub-call of SYS_DMA, SYS_POWER, SYS_STORE...) in bits 8-15, the thread in bits 16-31; 0 for every other hold (the service loop, a slice's pick, a preemption).
pub static WHAT: AtomicU64 = AtomicU64::new(0);
/// Holds at or past this many counter ticks go into the long-hold ring and the trace (`diag "lock long <us>"`; 0 until the first hold sets it from the counter's rate: 2^-13 s, 122 us).
pub static LONG_TICKS: AtomicU64 = AtomicU64::new(0);
/// The last holds past `LONG_TICKS`, each [taken at (counter), held (ticks), site (process.rs `site_word`), what (`WHAT`)]; `LONG_N` counts them all.
pub const LONG_HOLDS: usize = 1 << 8;
pub static LONG: [[AtomicU64; 4]; LONG_HOLDS] = [const { [const { AtomicU64::new(0) }; 4] }; LONG_HOLDS];
pub static LONG_N: AtomicU64 = AtomicU64::new(0);
/// Every hold by its length: bucket b holds [2^b, 2^(b+1)) ticks (b = 0 also 0 and 1 tick).
pub static HIST: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
/// Syscall holds by syscall number: [holds, ticks in all, the longest, the longest's x1 low byte].
pub const NRS: usize = 1 << 6;
pub static BY_NR: [[AtomicU64; 4]; NRS] = [const { [const { AtomicU64::new(0) }; 4] }; NRS];
/// Holds by anything else, by site (`site_word`), the first `SITES` sites seen: [site, holds, ticks in all, the longest].
pub const SITES: usize = 1 << 5;
pub static BY_SITE: [[AtomicU64; 4]; SITES] = [const { [const { AtomicU64::new(0) }; 4] }; SITES];
/// The alarm: holds at or past 2^-7 s (7.8 ms, the old long-hold mark) since boot, and how many of them the service loop has written to the flash log (`alarm_lines`).
pub static ALARMS: AtomicU64 = AtomicU64::new(0);
static ALARMS_LOGGED: AtomicU64 = AtomicU64::new(0);
/// The last alarm hold: [taken at, held, site, what].
pub static ALARM_LAST: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// One hold of the kernel lock ended (process.rs `unlock`, still holding it): counted by length, by syscall or site, into the ring when long, and the alarm past 2^-7 s. True when it was long (the caller traces it).
pub fn hold(at: u64, held: u64, site: u64) -> bool {
    let f = crate::platform::cntfrq();
    let mut long = LONG_TICKS.load(Relaxed);
    if long == 0 {
        long = f >> 13;
        LONG_TICKS.store(long, Relaxed);
    }
    let what = WHAT.load(Relaxed);
    // The bucket by the top bit, branchless: 63 - clz(held | 1) is 0..=63; holds past 2^31 ticks (87 s, past any watchdog) fold into the last bucket.
    HIST[(63 - (held | 1).leading_zeros() as usize) & 31].fetch_add(1, Relaxed);
    if what >> 63 != 0 {
        let b = &BY_NR[(what & 0xFF) as usize & (NRS - 1)];
        b[0].fetch_add(1, Relaxed);
        b[1].fetch_add(held, Relaxed);
        if held > b[2].load(Relaxed) {
            b[2].store(held, Relaxed);
            b[3].store(what >> 8 & 0xFF, Relaxed);
        }
    } else {
        // The site's row: the first free or matching one; a site past the table's room is counted in the histogram only.
        for s in &BY_SITE {
            let w = s[0].load(Relaxed);
            if w == site || w == 0 {
                s[0].store(site, Relaxed);
                s[1].fetch_add(1, Relaxed);
                s[2].fetch_add(held, Relaxed);
                s[3].fetch_max(held, Relaxed);
                break;
            }
        }
    }
    if held >= f >> 7 {
        ALARMS.fetch_add(1, Relaxed);
        for (a, v) in ALARM_LAST.iter().zip([at, held, site, what]) {
            a.store(v, Relaxed);
        }
    }
    if held < long {
        return false;
    }
    let i = LONG_N.fetch_add(1, Relaxed) as usize % LONG_HOLDS;
    for (a, v) in LONG[i].iter().zip([at, held, site, what]) {
        a.store(v, Relaxed);
    }
    true
}

/// `diag "lock reset"`: the hold counts zeroed (the alarm's count and the ring's records stay).
pub fn lock_reset() {
    HIST.iter().chain(BY_NR.iter().flatten()).chain(BY_SITE.iter().flatten()).for_each(|a| a.store(0, Relaxed));
    // The storage locks' measures with them (process.rs `Rank`).
    for r in crate::process::RANKED {
        r.reset();
    }
}

/// The syscall names `lock_report` prints (process.rs's numbers).
fn nr_name(nr: u64) -> &'static str {
    const N: [&str; 48] = [
        "exit", "call", "recv", "reply", "mmio_read", "mmio_write", "entropy", "dma", "input", "sleep", "spawn", "set_pref", "facts", "store", "thread_spawn", "thread_exit", "wait", "wake", "irq_wait", "irq_ack", "pin", "light", "power", "ira", "unlock", "pt", "sensors", "status", "pet", "block", "vault", "tracks", "control", "controls", "detail", "zone", "lang",
        "claim", "bell", "not_started", "arc", "offer", "caller", "notify", "thread_name", "irq_arm", "panic", "mailbox",
    ];
    N.get(nr as usize).copied().unwrap_or("?")
}

/// A site word as text (`serv* line A#1279`).
fn site_text(w: u64) -> alloc::string::String {
    let tag: alloc::string::String = ((w >> 24) as u32).to_le_bytes().iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '?' }).collect();
    alloc::format!("{tag}* line A#{}", w & 0xFF_FFFF)
}

/// A hold's "what" as text: the syscall, its x1's low byte and the thread's name, or the site alone.
fn what_text(what: u64, site: u64) -> alloc::string::String {
    if what >> 63 != 0 {
        alloc::format!("syscall {} (A#{}, x1 G#{:X}) by {}", nr_name(what & 0xFF), what & 0xFF, what >> 8 & 0xFF, alloc::string::String::from_utf8_lossy(&crate::process::thread_name_of((what >> 16 & 0xFFFF) as usize)))
    } else {
        site_text(site)
    }
}

/// `diag lock`: every hold since the last reset by length, the syscalls and sites that held it in all and at most, then the long holds, oldest first.
pub fn lock_report(out: &mut alloc::vec::Vec<u8>) {
    let f = crate::platform::cntfrq();
    let us = |t: u64| t * 1_000_000 / f;
    out.extend_from_slice(alloc::format!("lock holds by length (us from):").as_bytes());
    for (b, h) in HIST.iter().enumerate() {
        let n = h.load(Relaxed);
        if n != 0 {
            out.extend_from_slice(alloc::format!(" >=A#{}:A#{n}", ((1u128 << b) * 1_000_000 / f as u128) as u64).as_bytes());
        }
    }
    out.extend_from_slice(b"\nsyscalls holding it (holds, us in all, longest us and its x1):\n");
    for (nr, b) in BY_NR.iter().enumerate() {
        let n = b[0].load(Relaxed);
        if n != 0 {
            out.extend_from_slice(alloc::format!("  {:<12} A#{n}, A#{} us, longest A#{} us (x1 G#{:X})\n", nr_name(nr as u64), us(b[1].load(Relaxed)), us(b[2].load(Relaxed)), b[3].load(Relaxed)).as_bytes());
        }
    }
    out.extend_from_slice(b"other holders (site: holds, us in all, longest us):\n");
    for s in &BY_SITE {
        let w = s[0].load(Relaxed);
        if w != 0 {
            out.extend_from_slice(alloc::format!("  {}: A#{}, A#{} us, longest A#{} us\n", site_text(w), s[1].load(Relaxed), us(s[2].load(Relaxed)), us(s[3].load(Relaxed))).as_bytes());
        }
    }
    let n = LONG_N.load(Relaxed) as usize;
    out.extend_from_slice(alloc::format!("long holds (A#{} us or more): A#{n} in all, the last {}:\n", us(LONG_TICKS.load(Relaxed)), n.min(LONG_HOLDS)).as_bytes());
    // WHY saturating: fewer holds than the ring keeps start at the first; past it, the ring's oldest.
    for j in n.saturating_sub(LONG_HOLDS)..n {
        let h = &LONG[j % LONG_HOLDS];
        out.extend_from_slice(alloc::format!("  t{} A#{} us: {}\n", h[0].load(Relaxed) / (f / 1000), us(h[1].load(Relaxed)), what_text(h[3].load(Relaxed), h[2].load(Relaxed))).as_bytes());
    }
    out.extend_from_slice(alloc::format!("alarms (A#{} us or more) since boot: A#{}\n", us(f >> 7), ALARMS.load(Relaxed)).as_bytes());
    // The storage locks under the kernel lock (process.rs `Rank`, 2026-11-18): each one's holds and longest, and the waits for it a core made holding the kernel lock, which every other core may have waited out too.
    out.extend_from_slice(b"storage locks (rank order under the kernel lock):\n");
    for r in crate::process::RANKED {
        out.extend_from_slice(r.report().as_bytes());
    }
}

/// The service loop's pass: an alarm hold since the last pass goes to the flash log, the last of them with what held it (one line a pass, so a burst of them costs one line).
pub fn alarm_lines() {
    let n = ALARMS.load(Relaxed);
    let logged = ALARMS_LOGGED.load(Relaxed);
    if n == logged {
        return;
    }
    ALARMS_LOGGED.store(n, Relaxed);
    let f = crate::platform::cntfrq();
    let a: [u64; 4] = core::array::from_fn(|i| ALARM_LAST[i].load(Relaxed));
    crate::flashlog::line(&alloc::format!("lock held long: A#{} holds of 2^-7 s or more since the last line, the last t{} for A#{} us: {}", n - logged, a[0] / (f / 1000), a[1] * 1_000_000 / f, what_text(a[3], a[2])));
}

// ---------------------------------------------------------------- interrupt to run

/// Per granted line (process.rs `IRQS` index): [0] runs of its waiter after an interrupt, [1] ticks from the interrupt to the run in all, [2] the longest, [3] ticks from the interrupt to the wake (a core noticed it) in all, [4] the longest of those, [5] runs on the core the interrupt landed on, [6] waits that found it already pending (the thread came back late), [7] ticks those interrupts had waited in all, [8] the longest of those, [9] preemptions asked for it (`first`).
pub const LAT_WORDS: usize = 10;
pub static LAT: [[AtomicU64; LAT_WORDS]; 32] = [const { [const { AtomicU64::new(0) }; LAT_WORDS] }; 32];
/// Per line, the interrupt to the run by length: bucket b holds [2^b, 2^(b+1)) ticks.
pub static LAT_HIST: [[AtomicU64; 32]; 32] = [const { [const { AtomicU64::new(0) }; 32] }; 32];
/// Per line, the cores its interrupts landed on, the cores that noticed them, and the cores its waiter ran on (bit = core).
pub static LANDED: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
pub static NOTICED: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
pub static RAN: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

/// The trace's freeze (`diag "sched freeze <intid> <us>"`): the first run of that line's waiter later than that stops the scheduler trace, so `diag sched` shows what led to it. 0: no freeze armed.
pub static FREEZE_INTID: AtomicUsize = AtomicUsize::new(0);
pub static FREEZE_TICKS: AtomicU64 = AtomicU64::new(0);
pub static FROZEN: AtomicBool = AtomicBool::new(false);

/// The waiter of line `i` (IRQS index) runs on core `c` at `now`: the interrupt came at `at` on core `landed`, was noticed at `woke` on core `noticed`. True when the trace freezes now.
pub fn ran(i: usize, intid: usize, at: u64, woke: u64, now: u64, landed: usize, noticed: usize, c: usize) -> bool {
    // WHY saturating: the stamps come from other cores' counter reads, which nothing orders against this one's; a wrap would put 2^64 ticks in the longest.
    let (total, notice) = (now.saturating_sub(at), woke.saturating_sub(at));
    let l = &LAT[i];
    l[0].fetch_add(1, Relaxed);
    l[1].fetch_add(total, Relaxed);
    l[2].fetch_max(total, Relaxed);
    l[3].fetch_add(notice, Relaxed);
    l[4].fetch_max(notice, Relaxed);
    l[5].fetch_add((landed == c) as u64, Relaxed);
    LAT_HIST[i][(63 - (total | 1).leading_zeros() as usize) & 31].fetch_add(1, Relaxed);
    LANDED[i].fetch_or(1 << (landed & 15), Relaxed);
    NOTICED[i].fetch_or(1 << (noticed & 15), Relaxed);
    RAN[i].fetch_or(1 << (c & 15), Relaxed);
    let fz = FREEZE_INTID.load(Relaxed);
    let freeze = fz != 0 && fz == intid && total >= FREEZE_TICKS.load(Relaxed) && !FROZEN.swap(true, Relaxed);
    // What every core was doing up to this late run (irqarm.rs `snapshot`, said by `diag "irqs runs"`).
    if freeze {
        crate::irqarm::snapshot(now);
    }
    freeze
}

/// Per line: its waiter woken behind the run queue for a spent front credit (process.rs `enqueue_woken`), and the most credit spent at one of those wakes (ticks).
pub static SPENT: [[AtomicU64; 2]; 32] = [const { [const { AtomicU64::new(0) }; 2] }; 32];
pub fn spent(i: usize, credit: u64) {
    SPENT[i][0].fetch_add(1, Relaxed);
    SPENT[i][1].fetch_max(credit, Relaxed);
}

/// A wait on line `i` found its interrupt already pending, `age` ticks old: the thread was not waiting when it came.
pub fn late(i: usize, age: u64) {
    let l = &LAT[i];
    l[6].fetch_add(1, Relaxed);
    l[7].fetch_add(age, Relaxed);
    l[8].fetch_max(age, Relaxed);
}

/// Waits for the kernel lock by a thread running for its device's interrupt (process.rs `lock`, `Thread::on_irq`): [waits, ticks in all, the longest, the longest's holder site, its what], and the waits by length.
pub static IRQ_LOCK: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
pub static IRQ_LOCK_HIST: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

/// A device thread in its interrupt's run waited `waited` ticks for the lock, behind the hold at `site` doing `what`.
pub fn irq_lock_wait(waited: u64, site: u64, what: u64) {
    IRQ_LOCK[0].fetch_add(1, Relaxed);
    IRQ_LOCK[1].fetch_add(waited, Relaxed);
    if waited > IRQ_LOCK[2].load(Relaxed) {
        IRQ_LOCK[2].store(waited, Relaxed);
        IRQ_LOCK[3].store(site, Relaxed);
        IRQ_LOCK[4].store(what, Relaxed);
    }
    IRQ_LOCK_HIST[(63 - (waited | 1).leading_zeros() as usize) & 31].fetch_add(1, Relaxed);
}

/// `diag "dev reset"` (and `sched reset`): the interrupt measures zeroed, the trace thawed.
pub fn lat_reset() {
    LAT.iter().flatten().chain(LAT_HIST.iter().flatten()).chain(LANDED.iter()).chain(NOTICED.iter()).chain(RAN.iter()).chain(IRQ_LOCK.iter()).chain(IRQ_LOCK_HIST.iter()).chain(SPENT.iter().flatten()).for_each(|a| a.store(0, Relaxed));
    FROZEN.store(false, Relaxed);
}

/// `diag dev`: the switches, then each line that woke a thread: interrupt to run (mean, longest, by length), interrupt to notice, the cores it landed on, noticed on and ran on, the runs on the landing core, the late waits.
pub fn lat_report(out: &mut alloc::vec::Vec<u8>, names: &[(&str, u32)]) {
    let f = crate::platform::cntfrq();
    let us = |t: u64| t * 1_000_000 / f;
    let dc = CORE.load(Relaxed);
    out.extend_from_slice(alloc::format!("dev: first {}, follow {}, core {}; trace {}\n", on(FIRST.load(Relaxed)), on(FOLLOW.load(Relaxed)), if dc == 0 { alloc::string::String::from("none") } else { alloc::format!("A#{}", dc.trailing_zeros()) }, if FROZEN.load(Relaxed) { "frozen" } else { "running" }).as_bytes());
    let n = IRQ_LOCK[0].load(Relaxed);
    if n != 0 {
        out.extend_from_slice(alloc::format!("lock waits in interrupt runs: A#{n}, mean A#{} us, longest A#{} us behind {}; by length (us from):", us(IRQ_LOCK[1].load(Relaxed)) / n, us(IRQ_LOCK[2].load(Relaxed)), what_text(IRQ_LOCK[4].load(Relaxed), IRQ_LOCK[3].load(Relaxed))).as_bytes());
        for (b, h) in IRQ_LOCK_HIST.iter().enumerate() {
            let c = h.load(Relaxed);
            if c != 0 {
                out.extend_from_slice(alloc::format!(" >=A#{}:A#{c}", ((1u128 << b) * 1_000_000 / f as u128) as u64).as_bytes());
            }
        }
        out.push(b'\n');
    }
    for (i, (name, intid)) in names.iter().enumerate() {
        let l = &LAT[i];
        let n = l[0].load(Relaxed);
        let late = l[6].load(Relaxed);
        if n == 0 && late == 0 {
            continue;
        }
        let mean = |s: u64, n: u64| if n == 0 { 0 } else { us(s) / n };
        out.extend_from_slice(alloc::format!("{name} (INTID A#{intid}): A#{n} runs, interrupt to run mean A#{} us, longest A#{}; to the wake mean A#{}, longest A#{}; on the landing core A#{}; landed G#{:X}, noticed G#{:X}, ran G#{:X}; late waits A#{late} (waited mean A#{} us, longest A#{}); yields asked A#{}; woken behind the queue for a spent credit A#{} (credit up to A#{} us)\n  by length (us from):", mean(l[1].load(Relaxed), n), us(l[2].load(Relaxed)), mean(l[3].load(Relaxed), n), us(l[4].load(Relaxed)), l[5].load(Relaxed), LANDED[i].load(Relaxed), NOTICED[i].load(Relaxed), RAN[i].load(Relaxed), mean(l[7].load(Relaxed), late), us(l[8].load(Relaxed)), l[9].load(Relaxed), SPENT[i][0].load(Relaxed), us(SPENT[i][1].load(Relaxed))).as_bytes());
        for (b, h) in LAT_HIST[i].iter().enumerate() {
            let c = h.load(Relaxed);
            if c != 0 {
                out.extend_from_slice(alloc::format!(" >=A#{}:A#{c}", ((1u128 << b) * 1_000_000 / f as u128) as u64).as_bytes());
            }
        }
        out.push(b'\n');
    }
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}
