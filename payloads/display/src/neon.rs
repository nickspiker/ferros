//! The present stage's publish in NEON: 16 pixels per step, from the 8-bit canvas or an app's 16-bit surface, through the ODT (`Odt`, below) to one temporal phase frame's 10-bit words. One branchless path for every pixel: no grey test, no scalar fallback, no clamp.

use core::arch::aarch64::*;

/// The threshold vector for 16 pixels starting at a multiple of 16 on row `y`, for phase `k` (draw-side `threshold`: Bayer × 4 + the temporal rank offset per 4×4 cell).
pub fn thresholds(y: usize, k: usize, bayer: &[[u16; 4]; 4], rank: &[u32; 4]) -> [u8; 16] {
    let mut t = [0u8; 16];
    for (x, e) in t.iter_mut().enumerate() {
        *e = (bayer[y & 3][x & 3] as u32 * 4 + rank[(k + (x >> 2) + (y >> 2)) & 3]) as u8;
    }
    t
}

/// The ODT for every pixel, one branchless path (owner, 2026-10-24): no grey test, no scalar fallback, no clamp. Per 16 pixels:
/// - light (f16, the E peak at 2^14): from 16-bit working values (the canvas and an app's surface alike, to 16× white), tanh(k·v) in f32 by Lambert's continued fraction to 7th order (within 0.08 Q4), squared, narrowed to f16 (the 8-bit canvas's polynomial went with it, 2026-10-24);
/// - the ODT as gains + K: p = g∘l + K_R·(l_R − l_G) + K_B·(l_B − l_G), K = ODT − diag(g) (its rows sum to zero, so a grey's differences are exactly 0 and it gets g∘l);
/// - the panel's 1/2.19 from the f16's bits: the exponent picks the code at its octave's foot (TBL, 32 entries, ×2), the top six mantissa bits the curve's nudge across the octave (TBL, 64 entries, Q15) and its slope, interpolated on the low four; one widening multiply and a rounding narrow give the Q4 code. The tables are the clamp: a negative light's sign bit puts its exponent index at 32 or more, which TBL answers with 0 (code 0); exponents past the peak carry a scale that overflows, and the saturating dither holds it at the top;
/// - the dither (saturating add, saturating narrowing shift) and the pack: the 10-10-10-2 word's four bytes as four planes, one interleaving store.
/// Every table is a register-shaped field, loaded whole.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
pub struct Odt {
    pub nudge_hi: uint8x16x4_t,
    pub nudge_lo: uint8x16x4_t,
    pub slope: uint8x16x4_t,
    pub scale_hi: uint8x16x2_t,
    pub scale_lo: uint8x16x2_t,
    /// f16 bits by lane: [g_R, g_G, g_B, K_RR, K_RB, K_GR, K_GB, K_BR].
    pub c0: uint16x8_t,
    /// f16 bits by lane: [K_BB, 0, ...].
    pub c1: uint16x8_t,
    /// The app light's f32 constants: [k/16384, 2^14, 0, 0].
    pub k: float32x4_t,
}

/// An app's 16-bit working values (gamma 2, white = Q14, to 4.0), eight of them, to light (f16 bits): x = k·v/16384, tanh(x) by Lambert's continued fraction, x·(135135 + 17325x² + 378x⁴ + x⁶) / (135135 + 62370x² + 3150x⁴ + 28x⁶), squared, ×2^14.
#[inline(always)]
unsafe fn light16(o: &Odt, v: uint16x8_t) -> uint16x8_t {
    unsafe {
        let half = |w: uint32x4_t| -> float32x4_t {
            let x = vmulq_laneq_f32::<0>(vcvtq_f32_u32(w), o.k);
            let y = vmulq_f32(x, x);
            let num = vfmaq_f32(vdupq_n_f32(135135.0), y, vfmaq_f32(vdupq_n_f32(17325.0), y, vaddq_f32(vdupq_n_f32(378.0), y)));
            let den = vfmaq_f32(vdupq_n_f32(135135.0), y, vfmaq_f32(vdupq_n_f32(62370.0), y, vfmaq_f32(vdupq_n_f32(3150.0), y, vdupq_n_f32(28.0))));
            let t = vdivq_f32(vmulq_f32(x, num), den);
            vmulq_laneq_f32::<1>(vmulq_f32(t, t), o.k)
        };
        let (lo, hi) = (half(vmovl_u16(vget_low_u16(v))), half(vmovl_high_u16(v)));
        let out: uint16x8_t;
        core::arch::asm!("fcvtn {o:v}.4h, {lo:v}.4s", "fcvtn2 {o:v}.8h, {hi:v}.4s", lo = in(vreg) lo, hi = in(vreg) hi, o = out(vreg) out, options(pure, nomem, nostack));
        out
    }
}

/// Eight pixels' light through the ODT (f16, asm: Rust's f16 is unstable).
#[inline(always)]
unsafe fn matrix8(o: &Odt, lr: uint16x8_t, lg: uint16x8_t, lb: uint16x8_t) -> (uint16x8_t, uint16x8_t, uint16x8_t) {
    unsafe {
        let (pr, pg, pb): (uint16x8_t, uint16x8_t, uint16x8_t);
        core::arch::asm!(
            "fsub {dr:v}.8h, {lr:v}.8h, {lg:v}.8h",
            "fsub {db:v}.8h, {lb:v}.8h, {lg:v}.8h",
            "fmul {pr:v}.8h, {lr:v}.8h, {c0:v}.h[0]",
            "fmul {pg:v}.8h, {lg:v}.8h, {c0:v}.h[1]",
            "fmul {pb:v}.8h, {lb:v}.8h, {c0:v}.h[2]",
            "fmla {pr:v}.8h, {dr:v}.8h, {c0:v}.h[3]",
            "fmla {pg:v}.8h, {dr:v}.8h, {c0:v}.h[5]",
            "fmla {pb:v}.8h, {dr:v}.8h, {c0:v}.h[7]",
            "fmla {pr:v}.8h, {db:v}.8h, {c0:v}.h[4]",
            "fmla {pg:v}.8h, {db:v}.8h, {c0:v}.h[6]",
            "fmla {pb:v}.8h, {db:v}.8h, {c1:v}.h[0]",
            lr = in(vreg) lr, lg = in(vreg) lg, lb = in(vreg) lb,
            c0 = in(vreg_low16) o.c0, c1 = in(vreg_low16) o.c1,
            dr = out(vreg) _, db = out(vreg) _,
            pr = out(vreg) pr, pg = out(vreg) pg, pb = out(vreg) pb,
            options(pure, nomem, nostack),
        );
        (pr, pg, pb)
    }
}

/// 16 panel lights (f16 bits, two halves) to their Q4 10-bit codes (two halves).
#[inline(always)]
unsafe fn code16(o: &Odt, a: uint16x8_t, b: uint16x8_t) -> (uint16x8_t, uint16x8_t) {
    unsafe {
        let (a8, b8) = (vreinterpretq_u8_u16(a), vreinterpretq_u8_u16(b));
        // The 16 lights' high bytes (sign, exponent, top two mantissa bits) and low bytes, each in one unzip.
        let (hi, lo) = (vuzp2q_u8(a8, b8), vuzp1q_u8(a8, b8));
        let e = vshrq_n_u8::<2>(hi);
        let m6 = vandq_u8(vsliq_n_u8::<4>(vshrq_n_u8::<4>(lo), hi), vdupq_n_u8(63));
        let m4 = vandq_u8(lo, vdupq_n_u8(15));
        let (nh, nl, sl) = (vqtbl4q_u8(o.nudge_hi, m6), vqtbl4q_u8(o.nudge_lo, m6), vqtbl4q_u8(o.slope, m6));
        let (sh, sl2) = (vqtbl2q_u8(o.scale_hi, e), vqtbl2q_u8(o.scale_lo, e));
        // The nudge across the octave, interpolated: n + (slope·m4 + 8) >> 4.
        let f0 = vrsraq_n_u16::<4>(vreinterpretq_u16_u8(vzip1q_u8(nl, nh)), vmull_u8(vget_low_u8(sl), vget_low_u8(m4)));
        let f1 = vrsraq_n_u16::<4>(vreinterpretq_u16_u8(vzip2q_u8(nl, nh)), vmull_high_u8(sl, m4));
        let (s0, s1) = (vreinterpretq_u16_u8(vzip1q_u8(sl2, sh)), vreinterpretq_u16_u8(vzip2q_u8(sl2, sh)));
        // code = f·(2·foot) >> 16, rounded.
        let c = |f: uint16x8_t, s: uint16x8_t| vrshrn_high_n_u32::<16>(vrshrn_n_u32::<16>(vmull_u16(vget_low_u16(f), vget_low_u16(s))), vmull_high_u16(f, s));
        (c(f0, s0), c(f1, s1))
    }
}

/// Three channels of 16 Q4 codes, dithered by `th` (one threshold per pixel, 0-63), packed B[31:22] G[21:12] R[11:2] A[1:0] (each code's two low bits zero, for the DQE's truncation) and stored as 16 words: the word's four bytes are four planes — (R << 4) | 3, (G << 6) | (R >> 4), G >> 2, B — written by one interleaving store.
#[inline(always)]
unsafe fn dither_pack(q: [uint16x8_t; 6], th: uint8x16_t, dst: *mut u32) {
    unsafe {
        let (t0, t1) = (vmovl_u8(vget_low_u8(th)), vmovl_high_u8(th));
        let code = |a: uint16x8_t, b: uint16x8_t| vqshrn_high_n_u16::<6>(vqshrn_n_u16::<6>(vqaddq_u16(a, t0)), vqaddq_u16(b, t1));
        let (r, g, b) = (code(q[0], q[1]), code(q[2], q[3]), code(q[4], q[5]));
        let p0 = vsliq_n_u8::<4>(vdupq_n_u8(3), r);
        let p1 = vsliq_n_u8::<6>(vshrq_n_u8::<4>(r), g);
        let p2 = vshrq_n_u8::<2>(g);
        core::arch::asm!(
            "st4 {{v28.16b, v29.16b, v30.16b, v31.16b}}, [{p}]",
            p = in(reg) dst,
            in("v28") p0, in("v29") p1, in("v30") p2, in("v31") b,
            options(nostack, preserves_flags),
        );
    }
}

/// 16 of an app's pixels (u64 each: R, G, B working values in bits 0-15, 16-31, 32-47; bit 48 = no dither) through the ODT, dithered by `th` except where bit 48 asks not, into 16 words.
#[inline(always)]
pub unsafe fn sixteen_app(o: &Odt, src: *const u64, dst: *mut u32, th: uint8x16_t) {
    unsafe {
        let p0 = vld4q_u16(src as *const u16);
        let p1 = vld4q_u16((src as *const u16).add(32));
        let (r0, g0, b0) = matrix8(o, light16(o, p0.0), light16(o, p0.1), light16(o, p0.2));
        let (r1, g1, b1) = matrix8(o, light16(o, p1.0), light16(o, p1.1), light16(o, p1.2));
        let (cr, cg, cb) = (code16(o, r0, r1), code16(o, g0, g1), code16(o, b0, b1));
        let one = vdupq_n_u16(1);
        let nd = vcombine_u8(vmovn_u16(vtstq_u16(p0.3, one)), vmovn_u16(vtstq_u16(p1.3, one)));
        dither_pack([cr.0, cr.1, cg.0, cg.1, cb.0, cb.1], vbicq_u8(th, nd), dst);
    }
}

/// The 16 thresholds of row `y`, phase `k`, for 16 pixels starting at any `x` (the pattern repeats every 16 pixels; `base` = `thresholds(y, k, ..)` at x = 0).
#[inline(always)]
pub unsafe fn thresholds_at(base: uint8x16_t, x: usize) -> uint8x16_t {
    unsafe {
        const IDX: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let i = vandq_u8(vaddq_u8(vld1q_u8(IDX.as_ptr()), vdupq_n_u8((x & 15) as u8)), vdupq_n_u8(15));
        vqtbl1q_u8(base, i)
    }
}

/// 16 canvas pixels — the 16-bit canvas (draw.rs `Px`: R, G, B, A in 16-bit lanes, white at Q14, to 4.0) — through the ODT with this row's thresholds into 16 words: the light by Lambert's tanh as an app's (the canvas now has headroom above white), the ODT, the code, the dither, the pack. The canvas is composited over black, so its premultiplied colour is what shows and alpha plays no part. Sixteen black pixels are 16 black words without the ODT.
#[inline(always)]
pub unsafe fn sixteen_canvas(o: &Odt, src: *const u64, dst: *mut u32, th: uint8x16_t) {
    unsafe {
        let p0 = vld4q_u16(src as *const u16);
        let p1 = vld4q_u16((src as *const u16).add(32));
        let any = vorrq_u16(vorrq_u16(vorrq_u16(p0.0, p0.1), p0.2), vorrq_u16(vorrq_u16(p1.0, p1.1), p1.2));
        if vmaxvq_u16(any) == 0 {
            let k = vdupq_n_u32(3);
            core::arch::asm!("stp {k:q}, {k:q}, [{p}]", "stp {k:q}, {k:q}, [{p}, #32]", p = in(reg) dst, k = in(vreg) k, options(nostack, preserves_flags));
            return;
        }
        let (r0, g0, b0) = matrix8(o, light16(o, p0.0), light16(o, p0.1), light16(o, p0.2));
        let (r1, g1, b1) = matrix8(o, light16(o, p1.0), light16(o, p1.1), light16(o, p1.2));
        let (cr, cg, cb) = (code16(o, r0, r1), code16(o, g0, g1), code16(o, b0, b1));
        dither_pack([cr.0, cr.1, cg.0, cg.1, cb.0, cb.1], th, dst);
    }
}

/// Two 64-bit sprite pixels under two canvas pixels, front to back (draw::under, branch-free): each 16-bit lane d + (s · room) / 65535, saturated, room = 65535 − the canvas pixel's alpha (replicated across its four lanes by one TBL). Already s where the canvas is empty and d where it is opaque. The divide as (x + (x >> 16) + 2^15) >> 16.
#[inline(always)]
pub unsafe fn under2(dst: *mut u64, src: *const u64) {
    unsafe {
        const ALPHA: [u8; 16] = [6, 7, 6, 7, 6, 7, 6, 7, 14, 15, 14, 15, 14, 15, 14, 15];
        let d = vld1q_u16(dst as *const u16);
        let s = vld1q_u16(src as *const u16);
        let a = vreinterpretq_u16_u8(vqtbl1q_u8(vreinterpretq_u8_u16(d), vld1q_u8(ALPHA.as_ptr())));
        let room = vmvnq_u16(a);
        let lo = vmull_u16(vget_low_u16(s), vget_low_u16(room));
        let hi = vmull_high_u16(s, room);
        let q = vrshrn_high_n_u32::<16>(vrshrn_n_u32::<16>(vsraq_n_u32::<16>(lo, lo)), vsraq_n_u32::<16>(hi, hi));
        vst1q_u16(dst as *mut u16, vqaddq_u16(d, q));
    }
}
