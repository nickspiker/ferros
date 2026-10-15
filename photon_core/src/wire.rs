//! Wire frames: every Photon datagram is one VSF document (`RÅ<`). The header carries the eagle time (`e6`, oscillations), a provenance hash (`hp`), the sender's device Ed25519 key (`ke`) and its signature over the 32-byte provenance only (`ge`); the section name says what the frame is (photon/src/network/fgtw/protocol.rs to_vsf_bytes / from_vsf_bytes).
//!
//! - `ping`: header only; provenance = BLAKE3(device key ‖ time LE).
//! - `pong`: the ping's provenance, signed by the responder; optional `obs` (the pinger's observed address: IP bytes + port BE) and `enc` (a sealed tail this side doesn't produce).
//! - `msg` (a photon): `tok` conversation token, `lane`, `prev` (the previous frame's hash), optional `era`, `data` ciphertext; provenance = BLAKE3(tok ‖ prev).
//! - `ack`: `tok`, `time` (the acked photon's eagle time), `hash` (its plaintext hash); provenance = BLAKE3(tok ‖ time LE ‖ hash ‖ "ack").
//! - `pt_ack`: unsigned; provenance = BLAKE3(the acked datagram), inline `pt_ack:u0,u0` — the small-packet delivery ack (pt/packets.rs).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use vsf::file_format::VsfHeader;
use vsf::types::{EtType, VsfType};
use vsf::{Tensor, VsfBuilder};

/// Photon's UDP port.
pub const PORT: u16 = 4383;

/// A parsed frame. Signature not yet checked: see [`Frame::verified`].
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Ping { time: i64, key: [u8; 32], hp: [u8; 32], sig: [u8; 64] },
    Pong { time: i64, key: [u8; 32], hp: [u8; 32], sig: [u8; 64], obs: Option<Vec<u8>> },
    Photon { time: i64, key: [u8; 32], sig: [u8; 64], tok: [u8; 32], lane: Vec<u8>, prev: [u8; 32], era: Option<u64>, data: Vec<u8> },
    Ack { time: i64, key: [u8; 32], sig: [u8; 64], tok: [u8; 32], acked: i64, hash: [u8; 32] },
    PtAck { hp: [u8; 32] },
    /// A well-formed VSF frame of a kind this core doesn't handle (its section name).
    Other(String),
}

pub fn ping_provenance(key: &[u8; 32], time: i64) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(key);
    h.update(&time.to_le_bytes());
    *h.finalize().as_bytes()
}

pub fn photon_provenance(tok: &[u8; 32], prev: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(tok);
    h.update(prev);
    *h.finalize().as_bytes()
}

pub fn ack_provenance(tok: &[u8; 32], acked: i64, hash: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(tok);
    h.update(&acked.to_le_bytes());
    h.update(hash);
    h.update(b"ack");
    *h.finalize().as_bytes()
}

fn signed(key: &SigningKey, time: i64, hp: [u8; 32]) -> VsfBuilder {
    let sig = key.sign(&hp).to_bytes();
    VsfBuilder::new().creation_time_oscillations(time).provenance_hash(hp).signature_ed25519(key.verifying_key().to_bytes(), sig)
}

/// A presence ping from this device.
pub fn ping(key: &SigningKey, time: i64) -> Vec<u8> {
    let hp = ping_provenance(&key.verifying_key().to_bytes(), time);
    signed(key, time, hp).add_section("ping", vec![]).build().unwrap_or_default()
}

/// The answer to a ping with provenance `ping_hp`, echoing the address it came from (`obs`: IP bytes + port big-endian).
pub fn pong(key: &SigningKey, time: i64, ping_hp: [u8; 32], obs: Option<&[u8]>) -> Vec<u8> {
    let mut section = vsf::VsfSection::new("pong");
    if let Some(a) = obs {
        section.add_field_multi("obs", vec![VsfType::hb(a.to_vec())]);
    }
    signed(key, time, ping_hp).add_section_direct(section).build().unwrap_or_default()
}

/// A photon: the encrypted payload on `lane`, chained to `prev`.
pub fn photon(key: &SigningKey, time: i64, tok: &[u8; 32], lane: &[u8], prev: &[u8; 32], era: Option<u64>, data: &[u8]) -> Vec<u8> {
    let mut fields = vec![
        (String::from("tok"), VsfType::hg(tok.to_vec())),
        (String::from("lane"), VsfType::hb(lane.to_vec())),
        (String::from("prev"), VsfType::hp(prev.to_vec())),
    ];
    if let Some(e) = era {
        fields.push((String::from("era"), VsfType::u(e as usize, false)));
    }
    fields.push((String::from("data"), VsfType::t_u3(Tensor::new(vec![data.len()], data.to_vec()))));
    signed(key, time, photon_provenance(tok, prev)).add_section("msg", fields).build().unwrap_or_default()
}

/// Acknowledge a received photon (its eagle time and plaintext hash).
pub fn ack(key: &SigningKey, time: i64, tok: &[u8; 32], acked: i64, hash: &[u8; 32]) -> Vec<u8> {
    let fields = vec![
        (String::from("tok"), VsfType::hg(tok.to_vec())),
        (String::from("time"), VsfType::e(EtType::e6(acked))),
        (String::from("hash"), VsfType::hb(hash.to_vec())),
    ];
    signed(key, time, ack_provenance(tok, acked, hash)).add_section("ack", fields).build().unwrap_or_default()
}

/// The small-packet delivery ack for a received datagram (unsigned: its provenance is the datagram's hash).
pub fn pt_ack(time: i64, datagram: &[u8]) -> Vec<u8> {
    VsfBuilder::new()
        .creation_time_oscillations(time)
        .provenance_hash(*blake3::hash(datagram).as_bytes())
        .provenance_only()
        .add_inline_field("pt_ack", vec![VsfType::u(0, false), VsfType::u(0, false)])
        .build()
        .unwrap_or_default()
}

fn arr<const N: usize>(v: &[u8]) -> Option<[u8; N]> {
    v.try_into().ok()
}

/// Parse a datagram (no signature check).
pub fn parse(bytes: &[u8]) -> Option<Frame> {
    if bytes.len() < 4 || &bytes[0..3] != "RÅ".as_bytes() || bytes[3] != b'<' {
        return None;
    }
    let (header, end) = VsfHeader::decode(bytes).ok()?;
    let hp = match &header.provenance_hash {
        VsfType::hp(b) => arr::<32>(b),
        _ => None,
    };
    if header.fields.iter().any(|f| f.name == "pt_ack") {
        return Some(Frame::PtAck { hp: hp? });
    }
    let time = match &header.creation_time {
        Some(VsfType::e(EtType::e6(v))) => Some(*v),
        _ => None,
    };
    let key = match &header.signer_pubkey {
        Some(VsfType::ke(b)) => arr::<32>(b),
        _ => None,
    };
    let sig = match &header.signature {
        Some(VsfType::ge(b)) => arr::<64>(b),
        _ => None,
    };
    let section = header.primary_section(bytes, end).ok()?;
    let one = |name: &str| section.get_field(name).and_then(|f| f.values.first().cloned());
    let (time, key, sig) = match (time, key, sig) {
        (Some(t), Some(k), Some(s)) => (t, k, s),
        _ => return Some(Frame::Other(section.name.clone())),
    };
    Some(match section.name.as_str() {
        "ping" => Frame::Ping { time, key, hp: hp?, sig },
        "pong" => Frame::Pong {
            time,
            key,
            hp: hp?,
            sig,
            obs: match one("obs") {
                Some(VsfType::hb(b)) => Some(b),
                _ => None,
            },
        },
        "msg" => {
            let tok = match one("tok")? {
                VsfType::hg(b) => arr::<32>(&b)?,
                _ => return None,
            };
            let lane = match one("lane")? {
                VsfType::hb(b) => b,
                _ => return None,
            };
            let prev = match one("prev")? {
                VsfType::hp(b) => arr::<32>(&b)?,
                _ => return None,
            };
            let era = match one("era") {
                Some(VsfType::u(e, _)) => Some(e as u64),
                _ => None,
            };
            let data = match one("data")? {
                VsfType::t_u3(t) => t.data,
                _ => return None,
            };
            Frame::Photon { time, key, sig, tok, lane, prev, era, data }
        }
        "ack" => {
            let tok = match one("tok")? {
                VsfType::hg(b) => arr::<32>(&b)?,
                _ => return None,
            };
            let acked = match one("time")? {
                VsfType::e(EtType::e6(v)) => v,
                _ => return None,
            };
            let hash = match one("hash")? {
                VsfType::hb(b) => arr::<32>(&b)?,
                _ => return None,
            };
            Frame::Ack { time, key, sig, tok, acked, hash }
        }
        other => Frame::Other(String::from(other)),
    })
}

impl Frame {
    /// The signer's device key, for signed frames.
    pub fn key(&self) -> Option<[u8; 32]> {
        match self {
            Frame::Ping { key, .. } | Frame::Pong { key, .. } | Frame::Photon { key, .. } | Frame::Ack { key, .. } => Some(*key),
            _ => None,
        }
    }

    /// Whether the Ed25519 signature verifies over the provenance this frame kind must carry (recomputed for photons and acks; for ping/pong the header's, which ties a pong to its ping).
    pub fn verified(&self) -> bool {
        let (key, hp, sig) = match self {
            Frame::Ping { key, hp, sig, .. } | Frame::Pong { key, hp, sig, .. } => (*key, *hp, *sig),
            Frame::Photon { key, sig, tok, prev, .. } => (*key, photon_provenance(tok, prev), *sig),
            Frame::Ack { key, sig, tok, acked, hash, .. } => (*key, ack_provenance(tok, *acked, hash), *sig),
            _ => return false,
        };
        match VerifyingKey::from_bytes(&key) {
            Ok(vk) => vk.verify(&hp, &Signature::from_bytes(&sig)).is_ok(),
            Err(_) => false,
        }
    }
}
