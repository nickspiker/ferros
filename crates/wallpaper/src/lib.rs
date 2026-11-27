//! A person's default wallpaper: a Julia set drawn from a 32-byte seed, in the family of the owner's own wallpaper (a deep blue field, glowing periwinkle dendrites wound into spirals, small warm sparks where the filaments knot).
//! The picture is a pure function of the seed and the frame's size: the same seed gives a picture that looks the same on every device (owner, 2026-11-11: "if it looks roughly similar, that's totally fine ... the human WILL NOT NOTICE. Just needs to look the same"; aspect and resolution differ per device anyway), and different seeds give clearly different pictures.
//!
//! What the seed chooses (`candidate`):
//! - c, the Julia constant, just outside the Mandelbrot set, where the Julia set is a lace of spiralling dendrites: either a small step out from the main cardioid's boundary, c = e^{iθ}/2 − e^{2iθ}/4 + ε·n̂ (n̂ the outward normal), or out from the period-2 bulb's, c = −1 + (1 + ε)·e^{iφ}/4, conjugated half the time.
//!   Inside the set the picture would carry filled blobs, and far outside it thins to dust; the ranges keep it in the lace.
//! - The view: a point of the Julia set (the β fixed point taken back through a seeded run of preimages), pulled a seeded way toward the set's centre (`Anchor::Today`, the device's), or a spiral point (α or a seeded preimage of it) at the frame's centre or at a seeded third (`Anchor::AlphaCentre`, `Anchor::AlphaAt`); a seeded size and a seeded turn.
//! - The colours: one palette (the owner's) turned a seeded small angle about the grey axis, the warm sparks turned on their own, and the background's soft gradient in a seeded direction.
//!
//! Every candidate is looked at first at a sixteenth of the frame's size (`survey`): how much of the frame lies near the set, in the whole and in each third along its long side, and how much never escapes.
//! A candidate that leaves a third empty, covers too little or too much, or carries a filled blob is passed over, and the seed's next candidate (the same seed hashed with the attempt's number) is tried; after `ATTEMPTS` the best seen is kept.
//! So the result is always a filled composition, and still a function of the seed alone.
//! Every choice above is a field of `Knobs` (`params_with`), `Knobs::DEFAULT` the constants below; tools/tune moves them on the desktop.
//!
//! Colour: each pixel's orbit gives its smooth escape count n and its distance d to the set (the exterior distance estimate |z| ln|z| / |dz/dz₀|).
//! d, measured in spans of the frame, draws the filaments and their halo, so their width is the same fraction of the frame on every display; log₂(1 + n), normalised to the survey's median and 99th percentile, shades the field and lights the hottest filaments white and the knots warm.
//! The layers go front to back (AGENT.md "Compositing: Front to Back, `under` Only"): each a colour at a weight that is a C∞ function of d, n and the relief (a Cauchy bump, an algebraic sigmoid), taking its share of the light still uncovered; nothing is clamped. Behind the near set, the same set again zoomed out, turned and shifted, drawn as a soft dim glow, gives depth; a seeded lamp lights the set's relief (the potential's gradient), Lambert and a sheen, over a low ambient so the picture stays dark.
//! Each colour is a VSF RGB colour, held and mixed as its working code (√light, white at 1), as the display and fluor mix theirs: in light the mixes came out much lighter than they look they should (owner, 2026-11-07).
//!
//! Output: the display's working values, a u64 per pixel, R, G and B in bits 0-15, 16-31 and 32-47 as √light × 16384, α G#FFFF in bits 48-63.
//! One source for the CPU and the GPU (GPU-KISTS.md plan step 0; owner, 2026-11-11: "We should be using GPU path for the brot in all cases"): the orbit and the colouring are written over a lane (`lane.rs`, tapuwae's route). On the device it is valhall's recording type (`gpu.rs`, feature `gpu`): wallgen draws the picture with those kernels, and the CPU path stays for tests and tools/tune.
//! On the CPU, f32 and libm only: no fused multiply-add is ever formed (Rust never contracts), the square root is correctly rounded everywhere (aarch64's fsqrt, x86's sqrtss), and libm's logarithms are the same software on every target, so a row is the same wherever it is computed. The GPU's picture looks the same, not the same bits (gpu.rs).
#![cfg_attr(not(test), no_std)]

pub mod lane;
#[cfg(feature = "gpu")]
pub mod gpu;

use core::f32::consts::{PI, TAU};
use core::ops::Range;
use lane::{pick, Lane};
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

/// The choices a seed is turned into, as data (owner, 2026-11-10: "It would [change everyone's wallpaper], which is why I wanted to fidoodle with it some before we ship it"): what `candidate` draws c, the view and the hues from, and what `survey` holds a composition to.
/// `Knobs::DEFAULT` is the constants above, the device's wallpaper; tools/tune moves them on the desktop and prints the result as these constants' source.
/// The colouring's own weights (the layers' strengths and widths, the lamp, the flame) stay constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knobs {
    /// CARDIOID_T: the cardioid family's θ range in turns of its boundary; CARDIOID_EPS_LOG2: its step out, log₂ ε.
    pub cardioid_t: [f32; 2],
    pub cardioid_eps_log2: [f32; 2],
    /// BULB_PHI: the period-2 bulb's φ range in half turns; BULB_EPS: its step out, a fraction of the bulb's radius.
    pub bulb_phi: [f32; 2],
    pub bulb_eps: [f32; 2],
    /// BACK_STEPS: preimages taken back from β to find the point of the set the view is built on (`Anchor::Today`), each sign a bit of one 64-bit draw (past 64 steps the signs repeat).
    pub back_steps: u32,
    /// PULL: how far that point is pulled toward 0 (`Anchor::Today` only).
    pub pull: [f32; 2],
    /// VIEW_SPAN: the view's size in the plane per span of the frame.
    pub view_span: [f32; 2],
    /// FAR_ZOOM, FAR_TURN (radians, either way) and FAR_SHIFT (the far view's spans): the far layer against the near view.
    pub far_zoom: [f32; 2],
    pub far_turn: [f32; 2],
    pub far_shift: f32,
    /// WARM_HUE, FAR_HUE, HUE_STEP (radians): the warm sparks' and the far layer's turn off the palette's hue, and the map's step from stop to stop; SAT: the saturation's range.
    pub warm_hue: [f32; 2],
    pub far_hue: [f32; 2],
    pub hue_step: [f32; 2],
    pub sat: [f32; 2],
    /// GRAD_SPANS: how far the background gradient's ends sit apart, in spans.
    pub grad_spans: f32,
    /// ATTEMPTS: candidates surveyed before the best seen is kept.
    pub attempts: u32,
    /// NEAR, NEAR_THIRD, INSIDE, NEAR_SPAN: the survey's thresholds (`Survey::passes_with`).
    pub near: [f32; 2],
    pub near_third: f32,
    pub inside: f32,
    pub near_span: f32,
    /// Where the view is built (`Anchor`).
    pub anchor: Anchor,
    /// The spiral point for the α anchors: α itself at 0, else this many preimages back from it, each sign from the seed (every preimage of α is a conformal copy of its spiral), signs from the same draw as BACK_STEPS.
    pub alpha_steps: u32,
    /// `Anchor::AlphaAt`: where the spiral point lands in the frame, as fractions of its width and height from the top left (thirds by default), and whether the seed mirrors it across either axis (so thirds land on any of the four crossings).
    pub at: [f32; 2],
    pub at_mirror: bool,
}

/// Where a seed's view is built (owner, 2026-11-10, asked how the centre and the spiral are chosen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The device's today: a point of the Julia set BACK_STEPS seeded preimages back from β, pulled toward 0 by PULL, so the frame's centre usually sits in a gap and the spirals land wherever they fall.
    Today,
    /// The view's centre on the spiral point: α = ½ − √(¼ − c) (the repelling fixed point the dendrites wind round) or `alpha_steps` seeded preimages back from it, no pull.
    AlphaCentre,
    /// The spiral point at `at` in the frame (thirds, mirrored by the seed), the view's centre offset so the point lands there after the view's turn and scale.
    AlphaAt,
}

impl Knobs {
    /// The device's wallpaper: the constants above, the α anchors' knobs at their own defaults (unused by `Anchor::Today`).
    pub const DEFAULT: Knobs = Knobs {
        cardioid_t: CARDIOID_T,
        cardioid_eps_log2: CARDIOID_EPS_LOG2,
        bulb_phi: BULB_PHI,
        bulb_eps: BULB_EPS,
        back_steps: BACK_STEPS,
        pull: PULL,
        view_span: VIEW_SPAN,
        far_zoom: FAR_ZOOM,
        far_turn: FAR_TURN,
        far_shift: FAR_SHIFT,
        warm_hue: WARM_HUE,
        far_hue: FAR_HUE,
        hue_step: HUE_STEP,
        sat: SAT,
        grad_spans: GRAD_SPANS,
        attempts: ATTEMPTS,
        near: NEAR,
        near_third: NEAR_THIRD,
        inside: INSIDE,
        near_span: NEAR_SPAN,
        anchor: Anchor::Today,
        alpha_steps: 0,
        at: [1. / 3., 1. / 3.],
        at_mirror: true,
    };
}

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
    /// The plane point the view was built on (`Anchor`): the point of the set pulled toward 0 for `Today`, the spiral point for the α anchors.
    pub anchor: [f32; 2],
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
    /// Whether the composition is good enough to keep, by the device's thresholds.
    pub fn passes(&self) -> bool {
        self.passes_with(&Knobs::DEFAULT)
    }
    /// Whether the composition is good enough to keep, by `k`'s thresholds.
    pub fn passes_with(&self, k: &Knobs) -> bool {
        self.near > k.near[0] && self.near < k.near[1] && self.near_min_third > k.near_third && self.inside < k.inside
    }
    /// How close a failing candidate came, to keep the best when none passes.
    pub fn score_with(&self, k: &Knobs) -> f32 {
        self.near_min_third - self.inside - libm::fabsf(self.near - (k.near[0] + k.near[1]) * 0.5)
    }
}

/// A seed's wallpaper for a `width` × `height` frame, as the device draws it (`Knobs::DEFAULT`).
pub fn params(seed: &[u8; 32], width: usize, height: usize) -> Params {
    params_with(seed, width, height, &Knobs::DEFAULT)
}

/// A seed's wallpaper for a `width` × `height` frame under the knobs `k`.
pub fn params_with(seed: &[u8; 32], width: usize, height: usize, k: &Knobs) -> Params {
    params_seen(seed, width, height, k, |_| {})
}

/// `params_with`, handing each candidate to `seen` as it is surveyed (its `survey` filled in), so tools/tune can show why each attempt failed.
pub fn params_seen(seed: &[u8; 32], width: usize, height: usize, k: &Knobs, seen: impl FnMut(&Params)) -> Params {
    params_by(seed, width, height, k, |p| survey_with(p, k), seen)
}

/// `params_seen` with each candidate surveyed by `look`: the CPU's orbits (`survey_with`), or the GPU's (wallgen: `gpu::survey_kernel` run, its counts and distances read back and `tally`d), which is how the device chooses (owner, 2026-11-11: the brot on the GPU in all cases).
pub fn params_by(seed: &[u8; 32], width: usize, height: usize, k: &Knobs, mut look: impl FnMut(&Params) -> Survey, mut seen: impl FnMut(&Params)) -> Params {
    let mut best = candidate_with(seed, 0, width, height, k);
    let mut best_score = f32::MIN;
    for a in 0..k.attempts {
        let mut p = candidate_with(seed, a, width, height, k);
        let s = look(&p);
        p.survey = s;
        seen(&p);
        if s.passes_with(k) {
            return tune(p);
        }
        if s.score_with(k) > best_score {
            best_score = s.score_with(k);
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
        for x in 0..w {
            out[k * w + x] = pack(f.pixel(x as f32 + 0.5 - f.half[0], y as f32 + 0.5 - f.half[1]));
        }
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
    for (k, y) in rows.enumerate() {
        for x in 0..w {
            if mask[k * stride + x / 8] >> (x % 8) & 1 == 0 {
                continue;
            }
            out[k * w + x] = pack(f.refined(x as f32, y as f32));
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
/// A lane each (`lane.rs`): f32 on the CPU, a recorded value on the GPU.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Orbit<L = f32> {
    pub n: L,
    pub d: L,
    pub s: L,
    pub u: [L; 2],
    /// The flame (mandelbrot-exploder's, as tapuwae colours it): (conj(fc) − fq)·ln(n)/2⁴, fq the orbit's mean direction weighted by step² and fc its mean 1/z weighted by 1/|z| (owner, 2026-11-07: "more flames"); zero inside the set.
    pub f: [L; 2],
}

impl<L: Lane> Orbit<L> {
    /// An orbit that never escaped: the count's own limit, on the set, no rays.
    fn inside() -> Orbit<L> {
        let z = L::splat(0.);
        Orbit { n: L::splat((MAX_ITER + 1) as f32), d: z, s: z, u: [z, z], f: [z, z] }
    }
    /// `a` where `m` holds, `b` elsewhere, field by field.
    fn select(m: L::M, a: Orbit<L>, b: Orbit<L>) -> Orbit<L> {
        let s = |x: L, y: L| L::select(m, x, y);
        Orbit { n: s(a.n, b.n), d: s(a.d, b.d), s: s(a.s, b.s), u: [s(a.u[0], b.u[0]), s(a.u[1], b.u[1])], f: [s(a.f[0], b.f[0]), s(a.f[1], b.f[1])] }
    }
}

/// The orbit's state round its loop (`orbit`, through `Lane::iterate`), every field a lane, so the GPU carries it as a row of values: z; dz/dz₀ and its scale (1, times DZ_SCALE each time dz was scaled down); the stripe sum; the flame's sums (z's direction weighted by the step's number squared and that weight, z̄ / |z|² weighted by 1/|z| and that weight); and the head's: x², y², |z|² and the step's number.
#[derive(Clone, Copy)]
struct Step<L> {
    x: L,
    y: L,
    dx: L,
    dy: L,
    sc: L,
    sum: L,
    q0: L,
    q1: L,
    qw: L,
    c0: L,
    c1: L,
    cw: L,
    xx: L,
    yy: L,
    r2: L,
    i: L,
}

/// One pixel's orbit under z ↦ z² + c from (zx, zy).
/// The count runs continuously from i to i + 1 across the iteration where the orbit escapes, and an orbit that never escapes gets MAX_ITER + 1, the count's own limit, and distance 0.
/// dz/dz₀ grows by 2|z| ≤ 512 a step; once |dz|² passes 2^62 it is scaled down by 2^32 (exact, a power of two) and the scale kept, so it never overflows f32; the distance is multiplied by the scale at the end (until 2026-11-11 an integer exponent and ldexp, which the GPU has no instruction for: the same bits wherever the result is a normal float, and where it is not the distance is below any use of it).
/// The stripe average (Härkönen's): sin 4·arg z_k averaged over the orbit from z₁ on, blended across the escaping step by the count's fraction so it runs on continuously where the escape time steps; it draws rays out of the set through the field.
/// The loop is `Lane::iterate`: MAX_ITER + 1 passes, the last one's head finding the orbit escaped at MAX_ITER or never.
#[inline]
pub fn orbit<L: Lane>(zx: L, zy: L, cx: f32, cy: f32) -> Orbit<L> {
    let (zero, one) = (L::splat(0.), L::splat(1.));
    let init = Step { x: zx, y: zy, dx: one, dy: zero, sc: one, sum: zero, q0: zero, q1: zero, qw: zero, c0: zero, c1: zero, cw: zero, xx: zero, yy: zero, r2: zero, i: zero };
    let s = L::iterate(
        MAX_ITER + 1,
        init,
        |n, mut s| {
            s.xx = s.x * s.x;
            s.yy = s.y * s.y;
            s.r2 = s.xx + s.yy;
            s.i = n;
            // Running while |z|² ≤ 2^16 (not escaped) and below the cap.
            let running = L::splat(R2).ge(s.r2) & n.lt(L::splat(MAX_ITER as f32));
            (s, running)
        },
        |n, mut s| {
            let (x, y, xx, yy, r2) = (s.x, s.y, s.xx, s.yy, s.r2);
            s.sum = pick::<L, L>(n.gt(zero), || s.sum + stripe(x, y, xx, yy, r2), s.sum, L::select);
            // The flame's sums, this step the n-th (from 0).
            let inv = one / (r2.sqrt() + FLAME_EPS);
            let inv2 = inv * inv;
            let it = n + 1.;
            let wt = it * it * taper(r2);
            let wq = wt * inv;
            s.q0 = s.q0 + x * wq;
            s.q1 = s.q1 + y * wq;
            s.qw = s.qw + wt;
            s.c0 = s.c0 + x * inv2;
            s.c1 = s.c1 - y * inv2;
            s.cw = s.cw + inv;
            let ndx = (x * s.dx - y * s.dy) * 2.;
            let ndy = (x * s.dy + y * s.dx) * 2.;
            // Multiplying by 1 is exact, so dz is unchanged where it is not scaled.
            let f = L::select((ndx * ndx + ndy * ndy).gt(L::splat(DZ_BIG)), L::splat(DZ_SCALE), one);
            s.dx = ndx * f;
            s.dy = ndy * f;
            s.sc = s.sc * f;
            s.y = x * 2. * y + cy;
            s.x = xx - yy + cx;
            s
        },
    );
    pick::<L, _>(s.r2.gt(L::splat(R2)), || escaped(s), Orbit::inside(), Orbit::select)
}

/// Added to |z| under the flame's division: z = 0 exactly (a preimage of −c can land there) would divide by zero; anywhere |z| > 2^-76 it is below f32's resolution of |z| and changes nothing.
const FLAME_EPS: f32 = STRIPE_EPS;

/// The z-flame's taper at |z|² = r2 (tapuwae's, from mandelbrot-exploder): 1 while |z|² ≤ 4, then (1 − t²)³ with t = (ln ln |z|² − ln(2 ln 2)) / ln ln bail, which for this bailout (2^16) is (log₂ log₂ |z|² − 1) / 3, from 0 at |z|² = 4 to 1 at the bailout, where the weight reaches 0 with its slope.
/// Only the last few steps of an orbit pass |z|² = 4, so on the CPU its two logarithms are rare.
/// It tapers the z-flame's weight to nothing as the orbit nears the bailout, so the sums run on continuously where the escape step changes from pixel to pixel; without it the flames came out as blocky tiles (2026-11-07).
#[inline]
pub fn taper<L: Lane>(r2: L) -> L {
    let one = L::splat(1.);
    pick::<L, L>(
        r2.gt(L::splat(4.)),
        || {
            let t = (r2.log2().log2() - 1.) * (1. / 3.);
            let u = one - t * t;
            u * u * u
        },
        one,
        L::select,
    )
}

/// |dz|² past which dz is scaled down, and the scale (2^-32, exact).
const DZ_BIG: f32 = (1u64 << 62) as f32;
const DZ_SCALE: f32 = 1. / (1u64 << 32) as f32;
/// Added to |z|⁴ under the stripe's division: z = 0 exactly (a preimage of −c can land there) would otherwise make it 0/0, a NaN that colours the pixel; with it the stripe there is 0, and anywhere |z| > 2^-24 the term is below f32's resolution of |z|⁴, so it changes nothing else.
const STRIPE_EPS: f32 = 1. / (1u128 << 100) as f32;

/// sin 4·arg z = Im z⁴ / |z|⁴, without a trigonometric function.
#[inline]
fn stripe<L: Lane>(x: L, y: L, xx: L, yy: L, r2: L) -> L {
    x * 4. * y * (xx - yy) / (r2 * r2 + STRIPE_EPS)
}

/// The orbit that escaped in the state `s`: after `s.i` steps at z, with dz/dz₀ times its scale and the stripe sum over z₁ … z_{i−1}.
#[inline]
fn escaped<L: Lane>(s: Step<L>) -> Orbit<L> {
    let (x, y, dx, dy, xx, yy, r2, i) = (s.x, s.y, s.dx, s.dy, s.xx, s.yy, s.r2, s.i);
    // log₂|z|² > 16 here, so the count's log is of a value above 4 and its fraction 5 − log₂ log₂|z|² lies in [0, 1).
    let l2 = r2.log2();
    let frac = L::splat(5.) - l2.log2();
    let n = i + frac;
    let dz = (dx * dx + dy * dy).sqrt();
    let d = r2.sqrt() * (0.5 * core::f32::consts::LN_2) * l2 / dz;
    // The average with this step's term and without it, each over a prior of two zero terms (so neither divides by zero): an orbit that escapes just past the bailout (fraction near 1) takes the first, one that escaped a step later from far out (fraction near 0) the second, and the two meet.
    let all = s.sum + stripe(x, y, xx, yy, r2);
    let with = all / (i + 2.);
    let without = s.sum / (i + 1.);
    // z / dz's direction, z·conj(dz) normalised (the scale does not turn it); both are far from zero here (|z| > 256, and dz has grown every step), so the length is never zero.
    let (ux, uy) = (x * dx + y * dy, y * dx - x * dy);
    let um = (ux * ux + uy * uy).sqrt();
    // The flame at the escape: (conj(fc) − fq)·ln(n)/2⁴, as tapuwae has it. An orbit escapes only after at least one step from the frame's view (it starts within a few units of the set, |z| < 256), so both weights are above zero.
    let (fq, fc) = ([s.q0 / s.qw, s.q1 / s.qw], [s.c0 / s.cw, s.c1 / s.cw]);
    let g = n.ln() * (1. / (1 << 4) as f32);
    Orbit { n, d: d * s.sc, s: without + (with - without) * frac, u: [ux / um, uy / um], f: [(fc[0] - fq[0]) * g, (-fc[1] - fq[1]) * g] }
}

/// A C∞ step from 0 to 1, the algebraic sigmoid: ½ + ½·x/√(1 + x²).
#[inline]
fn sig<L: Lane>(x: L) -> L {
    x * 0.5 / (x * x + 1.).sqrt() + 0.5
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

    /// The plane in view `v` at (fx, fy) pixels from the frame's centre (any fraction: the refining pass samples inside a pixel).
    #[inline]
    fn z_at<L: Lane>(&self, v: usize, fx: L, fy: L) -> (L, L) {
        let (centre, step, _) = self.views[v];
        (fx * step[0] + centre[0] - fy * step[1], fx * step[1] + centre[1] + fy * step[0])
    }

    /// The far layer's weight at (fx, fy) pixels from the frame's centre: its soft glow, 1 / (1 + t²) with t the far view's distance in its spans, times FAR_INV.
    #[inline]
    fn far<L: Lane>(&self, fx: L, fy: L) -> L {
        let (zx, zy) = self.z_at(FAR_VIEW, fx, fy);
        let t = orbit(zx, zy, self.p.c[0], self.p.c[1]).d * self.views[FAR_VIEW].2 * FAR_INV;
        L::splat(1.) / (t * t + 1.)
    }

    /// The first pass's pixel, one sample at its centre (fx, fy pixels from the frame's centre): the far layer's weight, then the near orbit coloured.
    #[inline]
    fn pixel<L: Lane>(&self, fx: L, fy: L) -> [L; 3] {
        let far = self.far(fx, fy);
        let (zx, zy) = self.z_at(NEAR_VIEW, fx, fy);
        self.colour(fx, fy, orbit(zx, zy, self.p.c[0], self.p.c[1]), far)
    }

    /// The refining pass's pixel at column x and row y: the mean of REFINE × REFINE samples spread evenly across it, of working codes as the layers are mixed. The far layer is a soft glow, so it is sampled once at the pixel's centre.
    #[inline]
    fn refined<L: Lane>(&self, x: L, y: L) -> [L; 3] {
        let far = self.far(x + 0.5 - self.half[0], y + 0.5 - self.half[1]);
        let zero = L::splat(0.);
        let mut sum = [zero; 3];
        for j in 0..REFINE {
            let fy = y + (j as f32 + 0.5) / REFINE as f32 - self.half[1];
            for i in 0..REFINE {
                let fx = x + (i as f32 + 0.5) / REFINE as f32 - self.half[0];
                let (zx, zy) = self.z_at(NEAR_VIEW, fx, fy);
                let c = self.colour(fx, fy, orbit(zx, zy, self.p.c[0], self.p.c[1]), far);
                sum = [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2]];
            }
        }
        let n = (REFINE * REFINE) as f32;
        [sum[0] / n, sum[1] / n, sum[2] / n]
    }

    /// Pixel (fx, fy)'s working codes, from its near orbit and the far layer's weight there.
    /// Front to back (owner's rule, AGENT.md "Compositing: Front to Back, `under` Only"): each layer a colour at its weight α, taking its share of the light that is still uncovered (`t`), which it then covers by α; the frontmost goes first: the warm sparks, the white-hot core, the filament, the sheen, the halo, the flame's glow, the far layer, the field's shade, and the background last, opaque, so every pixel's light is a convex sum of the palette's.
    #[inline]
    fn colour<L: Lane>(&self, fx: L, fy: L, o: Orbit<L>, far: L) -> [L; 3] {
        let p = self.p;
        let one = L::splat(1.);
        let v = ((o.n + 1.).log2() - p.l_lo) * p.l_inv;
        let ds = o.d * self.views[NEAR_VIEW].2;
        let map = |t: L| gmap(&p.stops, t);
        let konst = |c: [f32; 3]| [L::splat(c[0]), L::splat(c[1]), L::splat(c[2])];
        // The lamp on the relief: Lambert, smooth, and the sheen where the relief faces it.
        let l = o.u[0] * p.lamp[0] + o.u[1] * p.lamp[1];
        let lam = (l + LAMP_H) * (1. / (1. + LAMP_H));
        let s = (l + 1.) * 0.5;
        let (s2, s4) = (s * s, s * s * s * s);
        let sheen = s4 * s4 * s2 * s2 * s4;
        let lit = |c: [L; 3], k: L| [c[0] * k, c[1] * k, c[2] * k];
        let mut acc = [L::splat(0.); 3];
        let mut t = one;
        let mut put = |c: [L; 3], a: L| {
            let w = a * t;
            acc = [acc[0] + c[0] * w, acc[1] + c[1] * w, acc[2] + c[2] * w];
            t = t * (one - a);
        };
        let slow = sig((v - SHADE_V) * SHADE_K);
        let tw = ds * WARM_INV;
        put(konst(p.warm), sig((v - WARM_V) * WARM_K) / (tw * tw + 1.));
        let tc = ds * CORE_INV;
        let core = one / (tc * tc + 1.);
        let core = core * core;
        put(konst(p.hot), core * sig((v - HOT_V) * HOT_K));
        put(lit(map(one), lam), core * FIL_A);
        let th = ds * HALO_INV;
        let near_halo = one / (th * th + 1.);
        put(konst(p.hot), sheen * SHEEN_A * near_halo);
        put(lit(map(slow * (HALO_T[1] - HALO_T[0]) + HALO_T[0]), lam * (1. - AMBIENT) + AMBIENT), near_halo * HALO_A);
        let tg = ds * GLOW_INV;
        let rays = sig((o.s - RAYS_MID) * RAYS_K);
        // The flame's glow is not lit: far from the set the potential's gradient spins round its critical points, and lit it drew small dark stars in the open field.
        put(map(rays * (RAYS_T[1] - RAYS_T[0]) + RAYS_T[0]), rays * RAYS_A / (tg * tg + 1.));
        put(map(far * (FAR_T[1] - FAR_T[0]) + FAR_T[0]), far * FAR_A);
        // The field, opaque, last: the map's dark end, by the background's seeded gradient and where escape is slow.
        // The soft flame on the field: its hue wave moves where the field reads the map, its lightness wave darkens it, both fading in away from the set.
        let px = ds * self.span;
        let fade = px * px / (px * px + FLAME_FADE * FLAME_FADE);
        let hue_wave = (o.f[0] * FLAME_F).sin_turns() * FLAME_T * fade;
        let dark = one - ((o.f[1] * FLAME_F).sin_turns() * 0.5 + 0.5) * FLAME_L * fade;
        let pos = sig(fx * p.grad[0] + fy * p.grad[1]);
        put(lit(map((pos + slow * SHADE_A) * FIELD_T * (1. / (1. + SHADE_A)) + hue_wave), dark), one);
        acc
    }
}

/// Light to working codes, √light a channel: the wallpaper mixes its layers in codes, as the display, fluor and tapuwae all do (owner, 2026-11-07: "we should be doing it in squared space, rather than linear to give it a dark appearance"); in light, a dark field half covered by a bright halo came out at half the bright light, much lighter than it looks it should.
fn codes(c: [f32; 3]) -> [f32; 3] {
    [libm::sqrtf(c[0]), libm::sqrtf(c[1]), libm::sqrtf(c[2])]
}

/// The density gradient map at `t` (0 the dark end, 1 the light): each stop weighted by a Cauchy bump about its place, the weights normalised, so the colour is a convex sum of the stops' and C∞ in t.
#[inline]
fn gmap<L: Lane>(stops: &[[f32; 3]; STOPS], t: L) -> [L; 3] {
    let zero = L::splat(0.);
    let mut c = [zero; 3];
    let mut sum = zero;
    for (k, s) in stops.iter().enumerate() {
        let d = (t - k as f32 * (1. / (STOPS - 1) as f32)) * GM_K;
        let w = L::splat(1.) / (d * d + 1.);
        c = [c[0] + w * s[0], c[1] + w * s[1], c[2] + w * s[2]];
        sum = sum + w;
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

/// The seed's candidate for one attempt: c, the view and the colours, as the device draws it (`Knobs::DEFAULT`).
pub fn candidate(seed: &[u8; 32], attempt: u32, width: usize, height: usize) -> Params {
    candidate_with(seed, attempt, width, height, &Knobs::DEFAULT)
}

/// The seed's candidate for one attempt under the knobs `k`.
/// Every anchor takes the same draws in the same order (the α anchors draw where to land last, after all the rest), so moving between anchors keeps a seed's c, size, turn and colours, and only the view's centre moves.
pub fn candidate_with(seed: &[u8; 32], attempt: u32, width: usize, height: usize, k: &Knobs) -> Params {
    let mut r = Rng::new(seed, attempt);
    let flip = if r.next() & 1 == 0 { 1. } else { -1. };
    let (cx, cy) = if r.next() & 1 == 0 {
        let th = r.range(k.cardioid_t) * TAU;
        let eps = libm::exp2f(r.range(k.cardioid_eps_log2));
        let (ct, st) = (libm::cosf(th), libm::sinf(th));
        let (c2, s2) = (libm::cosf(2. * th), libm::sinf(2. * th));
        // The boundary point e^{iθ}/2 − e^{2iθ}/4, and its outward normal e^{iθ}(1 − e^{iθ}), never zero for θ inside (0, 1) turns.
        let (nx, ny) = (ct - c2, st - s2);
        let nm = libm::sqrtf(nx * nx + ny * ny);
        (0.5 * ct - 0.25 * c2 + eps * nx / nm, 0.5 * st - 0.25 * s2 + eps * ny / nm)
    } else {
        let phi = r.range(k.bulb_phi) * PI;
        let rad = 0.25 * (1. + r.range(k.bulb_eps));
        (-1. + rad * libm::cosf(phi), rad * libm::sinf(phi))
    };
    let cy = flip * cy;
    // The two fixed points ½ ± √(¼ − c), both repelling outside the main cardioid: β (+) at the set's outer tip, α (−) where the dendrites wind.
    let (sx, sy) = csqrt(0.25 - cx, -cy);
    // Preimages ±√(z − c) from (zx, zy), the signs bits of one draw (bit k for step k, rotated so past 64 steps they repeat): every preimage of a point of the Julia set is in it, and of α a copy of its spiral.
    let bits = r.next();
    let back = |mut zx: f32, mut zy: f32, steps: u32| {
        for j in 0..steps {
            let (a, b) = csqrt(zx - cx, zy - cy);
            let s = if bits.rotate_right(j) & 1 == 0 { 1. } else { -1. };
            zx = s * a;
            zy = s * b;
        }
        [zx, zy]
    };
    let pull = r.range(k.pull);
    let sc = r.range(k.view_span) / span(width, height);
    let view_turn = r.unit() * TAU;
    let grad_turn = r.unit() * TAU;
    // The far layer: zoomed out, turned and shifted from the near view, all seeded.
    let far_sc = sc * r.range(k.far_zoom);
    let far_turn = view_turn + r.range(k.far_turn) * if r.next() & 1 == 0 { 1. } else { -1. };
    let shift_turn = r.unit() * TAU;
    let shift = k.far_shift * far_sc * span(width, height);
    let lamp_turn = r.unit() * TAU;
    let step = [sc * libm::cosf(view_turn), sc * libm::sinf(view_turn)];
    let (anchor, centre) = match k.anchor {
        Anchor::Today => {
            let z = back(0.5 + sx, sy, k.back_steps);
            ([z[0] * pull, z[1] * pull], [z[0] * pull, z[1] * pull])
        }
        Anchor::AlphaCentre => {
            let a = back(0.5 - sx, -sy, k.alpha_steps);
            (a, a)
        }
        Anchor::AlphaAt => {
            let a = back(0.5 - sx, -sy, k.alpha_steps);
            // The place in the frame, the seed mirroring it across either axis; then in pixels from the frame's centre, and the centre set back from the spiral point by the view's step (`Frame::z_at`), so the point lands there.
            let m = r.next();
            let fx = if k.at_mirror && m & 1 == 1 { 1. - k.at[0] } else { k.at[0] };
            let fy = if k.at_mirror && m & 2 == 2 { 1. - k.at[1] } else { k.at[1] };
            let (px, py) = ((fx - 0.5) * width as f32, (fy - 0.5) * height as f32);
            (a, [a[0] - (px * step[0] - py * step[1]), a[1] - (px * step[1] + py * step[0])])
        }
    };
    let mut hr = Rng::new(seed, HUE_STREAM);
    let hue = hr.unit() * TAU;
    let warm_hue = hue + hr.range(k.warm_hue);
    let far_hue = hue + hr.range(k.far_hue);
    let sat = hr.range(k.sat);
    // The map's hues: a walk round the circle from the seed's hue, a seeded step either way at each stop.
    let mut stops = [[0f32; 3]; STOPS];
    let mut h = hue;
    for j in 0..STOPS {
        stops[j] = codes(turn(STOP_CODES[j], h, sat));
        let s = hr.range(k.hue_step);
        h += if hr.next() & 1 == 0 { s } else { -s };
    }
    let g = 1. / (k.grad_spans * span(width, height));
    Params {
        width,
        height,
        c: [cx, cy],
        centre,
        step,
        grad: [g * libm::cosf(grad_turn), g * libm::sinf(grad_turn)],
        stops,
        hot: codes(turn(HOT, far_hue, sat)),
        warm: codes(turn(WARM, warm_hue, sat)),
        far_centre: [centre[0] + shift * libm::cosf(shift_turn), centre[1] + shift * libm::sinf(shift_turn)],
        far_step: [far_sc * libm::cosf(far_turn), far_sc * libm::sinf(far_turn)],
        lamp: [libm::cosf(lamp_turn), libm::sinf(lamp_turn)],
        l_lo: 0.,
        l_inv: 1.,
        anchor,
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

/// Looks at a candidate at a sixteenth of its size, "near the set" as the device holds it (`Knobs::DEFAULT`).
pub fn survey(p: &Params) -> Survey {
    survey_with(p, &Knobs::DEFAULT)
}

/// Looks at a candidate at a sixteenth of its size, "near the set" within `k.near_span` of a span: each preview pixel's orbit on the CPU, tallied (`tally`).
pub fn survey_with(p: &Params, k: &Knobs) -> Survey {
    let q = preview(p);
    let f = Frame::new(&q);
    tally(p, k, |emit| {
        for y in 0..q.height {
            for x in 0..q.width {
                let (zx, zy) = f.z_at(NEAR_VIEW, x as f32 + 0.5 - f.half[0], y as f32 + 0.5 - f.half[1]);
                let o = orbit(zx, zy, p.c[0], p.c[1]);
                emit(x, y, o.n, o.d);
            }
        }
    })
}

/// A candidate's frame at a sixteenth of its size, which the survey looks at: one preview pixel per PREVIEW × PREVIEW of the frame, the same views at PREVIEW times the step.
pub fn preview(p: &Params) -> Params {
    let mut q = *p;
    q.width = p.width / PREVIEW;
    q.height = p.height / PREVIEW;
    q.step = [p.step[0] * PREVIEW as f32, p.step[1] * PREVIEW as f32];
    q.far_step = [p.far_step[0] * PREVIEW as f32, p.far_step[1] * PREVIEW as f32];
    q
}

/// What a sixteenth-scale look at `p` found, from every preview pixel's column, row, smooth count and distance, as `samples` hands them to the function it is given: the CPU's orbits (`survey_with`) or the GPU's read back (`gpu::survey_kernel`, wallgen). How much of the frame lies near the set, in the whole and in each third along its long side; how much never escapes; and log₂(1 + n)'s median and 99th percentile from a histogram of the escaped.
pub fn tally(p: &Params, k: &Knobs, samples: impl FnOnce(&mut dyn FnMut(usize, usize, f32, f32))) -> Survey {
    let q = preview(p);
    let (w, h) = (q.width, q.height);
    let f = Frame::new(&q);
    let near_d = k.near_span / f.views[NEAR_VIEW].2;
    let lmax = libm::log2f((MAX_ITER + 2) as f32);
    // An escaped count is below MAX_ITER + 1, so its bin is below BINS; the extra bin takes a value that rounds up to the top edge, so no index is ever out of range. A count from the GPU is held to the same: one above MAX_ITER is counted inside, and a NaN's bin is 0 (`as usize`).
    let mut hist = [0u32; BINS + 1];
    let mut thirds = [0u32; 3];
    let (mut near, mut inside, mut outside) = (0u32, 0u32, 0u32);
    let long = if h >= w { h } else { w };
    samples(&mut |x, y, n, d| {
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

    /// `knobs_default_is_today`'s expected values, computed from the code as it stood before the knobs (0a1304b), in release and debug alike.
    const TODAY_PIXELS: &[(u32, u64)] = &[(9, 0xD6DD_8707_739F_8411), (9, 0x00C0_A2F4_FF7C_F056), (1, 0x21F0_B904_F9F4_C950), (1, 0x955C_065C_7E18_AB18), (3, 0x67FD_6934_1246_D3DE), (3, 0xDEAA_4EBF_F48B_D6AC), (0, 0x4AB5_094A_3D54_2807), (0, 0xCDB2_CE60_39BA_795B), (0, 0xE6AC_2E3A_1891_9771), (0, 0x095E_871E_2541_FF0C), (1, 0x6E01_4FC4_4970_BF4D), (1, 0xF5B3_4544_5F68_725B)];
    const TODAY_FULL: &[u64] = &[0x5648_F637_BCCA_ABA1, 0x4C49_405C_7FF2_70D0, 0x8E6E_3810_54A7_52BE, 0xC799_382D_F3AC_159A, 0x8B04_B6E0_B832_FF4F, 0x56B4_8C6E_562E_848D, 0xB11B_A6C5_A78E_8284, 0x7A30_F7E4_C3A1_A4AD];

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

    /// The pixels and choices of fixed seeds before the knobs became data (2026-11-10, computed from the constants' code at 0a1304b), and after: bit for bit the same.
    /// Each case: the seed, the frame, the attempt kept, and the hash of the rendered frame's working values; then the choices alone at husky's full size (c, the view and the attempt), where the survey runs at the size the device runs it.
    #[test]
    fn knobs_default_is_today() {
        let seeds: [[u8; 32]; 6] = [seed_of(0), seed_of(1), seed_of(2), seed_of(3), seed_of(41), core::array::from_fn(|k| (k as u8).wrapping_mul(37) ^ 0xA5)];
        let frames = [(168, 374), (135, 300)];
        let mut got = Vec::new();
        for s in &seeds {
            for &(w, h) in &frames {
                let p = params(s, w, h);
                let mut px = vec![0u64; w * h];
                render_rows(&p, 0..h, &mut px);
                got.push((p.attempt, hash(&px)));
            }
        }
        let choice = |p: &Params| [p.c[0], p.c[1], p.centre[0], p.centre[1], p.step[0], p.step[1], p.far_centre[0], p.far_centre[1], p.far_step[0], p.far_step[1], p.l_lo, p.l_inv].iter().fold(p.attempt as u64, |h, v| mix(h ^ v.to_bits() as u64));
        let full: Vec<u64> = (0..8).map(|n| choice(&params(&seed_of(n), 1344, 2992))).collect();
        println!("pixels {got:X?}");
        println!("full {full:X?}");
        assert_eq!(got, TODAY_PIXELS);
        assert_eq!(full, TODAY_FULL);
    }

    /// The α anchors: the spiral point is α (z² + c = z) at no steps, a preimage of it after some (taken forward that many steps it comes back to α), the view's centre for `AlphaCentre` and under the chosen third for `AlphaAt`; and every anchor keeps the seed's c, size, turn and colours.
    #[test]
    fn anchors_place_the_spiral() {
        let (w, h) = (1344, 2992);
        let fwd = |z: [f32; 2], c: [f32; 2]| [z[0] * z[0] - z[1] * z[1] + c[0], 2. * z[0] * z[1] + c[1]];
        for n in 0..16 {
            let s = seed_of(n);
            let today = candidate(&s, 0, w, h);
            for steps in [0u32, 3] {
                let centre = candidate_with(&s, 0, w, h, &Knobs { anchor: Anchor::AlphaCentre, alpha_steps: steps, ..Knobs::DEFAULT });
                let at = candidate_with(&s, 0, w, h, &Knobs { anchor: Anchor::AlphaAt, alpha_steps: steps, ..Knobs::DEFAULT });
                for p in [&centre, &at] {
                    assert_eq!((p.c, p.step, p.far_step, p.stops, p.lamp), (today.c, today.step, today.far_step, today.stops, today.lamp));
                }
                let mut z = centre.anchor;
                for _ in 0..steps {
                    z = fwd(z, centre.c);
                }
                let f = fwd(z, centre.c);
                assert!((f[0] - z[0]).abs() + (f[1] - z[1]).abs() < 1. / (1 << 12) as f32, "seed {n}: α {z:?} goes to {f:?}");
                assert_eq!(centre.centre, centre.anchor);
                assert_eq!(at.anchor, centre.anchor);
                // Under some crossing of the thirds: the plane at that place is the spiral point.
                let fr = Frame::new(&at);
                let hit = [(1., 1.), (1., 2.), (2., 1.), (2., 2.)].iter().any(|&(a, b)| {
                    let z = fr.z_at(NEAR_VIEW, (a / 3. - 0.5) * w as f32, (b / 3. - 0.5) * h as f32);
                    (z.0 - at.anchor[0]).abs() + (z.1 - at.anchor[1]).abs() < 1. / (1 << 16) as f32
                });
                assert!(hit, "seed {n}: the spiral point lands on no third");
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
