//! ahau's identity derivation (Claude memories/ferros_attest_os_level.md): what the attest screen computes from the handle typed on the honeycomb keyboard, with no std and without vsf's 8.9 MB text tables.
//!
//! The identity seed is ihi's `handle_to_hash`: BLAKE3 over vsf's `x` encoding of the handle, which is `x`, the character count after NFC as a vsf number, then each character's Huffman code most significant bit first, zero-padded to a byte. The keyboard can type only the characters in `codes::CODES`, every one of them its own NFC form and none combining with a neighbour, so NFC changes nothing and those codes are all the encoder needs. A character outside the table is refused, never guessed: the seed must be byte-exact or it names someone else.
#![no_std]

mod codes;
pub use codes::CODES;

/// A character the keyboard cannot type (and so this encoder has no code for).
#[derive(Debug, PartialEq, Eq)]
pub struct Untypeable(pub char);

/// Calls `out` with each byte of vsf's `x` encoding of `handle`, in order.
pub fn x_bytes(handle: &str, out: &mut impl FnMut(u8)) -> Result<(), Untypeable> {
    let mut codes = [(0u32, 0u8); 64];
    let mut n = 0usize;
    // Look every character up first: a refusal must come before any byte.
    for c in handle.chars() {
        let k = CODES.binary_search_by(|e| e.0.cmp(&c)).map_err(|_| Untypeable(c))?;
        if n == codes.len() {
            // Longer than any handle the screen takes; refused rather than cut.
            return Err(Untypeable(c));
        }
        codes[n] = (CODES[k].1, CODES[k].2);
        n += 1;
    }
    out(b'x');
    // vsf's usize number: '3' and one byte up to 255 (a handle here is at most 64 characters).
    out(b'3');
    out(n as u8);
    let (mut acc, mut bits) = (0u64, 0u32);
    for &(code, len) in &codes[..n] {
        acc = acc << len | code as u64;
        bits += len as u32;
        while bits >= 8 {
            bits -= 8;
            out((acc >> bits) as u8);
        }
    }
    if bits > 0 {
        out((acc << (8 - bits)) as u8);
    }
    Ok(())
}

/// The identity seed: ihi's `handle_to_hash` for a handle the keyboard can type.
pub fn identity_seed(handle: &str) -> Result<[u8; 32], Untypeable> {
    let mut h = blake3::Hasher::new();
    x_bytes(handle, &mut |b| {
        h.update(&[b]);
    })?;
    Ok(*h.finalize().as_bytes())
}

/// What the device keeps to recognise its identity: the seed under a key only this device has (its ira's `ahau verifier` key, kernel SYS_IRA). It says nothing about the handle off the device, and matching it means the typed handle is this device's.
pub fn verifier(device_key: &[u8; 32], seed: &[u8; 32]) -> [u8; 32] {
    *blake3::keyed_hash(device_key, seed).as_bytes()
}

/// Two 32-byte values equal, in time that does not depend on where they differ.
pub fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;
    use vsf::types::VsfType;

    fn ours(s: &str) -> Vec<u8> {
        let mut v = Vec::new();
        x_bytes(s, &mut |b| v.push(b)).unwrap();
        v
    }

    /// Every character alone, and random strings of up to 64 of them, encode exactly as vsf 0.6.42 does, and the seed is ihi's.
    #[test]
    fn matches_vsf() {
        for &(c, _, _) in CODES.iter() {
            let s: String = [c].iter().collect();
            assert_eq!(ours(&s), VsfType::x(s.clone()).flatten(), "{c:?}");
        }
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..4096 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let len = (x % 65) as usize;
            let s: String = (0..len)
                .map(|i| {
                    let r = x.rotate_left(i as u32 * 7) ^ (i as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
                    CODES[(r % CODES.len() as u64) as usize].0
                })
                .collect();
            assert_eq!(ours(&s), VsfType::x(s.clone()).flatten(), "{s:?}");
            assert_eq!(identity_seed(&s).unwrap(), *ihi::handle_to_hash(&s).as_bytes());
        }
    }

    #[test]
    fn refuses_untypeable() {
        assert_eq!(identity_seed("é"), Err(Untypeable('é')));
    }
}

#[cfg(test)]
mod proof_reference {
    extern crate std;

    /// The handle proof over a fixed seed of sevens, for checking ahau's on-phone bench against the host (`cargo test -p ahau_core -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn sevens() {
        let p = ihi::handle_proof(&blake3::Hash::from([7u8; 32]));
        std::println!("handle proof over [7; 32]: first byte {}", p.as_bytes()[0]);
    }
}
