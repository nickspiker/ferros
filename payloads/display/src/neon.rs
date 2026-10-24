//! The present stage's publish in NEON: 16 pixels per step, from the 8-bit canvas or an app's 16-bit surface, through the ODT (`Odt`, below) to one temporal phase frame's 10-bit words. One branchless path for every pixel: no grey test, no scalar fallback, no clamp.

use core::arch::aarch64::*;

/// 16 canvas pixels in one de-interleaving load: bytes B, G, R, A into four registers. In asm: under the target's strict-align LLVM rewrote `vld4q_u8` into 32-bit scalar loads glued with `orr` and passed through the stack (seen in the emitted code, 2026-10-24); processes now build without it (`cargo el0`), and the asm keeps the one load whatever the build. `ld4` needs four consecutive registers, so they are named.
#[inline(always)]
pub unsafe fn ld4(src: *const u32) -> (uint8x16_t, uint8x16_t, uint8x16_t) {
    let (b, g, r): (uint8x16_t, uint8x16_t, uint8x16_t);
    unsafe {
        core::arch::asm!(
            "ld4 {{v28.16b, v29.16b, v30.16b, v31.16b}}, [{p}]",
            p = in(reg) src,
            out("v28") b, out("v29") g, out("v30") r, out("v31") _,
            options(readonly, nostack, preserves_flags),
        );
    }
    (b, g, r)
}

/// The threshold vector for 16 pixels starting at a multiple of 16 on row `y`, for phase `k` (draw-side `threshold`: Bayer × 4 + the temporal rank offset per 4×4 cell).
pub fn thresholds(y: usize, k: usize, bayer: &[[u16; 4]; 4], rank: &[u32; 4]) -> [u8; 16] {
    let mut t = [0u8; 16];
    for (x, e) in t.iter_mut().enumerate() {
        *e = (bayer[y & 3][x & 3] as u32 * 4 + rank[(k + (x >> 2) + (y >> 2)) & 3]) as u8;
    }
    t
}

/// The ODT for every pixel, one branchless path (owner, 2026-10-24): no grey test, no scalar fallback, no clamp. Per 16 pixels:
/// - light (f16, the E peak at 2^14): for the 8-bit canvas, u = c²/65536 (c² exact by a u8 multiply, one fixed-point convert) and light = u·(a1 + u·(a2 + u·(a3 + u·a4))), the roll-off tanh²(k·v) fitted at each brightness (main.rs `odt_tables`); for an app's 16-bit working values, tanh(k·v) in f32 by Lambert's continued fraction to 7th order (within 0.08 Q4 to 16× white), squared, narrowed to f16;
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
    /// f16 bits by lane: [K_BB, a4, 0, ...].
    pub c1: uint16x8_t,
    /// The canvas polynomial's a3, a2, a1, broadcast (f16 bits): Horner's accumulators start from them.
    pub a3: uint16x8_t,
    pub a2: uint16x8_t,
    pub a1: uint16x8_t,
    /// The app light's f32 constants: [k/16384, 2^14, 0, 0].
    pub k: float32x4_t,
}

/// The canvas's three channels, eight pixels, to light (f16 bits).
#[inline(always)]
unsafe fn light8(o: &Odt, r: uint8x8_t, g: uint8x8_t, b: uint8x8_t) -> (uint16x8_t, uint16x8_t, uint16x8_t) {
    unsafe {
        let (r2, g2, b2) = (vmull_u8(r, r), vmull_u8(g, g), vmull_u8(b, b));
        let (lr, lg, lb): (uint16x8_t, uint16x8_t, uint16x8_t);
        core::arch::asm!(
            "ucvtf {ur:v}.8h, {r2:v}.8h, #16",
            "ucvtf {ug:v}.8h, {g2:v}.8h, #16",
            "ucvtf {ub:v}.8h, {b2:v}.8h, #16",
            "mov {hr:v}.16b, {a3:v}.16b",
            "mov {hg:v}.16b, {a3:v}.16b",
            "mov {hb:v}.16b, {a3:v}.16b",
            "fmla {hr:v}.8h, {ur:v}.8h, {c1:v}.h[1]",
            "fmla {hg:v}.8h, {ug:v}.8h, {c1:v}.h[1]",
            "fmla {hb:v}.8h, {ub:v}.8h, {c1:v}.h[1]",
            "mov {lr:v}.16b, {a2:v}.16b",
            "mov {lg:v}.16b, {a2:v}.16b",
            "mov {lb:v}.16b, {a2:v}.16b",
            "fmla {lr:v}.8h, {hr:v}.8h, {ur:v}.8h",
            "fmla {lg:v}.8h, {hg:v}.8h, {ug:v}.8h",
            "fmla {lb:v}.8h, {hb:v}.8h, {ub:v}.8h",
            "mov {hr:v}.16b, {a1:v}.16b",
            "mov {hg:v}.16b, {a1:v}.16b",
            "mov {hb:v}.16b, {a1:v}.16b",
            "fmla {hr:v}.8h, {lr:v}.8h, {ur:v}.8h",
            "fmla {hg:v}.8h, {lg:v}.8h, {ug:v}.8h",
            "fmla {hb:v}.8h, {lb:v}.8h, {ub:v}.8h",
            "fmul {lr:v}.8h, {hr:v}.8h, {ur:v}.8h",
            "fmul {lg:v}.8h, {hg:v}.8h, {ug:v}.8h",
            "fmul {lb:v}.8h, {hb:v}.8h, {ub:v}.8h",
            r2 = in(vreg) r2, g2 = in(vreg) g2, b2 = in(vreg) b2,
            c1 = in(vreg_low16) o.c1, a3 = in(vreg) o.a3, a2 = in(vreg) o.a2, a1 = in(vreg) o.a1,
            ur = out(vreg) _, ug = out(vreg) _, ub = out(vreg) _, hr = out(vreg) _, hg = out(vreg) _, hb = out(vreg) _,
            lr = out(vreg) lr, lg = out(vreg) lg, lb = out(vreg) lb,
            options(pure, nomem, nostack),
        );
        (lr, lg, lb)
    }
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

/// 16 canvas pixels to their Q4 codes: R, G, B, each as two halves of eight.
#[inline(always)]
pub unsafe fn odt16(o: &Odt, src: *const u32) -> [uint16x8_t; 6] {
    unsafe {
        let (b, g, r) = ld4(src);
        odt16_bgr(o, b, g, r)
    }
}

/// `odt16` from the three channel planes already loaded.
#[inline(always)]
unsafe fn odt16_bgr(o: &Odt, b: uint8x16_t, g: uint8x16_t, r: uint8x16_t) -> [uint16x8_t; 6] {
    unsafe {
        let (lr0, lg0, lb0) = light8(o, vget_low_u8(r), vget_low_u8(g), vget_low_u8(b));
        let (lr1, lg1, lb1) = light8(o, vget_high_u8(r), vget_high_u8(g), vget_high_u8(b));
        let (r0, g0, b0) = matrix8(o, lr0, lg0, lb0);
        let (r1, g1, b1) = matrix8(o, lr1, lg1, lb1);
        let (cr, cg, cb) = (code16(o, r0, r1), code16(o, g0, g1), code16(o, b0, b1));
        [cr.0, cr.1, cg.0, cg.1, cb.0, cb.1]
    }
}

/// 16 canvas pixels through the ODT, dithered by this row's thresholds, into 16 words. Sixteen black pixels (the map's empty plane, most of the screen) are 16 black words without the ODT: no work for nothing, one branch per 16 pixels that holds its direction across a black run (owner, 2026-10-24: a crowded screen's full publish spent half its time making black black).
#[inline(always)]
pub unsafe fn sixteen_odt(o: &Odt, src: *const u32, dst: *mut u32, th: uint8x16_t) {
    unsafe {
        let (b, g, r) = ld4(src);
        if vmaxvq_u8(vorrq_u8(vorrq_u8(r, g), b)) == 0 {
            let k = vdupq_n_u32(3);
            core::arch::asm!("stp {k:q}, {k:q}, [{p}]", "stp {k:q}, {k:q}, [{p}, #32]", p = in(reg) dst, k = in(vreg) k, options(nostack, preserves_flags));
            return;
        }
        dither_pack(odt16_bgr(o, b, g, r), th, dst)
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

/// Four sprite pixels under four canvas pixels, front to back (draw::under, branch-free): each byte d + (s·room + 127) / 255, saturated, room = 255 − the canvas pixel's alpha — which is already s where the canvas is empty (room 255) and d where it is opaque (room 0). The divide by 255 as (x + (x >> 8) + 128) >> 8, within one of the scalar's rounding.
#[inline(always)]
pub unsafe fn under4(dst: *mut u32, src: *const u32) {
    unsafe {
        const ALPHA: [u8; 16] = [3, 3, 3, 3, 7, 7, 7, 7, 11, 11, 11, 11, 15, 15, 15, 15];
        let d = vld1q_u8(dst as *const u8);
        let s = vld1q_u8(src as *const u8);
        let room = vmvnq_u8(vqtbl1q_u8(d, vld1q_u8(ALPHA.as_ptr())));
        let lo = vmull_u8(vget_low_u8(s), vget_low_u8(room));
        let hi = vmull_high_u8(s, room);
        let q = vrshrn_high_n_u16::<8>(vrshrn_n_u16::<8>(vsraq_n_u16::<8>(lo, lo)), vsraq_n_u16::<8>(hi, hi));
        vst1q_u8(dst as *mut u8, vqaddq_u8(d, q));
    }
}
