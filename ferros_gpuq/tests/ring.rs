//! The queue's protocol and its command-stream words, on the host.

use ferros_gpuq::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn words(n: usize) -> Vec<AtomicU64> {
    (0..n).map(|_| AtomicU64::new(0)).collect()
}

/// The GPU service's run words before the queue (payloads/gpu `Compute::prepare_grid`, as the device ran them from 2026-11-04 to 2026-11-20), for the same dispatch: `job_words` without stamps must be them exactly.
fn old_words(spd: u64, tsd: u64, fau: u64, nu: u32, groups: (u32, u32), max_threads: u32, done: u64) -> Vec<u64> {
    let (axis, inc) = if groups.1 > 1 { (1u64, 1u64) } else if 16 * groups.0 >= max_threads { (0, (max_threads / 16) as u64) } else { (2, 1) };
    vec![
        0x2200_0000_0000_0001u64,
        0x1C00_0000_0000_0003,
        0x1C00_0001_0000_0000,
        mv48(0, 0),
        mv32(8, fau as u32),
        mv32(9, (fau >> 32) as u32 | nu << 24),
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
    ]
}

#[test]
fn words_without_stamps_are_the_old_ones() {
    for (groups, mt) in [((1u32, 1u32), 1024u32), ((1024, 1), 1024), ((84, 2992), 1024), ((24576, 1), 768), ((3, 1), 1024)] {
        let d = Dispatch { spd: 0x3100_0000, tsd: 0x3100_0040, fau: 0x3100_0080, uniforms: 7, groups, max_threads: mt, stamps: None, sync: 0x3200_0200, value: 1 };
        let mut out = [0u64; JOB_WORDS_MOST];
        let n = job_words(&d, &mut out);
        assert_eq!(&out[..n], &old_words(d.spd, d.tsd, d.fau, 7, groups, mt, d.sync)[..], "{groups:?}");
    }
}

/// A field of a command-stream word, as genxml lays it out (start bit, size).
fn field(w: u64, start: u32, size: u32) -> u64 {
    (w >> start) & ((1u64 << size) - 1)
}

#[test]
fn stamps_are_store_state_timestamps() {
    let d = Dispatch { spd: 0x3100_0400, tsd: 0x3100_0040, fau: 0x8000_1040, uniforms: 3, groups: (256, 1), max_threads: 1024, stamps: Some(0x8000_1240), sync: 0x3200_0240, value: 0x1234_5678 };
    let mut out = [0u64; JOB_WORDS_MOST];
    let n = job_words(&d, &mut out);
    let w = &out[..n];
    assert!(n <= JOB_WORDS_MOST);
    // genxml v11 "CS STORE_STATE": Offset 0+16 (int), Wait Mask 16+16, State 32+8 (0 Timestamp), Address 40+8, Signal slot 48+4, Defer Mode 52+1, Opcode 56+8 (40).
    let stores: Vec<(usize, u64)> = w.iter().copied().enumerate().filter(|(_, x)| field(*x, 56, 8) == 40).collect();
    assert_eq!(stores.len(), 2);
    for (k, (_, s)) in stores.iter().enumerate() {
        assert_eq!(field(*s, 0, 16), [8, 16][k]);
        assert_eq!(field(*s, 16, 16), 0, "synchronous: no wait mask");
        assert_eq!(field(*s, 32, 8), 0, "the timestamp");
        assert_eq!(field(*s, 40, 8), STAMP_REG);
        assert_eq!(field(*s, 48, 5), 0, "no signal slot, immediate");
    }
    // The pair holds the result line, set before the first store (MOVE48: Immediate 0+48, Destination 48+8, Opcode 1).
    let mv = w.iter().position(|&x| field(x, 56, 8) == 1 && field(x, 48, 8) == STAMP_REG).unwrap();
    assert_eq!(field(w[mv], 0, 48), 0x8000_1240);
    let run = w.iter().position(|&x| field(x, 56, 8) == 4).unwrap();
    let wait3 = w.iter().position(|&x| x == 0x0300_0000_0008_0000).unwrap();
    assert!(mv < stores[0].0 && stores[0].0 < run && run < wait3 && wait3 < stores[1].0, "start stamp before the run, end stamp after its wait");
    // The end: the sync word set to the value (SYNC_SET32 last, its value in r72).
    assert_eq!(*w.last().unwrap(), 0x2600_4648_0000_0000);
    assert!(w.contains(&mv32(72, 0x1234_5678)));
    assert!(w.contains(&mv48(70, 0x3200_0240)));
    assert!(REGS_USED <= 96);
}

#[test]
fn ring_round_trip() {
    let depth = 2;
    let w = words(bytes(depth) / 8);
    let r = Ring::new(&w, depth).unwrap();
    assert_eq!(r.open(), 0);
    let u = [1u64, 2, 3];
    for n in 0..4 {
        assert!(r.submit(n, &Job { kernel: 1, groups: (8, 2), uniforms: &u }));
    }
    // Four of four in flight: no room for the fifth.
    assert!(!r.submit(4, &Job { kernel: 1, groups: (8, 2), uniforms: &u }));
    assert_eq!(r.submitted(), 4);
    assert_eq!(r.request(2), (1, 3, (8, 2)));
    assert!(r.result(0).is_none());
    // The GPU's stamps, then the service's end of the first two.
    for n in 0..2u64 {
        let rec = r.record_offset(n) / 8;
        w[rec + STARTED].store(1000 + 10 * n, Ordering::Relaxed);
        w[rec + ENDED_AT].store(1005 + 10 * n, Ordering::Relaxed);
        r.end(n, S_DONE, 900, 1100, (0, 0), 100);
    }
    r.finish(2, 0);
    let e = r.result(1).unwrap();
    assert_eq!((e.status, e.started, e.ended, e.gpu_ticks(), e.round_ticks()), (S_DONE, 910, 915, Some(5), 200));
    assert!(r.result(2).is_none());
    // Room again: job 4 reuses job 0's record.
    assert!(r.submit(4, &Job { kernel: 0, groups: (1, 1), uniforms: &[] }));
    assert_eq!(r.record_offset(4), r.record_offset(0));
    assert_eq!(r.request(4), (0, 0, (1, 1)));
    // A fault ends the queue: the failing job's result, the rest lost, DONE caught up.
    r.end(2, S_OUTSIDE, 900, 1200, (0xDEAD_0000, 0x4C3), 100);
    r.end(3, S_LOST, 900, 1200, (0, 0), 100);
    r.end(4, S_LOST, 900, 1200, (0, 0), 100);
    r.finish(5, E_FAULT);
    assert_eq!(r.ended(), E_FAULT);
    assert_eq!(r.result(2).unwrap().fault_address, 0xDEAD_0000);
    assert_eq!(r.result(2).unwrap().gpu_ticks(), None);
    assert_eq!(r.result(4).unwrap().status, S_LOST);
    // An ended queue takes nothing until the service opens it again.
    assert!(!r.submit(5, &Job { kernel: 0, groups: (1, 1), uniforms: &[] }));
    assert_eq!(r.open(), 5);
    assert!(r.submit(5, &Job { kernel: 0, groups: (1, 1), uniforms: &[] }));
}

#[test]
fn too_many_uniforms_refused() {
    let w = words(bytes(0) / 8);
    let r = Ring::new(&w, 0).unwrap();
    r.open();
    let u = [0u64; UNIFORMS_MOST + 1];
    assert!(!r.submit(0, &Job { kernel: 0, groups: (1, 1), uniforms: &u }));
    assert!(r.submit(0, &Job { kernel: 0, groups: (1, 1), uniforms: &u[..UNIFORMS_MOST] }));
    assert!(Ring::new(&w, 1).is_none(), "a buffer too short for the depth");
    assert!(Ring::new(&words(bytes(DEPTH_MOST_LOG2 + 1) / 8), DEPTH_MOST_LOG2 + 1).is_none());
}

#[test]
fn checks() {
    assert!(check(0, 0, (1, 1), 1));
    assert!(!check(1, 0, (1, 1), 1), "no such kernel");
    assert!(!check(0, 65, (1, 1), 1), "too many uniforms");
    assert!(!check(0, 0, (0, 1), 1) && !check(0, 0, (1, 0), 1), "an empty grid");
    assert!(!check(0, 0, (1 << 20 | 1, 1), 1), "past 2^20 workgroups");
}

/// A client thread and a service thread passing 2^16 jobs thru a ring of depth 2^2, the service ending each in order as it sees it: every result lands, in order, none lost; and the protocol's own cost on this host (no GPU, no syscalls: each side spins on the other's word), for scale only.
#[test]
fn threads() {
    let depth = 2;
    let w: &'static [AtomicU64] = Box::leak(words(bytes(depth) / 8).into_boxed_slice());
    let total = 1u64 << 16;
    let service = std::thread::spawn(move || {
        let r = Ring::new(w, depth).unwrap();
        let mut next = 0u64;
        while next < total {
            let s = r.submitted();
            while next < s {
                let (k, nu, g) = r.request(next);
                assert!(check(k, nu, g, 4));
                r.end(next, S_DONE, next, next + 1, (0, 0), 0);
                next += 1;
                r.finish(next, 0);
            }
            std::hint::spin_loop();
        }
    });
    let r = Ring::new(w, depth).unwrap();
    let t0 = std::time::Instant::now();
    for n in 0..total {
        while !r.room(n) {
            std::hint::spin_loop();
        }
        // Job n - 4's result is in the record job n takes: read before the submit.
        if n >= 4 {
            let e = r.result(n - 4).unwrap();
            assert_eq!((e.status, e.submitted), (S_DONE, n - 4));
        }
        assert!(r.submit(n, &Job { kernel: (n & 3) as u8, groups: (n as u32 + 1, 1), uniforms: &[n] }));
    }
    while r.done() < total {
        std::hint::spin_loop();
    }
    service.join().unwrap();
    println!("ring: A#{total} jobs thru depth 2^{depth} between two host threads in A#{} us (A#{} ns a job, spinning, no syscalls)", t0.elapsed().as_micros(), t0.elapsed().as_nanos() / total as u128);
}

/// `GONE`: the service's close and the kernel's marks, and the open that clears them.
#[test]
fn gone_marks() {
    let w = words(bytes(1) / 8);
    let r = Ring::new(&w, 1).unwrap();
    r.open();
    assert!(r.submit(0, &Job { kernel: 0, groups: (1, 1), uniforms: &[] }));
    // The service closes: its thread's word changes, the count is kept, the client may submit no more.
    let before = r.submit_raw();
    r.mark_closed();
    assert!(r.submit_gone() && r.submit_raw() != before && r.submitted() == 1);
    assert!(!r.submit(1, &Job { kernel: 0, groups: (1, 1), uniforms: &[] }));
    assert_eq!(r.open(), 1);
    assert!(!r.submit_gone());
    assert!(r.submit(1, &Job { kernel: 0, groups: (1, 1), uniforms: &[] }));
    // The kernel marks DONE at the service's end: the client's wait ends and the queue says why.
    w[DONE].fetch_or(GONE, Ordering::SeqCst);
    assert_eq!(r.ended(), E_SERVICE);
    assert_eq!(r.done(), 1, "the count without the mark");
}
