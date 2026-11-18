//! The block layout of keyreg/keys.rs's held-key functions (`keyreg_hold`, `keyreg_held_open`, `keyreg_held_derive`, `keyreg_held_keyed`, `keyreg_id_root`), step for step in scalar Rust: the same chaining values, messages, block lengths, counters and flags, with the reference BLAKE3 compression in place of the NEON one.
//! The NEON compression is checked on the device against the blake3 crate (keys.rs `self_test`, `held_test`); what can go wrong here is the layout (a flag, a length, a block split), and the tests below hold each layout to the blake3 crate and to the kernel's software derivations before 2026-11-17, which the person's stored data depends on.
#![cfg_attr(not(test), allow(dead_code))]

const IV: [u32; 8] = [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19];
const CHUNK_START: u32 = 1;
const CHUNK_END: u32 = 2;
const ROOT: u32 = 8;
const KEYED_HASH: u32 = 16;
const DERIVE_KEY_MATERIAL: u32 = 64;
const PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

fn g(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(x);
    s[d] = (s[d] ^ s[a]).rotate_right(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(12);
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(y);
    s[d] = (s[d] ^ s[a]).rotate_right(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(7);
}

/// The reference compression, truncated as the lifted code uses it: the first 32 bytes of output (state words 0-7 XOR 8-15).
fn compress(cv: &[u32; 8], block: &[u8; 64], counter: u64, len: u32, flags: u32) -> [u32; 8] {
    let mut m = [0u32; 16];
    for (k, w) in m.iter_mut().enumerate() {
        *w = u32::from_le_bytes(block[k * 4..k * 4 + 4].try_into().unwrap());
    }
    let mut s = [cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7], IV[0], IV[1], IV[2], IV[3], counter as u32, (counter >> 32) as u32, len, flags];
    for _ in 0..7 {
        g(&mut s, 0, 4, 8, 12, m[0], m[1]);
        g(&mut s, 1, 5, 9, 13, m[2], m[3]);
        g(&mut s, 2, 6, 10, 14, m[4], m[5]);
        g(&mut s, 3, 7, 11, 15, m[6], m[7]);
        g(&mut s, 0, 5, 10, 15, m[8], m[9]);
        g(&mut s, 1, 6, 11, 12, m[10], m[11]);
        g(&mut s, 2, 7, 8, 13, m[12], m[13]);
        g(&mut s, 3, 4, 9, 14, m[14], m[15]);
        let mut p = [0u32; 16];
        for (k, x) in p.iter_mut().enumerate() {
            *x = m[PERMUTATION[k]];
        }
        m = p;
    }
    core::array::from_fn(|k| s[k] ^ s[k + 8])
}

fn cv_of(b: &[u8; 32]) -> [u32; 8] {
    core::array::from_fn(|k| u32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap()))
}
fn bytes_of(w: &[u32; 8]) -> [u8; 32] {
    let mut b = [0u8; 32];
    for (k, x) in w.iter().enumerate() {
        b[k * 4..k * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
    b
}
fn block(parts: &[&[u8]]) -> [u8; 64] {
    let mut b = [0u8; 64];
    let mut at = 0;
    for p in parts {
        b[at..at + p.len()].copy_from_slice(p);
        at += p.len();
    }
    b
}

/// keys.rs `context_key`: `blake3::hazmat::hash_derive_key_context`, public.
fn context_key(ctx: &str) -> [u32; 8] {
    // The context hash: BLAKE3 of the context string under DERIVE_KEY_CONTEXT, all its blocks (here the blake3 crate's own, the one input the lifted code takes from outside).
    cv_of(&blake3::hazmat::hash_derive_key_context(ctx))
}

/// `pad!`: derive_key(rest, wairua ‖ nonce), one block of 64.
fn pad(rest: &str, wairua: &[u8; 32], nonce: &[u8; 32]) -> [u8; 32] {
    bytes_of(&compress(&context_key(rest), &block(&[wairua, nonce]), 0, 64, CHUNK_START | CHUNK_END | ROOT | DERIVE_KEY_MATERIAL))
}

fn xor(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    core::array::from_fn(|k| a[k] ^ b[k])
}

/// `keyreg_hold` and `keyreg_held_open`.
fn hold(rest: &str, wairua: &[u8; 32], nonce: &[u8; 32], plain: &[u8; 32]) -> [u8; 32] {
    xor(plain, &pad(rest, wairua, nonce))
}
fn open(rest: &str, wairua: &[u8; 32], nonce: &[u8; 32], ct: &[u8; 32]) -> [u8; 32] {
    xor(ct, &pad(rest, wairua, nonce))
}

/// `keyreg_held_derive` (in the clear): the key and `msg` (≤ 64 bytes) under the context key, one block up to 32 bytes of message, two past.
fn held_derive(key: &[u8; 32], ctx: &str, msg: &[u8]) -> [u8; 32] {
    let cv = context_key(ctx);
    let len = msg.len() as u32;
    if len <= 32 {
        bytes_of(&compress(&cv, &block(&[key, msg]), 0, 32 + len, CHUNK_START | CHUNK_END | ROOT | DERIVE_KEY_MATERIAL))
    } else {
        let cv1 = compress(&cv, &block(&[key, &msg[..32]]), 0, 64, CHUNK_START | DERIVE_KEY_MATERIAL);
        bytes_of(&compress(&cv1, &block(&[&msg[32..]]), 0, len - 32, CHUNK_END | ROOT | DERIVE_KEY_MATERIAL))
    }
}

/// `keyreg_held_keyed`: the key as the chaining value over `msg` (≤ 128 bytes), one block up to 64, two past.
fn held_keyed(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    let cv = cv_of(key);
    let len = msg.len() as u32;
    if len <= 64 {
        bytes_of(&compress(&cv, &block(&[msg]), 0, len, CHUNK_START | CHUNK_END | ROOT | KEYED_HASH))
    } else {
        let cv1 = compress(&cv, &block(&[&msg[..64]]), 0, 64, CHUNK_START | KEYED_HASH);
        bytes_of(&compress(&cv1, &block(&[&msg[64..]]), 0, len - 64, CHUNK_END | ROOT | KEYED_HASH))
    }
}

/// `keyreg_id_root`: `derive_rows!(0, id_context)` (the ira as 32 bytes of key material) as the key over the device secret and the seed.
fn id_root(ira: &[u8; 32], id_ctx: &str, secret: &[u8; 32], seed: &[u8; 32]) -> [u8; 32] {
    let base = compress(&context_key(id_ctx), &block(&[ira]), 0, 32, CHUNK_START | CHUNK_END | ROOT | DERIVE_KEY_MATERIAL);
    bytes_of(&compress(&base, &block(&[secret, seed]), 0, 64, CHUNK_START | CHUNK_END | ROOT | KEYED_HASH))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REST: &str = "ferros 2026-11-17 identity at rest v0";
    fn val(s: u8) -> [u8; 32] {
        core::array::from_fn(|k| (k as u8).wrapping_mul(s).wrapping_add(s ^ 0x5A))
    }
    fn msg() -> [u8; 128] {
        core::array::from_fn(|k| (k as u8).wrapping_mul(7).wrapping_add(1))
    }

    #[test]
    fn the_pad_is_derive_key_over_the_wairua_and_the_nonce() {
        let (w, n) = (val(3), val(9));
        let mut m = [0u8; 64];
        m[..32].copy_from_slice(&w);
        m[32..].copy_from_slice(&n);
        assert_eq!(pad(REST, &w, &n), blake3::derive_key(REST, &m));
        let p = val(11);
        assert_eq!(open(REST, &w, &n, &hold(REST, &w, &n, &p)), p);
        // Another nonce, another pad.
        assert_ne!(hold(REST, &w, &n, &p), hold(REST, &w, &val(10), &p));
    }

    #[test]
    fn held_derive_is_derive_key_over_the_key_and_the_message() {
        let (k, m) = (val(5), msg());
        for len in [0usize, 1, 5, 31, 32, 33, 37, 63, 64] {
            let mut h = blake3::Hasher::new_derive_key("ferros model");
            h.update(&k);
            h.update(&m[..len]);
            assert_eq!(held_derive(&k, "ferros model", &m[..len]), *h.finalize().as_bytes(), "{len}");
        }
    }

    #[test]
    fn held_keyed_is_keyed_hash() {
        let (k, m) = (val(6), msg());
        for len in [0usize, 1, 10, 63, 64, 65, 100, 127, 128] {
            assert_eq!(held_keyed(&k, &m[..len]), *blake3::keyed_hash(&k, &m[..len]).as_bytes(), "{len}");
        }
    }

    /// The kernel's unlock before 2026-11-17 (kvault `unlock`): base = derive_key(ID_CONTEXT, ira) in memory, root = keyed_hash(base, secret ‖ seed), the three keys derive_key(context, root).
    /// The held path must give the same keys, or the person's stored data no longer opens.
    #[test]
    fn the_unlock_chain_is_the_one_before() {
        let (ira, secret, seed) = (val(1), val(2), val(4));
        let id = "ferros identity vault v1";
        let base = blake3::derive_key(id, &ira);
        let mut h = blake3::Hasher::new_keyed(&base);
        h.update(&secret);
        h.update(&seed);
        let root = *h.finalize().as_bytes();
        assert_eq!(id_root(&ira, id, &secret, &seed), root);
        for c in ["ferros identity vault addr v1", "ferros identity vault payload v1", "ferros 2026-11-07 identity vault domain"] {
            assert_eq!(held_derive(&root, c, &[]), blake3::derive_key(c, &root));
        }
        // The wallpaper seed (ahau_core::wallpaper_seed's).
        assert_eq!(held_derive(&seed, "ferros 2026-11-07 wallpaper seed", &[]), blake3::derive_key("ferros 2026-11-07 wallpaper seed", &seed));
    }

    /// The kist alias (kvault `alias`): fgtw's known answer, the 96 bytes 0 to 95 (fgtw src/alias.rs `seed_known_answer`).
    #[test]
    fn the_alias_is_fgtws_known_answer() {
        let f: [u8; 96] = core::array::from_fn(|k| k as u8);
        let want = [
            0xaa, 0x02, 0xea, 0x59, 0x45, 0xdb, 0x02, 0x07, 0x81, 0x6b, 0xc2, 0xd1, 0x9e, 0xf5, 0x80, 0x8d, 0x2a, 0x70, 0x00, 0xbe, 0x7c, 0xe0, 0xa4, 0xdb, 0x91, 0x13, 0xbc, 0xe8, 0x14, 0x8c, 0x37, 0x7f,
        ];
        assert_eq!(held_derive(f[..32].try_into().unwrap(), "ferros 2026-11-12 kist alias v0", &f[32..]), want);
    }

    /// The person's domain keys (process.rs `domain_keys`): a scope's prefix (one block) and a kist's, `kist\0` and its 32-byte domain (two blocks), as the software derivation gave them.
    #[test]
    fn domain_keys_are_the_ones_before() {
        let d = val(8);
        let kist: Vec<u8> = [b"kist\0".as_slice(), &val(12)].concat();
        for prefix in [b"fgtw.".as_slice(), b"kists.", b"map.", &kist] {
            for ctx in ["ferros 2026-11-07 vault domain address", "ferros 2026-11-07 vault domain seal"] {
                let mut h = blake3::Hasher::new_derive_key(ctx);
                h.update(&d);
                h.update(prefix);
                assert_eq!(held_derive(&d, ctx, prefix), *h.finalize().as_bytes());
            }
        }
    }
}
