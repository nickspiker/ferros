//! The band coder's work on the GPU (the 60/s work, 2026-11-19; owner: "60FPS lock with 2x headroom on GPU or whatever's clever"): ferros_rawcodec `split_codes` and each block's codes laid end to end, as one Valhall kernel, an invocation a block of 32 residuals of one row, the whole frame in one run.
//! The CPU's half is ferros_rawcodec `pack_band_bodies` over what this writes (each block's header, which chains k from the block before, and its body's words into the two streams), `encode_band`'s bytes exactly.
//!
//! An invocation's id is (frame row) × `BLOCKS_LOG2` blocks + its block j; it reads its 40 bytes of RAW10 and its neighbours' (its row, the same-colour row above, and that row's own row above for a band's left edge), and writes:
//! - its block's facts at `info + id × 4`: k in bits 0-3, the selection in bit 4 (ferros_rawcodec `SPLIT_SEL`), the body's length in bits from bit 5 (`SPLIT_BITS_SHIFT`);
//! - its body at `codes + 4 × (id × 2^BODY_LOG2 + BODY_AT)`: its residuals' codes end to end, most significant bit first in u32 words, the last word's spare low bits zero.
//! Bayer only (the binned main's period 2); the quad array stays on the CPU coder.
//! The frame is read from `frame`, the rows `stride` bytes apart, with at least 64 bytes before its first row and after its last that the kernel may read (their values are never used).

use valhall::ssa::{self, Compiled, M, U};

/// Blocks a row in the kernel's numbering: 2^7 = 128 (4080 samples are 127 whole blocks and one of 16).
pub const BLOCKS_LOG2: u32 = 7;
/// u32 words a block's slot: 2^5, its body (at most 32 escapes of 26 bits, 26 words) from `BODY_AT`.
pub const BODY_LOG2: u32 = 5;
/// The body's first word in its slot: word 0 is the kernel's scratch.
pub const BODY_AT: usize = 1;
/// Uniform slots: the bodies' address, the frame's, the facts'.
pub const U_CODES: u8 = 0;
pub const U_FRAME: u8 = 1;
pub const U_INFO: u8 = 2;
/// The samples' bias in the median's compares: every value the median compares is a sample plus this, so the unsigned compares see no negative (a + b - c reaches -1023).
const BIAS: u32 = 1 << 11;

fn c(v: u32) -> U {
    U::splat(v)
}

/// The smaller and the larger of two (unsigned), one compare for both (the recorder shares no node by itself).
fn min_max(a: U, b: U) -> (U, U) {
    let lt = a.lt(b);
    (U::select(lt, a, b), U::select(lt, b, a))
}
fn min(a: U, b: U) -> U {
    U::select(a.lt(b), a, b)
}
fn max(a: U, b: U) -> U {
    U::select(a.lt(b), b, a)
}

/// The residual of sample x against prediction p (any multiple of 1024 added to p, as `BIAS` is), wrapped to the 10-bit ring and zigzagged (ferros_rawcodec `zig`): five instructions.
/// The residual's ten bits are put at the top of the word (r, signed, its sign in bit 31); a negative one has every bit flipped, and the word shifted down to 11 bits is 2r for r >= 0 and 2(1023 - (r mod 1024)) + 1 = -2r - 1 for r < 0 (the flipped bit 21 is the + 1).
fn zig(x: U, p: U) -> U {
    let w = (x - p) << 22;
    let neg = U::from_mask(w.ge(c(1 << 31)));
    (w ^ neg) >> 21
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
    // Five instructions a sample: the high byte shifted straight to bits 2-9 and masked, the low two bits shifted from their byte's place in its word and masked, or'd.
    let sample = |words: &[(i32, U)], i: i32| -> U {
        let word = |o: i32| -> (U, u32) { (words.iter().find(|x| x.0 == o.div_euclid(4)).expect("a word loaded").1, (o.rem_euclid(4) * 8) as u32) };
        let (gi, within) = (i.div_euclid(4), i.rem_euclid(4));
        let (wh, sh) = word(5 * gi + within);
        let hi = if sh >= 2 { (wh >> (sh - 2)) & c(0x3FC) } else { (wh << 2) & c(0x3FC) };
        let (wl, sl) = word(5 * gi + 4);
        let sl = sl + 2 * within as u32;
        let lo = if sl == 0 { wl & c(3) } else { (wl >> sl) & c(3) };
        hi | lo
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
        // Left, or Avg4 with a row above: the prediction chosen, then one residual.
        let ab = a + b;
        let z0 = zig(x, U::select(has_up, (ab + (cc + d)) >> 2, a));
        // MED: the median of a, b and a + b - c, every term biased (the bias is a multiple of 1024, which the residual's ring drops).
        let (lo, hi) = min_max(a, b);
        let p = (ab + c(BIAS)) - cc;
        (z0, zig(x, min(max(p, lo + c(BIAS)), hi + c(BIAS))))
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
    // Each sample's two residuals kept, one register a sample (Left/Avg4 low, MED above bit 16), for the codes after: the second pass reads them, not the frame (A#4610 instructions with a second pass over the frame, 2026-11-19, the GPU's 9 ms a frame).
    let mut kept: Vec<U> = Vec::with_capacity(rc::BLOCK);
    pass(&mut |i, z0, z1| {
        let (a0, a1) = match valid(i) {
            Some(l) => (U::select(l, c(0), z0), U::select(l, c(0), z1)),
            None => (z0, z1),
        };
        s0 = s0 + a0;
        s1 = s1 + a1;
        kept.push(z0 | (z1 << 16));
    });
    let sel = has_up.and_lt(s1, s0);
    let sum = U::select(sel, s1, s0);
    let mean = U::select(last, sum >> (if tail == 0 { 5 } else { tail.trailing_zeros() }), sum >> 5);
    // k = floor(log2 mean), 0 for 0 and 1: a count of the powers of two at or under it.
    let mut k = c(0);
    for e in 1..10u32 {
        k = k + (U::from_mask(mean.ge(c(1 << e))) & c(1));
    }
    let one = c(1);
    let top = one.shl_by(k);
    let low = top - one;
    // The body: each residual's code under the block's predictor and k, appended most significant bit first to a word `acc` holding `n` bits; a code past the word's end spills its low bits into the next.
    let scratch = g << BODY_LOG2;
    let base = scratch + one;
    let mut wi = base;
    let mut acc = c(0);
    let mut n = c(0);
    let shift = U::from_mask(sel) & c(16);
    // Every shift below is 0 to 31 (Valhall's shifts take a byte of the amount, and the emulator refuses 32 or more): a code is first put at the top of a word (its length 1 to 26), then moved down by the n bits already held; the bits that fall off the end are the spill, which is zero unless the code runs past the word.
    let k31 = c(31) - k;
    for (i, &p) in kept.iter().enumerate() {
        let z = p.shr_by(shift) & c(1023);
        let q = z.shr_by(k);
        let esc = q.ge(c(rc::ESC));
        // 32 less the code's length.
        let up = U::select(esc, c(32 - rc::ESC - rc::BITS), k31 - q);
        let mut code = U::select(esc, z, top | (z & low));
        let mut len = c(32) - up;
        // The short last block's spare lanes add nothing (a zero code of no bits leaves every value below as it was).
        if let Some(l) = valid(i) {
            len = U::select(l, c(0), len);
            code = U::select(l, c(0), code);
        }
        let aligned = code.shl_by(up);
        let word = acc | aligned.shr_by(n);
        let total = n + len;
        let full = total.ge(c(32));
        // A word is stored once, whole: one invocation's stores to one address do not land in the order they were made (2026-11-19, the Pro: storing the word at every code left about 40 of 393k blocks a frame with a word missing its last codes, different blocks each run).
        // A word not yet whole goes to the slot's word 0 instead, ahead of the body (`BODY_AT`), which nothing reads: in the body's first cache line, so it costs no line of its own (the slot's last word as the scratch made the run 7.0 ms against 5.7).
        ssa::store_at(U_CODES, U::select(full, wi, scratch), 2, 0, word);
        wi = wi + (U::from_mask(full) & one);
        acc = U::select(full, (aligned << 1).shl_by(c(31) - n), word);
        n = total & c(31);
    }
    // The last word, part full (a zero word past the body when the body ends on a word, inside its room).
    ssa::store_at(U_CODES, wi, 2, 0, acc);
    let bits = ((wi - base) << 5) + n;
    ssa::store_at(U_INFO, g, 2, 0, k | (U::from_mask(sel) & c(rc::SPLIT_SEL)) | (bits << rc::SPLIT_BITS_SHIFT));
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
