//! Anchor key derivation for the Linux host-file vault.
//!
//! On real ferros hardware, the [`crate::anchor::AnchorKey`] lives in a separate trust domain (UFS RPMB, RISC-V CSR, eFuse, etc.) — Trust Domain A in the anchor.rs design comment. For Photon-on-Linux there's no such hardware-isolated key store; instead, the anchor key is derived deterministically from photon's two existing roots:
//!
//! ```text
//! anchor_key = BLAKE3_KDF("photon.vault.anchor.v0", identity_seed || device_secret)
//! ```
//!
//! Same auth flow that builds [`crate::Ledger`] reproduces the same `anchor_key` on every launch. No separate key file, nothing extra to persist. Lose `identity_seed` or `device_secret` and the vault is unrecoverable — already the case for the per-file FAF storage this replaces.
//!
//! Context separator `"photon.vault.anchor.v0"` is versioned so a future re-key (different domain, different inputs) gets a fresh KDF context without collision risk.

use crate::anchor::AnchorKey;

/// Context string for the anchor key KDF. Bumping the `vN` suffix forces all callers to re-derive against new bytes — used if the derivation scheme itself changes in incompatible ways.
pub const ANCHOR_KEY_CONTEXT: &str = "photon.vault.anchor.v0";

/// Derive the 32-byte anchor key from photon's two storage roots. Deterministic, reproducible across launches, no on-disk key material.
///
/// `identity_seed` is photon's `ihi::handle_to_hash(handle)` — same as what [`crate::storage::FlatStorage`] uses. `device_secret` is the Ed25519 signing key bytes derived from the machine fingerprint.
///
/// The KDF context is fixed; both inputs are concatenated then fed to `blake3::derive_key`. BLAKE3's derive-key mode is the canonical "produce N bytes of pseudorandom output from key material + context" primitive.
pub fn derive_anchor_key(identity_seed: &[u8; 32], device_secret: &[u8; 32]) -> AnchorKey {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(identity_seed);
    input[32..].copy_from_slice(device_secret);
    AnchorKey(blake3::derive_key(ANCHOR_KEY_CONTEXT, &input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_deterministic() {
        let id = [1u8; 32];
        let dev = [2u8; 32];
        let key_a = derive_anchor_key(&id, &dev);
        let key_b = derive_anchor_key(&id, &dev);
        assert_eq!(key_a.0, key_b.0);
    }

    #[test]
    fn different_inputs_yield_different_keys() {
        let id_a = [1u8; 32];
        let id_b = [3u8; 32];
        let dev = [2u8; 32];
        let key_a = derive_anchor_key(&id_a, &dev);
        let key_b = derive_anchor_key(&id_b, &dev);
        assert_ne!(key_a.0, key_b.0);
    }

    #[test]
    fn swapping_id_and_secret_yields_different_keys() {
        // Defensive sanity check: even if the two inputs happened to hold equal bytes (extreme edge case in synthetic tests), the *positions* matter — swapping them through the KDF must produce different output. Catches "we forgot to length-prefix or domain-separate the two inputs" mistakes.
        let a = [0xAAu8; 32];
        let b = [0xBBu8; 32];
        let key_a = derive_anchor_key(&a, &b);
        let key_b = derive_anchor_key(&b, &a);
        assert_ne!(key_a.0, key_b.0);
    }
}
