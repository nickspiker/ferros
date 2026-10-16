//! A wave (voice call) and its beam (video): keys, media packets and express signals, mirrored byte for byte from photon/src/wave/{keys,packet,signal}.rs and beam.rs (BEAM.md). Randomness comes from the caller, so this stays no_std: ferros draws it from the kernel's entropy syscall.

use alloc::vec::Vec;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::XChaCha20;
use poly1305::universal_hash::KeyInit as _;
use poly1305::{Key as PolyKey, Poly1305};
use subtle::ConstantTimeEq;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::{EtType, VsfType};
use zeroize::Zeroize;

// ---- keys (wave/keys.rs, beam.rs) ----

/// A direction's key ratchets once per this many packets.
pub const PACKETS_PER_STEP: u32 = 100;

/// Which way media flows; each direction has its own key chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    OriginToAnswer,
    AnswerToOrigin,
}

impl Direction {
    fn label(self) -> &'static [u8] {
        match self {
            Direction::OriginToAnswer => b"o>a",
            Direction::AnswerToOrigin => b"a>o",
        }
    }
}

/// The wave secret: the offer's lane key, the wave id and both sides' nonces.
pub fn derive_wave_secret(offer_lane_key: &[u8; 32], wave_id: &[u8; 16], origin_nonce: &[u8; 32], answer_nonce: &[u8; 32]) -> [u8; 32] {
    let mut material = [0u8; 32 * 3 + 16];
    material[..32].copy_from_slice(offer_lane_key);
    material[32..48].copy_from_slice(wave_id);
    material[48..80].copy_from_slice(origin_nonce);
    material[80..].copy_from_slice(answer_nonce);
    let out = blake3::derive_key("PHOTON_WAVE_v1 wave secret", &material);
    material.zeroize();
    out
}

/// The recording-fill track's root.
pub fn fill_secret(wave_secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("PHOTON_WAVE_v1 recording fill", wave_secret)
}

/// The beam (video) track's root: its datagrams run their own chains and seq space beside audio.
pub fn beam_secret(wave_secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("PHOTON_WAVE_v1 beam video", wave_secret)
}

/// One direction's key chain: step 0 from the secret, then a one-way step every `PACKETS_PER_STEP` packets; a passed key is destroyed and cannot be re-derived.
pub struct StepChain {
    key: [u8; 32],
    step: u32,
}

impl StepChain {
    pub fn new(secret: &[u8; 32], dir: Direction) -> Self {
        let mut material = [0u8; 35];
        material[..32].copy_from_slice(secret);
        material[32..].copy_from_slice(dir.label());
        let key = blake3::derive_key("PHOTON_WAVE_v1 direction step0", &material);
        material.zeroize();
        Self { key, step: 0 }
    }

    pub fn step(&self) -> u32 {
        self.step
    }

    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    pub fn step_for_seq(seq: u32) -> u32 {
        seq / PACKETS_PER_STEP
    }

    /// Ratchet forward to `target`; false if `target` is behind (that key no longer exists anywhere).
    pub fn advance_to(&mut self, target: u32) -> bool {
        if target < self.step {
            return false;
        }
        while self.step < target {
            let next = blake3::derive_key("PHOTON_WAVE_v1 step", &self.key);
            self.key.zeroize();
            self.key = next;
            self.step += 1;
        }
        true
    }
}

impl Drop for StepChain {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

// ---- media packets (wave/packet.rs) ----

pub const MEDIA_MAGIC: u8 = 0xC7;
pub const FILL_MAGIC: u8 = 0xC8;
pub const BEAM_MAGIC: u8 = 0xCA;
pub const HEADER_LEN: usize = 1 + 4;
/// The tag on the wire: the first 4 bytes of the Poly1305 tag.
pub const TAG_LEN: usize = 4;

fn cipher_and_poly_key(key: &[u8; 32], nonce: &[u8; 24]) -> (XChaCha20, [u8; 32]) {
    let mut cipher = XChaCha20::new(key.into(), nonce.into());
    let mut block0 = [0u8; 64];
    cipher.apply_keystream(&mut block0);
    let mut poly_key = [0u8; 32];
    poly_key.copy_from_slice(&block0[..32]);
    (cipher, poly_key)
}

/// RFC 8439's tag over the ciphertext with no associated data.
fn full_tag(poly_key: &[u8; 32], ct: &[u8]) -> [u8; 16] {
    let pad = (16 - (ct.len() % 16)) % 16;
    let mut m = Vec::with_capacity(ct.len() + pad + 16);
    m.extend_from_slice(ct);
    m.resize(ct.len() + pad, 0);
    m.extend_from_slice(&0u64.to_le_bytes());
    m.extend_from_slice(&(ct.len() as u64).to_le_bytes());
    Poly1305::new(PolyKey::from_slice(poly_key)).compute_unpadded(&m).into()
}

fn nonce_for(seq: u32) -> [u8; 24] {
    let mut nonce = [0u8; 24];
    nonce[..4].copy_from_slice(&seq.to_le_bytes());
    nonce
}

/// Seal one media datagram: `[magic][seq u32 LE][XChaCha20 ciphertext][4-byte tag]`. The chain must sit at `seq`'s step.
pub fn seal(chain: &StepChain, seq: u32, payload: &[u8], magic: u8) -> Vec<u8> {
    let nonce = nonce_for(seq);
    let (mut cipher, poly_key) = cipher_and_poly_key(chain.key(), &nonce);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TAG_LEN);
    out.push(magic);
    out.extend_from_slice(&seq.to_le_bytes());
    let start = out.len();
    out.extend_from_slice(payload);
    cipher.apply_keystream(&mut out[start..]);
    let tag = full_tag(&poly_key, &out[start..]);
    out.extend_from_slice(&tag[..TAG_LEN]);
    out
}

/// A media datagram's magic and seq, and its sealed remainder.
pub fn parse_header(bytes: &[u8]) -> Option<(u8, u32, &[u8])> {
    if bytes.len() <= HEADER_LEN + TAG_LEN || !matches!(bytes[0], MEDIA_MAGIC | FILL_MAGIC | BEAM_MAGIC) {
        return None;
    }
    Some((bytes[0], u32::from_le_bytes(bytes[1..5].try_into().ok()?), &bytes[HEADER_LEN..]))
}

/// Open a sealed remainder at `seq`, ratcheting the chain forward as needed. None if the tag fails or the seq is behind the chain.
pub fn open(chain: &mut StepChain, seq: u32, sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() <= TAG_LEN || !chain.advance_to(StepChain::step_for_seq(seq)) {
        return None;
    }
    let (ct, wire_tag) = sealed.split_at(sealed.len() - TAG_LEN);
    let (mut cipher, poly_key) = cipher_and_poly_key(chain.key(), &nonce_for(seq));
    if full_tag(&poly_key, ct)[..TAG_LEN].ct_eq(wire_tag).unwrap_u8() != 1 {
        return None;
    }
    let mut pt = ct.to_vec();
    cipher.apply_keystream(&mut pt);
    Some(pt)
}

// ---- express signals (wave/signal.rs, types/row_control.rs) ----

pub const EXPRESS_MAGIC: u8 = 0xC9;
const EXPRESS_NONCE_LEN: usize = 24;
const EXPRESS_SECTION: &str = "wexp";
const CTL_WAVE: usize = 3;

/// A wave's signals. `Anchor` is express-only: "re-point your media at my source address".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveSignal {
    Offer { wave_id: [u8; 16], nonce: [u8; 32], device: Option<[u8; 32]> },
    Answer { wave_id: [u8; 16], nonce: [u8; 32], device: Option<[u8; 32]> },
    Decline { wave_id: [u8; 16] },
    Busy { wave_id: [u8; 16] },
    Hangup { wave_id: [u8; 16] },
    Taken { wave_id: [u8; 16] },
    Anchor { wave_id: [u8; 16] },
}

impl WaveSignal {
    pub fn wave_id(&self) -> &[u8; 16] {
        match self {
            WaveSignal::Offer { wave_id, .. } | WaveSignal::Answer { wave_id, .. } | WaveSignal::Decline { wave_id } | WaveSignal::Busy { wave_id } | WaveSignal::Hangup { wave_id } | WaveSignal::Taken { wave_id } | WaveSignal::Anchor { wave_id } => wave_id,
        }
    }

    /// The row-control sub-kind, nonce and device (row_control.rs `slots`).
    fn slots(&self) -> (usize, Option<[u8; 32]>, Option<[u8; 32]>) {
        match *self {
            WaveSignal::Offer { nonce, device, .. } => (1, Some(nonce), device),
            WaveSignal::Answer { nonce, device, .. } => (2, Some(nonce), device),
            WaveSignal::Decline { .. } => (3, None, None),
            WaveSignal::Busy { .. } => (4, None, None),
            WaveSignal::Hangup { .. } => (5, None, None),
            WaveSignal::Taken { .. } => (6, None, None),
            WaveSignal::Anchor { .. } => (7, None, None),
        }
    }
}

/// The express key: from the friendship's lane root and history key.
pub fn express_key(lane_root: &[u8; 32], history_key: &[u8; 32]) -> [u8; 32] {
    let mut material = [0u8; 64];
    material[..32].copy_from_slice(lane_root);
    material[32..].copy_from_slice(history_key);
    blake3::derive_key("PHOTON_WAVE_v1 express signal", &material)
}

/// The express section's schema: `ts`, `lk`, `bm`, then the row-control fields, declared exactly as Photon declares them.
fn express_schema() -> SectionSchema {
    SectionSchema::new(EXPRESS_SECTION)
        .field("ts", TypeConstraint::Any)
        .field("lk", TypeConstraint::Any)
        .field("bm", TypeConstraint::Any)
        .field("ck", TypeConstraint::AnyUnsigned)
        .field("cs", TypeConstraint::AnyUnsigned)
        .field("cts", TypeConstraint::Any)
        .field("cid", TypeConstraint::Any)
        .field("cn", TypeConstraint::Any)
        .field("cd", TypeConstraint::AnyKey)
        .field("cnum", TypeConstraint::AnyUnsigned)
        .field("ctag", TypeConstraint::AnyUnsigned)
        .field("cset", TypeConstraint::AnyUnsigned)
        .field("fr", TypeConstraint::AnyUnsigned)
        .field("fh", TypeConstraint::AnyHash)
        .field("fnm", TypeConstraint::Utf8Text)
        .field("fz", TypeConstraint::AnyUnsigned)
        .field("fhd", TypeConstraint::AnyHash)
}

/// The express plaintext: a VSF document stamped `ts`, provenance only, with the `wexp` section.
pub fn express_payload(ts: i64, lane_key: Option<&[u8; 32]>, sig: &WaveSignal, beam: bool) -> Option<Vec<u8>> {
    let mut b = express_schema().build().set("ts", VsfType::e(EtType::e6(ts))).ok()?;
    if let Some(k) = lane_key {
        b = b.set("lk", VsfType::hR(k.to_vec())).ok()?;
    }
    if beam && matches!(sig, WaveSignal::Offer { .. }) {
        b = b.set("bm", VsfType::u3(1)).ok()?;
    }
    let (sub, nonce, dev) = sig.slots();
    b = b.set("ck", VsfType::u(CTL_WAVE, false)).ok()?.set("cs", VsfType::u(sub, false)).ok()?;
    b = b.set("cid", VsfType::hR(sig.wave_id().to_vec())).ok()?;
    if let Some(n) = nonce {
        b = b.set("cn", VsfType::hR(n.to_vec())).ok()?;
    }
    if let Some(d) = dev {
        b = b.set("cd", VsfType::ke(d.to_vec())).ok()?;
    }
    let section = b.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(ts).provenance_only().add_unboxed(EXPRESS_SECTION, section).build().ok()
}

/// Seal an express frame: `[G#C9][nonce 24][XChaCha20-Poly1305(payload)]`. `nonce` must be fresh random bytes (the receiver drops a repeated nonce).
pub fn seal_express(key: &[u8; 32], nonce: [u8; 24], ts: i64, lane_key: Option<&[u8; 32]>, sig: &WaveSignal, beam: bool) -> Option<Vec<u8>> {
    use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
    let payload = express_payload(ts, lane_key, sig, beam)?;
    let sealed = XChaCha20Poly1305::new_from_slice(key).ok()?.encrypt(&XNonce::from(nonce), payload.as_slice()).ok()?;
    let mut out = Vec::with_capacity(1 + EXPRESS_NONCE_LEN + sealed.len());
    out.push(EXPRESS_MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Some(out)
}

/// Open an express frame: its stamp, the offer's lane key if carried, the signal, and whether an offer carries a beam. The caller checks the stamp is within ±30 s of its own eagle time and drops repeated nonces.
pub fn open_express(key: &[u8; 32], bytes: &[u8]) -> Option<(i64, Option<[u8; 32]>, WaveSignal, bool)> {
    use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
    if bytes.len() <= 1 + EXPRESS_NONCE_LEN + 16 || bytes[0] != EXPRESS_MAGIC {
        return None;
    }
    let nonce = XNonce::from_slice(&bytes[1..1 + EXPRESS_NONCE_LEN]);
    let payload = XChaCha20Poly1305::new_from_slice(key).ok()?.decrypt(nonce, &bytes[1 + EXPRESS_NONCE_LEN..]).ok()?;
    read_express_payload(&payload)
}

fn read_express_payload(payload: &[u8]) -> Option<(i64, Option<[u8; 32]>, WaveSignal, bool)> {
    let section = SectionBuilder::parse_document(express_schema(), payload, None).ok()?;
    let first = |name: &str| section.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    let bytes32 = |v: Option<VsfType>| match v {
        Some(VsfType::hR(b)) | Some(VsfType::ke(b)) => <[u8; 32]>::try_from(b.as_slice()).ok(),
        _ => None,
    };
    let ts = match first("ts")? {
        VsfType::e(EtType::e6(t)) => t,
        _ => return None,
    };
    let lane_key = bytes32(first("lk"));
    let beam = !section.get_fields("bm").is_empty();
    if first("ck")?.as_u64()? != CTL_WAVE as u64 {
        return None;
    }
    let sub = first("cs").and_then(|v| v.as_u64()).unwrap_or(0);
    let wave_id: [u8; 16] = match first("cid")? {
        VsfType::hR(b) => b.as_slice().try_into().ok()?,
        _ => return None,
    };
    let (nonce, device) = (bytes32(first("cn")), bytes32(first("cd")));
    let sig = match sub {
        1 => WaveSignal::Offer { wave_id, nonce: nonce?, device },
        2 => WaveSignal::Answer { wave_id, nonce: nonce?, device },
        3 => WaveSignal::Decline { wave_id },
        4 => WaveSignal::Busy { wave_id },
        5 => WaveSignal::Hangup { wave_id },
        6 => WaveSignal::Taken { wave_id },
        7 => WaveSignal::Anchor { wave_id },
        _ => return None,
    };
    Some((ts, lane_key, sig, beam))
}
