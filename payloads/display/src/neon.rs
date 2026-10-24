//! The present stage's publish in NEON: 16 pixels per step from the 8-bit canvas to one temporal phase frame's 10-bit words.
//!
//! Per 16 pixels: one de-interleaving load splits the channels; each channel's 256-entry table (its Q6 8-bit code, split into the code byte and the six fraction bits) is four `TBL`/`TBX` lookups per byte plane; the dither is one add and a shift — `code + ((fraction + threshold) >> 6)`, since both are under 64 the carry is 0 or 1 — and the threshold vector is the same for every 16-pixel step of a row (x>>2 advances by four, a whole turn of the temporal rank); then widen and pack B[31:22] G[21:12] R[11:2] A[1:0] with the low two bits of each code zero (the DQE's truncation passes the 8-bit codes). The scalar path (`publish10x4`) stays for the edges and is the reference this must match.

use core::arch::aarch64::*;

/// One channel's table as four 64-byte segments for `vqtbl4q_u8` / `vqtbx4q_u8`: the 8-bit code, and the fraction (0-63).
#[derive(Clone, Copy)]
pub struct Tables {
    pub code: [[[u8; 64]; 4]; 3],
    pub frac: [[[u8; 64]; 4]; 3],
}

impl Tables {
    /// From the Q4 10-bit (= Q6 8-bit) tables the scalar path uses.
    pub fn from_lut8(lut8: &[[u16; 256]; 3]) -> Tables {
        let mut t = Tables { code: [[[0; 64]; 4]; 3], frac: [[[0; 64]; 4]; 3] };
        for ch in 0..3 {
            for i in 0..256 {
                let q = lut8[ch][i];
                t.code[ch][i >> 6][i & 63] = (q >> 6).min(255) as u8;
                t.frac[ch][i >> 6][i & 63] = (q & 63) as u8;
            }
        }
        t
    }
}

#[inline(always)]
unsafe fn lookup(seg: &[[u8; 64]; 4], idx: uint8x16_t) -> uint8x16_t {
    unsafe {
        let t = |s: &[u8; 64]| uint8x16x4_t(vld1q_u8(s.as_ptr()), vld1q_u8(s.as_ptr().add(16)), vld1q_u8(s.as_ptr().add(32)), vld1q_u8(s.as_ptr().add(48)));
        let sixty4 = vdupq_n_u8(64);
        let mut r = vqtbl4q_u8(t(&seg[0]), idx);
        let i1 = vsubq_u8(idx, sixty4);
        r = vqtbx4q_u8(r, t(&seg[1]), i1);
        let i2 = vsubq_u8(i1, sixty4);
        r = vqtbx4q_u8(r, t(&seg[2]), i2);
        let i3 = vsubq_u8(i2, sixty4);
        vqtbx4q_u8(r, t(&seg[3]), i3)
    }
}

/// One channel through its table and the dither: the 8-bit code.
#[inline(always)]
unsafe fn channel(tabs: &Tables, ch: usize, v: uint8x16_t, th: uint8x16_t) -> uint8x16_t {
    unsafe {
        let code = lookup(&tabs.code[ch], v);
        let frac = lookup(&tabs.frac[ch], v);
        vqaddq_u8(code, vshrq_n_u8::<6>(vaddq_u8(frac, th)))
    }
}

/// 16 pixels of canvas (`src`) to 16 words (`dst`), with this row's threshold vector for the phase. Returns the lanes that are not grey (bit i = pixel i), whose words the caller writes again through the ODT: the channel tables are right only where R = G = B.
#[inline(always)]
pub unsafe fn sixteen(tabs: &Tables, src: *const u32, dst: *mut u32, th: uint8x16_t) -> u32 {
    unsafe {
        // Pixels are G#AARRGGBB little-endian: bytes B, G, R, A.
        let p = vld4q_u8(src as *const u8);
        let (b, g, r) = (p.0, p.1, p.2);
        if vmaxvq_u8(vorrq_u8(vorrq_u8(r, g), b)) == 0 {
            let k = vdupq_n_u32(3);
            for i in 0..4 {
                vst1q_u32(dst.add(4 * i), k);
            }
            return 0;
        }
        let (rc, gc, bc) = (channel(tabs, 0, r, th), channel(tabs, 1, g, th), channel(tabs, 2, b, th));
        // Widen each code byte to a word and place it: blue at 24, green at 14, red at 4, alpha bits 3.
        let widen = |v: uint8x16_t| -> [uint32x4_t; 4] {
            let lo = vmovl_u8(vget_low_u8(v));
            let hi = vmovl_high_u8(v);
            [vmovl_u16(vget_low_u16(lo)), vmovl_high_u16(lo), vmovl_u16(vget_low_u16(hi)), vmovl_high_u16(hi)]
        };
        let (rw, gw, bw) = (widen(rc), widen(gc), widen(bc));
        let three = vdupq_n_u32(3);
        for i in 0..4 {
            let w = vorrq_u32(vorrq_u32(vshlq_n_u32::<24>(bw[i]), vshlq_n_u32::<14>(gw[i])), vorrq_u32(vshlq_n_u32::<4>(rw[i]), three));
            vst1q_u32(dst.add(4 * i), w);
        }
        let grey = vandq_u8(vceqq_u8(r, g), vceqq_u8(g, b));
        if vminvq_u8(grey) == 0xFF {
            return 0;
        }
        let mut lanes = [0u8; 16];
        vst1q_u8(lanes.as_mut_ptr(), grey);
        let mut m = 0u32;
        for (i, l) in lanes.iter().enumerate() {
            if *l == 0 {
                m |= 1 << i;
            }
        }
        m
    }
}

/// The threshold vector for 16 pixels starting at a multiple of 16 on row `y`, for phase `k` (draw-side `threshold`: Bayer × 4 + the temporal rank offset per 4×4 cell).
pub fn thresholds(y: usize, k: usize, bayer: &[[u16; 4]; 4], rank: &[u32; 4]) -> [u8; 16] {
    let mut t = [0u8; 16];
    for (x, e) in t.iter_mut().enumerate() {
        *e = (bayer[y & 3][x & 3] as u32 * 4 + rank[(k + (x >> 2) + (y >> 2)) & 3]) as u8;
    }
    t
}
