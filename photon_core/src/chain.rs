//! The braid — rolling-chain encryption for photons (post-CLUTCH), ported byte-for-byte from photon/src/crypto/chain.rs so a ferros-sent photon decrypts in the stock app. Eagle time is passed as its raw i64 oscillations (no std clock here).
//!
//! A 512-link chain (16 KiB): links [0..256) history, [256..512) active, the current key at [511]. Per message: a salt from the previous plaintext + the last 12 links, a 30 KiB scratch pad, XChaCha20 under a key derived from the current link, then an XOR with the scratch. `advance` shifts the chain left and derives a fresh link (forward secrecy). Internal correctness is covered by an encrypt→decrypt round trip; byte-compatibility with the stock app is verified on device.

use alloc::vec;
use alloc::vec::Vec;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::XChaCha20;
use ihi::{smear_hash, spaghettify};

pub const CHAIN_LINKS: usize = 512;
pub const HISTORY_LINKS: usize = 256;
pub const ACTIVE_LINKS: usize = 256;
pub const LINK_SIZE: usize = 32;
pub const CURRENT_KEY_INDEX: usize = 511;
pub const L1_SIZE: usize = 30_720;
pub const L1_ROUNDS: usize = 3;

const DOMAIN_ADVANCE: &[u8] = b"PHOTON_ADVANCE_v\x01";
const DOMAIN_SALT: &[u8] = b"PHOTON_SALT_v\x01";
const DOMAIN_STRAND: &[u8] = b"PHOTON_STRAND_v\x01";
pub const BRAID_V2: u8 = 2;

const SALT_LINK_RANGE: core::ops::Range<usize> = 500..512;

/// A 512-link chain. Build from the 8 KiB CLUTCH-derived active portion, or from a full 16 KiB snapshot.
#[derive(Clone)]
pub struct Chain {
    links: [[u8; 32]; CHAIN_LINKS],
}

impl Chain {
    /// From the 8 KiB active portion (links [256..512)); history stays zero. None unless len == 8192.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != ACTIVE_LINKS * LINK_SIZE {
            return None;
        }
        let mut links = [[0u8; 32]; CHAIN_LINKS];
        for (i, chunk) in bytes.chunks_exact(LINK_SIZE).enumerate() {
            links[HISTORY_LINKS + i].copy_from_slice(chunk);
        }
        Some(Self { links })
    }

    /// From a full 16 KiB snapshot. None unless len == 16384.
    pub fn from_full_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != CHAIN_LINKS * LINK_SIZE {
            return None;
        }
        let mut links = [[0u8; 32]; CHAIN_LINKS];
        for (i, chunk) in bytes.chunks_exact(LINK_SIZE).enumerate() {
            links[i].copy_from_slice(chunk);
        }
        Some(Self { links })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.links.as_flattened().to_vec()
    }

    pub fn current_key(&self) -> &[u8; 32] {
        &self.links[CURRENT_KEY_INDEX]
    }

    pub fn link(&self, index: usize) -> Option<&[u8; 32]> {
        self.links.get(index)
    }

    /// Shift the chain left (oldest link drops) and derive a fresh current link from the eagle time, the peer strands and our plaintext (forward secrecy; the field order is the contract — see `derive_fresh_link`).
    pub fn advance(&mut self, eagle_time: i64, our_plaintext: &[u8], their_plaintexts: &[&[u8]]) {
        self.links.copy_within(1..CHAIN_LINKS, 0);
        let fresh = derive_fresh_link(eagle_time, our_plaintext, their_plaintexts, &self.links);
        self.links[CURRENT_KEY_INDEX] = fresh;
    }
}

/// Salt for a message: BLAKE3(domain ‖ previous plaintext ‖ last 12 links), spaghettified. Empty prev for the first message.
pub fn derive_salt(prev_plaintext: &[u8], chain: &Chain) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(DOMAIN_SALT);
    h.update(prev_plaintext);
    h.update(chain.links[SALT_LINK_RANGE].as_flattened());
    spaghettify(h.finalize().as_bytes())
}

/// A woven strand's braid-v2 value: its eagle time and identity bytes spaghettified (secret; never on the wire).
pub fn strand_value(eagle_time: i64, ident: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(DOMAIN_STRAND.len() + 8 + 4 + ident.len());
    input.extend_from_slice(DOMAIN_STRAND);
    input.extend_from_slice(&eagle_time.to_le_bytes());
    input.extend_from_slice(&(ident.len() as u32).to_le_bytes());
    input.extend_from_slice(ident);
    spaghettify(&input)
}

/// The 30 KiB scratch pad from a chain link and the salt: sequential smear_hash, then `L1_ROUNDS` data-dependent mixing rounds. `key_index` selects the link (the current key for sending).
pub fn generate_scratch_at(chain: &Chain, salt: &[u8; 32], key_index: usize) -> Vec<u8> {
    let mut scratch = vec![0u8; L1_SIZE];
    let mut state = [0u8; 32];
    for i in 0..32 {
        state[i] = chain.links[key_index][i] ^ salt[i];
    }
    scratch[0..32].copy_from_slice(&state);
    let mut i = 32;
    while i < L1_SIZE {
        state = smear_hash(&scratch[i - 32..i]);
        scratch[i..i + 32].copy_from_slice(&state);
        i += 32;
    }
    for _ in 0..L1_ROUNDS {
        let mut i = 32;
        while i < L1_SIZE {
            let read_idx = (u32::from_le_bytes(scratch[i..i + 4].try_into().unwrap()) as usize) % (i / 32) * 32;
            let mut mix = [0u8; 64];
            mix[0..32].copy_from_slice(&scratch[i - 32..i]);
            mix[32..64].copy_from_slice(&scratch[read_idx..read_idx + 32]);
            let mixed = smear_hash(&mix);
            scratch[i..i + 32].copy_from_slice(&mixed);
            i += 32;
        }
    }
    scratch
}

/// Scratch for sending (the current key).
pub fn generate_scratch(chain: &Chain, salt: &[u8; 32]) -> Vec<u8> {
    generate_scratch_at(chain, salt, CURRENT_KEY_INDEX)
}

/// The 24-byte XChaCha20 nonce: first 24 bytes of BLAKE3(eagle_time LE).
pub fn derive_xnonce(eagle_time: i64) -> [u8; 24] {
    let hash = blake3::hash(&eagle_time.to_le_bytes());
    let mut n = [0u8; 24];
    n.copy_from_slice(&hash.as_bytes()[..24]);
    n
}

/// Encrypt: XChaCha20 under derive_key("photon.chain.xchacha.v1", current key), then XOR the scratch (cycling). The caller built the plaintext VSF.
pub fn encrypt_layers(plaintext: &[u8], chain: &Chain, scratch: &[u8], eagle_time: i64) -> Vec<u8> {
    let key = blake3::derive_key("photon.chain.xchacha.v1", chain.current_key());
    let nonce = derive_xnonce(eagle_time);
    let mut cipher = XChaCha20::new(&key.into(), &nonce.into());
    let mut out = plaintext.to_vec();
    cipher.apply_keystream(&mut out);
    for (i, b) in out.iter_mut().enumerate() {
        *b ^= scratch[i % scratch.len()];
    }
    out
}

/// Decrypt: XOR the scratch, then XChaCha20 under the key derived from link `key_index`.
pub fn decrypt_layers(ciphertext: &[u8], chain: &Chain, key_index: usize, scratch: &[u8], eagle_time: i64) -> Vec<u8> {
    let mut mid = ciphertext.to_vec();
    for (i, b) in mid.iter_mut().enumerate() {
        *b ^= scratch[i % scratch.len()];
    }
    let key = blake3::derive_key("photon.chain.xchacha.v1", &chain.links[key_index]);
    let nonce = derive_xnonce(eagle_time);
    let mut cipher = XChaCha20::new(&key.into(), &nonce.into());
    cipher.apply_keystream(&mut mid);
    mid
}

/// A fresh chain link (advance): spaghettify over domain ‖ eagle_time(8) ‖ strand_count(4) ‖ [len(4)+strand]* ‖ chain[256..511] ‖ our_len(4)+our. The peer strands lead; the framing is injective. This layout is the chain-compatibility contract — both peers must match it exactly.
fn derive_fresh_link(eagle_time: i64, our_plaintext: &[u8], their_plaintexts: &[&[u8]], chain: &[[u8; 32]; CHAIN_LINKS]) -> [u8; 32] {
    let chain_portion = chain[HISTORY_LINKS..CURRENT_KEY_INDEX].as_flattened();
    let strands_len: usize = their_plaintexts.iter().map(|p| 4 + p.len()).sum();
    let mut input = Vec::with_capacity(DOMAIN_ADVANCE.len() + 8 + 4 + strands_len + chain_portion.len() + 4 + our_plaintext.len());
    input.extend_from_slice(DOMAIN_ADVANCE);
    input.extend_from_slice(&eagle_time.to_le_bytes());
    input.extend_from_slice(&(their_plaintexts.len() as u32).to_le_bytes());
    for strand in their_plaintexts {
        input.extend_from_slice(&(strand.len() as u32).to_le_bytes());
        input.extend_from_slice(strand);
    }
    input.extend_from_slice(chain_portion);
    input.extend_from_slice(&(our_plaintext.len() as u32).to_le_bytes());
    input.extend_from_slice(our_plaintext);
    spaghettify(&input)
}
