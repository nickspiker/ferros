//! Build each frame, parse it back, and check the fields and the signature survive — the wire format is the contract with the stock Photon app, so a round trip that loses or mis-widths a field is a real interop break.

use ed25519_dalek::SigningKey;
use photon_core::wire::*;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

#[test]
fn ping_pong() {
    let k = key();
    let pk = k.verifying_key().to_bytes();
    let bytes = ping(&k, 123_456);
    match parse(&bytes).expect("ping parses") {
        Frame::Ping { time, key, hp, .. } => {
            assert_eq!(time, 123_456);
            assert_eq!(key, pk);
            assert_eq!(hp, ping_provenance(&pk, 123_456));
        }
        f => panic!("not a ping: {f:?}"),
    }
    assert!(parse(&bytes).unwrap().verified(), "ping signature verifies");

    // A pong echoes the ping's provenance, signed by the responder, with the observed address.
    let hp = ping_provenance(&pk, 123_456);
    let obs = [192, 168, 1, 50, 0x11, 0x17]; // 192.168.1.50:4375
    let pbytes = pong(&k, 200, hp, Some(&obs));
    match parse(&pbytes).expect("pong parses") {
        Frame::Pong { hp: h, obs: o, .. } => {
            assert_eq!(h, hp);
            assert_eq!(o.as_deref(), Some(&obs[..]));
        }
        f => panic!("not a pong: {f:?}"),
    }
    assert!(parse(&pbytes).unwrap().verified());
}

#[test]
fn photon_with_era() {
    let k = key();
    let tok = [0x11; 32];
    let prev = [0x22; 32];
    let data = b"ciphertext bytes";
    // era = 300 forces a width above one byte — the field must survive whatever width VSF picks.
    let bytes = photon(&k, 9, &tok, b"lane-label", &prev, Some(300), data);
    match parse(&bytes).expect("photon parses") {
        Frame::Photon { tok: t, prev: p, era, data: d, lane, .. } => {
            assert_eq!(t, tok);
            assert_eq!(p, prev);
            assert_eq!(era, Some(300), "era survives at its true width");
            assert_eq!(d, data);
            assert_eq!(lane, b"lane-label");
        }
        f => panic!("not a photon: {f:?}"),
    }
    assert!(parse(&bytes).unwrap().verified(), "photon signature over tok||prev");
    // No era is distinct from era 0.
    let no_era = photon(&k, 9, &tok, b"l", &prev, None, data);
    match parse(&no_era).unwrap() {
        Frame::Photon { era, .. } => assert_eq!(era, None),
        _ => panic!(),
    }
}

#[test]
fn ack_and_pt_ack() {
    let k = key();
    let tok = [0x33; 32];
    let hash = [0x44; 32];
    let bytes = ack(&k, 10, &tok, 777, &hash);
    match parse(&bytes).expect("ack parses") {
        Frame::Ack { tok: t, acked, hash: h, .. } => {
            assert_eq!(t, tok);
            assert_eq!(acked, 777);
            assert_eq!(h, hash);
        }
        f => panic!("not an ack: {f:?}"),
    }
    assert!(parse(&bytes).unwrap().verified());

    let datagram = b"some received datagram bytes";
    let pa = pt_ack(5, datagram);
    match parse(&pa).expect("pt_ack parses") {
        Frame::PtAck { hp } => assert_eq!(hp, *blake3::hash(datagram).as_bytes()),
        f => panic!("not a pt_ack: {f:?}"),
    }
}

#[test]
fn tampered_signature_fails() {
    let k = key();
    let mut bytes = ping(&k, 1);
    let n = bytes.len();
    bytes[n - 20] ^= 0xFF; // flip a signature byte
    // It may still parse structurally, but must not verify.
    if let Some(f) = parse(&bytes) {
        assert!(!f.verified(), "a tampered ping must not verify");
    }
}
