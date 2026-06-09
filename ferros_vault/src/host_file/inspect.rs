//! Vault inspector — produces a human-readable dump of the on-disk vault format.
//!
//! Designed to be called from `vsfinfo` after it detects a VSF file containing a `vault` section. Returns a multi-line `String` with the same general feel as `vsfinfo`'s own output (just plaintext; no colour codes — the host can colourise if it wants).
//!
//! ## What the inspector can show, by access level
//!
//! - **0 keys** — slot layout, anchor field bytes (plaintext), object envelopes (hash + type + content_len), root_commit dict (it's stored as a plain vault object, no encryption at the vault layer). Photon-layer per-key encryption on object **content** stays opaque.
//! - **1 hex key (32-byte anchor_key)** — same as 0-key, plus HMAC verification on every decoded anchor (✓ / ✗).
//! - **2 hex keys (identity_seed + device_secret)** — derives anchor_key both orderings (`derive(a,b)` vs `derive(b,a)`), uses whichever HMAC-validates against slot 0. Object content remains opaque at the vault layer; photon's per-key KDF for decrypting content is photon's domain, not ferros_vault's — a higher-level inspector in photon's repo can wrap this output.
//!
//! ## Coupling note
//!
//! This module knows about photon only indirectly via `derive_anchor_key`, which is `blake3::derive_key("photon.vault.anchor.v0", identity_seed || device_secret)` — defined in `anchor_key_store.rs` and named photon-specific because the host-file backend was built for photon. If you fork that backend for a different host, swap the derivation accordingly.
//!
//! Output is intentionally line-oriented and grep-friendly. Don't go overboard with ASCII art — vsfinfo already does enough of that; the vault inspector's job is to expose the structured fields clearly.
//!
//! Tested by being run against real photon vaults; unit tests in this crate target the format encoders/decoders directly.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::anchor::AnchorKey;
use super::anchor_key_store::derive_anchor_key;
use super::root_commit::RootCommit;
use super::store::{object_region_start, OBJ_HEADER_BYTES, OBJ_MAGIC, OBJ_VERSION};
use super::vault_anchor::{
    self, VaultAnchor, SLOT_MAGIC, SLOT_STRIDE, SLOT_VERSION,
};
use super::vsf_wrapper;

/// Errors that prevent inspection. Anything recoverable (e.g. a single slot fails to decode) is reported in the output rather than returned as an error.
#[derive(Debug)]
pub enum InspectError {
    /// The VSF wrapper couldn't be decoded — file is not a vault (or is truncated/corrupted).
    WrapperDecode(String),
    /// A supplied key wasn't valid hex OR valid voca / didn't decode to 32 bytes.
    BadKey(String),
}

impl core::fmt::Display for InspectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InspectError::WrapperDecode(s) => write!(f, "wrapper decode: {}", s),
            InspectError::BadKey(s) => write!(f, "bad key: {}", s),
        }
    }
}

/// Inspect a vault file. `file_bytes` is the entire on-disk file (VSF wrapper + payload). `keys` is 0..=2 key strings; each is auto-detected as hex (64 chars in `[0-9a-fA-F]`) or voca (PascalCase word concatenation). The two encodings can't collide — hex digits don't form valid English words and voca's mixed-case never produces a 64-char hex string.
pub fn inspect_vault(file_bytes: &[u8], keys: &[&str]) -> Result<String, InspectError> {
    let payload = vsf_wrapper::decode(file_bytes)
        .map_err(|e| InspectError::WrapperDecode(format!("{:?}", e)))?;

    let parsed_keys = parse_keys(keys)?;
    let anchor_key = resolve_anchor_key(&payload, &parsed_keys);

    let mut out = String::new();
    let _ = writeln!(out, "═══ ferros_vault inspect ═══");
    let _ = writeln!(out, "payload bytes:    {}", payload.len());
    let _ = writeln!(
        out,
        "keys supplied:    {} ({})",
        keys.len(),
        key_supply_summary(&parsed_keys, &anchor_key)
    );

    let slot0_bytes = slot_bytes(&payload, 0);
    let slot0 = decode_anchor_maybe(slot0_bytes, anchor_key.as_ref());

    out.push_str("\n── Slot 0 (privileged bootstrap, offset 0) ──\n");
    render_slot(&mut out, slot0_bytes, &slot0, anchor_key.is_some());

    let canonical_anchor = slot0.as_ref().ok().cloned();

    // Other slots: with a key we can derive their offsets; without, we linear-scan for VLT0 magic at SLOT_STRIDE alignment past slot 0.
    if let (Some(key), Some(anchor)) = (anchor_key.as_ref(), canonical_anchor.as_ref()) {
        for n in 1..anchor.ring_size {
            let off = vault_anchor::derive_slot_offset(key, n, anchor.payload_capacity);
            let bytes = slot_bytes(&payload, off);
            let decoded = decode_anchor_maybe(bytes, Some(key));
            let _ = writeln!(
                out,
                "\n── Slot {} (derived offset {}) ──",
                n, off
            );
            render_slot(&mut out, bytes, &decoded, true);
        }
    } else {
        // Key-less scan: walk SLOT_STRIDE-aligned positions past slot 0 looking for `VLT0` magic. Catches the typical case where slots haven't moved since allocation.
        let mut found = 0u32;
        let mut off = SLOT_STRIDE;
        while (off as usize) + (SLOT_STRIDE as usize) <= payload.len() {
            let bytes = slot_bytes(&payload, off);
            if &bytes[0..4] == SLOT_MAGIC.as_ref() {
                let decoded = decode_anchor_maybe(bytes, None);
                let _ = writeln!(
                    out,
                    "\n── Slot ?? (magic at offset {}, no anchor_key) ──",
                    off
                );
                render_slot(&mut out, bytes, &decoded, false);
                found += 1;
                if found >= 16 {
                    let _ = writeln!(out, "  (further slots may exist; stopping at 16)");
                    break;
                }
            }
            off += SLOT_STRIDE;
        }
        if found == 0 {
            out.push_str("\n  (no additional slot magic found by linear scan)\n");
        }
    }

    // Object region — bounded by canonical anchor's object_tail when available; otherwise scan to end of payload.
    out.push_str("\n── Object region ──\n");
    let obj_start = canonical_anchor
        .as_ref()
        .map(|a| object_region_start(a.ring_size))
        .unwrap_or(2 * SLOT_STRIDE);
    let obj_end = canonical_anchor
        .as_ref()
        .map(|a| a.object_tail)
        .unwrap_or(payload.len() as u64);
    let _ = writeln!(out, "scan range:       [{}, {})", obj_start, obj_end);

    let objects = scan_objects(&payload, obj_start, obj_end);
    let _ = writeln!(out, "objects found:    {}\n", objects.len());

    // Locate the root_commit object so we can decode + display its dict (it's plaintext at the vault layer).
    let mut root_commit_dict: Option<RootCommit> = None;
    let root_hash_opt = canonical_anchor.as_ref().map(|a| a.root_commit.0);

    for (idx, entry) in objects.iter().enumerate() {
        let role = match root_hash_opt {
            Some(rh) if rh == entry.hash => " ← root_commit",
            _ => "",
        };
        let _ = writeln!(
            out,
            "[{:>3}] off={:<6} hash={}  type={}  gen={}  content_len={}{}",
            idx,
            entry.offset,
            hex32(&entry.hash),
            vsf_type_label(entry.vsf_type),
            entry.generation,
            entry.content_len,
            role
        );
        if matches!(root_hash_opt, Some(rh) if rh == entry.hash) {
            if let Ok(rc) = RootCommit::decode(&payload[entry.content_start..entry.content_end]) {
                root_commit_dict = Some(rc);
            }
        }
    }

    if let Some(rc) = root_commit_dict {
        out.push_str("\n── root_commit dict (logical_key → object_hash) ──\n");
        if rc.is_empty() {
            out.push_str("  (empty)\n");
        } else {
            for (k, h) in rc.iter() {
                let _ = writeln!(out, "  {} → {}", k, hex32(&h.0));
            }
        }
    }

    Ok(out)
}

// ============================================================================
// Internals
// ============================================================================

fn parse_keys(keys: &[&str]) -> Result<Vec<[u8; 32]>, InspectError> {
    keys.iter().map(|s| parse_one_key(s)).collect()
}

/// Try a string as hex first (64 chars `[0-9a-fA-F]`); fall back to voca FULL decode. Voca decode produces a `BigUint` which is left-padded to 32 bytes — leading zeros in the key value are recovered because vault keys are fixed-size by construction.
fn parse_one_key(s: &str) -> Result<[u8; 32], InspectError> {
    if is_hex_64(s) {
        let bytes = decode_hex(s).map_err(InspectError::BadKey)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        return Ok(out);
    }
    // Voca path: decode the PascalCase word concatenation. Any unknown token rejects loudly — fuzzy decode would mask a typo'd key, which silently maps to the wrong vault.
    let big = voca::decode(s)
        .map_err(|e| InspectError::BadKey(format!("not hex (64 chars) and voca decode failed: {:?}", e)))?;
    let bytes = big.to_bytes_be();
    if bytes.len() > 32 {
        return Err(InspectError::BadKey(format!(
            "voca decoded value is {} bytes, larger than the 32-byte key window",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(out)
}

fn is_hex_64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err(format!("hex length {} is odd", s.len()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| format!("at byte {}: {}", i / 2, e))
        })
        .collect()
}

/// Pick the anchor_key that HMAC-validates slot 0. With 1 key, treat it directly as anchor_key. With 2 keys, try both `derive_anchor_key` orderings. Returns `None` if no candidate validates (slot 0 still decodes structurally, just without integrity confirmation).
fn resolve_anchor_key(payload: &[u8], keys: &[[u8; 32]]) -> Option<AnchorKey> {
    let slot0 = slot_bytes(payload, 0);
    let candidates: Vec<AnchorKey> = match keys.len() {
        0 => Vec::new(),
        1 => alloc::vec![AnchorKey(keys[0])],
        _ => alloc::vec![
            AnchorKey(keys[0]),
            AnchorKey(keys[1]),
            derive_anchor_key(&keys[0], &keys[1]),
            derive_anchor_key(&keys[1], &keys[0]),
        ],
    };
    for k in candidates {
        if vault_anchor::decode(slot0, &k).is_ok() {
            return Some(k);
        }
    }
    None
}

fn key_supply_summary(keys: &[[u8; 32]], resolved: &Option<AnchorKey>) -> &'static str {
    match (keys.len(), resolved.is_some()) {
        (0, _) => "layout-only mode",
        (_, true) => "anchor_key resolved ✓",
        (_, false) => "supplied keys did NOT validate slot 0",
    }
}

fn slot_bytes(payload: &[u8], offset: u64) -> &[u8] {
    let start = offset as usize;
    let end = start.saturating_add(SLOT_STRIDE as usize).min(payload.len());
    &payload[start..end]
}

fn decode_anchor_maybe(bytes: &[u8], key: Option<&AnchorKey>) -> Result<VaultAnchor, String> {
    if bytes.len() < 97 {
        return Err(format!("slot too short ({} bytes)", bytes.len()));
    }
    if bytes[0..4] != SLOT_MAGIC {
        return Err(format!("bad magic {:?}", &bytes[0..4.min(bytes.len())]));
    }
    if bytes[4] != SLOT_VERSION {
        return Err(format!("unsupported version {}", bytes[4]));
    }
    // Decode fields by hand so we can show them even when HMAC fails (vault_anchor::decode refuses to return on HMAC mismatch).
    let mut p = 5usize;
    let anchor_seq = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
    p += 8;
    let ring_size = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
    p += 4;
    let payload_capacity = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
    p += 8;
    let object_tail = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
    p += 8;
    let mut root = [0u8; 32];
    root.copy_from_slice(&bytes[p..p + 32]);
    p += 32;
    let mut hmac = [0u8; 32];
    hmac.copy_from_slice(&bytes[p..p + 32]);

    let anchor = VaultAnchor {
        anchor_seq,
        ring_size,
        payload_capacity,
        object_tail,
        root_commit: crate::hash::ObjectHash(root),
        hmac,
    };
    // If a key was supplied, recompute HMAC and stash a verification note in a side channel — we return the anchor either way.
    if let Some(k) = key {
        let expected = vault_anchor::compute_hmac(&anchor, k);
        if expected != anchor.hmac {
            return Err(format!(
                "HMAC mismatch (key supplied does not authenticate this slot)"
            ));
        }
    }
    Ok(anchor)
}

fn render_slot(
    out: &mut String,
    bytes: &[u8],
    decoded: &Result<VaultAnchor, String>,
    hmac_verified: bool,
) {
    if bytes.is_empty() {
        out.push_str("  (out of bounds)\n");
        return;
    }
    match decoded {
        Ok(a) => {
            let _ = writeln!(out, "  magic:            VLT0  version: {}", SLOT_VERSION);
            let _ = writeln!(out, "  anchor_seq:       {}", a.anchor_seq);
            let _ = writeln!(out, "  ring_size:        {}", a.ring_size);
            let _ = writeln!(out, "  payload_capacity: {}", a.payload_capacity);
            let _ = writeln!(out, "  object_tail:      {}", a.object_tail);
            let _ = writeln!(out, "  root_commit:      {}", hex32(&a.root_commit.0));
            let verify_tag = if hmac_verified { "verified ✓" } else { "not checked — no anchor_key" };
            let _ = writeln!(out, "  hmac:             {} ({})", hex32(&a.hmac), verify_tag);
        }
        Err(e) => {
            let _ = writeln!(out, "  decode: {}", e);
            if bytes.len() >= 5 && bytes[0..4] == SLOT_MAGIC {
                // Magic OK — best-effort dump of the plaintext fields anyway.
                let mut p = 5usize;
                let take_u64 = |p: usize| u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
                let take_u32 = |p: usize| u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
                let _ = writeln!(out, "  anchor_seq:       {} (unverified)", take_u64(p));
                p += 8;
                let _ = writeln!(out, "  ring_size:        {} (unverified)", take_u32(p));
                p += 4;
                let _ = writeln!(out, "  payload_capacity: {} (unverified)", take_u64(p));
                p += 8;
                let _ = writeln!(out, "  object_tail:      {} (unverified)", take_u64(p));
                p += 8;
                let mut root = [0u8; 32];
                root.copy_from_slice(&bytes[p..p + 32]);
                let _ = writeln!(out, "  root_commit:      {} (unverified)", hex32(&root));
            }
        }
    }
}

struct ObjectEntry {
    offset: u64,
    hash: [u8; 32],
    vsf_type: u8,
    generation: u64,
    content_len: u64,
    content_start: usize,
    content_end: usize,
}

fn scan_objects(payload: &[u8], start: u64, end: u64) -> Vec<ObjectEntry> {
    let mut out = Vec::new();
    let mut p = start as usize;
    let end = end.min(payload.len() as u64) as usize;
    while p + OBJ_HEADER_BYTES <= end {
        if payload[p..p + 4] != OBJ_MAGIC || payload[p + 4] != OBJ_VERSION {
            // Not an object envelope at this position — bail. The object region should be contiguous in canonical state.
            break;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&payload[p + 5..p + 37]);
        let content_len = u32::from_le_bytes(payload[p + 37..p + 41].try_into().unwrap()) as u64;
        let vsf_type = payload[p + 41];
        let generation = u64::from_le_bytes(payload[p + 42..p + 50].try_into().unwrap());
        let content_start = p + OBJ_HEADER_BYTES;
        let content_end = content_start.saturating_add(content_len as usize).min(end);
        out.push(ObjectEntry {
            offset: p as u64,
            hash,
            vsf_type,
            generation,
            content_len,
            content_start,
            content_end,
        });
        p = content_end;
    }
    out
}

/// Translate the on-disk `vsf_type` discriminant back to a human-readable name. Mirrors `crate::object::VsfType`; an unrecognised byte renders as `?(0xNN)` rather than panicking — vault inspection should never crash on a corrupted byte.
fn vsf_type_label(byte: u8) -> String {
    match byte {
        0x01 => "Blob".into(),
        0x02 => "Record".into(),
        0x03 => "Sequence".into(),
        0x10 => "Capability".into(),
        0x20 => "MeshCommit".into(),
        0x30 => "FailureState".into(),
        0x40 => "BootAnchor".into(),
        b => format!("?(0x{:02X})", b),
    }
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{:02X}", b);
    }
    s
}
