//! photon_core's H.264 decoded by Photon's own decoder (OpenH264). I_PCM is lossless, so every pixel must come back exactly — including sizes that aren't a multiple of 16 (cropping) and pictures holding zero bytes (emulation prevention). The compressed I_16x16 + CAVLC path is lossy, so the decoder must return exactly our encoder's own reconstruction (proof every syntax element parses as intended), at a PSNR a correct encoder clears.

use photon::wave::h264 as ph;
use photon_core::h264 as ch;

fn test_card(w: usize, h: usize, t: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let y = (0..w * h).map(|i| { let (x, y) = (i % w, i / w); if y < 8 { 0 } else { ((x * 255 / w) ^ (y * 3 + t)) as u8 } }).collect();
    let u = (0..(w / 2) * (h / 2)).map(|i| ((i % (w / 2)) * 255 / (w / 2)) as u8).collect();
    let v = (0..(w / 2) * (h / 2)).map(|i| (((i / (w / 2)) * 255 / (h / 2)) as u8).wrapping_add(t as u8)).collect();
    (y, u, v)
}

#[test]
fn photon_decodes_our_frames_exactly() {
    let mut dec = ph::Decoder::new().expect("OpenH264 decoder");
    for (n, (w, h)) in [(640usize, 480usize), (320, 240), (330, 250), (176, 144), (640, 480)].into_iter().enumerate() {
        let (y, u, v) = test_card(w, h, n * 17);
        let au = ch::encode_pcm_idr(&ch::Picture { width: w, height: h, y: &y, u: &u, v: &v }, (n % 2) as u32);
        let pic = dec.decode(&au).unwrap_or_else(|e| panic!("{w}x{h}: {e}")).unwrap_or_else(|| panic!("{w}x{h}: no picture"));
        assert_eq!((pic.w, pic.h), (w, h), "geometry (cropping)");
        assert!(pic.y == y, "{w}x{h} luma exact");
        assert!(pic.u == u && pic.v == v, "{w}x{h} chroma exact");
        println!("{w}x{h}: {} bytes per frame, decoded exactly", au.len());
    }
}

/// Test cards for the compressed path: (name, luma, cb, cr) at w x h, `t` shifting the content between frames.
fn card(kind: &str, w: usize, h: usize, t: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (cw, ch) = (w / 2, h / 2);
    let mut seed = 0x9E37_79B9u32 ^ (t as u32).wrapping_mul(2654435761);
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    match kind {
        // Smooth: a diagonal ramp with a gentle parabolic bowl, chroma ramps.
        "smooth" => {
            let y = (0..w * h).map(|i| { let (x, y) = ((i % w) as i64, (i / w) as i64); let (dx, dy) = (x - w as i64 / 2, y - h as i64 / 2); (40 + x * 120 / w as i64 + y * 60 / h as i64 + (dx * dx + dy * dy) * 30 / (w * w) as i64 + (t % 32) as i64).clamp(0, 255) as u8 }).collect();
            let u = (0..cw * ch).map(|i| (64 + (i % cw) * 128 / cw) as u8).collect();
            let v = (0..cw * ch).map(|i| (192 - (i / cw) * 128 / ch) as u8).collect();
            (y, u, v)
        }
        // Hard edges: a 24-pixel checkerboard of near-black and near-white with a diagonal band, chroma blocks.
        "edges" => {
            let y = (0..w * h).map(|i| { let (x, y) = (i % w + t, i / w); if (x + y) % 64 < 8 { 128 } else if (x / 24 + y / 24) % 2 == 0 { 16 } else { 235 } }).collect();
            let u = (0..cw * ch).map(|i| if (i % cw / 12 + i / cw / 12) % 2 == 0 { 90 } else { 170 }).collect();
            let v = (0..cw * ch).map(|i| if (i % cw) < cw / 2 { 60 } else { 200 }).collect();
            (y, u, v)
        }
        // Noise: uniform random samples everywhere (the worst case for intra prediction).
        "noise" => {
            let y = (0..w * h).map(|_| rnd() as u8).collect();
            let u = (0..cw * ch).map(|_| rnd() as u8).collect();
            let v = (0..cw * ch).map(|_| rnd() as u8).collect();
            (y, u, v)
        }
        _ => test_card(w, h, t),
    }
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a.iter().zip(b).map(|(&x, &y)| { let d = x as f64 - y as f64; d * d }).sum();
    if se == 0.0 { return 99.0; }
    10.0 * (255.0f64 * 255.0 * a.len() as f64 / se).log10()
}

/// Minimum luma PSNR a correct I_16x16 encoder clears with margin, by content and QP.
fn floor_db(kind: &str, qp: u8) -> f64 {
    match (kind, qp) {
        ("smooth", 20) => 44.0,
        ("smooth", 26) => 40.0,
        ("smooth", 32) => 36.0,
        ("smooth", _) => 30.0,
        ("noise", 20) => 36.0,
        ("noise", 26) => 30.0,
        ("noise", 32) => 24.0,
        ("noise", _) => 15.0,
        (_, 20) => 40.0,
        (_, 26) => 34.0,
        (_, 32) => 28.0,
        _ => 22.0,
    }
}

#[test]
fn photon_decodes_our_intra_frames_exactly_as_we_reconstruct_them() {
    let mut dec = ph::Decoder::new().expect("OpenH264 decoder");
    let mut n = 0u32;
    for (w, h) in [(640usize, 480usize), (320, 240), (330, 250), (176, 144)] {
        for kind in ["smooth", "edges", "noise", "mixed"] {
            let (y, u, v) = card(kind, w, h, n as usize * 7);
            let pic = ch::Picture { width: w, height: h, y: &y, u: &u, v: &v };
            let pcm = ch::encode_pcm_idr(&pic, n % 2).len();
            for qp in [20u8, 26, 32, 40] {
                let (au, rec) = ch::encode_intra_idr_with_recon(&pic, n % 2, qp);
                n += 1;
                let out = dec.decode(&au).unwrap_or_else(|e| panic!("{w}x{h} {kind} qp {qp}: {e}")).unwrap_or_else(|| panic!("{w}x{h} {kind} qp {qp}: no picture"));
                assert_eq!((out.w, out.h), (w, h), "geometry (cropping)");
                assert!(out.y == rec.y && out.u == rec.u && out.v == rec.v, "{w}x{h} {kind} qp {qp}: decoder output differs from our reconstruction");
                let (py, pu, pv) = (psnr(&y, &out.y), psnr(&u, &out.u), psnr(&v, &out.v));
                println!("{w}x{h} {kind:6} qp {qp}: {:7} bytes ({:5.1}% of I_PCM {pcm}), PSNR Y {py:5.2} U {pu:5.2} V {pv:5.2} dB, recon exact", au.len(), 100.0 * au.len() as f64 / pcm as f64);
                assert!(py > floor_db(kind, qp), "{w}x{h} {kind} qp {qp}: luma PSNR {py:.2} dB below {}", floor_db(kind, qp));
                assert!(au.len() < pcm, "{w}x{h} {kind} qp {qp}: no smaller than I_PCM");
            }
        }
    }
}

#[test]
fn photon_decodes_an_intra_sequence() {
    let mut dec = ph::Decoder::new().expect("OpenH264 decoder");
    let (w, h) = (320usize, 240usize);
    let mut total = 0;
    for f in 0..16usize {
        let kind = ["smooth", "edges", "mixed", "noise"][f % 4];
        let (y, u, v) = card(kind, w, h, f * 3);
        let (au, rec) = ch::encode_intra_idr_with_recon(&ch::Picture { width: w, height: h, y: &y, u: &u, v: &v }, (f % 2) as u32, 26);
        total += au.len();
        let out = dec.decode(&au).unwrap_or_else(|e| panic!("frame {f}: {e}")).unwrap_or_else(|| panic!("frame {f}: no picture"));
        assert_eq!((out.w, out.h), (w, h));
        assert!(out.y == rec.y && out.u == rec.u && out.v == rec.v, "frame {f}: decoder output differs from our reconstruction");
    }
    println!("16-frame 320x240 sequence at qp 26: {total} bytes, every frame reconstructed exactly");
}

/// The ends of the QP range: QP 10 (the floor; finer requests are raised to it) drives large levels through CAVLC's escape codes (level_prefix 14 and 15, suffixLength up to 6), QP 51 drives nearly everything to zero.
#[test]
fn photon_decodes_extreme_qps_exactly() {
    let mut dec = ph::Decoder::new().expect("OpenH264 decoder");
    let (w, h) = (176usize, 144usize);
    let mut n = 0u32;
    for kind in ["smooth", "edges", "noise", "mixed"] {
        let (y, u, v) = card(kind, w, h, 5);
        for qp in [0u8, 4, 10, 16, 46, 51] {
            let (au, rec) = ch::encode_intra_idr_with_recon(&ch::Picture { width: w, height: h, y: &y, u: &u, v: &v }, n % 2, qp);
            n += 1;
            let out = dec.decode(&au).unwrap_or_else(|e| panic!("{kind} qp {qp}: {e}")).unwrap_or_else(|| panic!("{kind} qp {qp}: no picture"));
            assert!(out.y == rec.y && out.u == rec.u && out.v == rec.v, "{kind} qp {qp}: decoder output differs from our reconstruction");
            let py = psnr(&y, &out.y);
            println!("176x144 {kind:6} qp {qp:2}: {:6} bytes, PSNR Y {py:5.2} dB, recon exact", au.len());
            if qp <= 10 {
                assert!(py > 45.0, "{kind} qp {qp}: luma PSNR {py:.2} dB (clamped levels?)");
            }
        }
    }
}
