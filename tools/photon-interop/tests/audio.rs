//! photon_core's plaid audio against Photon: frame numbers agree with Photon's clock math, Photon's packet layer opens every window, and each payload has the exact shape Photon's receive loop accepts (engine.rs: [ctrl][fno][source][tail], length checked exactly).
//!
//! Photon's receive loop runs inside its audio engine's threads and has no pure entry point, so the acceptance rule is restated here from engine.rs rather than called: ctrl = tier 4 with no repair, source = one 488-byte slot, tail 0 or 11 bytes.

use photon::wave::{keys as pk, packet as pp};
use photon_core::wave as cw;

#[test]
fn frame_numbers_agree() {
    for t in [0i64, 1, 1_420_000_000_000, 1_420_000_123_457, -5_000_000_000] {
        let theirs = vsf::grid::eagle_to_sample(t).div_euclid(photon::wave::align::FRAME) as u32;
        assert_eq!(cw::eagle_frame_no(t), theirs, "eagle {t}");
    }
}

#[test]
fn photon_opens_plaid_windows() {
    let secret = cw::derive_wave_secret(&[9; 32], &[8; 16], &[7; 32], &[6; 32]);
    let mut tx = cw::AudioTx::new(&secret, cw::Direction::OriginToAnswer);
    let mut rx = pk::StepChain::new(&secret, pk::Direction::OriginToAnswer);
    let base = cw::eagle_frame_no(1_420_000_000_000);
    for w in 0..250u32 {
        let pcm: [i16; 240] = core::array::from_fn(|i| ((i as i32 * 137 + w as i32 * 11) % 30_000 - 15_000) as i16);
        let tail = (w % 2 == 0).then_some(cw::LinkTail { stamp_us: w * 5_000, echo: 0, hold_us: 0, peer_loss: 0 });
        let wire = tx.plaid(base.wrapping_add(w), &pcm, tail.as_ref());
        let (h, sealed) = pp::parse_header(&wire).expect("an audio datagram");
        assert!(!h.beam && !h.fill && h.seq == w, "audio magic, seq = window {w}");
        let p = pp::open(&mut rx, &h, sealed).unwrap_or_else(|| panic!("Photon opens window {w}"));
        assert_eq!(p[0], 4, "ctrl: plaid, no repair");
        assert_eq!(u32::from_le_bytes(p[1..5].try_into().unwrap()), base.wrapping_add(w));
        let expected = 1 + 4 + 488;
        assert!(p.len() == expected || p.len() == expected + 11, "window {w}: {} bytes is a shape Photon accepts", p.len());
        assert_eq!(u16::from_le_bytes([p[5], p[6]]), 480, "slot length");
        let back: Vec<i16> = p[7..487].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(back, pcm.to_vec(), "PCM");
        assert!(p[487..493].iter().all(|&b| b == 0), "slot padding");
    }
}
