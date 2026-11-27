//! Host tests: a publish reads back whole, a read racing a publish is never taken for one, the kernel's end of a writer, a new writer after it, and the GPS and thermometer layouts' round trips.

extern crate std;
use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::vec::Vec;

/// A mailbox's words on the heap (2 MiB of atomics).
fn mailbox() -> Arc<Vec<AtomicU64>> {
    Arc::new((0..WORDS).map(|_| AtomicU64::new(0)).collect())
}

#[test]
fn never_then_fresh() {
    let m = mailbox();
    let mut out = [0u64; 4];
    assert_eq!(try_read(&m, &mut out), Got::Never);
    let s = publish(&m, 7, 9, &[1, 2, 3]);
    assert_eq!(s, 2);
    assert_eq!(try_read(&m, &mut out), Got::Fresh { seq: 2, kind: 7, len: 3, at: 9 });
    assert_eq!(out[..3], [1, 2, 3]);
    // A second publish moves the sequence two on.
    assert_eq!(publish(&m, 7, 10, &[4]), 4);
}

#[test]
fn short_buffer_takes_what_fits() {
    let m = mailbox();
    publish(&m, 1, 0, &[5, 6, 7, 8]);
    let mut out = [0u64; 2];
    assert_eq!(try_read(&m, &mut out), Got::Fresh { seq: 2, kind: 1, len: 4, at: 0 });
    assert_eq!(out, [5, 6]);
}

#[test]
fn a_publish_under_way_is_busy() {
    let m = mailbox();
    publish(&m, 1, 0, &[1]);
    let p = Publish::begin(&m);
    p.put(0, 2);
    let mut out = [0u64; 1];
    // The sequence is odd: the reader waits on it.
    assert_eq!(try_read(&m, &mut out), Got::Busy { seq: 3 });
    assert_eq!(p.finish(1, 1, 0), 4);
    assert_eq!(try_read(&m, &mut out), Got::Fresh { seq: 4, kind: 1, len: 1, at: 0 });
    assert_eq!(out, [2]);
}

#[test]
fn the_kernel_ends_a_writer() {
    let m = mailbox();
    publish(&m, 1, 0, &[9, 9, 9]);
    let s = kill(&m);
    assert_eq!(s, 4 | DEAD);
    let mut out = [0u64; 3];
    assert_eq!(try_read(&m, &mut out), Got::Dead { seq: 4 | DEAD });
    // The dead writer's words are gone, not left for whoever maps the view next.
    assert!(m[HEADER..HEADER + 3].iter().all(|w| w.load(Ordering::Relaxed) == 0));
    // A new writer counts on from the dead sequence, so a reader waiting on it is woken by the first publish.
    assert_eq!(publish(&m, 1, 0, &[1]), 6);
    assert_eq!(try_read(&m, &mut out), Got::Fresh { seq: 6, kind: 1, len: 1, at: 0 });
}

#[test]
fn a_writer_that_never_finished() {
    let m = mailbox();
    publish(&m, 1, 0, &[1]);
    // Its process ended mid-publish: the sequence odd. The kernel's end rounds it up.
    let p = Publish::begin(&m);
    p.put(0, 2);
    core::mem::forget(p);
    assert_eq!(m[SEQ].load(Ordering::Relaxed), 3);
    assert_eq!(kill(&m), 6 | DEAD);
    let mut out = [0u64; 1];
    assert_eq!(try_read(&m, &mut out), Got::Dead { seq: 6 | DEAD });
}

/// Readers racing a writer that publishes as fast as it can: every `Fresh` read is one publish's words (each payload is its sequence, repeated), and the readers see the sequence only grow.
#[test]
fn readers_racing_a_writer_never_read_a_torn_publish() {
    const LEN: usize = 1 << 9;
    const PUBLISHES: u64 = 1 << 14;
    let m = mailbox();
    let done = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..1 << 3)
        .map(|_| {
            let (m, done) = (m.clone(), done.clone());
            std::thread::spawn(move || {
                let mut out = [0u64; LEN];
                let (mut fresh, mut busy, mut last) = (0u64, 0u64, 0u64);
                while !done.load(Ordering::Acquire) {
                    match try_read(&m, &mut out) {
                        Got::Fresh { seq, len, .. } => {
                            assert_eq!(len, LEN);
                            assert!(seq >= last, "the sequence went back");
                            last = seq;
                            assert!(out.iter().all(|&w| w == seq), "a torn publish read as whole at sequence {seq}");
                            fresh += 1;
                        }
                        Got::Busy { .. } => busy += 1,
                        Got::Never => {}
                        Got::Dead { .. } => panic!("no kernel end in this test"),
                    }
                }
                (fresh, busy)
            })
        })
        .collect();
    let mut payload = [0u64; LEN];
    for _ in 0..PUBLISHES {
        // The next sequence is the current one plus two: each publish carries its own number in every word.
        let next = m[SEQ].load(Ordering::Relaxed) + 2;
        payload.fill(next);
        assert_eq!(publish(&m, 1, 0, &payload), next);
        // A breath between publishes, as a driver has: readers get whole reads in between as well as racing ones.
        for _ in 0..1 << 10 {
            core::hint::spin_loop();
        }
    }
    done.store(true, Ordering::Release);
    let (mut fresh, mut busy) = (0, 0);
    for r in readers {
        let (f, b) = r.join().unwrap();
        fresh += f;
        busy += b;
    }
    std::println!("{fresh} whole reads, {busy} reads that caught a publish and were refused");
    assert!(fresh > 0);
}

#[test]
fn gps_round_trip() {
    let mut g = gps::Gps { valid: true, has_track: true, kinds: 5, fixes: 1 << 20, fix_at: u64::MAX - 1, lat: -(1 << 30), lon: (1 << 30) + 7, alt: -12, err: 3_000_000_000, speed: -64, track: 360 * 64, frames: 1 << 31, msgs: 77, sky_at: 1 << 40, generation: u32::MAX, n: 3, tracks: [0; gps::TRACKS] };
    g.tracks[..3].copy_from_slice(&[1, 2, u64::MAX]);
    let mut w = [0u64; gps::WORDS];
    g.encode(&mut w);
    assert_eq!(gps::Gps::decode(&w), Some(g));
    // A count past the tracks is cut to them, never read past the words.
    w[gps::TRACKS_N] = 255;
    assert_eq!(gps::Gps::decode(&w).unwrap().n, gps::TRACKS);
    assert_eq!(gps::Gps::decode(&w[..gps::WORDS - 1]), None);
}

#[test]
fn the_names_line_up() {
    for k in 0..NAMES.len() {
        assert_eq!(BOX_NAMES[k], std::format!("{}-mailbox", NAMES[k]));
        assert_eq!(VIEW_NAMES[k], std::format!("{}-view", NAMES[k]));
    }
    assert_eq!(NAMES[GPS], "gps");
    assert_eq!(NAMES[MOTION], "motion");
    assert_eq!(NAMES[MIC], "mic");
    assert_eq!(NAMES[TEST], "test");
    assert_eq!(NAMES[FRAME], "frame");
    assert_eq!(NAMES[THERMO], "thermo");
}

#[test]
fn thermo_round_trip() {
    let mut r = [thermo::Range::default(); thermo::RANGES];
    r[thermo::NORMAL] = thermo::Range { samples: u32::MAX, floats: 7, aoc_ns: u64::MAX - 3, at: 1 << 40, raw: [0.; thermo::FLOATS] };
    for k in 0..7 {
        r[thermo::NORMAL].raw[k] = 31.5 + k as f32 * -0.25;
    }
    r[thermo::EXTENDED] = thermo::Range { samples: 1, floats: 1 << 6, aoc_ns: 5, at: 6, raw: [f32::NAN; thermo::FLOATS] };
    let mut w = [0u64; thermo::WORDS];
    thermo::encode(&r, &mut w);
    let d = thermo::decode(&w).unwrap();
    assert_eq!(d[thermo::NORMAL], r[thermo::NORMAL]);
    assert_eq!(d[thermo::NORMAL].sensor(), 31.5);
    assert_eq!(d[thermo::NORMAL].object(), 31.25);
    // NaN is never equal to itself: the bits come back as they went.
    let e = d[thermo::EXTENDED];
    assert_eq!((e.samples, e.floats, e.aoc_ns, e.at), (1, 1 << 6, 5, 6));
    assert!(e.raw.iter().all(|f| f.to_bits() == f32::NAN.to_bits()));
    // The temperatures' word is the object (float 1), then the sensor (float 0), as a reader of the word alone takes them.
    assert_eq!(f32::from_bits(w[thermo::TEMPS] as u32), 31.25);
    assert_eq!(f32::from_bits((w[thermo::TEMPS] >> 32) as u32), 31.5);
    assert_eq!(thermo::decode(&w[..thermo::WORDS - 1]), None);
}

/// The microphones' ring: `n` frames from position `f0` published as one period, each sample `f * 8 + c` so a reader can check every one, the writer's position after them.
fn mic_period(m: &[AtomicU64], f0: u64, n: u64, chans: u8) -> u64 {
    let p = Publish::begin(m);
    for f in f0..f0 + n {
        for c in 0..chans as usize {
            mic::put(&p, f, c, (f * 8 + c as u64) as i32);
        }
    }
    let h = mic::Head { pos: f0 + n, rate: 48_000, channels: chans, anchor_pos: f0 + n - 1, anchor_at: 1000 + f0, overrun: 0 };
    mic::finish(p, &h, 7);
    f0 + n
}

#[test]
fn mic_ring_reads_by_position_and_catches_up() {
    let m = mailbox();
    let mut out = std::vec![0i32; 1 << 12];
    assert_eq!(mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut out), mic::Read::Never);
    let mut pos = 0;
    for _ in 0..3 {
        pos = mic_period(&m, pos, 48, 3);
    }
    // From the start: every frame, interleaved, each sample where it was put.
    match mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut out) {
        mic::Read::Frames { head, start, frames } => {
            assert_eq!((head.pos, head.channels, head.rate, start, frames), (144, 3, 48_000, 0, 144));
            assert_eq!(head.anchor_pos, 143);
            for f in 0..frames {
                for c in 0..3 {
                    assert_eq!(out[f * 3 + c], (f as u64 * 8 + c as u64) as i32);
                }
            }
        }
        r => panic!("{r:?}"),
    }
    // A reader that had taken up to 100 takes the rest.
    assert!(matches!(mic::read_by(|i| m[i].load(Ordering::Relaxed), 100, &mut out), mic::Read::Frames { start: 100, frames: 44, .. }));
    // A small buffer takes whole frames only.
    let mut small = [0i32; 10];
    assert!(matches!(mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut small), mic::Read::Frames { start: 0, frames: 3, .. }));
}

#[test]
fn mic_ring_says_what_a_late_reader_lost() {
    let m = mailbox();
    let mut pos = 0;
    while pos < mic::RING_FRAMES as u64 + 500 {
        pos = mic_period(&m, pos, 48, 3);
    }
    let mut out = std::vec![0i32; mic::RING_FRAMES * 3];
    match mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut out) {
        mic::Read::Frames { head, start, frames } => {
            // The oldest frame still held, a ring behind the newest: frames 0 to start were written over (the reader's loss), and those held read back right.
            assert_eq!(start, head.pos - mic::RING_FRAMES as u64);
            assert_eq!(frames, mic::RING_FRAMES);
            assert_eq!(out[0], (start * 8) as i32);
            assert_eq!(out[(frames - 1) * 3 + 2], ((head.pos - 1) * 8 + 2) as i32);
        }
        r => panic!("{r:?}"),
    }
}

#[test]
fn mic_ring_busy_dead_and_other() {
    let m = mailbox();
    let mut out = [0i32; 16];
    mic_period(&m, 0, 4, 1);
    let p = Publish::begin(&m);
    assert!(matches!(mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut out), mic::Read::Busy { .. }));
    p.finish(mic::KIND, mic::WORDS, 0);
    kill(&m);
    assert_eq!(mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut out), mic::Read::Dead);
    publish(&m, thermo::KIND, 0, &[1, 2]);
    assert_eq!(mic::read_by(|i| m[i].load(Ordering::Relaxed), 0, &mut out), mic::Read::Other);
}
