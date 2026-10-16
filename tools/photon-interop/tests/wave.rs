//! photon_core::wave against Photon's own wave code: keys (with Photon's known-answer vector), media packets byte for byte, and express signals opened both ways with identical plaintexts.

use photon::wave::{keys as pk, packet as pp, signal as ps};
use photon_core::wave as cw;

fn secret() -> [u8; 32] {
    cw::derive_wave_secret(&[0x33; 32], &[0x44; 16], &[0x55; 32], &[0x66; 32])
}

#[test]
fn keys_match() {
    let ours = secret();
    assert_eq!(ours, pk::derive_wave_secret(&[0x33; 32], &[0x44; 16], &[0x55; 32], &[0x66; 32]));
    let kat = "48be713c8a80427eaf77732d8940b937229818d8e7370236625371f3cd04132b";
    assert_eq!(ours.iter().map(|b| format!("{b:02x}")).collect::<String>(), kat, "Photon's known-answer vector");
    assert_eq!(cw::beam_secret(&ours), photon::wave::beam::beam_secret(&ours));
    assert_eq!(cw::fill_secret(&ours), pk::fill_secret(&ours));
    for (cd, pd) in [(cw::Direction::OriginToAnswer, pk::Direction::OriginToAnswer), (cw::Direction::AnswerToOrigin, pk::Direction::AnswerToOrigin)] {
        let (mut c, mut p) = (cw::StepChain::new(&ours, cd), pk::StepChain::new(&ours, pd));
        for step in 0..8 {
            assert!(c.advance_to(step) && p.advance_to(step));
            assert_eq!(c.key(), p.key(), "{cd:?} step {step}");
        }
    }
}

#[test]
fn media_packets_match() {
    let s = cw::beam_secret(&secret());
    for seq in [0u32, 7, 99, 100, 103, 250, 1_000_000] {
        let step = cw::StepChain::step_for_seq(seq);
        let mut c = cw::StepChain::new(&s, cw::Direction::OriginToAnswer);
        let mut p = pk::StepChain::new(&s, pk::Direction::OriginToAnswer);
        c.advance_to(step);
        p.advance_to(step);
        let payload: Vec<u8> = (0..(seq as usize % 1200 + 17)).map(|i| (i * 31 + seq as usize) as u8).collect();
        let ours = cw::seal(&c, seq, &payload, cw::BEAM_MAGIC);
        assert_eq!(ours, pp::seal_beam(&p, seq, &payload).unwrap(), "beam packet seq {seq}");
        assert_eq!(cw::seal(&c, seq, &payload, cw::MEDIA_MAGIC), pp::seal(&p, seq, &payload).unwrap(), "audio packet seq {seq}");
        // Photon opens ours, from a fresh chain that has to ratchet forward.
        let (h, sealed) = pp::parse_header(&ours).unwrap();
        assert!(h.beam && h.seq == seq);
        let mut prx = pk::StepChain::new(&s, pk::Direction::OriginToAnswer);
        assert_eq!(pp::open(&mut prx, &h, sealed).unwrap(), payload, "Photon opens our seq {seq}");
        // And a flipped byte fails.
        let mut bad = ours.clone();
        bad[6] ^= 1;
        let (h, sealed) = pp::parse_header(&bad).unwrap();
        assert!(pp::open(&mut pk::StepChain::new(&s, pk::Direction::OriginToAnswer), &h, sealed).is_none());
    }
}

fn to_photon(s: &cw::WaveSignal) -> ps::WaveSignal {
    match *s {
        cw::WaveSignal::Offer { wave_id, nonce, device } => ps::WaveSignal::Offer { wave_id, nonce, device },
        cw::WaveSignal::Answer { wave_id, nonce, device } => ps::WaveSignal::Answer { wave_id, nonce, device },
        cw::WaveSignal::Decline { wave_id } => ps::WaveSignal::Decline { wave_id },
        cw::WaveSignal::Busy { wave_id } => ps::WaveSignal::Busy { wave_id },
        cw::WaveSignal::Hangup { wave_id } => ps::WaveSignal::Hangup { wave_id },
        cw::WaveSignal::Taken { wave_id } => ps::WaveSignal::Taken { wave_id },
        cw::WaveSignal::Anchor { wave_id } => ps::WaveSignal::Anchor { wave_id },
    }
}

#[test]
fn express_matches() {
    use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
    let key = cw::express_key(&[0x10; 32], &[0x20; 32]);
    assert_eq!(key, ps::express_key(&[0x10; 32], &[0x20; 32]));
    let (id, n, d) = ([0xAB; 16], [0xCD; 32], [0xEF; 32]);
    let ts = 1_420_000_000_000i64;
    let signals = [
        cw::WaveSignal::Offer { wave_id: id, nonce: n, device: Some(d) },
        cw::WaveSignal::Offer { wave_id: id, nonce: n, device: None },
        cw::WaveSignal::Answer { wave_id: id, nonce: n, device: Some(d) },
        cw::WaveSignal::Decline { wave_id: id },
        cw::WaveSignal::Busy { wave_id: id },
        cw::WaveSignal::Hangup { wave_id: id },
        cw::WaveSignal::Taken { wave_id: id },
        cw::WaveSignal::Anchor { wave_id: id },
    ];
    let open = |frame: &[u8]| XChaCha20Poly1305::new_from_slice(&key).unwrap().decrypt(XNonce::from_slice(&frame[1..25]), &frame[25..]).unwrap();
    for sig in signals {
        for (lk, beam) in [(None, false), (Some([0x77u8; 32]), false), (Some([0x77u8; 32]), true)] {
            let ours = cw::seal_express(&key, [0x5A; 24], ts, lk.as_ref(), &sig, beam).unwrap();
            let want_beam = beam && matches!(sig, cw::WaveSignal::Offer { .. });
            assert_eq!(ps::open_express_beam(&key, &ours), Some((ts, lk, to_photon(&sig), want_beam)), "Photon opens our {sig:?} lk={} beam={beam}", lk.is_some());
            let theirs = ps::seal_express_beam(&key, ts, lk.as_ref(), &to_photon(&sig), beam).unwrap();
            assert_eq!(cw::open_express(&key, &theirs), Some((ts, lk, sig, want_beam)), "we open Photon's {sig:?}");
            assert_eq!(open(&ours), open(&theirs), "identical plaintext for {sig:?} lk={} beam={beam}", lk.is_some());
        }
    }
}
