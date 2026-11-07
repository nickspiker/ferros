//! A person's default wallpaper: a Julia set drawn from a 32-byte seed, in the family of the owner's own wallpaper (a deep blue field, glowing periwinkle dendrites wound into spirals, small warm sparks where the filaments knot).
//! The picture is a pure function of the seed and the frame's size: the same seed gives the same pixels bit for bit on every device, and different seeds give clearly different pictures.
//!
//! What the seed chooses (`candidate`):
//! - c, the Julia constant, just outside the Mandelbrot set, where the Julia set is a lace of spiralling dendrites: either a small step out from the main cardioid's boundary, c = e^{iθ}/2 − e^{2iθ}/4 + ε·n̂ (n̂ the outward normal), or out from the period-2 bulb's, c = −1 + (1 + ε)·e^{iφ}/4, conjugated half the time.
//!   Inside the set the picture would carry filled blobs, and far outside it thins to dust; the ranges keep it in the lace.
//! - The view: a point of the Julia set (the β fixed point taken back through a seeded run of preimages), pulled a seeded way toward the set's centre, a seeded size and a seeded turn.
//! - The colours: one palette (the owner's) turned a seeded small angle about the grey axis, the warm sparks turned on their own, and the background's soft gradient in a seeded direction.
//!
//! Every candidate is looked at first at a sixteenth of the frame's size (`survey`): how much of the frame lies near the set, in the whole and in each third along its long side, and how much never escapes.
//! A candidate that leaves a third empty, covers too little or too much, or carries a filled blob is passed over, and the seed's next candidate (the same seed hashed with the attempt's number) is tried; after `ATTEMPTS` the best seen is kept.
//! So the result is always a filled composition, and still a function of the seed alone.
//!
//! Colour: each pixel's orbit gives its smooth escape count n and its distance d to the set (the exterior distance estimate |z| ln|z| / |dz/dz₀|).
//! d, measured in spans of the frame, draws the filaments and their halo, so their width is the same fraction of the frame on every display; log₂(1 + n), normalised to the survey's median and 99th percentile, shades the field and lights the hottest filaments white and the knots warm.
//! Every layer is a colour mixed over what is below it in linear light, by a weight that is a C∞ function of d and n (a Cauchy bump, an algebraic sigmoid); nothing is clamped.
//! Each colour is a VSF RGB colour, written as its working value (√light, white at Q14) and mixed as light.
//!
//! Output: the display's working values, a u64 per pixel, R, G and B in bits 0-15, 16-31 and 32-47 as √light × 16384, α G#FFFF in bits 48-63.
//! f32 and libm only: no fused multiply-add is ever formed (Rust never contracts), the square root is correctly rounded everywhere (aarch64's fsqrt, x86's sqrtss), and libm's logarithms are the same software on every target, so a row is the same wherever it is computed.
#![cfg_attr(not(test), no_std)]

use core::f32::consts::{PI, TAU};
use core::ops::Range;

/// The most iterations a pixel's orbit is followed.
pub const MAX_ITER: u32 = 1 << 9;
/// Bailout: an orbit has escaped once |z|² passes 2^16 (|z| > 256), far enough out that the smooth count and the distance estimate are accurate.
const R2: f32 = (1u32 << 16) as f32;

/// How many seeded candidates are surveyed before the best seen is kept.
pub const ATTEMPTS: u32 = 1 << 5;
/// The survey's scale: one preview pixel per 16 × 16 of the frame.
pub const PREVIEW: usize = 1 << 4;
/// The survey's histogram of log₂(1 + n).
const BINS: usize = 1 << 8;

/// The cardioid family: θ as turns of the boundary, and ε as a power of two.
const CARDIOID_T: [f32; 2] = [0.1, 0.48];
const CARDIOID_EPS_LOG2: [f32; 2] = [-6., -3.5];
/// The period-2 bulb family: φ as half turns, and ε as a fraction of the bulb's radius.
const BULB_PHI: [f32; 2] = [0.08, 0.55];
const BULB_EPS: [f32; 2] = [0.03, 0.2];
/// Preimages taken back from the β fixed point to find a point of the set.
const BACK_STEPS: u32 = 1 << 4;
/// How far the view's centre is pulled from that point toward the set's centre (0), and the view's size in the plane per span of the frame.
const PULL: [f32; 2] = [0.1, 0.7];
const VIEW_SPAN: [f32; 2] = [0.6, 1.4];
/// The seeded turn of the palette about the grey axis: from 7.5° toward cyan to 15° toward violet (a positive turn takes blue toward red), so every palette stays in the blue-violet family; and of the warm sparks on their own, 15° either way.
const HUE: [f32; 2] = [-PI / 24., PI / 12.];
const WARM_HUE: [f32; 2] = [-PI / 12., PI / 12.];

/// A candidate passes when the fraction of its frame near the set lies between these, each third holds at least `NEAR_THIRD`, and under `INSIDE` never escapes.
const NEAR: [f32; 2] = [0.25, 0.6];
const NEAR_THIRD: f32 = 1. / 8.;
const INSIDE: f32 = 1. / (1u32 << 6) as f32;
/// "Near the set": within 1/128 of a span.
const NEAR_SPAN: f32 = 1. / (1u32 << 7) as f32;

/// The palette, as working values (√light, white = 16384): the background's two ends, the field's shade, the halo, the filament, its white-hot core and the warm spark.
const BG: [[u16; 3]; 2] = [[2785, 2785, 6390], [3768, 3932, 8028]];
const SHADE: [u16; 3] = [5243, 5079, 8520];
const HALO: [u16; 3] = [4588, 5407, 11469];
const FIL: [u16; 3] = [5898, 7209, 14090];
const HOT: [u16; 3] = [10158, 11469, 14090];
const WARM: [u16; 3] = [13107, 11141, 6226];

/// How strongly each layer is laid on, where it sits and how sharply it turns on: the field's shade by the normalised count v, the halo and filament by distance (their widths as the inverse of a span fraction), and the hot and warm cores by v.
const SHADE_A: f32 = 5. / 8.;
const SHADE_V: f32 = 3. / 8.;
const SHADE_K: f32 = 3.;
const HALO_A: f32 = 1. / 2.;
const HALO_INV: f32 = (3u32 << 7) as f32;
const FIL_A: f32 = 3. / 4.;
const CORE_INV: f32 = (1u32 << 12) as f32;
const HOT_V: f32 = 1.;
const HOT_K: f32 = 4.;
const WARM_V: f32 = 9. / 8.;
const WARM_K: f32 = 8.;
/// The background's gradient: how far its ends sit apart along the seeded direction, in spans.
const GRAD_SPANS: f32 = 2.;

/// Everything a wallpaper is drawn from: the seed's choices, worked out once (`params`) and shared by every core that draws rows of it.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub width: usize,
    pub height: usize,
    /// The Julia constant.
    pub c: [f32; 2],
    /// The plane at the frame's centre.
    pub centre: [f32; 2],
    /// The plane's step per pixel along x: its size and turn (a step along y is this turned a quarter).
    pub step: [f32; 2],
    /// The background gradient's direction, scaled so a span is 1 / GRAD_SPANS.
    pub grad: [f32; 2],
    /// The colours in linear light: background ends, shade, halo, filament, hot, warm.
    pub light: [[f32; 3]; 7],
    /// log₂(1 + n) at the survey's median, and the inverse of its spread to the 99th percentile: v = (log₂(1 + n) − l_lo) · l_inv.
    pub l_lo: f32,
    pub l_inv: f32,
    /// Which attempt was kept, and what its survey found.
    pub attempt: u32,
    pub survey: Survey,
}

/// What a sixteenth-scale look at a candidate found.
#[derive(Clone, Copy, Debug, Default)]
pub struct Survey {
    /// The fraction of pixels near the set, overall and in the emptiest third along the frame's long side.
    pub near: f32,
    pub near_min_third: f32,
    /// The fraction of pixels that never escaped.
    pub inside: f32,
    /// log₂(1 + n) over the escaped pixels: its median and 99th percentile.
    pub p50: f32,
    pub p99: f32,
}

impl Survey {
    /// Whether the composition is good enough to keep.
    pub fn passes(&self) -> bool {
        self.near > NEAR[0] && self.near < NEAR[1] && self.near_min_third > NEAR_THIRD && self.inside < INSIDE
    }
    /// How close a failing candidate came, to keep the best when none passes.
    fn score(&self) -> f32 {
        self.near_min_third - self.inside - libm::fabsf(self.near - (NEAR[0] + NEAR[1]) * 0.5)
    }
}

/// A seed's wallpaper for a `width` × `height` frame.
pub fn params(seed: &[u8; 32], width: usize, height: usize) -> Params {
    let mut best = candidate(seed, 0, width, height);
    let mut best_score = f32::MIN;
    for a in 0..ATTEMPTS {
        let mut p = candidate(seed, a, width, height);
        let s = survey(&p);
        p.survey = s;
        if s.passes() {
            return tune(p);
        }
        if s.score() > best_score {
            best_score = s.score();
            best = p;
        }
    }
    tune(best)
}

/// Draws `rows` of a seed's wallpaper into `out`, which holds exactly those rows (`rows.len() × width` pixels, the first row first), so cores can each draw their own band.
/// Each call surveys the seed again; a caller splitting the frame over cores works out `params` once and calls `render_rows` on each.
pub fn render(seed: &[u8; 32], width: usize, height: usize, rows: Range<usize>, out: &mut [u64]) {
    render_rows(&params(seed, width, height), rows, out);
}

/// Draws `rows` of the wallpaper `p` into `out` (`rows.len() × width` pixels, the first row first).
pub fn render_rows(p: &Params, rows: Range<usize>, out: &mut [u64]) {
    // A mismatched buffer is the caller's bug: fail loud.
    assert_eq!(out.len(), rows.len() * p.width);
    let f = Frame::new(p);
    let w = p.width;
    for (k, y) in rows.enumerate() {
        let line = &mut out[k * w..k * w + w];
        f.row(y, line);
    }
}

/// Packs linear light into a working value: √light × 16384, opaque.
/// Every colour mixed is a convex combination of the palette's, whose channels are at most 1, so each channel's code is at most 16384 and fits its 16 bits.
#[inline]
pub fn pack(c: [f32; 3]) -> u64 {
    let q = |v: f32| (libm::sqrtf(v) * 16384.) as u64;
    q(c[0]) | q(c[1]) << 16 | q(c[2]) << 32 | 0xFFFF << 48
}

/// One pixel's orbit under z ↦ z² + c from (zx, zy): its smooth escape count, and its distance to the set in plane units.
/// The count runs continuously from i to i + 1 across the iteration where the orbit escapes, and an orbit that never escapes gets MAX_ITER + 1, the count's own limit, and distance 0.
/// dz/dz₀ grows by 2|z| ≤ 512 a step; once |dz|² passes 2^62 it is scaled down by 2^32 (exact, a power of two) and the scale kept in `e`, so it never overflows f32.
/// `orbit4` is the same orbit for four pixels side by side, operation for operation, so the two agree bit for bit (tested).
#[inline]
pub fn orbit(zx: f32, zy: f32, cx: f32, cy: f32) -> (f32, f32) {
    let (mut x, mut y) = (zx, zy);
    let (mut dx, mut dy) = (1f32, 0f32);
    let mut e = 0i32;
    let mut i = 0u32;
    loop {
        let (xx, yy) = (x * x, y * y);
        let r2 = xx + yy;
        if r2 > R2 {
            return escaped(i, r2, dx, dy, e);
        }
        if i == MAX_ITER {
            return ((MAX_ITER + 1) as f32, 0.);
        }
        let ndx = 2. * (x * dx - y * dy);
        dy = 2. * (x * dy + y * dx);
        dx = ndx;
        if dx * dx + dy * dy > DZ_BIG {
            dx *= DZ_SCALE;
            dy *= DZ_SCALE;
            e += 32;
        }
        y = 2. * x * y + cy;
        x = xx - yy + cx;
        i += 1;
    }
}

/// |dz|² past which dz is scaled down, and the scale (2^-32, exact).
const DZ_BIG: f32 = (1u64 << 62) as f32;
const DZ_SCALE: f32 = 1. / (1u64 << 32) as f32;

/// The smooth count and distance of an orbit that escaped after `i` steps with |z|² = r2 and dz/dz₀ = (dx, dy)·2^e.
#[inline]
fn escaped(i: u32, r2: f32, dx: f32, dy: f32, e: i32) -> (f32, f32) {
    // log₂|z|² > 16 here, so the count's log is of a value above 4 and the count lands in [i, i + 1).
    let l2 = libm::log2f(r2);
    let n = (i + 5) as f32 - libm::log2f(l2);
    let dz = libm::sqrtf(dx * dx + dy * dy);
    let d = libm::sqrtf(r2) * (0.5 * core::f32::consts::LN_2) * l2 / dz;
    (n, if e == 0 { d } else { libm::ldexpf(d, -e) })
}

/// Four orbits side by side (four pixels of a row): every lane takes the same steps as `orbit`, and a lane that has escaped keeps its state while the others run on, so each lane's result is `orbit`'s.
/// Four independent chains hide the multiply's latency, and the lanes map onto one NEON register.
#[cfg(not(all(target_arch = "aarch64", target_feature = "neon")))]
#[inline]
pub fn orbit4(zx: [f32; 4], zy: [f32; 4], cx: f32, cy: f32) -> [(f32, f32); 4] {
    let (mut x, mut y) = (zx, zy);
    let (mut dx, mut dy) = ([1f32; 4], [0f32; 4]);
    let mut e = [0u32; 4];
    let mut i = [0u32; 4];
    for _ in 0..MAX_ITER {
        let mut live = [false; 4];
        for j in 0..4 {
            live[j] = x[j] * x[j] + y[j] * y[j] <= R2;
        }
        if !(live[0] | live[1] | live[2] | live[3]) {
            break;
        }
        for j in 0..4 {
            let (xx, yy) = (x[j] * x[j], y[j] * y[j]);
            let mut ndx = 2. * (x[j] * dx[j] - y[j] * dy[j]);
            let mut ndy = 2. * (x[j] * dy[j] + y[j] * dx[j]);
            let big = ndx * ndx + ndy * ndy > DZ_BIG;
            // Multiplying by 1 is exact, so a lane that is not rescaled is unchanged, as in `orbit`.
            let f = if big { DZ_SCALE } else { 1. };
            ndx *= f;
            ndy *= f;
            let ny = 2. * x[j] * y[j] + cy;
            let nx = xx - yy + cx;
            let l = live[j];
            x[j] = if l { nx } else { x[j] };
            y[j] = if l { ny } else { y[j] };
            dx[j] = if l { ndx } else { dx[j] };
            dy[j] = if l { ndy } else { dy[j] };
            e[j] += (l & big) as u32 * 32;
            i[j] += l as u32;
        }
    }
    finish4(x, y, dx, dy, e, i)
}

/// `orbit4` on NEON: the same steps in one register per quantity, each operation the lane-wise IEEE twin of the scalar one (fmul, fadd, fsub, never a fused fmla; AArch64's vector floating point rounds exactly as its scalar does, under the same FPCR), so every lane is `orbit`'s result bit for bit.
/// LLVM does not vectorise the portable form (its data-dependent exit and per-lane selects), so the lanes are written out.
#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
#[inline]
pub fn orbit4(zx: [f32; 4], zy: [f32; 4], cx: f32, cy: f32) -> [(f32, f32); 4] {
    // SAFETY: the only requirement of a `target_feature` function is that the CPU has the feature, and this is compiled only where `neon` is part of the target itself (aarch64-unknown-none has it; every AArch64 core does).
    unsafe { orbit4_neon(zx, zy, cx, cy) }
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
#[target_feature(enable = "neon")]
fn orbit4_neon(zx: [f32; 4], zy: [f32; 4], cx: f32, cy: f32) -> [(f32, f32); 4] {
    use core::arch::aarch64::*;
    let lanes = |a: [f32; 4]| {
        let v = vdupq_n_f32(a[0]);
        let v = vsetq_lane_f32::<1>(a[1], v);
        let v = vsetq_lane_f32::<2>(a[2], v);
        vsetq_lane_f32::<3>(a[3], v)
    };
    let (mut x, mut y) = (lanes(zx), lanes(zy));
    let (cxv, cyv) = (vdupq_n_f32(cx), vdupq_n_f32(cy));
    let (two, one) = (vdupq_n_f32(2.), vdupq_n_f32(1.));
    let (r2max, big, scale) = (vdupq_n_f32(R2), vdupq_n_f32(DZ_BIG), vdupq_n_f32(DZ_SCALE));
    let (mut dx, mut dy) = (one, vdupq_n_f32(0.));
    let mut e = vdupq_n_u32(0);
    let mut i = vdupq_n_u32(0);
    for _ in 0..MAX_ITER {
        let xx = vmulq_f32(x, x);
        let yy = vmulq_f32(y, y);
        let live = vcleq_f32(vaddq_f32(xx, yy), r2max);
        if vmaxvq_u32(live) == 0 {
            break;
        }
        let ndx = vmulq_f32(two, vsubq_f32(vmulq_f32(x, dx), vmulq_f32(y, dy)));
        let ndy = vmulq_f32(two, vaddq_f32(vmulq_f32(x, dy), vmulq_f32(y, dx)));
        let bigm = vcgtq_f32(vaddq_f32(vmulq_f32(ndx, ndx), vmulq_f32(ndy, ndy)), big);
        let f = vbslq_f32(bigm, scale, one);
        let ny = vaddq_f32(vmulq_f32(vmulq_f32(two, x), y), cyv);
        let nx = vaddq_f32(vsubq_f32(xx, yy), cxv);
        x = vbslq_f32(live, nx, x);
        y = vbslq_f32(live, ny, y);
        dx = vbslq_f32(live, vmulq_f32(ndx, f), dx);
        dy = vbslq_f32(live, vmulq_f32(ndy, f), dy);
        e = vaddq_u32(e, vandq_u32(vandq_u32(bigm, live), vdupq_n_u32(32)));
        // A live lane's mask is all ones, −1 as an integer: subtracting it counts the step.
        i = vsubq_u32(i, live);
    }
    let get = |v: float32x4_t| [vgetq_lane_f32::<0>(v), vgetq_lane_f32::<1>(v), vgetq_lane_f32::<2>(v), vgetq_lane_f32::<3>(v)];
    let geti = |v: uint32x4_t| [vgetq_lane_u32::<0>(v), vgetq_lane_u32::<1>(v), vgetq_lane_u32::<2>(v), vgetq_lane_u32::<3>(v)];
    finish4(get(x), get(y), get(dx), get(dy), geti(e), geti(i))
}

/// Each lane's result from its final state, as `orbit` gives it.
#[inline]
fn finish4(x: [f32; 4], y: [f32; 4], dx: [f32; 4], dy: [f32; 4], e: [u32; 4], i: [u32; 4]) -> [(f32, f32); 4] {
    let mut out = [(0f32, 0f32); 4];
    for j in 0..4 {
        let r2 = x[j] * x[j] + y[j] * y[j];
        out[j] = if r2 > R2 { escaped(i[j], r2, dx[j], dy[j], e[j] as i32) } else { ((MAX_ITER + 1) as f32, 0.) };
    }
    out
}

/// A C∞ step from 0 to 1, the algebraic sigmoid: ½ + ½·x/√(1 + x²).
#[inline]
fn sig(x: f32) -> f32 {
    0.5 + 0.5 * x / libm::sqrtf(1. + x * x)
}

#[inline]
fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// A frame's constants, worked out once per call.
struct Frame<'a> {
    p: &'a Params,
    half: [f32; 2],
    /// From plane distance to spans of the frame.
    ds: f32,
}

impl<'a> Frame<'a> {
    fn new(p: &'a Params) -> Frame<'a> {
        let px = libm::sqrtf(p.step[0] * p.step[0] + p.step[1] * p.step[1]);
        Frame { p, half: [p.width as f32 * 0.5, p.height as f32 * 0.5], ds: 1. / (px * span(p.width, p.height)) }
    }

    /// The plane under pixel (x, y).
    #[inline]
    fn z(&self, x: usize, y: usize) -> (f32, f32) {
        let p = self.p;
        let fx = x as f32 + 0.5 - self.half[0];
        let fy = y as f32 + 0.5 - self.half[1];
        (p.centre[0] + fx * p.step[0] - fy * p.step[1], p.centre[1] + fx * p.step[1] + fy * p.step[0])
    }

    /// Row y's orbits, each handed to `f` with its column: four pixels at a time through `orbit4`, any left over one at a time through `orbit` (the same values).
    #[inline]
    fn each(&self, y: usize, mut f: impl FnMut(usize, f32, f32)) {
        let p = self.p;
        let quads = p.width / 4;
        for q in 0..quads {
            let x0 = q * 4;
            let mut zx = [0f32; 4];
            let mut zy = [0f32; 4];
            for j in 0..4 {
                (zx[j], zy[j]) = self.z(x0 + j, y);
            }
            let o = orbit4(zx, zy, p.c[0], p.c[1]);
            for j in 0..4 {
                f(x0 + j, o[j].0, o[j].1);
            }
        }
        for x in quads * 4..p.width {
            let (zx, zy) = self.z(x, y);
            let (n, d) = orbit(zx, zy, p.c[0], p.c[1]);
            f(x, n, d);
        }
    }

    /// Row y into `line`.
    fn row(&self, y: usize, line: &mut [u64]) {
        self.each(y, |x, n, d| line[x] = pack(self.colour(x, y, n, d)));
    }

    /// Pixel (x, y)'s linear light, from its orbit's count n and distance d.
    #[inline]
    fn colour(&self, x: usize, y: usize, n: f32, d: f32) -> [f32; 3] {
        let p = self.p;
        let fx = x as f32 + 0.5 - self.half[0];
        let fy = y as f32 + 0.5 - self.half[1];
        let [bg0, bg1, shade, halo, fil, hot, warm] = p.light;
        let mut col = lerp(bg0, bg1, sig(fx * p.grad[0] + fy * p.grad[1]));
        let v = (libm::log2f(1. + n) - p.l_lo) * p.l_inv;
        col = lerp(col, shade, SHADE_A * sig((v - SHADE_V) * SHADE_K));
        let ds = d * self.ds;
        let th = ds * HALO_INV;
        col = lerp(col, halo, HALO_A / (1. + th * th));
        let tc = ds * CORE_INV;
        let core = 1. / (1. + tc * tc);
        let core = core * core;
        col = lerp(col, fil, FIL_A * core);
        col = lerp(col, hot, core * sig((v - HOT_V) * HOT_K));
        lerp(col, warm, core * sig((v - WARM_V) * WARM_K))
    }
}

/// The frame's span: the harmonic mean of its sides, 2wh / (w + h).
fn span(w: usize, h: usize) -> f32 {
    2. * w as f32 * h as f32 / (w + h) as f32
}

/// SplitMix64's finaliser: the seed's bits mixed into every output bit (its wrapping multiplies are the hash, arithmetic mod 2^64 by definition).
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The seed's stream of choices for one attempt: all 32 bytes and the attempt's number folded into one state, then SplitMix64.
struct Rng(u64);

impl Rng {
    fn new(seed: &[u8; 32], attempt: u32) -> Rng {
        // "WALLPAPE" in ASCII: this stream is the wallpaper's own, whatever else the seed feeds.
        let mut s = mix(attempt as u64 ^ 0x5741_4C4C_5041_5045);
        for w in seed.as_chunks::<8>().0 {
            s = mix(s ^ u64::from_le_bytes(*w));
        }
        Rng(s)
    }
    fn next(&mut self) -> u64 {
        // The golden-ratio increment, mod 2^64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }
    /// Uniform in [0, 1), from the top 24 bits (exact in f32).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 * (1. / (1u32 << 24) as f32)
    }
    fn range(&mut self, r: [f32; 2]) -> f32 {
        r[0] + (r[1] - r[0]) * self.unit()
    }
}

/// The principal complex square root.
fn csqrt(x: f32, y: f32) -> (f32, f32) {
    let m = libm::sqrtf(x * x + y * y);
    let re = libm::sqrtf(0.5 * (m + x));
    let im = libm::sqrtf(0.5 * (m - x));
    (re, if y < 0. { -im } else { im })
}

/// Turns a working value by `a` about the grey axis (Rodrigues' rotation about (1, 1, 1)/√3) and returns its light.
/// A rotation keeps a colour's grey (its mean) and its distance from the grey axis; for every palette colour at 15° the channels stay within their mean ± that distance·√(2/3), which lies inside [0, 1] (the darkest, the background's dark end, keeps at least 1/16 of white in its working value), so no channel can go negative or past white.
fn turn(code: [u16; 3], a: f32) -> [f32; 3] {
    let v = [code[0] as f32 / 16384., code[1] as f32 / 16384., code[2] as f32 / 16384.];
    let (c, s) = (libm::cosf(a), libm::sinf(a));
    let k = 1. / libm::sqrtf(3.);
    let dot = (v[0] + v[1] + v[2]) * k;
    // k × v for k = (1, 1, 1)/√3.
    let cr = [k * (v[2] - v[1]), k * (v[0] - v[2]), k * (v[1] - v[0])];
    let mut out = [0f32; 3];
    for j in 0..3 {
        let t = v[j] * c + cr[j] * s + k * dot * (1. - c);
        out[j] = t * t;
    }
    out
}

/// The seed's candidate for one attempt: c, the view and the colours.
pub fn candidate(seed: &[u8; 32], attempt: u32, width: usize, height: usize) -> Params {
    let mut r = Rng::new(seed, attempt);
    let flip = if r.next() & 1 == 0 { 1. } else { -1. };
    let (cx, cy) = if r.next() & 1 == 0 {
        let th = r.range(CARDIOID_T) * TAU;
        let eps = libm::exp2f(r.range(CARDIOID_EPS_LOG2));
        let (ct, st) = (libm::cosf(th), libm::sinf(th));
        let (c2, s2) = (libm::cosf(2. * th), libm::sinf(2. * th));
        // The boundary point e^{iθ}/2 − e^{2iθ}/4, and its outward normal e^{iθ}(1 − e^{iθ}), never zero for θ inside (0, 1) turns.
        let (nx, ny) = (ct - c2, st - s2);
        let nm = libm::sqrtf(nx * nx + ny * ny);
        (0.5 * ct - 0.25 * c2 + eps * nx / nm, 0.5 * st - 0.25 * s2 + eps * ny / nm)
    } else {
        let phi = r.range(BULB_PHI) * PI;
        let rad = 0.25 * (1. + r.range(BULB_EPS));
        (-1. + rad * libm::cosf(phi), rad * libm::sinf(phi))
    };
    let cy = flip * cy;
    // The β fixed point ½ + √(¼ − c), repelling, then preimages ±√(z − c) with seeded signs: a point of the Julia set.
    let (sx, sy) = csqrt(0.25 - cx, -cy);
    let (mut zx, mut zy) = (0.5 + sx, sy);
    let bits = r.next();
    for k in 0..BACK_STEPS {
        let (a, b) = csqrt(zx - cx, zy - cy);
        let s = if (bits >> k) & 1 == 0 { 1. } else { -1. };
        zx = s * a;
        zy = s * b;
    }
    let pull = r.range(PULL);
    let sc = r.range(VIEW_SPAN) / span(width, height);
    let view_turn = r.unit() * TAU;
    let grad_turn = r.unit() * TAU;
    let hue = r.range(HUE);
    let warm_hue = r.range(WARM_HUE);
    let g = 1. / (GRAD_SPANS * span(width, height));
    Params {
        width,
        height,
        c: [cx, cy],
        centre: [zx * pull, zy * pull],
        step: [sc * libm::cosf(view_turn), sc * libm::sinf(view_turn)],
        grad: [g * libm::cosf(grad_turn), g * libm::sinf(grad_turn)],
        light: [turn(BG[0], hue), turn(BG[1], hue), turn(SHADE, hue), turn(HALO, hue), turn(FIL, hue), turn(HOT, hue), turn(WARM, warm_hue)],
        l_lo: 0.,
        l_inv: 1.,
        attempt,
        survey: Survey::default(),
    }
}

/// Fits the count's shading to what the survey found.
/// The 99th percentile is never below the median; the histogram's bin width is added to their spread because neither is known closer than a bin, which also keeps the spread above zero for a field whose counts all fall in one bin.
fn tune(mut p: Params) -> Params {
    let s = p.survey;
    p.l_lo = s.p50;
    p.l_inv = 1. / (s.p99 - s.p50 + libm::log2f((MAX_ITER + 2) as f32) / BINS as f32);
    p
}

/// Looks at a candidate at a sixteenth of its size.
pub fn survey(p: &Params) -> Survey {
    let (w, h) = (p.width / PREVIEW, p.height / PREVIEW);
    let mut q = *p;
    q.width = w;
    q.height = h;
    q.step = [p.step[0] * PREVIEW as f32, p.step[1] * PREVIEW as f32];
    let f = Frame::new(&q);
    let near_d = NEAR_SPAN / f.ds;
    let lmax = libm::log2f((MAX_ITER + 2) as f32);
    // An escaped count is below MAX_ITER + 1, so its bin is below BINS; the extra bin takes a value that rounds up to the top edge, so no index is ever out of range.
    let mut hist = [0u32; BINS + 1];
    let mut thirds = [0u32; 3];
    let (mut near, mut inside, mut outside) = (0u32, 0u32, 0u32);
    let long = if h >= w { h } else { w };
    for y in 0..h {
        f.each(y, |x, n, d| {
            if d < near_d {
                near += 1;
                thirds[if h >= w { y } else { x } * 3 / long] += 1;
            }
            if n > MAX_ITER as f32 {
                inside += 1;
            } else {
                outside += 1;
                hist[(libm::log2f(1. + n) / lmax * BINS as f32) as usize] += 1;
            }
        });
    }
    let total = (w * h) as f32;
    let pct = |frac: f32| -> f32 {
        let want = frac * outside as f32;
        let mut acc = 0f32;
        for (k, &c) in hist.iter().enumerate() {
            if acc + c as f32 >= want {
                // An empty bin is reached here only when nothing escaped (want = 0); it is passed through at its lower edge rather than dividing 0 by 0.
                let t = if c == 0 { 0. } else { (want - acc) / c as f32 };
                return (k as f32 + t) / BINS as f32 * lmax;
            }
            acc += c as f32;
        }
        lmax
    };
    Survey {
        near: near as f32 / total,
        near_min_third: thirds[0].min(thirds[1]).min(thirds[2]) as f32 * 3. / total,
        inside: inside as f32 / total,
        p50: pct(0.5),
        p99: pct(0.99),
    }
}

/// A seed for testing and for looking: `n` spread over 32 bytes by SplitMix64, so neighbouring numbers give unrelated seeds.
pub fn seed_of(n: u64) -> [u8; 32] {
    let mut r = Rng(n);
    let mut s = [0u8; 32];
    for k in 0..4 {
        s[k * 8..k * 8 + 8].copy_from_slice(&r.next().to_le_bytes());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `orbit4` (on this target's path) agrees with `orbit` bit for bit, across escaping, slow and interior points.
    #[test]
    fn lanes_match_scalar() {
        let cs = [(-0.835f32, -0.2321f32), (0.285, 0.01), (-0.12, 0.75), (-0.75, 0.11), (0.3773, 0.1933), (-1.2, 0.)];
        for &(cx, cy) in &cs {
            for k in 0..(1 << 12) {
                let mut zx = [0f32; 4];
                let mut zy = [0f32; 4];
                for j in 0..4 {
                    let t = (k * 4 + j) as f32;
                    zx[j] = -1.6 + 3.2 * libm::fmodf(t * 0.618_034, 1.);
                    zy[j] = -1.2 + 2.4 * libm::fmodf(t * 0.754_877_7, 1.);
                }
                let o = orbit4(zx, zy, cx, cy);
                for j in 0..4 {
                    let (n, d) = orbit(zx[j], zy[j], cx, cy);
                    assert_eq!((o[j].0.to_bits(), o[j].1.to_bits()), (n.to_bits(), d.to_bits()), "c {cx} {cy} z {} {}", zx[j], zy[j]);
                }
            }
        }
    }

    /// Every palette colour, turned to either end of its hue range, keeps every channel within black and white.
    #[test]
    fn turned_palette_in_range() {
        for code in [BG[0], BG[1], SHADE, HALO, FIL, HOT] {
            for a in [HUE[0], HUE[1], 0.] {
                for c in turn(code, a) {
                    assert!((0. ..=1.).contains(&c), "{code:?} at {a}: {c}");
                }
            }
        }
        for a in [WARM_HUE[0], WARM_HUE[1]] {
            for c in turn(WARM, a) {
                assert!((0. ..=1.).contains(&c));
            }
        }
    }

    fn hash(px: &[u64]) -> u64 {
        px.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &p| mix(h ^ p))
    }

    /// The same seed gives the same pixels, row bands give the same pixels as the whole, a different seed gives different ones, and every pixel is opaque with channels at most white.
    #[test]
    fn deterministic_and_banded() {
        let (w, h) = (84, 187);
        let p = params(&seed_of(7), w, h);
        let mut whole = [0u64; 84 * 187];
        render_rows(&p, 0..h, &mut whole);
        let mut again = [0u64; 84 * 187];
        render(&seed_of(7), w, h, 0..64, &mut again[..64 * w]);
        render_rows(&params(&seed_of(7), w, h), 64..h, &mut again[64 * w..]);
        assert_eq!(hash(&whole), hash(&again));
        let mut other = [0u64; 84 * 187];
        render(&seed_of(8), w, h, 0..h, &mut other);
        assert_ne!(hash(&whole), hash(&other));
        for &px in &whole {
            assert_eq!(px >> 48, 0xFFFF);
            for c in 0..3 {
                assert!((px >> (16 * c)) & 0xFFFF <= 16384);
            }
        }
    }

    /// The device's composition, for a handful of seeds: each passes the survey (the example `validate` runs many more, built for speed).
    #[test]
    fn seeds_compose() {
        for n in 0..8 {
            let p = params(&seed_of(n), 1344, 2992);
            assert!(p.survey.passes(), "seed {n}: {:?}", p.survey);
        }
    }
}
