//! Armed actions (SYS_IRQ_ARM; docs/KERNEL.md "Armed actions"; the encoding and its check: ferros_layout irqarm): a driver arms up to four register operations on its own granted line, inside one of its own hardware grants, checked once at the arm; `.Lirq` runs them when the line fires, before the line is masked and the driver woken, and never takes the kernel lock.
//! The first user is the camera (camera-full-rate, 2026-11-18): its next PDMA descriptor released inside the frame's blanking whatever holds the lock.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use ferros_layout::irqarm::{Op, ADVANCE, OPS_MOST, TEST_BITS, TEST_EQ, UNMASK, WRITE};

/// Lines, by `IRQS` index (process.rs `IRQ_GUARDED`: room for 2^5).
const LINES: usize = 1 << 5;

/// Per INTID, the line's slot plus one while it is armed (0 not): `.Lirq` tests it with one load and calls `ferros_irq_armed` only for an armed line.
#[unsafe(no_mangle)]
pub static mut __irq_armed: [u8; 1024] = [0; 1024];

/// Whether armed actions run at all (`diag irqarm on|off`, on): off, every line takes the path it took before them, for an A/B.
pub static ON: AtomicBool = AtomicBool::new(true);

/// Per slot: a sequence count, odd while the slot is being written (the interrupt copies the program and keeps it only if the count did not move); the operations armed (0 none); each operation as three words, resolved to absolute addresses at the arm: kind | a << 32, the register, and b (a second register for TEST_EQ, else the operand); the arming process's epoch.
static SEQ: [AtomicU32; LINES] = [const { AtomicU32::new(0) }; LINES];
static N: [AtomicU32; LINES] = [const { AtomicU32::new(0) }; LINES];
static OPS: [[[AtomicU64; 3]; OPS_MOST]; LINES] = [const { [const { [const { AtomicU64::new(0) }; 3] }; OPS_MOST] }; LINES];
static EPOCH: [AtomicU64; LINES] = [const { AtomicU64::new(0) }; LINES];
/// Per slot, for `diag irqs`: runs, runs whose every operation ran (acted), runs a test stopped, runs left because the slot was being rewritten, acted runs that left the line open (UNMASK).
static COUNTS: [[AtomicU64; 5]; LINES] = [const { [const { AtomicU64::new(0) }; 5] }; LINES];

fn rd(a: u64) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: u64, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}

/// Arm line `slot` (INTID `intid`) with `ops`, checked already (ferros_layout irqarm `check`) against the grant at `base`, for the process of `epoch`. Under the kernel lock, as every arm and disarm is: two never race; the interrupt may, and the sequence count covers it.
pub fn arm(slot: usize, intid: u32, base: usize, ops: &[Op], epoch: u64) {
    untake(intid);
    SEQ[slot].fetch_add(1, Ordering::AcqRel);
    for (k, o) in ops.iter().enumerate() {
        let b = if o.op == TEST_EQ { (base + o.b as usize) as u64 } else { o.b as u64 };
        OPS[slot][k][0].store(o.op as u64 | (o.a as u64) << 32, Ordering::Relaxed);
        OPS[slot][k][1].store((base + o.off as usize) as u64, Ordering::Relaxed);
        OPS[slot][k][2].store(b, Ordering::Relaxed);
    }
    N[slot].store(ops.len() as u32, Ordering::Relaxed);
    EPOCH[slot].store(epoch, Ordering::Relaxed);
    SEQ[slot].fetch_add(1, Ordering::Release);
    unsafe { core::ptr::write_volatile(&raw mut __irq_armed[intid as usize], slot as u8 + 1) };
    for c in COUNTS[slot].iter() {
        c.store(0, Ordering::Relaxed);
    }
    runs_reset(None);
}

/// Line `slot` (INTID `intid`) disarmed: the byte first, so `.Lirq` stops calling, then the program.
pub fn disarm(slot: usize, intid: u32) {
    untake(intid);
    unsafe { core::ptr::write_volatile(&raw mut __irq_armed[intid as usize], 0) };
    SEQ[slot].fetch_add(1, Ordering::AcqRel);
    N[slot].store(0, Ordering::Relaxed);
    EPOCH[slot].store(0, Ordering::Relaxed);
    SEQ[slot].fetch_add(1, Ordering::Release);
}

/// Every line the process of `epoch` armed, disarmed: at its end and when it turns a power switch off (no action may touch a block whose clocks are going).
pub fn disarm_epoch(epoch: u64) {
    if epoch == 0 {
        return;
    }
    for (slot, e) in EPOCH.iter().enumerate() {
        if e.load(Ordering::Relaxed) == epoch {
            disarm(slot, crate::process::IRQS[slot].intid);
        }
    }
}

/// Per INTID, 1 from the moment an armed line's interrupt is taken (`ferros_irq_armed`'s first store) until its waiter takes the pending byte (process.rs `irq_pending_take`).
/// Why (2026-11-18, slit-robust, the Pro): SYS_IRQ_WAIT unmasks a line it finds nothing pending on, and an armed line's interrupt is taken, its program run and only then masked and marked pending (a few us of register reads); a wait in that window unmasked the line after `.Lirq` masked it, the still-asserted line fired again at once, and the program ran twice on one DMA_DONE (acted 8 us apart, two descriptors released for one frame: the slit ring one buffer out for the rest of the stream). A wait that finds this set sleeps without unmasking: the pending byte is a few instructions away.
static TAKEN: [core::sync::atomic::AtomicU8; 1024] = [const { core::sync::atomic::AtomicU8::new(0) }; 1024];
/// Whether INTID `intid` has an armed program (process.rs `irq_route`: an armed line follows its waiter's core).
pub fn armed(intid: u32) -> bool {
    // A granted line's INTID, under 1020 (process.rs `IRQS`).
    unsafe { core::ptr::read_volatile(&raw const __irq_armed[intid as usize]) != 0 }
}
/// Whether an armed line's interrupt is taken and not yet handed to its waiter (`TAKEN`).
pub fn taken(intid: u32) -> bool {
    TAKEN[intid as usize].load(Ordering::Relaxed) != 0
}
/// Its waiter took the pending byte (process.rs `irq_pending_take`): the next interrupt marks it again.
pub fn untake(intid: u32) {
    TAKEN[intid as usize].store(0, Ordering::Relaxed);
}

/// The last `RUNS` runs of any armed line, for `diag "irqs runs"` (2026-11-18, slit-robust: a slit stream died with one run stopped by a test and no record of which, or of what the others saw): per run the counter when it ran; the slot, the core (MPIDR Aff1), how it ended (0 acted, 1 + k stopped at operation k, G#FF left mid-arm) and how many words it read; and up to four words its operations read, in order (TEST_BITS one, TEST_EQ two, ADVANCE one: the camera's status, place, index, index).
/// A run that a test stopped reads the registers of the operations it did not reach too, so its record holds what the whole program would have seen (the camera's place and index beside its status).
/// Any core may run one (a line refired on another core), so each takes its own entry by `RUN_N`.
const RUNS: usize = 1 << 6;
static RUN_N: AtomicU64 = AtomicU64::new(0);
static RUN: [[AtomicU64; 4]; RUNS] = [const { [const { AtomicU64::new(0) }; 4] }; RUNS];
/// The last `STOPS` stopped runs, kept apart from `RUN` so a long stream's acted runs never push them out: the same four words.
const STOPS: usize = 1 << 3;
static STOP_N: AtomicU64 = AtomicU64::new(0);
static STOP: [[AtomicU64; 4]; STOPS] = [const { [const { AtomicU64::new(0) }; 4] }; STOPS];
/// Per slot: the counter at its last acted run, and the gaps between acted runs, the most and a histogram by 2^k us (k 0 to 15, the last bin 2^15 us and more): a stream's period, an interrupt taken late (a gap of two periods), or an action run twice for one event (a gap of a few us).
static LAST_ACT: [AtomicU64; LINES] = [const { AtomicU64::new(0) }; LINES];
static GAP_MOST: [AtomicU64; LINES] = [const { AtomicU64::new(0) }; LINES];
static GAP_HIST: [[AtomicU64; 16]; LINES] = [const { [const { AtomicU64::new(0) }; 16] }; LINES];
/// `diag "irqs freeze <us>"` (0 off): `RUN` stops taking entries half its length after the first gap between acted runs longer than this, so the runs around the first late one stay to be read. `FROZEN` holds that run's number plus one (0 none yet).
pub static FREEZE_US: AtomicU64 = AtomicU64::new(0);
static FROZEN: AtomicU64 = AtomicU64::new(0);
/// `diag "irqs freeze short <us>"` (0 off): the same, at the first gap between acted runs shorter than this (an action run twice for one event).
pub static FREEZE_SHORT_US: AtomicU64 = AtomicU64::new(0);
/// At the freeze: the counter, and each core's last crumbs with their counters and its count (platform.rs `CORE_CRUMBS`), cores 0 to 8.
static SNAP_AT: AtomicU64 = AtomicU64::new(0);
const SNAP_W: usize = 2 * crate::platform::CORE_CRUMB_N + 1;
static SNAP: [[AtomicU64; SNAP_W]; 9] = [const { [const { AtomicU64::new(0) }; SNAP_W] }; 9];

/// One run recorded in `RUN` (unless frozen) and, when a test stopped it, in `STOP`.
fn record(t: u64, slot: usize, end: u32, vals: &[u32; 4], nv: usize) {
    let w = [t, slot as u64 | (crate::process::core() as u64) << 8 | (end as u64) << 16 | (nv as u64) << 24, vals[0] as u64 | (vals[1] as u64) << 32, vals[2] as u64 | (vals[3] as u64) << 32];
    let f = FROZEN.load(Ordering::Relaxed);
    let n = RUN_N.load(Ordering::Relaxed);
    if f == 0 || n < f - 1 + (RUNS >> 1) as u64 {
        let i = RUN_N.fetch_add(1, Ordering::Relaxed) as usize & (RUNS - 1);
        for (e, v) in RUN[i].iter().zip(w) {
            e.store(v, Ordering::Relaxed);
        }
    }
    if (1..=OPS_MOST as u32).contains(&end) {
        let i = STOP_N.fetch_add(1, Ordering::Relaxed) as usize & (STOPS - 1);
        for (e, v) in STOP[i].iter().zip(w) {
            e.store(v, Ordering::Relaxed);
        }
    }
}

/// `.Lirq` for an armed line (main.rs `.Lirq_armed`, caller-saved registers saved): its program copied out, kept only if no arm or disarm moved it meanwhile, then run in order, a failed test ending the run. 1 when the run reached UNMASK: `.Lirq_armed` then leaves the line unmasked (`.Lirq_spi_open`); 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn ferros_irq_armed(intid: u64) -> u64 {
    // `__irq_armed` holds only a slot plus one that `arm` wrote, under `LINES`, and the INTID is the GIC's, under 1020 (`.Lirq` sends nothing past it).
    let slot = unsafe { core::ptr::read_volatile(&raw const __irq_armed[intid as usize]) } as usize;
    if slot == 0 || !ON.load(Ordering::Relaxed) {
        return 0;
    }
    // Taken, before the program runs and long before `.Lirq_spi` masks the line and marks it pending: a SYS_IRQ_WAIT that finds nothing pending meanwhile must not unmask it (`taken`).
    TAKEN[intid as usize].store(1, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::SeqCst);
    let t = crate::platform::cntpct();
    let slot = slot - 1;
    let c = &COUNTS[slot];
    c[0].fetch_add(1, Ordering::Relaxed);
    let s = SEQ[slot].load(Ordering::Acquire);
    // `arm` stores at most `OPS_MOST` (the check refused more).
    let n = N[slot].load(Ordering::Relaxed) as usize;
    let mut prog = [[0u64; 3]; OPS_MOST];
    for (k, p) in prog.iter_mut().take(n).enumerate() {
        *p = [OPS[slot][k][0].load(Ordering::Relaxed), OPS[slot][k][1].load(Ordering::Relaxed), OPS[slot][k][2].load(Ordering::Relaxed)];
    }
    core::sync::atomic::fence(Ordering::Acquire);
    let mut vals = [0u32; 4];
    if s & 1 != 0 || SEQ[slot].load(Ordering::Relaxed) != s {
        c[3].fetch_add(1, Ordering::Relaxed);
        record(t, slot, 0xFF, &vals, 0);
        return 0;
    }
    // The words read, in order, for the record: at most two an operation and four kept (the camera's program reads four). WHY the mask: a fifth word (a program of four tests) is still read and tested, only not kept.
    let mut nv = 0usize;
    let mut keep = |v: u32| {
        vals[nv & 3] = v;
        nv += 1;
        v
    };
    let mut stopped = 0u32;
    let mut open = 0u64;
    for (k, &[w, reg, b]) in prog.iter().take(n).enumerate() {
        let a = (w >> 32) as u32;
        let hit = match w as u32 {
            TEST_BITS => keep(rd(reg)) & a == 0,
            TEST_EQ => keep(rd(reg)) != keep(rd(b)),
            // WHY wrapping: a ring index advanced and masked; the mask makes the wrap the ring's own.
            ADVANCE => {
                wr(reg, keep(rd(reg)).wrapping_add(a) & b as u32);
                false
            }
            WRITE => {
                wr(reg, b as u32);
                false
            }
            // A read of the device first: its writes above are posted, and the read returns only after the device has taken them (the same device, in order), so its line has had its source cleared before the EOI.
            UNMASK => {
                rd(reg);
                open = 1;
                false
            }
            _ => false,
        };
        if hit {
            stopped = k as u32 + 1;
            break;
        }
    }
    if stopped != 0 {
        c[2].fetch_add(1, Ordering::Relaxed);
        // What the operations not reached would have read: registers inside the grant the arm checked, read only for the record.
        for &[w, reg, b] in prog.iter().take(n).skip(stopped as usize) {
            match w as u32 {
                TEST_BITS | ADVANCE => {
                    keep(rd(reg));
                }
                TEST_EQ => {
                    keep(rd(reg));
                    keep(rd(b));
                }
                _ => {}
            }
        }
        // WHY the min: `keep` counts past four; four are kept.
        record(t, slot, stopped, &vals, nv.min(4));
        return 0;
    }
    c[1].fetch_add(1, Ordering::Relaxed);
    let last = LAST_ACT[slot].swap(t, Ordering::Relaxed);
    if last != 0 {
        let us = (t - last) * 1_000_000 / crate::platform::cntfrq();
        GAP_MOST[slot].fetch_max(us, Ordering::Relaxed);
        // WHY the min: the histogram has 2^4 bins, a gap of 2^15 us or more lands in the last.
        GAP_HIST[slot][((64 - us.leading_zeros()) as usize).min(15)].fetch_add(1, Ordering::Relaxed);
        let f = FREEZE_US.load(Ordering::Relaxed);
        let short = FREEZE_SHORT_US.load(Ordering::Relaxed);
        if ((f != 0 && us > f) || us < short) && FROZEN.load(Ordering::Relaxed) == 0 {
            FROZEN.store(RUN_N.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
            // What every core was doing (its last two crumbs and when), and the scheduler trace and the kernel's crumb ring stopped here (devsched `FROZEN`): the core that took this run late had its interrupts masked meanwhile, and its last crumbs say where.
            snapshot(t);
            crate::devsched::FROZEN.store(true, Ordering::Relaxed);
        }
    }
    // WHY the min: as above. An acted run that leaves the line open records end G#40.
    record(t, slot, (open as u32) << 6, &vals, nv.min(4));
    c[4].fetch_add(open, Ordering::Relaxed);
    open
}

/// Every core's last crumbs kept, once (the first freeze: this one's, or devsched's at a late run of a device's waiter, `diag "dev freeze"`), at counter `t`, for `diag "irqs runs"`.
pub fn snapshot(t: u64) {
    if SNAP_AT.load(Ordering::Relaxed) != 0 {
        return;
    }
    SNAP_AT.store(t, Ordering::Relaxed);
    for (s, c) in SNAP.iter().zip(crate::platform::CORE_CRUMBS.iter()) {
        for (a, b) in s.iter().zip(c.iter()) {
            a.store(b.load(Ordering::Relaxed), Ordering::Relaxed);
        }
    }
}

/// `diag "irqs runs"`: the recorded runs, oldest first, each as its counter in us (the counter's own, to set beside a driver's timeline), us after the one before, the slot and core, how it ended, and the words read; then the stopped runs kept apart.
pub fn runs_report(out: &mut alloc::vec::Vec<u8>) {
    let f = crate::platform::cntfrq();
    let us = |t: u64| t * 1_000_000 / f;
    let n = RUN_N.load(Ordering::Relaxed) as usize;
    let fz = FROZEN.load(Ordering::Relaxed);
    out.extend_from_slice(alloc::format!("armed runs: A#{n} recorded{}; now A#{} us on the counter; the last A#{} (us on the counter, us after the one before, slot.core, end: 0 acted, k+1 stopped at op k, G#FF mid-arm; the words read):\n", if fz != 0 { alloc::format!(", frozen after run A#{} (FREEZE_US A#{})", fz - 1, FREEZE_US.load(Ordering::Relaxed)) } else { alloc::string::String::new() }, us(crate::platform::cntpct()), n.min(RUNS)).as_bytes());
    let say = |out: &mut alloc::vec::Vec<u8>, e: &[AtomicU64; 4], prev: u64| {
        let w: [u64; 4] = core::array::from_fn(|k| e[k].load(Ordering::Relaxed));
        let nv = (w[1] >> 24 & 0xFF) as usize;
        let v = [w[2] as u32, (w[2] >> 32) as u32, w[3] as u32, (w[3] >> 32) as u32];
        // WHY wrapping: the first entry said has no entry before it (prev 0), and its "after" is then the counter itself, read as such.
        out.extend_from_slice(alloc::format!(" {}:{}:{}.{}:{:X}", us(w[0]), us(w[0].wrapping_sub(prev)), w[1] & 0xFF, w[1] >> 8 & 0xFF, w[1] >> 16 & 0xFF).as_bytes());
        for x in v.iter().take(nv) {
            out.extend_from_slice(alloc::format!(":{x:X}").as_bytes());
        }
        w[0]
    };
    let k0 = n.saturating_sub(RUNS);
    let mut prev = 0;
    for k in k0..n {
        prev = say(out, &RUN[k & (RUNS - 1)], prev);
        if (k - k0) & 7 == 7 {
            out.push(b'\n');
        }
    }
    let at = SNAP_AT.load(Ordering::Relaxed);
    if at != 0 {
        out.extend_from_slice(b"\nat the freeze, each core's last crumbs, oldest first (crumb G#, us before the freeze's run):");
        let n = crate::platform::CORE_CRUMB_N;
        for (k, s) in SNAP.iter().enumerate() {
            let w: [u64; SNAP_W] = core::array::from_fn(|j| s[j].load(Ordering::Relaxed));
            // Signed: a crumb's counter read on another core may be past this run's (written after the freeze).
            let before = |x: u64| (at as i64 - x as i64) * 1_000_000 / f as i64;
            out.extend_from_slice(alloc::format!("\n c{k}:").as_bytes());
            let c = w[2 * n] as usize;
            for j in 0..n {
                let i = (c + j) & (n - 1);
                if w[2 * i + 1] != 0 {
                    out.extend_from_slice(alloc::format!(" G#{:X}@{}", w[2 * i], before(w[2 * i + 1])).as_bytes());
                }
            }
        }
    }
    let m = STOP_N.load(Ordering::Relaxed) as usize;
    out.extend_from_slice(alloc::format!("\nstopped runs: A#{m}, the last A#{}:", m.min(STOPS)).as_bytes());
    let mut prev = 0;
    for k in m.saturating_sub(STOPS)..m {
        prev = say(out, &STOP[k & (STOPS - 1)], prev);
    }
    out.push(b'\n');
}

/// The record cleared (`diag "irqs runs reset"`, and at each arm): its runs, stops, gaps and the freeze.
fn runs_reset(slot: Option<usize>) {
    if slot.is_none() {
        RUN_N.store(0, Ordering::Relaxed);
        STOP_N.store(0, Ordering::Relaxed);
        FROZEN.store(0, Ordering::Relaxed);
        SNAP_AT.store(0, Ordering::Relaxed);
    }
    for s in 0..LINES {
        if slot.is_some_and(|x| x != s) {
            continue;
        }
        LAST_ACT[s].store(0, Ordering::Relaxed);
        GAP_MOST[s].store(0, Ordering::Relaxed);
        for h in GAP_HIST[s].iter() {
            h.store(0, Ordering::Relaxed);
        }
    }
}
pub fn runs_reset_all() {
    runs_reset(None);
}

/// For `diag irqs`: each armed line, its operations and its counts.
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    out.extend_from_slice(alloc::format!("armed actions {}:", if ON.load(Ordering::Relaxed) { "on" } else { "off (diag irqarm on)" }).as_bytes());
    let mut any = false;
    // WHY the min: the table has room for `LINES`, the kernel names fewer lines; only those have a name to say.
    for slot in 0..LINES.min(crate::process::IRQS.len()) {
        let n = N[slot].load(Ordering::Relaxed);
        let c = &COUNTS[slot];
        if n == 0 && c[0].load(Ordering::Relaxed) == 0 {
            continue;
        }
        any = true;
        out.extend_from_slice(alloc::format!(" {} (A#{n} ops) runs A#{}, acted A#{}, stopped by a test A#{}, left mid-arm A#{}, left open A#{}; acted to acted A#{} us the most, by 2^k us:", crate::process::IRQS[slot].name, c[0].load(Ordering::Relaxed), c[1].load(Ordering::Relaxed), c[2].load(Ordering::Relaxed), c[3].load(Ordering::Relaxed), c[4].load(Ordering::Relaxed), GAP_MOST[slot].load(Ordering::Relaxed)).as_bytes());
        for (k, h) in GAP_HIST[slot].iter().enumerate() {
            let v = h.load(Ordering::Relaxed);
            if v != 0 {
                out.extend_from_slice(alloc::format!(" {k}:{v}").as_bytes());
            }
        }
        out.push(b';');
    }
    if !any {
        out.extend_from_slice(b" none armed");
    }
    out.push(b'\n');
}
