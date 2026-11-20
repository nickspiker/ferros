//! The band coder's first half on the GPU (the 60/s work, 2026-11-19; owner: "60FPS lock with 2x headroom on GPU or whatever's clever"): ferros_rawcodec `split_codes` as one Valhall kernel, an invocation a block of 32 residuals of one row, the whole frame in one run.
//! The CPU's half is ferros_rawcodec `pack_band` over what this writes, `encode_band`'s bytes exactly.
//!
//! An invocation's id is (frame row) × `BLOCKS_LOG2` blocks + its block j; it reads its 40 bytes of RAW10 and its neighbours' (its row, the same-colour row above, and that row's own row above for a band's left edge), and writes:
//! - its block's facts at `info + id × 4`: k in bits 0-3, the selection in bit 4 (ferros_rawcodec `SPLIT_SEL`);
//! - its 32 residuals' codes at `codes + id × 64`, two u16 a word (code in bits 0-9, length in 10-14), so a row's codes are `1 << BLOCKS_LOG2` blocks × 32 apart (a short last block's spare lanes land in the row's padding).
//! Bayer only (the binned main's period 2); the quad array stays on the CPU coder.
//! The frame is read from `frame`, the rows `stride` bytes apart, with at least 64 bytes before its first row and after its last that the kernel may read (their values are never used).

use valhall::ssa::{self, Compiled, M, U};

/// Blocks a row in the kernel's numbering: 2^7 = 128 (4080 samples are 127 whole blocks and one of 16).
pub const BLOCKS_LOG2: u32 = 7;
/// Codes a row in the output (`1 << BLOCKS_LOG2` blocks of 32).
pub const CODES_ROW: usize = 32 << BLOCKS_LOG2;
/// Uniform slots: the codes' address, the frame's, the facts'.
pub const U_CODES: u8 = 0;
pub const U_FRAME: u8 = 1;
pub const U_INFO: u8 = 2;
/// The samples' bias in the median's compares: every value the median compares is a sample plus this, so the unsigned compares see no negative (a + b - c reaches -1023).
const BIAS: u32 = 1 << 11;

fn c(v: u32) -> U {
    U::splat(v)
}

/// The smaller and larger of two (unsigned).
fn min(a: U, b: U) -> U {
    U::select(a.lt(b), a, b)
}
fn max(a: U, b: U) -> U {
    U::select(a.lt(b), b, a)
}

/// The residual of sample x against prediction p (both 0 to 1023, p biased by `BIAS` when `biased`), wrapped to the 10-bit ring and zigzagged (ferros_rawcodec `zig`): with t = (x - p + 512) mod 1024, z = 2t - 1024 when t >= 512, else 1023 - 2t.
fn zig(x: U, p: U, biased: bool) -> U {
    let t = if biased { (x + c(512 + BIAS)) - p } else { (x + c(512)) - p } & c(1023);
    let hi = t.ge(c(512));
    U::select(hi, (t << 1) - c(1024), c(1023) - (t << 1))
}

/// The kernel for rows of `width` samples (a multiple of 4, at most 2^`BLOCKS_LOG2` blocks, a short last block a power of two long) `stride` bytes apart, in bands of `ferros_rawcodec::BAND_ROWS`.
/// valhall's SSA recorder is one global, so one thread at a time records a kernel.
pub fn kernel(width: usize, stride: usize) -> Compiled {
    use ferros_rawcodec as rc;
    let nb = rc::row_blocks(width);
    let tail = width % rc::BLOCK;
    assert!(width % 4 == 0 && nb <= 1 << BLOCKS_LOG2, "width {width}");
    assert!(tail == 0 || tail.is_power_of_two(), "a short last block of {tail}: the mean's shift needs a power of two");
    ssa::begin();
    ssa::reserve(3);
    let g = ssa::gid();
    let row = g >> BLOCKS_LOG2;
    let j = g & c((1 << BLOCKS_LOG2) - 1);
    let yb = row & c(rc::BAND_ROWS as u32 - 1);
    let has_up = yb.ge(c(2));
    let has_uu = yb.ge(c(4));
    let first = j.eq(c(0));
    let last = j.eq(c(nb as u32 - 1));
    // The block's byte in its row, then the three rows' (a missing row reads this row's: its values are never used).
    let at = row.wrapping_mul(c(stride as u32)) + j.wrapping_mul(c(40));
    let up = U::select(has_up, at - c(2 * stride as u32), at);
    let uu = U::select(has_uu, at - c(4 * stride as u32), at);
    // A row's words: those its needed samples' bytes lie in (from the first group's first byte to the last group's low byte), loaded once and kept for both passes.
    let words = |base: U, need_lo: i32, need_hi: i32| -> Vec<(i32, U)> {
        let (b0, b1) = (5 * need_lo.div_euclid(4), 5 * need_hi.div_euclid(4) + 4);
        (b0.div_euclid(4)..=b1.div_euclid(4)).map(|w| (w, ssa::load(U_FRAME, base, 0, (4 * w) as i16))).collect()
    };
    // Sample i of a row from its words (RAW10: the high 8 bits at byte 5g + i mod 4 of its group g, the low 2 at byte 5g + 4).
    let sample = |words: &[(i32, U)], i: i32| -> U {
        let byte = |o: i32| -> U {
            let v = words.iter().find(|x| x.0 == o.div_euclid(4)).expect("a word loaded").1;
            (v >> ((o.rem_euclid(4) * 8) as u32)) & c(255)
        };
        let (gi, within) = (i.div_euclid(4), i.rem_euclid(4));
        (byte(5 * gi + within) << 2) | ((byte(5 * gi + 4) >> (2 * within as u32)) & c(3))
    };
    let cw = words(at, -2, 31);
    let uw = words(up, -2, 33);
    let vw = words(uu, 2, 3);
    let edge = c(rc::EDGE as u32);
    let n = if tail == 0 { rc::BLOCK } else { tail };
    // The residuals of sample i under Left (or Avg4 in a row with a row above) and MED: `x`, `a` and the row above's three from a window of samples the caller keeps (each extracted once a pass).
    let resid = |i: i32, cur: &dyn Fn(i32) -> U, upr: &dyn Fn(i32) -> U| -> (U, U) {
        let x = cur(i);
        // a: the row's own sample two back, at the band's left edge its pad (the row above's samples 2 and 3, or EDGE without one).
        let a = if i < 2 { U::select(first, U::select(has_up, upr(i + 2), edge), cur(i - 2)) } else { cur(i - 2) };
        let b = upr(i);
        // c: the row above's sample two back, at the left edge that row's pad (its own row above's 2 and 3, or EDGE).
        let cc = if i < 2 { U::select(first, U::select(has_uu, sample(&vw, i + 2), edge), upr(i - 2)) } else { upr(i - 2) };
        // d: the row above's sample two on, at the right edge that row's last four repeated.
        let d = if (i as usize) == n - 2 || (i as usize) == n - 1 { U::select(last, upr(i - 2), upr(i + 2)) } else { upr(i + 2) };
        let z0 = U::select(has_up, zig(x, ((a + b) + (cc + d)) >> 2, false), zig(x, a, false));
        // MED: the median of a, b and a + b - c, every term biased.
        let (ab_lo, ab_hi) = (min(a, b) + c(BIAS), max(a, b) + c(BIAS));
        let p = (a + b + c(BIAS)) - cc;
        (z0, zig(x, min(max(p, ab_lo), ab_hi), true))
    };
    // A pass over the block: each sample's two residuals, the samples extracted once into a window the pass keeps.
    let pass = |f: &mut dyn FnMut(usize, U, U)| {
        let mut cur: Vec<(i32, U)> = Vec::new();
        let mut upr: Vec<(i32, U)> = Vec::new();
        for i in 0..rc::BLOCK as i32 {
            for (win, wds, lo, hi) in [(&mut cur, &cw, i - 2, i), (&mut upr, &uw, i - 2, i + 2)] {
                win.retain(|x| x.0 >= lo);
                for k in lo..=hi {
                    if !win.iter().any(|x| x.0 == k) {
                        win.push((k, sample(wds, k)));
                    }
                }
            }
            let (cv, uv) = (cur.clone(), upr.clone());
            let get_c = move |k: i32| cv.iter().find(|x| x.0 == k).expect("in the window").1;
            let get_u = move |k: i32| uv.iter().find(|x| x.0 == k).expect("in the window").1;
            let (z0, z1) = resid(i, &get_c, &get_u);
            f(i as usize, z0, z1);
        }
    };
    // The short last block's spare lanes count for nothing.
    let valid = |i: usize| -> Option<M> { if tail != 0 && i >= tail { Some(last) } else { None } };
    let mut s0 = c(0);
    let mut s1 = c(0);
    pass(&mut |i, z0, z1| {
        let (a0, a1) = match valid(i) {
            Some(l) => (U::select(l, c(0), z0), U::select(l, c(0), z1)),
            None => (z0, z1),
        };
        s0 = s0 + a0;
        s1 = s1 + a1;
    });
    let sel = has_up.and_lt(s1, s0);
    let sum = U::select(sel, s1, s0);
    let mean = U::select(last, sum >> (if tail == 0 { 5 } else { tail.trailing_zeros() }), sum >> 5);
    // k = floor(log2 mean), 0 for 0 and 1: a count of the powers of two at or under it.
    let mut k = c(0);
    for e in 1..10u32 {
        k = k + (U::from_mask(mean.ge(c(1 << e))) & c(1));
    }
    let info = k | (U::from_mask(sel) & c(rc::SPLIT_SEL));
    let one = c(1);
    let top = one.shl_by(k);
    let low = top - one;
    let k1 = k + one;
    ssa::store_at(U_INFO, g, 2, 0, info);
    // The second pass: each residual's code under the block's predictor and k, two to a word, each word stored as it is made.
    let mut even = c(0);
    pass(&mut |i, z0, z1| {
        let z = U::select(sel, z1, z0);
        let q = z.shr_by(k);
        let esc = q.ge(c(rc::ESC));
        let len = U::select(esc, c(rc::ESC + rc::BITS), q + k1);
        let code = U::select(esc, z, top | (z & low));
        let p = code | (len << rc::SPLIT_LEN_SHIFT);
        if i & 1 == 0 {
            even = p;
        } else {
            ssa::store_at(U_CODES, g, 6, (2 * (i - 1)) as i16, even | (p << 16));
        }
    });
    ssa::compile()
}

/// A mask's and with an unsigned compare: `m` where `a < b`, as one value.
trait AndLt {
    fn and_lt(self, a: U, b: U) -> M;
}
impl AndLt for M {
    fn and_lt(self, a: U, b: U) -> M {
        (U::from_mask(self) & U::from_mask(a.lt(b))).as_mask()
    }
}
