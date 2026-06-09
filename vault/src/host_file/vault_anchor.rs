//! `VaultAnchor` — the host-file vault's per-slot anchor record.
//!
//! Distinct from [`crate::anchor::MeshAnchor`] for one reason: the vault needs to self-describe its own layout (ring_size, payload_capacity, object_tail), and those fields must be covered by the HMAC. Modifying `MeshAnchor` to add them would fork the ferros hardware-side format; defining a new struct here keeps both formats clean.
//!
//! Wire format (in slot bytes):
//! ```text
//!   [magic: 4 bytes "VLT0"] [version: u8]                   (currently 0) [anchor_seq: u64 LE]            (monotonic; clobbers detection) [ring_size: u32 LE]             (number of slots; anchor self-describes layout) [payload_capacity: u64 LE]      (vault payload bytes; slot offsets % this) [object_tail: u64 LE]           (append cursor; bytes 0..object_tail are claimed) [root_commit: 32 bytes]         (BLAKE3 hash of the root commit object) [hmac: 32 bytes]                (BLAKE3-keyed over all above fields)
//! ```
//! Fixed-width encoding (no EWE) because the vault knows the format intimately and the size is bounded (~93 bytes); EWE would buy nothing here.
//!
//! HMAC covers magic + version + anchor_seq + ring_size + payload_capacity + object_tail + root_commit. Tampering with any field flips the HMAC.
//!
//! Slot 0 is privileged: always at offset 0 of the vault payload. Slots 1..ring_size are at key-derived offsets via [`derive_slot_offset`], scattered through the payload so an attacker without the key can't distinguish slot regions from object regions.

use alloc::vec::Vec;

use crate::anchor::AnchorKey;
use crate::hash::ObjectHash;

/// Slot 0 lives at vault payload offset 0 — privileged bootstrap location. Always read first; tells the reader the ring_size + payload_capacity needed to compute other slot offsets.
pub const SLOT_ZERO_OFFSET: u64 = 0;

/// Bytes reserved per anchor slot. Wire format is ~93 bytes; reserving 128 buys headroom for future fields without re-formatting. Power-of-two for clean alignment.
pub const SLOT_STRIDE: u64 = 128;

/// 4-byte magic at the start of each slot. Helps distinguish "this slot has been written" from "this slot is full of random noise that happens to almost-decode." Combined with the HMAC the false-positive rate is ~zero, but the magic catches the obvious-misalignment cases cheaply.
pub const SLOT_MAGIC: [u8; 4] = *b"VLT0";

/// Current wire-format version. Bumped only if the binary layout above changes incompatibly.
pub const SLOT_VERSION: u8 = 0;

/// Vault anchor — the host-file equivalent of [`crate::anchor::MeshAnchor`] with extra self-describing layout fields covered by the HMAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultAnchor {
    /// Sequence number; monotonically increasing across all writes. Slot for this anchor is `anchor_seq % ring_size`. On read, the latest valid anchor is the one with the highest seq across all slots.
    pub anchor_seq: u64,
    /// Current ring size — number of slots in the anchor ring. Self-describing; the slot 0 anchor tells the reader how many slots to scan.
    pub ring_size: u32,
    /// Current vault payload size in bytes. Slot offsets are computed modulo this. Independent of the underlying file size (the file may be larger; payload_capacity is what the anchor declares as usable).
    pub payload_capacity: u64,
    /// Append cursor for the object store region — bytes `[0, object_tail)` are claimed (anchor slots + appended objects); free space is `[object_tail, payload_capacity)`.
    pub object_tail: u64,
    /// BLAKE3 hash of the root commit object (the `logical_key → content_hash` dictionary in Photon's case).
    pub root_commit: ObjectHash,
    /// BLAKE3-keyed HMAC over (magic || version || anchor_seq || ring_size || payload_capacity || object_tail || root_commit). Verified on read; tampering with any covered field flips this.
    pub hmac: [u8; 32],
}

/// Errors from anchor encode/decode/verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// Slot bytes are shorter than the minimum anchor size.
    TooShort,
    /// First 4 bytes don't match `SLOT_MAGIC` — slot was never written, or was overwritten by something unrelated.
    BadMagic,
    /// Version byte is unrecognized — likely a future-format anchor we don't know how to parse.
    UnsupportedVersion(u8),
    /// HMAC verification failed — wrong key OR tampered fields OR uninitialized slot that happened to pass the magic check.
    HmacMismatch,
}

/// Compute the HMAC over the anchor's covered fields. Same input shape as encode; same output as the `hmac` field on a freshly-built anchor.
pub fn compute_hmac(anchor: &VaultAnchor, key: &AnchorKey) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(&key.0);
    h.update(&SLOT_MAGIC);
    h.update(&[SLOT_VERSION]);
    h.update(&anchor.anchor_seq.to_le_bytes());
    h.update(&anchor.ring_size.to_le_bytes());
    h.update(&anchor.payload_capacity.to_le_bytes());
    h.update(&anchor.object_tail.to_le_bytes());
    h.update(&anchor.root_commit.0);
    *h.finalize().as_bytes()
}

/// Constant-time comparison of two 32-byte HMACs. Returns true if equal.
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Encode an anchor into `SLOT_STRIDE` bytes ready to write to disk. Bytes past the fixed-format payload are left as zeros (or whatever the caller passed in); the decode side stops at the known field count.
pub fn encode(anchor: &VaultAnchor) -> [u8; SLOT_STRIDE as usize] {
    let mut buf = [0u8; SLOT_STRIDE as usize];
    let mut p = 0usize;
    buf[p..p + 4].copy_from_slice(&SLOT_MAGIC);
    p += 4;
    buf[p] = SLOT_VERSION;
    p += 1;
    buf[p..p + 8].copy_from_slice(&anchor.anchor_seq.to_le_bytes());
    p += 8;
    buf[p..p + 4].copy_from_slice(&anchor.ring_size.to_le_bytes());
    p += 4;
    buf[p..p + 8].copy_from_slice(&anchor.payload_capacity.to_le_bytes());
    p += 8;
    buf[p..p + 8].copy_from_slice(&anchor.object_tail.to_le_bytes());
    p += 8;
    buf[p..p + 32].copy_from_slice(&anchor.root_commit.0);
    p += 32;
    buf[p..p + 32].copy_from_slice(&anchor.hmac);
    // Remaining bytes (~31 of SLOT_STRIDE=128) left as zeros — irrelevant, the decoder ignores them.
    buf
}

/// Decode an anchor from `SLOT_STRIDE` bytes. Verifies magic + version + HMAC. Returns the parsed anchor only if all checks pass.
///
/// HMAC verification is part of decode rather than a separate step: a "decoded but un-verified" anchor is a footgun the caller shouldn't have to remember to handle. If you got an anchor back, it's good.
pub fn decode(bytes: &[u8], key: &AnchorKey) -> Result<VaultAnchor, AnchorError> {
    // Minimum field bytes: 4 magic + 1 version + 8 seq + 4 ring_size + 8 cap + 8 tail + 32 root + 32 hmac = 97.
    if bytes.len() < 97 {
        return Err(AnchorError::TooShort);
    }
    if bytes[0..4] != SLOT_MAGIC {
        return Err(AnchorError::BadMagic);
    }
    let version = bytes[4];
    if version != SLOT_VERSION {
        return Err(AnchorError::UnsupportedVersion(version));
    }
    let mut p = 5usize;
    let anchor_seq = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
    p += 8;
    let ring_size = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
    p += 4;
    let payload_capacity = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
    p += 8;
    let object_tail = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
    p += 8;
    let mut root_commit_bytes = [0u8; 32];
    root_commit_bytes.copy_from_slice(&bytes[p..p + 32]);
    p += 32;
    let mut hmac = [0u8; 32];
    hmac.copy_from_slice(&bytes[p..p + 32]);

    let anchor = VaultAnchor {
        anchor_seq,
        ring_size,
        payload_capacity,
        object_tail,
        root_commit: ObjectHash(root_commit_bytes),
        hmac,
    };
    let expected = compute_hmac(&anchor, key);
    if !ct_eq(&anchor.hmac, &expected) {
        return Err(AnchorError::HmacMismatch);
    }
    Ok(anchor)
}

/// Compute the slot offset for slot N within the vault payload.
///
/// Slot 0 is always at offset 0 of the payload — privileged bootstrap location. Slots 1..ring_size are scattered at key-derived offsets within `[SLOT_STRIDE, payload_capacity - SLOT_STRIDE)`, with linear probing on collision against previously-claimed slots (caller is responsible for collision avoidance during ring sizing).
///
/// Collision handling: each call is deterministic — same inputs → same offset. If two slot indices N₁ ≠ N₂ produce the same offset (birthday collision in a small payload), this function returns the same offset for both. The caller's write path must detect and probe forward (see `derive_slot_offset_with_probe`).
pub fn derive_slot_offset(key: &AnchorKey, slot_index: u32, payload_capacity: u64) -> u64 {
    if slot_index == 0 {
        return SLOT_ZERO_OFFSET;
    }
    let mut h = blake3::Hasher::new_keyed(&key.0);
    h.update(b"vault.slot_offset.v0");
    h.update(&slot_index.to_le_bytes());
    let hash = h.finalize();
    let raw = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
    // Reserve the first SLOT_STRIDE bytes for slot 0; mod into the remaining space.
    let usable = payload_capacity.saturating_sub(2 * SLOT_STRIDE).max(1);
    SLOT_STRIDE + (raw % usable)
}

/// Probe forward to resolve a slot offset collision. Calls `derive_slot_offset` then, if the offset was already claimed by another slot or by the object_tail region, picks a probe-derived offset and tries again. Returns `None` if no free offset is found within `max_probes` attempts.
///
/// `claimed_offsets` is the set of offsets already taken by lower-index slots (caller passes this in; only the first `slot_index` slots are populated by the time we derive slot N). `object_tail` is the current append cursor; offsets `< object_tail + SLOT_STRIDE` would collide with appended object data.
pub fn derive_slot_offset_with_probe(
    key: &AnchorKey,
    slot_index: u32,
    payload_capacity: u64,
    claimed_offsets: &[u64],
    object_tail: u64,
    max_probes: u32,
) -> Option<u64> {
    if slot_index == 0 {
        return Some(SLOT_ZERO_OFFSET);
    }
    let claimed_by_objects = |off: u64| off < object_tail.saturating_add(SLOT_STRIDE);
    let claimed_by_slot = |off: u64| {
        claimed_offsets
            .iter()
            .any(|&c| off.abs_diff(c) < SLOT_STRIDE)
    };

    // First try the canonical offset, then up to max_probes alternates.
    let mut h = blake3::Hasher::new_keyed(&key.0);
    h.update(b"vault.slot_offset.v0");
    h.update(&slot_index.to_le_bytes());
    let usable = payload_capacity.saturating_sub(2 * SLOT_STRIDE).max(1);

    let raw = u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap());
    let candidate = SLOT_STRIDE + (raw % usable);
    if !claimed_by_objects(candidate) && !claimed_by_slot(candidate) {
        return Some(candidate);
    }
    for probe in 0..max_probes {
        let mut h2 = blake3::Hasher::new_keyed(&key.0);
        h2.update(b"vault.slot_offset.v0");
        h2.update(&slot_index.to_le_bytes());
        h2.update(b"probe");
        h2.update(&probe.to_le_bytes());
        let raw_p = u64::from_le_bytes(h2.finalize().as_bytes()[..8].try_into().unwrap());
        let candidate_p = SLOT_STRIDE + (raw_p % usable);
        if !claimed_by_objects(candidate_p) && !claimed_by_slot(candidate_p) {
            return Some(candidate_p);
        }
    }
    None
}

/// Build a fresh anchor with the given fields and the computed HMAC. Caller supplies everything except the HMAC; this fills it in. Used at write time.
pub fn build(
    anchor_seq: u64,
    ring_size: u32,
    payload_capacity: u64,
    object_tail: u64,
    root_commit: ObjectHash,
    key: &AnchorKey,
) -> VaultAnchor {
    let mut anchor = VaultAnchor {
        anchor_seq,
        ring_size,
        payload_capacity,
        object_tail,
        root_commit,
        hmac: [0u8; 32],
    };
    anchor.hmac = compute_hmac(&anchor, key);
    anchor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> AnchorKey {
        AnchorKey([0x42u8; 32])
    }

    fn sample_anchor(seq: u64) -> VaultAnchor {
        build(
            seq,
            16,
            64 * 1024,
            1024,
            ObjectHash([0xAAu8; 32]),
            &test_key(),
        )
    }

    #[test]
    fn round_trip_encode_decode() {
        let anchor = sample_anchor(7);
        let bytes = encode(&anchor);
        let decoded = decode(&bytes, &test_key()).unwrap();
        assert_eq!(anchor, decoded);
    }

    #[test]
    fn tampered_byte_fails_hmac() {
        let anchor = sample_anchor(7);
        let mut bytes = encode(&anchor);
        // Flip a bit in the ring_size field (offset 4+1+8 = 13)
        bytes[13] ^= 0x01;
        let res = decode(&bytes, &test_key());
        assert!(matches!(res, Err(AnchorError::HmacMismatch)));
    }

    #[test]
    fn wrong_key_fails_hmac() {
        let anchor = sample_anchor(1);
        let bytes = encode(&anchor);
        let other_key = AnchorKey([0x99u8; 32]);
        let res = decode(&bytes, &other_key);
        assert!(matches!(res, Err(AnchorError::HmacMismatch)));
    }

    #[test]
    fn zero_bytes_fail_magic() {
        let bytes = [0u8; SLOT_STRIDE as usize];
        let res = decode(&bytes, &test_key());
        assert!(matches!(res, Err(AnchorError::BadMagic)));
    }

    #[test]
    fn unsupported_version_rejected() {
        let anchor = sample_anchor(1);
        let mut bytes = encode(&anchor);
        bytes[4] = 99; // bump version byte to nonsense
        let res = decode(&bytes, &test_key());
        assert!(matches!(res, Err(AnchorError::UnsupportedVersion(99))));
    }

    #[test]
    fn slot_zero_is_at_zero() {
        assert_eq!(derive_slot_offset(&test_key(), 0, 64 * 1024), 0);
    }

    #[test]
    fn slot_offsets_are_within_payload() {
        let key = test_key();
        let cap = 64 * 1024;
        for i in 1..256u32 {
            let off = derive_slot_offset(&key, i, cap);
            assert!(off >= SLOT_STRIDE);
            assert!(off + SLOT_STRIDE <= cap);
        }
    }

    #[test]
    fn slot_offsets_are_deterministic() {
        let key = test_key();
        let cap = 64 * 1024;
        for i in 0..32u32 {
            assert_eq!(
                derive_slot_offset(&key, i, cap),
                derive_slot_offset(&key, i, cap)
            );
        }
    }

    #[test]
    fn probe_avoids_object_region() {
        let key = test_key();
        let cap = 8 * 1024; // small payload to make collisions likely
        // Object tail is at half the payload — slot offsets must avoid the lower half.
        let object_tail = cap / 2;
        for i in 1..16u32 {
            let off = derive_slot_offset_with_probe(&key, i, cap, &[], object_tail, 8);
            if let Some(o) = off {
                assert!(
                    o >= object_tail + SLOT_STRIDE,
                    "slot {} offset {} collided with object region (tail {})",
                    i, o, object_tail
                );
            }
        }
    }

    #[test]
    fn probe_avoids_claimed_slots() {
        let key = test_key();
        let cap = 64 * 1024;
        // Pretend slot 1 is already at offset 2000; slot 2's probe must not land within SLOT_STRIDE of that.
        let claimed: Vec<u64> = alloc::vec![2000];
        let off = derive_slot_offset_with_probe(&key, 2, cap, &claimed, 0, 8).unwrap();
        assert!(off.abs_diff(2000) >= SLOT_STRIDE);
    }
}
