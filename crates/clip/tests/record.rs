//! The recorder on a host: 64 frames thru the workers and the writer into memory, read back by vsf's clip reader, every sample bit exact; the pacing for every whole rate.

use ferros_clip::{cfa_of_byte, Camera, FrameIn, MemSink, Mode, Pacing, Recorder, Taken};
use vsf::visual::clip::Reader;

#[test]
fn a_long_shutter_opens_short_then_sets() {
    // Lumis's 67.8 ms shutter at 30/s: 21342 lines, a 10586-line frame.
    let p = Pacing::of(30);
    assert!(p.open_needs_set(67_800_000) && !p.open_needs_set(16_000_000));
    let m = p.open_msg(0, 67_800_000);
    let lines = u16::from_le_bytes([m[8], m[9]]) as u64;
    assert_eq!(lines, p.fll as u64 - ferros_clip::OPEN_MARGIN);
}

#[test]
fn a_rate_change_is_a_frame_length_set() {
    // Still to RAW video at 60, 60 to 24, and back to the still's frame: SET what 4 with the pacing's frame length, then 0 for the open's own.
    for fps in [60, 24, 1] {
        let p = Pacing::of(fps);
        let m = p.set_msg();
        assert_eq!((m[0], m[1]), (3, 4));
        assert_eq!(u64::from_le_bytes(m[2..10].try_into().unwrap()), p.fll as u64);
        assert!(p.fll as u64 >= ferros_clip::FLL_LEAST && p.fll as u64 <= ferros_clip::FLL_MOST);
    }
    assert_eq!(ferros_clip::set_open_fll_msg(), [3, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn space_from_the_vaults_reply() {
    // The vault's EOP_SPACE reply in blocks: a 48 GiB window, 2^20 blocks taken, 2^21 the longest free stretch, 2^18 this app's in 3 clips.
    let mut b = [0u8; 40];
    for (k, v) in [12u64 << 20, 1 << 20, 1 << 21, 1 << 18, 3].iter().enumerate() {
        b[k * 8..k * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    let s = ferros_clip::Space::from_reply(&b);
    assert_eq!(s, ferros_clip::Space { window_bytes: 48 << 30, free_bytes: 44 << 30, largest_free_bytes: 8 << 30, mine_bytes: 1 << 30, clips: 3 });
}

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

#[test]
fn coders_spread_one_to_a_core() {
    use ferros_clip::spread;
    assert_eq!(spread(&[0x00F0; 4]), vec![0x10, 0x20, 0x40, 0x80]);
    assert_eq!(spread(&[0x00F0, 0x000F, 0x00F0, 0x000F, 0x00F0]), vec![0x10, 0x1, 0x20, 0x2, 0x40]);
    // More workers than cores: round again.
    assert_eq!(spread(&[0x0003; 3]), vec![0x1, 0x2, 0x1]);
    assert_eq!(spread(&[0x0100]), vec![0x0100]);
}

/// 32 frames recorded on the CPU coders, or on the GPU path with the host's stand-in for the GPU (`gpu::HostDevice`), the source hash asked for or not: the clip's bytes, and its frames decoded against the samples.
fn record(gpu: bool, digests: bool) -> Vec<u8> {
    let (w, h, stride) = (1024usize, 320usize, 1280usize);
    let cam = Camera { width: w, height: h, stride, cfa: cfa_of_byte(0x49), black: 64, white: 1023, make: "ferros".into(), model: "test".into(), sensor_module: "main".into(), lens: String::new(), f_number: None, focal_mm: None, characterization: None, observer: String::new(), orientation: 1 };
    let sink = MemSink::default();
    let bytes = sink.bytes.clone();
    // As many slots as frames: the writer never makes the hand-over drop one, so both paths' clips hold the same frames.
    let mode = Mode { pacing: Pacing::of(30), title: "host test".into(), workers: vec![0; 4], writer: 0, slots: 32, blocks: 1 << 14, at: 1 << 40, source_digests: digests };
    let mut rec = if gpu { Recorder::start_gpu(&cam, &mode, Box::new(sink), ferros_clip::gpu::HostDevice::new(w, h, stride)).unwrap() } else { Recorder::start(&cam, &mode, Box::new(sink)).unwrap() };
    let mut frames = Vec::new();
    for k in 0..32u64 {
        let (packed, px) = frame(w, h, stride, k);
        let f = FrameIn { rows: &packed, count: 100 + k, eagle_ns: 1_000_000_000_000 + k * 33_334_420, exposure_ns: 1 << 20, gain: 2048, focus: 7 };
        assert_eq!(rec.frame(&f), Taken::Recorded, "frame {k}");
        if digests {
            assert_eq!(rec.source_hash(), ferros_clip::samples_source_hash(&px, w, h), "frame {k}'s source hash");
        }
        frames.push(px);
    }
    assert_eq!(rec.gpu().is_some(), gpu, "{}", rec.gpu_note());
    let info = rec.stop();
    assert!(info.error.is_none(), "{:?}", info.error);
    assert_eq!(info.stats.frames, 32);
    let clip = bytes.lock().unwrap()[..info.len as usize].to_vec();
    let r = Reader::open(clip.as_slice()).unwrap();
    assert_eq!(r.frames().len(), 32);
    for (k, px) in frames.iter().enumerate() {
        assert_eq!(&r.decode(k).unwrap(), px, "frame {k}");
    }
    clip
}

#[test]
fn the_gpu_path_writes_the_cpu_path_s_clip() {
    let cpu = record(false, true);
    assert!(record(true, true) == cpu, "the GPU path's clip, source hashes asked");
    assert!(record(true, false) == record(false, false), "the GPU path's clip, packed behind the hand-over");
}

/// Every Pro camera's binned readout paced from its list section's line (ferros_cam cams' `line_ps`, the frame at least its rows and its least blanking: the driver's 128 lines, the tele's own 250): the fastest whole rate each holds, every rate up to it within half a line, and the main by its picosecond line the same frames as by its exact clock.
#[test]
fn pacing_for_each_pro_camera() {
    use ferros_clip::{Line, MAIN_LINE};
    // The exact main as before the line: 24/s 13232 lines, 30/s 10586, 60/s 5293 (docs/RAW-VIDEO.md "Pacing").
    assert_eq!([24, 30, 60].map(|f| Pacing::of(f).fll), [13232, 10586, 5293]);
    for (name, ps, rows, blank, most) in [("main", 3_148_916u64, 3072u64, 1u64 << 7, 60u32), ("ultrawide", 6_561_622, 3000, 1 << 7, 48), ("front", 10_437_753, 2736, 1 << 7, 33), ("tele", 6_106_805, 3024, 250, 50)] {
        let line = Line::of_ps(ps, rows + blank);
        assert_eq!(line.fps_most(), most, "{name}");
        assert!(Pacing::of_line(most + 1, line).is_none() || most == 60, "{name}");
        for fps in 1..=most {
            let p = Pacing::of_line(fps, line).unwrap();
            let want = 1_000_000_000f64 / fps as f64;
            let err_ppm = (p.period_ns as f64 - want) / want * 1e6;
            // The nearest whole frame: within half a line of the rate (the ultrawide's 6.56 us line is 154 ppm of a 47/s frame), each kept frame of `keep`.
            assert!((p.period_ns as f64 - want).abs() <= (line.ns_of(1) as f64 / 2. + 1.) * p.keep as f64, "{name} fps {fps}: {} ns ({err_ppm:+.1} ppm)", p.period_ns);
            assert!(p.fll as u64 >= line.fll_least && p.fll as u64 <= ferros_clip::FLL_MOST);
            if name == "main" {
                assert!((p.fll as i64 - Pacing::of(fps).fll as i64).abs() <= 1, "fps {fps}");
            }
        }
        let p = Pacing::of_line(30.min(most), line).unwrap();
        eprintln!("{name}: the fastest A#{most} a second; at 30 fll A#{}, a frame A#{} ns; an open at 16 ms asks A#{} lines", p.fll, p.sensor_ns, u16::from_le_bytes([p.open_msg(0, 16_000_000)[8], p.open_msg(0, 16_000_000)[9]]));
    }
    assert_eq!(MAIN_LINE.fps_most(), 60);
    assert!(Pacing::of_line(0, MAIN_LINE).is_none() && Pacing::of_line(61, MAIN_LINE).is_none());
}

/// A readout's line read from a LIST reply as the driver builds it (ferros_cam list: a `camera` section a readout with its `id`, `mode`, `line_ps` and `fll_least`); a list from a driver before them gives none.
#[test]
fn a_line_from_the_list() {
    use ferros_clip::Line;
    use vsf::VsfType;
    let section = |id: u64, mode: u64, line: Option<(u64, u64)>| {
        let mut s = vsf::file_format::VsfSection::new("camera");
        let mut put = |name: &str, v: u64| {
            let mut f = vsf::file_format::VsfField::new(name);
            f.values = vec![VsfType::u(v as usize, false)];
            s.fields.push(f);
        };
        put("id", id);
        put("mode", mode);
        if let Some((ps, least)) = line {
            put("line_ps", ps);
            put("fll_least", least);
        }
        s
    };
    let doc = vsf::VsfBuilder::new().add_section_direct(section(0, 0, Some((3_148_916, 3200)))).add_section_direct(section(0, 2, Some((6_586_000, 6272)))).add_section_direct(section(1, 0, Some((6_561_622, 3128)))).add_section_direct(section(2, 0, None)).build().unwrap();
    assert_eq!(Line::of_list(&doc, 1, 0), Some(Line::of_ps(6_561_622, 3128)));
    assert_eq!(Line::of_list(&doc, 0, 2), Some(Line::of_ps(6_586_000, 6272)));
    assert_eq!(Line::of_list(&doc, 0, 0).unwrap().fps_most(), 60);
    assert_eq!((Line::of_list(&doc, 2, 0), Line::of_list(&doc, 3, 0)), (None, None));
}
