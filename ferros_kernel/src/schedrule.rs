//! The scheduler's choices that are plain arithmetic over what the kernel lock lets it see (process.rs gathers it): where a thread kept after a yield joins the run queue, which core `make_way` asks, and how long the next slice on a core runs.
//! No state, no locks, no kernel types, so the host tests them as the kernel runs them (`rustc --test ferros_kernel/src/schedrule.rs`; KERNEL.md Order item 13).

/// Where a thread kept after a yield joins the run queue (`enqueue_second`), given for each thread queued now, head first, whether it was queued at the front (`front`).
/// `fifo` (2026-11-24): behind every thread waiting at the front, ahead of those queued at the back; off, just behind the head, as before.
/// Never at the head while the queue holds one: the head is the thread the core was asked for, which the core must still pick.
pub fn kept_slot(front: impl Iterator<Item = bool>, len: usize, fifo: bool) -> usize {
    // WHY the min: with the queue empty the slot is the front, and an insert past the end panics.
    let head = len.min(1);
    if !fifo {
        return head;
    }
    let mut at = len;
    for (i, f) in front.enumerate() {
        if !f {
            at = i;
            break;
        }
    }
    // WHY the max: a head queued at the back (the asked-for thread's place taken by none at the front) still stays ahead.
    at.max(head)
}

/// A core `make_way` could ask, as process.rs sees it under the lock.
#[derive(Clone, Copy)]
pub struct Candidate {
    pub core: usize,
    /// Its thread is of the woken thread's own process.
    pub kin: bool,
    /// Its thread is in its device interrupt's run, inside the budget: never asked.
    pub device_run: bool,
    /// Its thread's process holds a caller's reply token (`midcall`).
    pub midcall: bool,
    /// How long its thread has run in this turn (counter ticks).
    pub run: u64,
}

/// The core `make_way` asks to yield for a woken thread, if any: the longest-running thread at least `least` into its turn (2^-11 s past the woken thread's spent credit), never a device thread in its run, never one mid-call while `midcall` is on, never one of the woken thread's own process unless `kin` allows it.
/// `kin` (2026-11-24): a thread of the woken one's own process is asked only when that process holds every core in `cands` (no other process's thread, device run or mid-call one among them) and the wake came from a core the woken thread may not run on (`from_outside`).
pub fn way_choice(cands: &[Candidate], least: u64, midcall: bool, kin: bool, from_outside: bool) -> Option<usize> {
    let mut best = None;
    let mut longest = least;
    let mut kin_best = None;
    let mut kin_longest = least;
    let mut others = false;
    for c in cands {
        if c.device_run {
            others |= !c.kin;
            continue;
        }
        if midcall && c.midcall {
            others |= !c.kin;
            continue;
        }
        if c.kin {
            if c.run >= kin_longest {
                kin_longest = c.run;
                kin_best = Some(c.core);
            }
            continue;
        }
        others = true;
        if c.run >= longest {
            longest = c.run;
            best = Some(c.core);
        }
    }
    if best.is_none() && kin && from_outside && !others {
        return kin_best;
    }
    best
}

/// The ticks the next slice on a core runs: what is left of a slice a yield interrupted there (`end`, its compare value, 0 none) while it has time left, else a fresh `fresh`.
pub fn slice_left(end: u64, now: u64, fresh: u64) -> u64 {
    if end > now { end - now } else { fresh }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(core: usize, kin: bool, run: u64) -> Candidate {
        Candidate { core, kin, device_run: false, midcall: false, run }
    }

    #[test]
    fn kept_goes_behind_the_front() {
        // Head (the asked-for thread), two waiting at the front, two at the back.
        let q = [true, true, true, false, false];
        assert_eq!(kept_slot(q.iter().copied(), 5, true), 3);
        assert_eq!(kept_slot(q.iter().copied(), 5, false), 1);
        // All at the front: the end.
        assert_eq!(kept_slot([true, true].iter().copied(), 2, true), 2);
        // A head queued at the back stays the head.
        assert_eq!(kept_slot([false, false].iter().copied(), 2, true), 1);
        // Empty: the front.
        assert_eq!(kept_slot(core::iter::empty(), 0, true), 0);
    }

    #[test]
    fn kept_twice_keeps_order() {
        // The 2026-11-23 sequence: lumis.3 woken to the front, a device thread pushed ahead of it, a displaced compute thread kept.
        // Before: [dev, kept, lumis.3]; now [dev, lumis.3, kept], and a second displaced one behind the first.
        let q = [true, true];
        assert_eq!(kept_slot(q.iter().copied(), 2, true), 2);
        let q = [true, true, true];
        assert_eq!(kept_slot(q.iter().copied(), 3, true), 3);
    }

    #[test]
    fn way_longest_other_process() {
        let c = [cand(4, false, 900), cand(5, false, 2000), cand(6, true, 5000)];
        assert_eq!(way_choice(&c, 500, true, true, true), Some(5));
    }

    #[test]
    fn way_kin_only_when_the_process_holds_every_core() {
        // Lumis's four compute threads on the four A715s, the camera thread woken from an A510.
        let all = [cand(4, true, 3000), cand(5, true, 9000), cand(6, true, 700), cand(7, true, 4000)];
        assert_eq!(way_choice(&all, 500, true, true, true), Some(5));
        // Woken from one of those cores (a parallel section handing work along): not asked.
        assert_eq!(way_choice(&all, 500, true, true, false), None);
        // The rule off: as before, none.
        assert_eq!(way_choice(&all, 500, true, false, true), None);
        // Another process on one core, too short to ask: no kin ask either.
        let mixed = [cand(4, true, 3000), cand(5, false, 100)];
        assert_eq!(way_choice(&mixed, 500, true, true, true), None);
        // A device thread of another process in its run: not every core is the process's.
        let dev = [cand(4, true, 3000), Candidate { core: 5, kin: false, device_run: true, midcall: false, run: 50 }];
        assert_eq!(way_choice(&dev, 500, true, true, true), None);
        // None of them past the least: none.
        assert_eq!(way_choice(&[cand(4, true, 400)], 500, true, true, true), None);
    }

    #[test]
    fn way_never_midcall() {
        let server = Candidate { core: 6, kin: false, device_run: false, midcall: true, run: 580 };
        assert_eq!(way_choice(&[server], 488, true, true, true), None);
        assert_eq!(way_choice(&[server], 488, false, true, true), Some(6));
        // A mid-call server is another process's: it keeps a kin ask from being made.
        assert_eq!(way_choice(&[server, cand(4, true, 9000)], 488, true, true, true), None);
        // And a longer-running other thread is still asked.
        assert_eq!(way_choice(&[server, cand(5, false, 3000)], 488, true, true, true), Some(5));
    }

    #[test]
    fn slice_rest_or_fresh() {
        assert_eq!(slice_left(1000, 400, 245760), 600);
        assert_eq!(slice_left(400, 400, 245760), 245760);
        assert_eq!(slice_left(0, 400, 245760), 245760);
    }
}
