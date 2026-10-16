//! photon_core's H.264 decoded by Photon's own decoder (OpenH264): I_PCM is lossless, so every pixel must come back exactly — including sizes that aren't a multiple of 16 (cropping) and pictures holding zero bytes (emulation prevention).

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
