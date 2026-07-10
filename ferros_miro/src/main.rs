// MIRO-P prototype + measurement harness. See MIRO.md (to be written from these numbers).
//
// MIRO-P is BLAKE3's mixing (the G function + column/diagonal round) with the feed-forward
// removed, so the permutation is a clean bijection: forward = mix, inverse = unmix. The
// message-word slots that BLAKE3 feeds data into become fixed round constants here, so G is a
// fixed permutation rather than a compression step.
//
// This harness measures the three things that must hold before MIRO.md asserts anything:
//   1. round-trip: permute then permute_inv is the identity (invertibility is real, not hoped)
//   2. avalanche: one input bit flipped diffuses to ~half the 512 output bits (mixing is real)
//   3. corruption detection + perf: seal/open catches flips, and how fast it runs vs round count
//
// NOT security-audited. First-cut round-constant schedule. The check-word derivation here is a
// splitmix stand-in for BLAKE3(domain || uid || copy) so the harness has no external deps.

use std::hint::black_box;
use std::time::Instant;

// Nothing-up-sleeve constant pool: the first 16 SHA-256 round constants K.
const K16: [u32; 16] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
];

// BLAKE3's column step (0..4) then diagonal step (4..8).
const QUADS: [(usize, usize, usize, usize); 8] = [
    (0, 4, 8, 12), (1, 5, 9, 13), (2, 6, 10, 14), (3, 7, 11, 15),
    (0, 5, 10, 15), (1, 6, 11, 12), (2, 7, 8, 13), (3, 4, 9, 14),
];

// Per-(position, round) constant pair. Deterministic and reproducible in both directions.
#[inline(always)]
fn mc(p: usize, r: usize) -> (u32, u32) {
    let base = 3 * r;
    (K16[(2 * p + base) & 15], K16[(2 * p + 1 + base) & 15])
}

#[inline(always)]
fn g(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(mx);
    s[d] = (s[d] ^ s[a]).rotate_right(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(12);
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(my);
    s[d] = (s[d] ^ s[a]).rotate_right(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(7);
}

// Exact inverse of g: reverse order, rotl for rotr, sub for add.
#[inline(always)]
fn g_inv(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    s[b] = s[b].rotate_left(7) ^ s[c];
    s[c] = s[c].wrapping_sub(s[d]);
    s[d] = s[d].rotate_left(8) ^ s[a];
    s[a] = s[a].wrapping_sub(s[b]).wrapping_sub(my);
    s[b] = s[b].rotate_left(12) ^ s[c];
    s[c] = s[c].wrapping_sub(s[d]);
    s[d] = s[d].rotate_left(16) ^ s[a];
    s[a] = s[a].wrapping_sub(s[b]).wrapping_sub(mx);
}

fn round(s: &mut [u32; 16], r: usize) {
    for p in 0..8 {
        let (mx, my) = mc(p, r);
        let (a, b, c, d) = QUADS[p];
        g(s, a, b, c, d, mx, my);
    }
}

fn round_inv(s: &mut [u32; 16], r: usize) {
    for p in (0..8).rev() {
        let (mx, my) = mc(p, r);
        let (a, b, c, d) = QUADS[p];
        g_inv(s, a, b, c, d, mx, my);
    }
}

fn permute(s: &mut [u32; 16], rounds: usize) {
    for r in 0..rounds {
        round(s, r);
    }
}

fn permute_inv(s: &mut [u32; 16], rounds: usize) {
    for r in (0..rounds).rev() {
        round_inv(s, r);
    }
}

// --- seal/open (MIRO allocation: k=256 secret, r=256 check) ---

fn check_words(tweak: u64) -> [u32; 8] {
    // Stand-in for BLAKE3(domain || uid || copy). Real crate uses BLAKE3.
    let mut x = tweak ^ 0xA5A5_A5A5_A5A5_A5A5;
    let mut c = [0u32; 8];
    for i in 0..4 {
        let v = splitmix64(&mut x);
        c[2 * i] = v as u32;
        c[2 * i + 1] = (v >> 32) as u32;
    }
    c
}

fn seal(shard: [u32; 8], tweak: u64, rounds: usize) -> [u32; 16] {
    let mut s = [0u32; 16];
    s[0..8].copy_from_slice(&shard);
    s[8..16].copy_from_slice(&check_words(tweak));
    permute(&mut s, rounds);
    s
}

fn open(cw: [u32; 16], tweak: u64, rounds: usize) -> Option<[u32; 8]> {
    let mut s = cw;
    permute_inv(&mut s, rounds);
    if s[8..16] == check_words(tweak) {
        let mut o = [0u32; 8];
        o.copy_from_slice(&s[0..8]);
        Some(o)
    } else {
        None
    }
}

// --- reproducible PRNG (no system entropy, deterministic vectors) ---

fn splitmix64(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn rand_state(seed: &mut u64) -> [u32; 16] {
    let mut s = [0u32; 16];
    for i in 0..8 {
        let v = splitmix64(seed);
        s[2 * i] = v as u32;
        s[2 * i + 1] = (v >> 32) as u32;
    }
    s
}

fn hamming(a: &[u32; 16], b: &[u32; 16]) -> u32 {
    let mut d = 0;
    for i in 0..16 {
        d += (a[i] ^ b[i]).count_ones();
    }
    d
}

fn main() {
    println!("MIRO-P measurement harness  (512-bit state, BLAKE3-G core, feed-forward removed)\n");

    // 1. ROUND-TRIP: permute∘permute_inv == identity, all round counts.
    {
        let mut seed = 0x0123_4567_89AB_CDEF;
        let mut fails = 0u64;
        for _ in 0..200_000 {
            let orig = rand_state(&mut seed);
            for &rounds in &[1usize, 7, 8, 16, 24] {
                let mut s = orig;
                permute(&mut s, rounds);
                permute_inv(&mut s, rounds);
                if s != orig {
                    fails += 1;
                }
            }
        }
        println!("[1] round-trip over 200k states x {{1,7,8,16,24}} rounds: {}",
                 if fails == 0 { "ALL identity  ✓".to_string() } else { format!("{fails} FAILURES ✗") });
    }

    // 2. AVALANCHE: flip one input bit, measure mean output Hamming distance (ideal = 256/512).
    {
        println!("\n[2] avalanche  (ideal = 256.0 bits = 50.0%)");
        let mut seed = 0xF00D_BABE_1234_5678;
        for &rounds in &[1usize, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16] {
            let mut total = 0u64;
            let mut samples = 0u64;
            for _ in 0..4000 {
                let base = rand_state(&mut seed);
                let mut out0 = base;
                permute(&mut out0, rounds);
                // flip a handful of distinct bit positions per base state
                for &bit in &[0usize, 137, 255, 383, 511] {
                    let mut flipped = base;
                    flipped[bit / 32] ^= 1 << (bit % 32);
                    permute(&mut flipped, rounds);
                    total += hamming(&out0, &flipped) as u64;
                    samples += 1;
                }
            }
            let mean = total as f64 / samples as f64;
            println!("    rounds {:>2}: {:>7.2} bits  ({:>5.1}%)", rounds, mean, 100.0 * mean / 512.0);
        }
    }

    // 3. CORRUPTION DETECTION: seal, flip each codeword bit, open must reject.
    {
        let mut seed = 0xDEAD_BEEF_CAFE_F00D;
        let rounds = 16;
        let mut trials = 0u64;
        let mut detected = 0u64;
        let mut clean_ok = 0u64;
        for _ in 0..20_000 {
            let mut sh = [0u32; 8];
            let r = rand_state(&mut seed);
            sh.copy_from_slice(&r[0..8]);
            let tweak = splitmix64(&mut seed);
            let cw = seal(sh, tweak, rounds);
            // clean open recovers the shard
            if open(cw, tweak, rounds) == Some(sh) {
                clean_ok += 1;
            }
            // flip one random bit -> must be rejected
            let bit = (splitmix64(&mut seed) % 512) as usize;
            let mut bad = cw;
            bad[bit / 32] ^= 1 << (bit % 32);
            trials += 1;
            if open(bad, tweak, rounds).is_none() {
                detected += 1;
            }
        }
        println!("\n[3] seal/open  (rounds=16)");
        println!("    clean opens recovered:   {clean_ok}/20000");
        println!("    single-bit flips caught: {detected}/{trials}");
    }

    // 4. PERF: forward permute, and seal+open, vs round count.
    {
        println!("\n[4] perf  (opt-level=3, lto)  — ns/op, chained to defeat DCE");
        for &rounds in &[8usize, 16, 24] {
            let iters = 20_000_000u64;
            let mut s = rand_state(&mut 0x1111_2222_3333_4444);
            let t = Instant::now();
            for _ in 0..iters {
                permute(black_box(&mut s), rounds);
            }
            let ns = t.elapsed().as_nanos() as f64 / iters as f64;
            black_box(&s);
            let mbps = 64.0 / (ns / 1000.0); // 64 bytes per permutation, ns->us
            println!("    permute  rounds {:>2}: {:>6.1} ns  ({:>6.0} MB/s over 64B blocks)", rounds, ns, mbps);
        }
        // seal+open together at the shipping round count
        {
            let rounds = 16usize;
            let iters = 5_000_000u64;
            let mut sh = [0u32; 8];
            let r = rand_state(&mut 0x9999_8888_7777_6666);
            sh.copy_from_slice(&r[0..8]);
            let tweak = 0x5555_5555_5555_5555;
            let t = Instant::now();
            for _ in 0..iters {
                let cw = seal(black_box(sh), tweak, rounds);
                let got = open(black_box(cw), tweak, rounds).unwrap();
                sh = got; // chain
            }
            let ns = t.elapsed().as_nanos() as f64 / iters as f64;
            black_box(&sh);
            println!("    seal+open rounds 16: {:>6.1} ns  (one 32B shard sealed and re-opened)", ns);
        }
    }

    println!("\ndone.");
}
