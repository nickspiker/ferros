//! Vault crypto — XChaCha20-Poly1305 AEAD and keyed content addressing.
//!
//! This is the single crypto surface for vault object/index confidentiality, per VAULT-INDEX.md. Two primitives:
//!
//! 1. [`seal`] / [`open`] — authenticated encryption with XChaCha20-Poly1305.
//! 2. [`content_address`] — a *keyed* content address (`blake3::keyed_hash`), replacing bare `blake3(plaintext)`.
//!
//! ## Why XChaCha, not ChaCha or AES-GCM
//! XChaCha20-Poly1305 has a **192-bit (24-byte) nonce**, so a *random* nonce per message is birthday-safe essentially forever (collision negligible around 2⁹⁶ messages). ChaCha20-Poly1305 and AES-256-GCM use **96-bit (12-byte)** nonces, which are birthday-bound near 2⁴⁸ messages, and a single nonce reuse under either is catastrophic (keystream XOR leak + forgery). The vsf crate's built-in AEADs are the 12-byte-nonce kind, which is exactly why vault crypto lives here and hands vsf an already-opaque blob.
//!
//! ## Random nonces, not counters
//! The nonce is *caller-supplied* and must be freshly random each call (kernel: the SMCCC/RNDR TRNG; host: `rand`). A monotonic counter is fragile under exactly the operations a vault does — rollback, mirror, restore-from-backup — any of which can replay a counter value and reuse a nonce. 192 random bits sidestep counter-state entirely; that is the whole reason for XChaCha over ChaCha.
//!
//! ## Keyed addresses and cross-vault convergence
//! `content_address` is `blake3::keyed_hash(addr_key, plaintext)`, never bare `blake3(plaintext)`. Bare content addressing is globally deterministic: identical bytes hash to identical addresses on every device, so a disk-holder can hash a *guess* and confirm its presence (confirmation-of-content), and two seized vaults can be intersected to reveal shared content (cross-vault correlation). Keying the address with a per-vault secret defeats both. The cost is that dedup becomes **vault-scoped** — you still collapse duplicates *within your own* vault, but identical content in two different vaults lands at different addresses. For a sovereign personal vault that is the correct trade: cross-user dedup was always a privacy leak wearing an efficiency costume.
//!
//! ## Dedup and random nonces coexist
//! The address is the keyed hash of *plaintext* (stable → dedup works); the stored bytes are `seal(random nonce, plaintext)` (fresh nonce → no reuse). Each distinct plaintext is sealed once (dedup skips re-sealing an address that already exists), so every (key, nonce) pair is used exactly once. The Poly1305 tag plus the keyed address give integrity twice over.

use alloc::vec::Vec;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::anchor::AnchorKey;

/// XChaCha20-Poly1305 nonce length (bytes).
pub const NONCE_LEN: usize = 24;
/// Poly1305 authentication tag length (bytes).
pub const TAG_LEN: usize = 16;

/// KDF context for the vault payload (object/index) encryption key.
const PAYLOAD_KEY_CONTEXT: &str = "ferros.vault.payload.v0";
/// KDF context for the keyed content-addressing key.
const ADDR_KEY_CONTEXT: &str = "ferros.vault.addr.v0";

/// Crypto failures. Deliberately coarse — a vault never reveals *why* an open failed (wrong key vs tampered vs truncated look the same to an attacker).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// AEAD encryption failed (should not happen for well-formed inputs).
    Encrypt,
    /// AEAD decryption/authentication failed — wrong key, wrong nonce, or tampered ciphertext.
    Decrypt,
    /// Sealed blob is too short to contain a nonce and tag.
    Truncated,
}

/// Derive the vault payload-encryption key from the anchor key (domain-separated).
pub fn payload_key(anchor: &AnchorKey) -> [u8; 32] {
    AnchorKey::derive(PAYLOAD_KEY_CONTEXT, &anchor.0)
}

/// Derive the keyed content-addressing key from the anchor key (domain-separated).
///
/// Separate from [`payload_key`] so the value used to *locate* an object is never the value used to *decrypt* it — a compromise of one derivation context does not hand over the other.
pub fn addr_key(anchor: &AnchorKey) -> [u8; 32] {
    AnchorKey::derive(ADDR_KEY_CONTEXT, &anchor.0)
}

/// Keyed content address: `blake3::keyed_hash(addr_key, plaintext)`.
///
/// Use in place of bare `blake3(plaintext)` everywhere the store addresses content. See the module docs on convergence.
pub fn content_address(addr_key: &[u8; 32], plaintext: &[u8]) -> [u8; 32] {
    *blake3::keyed_hash(addr_key, plaintext).as_bytes()
}

/// Seal `plaintext` under `key` with the caller-supplied random `nonce`.
///
/// Output is self-framing: `nonce ‖ ciphertext ‖ tag`, so [`open`] needs only the key. The caller MUST supply a freshly random 24-byte nonce each call (see module docs); reusing a (key, nonce) pair is catastrophic.
pub fn seal(key: &[u8; 32], nonce: &[u8; NONCE_LEN], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let ct = cipher
        .encrypt(XNonce::from_slice(nonce), plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a `nonce ‖ ciphertext ‖ tag` blob produced by [`seal`].
///
/// Returns [`CryptoError::Decrypt`] on any authentication failure (wrong key, wrong nonce, tampered bytes) — indistinguishable by design.
pub fn open(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if sealed.len() < NONCE_LEN + TAG_LEN {
        return Err(CryptoError::Truncated);
    }
    let (nonce, ct) = sealed.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(XNonce::from_slice(nonce), ct)
        .map_err(|_| CryptoError::Decrypt)
}

/// A source of fresh, unpredictable 24-byte nonces for [`seal`].
///
/// The core vault is no_std and holds no RNG, so the platform injects one: the kernel draws from the SMCCC/RNDR TRNG (see `ferros_kernel::wairua`), the host from `rand`. Each call MUST return a value never returned before under the same key — a repeat is catastrophic under any AEAD.
pub trait NonceSource {
    /// Return a fresh random 24-byte nonce.
    fn next_nonce(&mut self) -> [u8; NONCE_LEN];
}

/// Host-side [`NonceSource`] backed by the operating system RNG. Available only with `rand` (the `host-file` feature); the kernel supplies its own TRNG-backed source.
#[cfg(feature = "host-file")]
pub struct RandNonce;

#[cfg(feature = "host-file")]
impl NonceSource for RandNonce {
    fn next_nonce(&mut self) -> [u8; NONCE_LEN] {
        use rand::RngCore;
        let mut n = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut n);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> [u8; 32] {
        [b; 32]
    }
    fn nonce(b: u8) -> [u8; NONCE_LEN] {
        [b; NONCE_LEN]
    }

    #[test]
    fn seal_open_round_trip() {
        let k = key(0x11);
        let n = nonce(0x22);
        let pt = b"contacts/alice: the whole point of a vault";
        let sealed = seal(&k, &n, pt).unwrap();
        // Framed as nonce ‖ ct ‖ tag.
        assert_eq!(&sealed[..NONCE_LEN], &n);
        assert_eq!(sealed.len(), NONCE_LEN + pt.len() + TAG_LEN);
        assert_eq!(open(&k, &sealed).unwrap(), pt);
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let sealed = seal(&key(1), &nonce(2), b"secret").unwrap();
        assert_eq!(open(&key(9), &sealed), Err(CryptoError::Decrypt));
    }

    #[test]
    fn tamper_is_detected() {
        let mut sealed = seal(&key(1), &nonce(2), b"secret").unwrap();
        // Flip a ciphertext byte (past the nonce) — the Poly1305 tag must reject it.
        let i = NONCE_LEN + 1;
        sealed[i] ^= 0x80;
        assert_eq!(open(&key(1), &sealed), Err(CryptoError::Decrypt));
    }

    #[test]
    fn truncated_blob_errors_not_panics() {
        assert_eq!(open(&key(1), &[0u8; 3]), Err(CryptoError::Truncated));
        assert_eq!(open(&key(1), &[]), Err(CryptoError::Truncated));
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let sealed = seal(&key(1), &nonce(2), b"").unwrap();
        assert_eq!(sealed.len(), NONCE_LEN + TAG_LEN);
        assert_eq!(open(&key(1), &sealed).unwrap(), b"");
    }

    #[test]
    fn keyed_address_is_deterministic_and_key_scoped() {
        let ka = addr_key(&AnchorKey(key(0xA1)));
        let kb = addr_key(&AnchorKey(key(0xB2)));
        let pt = b"We hold these truths to be self-evident";
        // Deterministic within a vault (dedup works).
        assert_eq!(content_address(&ka, pt), content_address(&ka, pt));
        // Different vaults place the same content at different addresses (convergence defeated).
        assert_ne!(content_address(&ka, pt), content_address(&kb, pt));
    }

    #[test]
    fn keyed_address_differs_from_bare_blake3() {
        // A disk-holder computing bare blake3(plaintext) must not match our stored address.
        let ka = addr_key(&AnchorKey(key(0x5A)));
        let pt = b"guessable content";
        let bare = *blake3::hash(pt).as_bytes();
        assert_ne!(content_address(&ka, pt), bare);
    }

    #[test]
    fn payload_and_addr_keys_are_distinct() {
        // The locate-key and the decrypt-key must never coincide.
        let anchor = AnchorKey(key(0x33));
        assert_ne!(payload_key(&anchor), addr_key(&anchor));
    }
}
