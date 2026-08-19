//! Outer VSF envelope for a single-file vault.
//!
//! Wraps the opaque vault payload bytes (anchors + encrypted objects) in a minimal VSF file so that `file ~/.config/photon/photon.vsf` returns "VSF data" rather than "data" and so an inspector tool can identify it as a versioned VSF file. The wrapper carries:
//!
//! - VSF magic `RÅ<`
//! - Version + backward_compat (semver-style)
//! - **Provenance hash** of the contents (BLAKE3 over the whole file) — defense-in-depth integrity check on top of the per-anchor HMAC inside the vault. Recomputed and rewritten on every flush.
//! - One section named `vault` with a single field `v` carrying the encrypted payload as a `VsfType::v(b'e', ...)` (encrypted-data type marker).
//!
//! NOT carried (intentional opsec):
//! - No `creation_time` (would leak when the vault was first formatted or last touched)
//! - No rolling hash (no need to track mutable state in the outer wrapper; the anchor ring handles that)
//! - No signature (single-user single-device; the anchor HMAC suffices)
//!
//! Implication for `vsfinfo` / other VSF inspectors: a vault file will look unusual because creation_time and rolling_hash are absent. Inspectors that hard-require these fields will need an exception for vault-type files. Tracked as an open note in the plan doc.

use alloc::vec::Vec;

use vsf::{VsfBuilder, VsfHeader, VsfSection, VsfType};

/// Errors from wrapper encode/decode.
#[derive(Debug)]
pub enum WrapperError {
    /// VsfBuilder.build() failed (schema invalid, etc.). Carries the stringified reason.
    BuildFailed(alloc::string::String),
    /// VsfHeader.decode() failed (not a VSF file, truncated, version mismatch). Carries reason.
    HeaderDecodeFailed(alloc::string::String),
    /// The decoded header doesn't contain a `vault` section.
    VaultSectionMissing,
    /// The `vault` section is present but doesn't contain a `v` field with the expected encrypted-data type.
    VaultPayloadMissing,
    /// Section parse failed.
    SectionParseFailed(alloc::string::String),
}

/// Wire-format constants.
pub const VAULT_SECTION_NAME: &str = "vault";
pub const VAULT_PAYLOAD_FIELD: &str = "v";

/// Encode the outer VSF envelope around `payload`. Returns the complete VSF file bytes ready to be written to disk. The provenance hash is computed automatically by [`VsfBuilder::build`].
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, WrapperError> {
    // The payload bytes are stored as `VsfType::v(b'e', ...)` — VSF's "encrypted variable-length data" marker. The actual encryption is handled inside the vault (per-key ChaCha20-Poly1305 of each object); this marker just tells any future inspector "the contents of this field are not plaintext, don't try to render them."
    let payload_value = VsfType::v(b'e', payload.to_vec());

    VsfBuilder::new()
        .provenance_only() // disables rolling hash, includes provenance hash, no signature
        .add_section(
            VAULT_SECTION_NAME,
            alloc::vec![(VAULT_PAYLOAD_FIELD.into(), payload_value)],
        )
        .build()
        .map_err(WrapperError::BuildFailed)
}

/// Read the total wrapped-file length from a header prefix, without decoding the payload.
///
/// The VSF header encodes `file_length` (total bytes) right after the magic — for exactly this purpose (TCP streaming / bounded reads).
/// A caller that only has the device can read a small prefix (a few hundred bytes covers the single-section vault header), learn the true size, then read precisely that many bytes — instead of the whole device.
/// The prefix must contain the full header; a few KiB is ample for a one-section vault.
pub fn wrapped_len(prefix: &[u8]) -> Result<usize, WrapperError> {
    let (header, _header_len) =
        VsfHeader::decode(prefix).map_err(WrapperError::HeaderDecodeFailed)?;
    Ok(header.file_length)
}

/// Decode the outer VSF envelope from `bytes`, returning the vault payload bytes (the inner opaque data). Verifies the file parses as VSF and contains the expected `vault` section + `v` field.
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, WrapperError> {
    // Step 1: parse the header.
    let (header, header_len) =
        VsfHeader::decode(bytes).map_err(WrapperError::HeaderDecodeFailed)?;

    // Step 2: find the `vault` section's offset from the header's field table.
    let vault_field = header
        .fields
        .iter()
        .find(|f| f.name == VAULT_SECTION_NAME)
        .ok_or(WrapperError::VaultSectionMissing)?;

    // Step 3: parse the section at that offset. The offset is relative to file start.
    let mut ptr = vault_field.offset_bytes;
    if ptr < header_len {
        return Err(WrapperError::SectionParseFailed(
            alloc::format!(
                "vault section offset {} is inside the header (len {})",
                ptr, header_len
            ),
        ));
    }
    let section = VsfSection::parse(bytes, &mut ptr).map_err(|e| {
        WrapperError::SectionParseFailed(alloc::format!("{:?}", e))
    })?;

    // Step 4: pull the `v` field's bytes back out.
    let field = section
        .get_field(VAULT_PAYLOAD_FIELD)
        .ok_or(WrapperError::VaultPayloadMissing)?;
    let value = field
        .values
        .first()
        .ok_or(WrapperError::VaultPayloadMissing)?;
    match value {
        VsfType::v(_marker, payload) => Ok(payload.clone()),
        _ => Err(WrapperError::VaultPayloadMissing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trip_empty() {
        let bytes = encode(&[]).unwrap();
        let payload = decode(&bytes).unwrap();
        assert!(payload.is_empty());
    }

    #[test]
    fn round_trip_short_payload() {
        let original: Vec<u8> = (0..=255u8).collect();
        let bytes = encode(&original).unwrap();
        let recovered = decode(&bytes).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn round_trip_64kb() {
        // Realistic vault size: 64 KiB of pseudorandom bytes (xorshift over an index).
        let mut original = vec![0u8; 64 * 1024];
        let mut x: u32 = 0xDEAD_BEEF;
        for byte in original.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *byte = x as u8;
        }
        let bytes = encode(&original).unwrap();
        let recovered = decode(&bytes).unwrap();
        assert_eq!(original.len(), recovered.len());
        assert_eq!(original, recovered);
    }

    #[test]
    fn decode_rejects_non_vsf() {
        let res = decode(b"not a vsf file at all");
        assert!(matches!(res, Err(WrapperError::HeaderDecodeFailed(_))));
    }

    #[test]
    fn encoded_file_starts_with_vsf_magic() {
        let bytes = encode(b"payload").unwrap();
        // R Å < — VSF magic is `R` then U+00C5 (Å, 0xC3 0x85 in UTF-8) then `<`.
        assert_eq!(bytes[0], b'R');
        assert_eq!(bytes[1], 0xC3);
        assert_eq!(bytes[2], 0x85);
        assert_eq!(bytes[3], b'<');
    }
}
