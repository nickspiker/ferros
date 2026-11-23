//! Stacked clips (ferros_clip::packed): frames of exact sums, packed at the width each needs, written and read back whole by vsf's reader.

use ferros_clip::packed::{PackedFrame, PackedMode, PackedRecorder};
use ferros_clip::{Camera, MemSink};
use vsf::types::BitPackedTensor;
use vsf::visual::clip::{FrameMeta, Levels, Reader, Sum};

fn camera(w: usize, h: usize) -> Camera {
    Camera { width: w, height: h, stride: w * 5 / 4, cfa: vec![0, 1, 1, 2], black: 64, white: 1023, make: "test".into(), model: "test".into(), sensor_module: String::new(), lens: String::new(), f_number: None, focal_mm: None, characterization: None, observer: String::new(), orientation: 1 }
}

/// A frame of `n` exposures: sums that grow with the frame (so each frame's width differs), packed at their peak's width, with n times the sensor's levels; a motion plane when asked.
fn frame(k: u64, n: u32, w: usize, h: usize, motion: bool) -> (PackedFrame, Vec<u64>, Option<Vec<u64>>) {
    let sums: Vec<u64> = (0..w * h).map(|p| (p as u64 * 37 + k * 1000) % (n as u64 * 1023) + n as u64 * 64).collect();
    let bits = |v: &[u64], white: u64| (64 - v.iter().copied().max().unwrap_or(0).max(white).leading_zeros()) as u8;
    let samples = BitPackedTensor::pack(bits(&sums, n as u64 * 1023), vec![h, w], &sums);
    let m: Option<Vec<u64>> = motion.then(|| (0..w * h).map(|p| (p as u64 * 11 + k) % (n as u64 * 1023)).collect());
    let mp = m.as_ref().map(|m| BitPackedTensor::pack(bits(m, n as u64 * 1023), vec![h, w], m));
    let levels = |b: f32, wh: f32| Levels { black: vec![b; 3], white: vec![wh; 3] };
    let sum = Sum { exposures: n, levels: levels(n as f32 * 64., n as f32 * 1023.), motion: motion.then(|| levels(0., n as f32 * 1023.)) };
    let meta = FrameMeta { index: k, at: k as i64 * 1_000_000, exposure_s: 1. / 64., gain: 1., focus: 0 };
    (PackedFrame { meta, sum, samples, motion: mp }, sums, m)
}

fn record(motion: bool) {
    let (w, h) = (16, 8);
    let sink = MemSink::default();
    let (bytes, sealed) = (sink.bytes.clone(), sink.sealed.clone());
    let mode = PackedMode { title: "a time-lapse".into(), period_ns: 2_000_000_000, motion, at: 0, blocks: 1 << 10 };
    let rec = PackedRecorder::start(&camera(w, h), &mode, Box::new(sink)).unwrap();
    let mut want = Vec::new();
    for k in 0..5u64 {
        // A frame's exposures grow, as a time-lapse's might when its shutter shortens: each frame its own width and levels.
        let n = 120 * (k as u32 + 1);
        let (f, sums, m) = frame(k, n, w, h, motion);
        assert!(rec.frame(f));
        want.push((n, sums, m));
    }
    let info = rec.stop();
    assert_eq!(info.error, None);
    let (len, hash) = sealed.lock().unwrap().expect("sealed");
    assert_eq!((len, hash), (info.len, info.hash));
    let clip = bytes.lock().unwrap()[..len as usize].to_vec();
    let r = Reader::open(clip.as_slice()).unwrap();
    assert!(!r.recovered());
    assert_eq!(r.frames().len(), 5);
    for (k, (n, sums, m)) in want.iter().enumerate() {
        let (sum, samples, mp) = r.packed(k).unwrap();
        assert_eq!(sum.exposures, *n);
        assert_eq!(sum.levels.white, vec![*n as f32 * 1023.; 3]);
        assert_eq!(samples.unpack_u64(), *sums, "frame {k}'s sums, exact");
        assert_eq!(mp.map(|t| t.unpack_u64()), m.clone(), "frame {k}'s motion plane");
    }
}

#[test]
fn a_stacked_clip_reads_back_every_sum() {
    record(false);
}

#[test]
fn a_motion_clip_reads_back_both_planes() {
    record(true);
}

#[test]
fn a_frame_period_of_zero_is_refused() {
    let mode = PackedMode { title: String::new(), period_ns: 0, motion: false, at: 0, blocks: 1 << 10 };
    assert!(PackedRecorder::start(&camera(16, 8), &mode, Box::new(MemSink::default())).is_err());
}
