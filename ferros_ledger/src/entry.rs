//! Ledger entry — a complete BLAKE3-chained record.
//!
//! Every entry has three sections:
//!   1. Identity: category + writer_cap_hash + writer_sig
//!   2. Order: global_seq + cat_seq + eagle_time + prev_hash
//!   3. Payload: level + structured event
//!
//! The entry is hashed with BLAKE3 to produce its provenance hash, which the next entry references as prev_hash.

use crate::category::Category;
use crate::event::Event;
use crate::ewe;

/// Maximum serialized entry size. Entries that exceed this are rejected. 512 bytes is generous for current event types.
pub const MAX_ENTRY_SIZE: usize = 512;

/// A 32-byte BLAKE3 hash.
pub type Hash = [u8; 32];

/// The zero hash — genesis prev_hash is BLAKE3([0u8; 32]).
pub const ZERO_HASH: Hash = [0u8; 32];

/// Compute the genesis prev_hash: BLAKE3 of 32 zero bytes.
pub fn genesis_prev_hash() -> Hash {
    let h = blake3::hash(&ZERO_HASH);
    *h.as_bytes()
}

/// A complete ledger entry, serialized into a fixed buffer.
///
/// The entry bytes are the canonical representation — hashing these bytes produces the entry's provenance hash.
pub struct Entry {
    /// Serialized entry data.
    pub data: [u8; MAX_ENTRY_SIZE],
    /// Number of valid bytes in `data`.
    pub len: usize,
    /// BLAKE3 provenance hash of `data[..len]`.
    pub hash: Hash,
}

/// Build a ledger entry.
///
/// `cap_hash` and `sig` are placeholder fields (populated but not validated until the capability system is online).
pub fn build_entry(
    category: Category,
    global_seq: u64,
    cat_seq: u64,
    prev_hash: &Hash,
    event: &Event,
    cap_hash: &Hash,
    sig: &Hash,
) -> Option<Entry> {
    let mut buf = [0u8; MAX_ENTRY_SIZE];
    let mut pos = 0;

    // ---- Schema identifier ---- 'l' tag + length + "ferros.ledger"
    let schema = b"ferros.ledger";
    if pos + 2 + schema.len() > MAX_ENTRY_SIZE {
        return None;
    }
    buf[pos] = b'l';
    pos += 1;
    buf[pos] = schema.len() as u8;
    pos += 1;
    buf[pos..pos + schema.len()].copy_from_slice(schema);
    pos += schema.len();

    // ---- Identity section ---- Category path
    let cat_bytes = category.as_bytes();
    if pos + 2 + cat_bytes.len() > MAX_ENTRY_SIZE {
        return None;
    }
    buf[pos] = b'c';
    pos += 1; // 'c' = category tag
    buf[pos] = cat_bytes.len() as u8;
    pos += 1;
    buf[pos..pos + cat_bytes.len()].copy_from_slice(cat_bytes);
    pos += cat_bytes.len();

    // Writer cap hash (32 bytes)
    if pos + 1 + 32 > MAX_ENTRY_SIZE {
        return None;
    }
    buf[pos] = b'k';
    pos += 1; // 'k' = cap hash tag
    buf[pos..pos + 32].copy_from_slice(cap_hash);
    pos += 32;

    // Writer signature (32 bytes)
    if pos + 1 + 32 > MAX_ENTRY_SIZE {
        return None;
    }
    buf[pos] = b'g';
    pos += 1; // 'g' = signature tag
    buf[pos..pos + 32].copy_from_slice(sig);
    pos += 32;

    // ---- Order section ---- Global sequence (EWE)
    if pos + 10 > MAX_ENTRY_SIZE {
        return None;
    }
    pos += ewe::encode_u64(&mut buf[pos..], global_seq);

    // Category sequence (EWE)
    if pos + 10 > MAX_ENTRY_SIZE {
        return None;
    }
    pos += ewe::encode_u64(&mut buf[pos..], cat_seq);

    // Eagle time — TODO: placeholder 0 until physics-bounded clock
    if pos + 10 > MAX_ENTRY_SIZE {
        return None;
    }
    pos += ewe::encode_u64(&mut buf[pos..], 0);

    // Prev hash (32 bytes)
    if pos + 1 + 32 > MAX_ENTRY_SIZE {
        return None;
    }
    buf[pos] = b'p';
    pos += 1; // 'p' = prev hash tag
    buf[pos..pos + 32].copy_from_slice(prev_hash);
    pos += 32;

    // ---- Payload section ---- Event payload
    if pos + 1 > MAX_ENTRY_SIZE {
        return None;
    }
    buf[pos] = b'e';
    pos += 1; // 'e' = event tag
    let event_len = event.encode(&mut buf[pos..]);
    pos += event_len;

    // ---- Compute provenance hash ----
    let h = blake3::hash(&buf[..pos]);

    Some(Entry {
        data: buf,
        len: pos,
        hash: *h.as_bytes(),
    })
}
