//! Root commit object — the `logical_key → content_hash` dictionary that bridges Photon's logical-key storage API onto the content-addressed vault.
//!
//! Every vault write rewrites this dict and stores it as a fresh object; the new anchor's `root_commit` field points at the new object's hash. Reads walk anchor → root commit object → dict lookup → fetch referenced object.
//!
//! Wire format (bespoke, deterministic):
//! ```text
//!   [magic: 4 bytes "RCM0"]
//!   [version: u8]
//!   [entry_count: u32 LE]
//!   for each entry (sorted by logical_key ascending — deterministic):
//!     [key_len: u16 LE]
//!     [key bytes: UTF-8]
//!     [content_hash: 32 bytes]
//! ```
//!
//! Sorted-on-encode is load-bearing: the root commit is itself content-addressed, so the same logical dict must always encode to the same bytes (hence the same hash). Sorting by key gives a canonical order independent of insertion order.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::hash::ObjectHash;

pub const RC_MAGIC: [u8; 4] = *b"RCM0";
pub const RC_VERSION: u8 = 0;

/// In-memory representation of the root commit dictionary. `BTreeMap` chosen over `HashMap` so iteration order is deterministic — sorted by logical_key — which feeds into the canonical encode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootCommit {
    entries: BTreeMap<String, ObjectHash>,
}

/// Errors from root commit encode/decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootCommitError {
    /// Decoded bytes are too short for the minimum header.
    TooShort,
    /// First 4 bytes don't match `RC_MAGIC`.
    BadMagic,
    /// Version byte is unrecognized.
    UnsupportedVersion(u8),
    /// Decode walked past the buffer end (malformed length field, truncated data).
    Truncated,
    /// Key bytes weren't valid UTF-8.
    InvalidUtf8,
}

impl RootCommit {
    /// Construct an empty root commit. The fresh-vault state starts with no entries; the first write populates one entry.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Number of entries in the dict.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the dict has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a logical key's content hash. Returns `None` if the key isn't in the dict.
    pub fn get(&self, logical_key: &str) -> Option<&ObjectHash> {
        self.entries.get(logical_key)
    }

    /// Insert or update an entry. Returns the previous hash if the key already existed.
    pub fn insert(&mut self, logical_key: String, content_hash: ObjectHash) -> Option<ObjectHash> {
        self.entries.insert(logical_key, content_hash)
    }

    /// Remove an entry. Returns the removed hash if the key existed.
    pub fn remove(&mut self, logical_key: &str) -> Option<ObjectHash> {
        self.entries.remove(logical_key)
    }

    /// Iterate entries in canonical (sorted-by-key) order.
    pub fn iter(&self) -> alloc::collections::btree_map::Iter<'_, String, ObjectHash> {
        self.entries.iter()
    }

    /// Encode to the canonical wire format. Result bytes are deterministic for a given set of entries — same dict → same bytes → same hash.
    pub fn encode(&self) -> Vec<u8> {
        // Estimate capacity: header (9) + per-entry (2 + avg_key_len + 32). Most logical_keys are 20-40 chars so we'll over-allocate slightly; harmless.
        let estimated_size = 9 + self.entries.len() * (2 + 32 + 32);
        let mut out = Vec::with_capacity(estimated_size);
        out.extend_from_slice(&RC_MAGIC);
        out.push(RC_VERSION);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (key, hash) in &self.entries {
            // Defensive: key length capped at u16::MAX. In practice Photon's logical keys are well under 256 chars; this check just rules out a misuse that would silently truncate.
            assert!(key.len() <= u16::MAX as usize, "logical_key length exceeds u16::MAX");
            out.extend_from_slice(&(key.len() as u16).to_le_bytes());
            out.extend_from_slice(key.as_bytes());
            out.extend_from_slice(&hash.0);
        }
        out
    }

    /// Decode from the canonical wire format. Returns the populated dict or a `RootCommitError`.
    pub fn decode(bytes: &[u8]) -> Result<Self, RootCommitError> {
        if bytes.len() < 9 {
            return Err(RootCommitError::TooShort);
        }
        if bytes[0..4] != RC_MAGIC {
            return Err(RootCommitError::BadMagic);
        }
        let version = bytes[4];
        if version != RC_VERSION {
            return Err(RootCommitError::UnsupportedVersion(version));
        }
        let entry_count = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
        let mut p = 9usize;
        let mut entries = BTreeMap::new();
        for _ in 0..entry_count {
            if p + 2 > bytes.len() {
                return Err(RootCommitError::Truncated);
            }
            let key_len = u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap()) as usize;
            p += 2;
            if p + key_len + 32 > bytes.len() {
                return Err(RootCommitError::Truncated);
            }
            let key = core::str::from_utf8(&bytes[p..p + key_len])
                .map_err(|_| RootCommitError::InvalidUtf8)?
                .to_string();
            p += key_len;
            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&bytes[p..p + 32]);
            p += 32;
            entries.insert(key, ObjectHash(hash_bytes));
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::string::ToString;

    #[test]
    fn empty_round_trip() {
        let rc = RootCommit::new();
        let bytes = rc.encode();
        let decoded = RootCommit::decode(&bytes).unwrap();
        assert_eq!(rc, decoded);
        assert_eq!(decoded.len(), 0);
    }

    #[test]
    fn single_entry_round_trip() {
        let mut rc = RootCommit::new();
        rc.insert("contacts/index".to_string(), ObjectHash([0x42; 32]));
        let bytes = rc.encode();
        let decoded = RootCommit::decode(&bytes).unwrap();
        assert_eq!(rc, decoded);
        assert_eq!(
            decoded.get("contacts/index"),
            Some(&ObjectHash([0x42; 32]))
        );
    }

    #[test]
    fn many_entries_round_trip() {
        let mut rc = RootCommit::new();
        for i in 0..100 {
            let key = format!("contacts/{:08x}/state", i);
            let mut h = [0u8; 32];
            h[0] = i as u8;
            rc.insert(key, ObjectHash(h));
        }
        let bytes = rc.encode();
        let decoded = RootCommit::decode(&bytes).unwrap();
        assert_eq!(rc, decoded);
        assert_eq!(decoded.len(), 100);
    }

    #[test]
    fn encoding_is_deterministic_regardless_of_insertion_order() {
        let mut rc_a = RootCommit::new();
        rc_a.insert("b_key".to_string(), ObjectHash([1; 32]));
        rc_a.insert("a_key".to_string(), ObjectHash([2; 32]));
        rc_a.insert("c_key".to_string(), ObjectHash([3; 32]));

        let mut rc_b = RootCommit::new();
        rc_b.insert("c_key".to_string(), ObjectHash([3; 32]));
        rc_b.insert("a_key".to_string(), ObjectHash([2; 32]));
        rc_b.insert("b_key".to_string(), ObjectHash([1; 32]));

        // Different insertion order, same set of entries → same encoded bytes.
        assert_eq!(rc_a.encode(), rc_b.encode());
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let bad_bytes = [0u8; 9];
        let res = RootCommit::decode(&bad_bytes);
        assert!(matches!(res, Err(RootCommitError::BadMagic)));
    }

    #[test]
    fn decode_rejects_unsupported_version() {
        let mut bytes = alloc::vec![0u8; 9];
        bytes[0..4].copy_from_slice(&RC_MAGIC);
        bytes[4] = 99;
        let res = RootCommit::decode(&bytes);
        assert!(matches!(res, Err(RootCommitError::UnsupportedVersion(99))));
    }

    #[test]
    fn decode_rejects_truncated_data() {
        let mut rc = RootCommit::new();
        rc.insert("k".to_string(), ObjectHash([1; 32]));
        let mut bytes = rc.encode();
        // Lop off the last 5 bytes (mid-hash).
        bytes.truncate(bytes.len() - 5);
        let res = RootCommit::decode(&bytes);
        assert!(matches!(res, Err(RootCommitError::Truncated)));
    }

    #[test]
    fn remove_works() {
        let mut rc = RootCommit::new();
        rc.insert("k1".to_string(), ObjectHash([1; 32]));
        rc.insert("k2".to_string(), ObjectHash([2; 32]));
        assert_eq!(rc.remove("k1"), Some(ObjectHash([1; 32])));
        assert_eq!(rc.len(), 1);
        assert_eq!(rc.get("k1"), None);
        assert_eq!(rc.get("k2"), Some(&ObjectHash([2; 32])));
    }
}
