//! The recorder on a host: 64 frames thru the workers and the writer into memory, read back by vsf's clip reader, every sample bit exact; the pacing for every whole rate.

use ferros_clip::{cfa_of_byte, Camera, FrameIn, MemSink, Mode, Pacing, Recorder, Taken};
use vsf::visual::clip::Reader;

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); // the algorithm: an LCG wraps by definition
    *s >> 33
}

fn frame(w: usize, h: usize, stride: usize, seed: u64) -> (Vec<u8>, Vec<u16>) {
    let mut s = seed;
    let mut px = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            px[y * w + x] = ((64 + ((x & 1) << 6) + ((y & 1) << 5) + ((x * 3 + y) >> 3) + (lcg(&mut s) & 63) as usize) & 1023) as u16;
        }
    }
    let mut packed = vec![0x5Au8; stride * h];
    for y in 0..h {
        for g in 0..w / 4 {
            let v = &px[y * w + g * 4..y * w + g * 4 + 4];
            let o = y * stride + g * 5;
            for i in 0..4 {
                packed[o + i] = (v[i] >> 2) as u8;
            }
            packed[o + 4] = (0..4).map(|i| ((v[i] & 3) << (2 * i)) as u8).fold(0, |a, b| a | b);
        }
    }
    (packed, px)
}

#[test]
fn sixty_four_frames_bit_exact() {
    let (w, h, stride) = (1024usize, 320usize, 1280usize);
    let cam = Camera { width: w, height: h, stride, cfa: cfa_of_byte(0x49), black: 64, white: 1023, make: "ferros".into(), model: "test".into(), sensor_module: "main".into(), lens: String::new(), f_number: None, focal_mm: None, characterization: None, observer: String::new(), orientation: 1 };
    let sink = MemSink::default();
    let (bytes, sealed) = (sink.bytes.clone(), sink.sealed.clone());
    let mode = Mode { pacing: Pacing::of(30), title: "host test".into(), workers: vec![0; 4], writer: 0, slots: 4, blocks: 1 << 14, at: 1 << 40, source_digests: true };
    let mut rec = Recorder::start(&cam, &mode, Box::new(sink)).unwrap();
    let mut frames = Vec::new();
    for k in 0..64u64 {
        let (packed, px) = frame(w, h, stride, k);
        let f = FrameIn { rows: &packed, count: 100 + k, eagle_ns: 1_000_000_000_000 + k * 33_334_420, exposure_ns: 1 << 20, gain: 2048, focus: 7 };
        // The writer may lag on a slow host; a frame that finds no slot is dropped and counted, never waited for.
        loop {
            match rec.frame(&f) {
                Taken::Recorded => {
                    assert_eq!(rec.source_hash(), ferros_clip::samples_source_hash(&px, w, h), "frame {k}'s source hash");
                    break;
                }
                Taken::Dropped => std::thread::yield_now(),
                t => panic!("frame {k}: {t:?}"),
            }
        }
        frames.push(px);
    }
    let info = rec.stop();
    assert!(info.error.is_none(), "{:?}", info.error);
    assert_eq!(info.stats.frames, 64);
    let clip = bytes.lock().unwrap()[..info.len as usize].to_vec();
    assert_eq!(*sealed.lock().unwrap(), Some((info.len, info.hash)));
    let r = Reader::open(clip.as_slice()).unwrap();
    assert!(!r.recovered());
    assert_eq!(r.frames().len(), 64);
    for (k, px) in frames.iter().enumerate() {
        assert_eq!(&r.decode(k).unwrap(), px, "frame {k}");
    }
    eprintln!("A#64 frames: A#{} bytes coded of A#{} raw (ratio {:.2}), the clip A#{} bytes, coding A#{} us a frame at most", info.stats.coded, info.stats.raw, info.stats.raw as f64 / info.stats.coded as f64, info.len, info.stats.code_us_most);
}

#[test]
fn a_full_extent_ends_the_clip_closed() {
    let (w, h, stride) = (256usize, 128usize, 320usize);
    let cam = Camera { width: w, height: h, stride, cfa: cfa_of_byte(0x49), black: 64, white: 1023, make: String::new(), model: String::new(), sensor_module: String::new(), lens: String::new(), f_number: None, focal_mm: None, characterization: None, observer: String::new(), orientation: 1 };
    let sink = MemSink::default();
    let bytes = sink.bytes.clone();
    // 2^5 blocks: 128 KiB, a few frames of about 20 KiB each.
    let mode = Mode { pacing: Pacing::of(24), title: String::new(), workers: vec![0; 2], writer: 0, slots: 1, blocks: 1 << 5, at: 0, source_digests: false };
    let mut rec = Recorder::start(&cam, &mode, Box::new(sink)).unwrap();
    let mut recorded = 0;
    for k in 0..64u64 {
        let (packed, _) = frame(w, h, stride, k);
        let f = FrameIn { rows: &packed, count: k, eagle_ns: k * 41_666_000, exposure_ns: 1 << 20, gain: 1024, focus: 0 };
        match rec.frame(&f) {
            Taken::Recorded => recorded += 1,
            Taken::Dropped => {}
            Taken::Ended => break,
            Taken::Paced => panic!("24/s keeps every frame"),
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let info = rec.stop();
    assert!(info.len > 0 && info.len <= 32 * 4096, "{info:?}");
    let clip = bytes.lock().unwrap()[..info.len as usize].to_vec();
    let r = Reader::open(clip.as_slice()).unwrap();
    assert!(!r.recovered() && !r.frames().is_empty() && r.frames().len() < recorded + 1);
}

#[test]
fn pacing_for_every_whole_rate() {
    for fps in 1..=60u32 {
        let p = Pacing::of(fps);
        let want = 1_000_000_000f64 / fps as f64;
        let err_ppm = (p.period_ns as f64 - want) / want * 1e6;
        assert!(err_ppm.abs() < 128., "fps {fps}: {} ns", p.period_ns);
        assert!(p.fll as u64 >= ferros_clip::FLL_LEAST);
        eprintln!("fps A#{fps}: fll A#{} keep A#{} sensor A#{} ns, the clip's frame A#{} ns ({:+.1} ppm)", p.fll, p.keep, p.sensor_ns, p.period_ns, err_ppm);
    }
}
