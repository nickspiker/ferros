//! photon_core against the stock Photon app's own code (the photon-messenger library), byte for byte.
//!
//! Ed25519 signatures are deterministic and these frames take their header time from the frame, so for the same key and inputs both sides must produce identical bytes; any difference is an interop break with a real phone. Each frame is also parsed by the other side.

use ed25519_dalek::{Signer, SigningKey};
use photon::crypto::chain as pchain;
use photon::network::fgtw::FgtwMessage;
use photon::types::DevicePubkey;
use photon_core::chain as cchain;
use photon_core::wire;
use std::net::SocketAddr;
use vsf::EagleTime;

const T: i64 = 1_420_000_000_000;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn pk(k: &SigningKey) -> DevicePubkey {
    DevicePubkey::from_bytes(k.verifying_key().to_bytes())
}

#[test]
fn ping_matches() {
    let k = key();
    let hp = wire::ping_provenance(&k.verifying_key().to_bytes(), T);
    let theirs = FgtwMessage::StatusPing { timestamp: T, sender_pubkey: pk(&k), provenance_hash: hp, signature: k.sign(&hp).to_bytes() }.to_vsf_bytes();
    let ours = wire::ping(&k, T);
    assert_eq!(ours, theirs, "ping bytes");
    match FgtwMessage::from_vsf_bytes(&ours).expect("Photon parses our ping") {
        FgtwMessage::StatusPing { timestamp, provenance_hash, .. } => assert_eq!((timestamp, provenance_hash), (T, hp)),
        m => panic!("Photon read our ping as {m:?}"),
    }
}

#[test]
fn pong_matches() {
    let k = key();
    let hp = wire::ping_provenance(&[9u8; 32], T - 5);
    let addr: SocketAddr = "192.168.1.50:4383".parse().unwrap();
    let obs = [192, 168, 1, 50, 0x11, 0x1F];
    let theirs = FgtwMessage::StatusPong { timestamp: T, responder_pubkey: pk(&k), provenance_hash: hp, signature: k.sign(&hp).to_bytes(), sync_records: vec![], observed_addr: Some(addr), display_name: None, avatar_pin: None, sealed: None }.to_vsf_bytes();
    let ours = wire::pong(&k, T, hp, Some(&obs));
    assert_eq!(ours, theirs, "pong bytes");
    match FgtwMessage::from_vsf_bytes(&ours).expect("Photon parses our pong") {
        FgtwMessage::StatusPong { observed_addr, provenance_hash, .. } => assert_eq!((observed_addr, provenance_hash), (Some(addr), hp)),
        m => panic!("Photon read our pong as {m:?}"),
    }
}

#[test]
fn photon_frame_matches() {
    let k = key();
    let (tok, prev, lane) = ([0x11u8; 32], [0x22u8; 32], [0x33u8; 32]);
    let data = b"ciphertext bytes of some length".to_vec();
    for era in [None, Some(300u32), Some(70_000)] {
        let hp = wire::photon_provenance(&tok, &prev);
        let theirs = FgtwMessage::ChatMessage { timestamp: T, conversation_token: tok, lane, prev_msg_hp: prev, ciphertext: data.clone(), sender_pubkey: pk(&k), signature: k.sign(&hp).to_bytes(), era }.to_vsf_bytes();
        let ours = wire::photon(&k, T, &tok, &lane, &prev, era.map(u64::from), &data);
        assert_eq!(ours, theirs, "photon frame bytes, era {era:?}");
        match FgtwMessage::from_vsf_bytes(&ours).expect("Photon parses our photon") {
            FgtwMessage::ChatMessage { era: e, lane: l, ciphertext, .. } => assert_eq!((e, l, ciphertext), (era, lane, data.clone())),
            m => panic!("Photon read our photon as {m:?}"),
        }
        // And ours parses theirs.
        match wire::parse(&theirs).expect("we parse Photon's photon") {
            f @ wire::Frame::Photon { .. } => assert!(f.verified(), "Photon's photon verifies here"),
            f => panic!("we read Photon's photon as {f:?}"),
        }
    }
}

#[test]
fn ack_matches() {
    let k = key();
    let (tok, hash) = ([0x44u8; 32], [0x55u8; 32]);
    let acked = T - 1_000_000;
    let hp = wire::ack_provenance(&tok, acked, &hash);
    let theirs = FgtwMessage::MessageAck { timestamp: T, conversation_token: tok, acked_eagle_time: acked, plaintext_hash: hash, sender_pubkey: pk(&k), signature: k.sign(&hp).to_bytes() }.to_vsf_bytes();
    let ours = wire::ack(&k, T, &tok, acked, &hash);
    assert_eq!(ours, theirs, "ack bytes");
}

fn active() -> Vec<u8> {
    (0..8192).map(|i| (i * 7 + 3) as u8).collect()
}

#[test]
fn braid_matches() {
    let (pc, cc) = (pchain::Chain::from_bytes(&active()).unwrap(), cchain::Chain::from_bytes(&active()).unwrap());
    assert_eq!(pc.current_key(), cc.current_key());
    for prev in [&b""[..], b"the previous photon"] {
        let (ps, cs) = (pchain::derive_salt(prev, &pc), cchain::derive_salt(prev, &cc));
        assert_eq!(ps, cs, "salt");
        let (pscr, cscr) = (pchain::generate_scratch(&pc, &ps), cchain::generate_scratch(&cc, &cs));
        assert_eq!(pscr, cscr, "scratch pad");
        let msg = b"a photon body, any length at all \x00\x01\x02";
        let ct = pchain::encrypt_layers(msg, &pc, &pscr, &EagleTime::from_oscillations(T));
        assert_eq!(ct, cchain::encrypt_layers(msg, &cc, &cscr, T), "ciphertext");
        assert_eq!(cchain::decrypt_layers(&ct, &cc, cchain::CURRENT_KEY_INDEX, &cscr, T), msg.to_vec(), "we decrypt Photon's ciphertext");
    }
    assert_eq!(pchain::strand_value(T, b"strand"), cchain::strand_value(T, b"strand"), "braid-v2 strand value");
    let (mut pc, mut cc) = (pc, cc);
    let strands: [&[u8]; 2] = [b"their first", b"their second"];
    pc.advance(&EagleTime::from_oscillations(T + 1), b"ours", &strands);
    cc.advance(T + 1, b"ours", &strands);
    assert_eq!(pc.current_key(), cc.current_key(), "advance");
}
