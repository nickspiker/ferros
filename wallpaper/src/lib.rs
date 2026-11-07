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
//! The layers go front to back (AGENT.md "Compositing: Front to Back, `under` Only"): each a colour at a weight that is a C∞ function of d, n and the relief (a Cauchy bump, an algebraic sigmoid), taking its share of the light still uncovered; nothing is clamped. Behind the near set, the same set again zoomed out, turned and shifted, drawn as a soft dim glow, gives depth; a seeded lamp lights the set's relief (the potential's gradient), Lambert and a sheen, over a low ambient so the picture stays dark.
//! Each colour is a VSF RGB colour, held and mixed as its working code (√light, white at 1), as the display and fluor mix theirs: in light the mixes came out much lighter than they look they should (owner, 2026-11-07).
//!
//! Output: the display's working values, a u64 per pixel, R, G and B in bits 0-15, 16-31 and 32-47 as √light × 16384, α G#FFFF in bits 48-63.
//! f32 and libm only: no fused multiply-add is ever formed (Rust never contracts), the square root is correctly rounded everywhere (aarch64's fsqrt, x86's sqrtss), and libm's logarithms are the same software on every target, so a row is the same wherever it is computed.
#![cfg_attr(not(test), no_std)]

use core::f32::consts::{PI, TAU};
use core::ops::Range;
use vsf::colour::{LMS2PHOTOPIC, LMS2VSF_RGB, VSF_RGB2LMS};

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
/// The warm sparks turn with the palette (they stay its complement) and a seeded 15° either way on their own.
const WARM_HUE: [f32; 2] = [-PI / 12., PI / 12.];
/// The hue is drawn from its own stream of the seed (this number in place of an attempt's), so it is the same whichever attempt's composition is kept, and uniform round the whole circle.
const HUE_STREAM: u32 = u32::MAX;

/// A candidate passes when the fraction of its frame near the set lies between these, each third holds at least `NEAR_THIRD`, and under `INSIDE` never escapes.
const NEAR: [f32; 2] = [0.25, 0.6];
const NEAR_THIRD: f32 = 1. / 8.;
const INSIDE: f32 = 1. / (1u32 << 6) as f32;
/// "Near the set": within 1/128 of a span.
const NEAR_SPAN: f32 = 1. / (1u32 << 7) as f32;

/// The palette, as working values (√light, white = 16384): the background's two ends, the field's shade, the halo, the filament, its white-hot core and the warm spark.
const BG: [[u16; 3]; 2] = [[1638, 1638, 3932], [2294, 2392, 4915]];
const SHADE: [u16; 3] = [5243, 5079, 8520];
const HALO: [u16; 3] = [4588, 5407, 11469];
const FIL: [u16; 3] = [5898, 7209, 14090];
const HOT: [u16; 3] = [10158, 11469, 14090];
const WARM: [u16; 3] = [13107, 11141, 6226];

/// How strongly each layer is laid on, where it sits and how sharply it turns on: the field's shade by the normalised count v, the halo and filament by distance (their widths as the inverse of a span fraction), and the hot and warm cores by v.
const SHADE_A: f32 = 1. / 8.;
const SHADE_V: f32 = 3. / 8.;
const SHADE_K: f32 = 3.;
const RAYS_A: f32 = 1. / 2.;
const RAYS_K: f32 = 8.;
const RAYS_MID: f32 = 3. / 8.;
const GLOW_INV: f32 = (1u32 << 4) as f32;
const HALO_A: f32 = 5. / 8.;
const HALO_INV: f32 = (1u32 << 7) as f32;
const FIL_A: f32 = 3. / 4.;
const CORE_INV: f32 = (1u32 << 12) as f32;
const HOT_V: f32 = 1.;
const HOT_K: f32 = 4.;
const WARM_INV: f32 = (1u32 << 11) as f32;
const WARM_V: f32 = 9. / 8.;
const WARM_K: f32 = 8.;
/// The background's gradient: how far its ends sit apart along the seeded direction, in spans.
const GRAD_SPANS: f32 = 2.;

/// The far layer (owner, 2026-11-07: "more layers"): the same set seen again behind the flames, zoomed out by a seeded factor, offset and turned, drawn only as a wide soft glow (defocused), dim, in its own colour: depth behind the flower.
const FAR: [u16; 3] = [4588, 5100, 10650];
const FAR_ZOOM: [f32; 2] = [2., 4.];
const FAR_TURN: [f32; 2] = [PI / 8., PI / 2.];
/// How far the far view's centre sits from the near one's, in the far view's spans.
const FAR_SHIFT: f32 = 1. / 4.;
/// The far glow's width (the inverse of a span fraction, of the far view's own span) and how strongly it is laid on.
const FAR_INV: f32 = (1u32 << 2) as f32;
const FAR_A: f32 = 3. / 8.;
/// The far layer's hue sits a seeded way off the palette's, within this either side.
const FAR_HUE: [f32; 2] = [-PI / 4., PI / 4.];

/// The lighting (owner, 2026-11-07: "a little bit of lighting/shading"): a lamp from a seeded direction lights the set's relief (`Orbit::u`).
/// Lambert, smooth: (u·l + LAMP_H) / (1 + LAMP_H), from (LAMP_H − 1)/(LAMP_H + 1) facing away to 1 facing the lamp, a C∞ function of the normal (no max); the halo keeps AMBIENT of its light whatever the facing, so it stays dark rather than black.
const LAMP_H: f32 = 2.;
const AMBIENT: f32 = 1. / 4.;
/// A sheen where the relief faces the lamp: ((u·l + 1)/2)^16, the light's colour laid over the halo by this much at most.
const SHEEN_A: f32 = 1. / 2.;

/// The density gradient map (owner, 2026-11-07: "density gradient map, mix shit together"; "each one is very monochromatic"): STOPS colours from the field's dark to the filament's light, each its own hue, the seed walking the hue circle from stop to stop by a seeded step either way, so every wallpaper is several hues, not one. Every layer takes its colour from the map at a density of its own (where escape is slow, how near the set, the flames, the far layer), so the hues mix through the picture.
const STOPS: usize = 6;
const STOP_CODES: [[u16; 3]; STOPS] = [BG[0], BG[1], SHADE, FAR, HALO, FIL];
const HUE_STEP: [f32; 2] = [PI / 4., 2. * PI / 3.];
/// How sharply the map's stops hold their colour between neighbours: each stop's weight is a Cauchy bump 1/(1 + ((t − t_k)·GM_K)²), the weights normalised: a C∞ partition of unity, so the map's colour has no kink anywhere.
const GM_K: f32 = 8.;
/// Where each layer reads the map: the field across its dark end (by the background's gradient and the slow-escape shade), the far layer and the flames in the middle, the halo above them (drifting with escape speed), the filament at the bright end.
const FIELD_T: f32 = 1. / 4.;
const FAR_T: [f32; 2] = [3. / 8., 5. / 8.];
const RAYS_T: [f32; 2] = [1. / 4., 5. / 8.];
const HALO_T: [f32; 2] = [5. / 8., 7. / 8.];

/// The soft flame (tapuwae's, owner 2026-11-07: "more flames"): two waves along the flame's contours at FLAME_F times its own scale, fading in away from the set as (d²/(d² + FLAME_FADE²)) in pixels, so they read in the open field and never crowd near the set: a hue wave from its real part, moving where the field reads the map by FLAME_T either way (the map's neighbouring stops are other hues), and a lightness wave from its imaginary part that only darkens, by FLAME_L at most (owner: "pretty dark").
const FLAME_F: f32 = (1u32 << 2) as f32;
const FLAME_FADE: f32 = (1u32 << 2) as f32;
const FLAME_T: f32 = 3. / 16.;
const FLAME_L: f32 = 3. / 8.;

/// Saturation from the seed too (owner, 2026-11-07: "balanced around grey, saturation/hue dependent upon hash"): the palette's chroma times a seeded factor in this range, muted to rich, so over all people the colours average to grey.
const SAT: [f32; 2] = [1. / 2., 1.];

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
    /// The density gradient map's colours as working codes (√light), dark to light, each its own hue (`STOPS`); and the white-hot core's and the warm sparks'.
    pub stops: [[f32; 3]; STOPS],
    pub hot: [f32; 3],
    pub warm: [f32; 3],
    /// The far layer's view: the plane at the frame's centre and its step per pixel along x.
    pub far_centre: [f32; 2],
    pub far_step: [f32; 2],
    /// The lamp's direction in the plane (a unit vector).
    pub lamp: [f32; 2],
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

/// The refining pass's samples a side in a marked pixel (4 × 4, owner 2026-11-07).
pub const REFINE: usize = 1 << 2;
/// A pixel is marked for refining when any channel differs from one of its four neighbours' by more than this, in working value units (white is 16384): 1/64 of white.
const EDGE: u64 = 1 << 8;

/// The bytes a row of the refining mask takes: a bit a pixel.
pub fn mask_stride(width: usize) -> usize {
    (width + 7) / 8
}

/// Marks the pixels of `rows` that the refining pass samples again: those whose colour in the finished first pass (`frame`, the whole frame) differs from one of their four neighbours' by more than EDGE in any channel. A bit each into `mask`, which holds exactly these rows (`rows.len() × mask_stride(width)` bytes, first row first). It reads only the frame and writes only these rows' bits, so bands are marked side by side, and the marks are the same however the bands are shared out.
pub fn mark_rows(p: &Params, frame: &[u64], rows: Range<usize>, mask: &mut [u8]) {
    let (w, h, stride) = (p.width, p.height, mask_stride(p.width));
    // A mismatched buffer is the caller's bug: fail loud.
    assert_eq!(frame.len(), w * h);
    assert_eq!(mask.len(), rows.len() * stride);
    mask.fill(0);
    let differ = |a: u64, b: u64| (0..3).any(|c| ((a >> (16 * c)) & 0xFFFF).abs_diff((b >> (16 * c)) & 0xFFFF) > EDGE);
    for (k, y) in rows.enumerate() {
        for x in 0..w {
            let a = frame[y * w + x];
            // The neighbours that exist: a pixel on the frame's edge has fewer.
            let edge = (x > 0 && differ(a, frame[y * w + x - 1])) || (x + 1 < w && differ(a, frame[y * w + x + 1])) || (y > 0 && differ(a, frame[(y - 1) * w + x])) || (y + 1 < h && differ(a, frame[(y + 1) * w + x]));
            if edge {
                mask[k * stride + x / 8] |= 1 << (x % 8);
            }
        }
    }
}

/// The refining pass over `rows`: each pixel marked in `mask` (these rows' bits, as `mark_rows` made them) drawn again as the mean of REFINE × REFINE samples spread evenly across it, into `out` (these rows' pixels). The far layer is a soft glow, so it is sampled once at the pixel's centre. The mean is of working codes, as the layers are mixed.
pub fn refine_rows(p: &Params, rows: Range<usize>, mask: &[u8], out: &mut [u64]) {
    let (w, stride) = (p.width, mask_stride(p.width));
    assert_eq!(out.len(), rows.len() * w);
    assert_eq!(mask.len(), rows.len() * stride);
    let f = Frame::new(p);
    let far_ds = f.views[FAR_VIEW].2;
    let n = (REFINE * REFINE) as f32;
    for (k, y) in rows.enumerate() {
        for x in 0..w {
            if mask[k * stride + x / 8] >> (x % 8) & 1 == 0 {
                continue;
            }
            let (cx, cy) = (x as f32 + 0.5 - f.half[0], y as f32 + 0.5 - f.half[1]);
            let (zx, zy) = f.z_at(FAR_VIEW, cx, cy);
            let t = orbit(zx, zy, p.c[0], p.c[1]).d * far_ds * FAR_INV;
            let far = 1. / (1. + t * t);
            let mut sum = [0f32; 3];
            // A row of samples at a time through `orbit4` (REFINE is its four lanes).
            for j in 0..REFINE {
                let fy = y as f32 + (j as f32 + 0.5) / REFINE as f32 - f.half[1];
                let fxs: [f32; REFINE] = core::array::from_fn(|i| x as f32 + (i as f32 + 0.5) / REFINE as f32 - f.half[0]);
                let (mut zx, mut zy) = ([0f32; 4], [0f32; 4]);
                for i in 0..REFINE {
                    (zx[i], zy[i]) = f.z_at(NEAR_VIEW, fxs[i], fy);
                }
                let o = orbit4(zx, zy, p.c[0], p.c[1]);
                for i in 0..REFINE {
                    let c = f.colour(fxs[i], fy, o[i], far);
                    sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
                }
            }
            out[k * w + x] = pack([sum[0] / n, sum[1] / n, sum[2] / n]);
        }
    }
}

/// Packs a colour in working codes (√light, white at 1) into a working value: the code × 16384, opaque.
/// Every colour mixed is a convex combination of the palette's codes, at most 1, times factors at most 1 (the lamp, the flame's darkening), so each channel is at most 16384 and fits its 16 bits.
#[inline]
pub fn pack(c: [f32; 3]) -> u64 {
    let q = |v: f32| (v * 16384.) as u64;
    q(c[0]) | q(c[1]) << 16 | q(c[2]) << 32 | 0xFFFF << 48
}

/// What a pixel's orbit gives: its smooth escape count, its distance to the set in plane units, its stripe average (the field's rays), and the set's relief there: the unit direction of z / (dz/dz₀) at the escape, the potential's gradient, which lighting takes as the surface's normal (zero inside the set).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Orbit {
    pub n: f32,
    pub d: f32,
    pub s: f32,
    pub u: [f32; 2],
    /// The flame (mandelbrot-exploder's, as tapuwae colours it): (conj(fc) − fq)·ln(n)/2⁴, fq the orbit's mean direction weighted by step² and fc its mean 1/z weighted by 1/|z| (owner, 2026-11-07: "more flames"); zero inside the set.
    pub f: [f32; 2],
}

/// An orbit that never escaped: the count's own limit, on the set, no rays.
const INSIDE_ORBIT: Orbit = Orbit { n: (MAX_ITER + 1) as f32, d: 0., s: 0., u: [0., 0.], f: [0., 0.] };

/// One pixel's orbit under z ↦ z² + c from (zx, zy).
/// The count runs continuously from i to i + 1 across the iteration where the orbit escapes, and an orbit that never escapes gets MAX_ITER + 1, the count's own limit, and distance 0.
/// dz/dz₀ grows by 2|z| ≤ 512 a step; once |dz|² passes 2^62 it is scaled down by 2^32 (exact, a power of two) and the scale kept in `e`, so it never overflows f32.
/// The stripe average (Härkönen's): sin 4·arg z_k averaged over the orbit from z₁ on, blended across the escaping step by the count's fraction so it runs on continuously where the escape time steps; it draws rays out of the set through the field.
/// `orbit4` is the same orbit for four pixels side by side, operation for operation, so the two agree bit for bit (tested).
#[inline]
pub fn orbit(zx: f32, zy: f32, cx: f32, cy: f32) -> Orbit {
    let (mut x, mut y) = (zx, zy);
    let (mut dx, mut dy) = (1f32, 0f32);
    let mut e = 0i32;
    let mut sum = 0f32;
    let mut fl = Flame::default();
    let mut i = 0u32;
    loop {
        let (xx, yy) = (x * x, y * y);
        let r2 = xx + yy;
        if r2 > R2 {
            return escaped(i, x, y, dx, dy, e, sum, fl);
        }
        if i == MAX_ITER {
            return INSIDE_ORBIT;
        }
        if i > 0 {
            sum += stripe(x, y, xx, yy, r2);
        }
        fl = fl.step(x, y, r2, i);
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

/// The flame's sums over an orbit (tapuwae's, from mandelbrot-exploder): z's direction weighted by the step's number squared, and that weight; z̄ / |z|² weighted by 1/|z|, and that weight.
/// The z-flame's weight tapers to nothing as the orbit nears the bailout (`taper`), so the sums run on continuously where the escape step changes from pixel to pixel; without it the flames came out as blocky tiles (2026-11-07).
#[derive(Clone, Copy, Debug, Default)]
pub struct Flame {
    q: [f32; 2],
    qw: f32,
    c: [f32; 2],
    cw: f32,
}

/// Added to |z| under the flame's division: z = 0 exactly (a preimage of −c can land there) would divide by zero; anywhere |z| > 2^-76 it is below f32's resolution of |z| and changes nothing.
const FLAME_EPS: f32 = STRIPE_EPS;

/// The z-flame's taper at |z|² = r2 (tapuwae's, from mandelbrot-exploder): 1 while |z|² ≤ 4, then (1 − t²)³ with t = (ln ln |z|² − ln(2 ln 2)) / ln ln bail, which for this bailout (2^16) is (log₂ log₂ |z|² − 1) / 3, from 0 at |z|² = 4 to 1 at the bailout, where the weight reaches 0 with its slope.
/// Only the last few steps of an orbit pass |z|² = 4, so its two logarithms are rare; `orbit4_neon` takes it lane by lane from this same function.
#[inline]
pub fn taper(r2: f32) -> f32 {
    if r2 <= 4. {
        return 1.;
    }
    let t = (libm::log2f(libm::log2f(r2)) - 1.) * (1. / 3.);
    let u = 1. - t * t;
    u * u * u
}

impl Flame {
    /// One step at z = (x, y), |z|² = r2, the `i`-th (from 0). `orbit4`'s lanes take these operations in this order, so every lane's sums are `orbit`'s.
    #[inline]
    fn step(self, x: f32, y: f32, r2: f32, i: u32) -> Flame {
        let inv = 1. / (libm::sqrtf(r2) + FLAME_EPS);
        let inv2 = inv * inv;
        let it = (i + 1) as f32;
        let wt = it * it * taper(r2);
        let wq = wt * inv;
        Flame { q: [self.q[0] + x * wq, self.q[1] + y * wq], qw: self.qw + wt, c: [self.c[0] + x * inv2, self.c[1] - y * inv2], cw: self.cw + inv }
    }
    /// The flame at the escape, after `n` (the smooth count): (conj(fc) − fq)·ln(n)/2⁴, as tapuwae has it.
    #[inline]
    fn value(self, n: f32) -> [f32; 2] {
        // An orbit escapes only after at least one step from the frame's view (it starts within a few units of the set, |z| < 256), so both weights are above zero.
        let (fq, fc) = ([self.q[0] / self.qw, self.q[1] / self.qw], [self.c[0] / self.cw, self.c[1] / self.cw]);
        let g = libm::logf(n) * (1. / (1 << 4) as f32);
        [(fc[0] - fq[0]) * g, (-fc[1] - fq[1]) * g]
    }
}

/// |dz|² past which dz is scaled down, and the scale (2^-32, exact).
const DZ_BIG: f32 = (1u64 << 62) as f32;
const DZ_SCALE: f32 = 1. / (1u64 << 32) as f32;
/// Added to |z|⁴ under the stripe's division: z = 0 exactly (a preimage of −c can land there) would otherwise make it 0/0, a NaN that colours the pixel; with it the stripe there is 0, and anywhere |z| > 2^-24 the term is below f32's resolution of |z|⁴, so it changes nothing else.
const STRIPE_EPS: f32 = 1. / (1u128 << 100) as f32;

/// sin 4·arg z = Im z⁴ / |z|⁴, without a trigonometric function.
#[inline]
fn stripe(x: f32, y: f32, xx: f32, yy: f32, r2: f32) -> f32 {
    4. * x * y * (xx - yy) / (r2 * r2 + STRIPE_EPS)
}

/// The orbit that escaped after `i` steps at z = (x, y), with dz/dz₀ = (dx, dy)·2^e and the stripe sum over z₁ … z_{i−1}.
#[inline]
fn escaped(i: u32, x: f32, y: f32, dx: f32, dy: f32, e: i32, sum: f32, fl: Flame) -> Orbit {
    let (xx, yy) = (x * x, y * y);
    let r2 = xx + yy;
    // log₂|z|² > 16 here, so the count's log is of a value above 4 and its fraction 5 − log₂ log₂|z|² lies in [0, 1).
    let l2 = libm::log2f(r2);
    let frac = 5. - libm::log2f(l2);
    let n = i as f32 + frac;
    let dz = libm::sqrtf(dx * dx + dy * dy);
    let d = libm::sqrtf(r2) * (0.5 * core::f32::consts::LN_2) * l2 / dz;
    // The average with this step's term and without it, each over a prior of two zero terms (so neither divides by zero): an orbit that escapes just past the bailout (fraction near 1) takes the first, one that escaped a step later from far out (fraction near 0) the second, and the two meet.
    let all = sum + stripe(x, y, xx, yy, r2);
    let with = all / (i + 2) as f32;
    let without = sum / (i + 1) as f32;
    // z / dz's direction, z·conj(dz) normalised (the scale 2^e does not turn it); both are far from zero here (|z| > 256, and dz has grown every step), so the length is never zero.
    let (ux, uy) = (x * dx + y * dy, y * dx - x * dy);
    let um = libm::sqrtf(ux * ux + uy * uy);
    Orbit { n, d: if e == 0 { d } else { libm::ldexpf(d, -e) }, s: without + (with - without) * frac, u: [ux / um, uy / um], f: fl.value(n) }
}

/// Four orbits side by side (four pixels of a row): every lane takes the same steps as `orbit`, and a lane that has escaped keeps its state while the others run on, so each lane's result is `orbit`'s.
/// Four independent chains hide the multiply's latency, and the lanes map onto one NEON register.
#[cfg(not(all(target_arch = "aarch64", target_feature = "neon")))]
#[inline]
pub fn orbit4(zx: [f32; 4], zy: [f32; 4], cx: f32, cy: f32) -> [Orbit; 4] {
    let (mut x, mut y) = (zx, zy);
    let (mut dx, mut dy) = ([1f32; 4], [0f32; 4]);
    let mut e = [0u32; 4];
    let mut fl = [Flame::default(); 4];
    let mut sum = [0f32; 4];
    let mut i = [0u32; 4];
    for k in 0..MAX_ITER {
        let mut live = [false; 4];
        for j in 0..4 {
            live[j] = x[j] * x[j] + y[j] * y[j] <= R2;
        }
        if !(live[0] | live[1] | live[2] | live[3]) {
            break;
        }
        for j in 0..4 {
            let (xx, yy) = (x[j] * x[j], y[j] * y[j]);
            let t = stripe(x[j], y[j], xx, yy, xx + yy);
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
            // A live lane's step count is k, so `k > 0` is `orbit`'s `i > 0`.
            sum[j] = if l && k > 0 { sum[j] + t } else { sum[j] };
            // A live lane's step number is k, as `orbit`'s i.
            fl[j] = if l { fl[j].step(x[j], y[j], xx + yy, k) } else { fl[j] };
            x[j] = if l { nx } else { x[j] };
            y[j] = if l { ny } else { y[j] };
            dx[j] = if l { ndx } else { dx[j] };
            dy[j] = if l { ndy } else { dy[j] };
            e[j] += (l & big) as u32 * 32;
            i[j] += l as u32;
        }
    }
    finish4(x, y, dx, dy, e, sum, i, fl)
}

/// `orbit4` on NEON: the same steps in one register per quantity, each operation the lane-wise IEEE twin of the scalar one (fmul, fadd, fsub, fdiv, never a fused fmla; AArch64's vector floating point rounds exactly as its scalar does, under the same FPCR), so every lane is `orbit`'s result bit for bit.
/// LLVM does not vectorise the portable form (its data-dependent exit and per-lane selects), so the lanes are written out.
#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
#[inline]
pub fn orbit4(zx: [f32; 4], zy: [f32; 4], cx: f32, cy: f32) -> [Orbit; 4] {
    // SAFETY: the only requirement of a `target_feature` function is that the CPU has the feature, and this is compiled only where `neon` is part of the target itself (aarch64-unknown-none has it; every AArch64 core does).
    unsafe { orbit4_neon(zx, zy, cx, cy) }
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
#[target_feature(enable = "neon")]
fn orbit4_neon(zx: [f32; 4], zy: [f32; 4], cx: f32, cy: f32) -> [Orbit; 4] {
    use core::arch::aarch64::*;
    let lanes = |a: [f32; 4]| {
        let v = vdupq_n_f32(a[0]);
        let v = vsetq_lane_f32::<1>(a[1], v);
        let v = vsetq_lane_f32::<2>(a[2], v);
        vsetq_lane_f32::<3>(a[3], v)
    };
    let get = |v: float32x4_t| [vgetq_lane_f32::<0>(v), vgetq_lane_f32::<1>(v), vgetq_lane_f32::<2>(v), vgetq_lane_f32::<3>(v)];
    let (mut x, mut y) = (lanes(zx), lanes(zy));
    let (cxv, cyv) = (vdupq_n_f32(cx), vdupq_n_f32(cy));
    let (two, four, one) = (vdupq_n_f32(2.), vdupq_n_f32(4.), vdupq_n_f32(1.));
    let (r2max, big, scale, eps) = (vdupq_n_f32(R2), vdupq_n_f32(DZ_BIG), vdupq_n_f32(DZ_SCALE), vdupq_n_f32(STRIPE_EPS));
    let (mut dx, mut dy) = (one, vdupq_n_f32(0.));
    let mut sum = vdupq_n_f32(0.);
    let mut e = vdupq_n_u32(0);
    let mut i = vdupq_n_u32(0);
    // The flame's sums (`Flame`), lane by lane.
    let epsv = vdupq_n_f32(FLAME_EPS);
    let (mut q0, mut q1, mut qw) = (vdupq_n_f32(0.), vdupq_n_f32(0.), vdupq_n_f32(0.));
    let (mut c0, mut c1, mut cw) = (vdupq_n_f32(0.), vdupq_n_f32(0.), vdupq_n_f32(0.));
    for k in 0..MAX_ITER {
        let xx = vmulq_f32(x, x);
        let yy = vmulq_f32(y, y);
        let r2 = vaddq_f32(xx, yy);
        let live = vcleq_f32(r2, r2max);
        if vmaxvq_u32(live) == 0 {
            break;
        }
        if k > 0 {
            let t = vdivq_f32(vmulq_f32(vmulq_f32(vmulq_f32(four, x), y), vsubq_f32(xx, yy)), vaddq_f32(vmulq_f32(r2, r2), eps));
            sum = vbslq_f32(live, vaddq_f32(sum, t), sum);
        }
        // `Flame::step`, operation for operation: a live lane's step number is k.
        let inv = vdivq_f32(one, vaddq_f32(vsqrtq_f32(r2), epsv));
        let inv2 = vmulq_f32(inv, inv);
        let it = vdupq_n_f32((k + 1) as f32);
        // The taper lane by lane from `taper` itself, and only on a step where some lane is past |z|² = 4 (it is 1 everywhere else).
        let tp = if vmaxvq_u32(vcgtq_f32(r2, four)) == 0 {
            one
        } else {
            let r = get(r2);
            lanes([taper(r[0]), taper(r[1]), taper(r[2]), taper(r[3])])
        };
        let wt = vmulq_f32(vmulq_f32(it, it), tp);
        let wq = vmulq_f32(wt, inv);
        q0 = vbslq_f32(live, vaddq_f32(q0, vmulq_f32(x, wq)), q0);
        q1 = vbslq_f32(live, vaddq_f32(q1, vmulq_f32(y, wq)), q1);
        qw = vbslq_f32(live, vaddq_f32(qw, wt), qw);
        c0 = vbslq_f32(live, vaddq_f32(c0, vmulq_f32(x, inv2)), c0);
        c1 = vbslq_f32(live, vsubq_f32(c1, vmulq_f32(y, inv2)), c1);
        cw = vbslq_f32(live, vaddq_f32(cw, inv), cw);
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
    let geti = |v: uint32x4_t| [vgetq_lane_u32::<0>(v), vgetq_lane_u32::<1>(v), vgetq_lane_u32::<2>(v), vgetq_lane_u32::<3>(v)];
    let (q0, q1, qw, c0, c1, cw) = (get(q0), get(q1), get(qw), get(c0), get(c1), get(cw));
    let fl = core::array::from_fn(|j| Flame { q: [q0[j], q1[j]], qw: qw[j], c: [c0[j], c1[j]], cw: cw[j] });
    finish4(get(x), get(y), get(dx), get(dy), geti(e), get(sum), geti(i), fl)
}

/// Each lane's result from its final state, as `orbit` gives it.
#[inline]
fn finish4(x: [f32; 4], y: [f32; 4], dx: [f32; 4], dy: [f32; 4], e: [u32; 4], sum: [f32; 4], i: [u32; 4], fl: [Flame; 4]) -> [Orbit; 4] {
    let mut out = [INSIDE_ORBIT; 4];
    for j in 0..4 {
        let r2 = x[j] * x[j] + y[j] * y[j];
        if r2 > R2 {
            out[j] = escaped(i[j], x[j], y[j], dx[j], dy[j], e[j] as i32, sum[j], fl[j]);
        }
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
    /// The frame's span in pixels (for the flame's fade, in pixels from the set).
    span: f32,
    /// The near view and the far one: each its plane at the frame's centre, its step per pixel along x, and the factor from plane distance to spans of the frame.
    views: [([f32; 2], [f32; 2], f32); 2],
}

/// Which of the frame's views an orbit is taken in.
const NEAR_VIEW: usize = 0;
const FAR_VIEW: usize = 1;

impl<'a> Frame<'a> {
    fn new(p: &'a Params) -> Frame<'a> {
        let ds = |s: [f32; 2]| 1. / (libm::sqrtf(s[0] * s[0] + s[1] * s[1]) * span(p.width, p.height));
        Frame { p, half: [p.width as f32 * 0.5, p.height as f32 * 0.5], span: span(p.width, p.height), views: [(p.centre, p.step, ds(p.step)), (p.far_centre, p.far_step, ds(p.far_step))] }
    }

    /// The plane under pixel (x, y) in view `v`.
    #[inline]
    fn z(&self, v: usize, x: usize, y: usize) -> (f32, f32) {
        self.z_at(v, x as f32 + 0.5 - self.half[0], y as f32 + 0.5 - self.half[1])
    }

    /// The plane in view `v` at (fx, fy) pixels from the frame's centre (any fraction: the refining pass samples inside a pixel).
    #[inline]
    fn z_at(&self, v: usize, fx: f32, fy: f32) -> (f32, f32) {
        let (centre, step, _) = self.views[v];
        (centre[0] + fx * step[0] - fy * step[1], centre[1] + fx * step[1] + fy * step[0])
    }

    /// Row y's orbits in view `v`, each handed to `f` with its column: four pixels at a time through `orbit4`, any left over one at a time through `orbit` (the same values).
    #[inline]
    fn each(&self, v: usize, y: usize, mut f: impl FnMut(usize, Orbit)) {
        let p = self.p;
        let quads = p.width / 4;
        for q in 0..quads {
            let x0 = q * 4;
            let mut zx = [0f32; 4];
            let mut zy = [0f32; 4];
            for j in 0..4 {
                (zx[j], zy[j]) = self.z(v, x0 + j, y);
            }
            let o = orbit4(zx, zy, p.c[0], p.c[1]);
            for j in 0..4 {
                f(x0 + j, o[j]);
            }
        }
        for x in quads * 4..p.width {
            let (zx, zy) = self.z(v, x, y);
            f(x, orbit(zx, zy, p.c[0], p.c[1]));
        }
    }

    /// Row y into `line`, in two passes over the row: the far layer's weight first, kept in each pixel's slot (its f32's bits) until the near pass reads it as it colours the pixel.
    fn row(&self, y: usize, line: &mut [u64]) {
        let far_ds = self.views[FAR_VIEW].2;
        self.each(FAR_VIEW, y, |x, o| {
            let t = o.d * far_ds * FAR_INV;
            line[x] = (1. / (1. + t * t)).to_bits() as u64;
        });
        self.each(NEAR_VIEW, y, |x, o| line[x] = pack(self.colour(x as f32 + 0.5 - self.half[0], y as f32 + 0.5 - self.half[1], o, f32::from_bits(line[x] as u32))));
    }

    /// Pixel (x, y)'s linear light, from its near orbit and the far layer's weight there.
    /// Front to back (owner's rule, AGENT.md "Compositing: Front to Back, `under` Only"): each layer a colour at its weight α, taking its share of the light that is still uncovered (`t`), which it then covers by α; the frontmost goes first: the warm sparks, the white-hot core, the filament, the sheen, the halo, the flame's glow, the far layer, the field's shade, and the background last, opaque, so every pixel's light is a convex sum of the palette's.
    #[inline]
    fn colour(&self, fx: f32, fy: f32, o: Orbit, far: f32) -> [f32; 3] {
        let p = self.p;
        let v = (libm::log2f(1. + o.n) - p.l_lo) * p.l_inv;
        let ds = o.d * self.views[NEAR_VIEW].2;
        let map = |t: f32| gmap(&p.stops, t);
        // The lamp on the relief: Lambert, smooth, and the sheen where the relief faces it.
        let l = o.u[0] * p.lamp[0] + o.u[1] * p.lamp[1];
        let lam = (l + LAMP_H) * (1. / (1. + LAMP_H));
        let s = (l + 1.) * 0.5;
        let (s2, s4) = (s * s, s * s * s * s);
        let sheen = s4 * s4 * s2 * s2 * s4;
        let lit = |c: [f32; 3], k: f32| [c[0] * k, c[1] * k, c[2] * k];
        let mut acc = [0f32; 3];
        let mut t = 1f32;
        let mut put = |c: [f32; 3], a: f32| {
            let w = a * t;
            acc = [acc[0] + c[0] * w, acc[1] + c[1] * w, acc[2] + c[2] * w];
            t *= 1. - a;
        };
        let slow = sig((v - SHADE_V) * SHADE_K);
        let tw = ds * WARM_INV;
        put(p.warm, sig((v - WARM_V) * WARM_K) / (1. + tw * tw));
        let tc = ds * CORE_INV;
        let core = 1. / (1. + tc * tc);
        let core = core * core;
        put(p.hot, core * sig((v - HOT_V) * HOT_K));
        put(lit(map(1.), lam), FIL_A * core);
        let th = ds * HALO_INV;
        let near_halo = 1. / (1. + th * th);
        put(p.hot, SHEEN_A * sheen * near_halo);
        put(lit(map(HALO_T[0] + (HALO_T[1] - HALO_T[0]) * slow), AMBIENT + (1. - AMBIENT) * lam), HALO_A * near_halo);
        let tg = ds * GLOW_INV;
        let rays = sig((o.s - RAYS_MID) * RAYS_K);
        // The flame's glow is not lit: far from the set the potential's gradient spins round its critical points, and lit it drew small dark stars in the open field.
        put(map(RAYS_T[0] + (RAYS_T[1] - RAYS_T[0]) * rays), RAYS_A * rays / (1. + tg * tg));
        put(map(FAR_T[0] + (FAR_T[1] - FAR_T[0]) * far), FAR_A * far);
        // The field, opaque, last: the map's dark end, by the background's seeded gradient and where escape is slow.
        // The soft flame on the field: its hue wave moves where the field reads the map, its lightness wave darkens it, both fading in away from the set.
        let px = ds * self.span;
        let fade = px * px / (px * px + FLAME_FADE * FLAME_FADE);
        let hue_wave = libm::sinf(o.f[0] * FLAME_F * TAU) * FLAME_T * fade;
        let dark = 1. - FLAME_L * (0.5 + 0.5 * libm::sinf(o.f[1] * FLAME_F * TAU)) * fade;
        let pos = sig(fx * p.grad[0] + fy * p.grad[1]);
        put(lit(map(FIELD_T * (pos + SHADE_A * slow) * (1. / (1. + SHADE_A)) + hue_wave), dark), 1.);
        acc
    }
}

/// Light to working codes, √light a channel: the wallpaper mixes its layers in codes, as the display, fluor and tapuwae all do (owner, 2026-11-07: "we should be doing it in squared space, rather than linear to give it a dark appearance"); in light, a dark field half covered by a bright halo came out at half the bright light, much lighter than it looks it should.
fn codes(c: [f32; 3]) -> [f32; 3] {
    [libm::sqrtf(c[0]), libm::sqrtf(c[1]), libm::sqrtf(c[2])]
}

/// The density gradient map at `t` (0 the dark end, 1 the light): each stop weighted by a Cauchy bump about its place, the weights normalised, so the colour is a convex sum of the stops' and C∞ in t.
#[inline]
fn gmap(stops: &[[f32; 3]; STOPS], t: f32) -> [f32; 3] {
    let mut c = [0f32; 3];
    let mut sum = 0f32;
    for (k, s) in stops.iter().enumerate() {
        let d = (t - k as f32 * (1. / (STOPS - 1) as f32)) * GM_K;
        let w = 1. / (1. + d * d);
        c = [c[0] + s[0] * w, c[1] + s[1] * w, c[2] + s[2] * w];
        sum += w;
    }
    [c[0] / sum, c[1] / sum, c[2] / sum]
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

/// The opponent space colours are turned in: q = √(l, m, s) (VSF RGB through vsf's cone matrix, white at (1, 1, 1)), and from it a lightness J (the cones' photopic shares, vsf's LMS2PHOTOPIC) and two opponent axes, red-green and yellow-blue (IPT's, Ebner and Fairchild 1998, where equal angles look about equally far apart in hue).
/// A turn moves only the opponent pair, so J, the colour's lightness, is the same at every hue; white and every grey sit on the axis and never move.
const OPP: [[f32; 3]; 3] = [[LMS2PHOTOPIC[0], LMS2PHOTOPIC[1], LMS2PHOTOPIC[2]], [4.455, -4.851, 0.396], [0.8056, 0.3572, -1.1628]];
/// Chroma kept through the turn: the palette's own chroma times this stays inside VSF RGB at every hue (the darkest, the background, has the least room; `palette_in_gamut_all_round` checks 4096 hues).
const CHROMA: f32 = 7. / 8.;

/// Column-major 3×3 (vsf's layout: a column per input) times a vector.
fn cols(m: &[f32; 9], v: [f32; 3]) -> [f32; 3] {
    [m[0] * v[0] + m[3] * v[1] + m[6] * v[2], m[1] * v[0] + m[4] * v[1] + m[7] * v[2], m[2] * v[0] + m[5] * v[1] + m[8] * v[2]]
}

/// Row-major 3×3 times a vector.
fn rows(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
}

/// The inverse of a row-major 3×3 by cofactors (OPP's determinant is far from zero).
fn inverse(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let k = 1. / det;
    [[(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k], [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k], [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k]]
}

/// A working value turned `a` round the hue circle at its own lightness, its chroma times CHROMA and the seed's saturation `sat` (at most 1), returned as light.
fn turn(code: [u16; 3], a: f32, sat: f32) -> [f32; 3] {
    turned(code, a, CHROMA * sat).1
}

/// `turn` at chroma factor `chroma`, with the turned colour's q as well (the gamut is checked at the most chroma a seed can give, CHROMA; less only moves a colour toward its grey): both must stay in range (q ≥ 0, else squaring it would fold a negative cone response back positive; light within [0, 1]), which the tests check all round the circle.
fn turned(code: [u16; 3], a: f32, chroma: f32) -> ([f32; 3], [f32; 3]) {
    let v = [code[0] as f32 / 16384., code[1] as f32 / 16384., code[2] as f32 / 16384.];
    let lms = cols(&VSF_RGB2LMS, [v[0] * v[0], v[1] * v[1], v[2] * v[2]]);
    let o = rows(&OPP, [libm::sqrtf(lms[0]), libm::sqrtf(lms[1]), libm::sqrtf(lms[2])]);
    let (c, s) = (libm::cosf(a) * chroma, libm::sinf(a) * chroma);
    let q = rows(&inverse(&OPP), [o[0], o[1] * c - o[2] * s, o[1] * s + o[2] * c]);
    (q, cols(&LMS2VSF_RGB, [q[0] * q[0], q[1] * q[1], q[2] * q[2]]))
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
    // The far layer: zoomed out, turned and shifted from the near view, all seeded.
    let far_sc = sc * r.range(FAR_ZOOM);
    let far_turn = view_turn + r.range(FAR_TURN) * if r.next() & 1 == 0 { 1. } else { -1. };
    let shift_turn = r.unit() * TAU;
    let shift = FAR_SHIFT * far_sc * span(width, height);
    let lamp_turn = r.unit() * TAU;
    let mut hr = Rng::new(seed, HUE_STREAM);
    let hue = hr.unit() * TAU;
    let warm_hue = hue + hr.range(WARM_HUE);
    let far_hue = hue + hr.range(FAR_HUE);
    let sat = hr.range(SAT);
    // The map's hues: a walk round the circle from the seed's hue, a seeded step either way at each stop.
    let mut stops = [[0f32; 3]; STOPS];
    let mut h = hue;
    for k in 0..STOPS {
        stops[k] = codes(turn(STOP_CODES[k], h, sat));
        let s = hr.range(HUE_STEP);
        h += if hr.next() & 1 == 0 { s } else { -s };
    }
    let g = 1. / (GRAD_SPANS * span(width, height));
    Params {
        width,
        height,
        c: [cx, cy],
        centre: [zx * pull, zy * pull],
        step: [sc * libm::cosf(view_turn), sc * libm::sinf(view_turn)],
        grad: [g * libm::cosf(grad_turn), g * libm::sinf(grad_turn)],
        stops,
        hot: codes(turn(HOT, far_hue, sat)),
        warm: codes(turn(WARM, warm_hue, sat)),
        far_centre: [zx * pull + shift * libm::cosf(shift_turn), zy * pull + shift * libm::sinf(shift_turn)],
        far_step: [far_sc * libm::cosf(far_turn), far_sc * libm::sinf(far_turn)],
        lamp: [libm::cosf(lamp_turn), libm::sinf(lamp_turn)],
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
    q.far_step = [p.far_step[0] * PREVIEW as f32, p.far_step[1] * PREVIEW as f32];
    let f = Frame::new(&q);
    let near_d = NEAR_SPAN / f.views[NEAR_VIEW].2;
    let lmax = libm::log2f((MAX_ITER + 2) as f32);
    // An escaped count is below MAX_ITER + 1, so its bin is below BINS; the extra bin takes a value that rounds up to the top edge, so no index is ever out of range.
    let mut hist = [0u32; BINS + 1];
    let mut thirds = [0u32; 3];
    let (mut near, mut inside, mut outside) = (0u32, 0u32, 0u32);
    let long = if h >= w { h } else { w };
    for y in 0..h {
        f.each(NEAR_VIEW, y, |x, o| {
            let (n, d) = (o.n, o.d);
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
        let bits = |o: Orbit| (o.n.to_bits(), o.d.to_bits(), o.s.to_bits(), o.u[0].to_bits(), o.u[1].to_bits(), o.f[0].to_bits(), o.f[1].to_bits());
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
                    assert_eq!(bits(o[j]), bits(orbit(zx[j], zy[j], cx, cy)), "c {cx} {cy} z {} {}", zx[j], zy[j]);
                }
            }
        }
    }

    /// Every palette colour, turned to any of 4096 hues round the whole circle (and the warm sparks past it by their jitter), stays a real colour: no negative cone response, every channel within black and white; and its lightness J is unchanged.
    #[test]
    fn palette_in_gamut_all_round() {
        for code in [BG[0], BG[1], SHADE, HALO, FIL, HOT, WARM, FAR] {
            let v = [code[0] as f32 / 16384., code[1] as f32 / 16384., code[2] as f32 / 16384.];
            let lms = cols(&VSF_RGB2LMS, [v[0] * v[0], v[1] * v[1], v[2] * v[2]]);
            let j0 = rows(&OPP, [libm::sqrtf(lms[0]), libm::sqrtf(lms[1]), libm::sqrtf(lms[2])])[0];
            for k in 0..(1 << 12) {
                let a = k as f32 * (TAU / (1u32 << 12) as f32);
                let (q, rgb) = turned(code, a, CHROMA);
                assert!(q.iter().all(|&c| c >= 0.), "{code:?} at {a}: q {q:?}");
                assert!(rgb.iter().all(|c| (0. ..=1.).contains(c)), "{code:?} at {a}: {rgb:?}");
                let j = rows(&OPP, q)[0];
                assert!((j - j0).abs() < 1. / (1u32 << 16) as f32, "{code:?} at {a}: J {j0} to {j}");
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

    /// Marking and refining in bands gives the same pixels as over the whole frame, refining changes only marked pixels, and the share refined is reported.
    #[test]
    fn refine_banded() {
        let (w, h) = (336, 748);
        let p = params(&seed_of(23), w, h);
        let mut first = vec![0u64; w * h];
        render_rows(&p, 0..h, &mut first);
        let s = mask_stride(w);
        let mut mask = vec![0u8; h * s];
        mark_rows(&p, &first, 0..h, &mut mask);
        let mut whole = first.clone();
        refine_rows(&p, 0..h, &mask, &mut whole);
        let mut banded = first.clone();
        for y0 in (0..h).step_by(64) {
            let y1 = (y0 + 64).min(h);
            let mut m = vec![0u8; (y1 - y0) * s];
            mark_rows(&p, &first, y0..y1, &mut m);
            assert_eq!(m[..], mask[y0 * s..y1 * s]);
            refine_rows(&p, y0..y1, &m, &mut banded[y0 * w..y1 * w]);
        }
        assert_eq!(hash(&whole), hash(&banded));
        let marked = mask.iter().map(|b| b.count_ones()).sum::<u32>();
        for i in 0..w * h {
            if mask[i / w * s + (i % w) / 8] >> (i % w % 8) & 1 == 0 {
                assert_eq!(whole[i], first[i]);
            }
        }
        println!("refined {} of {} pixels ({:.1} %)", marked, w * h, 100. * marked as f64 / (w * h) as f64);
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
