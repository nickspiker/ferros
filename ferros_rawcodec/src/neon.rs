//! Hand-written NEON for the encoder's lane steps: the residuals, the block sums and the codes.
//! Each function is the scalar one in lib.rs eight u16 lanes at a time, the same operations in the same widths, so the bytes are the same; the scalar path stays the reference.
//! The residual and code functions do whole vectors of 8 and return where they stopped, the scalar path finishing the 4 a row or block may have left.
//! Untested on hardware when written (2026-11-19, built for aarch64-unknown-none only): run the crate's tests on a device before trusting it.

use crate::{AVG4, BITS, ESC, HALF, LEFT, MASK, PAD, ROW};
use core::arch::aarch64::*;

const LANES: usize = 1 << 3;

/// The scalar zig: e = ((x - p + 512) & 1023) - 512 in i16, then (e << 1) ^ (e >> 15).
#[inline(always)]
fn zig8(x: uint16x8_t, p: int16x8_t) -> uint16x8_t {
    // SAFETY: register-only NEON arithmetic; aarch64-unknown-none and every ferros core have NEON.
    unsafe {
        let t = vaddq_s16(vsubq_s16(vreinterpretq_s16_u16(x), p), vdupq_n_s16(HALF));
        let e = vsubq_s16(vandq_s16(t, vdupq_n_s16(MASK)), vdupq_n_s16(HALF));
        vreinterpretq_u16_s16(veorq_s16(vshlq_n_s16::<1>(e), vshrq_n_s16::<15>(e)))
    }
}

/// The scalar predict on lanes of samples (0 to 1023, so i16 holds every sum and difference as the scalar's i16 does).
#[inline(always)]
fn predict8<const P: u8>(a: uint16x8_t, b: uint16x8_t, c: uint16x8_t, d: uint16x8_t) -> int16x8_t {
    // SAFETY: register-only NEON arithmetic.
    unsafe {
        match P {
            LEFT => vreinterpretq_s16_u16(a),
            AVG4 => vreinterpretq_s16_u16(vshrq_n_u16::<2>(vaddq_u16(vaddq_u16(a, b), vaddq_u16(c, d)))),
            _ => {
                let (a, b, c) = (vreinterpretq_s16_u16(a), vreinterpretq_s16_u16(b), vreinterpretq_s16_u16(c));
                let p = vsubq_s16(vaddq_s16(a, b), c);
                vminq_s16(vmaxq_s16(p, vminq_s16(a, b)), vmaxq_s16(a, b))
            }
        }
    }
}

/// Eight u16 lanes from s[i..i + 8].
#[inline(always)]
fn ld(s: &[u16; ROW], i: usize) -> uint16x8_t {
    let v = &s[i..i + LANES];
    // SAFETY: v is 8 u16 by the slice above.
    unsafe { vld1q_u16(v.as_ptr()) }
}

/// Residuals of whole vectors of a row, returning the first column left for the scalar path (w rounded down to 8).
#[inline(always)]
pub(crate) fn residuals<const P: u8, const DXE: usize, const DXO: usize, const DRE: usize, const DRO: usize>(cur: &[u16; ROW], up: &[u16; ROW], w: usize, m: &mut [u16]) -> usize {
    let end = w & !(LANES - 1);
    // Lanes 0, 2, 4, 6 (even columns, as every vector starts on a multiple of 8) all ones: the quad's parity blend.
    // SAFETY: register-only NEON.
    let even = unsafe { vreinterpretq_u16_u32(vdupq_n_u32(0xFFFF)) };
    let mut x = 0;
    while x < end {
        let i = PAD + x;
        let c = ld(cur, i);
        let b = ld(up, i);
        let (a, ul, ur) = if DXE == DXO {
            (ld(cur, i - DXE), ld(up, i - DXE), ld(up, i + DRE))
        } else {
            // SAFETY: register-only NEON.
            unsafe { (vbslq_u16(even, ld(cur, i - DXE), ld(cur, i - DXO)), vbslq_u16(even, ld(up, i - DXE), ld(up, i - DXO)), vbslq_u16(even, ld(up, i + DRE), ld(up, i + DRO))) }
        };
        let z = zig8(c, predict8::<P>(a, b, ul, ur));
        let o = &mut m[x..x + LANES];
        // SAFETY: o is 8 u16 by the slice above.
        unsafe { vst1q_u16(o.as_mut_ptr(), z) };
        x += LANES;
    }
    end
}

/// A block's sum: up to four vectors of 8 lanes added in u16 (at most 4 x 1023 a lane for a block of 2^5), then widened across.
#[inline(always)]
pub(crate) fn block_sum(v: &[u16]) -> u32 {
    let ch = v.chunks_exact(LANES);
    let rem = ch.remainder();
    // SAFETY: each chunk is 8 u16; register-only NEON otherwise.
    let mut s = unsafe {
        let mut acc = vdupq_n_u16(0);
        for c in ch {
            acc = vaddq_u16(acc, vld1q_u16(c.as_ptr()));
        }
        vaddlvq_u16(acc)
    };
    for &r in rem {
        s += r as u32;
    }
    s
}

/// The scalar codes on whole vectors of 8, returning the first index left for the scalar path.
#[inline(always)]
pub(crate) fn codes(v: &[u16], k: u32, lens: &mut [u16], codes: &mut [u16]) -> usize {
    let end = v.len() & !(LANES - 1);
    let k16 = k as u16;
    // SAFETY: every load and store is 8 u16 inside its slice, bounded by end; register-only NEON otherwise.
    unsafe {
        let shr = vdupq_n_s16(-(k16 as i16));
        let qm = vdupq_n_u16(ESC as u16);
        let one_k = vdupq_n_u16(1 + k16);
        let top = vdupq_n_u16(1 << k16);
        let low = vdupq_n_u16((1u16 << k16) - 1);
        let esc_len = vdupq_n_u16(ESC as u16 + BITS as u16);
        let mut i = 0;
        while i < end {
            let x = vld1q_u16(v[i..i + LANES].as_ptr());
            let q = vshlq_u16(x, shr);
            let esc = vcgeq_u16(q, qm);
            let len_n = vaddq_u16(q, one_k);
            let code_n = vorrq_u16(top, vandq_u16(x, low));
            vst1q_u16(lens[i..i + LANES].as_mut_ptr(), vbslq_u16(esc, esc_len, len_n));
            vst1q_u16(codes[i..i + LANES].as_mut_ptr(), vbslq_u16(esc, x, code_n));
            i += LANES;
        }
    }
    end
}
