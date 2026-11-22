//! The background noise in its parallel form (the Photon session's, 2026-11-21: its scalar reference copied here as it is, cpu_lib.rs `reference_row`): each pixel's random bits are a hash of where it is (two lowbias32 of a key of its texture row, its side and its distance from the axis), and its colour a segmented sum of the steps since the last speckle, so the GPU makes it with a workgroup's prefix sum (kernel.rs `record_par`).
//! A different generator from fluor's serial walk (noise.rs `Walk`): the bar is that it looks the same (its statistics against fluor's: tools/compbench-check); whether fluor takes it is the Photon session's call.
//! The GPU's pixels are checked against `reference_row` bit for bit.

/// Bits of the key that hold the pixel's distance from the axis.
pub const K_BITS: u32 = 12;
/// The key's pixel index that is no pixel: the row's starting colour is hashed from it.
pub const K_START: u32 = (1 << K_BITS) - 1;
/// Seeds of the two step words and the twinkle word.
pub const SA: u32 = 0xDEAD_BEEF;
pub const SB: u32 = 0x0123_4567;
pub const SC: u32 = 0x5BD1_E995;
/// The golden-ratio odd constant, spreading a frame number and the twinkle product.
pub const GOLD: u32 = 0x9E37_79B9;
/// One step draw: 9 bits, up and down each uniform over 0 to 2^9.
pub const DRAW: u32 = (1 << 9) - 1;
/// The speckle's low grain bits under its 9 twinkle bits.
pub const GRAIN: u32 = (1 << 7) - 1;
/// lowbias32's two multipliers.
pub const M0: u32 = 0x7FEB_352D;
pub const M1: u32 = 0x846C_A68B;

/// Chris Wellons' lowbias32: two 32-bit multiplies, a bijection on u32.
pub fn lowbias32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(M0);
    x ^= x >> 15;
    x = x.wrapping_mul(M1);
    x ^= x >> 16;
    x
}

/// The key of texture row `t`, side `left`: the row in the high bits, the side next, the pixel index in the low 12.
pub fn row_key(t: u32, left: bool) -> u32 {
    (t << (K_BITS + 1)) | ((left as u32) << K_BITS)
}

/// The two step words of key `key`.
pub fn words(key: u32) -> (u32, u32) {
    (lowbias32(key ^ SA), lowbias32(key ^ SB))
}

/// A speckle: 5 bits of `a` and 3 of `b` all zero, one pixel in 2^8.
pub fn is_speckle(a: u32, b: u32) -> bool {
    ((a >> 27) | (b >> 29)) == 0
}

/// The colour a speckle (or a row's start) sets, per channel R, G, B, before the mask.
pub fn start(a: u32, b: u32) -> [u32; 3] {
    [a, b, (a ^ b) >> 12]
}

/// The step per channel, up minus down, before the mask (masked, the step modulo the lane's 2^k).
pub fn delta(a: u32, b: u32) -> [u32; 3] {
    [(a & DRAW).wrapping_sub((a >> 9) & DRAW), ((a >> 18) & DRAW).wrapping_sub(b & DRAW), ((b >> 9) & DRAW).wrapping_sub((b >> 18) & DRAW)]
}

/// A row's twinkle seed for frame `frame`.
pub fn row_twinkle(rk: u32, frame: u32) -> u32 {
    lowbias32(rk ^ SC ^ frame.wrapping_mul(GOLD))
}

/// A speckle's displayed colour per channel, before the speckle mask: 9 twinkle bits over 7 grain bits.
pub fn twinkle(a: u32, b: u32, srow: u32) -> [u32; 3] {
    let mut tw = (a ^ b ^ srow).wrapping_mul(GOLD);
    tw ^= tw >> 15;
    [((tw >> 23) << 7) | (a & GRAIN), (((tw >> 14) & DRAW) << 7) | (b & GRAIN), (((tw >> 5) & DRAW) << 7) | ((a >> 7) & GRAIN)]
}

/// The three channels of a walk word (layout.rs `widen_mask`, `widen_base`: R in bits 32-47, G 16-31, B 0-15).
pub fn lanes(w: u64) -> [u32; 3] {
    [(w >> 32) as u32 & 0xFFFF, (w >> 16) as u32 & 0xFFFF, w as u32 & 0xFFFF]
}

/// The visible 16-bit colour per channel as a finished opaque G2t16: each lane turned to darkness, α's lane all ones.
pub fn pack(v: [u32; 3]) -> u64 {
    !(((v[0] as u64) << 32) | ((v[1] as u64) << 16) | v[2] as u64)
}

/// One half-row, scalar, straight from the definition: `n` pixels of texture row `t` on side `left` from the axis outward, `mask`, `speckle` and `base` the walk words.
pub fn reference_row(t: u32, left: bool, n: usize, mask: u64, speckle: u64, base: u64, frame: u32, out: &mut [u64]) {
    let (mask, speckle, base) = (lanes(mask), lanes(speckle), lanes(base));
    let rk = row_key(t, left);
    let srow = row_twinkle(rk, frame);
    let (a, b) = words(row_key(t, false) | K_START);
    let s0 = start(a, b);
    let mut c = [0u32; 3];
    for ch in 0..3 {
        c[ch] = s0[ch] & mask[ch];
    }
    for k in 0..n {
        let (a, b) = words(rk | k as u32);
        let mut v = [0u32; 3];
        if is_speckle(a, b) {
            let s0 = start(a, b);
            let tw = twinkle(a, b, srow);
            for ch in 0..3 {
                c[ch] = s0[ch] & mask[ch];
                v[ch] = (tw[ch] & speckle[ch]) + base[ch];
            }
        } else {
            let d = delta(a, b);
            for ch in 0..3 {
                c[ch] = c[ch].wrapping_add(d[ch]) & mask[ch];
                v[ch] = c[ch] + base[ch];
            }
        }
        out[k] = pack(v);
    }
}
