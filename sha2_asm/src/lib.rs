//! SHA-256 and SHA-512 compression in pure Rust, in the place of crates.io's sha2-asm (its Cargo.toml says why).
//! - `compress256`: on aarch64 with the `sha2` target feature (ferros builds every target with it, CPU-FEATURES.md), the CPU's SHA-256 instructions thru core::arch's intrinsics; elsewhere the portable rounds of FIPS 180-4.
//! - `compress512`: the portable rounds, off aarch64 only, as sha2-asm has it (sha2 0.10 on aarch64 never calls it).
//! Each takes whole 64- or 128-byte blocks and updates the state in place, as sha2's own `compress256` and `compress512` do.
#![no_std]

/// FIPS 180-4 4.2.2: the first 32 bits of the fractional parts of the cube roots of the first 64 primes.
const K32: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// FIPS 180-4 4.2.3: the first 64 bits of the fractional parts of the cube roots of the first 80 primes.
#[cfg(any(test, not(target_arch = "aarch64")))]
const K64: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc, 0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118, 0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2, 0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694, 0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65, 0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5, 0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4, 0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70, 0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df, 0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b, 0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30, 0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8, 0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8, 0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3, 0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec, 0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b, 0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178, 0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b, 0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c, 0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// SHA-256's compression over whole blocks, the CPU's instructions where the target has them.
#[inline]
pub fn compress256(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    #[cfg(all(target_arch = "aarch64", target_feature = "sha2"))]
    // SAFETY: the target enables `sha2`, so every core this code runs on has the SHA-256 instructions; the loads read only the state and each 64-byte block.
    unsafe {
        compress256_hw(state, blocks)
    }
    #[cfg(not(all(target_arch = "aarch64", target_feature = "sha2")))]
    compress256_soft(state, blocks)
}

/// SHA-512's compression over whole blocks, portable (sha2-asm has none on aarch64 either).
#[cfg(not(target_arch = "aarch64"))]
#[inline]
pub fn compress512(state: &mut [u64; 8], blocks: &[[u8; 128]]) {
    compress512_soft(state, blocks)
}

/// Four rounds at a time with SHA256H and SHA256H2, the schedule's next four words with SHA256SU0 and SHA256SU1 (Arm ARM, "SHA256 instructions"): the message words as big-endian 32-bit lanes, each group of four added to its round constants first.
#[cfg(all(target_arch = "aarch64", target_feature = "sha2"))]
#[target_feature(enable = "sha2")]
unsafe fn compress256_hw(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    use core::arch::aarch64::*;
    unsafe {
        let mut abcd = vld1q_u32(state.as_ptr());
        let mut efgh = vld1q_u32(state.as_ptr().add(4));
        for block in blocks {
            let (abcd0, efgh0) = (abcd, efgh);
            let p = block.as_ptr();
            let mut m0 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p)));
            let mut m1 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p.add(16))));
            let mut m2 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p.add(32))));
            let mut m3 = vreinterpretq_u32_u8(vrev32q_u8(vld1q_u8(p.add(48))));
            for g in 0..16 {
                let wk = vaddq_u32(m0, vld1q_u32(K32.as_ptr().add(g << 2)));
                // The words four groups on: needed for the first twelve groups only (the last four use what is already scheduled).
                let next = if g < 12 { vsha256su1q_u32(vsha256su0q_u32(m0, m1), m2, m3) } else { m0 };
                let before = abcd;
                abcd = vsha256hq_u32(abcd, efgh, wk);
                efgh = vsha256h2q_u32(efgh, before, wk);
                (m0, m1, m2, m3) = (m1, m2, m3, next);
            }
            abcd = vaddq_u32(abcd, abcd0);
            efgh = vaddq_u32(efgh, efgh0);
        }
        vst1q_u32(state.as_mut_ptr(), abcd);
        vst1q_u32(state.as_mut_ptr().add(4), efgh);
    }
}

/// FIPS 180-4 6.2.2, a block at a time.
#[cfg(any(test, not(all(target_arch = "aarch64", target_feature = "sha2"))))]
fn compress256_soft(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
    for block in blocks {
        let mut w = [0u32; 64];
        for t in 0..16 {
            w[t] = u32::from_be_bytes([block[t << 2], block[(t << 2) + 1], block[(t << 2) + 2], block[(t << 2) + 3]]);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16].wrapping_add(s0).wrapping_add(w[t - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for t in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K32[t]).wrapping_add(w[t]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// FIPS 180-4 6.4.2, a block at a time.
#[cfg(any(test, not(target_arch = "aarch64")))]
fn compress512_soft(state: &mut [u64; 8], blocks: &[[u8; 128]]) {
    for block in blocks {
        let mut w = [0u64; 80];
        for t in 0..16 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&block[t << 3..(t << 3) + 8]);
            w[t] = u64::from_be_bytes(b);
        }
        for t in 16..80 {
            let s0 = w[t - 15].rotate_right(1) ^ w[t - 15].rotate_right(8) ^ (w[t - 15] >> 7);
            let s1 = w[t - 2].rotate_right(19) ^ w[t - 2].rotate_right(61) ^ (w[t - 2] >> 6);
            w[t] = w[t - 16].wrapping_add(s0).wrapping_add(w[t - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for t in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K64[t]).wrapping_add(w[t]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// A whole SHA-256 over `m` with `compress`: FIPS 180-4 5.1.1's padding, then the blocks. For the tests here and the device's own check (pqprobe), which hold the compression to known digests.
pub fn sha256_with(m: &[u8], compress: fn(&mut [u32; 8], &[[u8; 64]])) -> [u8; 32] {
    let mut state: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let whole = m.len() >> 6;
    for k in 0..whole {
        let mut b = [0u8; 64];
        b.copy_from_slice(&m[k << 6..(k + 1) << 6]);
        compress(&mut state, &[b]);
    }
    let rest = &m[whole << 6..];
    // The tail, the 1 bit, zeros and the length in bits: one block, or two when the tail leaves no room for the 8-byte length.
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let n = if rest.len() < 56 { 64 } else { 128 };
    tail[n - 8..n].copy_from_slice(&((m.len() as u64) << 3).to_be_bytes());
    for k in 0..n >> 6 {
        let mut b = [0u8; 64];
        b.copy_from_slice(&tail[k << 6..(k + 1) << 6]);
        compress(&mut state, &[b]);
    }
    let mut out = [0u8; 32];
    for (i, s) in state.iter().enumerate() {
        out[i << 2..(i << 2) + 4].copy_from_slice(&s.to_be_bytes());
    }
    out
}

/// FIPS 180-4's "abc" and 448-bit examples' digests.
const ABC: [u8; 32] = [0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad];
const M448: &[u8] = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
const D448: [u8; 32] = [0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8, 0xe5, 0xc0, 0x26, 0x93, 0x0c, 0x3e, 0x60, 0x39, 0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67, 0xf6, 0xec, 0xed, 0xd4, 0x19, 0xdb, 0x06, 0xc1];

/// `chain`'s digest as sha2's software SHA-256 gives it (the host test `the_chain_is_sha2s` holds it to that).
pub const CHAIN: [u8; 32] = [0x0c, 0x3b, 0xd1, 0x03, 0x0a, 0x35, 0x8e, 0x0f, 0xd3, 0xec, 0x7a, 0x48, 0x99, 0xb2, 0x17, 0x59, 0xb6, 0x8b, 0x1b, 0xd7, 0x39, 0x33, 0x4c, 0x51, 0x0d, 0x2b, 0xc0, 0x03, 0xa7, 0x88, 0xd4, 0x7c];

/// A xorshift stream into `out`, for inputs no one picked.
fn fill(out: &mut [u8], seed: u64) {
    let mut x = seed | 1;
    for b in out.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
}

/// One digest over many: the SHA-256 of every length 0 to 1024 and of 256 longer inputs (1025 to 4095 bytes), each of its own stream, folded in order (the running digest and each new one hashed together). Equal to `CHAIN` only when `compress` gives every one of them as sha2's software does.
pub fn chain(compress: fn(&mut [u32; 8], &[[u8; 64]])) -> [u8; 32] {
    let mut buf = [0u8; 1 << 12];
    let mut acc = [0u8; 32];
    for n in 0..1024 + 1 + 256 {
        // WHY the modulo: a length past 1024 picked from the index, below the buffer's 4096 bytes.
        let len = if n <= 1024 { n } else { 1025 + (n * 2654435761) % 3071 };
        fill(&mut buf[..len], n as u64 + 3);
        let h = sha256_with(&buf[..len], compress);
        let mut pair = [0u8; 64];
        pair[..32].copy_from_slice(&acc);
        pair[32..].copy_from_slice(&h);
        acc = sha256_with(&pair, compress);
    }
    acc
}

/// `compress` against FIPS 180-4's two short examples and against `CHAIN`: the device's own check of whichever compression it runs (payloads/pqprobe).
pub fn self_check(compress: fn(&mut [u32; 8], &[[u8; 64]])) -> bool {
    sha256_with(b"abc", compress) == ABC && sha256_with(M448, compress) == D448 && chain(compress) == CHAIN
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// A xorshift stream, for inputs no one picked.
    fn bytes(n: usize, seed: u64) -> Vec<u8> {
        let mut x = seed | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    fn sha2_256(m: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(m).into()
    }

    /// FIPS 180-4's examples (NIST CSRC, SHA256.pdf and SHA2_Additional): empty, "abc", the 448- and 896-bit messages, a million a's.
    #[test]
    fn nist_vectors() {
        let million = std::vec![b'a'; 1_000_000];
        let cases: [(&[u8], &str); 5] = [
            (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
            (b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu", "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"),
            (&million, "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"),
        ];
        for (m, want) in cases {
            assert_eq!(sha256_with(m, compress256).to_vec(), hex(want), "message of {} bytes", m.len());
        }
    }

    /// `CHAIN` is the chain with sha2's own software SHA-256 (thru its `compress256`), and this crate's compressions give it too.
    #[test]
    fn the_chain_is_sha2s() {
        fn sha2_compress(state: &mut [u32; 8], blocks: &[[u8; 64]]) {
            let ga: Vec<sha2::digest::generic_array::GenericArray<u8, sha2::digest::typenum::U64>> = blocks.iter().map(|x| (*x).into()).collect();
            sha2::compress256(state, &ga);
        }
        let want = chain(sha2_compress);
        std::println!("CHAIN = {want:02x?}");
        assert_eq!(want, CHAIN);
        assert!(self_check(compress256));
        assert!(self_check(compress256_soft));
    }

    /// Every length 0 to 1024 and random longer messages: the whole hash against sha2's software one.
    #[test]
    fn every_length_and_random_inputs_match_sha2() {
        for n in 0..=1024 {
            let m = bytes(n, n as u64 + 3);
            assert_eq!(sha256_with(&m, compress256), sha2_256(&m), "length {n}");
        }
        for seed in 0..256u64 {
            let n = (bytes(2, seed + 9)[0] as usize) << 8 | bytes(2, seed + 9)[1] as usize;
            let m = bytes(n, seed + 77);
            assert_eq!(sha256_with(&m, compress256), sha2_256(&m), "seed {seed}, {n} bytes");
        }
    }

    /// The compressions alone, from random states over random runs of blocks, against sha2's own.
    #[test]
    fn compressions_match_sha2_from_any_state() {
        for seed in 0..1024u64 {
            let r = bytes(32 + 64, seed);
            let start: [u32; 8] = r[..32].chunks(4).map(|q| u32::from_le_bytes(q.try_into().unwrap())).collect::<Vec<_>>().try_into().unwrap();
            let n = (r[32] & 7) as usize;
            let data = bytes(n << 6, seed ^ 0xFFFF);
            let blocks: Vec<[u8; 64]> = data.chunks(64).map(|c| c.try_into().unwrap()).collect();
            let (mut a, mut b, mut c) = (start, start, start);
            compress256(&mut a, &blocks);
            compress256_soft(&mut b, &blocks);
            let ga: Vec<sha2::digest::generic_array::GenericArray<u8, sha2::digest::typenum::U64>> = blocks.iter().map(|x| (*x).into()).collect();
            sha2::compress256(&mut c, &ga);
            assert_eq!(a, c, "256, seed {seed}");
            assert_eq!(b, c, "256 portable, seed {seed}");
            let r = bytes(64, seed + 5);
            let mut s5: [u64; 8] = r.chunks(8).map(|q| u64::from_le_bytes(q.try_into().unwrap())).collect::<Vec<_>>().try_into().unwrap();
            let mut t5 = s5;
            let data = bytes(n << 7, seed ^ 0xABCD);
            let blocks: Vec<[u8; 128]> = data.chunks(128).map(|c| c.try_into().unwrap()).collect();
            compress512_soft(&mut s5, &blocks);
            let ga: Vec<sha2::digest::generic_array::GenericArray<u8, sha2::digest::typenum::U128>> = blocks.iter().map(|x| (*x).into()).collect();
            sha2::compress512(&mut t5, &ga);
            assert_eq!(s5, t5, "512, seed {seed}");
        }
    }
}
