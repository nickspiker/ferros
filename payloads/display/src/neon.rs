//! The display's pixel work in NEON, 16 pixels a step, one branchless path for every pixel: no grey test, no scalar fallback.
//! - **The present** (owner, 2026-11-12: "a straight thru render and a sqrt to get the gamma close"): the canvas and a baked surface already hold the panel's own codes (COMPOSITOR.md "Baked"), so a phase's words are each code plus its threshold, the high byte, packed (`present16`, `present_app16`, `present_pack`). About one instruction a pixel.
//! - **The bake** (`bake16`, `bake16_lights`): working values (VSF RGB, gamma 2, white at Q14) thru the roll-off, the ODT and the panel's 1/2.19 to those codes, the ODT's tables (`Odt`) as before. What every frame paid until 2026-11-12 is now paid once per image (the wallpaper as it loads, the icons, the console's decoded rows) and per frame only for a surface that is not baked (a format 0 or 1 window).
//! - The old present (`sixteen_canvas`, `codes_app`, `dither_codes`, `thresholds`) is kept for the start-up bench that measures one against the other and for the GPU driver's check of its present kernel (payloads/gpu).

use core::arch::aarch64::*;

/// The present's dither (owner, 2026-11-12: "if I remember right, there's some tricks to get a nice dither for our four panes"): a 16 × 16 blue-noise threshold field, every rank 0-255 once (void and cluster on the torus, Ulichney's σ 1.5), so each phase alone carries the eight fraction bits a baked code has below the 8-bit code the link takes; and phase k turns each threshold's two low bits by `TURN[k]` (an XOR), so over the four phases a pixel visits four thresholds within 3/256 of a code of each other.
/// Measured on the host against the dithers it could have been (each phase's error and the change from one phase to the next thru an eye-like blur of 0.75, 1 and 1.5 px, every fraction of a code; COMPOSITOR.md "The dither"): at rest, where each phase is held a tenth of a second, as quiet as the old 4×4 Bayer with its temporal rank and the same as a 16 × 16 Bayer; the shimmer from phase to phase 2.5 times smaller than the old one's and the smallest of every candidate (a 2 × 2 pattern turned thru the phases shimmers twice as much at arm's length, a fresh noise each phase four times).
pub const BLUE: [[u8; 16]; 16] = [
    [78, 190, 245, 170, 143, 39, 192, 90, 172, 247, 163, 138, 7, 187, 133, 236],
    [3, 152, 47, 71, 210, 120, 234, 0, 213, 114, 20, 203, 83, 55, 208, 98],
    [221, 112, 135, 229, 17, 96, 159, 70, 140, 48, 97, 232, 176, 121, 23, 165],
    [41, 201, 26, 82, 174, 50, 200, 30, 184, 243, 155, 66, 33, 149, 240, 69],
    [88, 252, 158, 195, 129, 248, 108, 224, 80, 118, 15, 188, 91, 215, 110, 183],
    [14, 123, 60, 102, 5, 73, 147, 13, 169, 44, 207, 130, 251, 8, 51, 142],
    [168, 218, 34, 209, 238, 186, 38, 217, 134, 239, 100, 59, 160, 77, 199, 235],
    [99, 74, 179, 139, 53, 164, 122, 92, 64, 180, 4, 227, 31, 173, 119, 29],
    [154, 246, 11, 113, 84, 22, 233, 191, 25, 153, 115, 193, 141, 94, 220, 57],
    [136, 43, 194, 231, 151, 211, 105, 54, 253, 85, 219, 72, 49, 249, 12, 189],
    [212, 81, 124, 65, 35, 178, 2, 145, 204, 36, 131, 19, 205, 157, 76, 111],
    [228, 6, 167, 206, 95, 241, 128, 67, 166, 109, 185, 237, 101, 127, 181, 32],
    [150, 106, 250, 21, 146, 46, 216, 89, 244, 10, 63, 161, 45, 1, 242, 62],
    [177, 40, 137, 61, 196, 116, 18, 182, 42, 148, 230, 87, 214, 144, 198, 93],
    [226, 202, 86, 223, 175, 79, 156, 225, 107, 197, 125, 28, 171, 68, 117, 16],
    [56, 126, 27, 104, 9, 254, 58, 132, 24, 75, 52, 222, 103, 255, 37, 162],
];
/// Each phase's turn of the thresholds' two low bits (an XOR): the order of the four that shimmered least of the 24.
pub const TURN: [u8; 4] = [0, 2, 3, 1];

/// The present's thresholds for row `y` in phase `k`, sixteen of them (the field repeats every 16 pixels, and every publish starts its blocks at a multiple of 16), widened to two u16 vectors for the add: built once a row.
#[inline(always)]
pub fn row_thresholds(y: usize, k: usize) -> (uint16x8_t, uint16x8_t) {
    row_thresholds_at(y, k, 0)
}

/// `row_thresholds` for blocks that start `x` columns past a multiple of 16 (an app's publish starts where its damage does): the row's sixteen turned by one table lookup.
#[inline(always)]
pub fn row_thresholds_at(y: usize, k: usize, x: usize) -> (uint16x8_t, uint16x8_t) {
    unsafe {
        const IDX: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let i = vandq_u8(vaddq_u8(vld1q_u8(IDX.as_ptr()), vdupq_n_u8((x & 15) as u8)), vdupq_n_u8(15));
        let t = vqtbl1q_u8(veorq_u8(vld1q_u8(BLUE[y & 15].as_ptr()), vdupq_n_u8(TURN[k & 3])), i);
        (vmovl_u8(vget_low_u8(t)), vmovl_high_u8(t))
    }
}

/// The panel's channels as the present writes them (main.rs `CHANNELS`): a byte mask a channel, G#FF on and 0 off, each code ANDed with its channel's before the pack.
#[derive(Clone, Copy)]
pub struct Chans(pub uint8x16_t, pub uint8x16_t, pub uint8x16_t);
impl Chans {
    /// From the mask's bits: 0 red, 1 green, 2 blue, set = on.
    pub fn new(m: u32) -> Chans {
        let on = |c: u32| ((m >> c & 1) as u8).wrapping_neg();
        unsafe { Chans(vdupq_n_u8(on(0)), vdupq_n_u8(on(1)), vdupq_n_u8(on(2))) }
    }
}

/// 16 pixels' baked codes (Q16 of the panel's top code: red, green, blue, each two halves of eight) dithered by the row's thresholds (t0, t1) and packed into 16 words. Each code is (b + t) >> 8: the saturating add holds a code past 255.996 at 255 (a baked value is at most 65535, so only the add can pass the field), and the high bytes of sixteen sums are one unzip. Packed B[31:22] G[21:12] R[11:2] A[1:0], each code's two low bits zero for the DQE's truncation: the word's four bytes as four planes, (R << 4) | 3, (G << 6) | (R >> 4), G >> 2, B, written by one interleaving store.
#[inline(always)]
pub unsafe fn present_pack(q: [uint16x8_t; 6], t0: uint16x8_t, t1: uint16x8_t, cm: Chans, dst: *mut u32) {
    unsafe {
        let code = |a: uint16x8_t, b: uint16x8_t| vuzp2q_u8(vreinterpretq_u8_u16(vqaddq_u16(a, t0)), vreinterpretq_u8_u16(vqaddq_u16(b, t1)));
        let (r, g, b) = (vandq_u8(code(q[0], q[1]), cm.0), vandq_u8(code(q[2], q[3]), cm.1), vandq_u8(code(q[4], q[5]), cm.2));
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

/// 16 canvas pixels (baked, its fourth lane its α, which plays no part: the canvas is over black) presented into 16 words.
#[inline(always)]
pub unsafe fn present16(src: *const u64, dst: *mut u32, t0: uint16x8_t, t1: uint16x8_t, cm: Chans) {
    unsafe {
        let p0 = vld4q_u16(src as *const u16);
        let p1 = vld4q_u16((src as *const u16).add(32));
        present_pack([p0.0, p1.0, p0.1, p1.1, p0.2, p1.2], t0, t1, cm, dst);
    }
}

/// 16 composed pixels of a surface or the glass (baked, bit 48 = no dither) presented into 16 words: a pixel with bit 48 takes threshold 0 (the mask's black and orange, flat).
#[inline(always)]
pub unsafe fn present_app16(src: *const u64, dst: *mut u32, t0: uint16x8_t, t1: uint16x8_t, cm: Chans) {
    unsafe {
        let p0 = vld4q_u16(src as *const u16);
        let p1 = vld4q_u16((src as *const u16).add(32));
        let one = vdupq_n_u16(1);
        let (t0, t1) = (vbicq_u16(t0, vtstq_u16(p0.3, one)), vbicq_u16(t1, vtstq_u16(p1.3, one)));
        present_pack([p0.0, p1.0, p0.1, p1.1, p0.2, p1.2], t0, t1, cm, dst);
    }
}

/// The present with the owner's root first, measured against the bake's (COMPOSITOR.md "Baked"; only the start-up bench runs it): 16 pixels whose channels are the square of the panel's code (Q16: light-like values, as a canvas composited in light would hold them), each channel rooted in f16 (`fsqrt`, or with `newton` the estimate and one Newton step, x · rsqrt(x)), back to Q16, and presented.
#[inline(never)]
pub unsafe fn present_sqrt16(src: *const u64, dst: *mut u32, t0: uint16x8_t, t1: uint16x8_t, cm: Chans, newton: bool) {
    unsafe {
        let p0 = vld4q_u16(src as *const u16);
        let p1 = vld4q_u16((src as *const u16).add(32));
        let root = |v: uint16x8_t| -> uint16x8_t {
            let out: uint16x8_t;
            if newton {
                core::arch::asm!(
                    "ucvtf {x:v}.8h, {v:v}.8h, #16",
                    "frsqrte {e:v}.8h, {x:v}.8h",
                    "fmul {t:v}.8h, {x:v}.8h, {e:v}.8h",
                    "frsqrts {t:v}.8h, {t:v}.8h, {e:v}.8h",
                    "fmul {e:v}.8h, {e:v}.8h, {t:v}.8h",
                    "fmul {x:v}.8h, {x:v}.8h, {e:v}.8h",
                    "fcvtzu {o:v}.8h, {x:v}.8h, #16",
                    v = in(vreg) v, x = out(vreg) _, e = out(vreg) _, t = out(vreg) _, o = out(vreg) out,
                    options(pure, nomem, nostack),
                );
            } else {
                core::arch::asm!(
                    "ucvtf {x:v}.8h, {v:v}.8h, #16",
                    "fsqrt {x:v}.8h, {x:v}.8h",
                    "fcvtzu {o:v}.8h, {x:v}.8h, #16",
                    v = in(vreg) v, x = out(vreg) _, o = out(vreg) out,
                    options(pure, nomem, nostack),
                );
            }
            out
        };
        present_pack([root(p0.0), root(p1.0), root(p0.1), root(p1.1), root(p0.2), root(p1.2)], t0, t1, cm, dst);
    }
}

/// 16 working pixels (VSF RGB, gamma 2, white at Q14, to 4.0) baked into 16 (`dst` may be `src`): the light by Lambert's tanh (`light16h`), the ODT, the panel's code (`code16`, Q4 of a 10-bit code, its top 16368), then ×4 into the baked Q16 by a saturating shift. WHY saturating: a light past the panel's peak (a colour the panel's primaries reach only past its top, VSF red's panel red at 1.36 of the peak) codes past 16383, and its ×4 would carry out of the field into a dark code; the old present held it at 255 by its saturating narrow, and the bake holds it at the top the same way. Each pixel's fourth lane (its α or its flags) is kept as it came.
#[inline(always)]
pub unsafe fn bake16(o: &Odt, src: *const u64, dst: *mut u64) {
    unsafe {
        let p0 = vld4q_u16(src as *const u16);
        let p1 = vld4q_u16((src as *const u16).add(32));
        let (r0, g0, b0) = matrix8(o, light16h(o, p0.0), light16h(o, p0.1), light16h(o, p0.2));
        let (r1, g1, b1) = matrix8(o, light16h(o, p1.0), light16h(o, p1.1), light16h(o, p1.2));
        bake_store(o, [r0, r1, g0, g1, b0, b1], p0.3, p1.3, dst);
    }
}

/// 16 pixels' lights (f16 bits, the E peak at 2^14, VSF RGB: red, green, blue, two halves each) thru the ODT and the panel's code into baked pixels, `a0`, `a1` their fourth lanes.
#[inline(always)]
pub unsafe fn bake16_lights(o: &Odt, l: [uint16x8_t; 6], a0: uint16x8_t, a1: uint16x8_t, dst: *mut u64) {
    unsafe {
        let (r0, g0, b0) = matrix8(o, l[0], l[2], l[4]);
        let (r1, g1, b1) = matrix8(o, l[1], l[3], l[5]);
        bake_store(o, [r0, r1, g0, g1, b0, b1], a0, a1, dst);
    }
}

/// Panel lights (red, green, blue, two halves each) to codes, ×4 saturating (`bake16`'s WHY), stored as 16 pixels with their fourth lanes.
#[inline(always)]
unsafe fn bake_store(o: &Odt, p: [uint16x8_t; 6], a0: uint16x8_t, a1: uint16x8_t, dst: *mut u64) {
    unsafe {
        let (cr, cg, cb) = (code16(o, p[0], p[1]), code16(o, p[2], p[3]), code16(o, p[4], p[5]));
        let q = |c: uint16x8_t| vqshlq_n_u16::<2>(c);
        vst4q_u16(dst as *mut u16, uint16x8x4_t(q(cr.0), q(cg.0), q(cb.0), a0));
        vst4q_u16((dst as *mut u16).add(32), uint16x8x4_t(q(cr.1), q(cg.1), q(cb.1), a1));
    }
}

/// Eight baked pixels' codes, red, green and blue (a flight frame's half resolution, COMPOSITOR.md "The flight, prerendered"): the lanes themselves, the codes being what the canvas holds.
#[inline(always)]
pub unsafe fn lanes8(src: *const u64) -> [uint16x8_t; 3] {
    unsafe {
        let p = vld4q_u16(src as *const u16);
        [p.0, p.1, p.2]
    }
}

/// The old present's threshold vector for 16 pixels starting at a multiple of 16 on row `y`, for phase `k` (Bayer × 4 + the temporal rank offset per 4×4 cell), kept for the bench and the GPU driver's check.
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
    /// The app light in f16 (`light16h`), f16 bits by lane: Lambert's numerator and denominator in u = x/2, each over 135135, [1, 69300, 6048, 64, 1, 249480, 50400, 1792] / 135135.
    pub hn: uint16x8_t,
    /// f16 bits by lane: [k/2048, 256, 0, ...].
    pub hk: uint16x8_t,
}

/// An app's 16-bit working values (gamma 2, white = Q14, to 4.0), eight of them, to light (f16 bits): x = k·v/16384, tanh(x) by Lambert's continued fraction, x·(135135 + 17325x² + 378x⁴ + x⁶) / (135135 + 62370x² + 3150x⁴ + 28x⁶), squared, ×2^14.
#[inline(always)]
pub unsafe fn light16(o: &Odt, v: uint16x8_t) -> uint16x8_t {
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

/// `light16` in f16, eight lanes an instruction and no divide (2026-11-12: the f32 form, its two halves and their divides, was most of the 12 cycles a pixel an app's whole-screen publish spent on its codes on the X3). u = x/2 = k·v/32768 from the convert's four fraction bits (v/16, so 4× white stays inside f16), tanh x = 2u·N(u²)/D(u²) with N and D Lambert's to 7th order over 135135 (every coefficient a normal f16), 1/D by the estimate and one Newton step, then (256·u·N/D)² = tanh²·2^14. Within a few f16 steps of the f32 form (`light_check`).
#[inline(always)]
pub unsafe fn light16h(o: &Odt, v: uint16x8_t) -> uint16x8_t {
    unsafe {
        let out: uint16x8_t;
        core::arch::asm!(
            "ucvtf {u:v}.8h, {v:v}.8h, #4",
            "fmul {u:v}.8h, {u:v}.8h, {ck:v}.h[0]",
            "fmul {y:v}.8h, {u:v}.8h, {u:v}.8h",
            "dup {n:v}.8h, {cn:v}.h[2]",
            "fmla {n:v}.8h, {y:v}.8h, {cn:v}.h[3]",
            "dup {t:v}.8h, {cn:v}.h[1]",
            "fmla {t:v}.8h, {y:v}.8h, {n:v}.8h",
            "dup {n:v}.8h, {cn:v}.h[0]",
            "fmla {n:v}.8h, {y:v}.8h, {t:v}.8h",
            "dup {d:v}.8h, {cn:v}.h[6]",
            "fmla {d:v}.8h, {y:v}.8h, {cn:v}.h[7]",
            "dup {t:v}.8h, {cn:v}.h[5]",
            "fmla {t:v}.8h, {y:v}.8h, {d:v}.8h",
            "dup {d:v}.8h, {cn:v}.h[4]",
            "fmla {d:v}.8h, {y:v}.8h, {t:v}.8h",
            "frecpe {t:v}.8h, {d:v}.8h",
            "frecps {d:v}.8h, {d:v}.8h, {t:v}.8h",
            "fmul {t:v}.8h, {t:v}.8h, {d:v}.8h",
            "fmul {n:v}.8h, {n:v}.8h, {u:v}.8h",
            "fmul {n:v}.8h, {n:v}.8h, {t:v}.8h",
            "fmul {n:v}.8h, {n:v}.8h, {ck:v}.h[1]",
            "fmul {o:v}.8h, {n:v}.8h, {n:v}.8h",
            v = in(vreg) v, cn = in(vreg_low16) o.hn, ck = in(vreg_low16) o.hk,
            u = out(vreg) _, y = out(vreg) _, n = out(vreg) _, t = out(vreg) _, d = out(vreg) _,
            o = out(vreg) out,
            options(pure, nomem, nostack),
        );
        out
    }
}

/// `light16h` against `light16` over every 16-bit value: the largest difference relative to the f32 form's light (×2^20) and the value it is at, and the largest difference in f16 steps (development: said once at start).
pub fn light_check(o: &Odt) -> (u32, u32, u32) {
    let (mut worst, mut at, mut steps) = (0u32, 0u32, 0u32);
    let f = |h: u16| -> f32 {
        let r: f32;
        unsafe { core::arch::asm!("fmov {t:h}, {h:w}", "fcvt {r:s}, {t:h}", h = in(reg) h as u32, t = out(vreg) _, r = out(vreg) r, options(pure, nomem, nostack)) };
        r
    };
    for base in (0..1u32 << 16).step_by(8) {
        let mut vs = [0u16; 8];
        for (i, e) in vs.iter_mut().enumerate() {
            *e = (base + i as u32) as u16;
        }
        let (a, b) = unsafe {
            let v = vld1q_u16(vs.as_ptr());
            let (mut a, mut b) = ([0u16; 8], [0u16; 8]);
            vst1q_u16(a.as_mut_ptr(), light16(o, v));
            vst1q_u16(b.as_mut_ptr(), light16h(o, v));
            (a, b)
        };
        for i in 0..8 {
            let (x, y) = (f(a[i]), f(b[i]));
            steps = steps.max((a[i] as i32 - b[i] as i32).unsigned_abs());
            // Relative to the larger of the light and 2^-10 of white (2^4 in these units): below that both are black on the panel.
            let rel = ((x - y).abs() / x.max(16.) * (1u32 << 20) as f32) as u32;
            if rel > worst {
                worst = rel;
                at = base + i as u32;
            }
        }
    }
    (worst, at, steps)
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

/// The old present's dither and pack (`sixteen_canvas`): three channels of 16 Q4 codes, dithered by `th` (one threshold per pixel, 0-63), packed B[31:22] G[21:12] R[11:2] A[1:0] (each code's two low bits zero, for the DQE's truncation) and stored as 16 words: the word's four bytes are four planes: (R << 4) | 3, (G << 6) | (R >> 4), G >> 2, B, written by one interleaving store.
#[inline(always)]
pub unsafe fn dither_pack(q: [uint16x8_t; 6], th: uint8x16_t, dst: *mut u32) {
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

/// The old present (until 2026-11-12, the bench's "before" and the GPU driver's reference): 16 working pixels (R, G, B, A in 16-bit lanes, white at Q14, to 4.0) thru the ODT with this row's thresholds into 16 words: the light by Lambert's tanh, the ODT, the code, the old dither, the pack. Over black, so alpha plays no part. Sixteen black pixels are 16 black words without the ODT.
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
        let (r0, g0, b0) = matrix8(o, light16h(o, p0.0), light16h(o, p0.1), light16h(o, p0.2));
        let (r1, g1, b1) = matrix8(o, light16h(o, p1.0), light16h(o, p1.1), light16h(o, p1.2));
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

/// Two pixels as an opaque layer at opacity `a` (16-bit coverage, 65535 whole): each colour lane c · a / 65535 (rounded as `under2` divides), the alpha lane a. A flight's page image (COMPOSITOR.md "The flight, prerendered"), made opaque first by composing it over the wallpaper or black, at the flight's progress.
#[inline(always)]
pub unsafe fn opaque_at2(p: *mut u64, a: u16) {
    unsafe {
        const COLOUR: [u16; 8] = [0xFFFF, 0xFFFF, 0xFFFF, 0, 0xFFFF, 0xFFFF, 0xFFFF, 0];
        let v = vld1q_u16(p as *const u16);
        let av = vdupq_n_u16(a);
        let lo = vmull_u16(vget_low_u16(v), vget_low_u16(av));
        let hi = vmull_high_u16(v, av);
        let q = vrshrn_high_n_u32::<16>(vrshrn_n_u32::<16>(vsraq_n_u32::<16>(lo, lo)), vsraq_n_u32::<16>(hi, hi));
        vst1q_u16(p as *mut u16, vbslq_u16(vld1q_u16(COLOUR.as_ptr()), q, av));
    }
}

/// Eight half-resolution codes a channel spread over sixteen columns, linearly: each even column its own code, each odd one the rounded mean of its code and the next (`next`, the following block's first codes, or these again at a row's end), so the half resolution shows no blocks.
#[inline(always)]
pub unsafe fn spread16(c: [uint16x8_t; 3], next: [uint16x8_t; 3]) -> [uint16x8_t; 6] {
    unsafe {
        let s = |a: uint16x8_t, n: uint16x8_t| {
            let odd = vrhaddq_u16(a, vextq_u16::<1>(a, n));
            (vzip1q_u16(a, odd), vzip2q_u16(a, odd))
        };
        let (r, g, b) = (s(c[0], next[0]), s(c[1], next[1]), s(c[2], next[2]));
        [r.0, r.1, g.0, g.1, b.0, b.1]
    }
}
