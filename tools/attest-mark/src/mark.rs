//! The mark: the Julia set of z² + c seen through an inversion about its root spiral's eye, with c placed by the descent and the eye found on J(c), both drawn from a hash of (salt, text).
//!
//! The salt (on the phone, the device's own, sealed under its ira; here a stand-in) fixes the colouring method and the palette.
//!
//! Placement, the descent: pick x in [−2, 0.375] and come straight down the line Re c = x from Im c = 2, each step the proven lower bound on the distance to the Mandelbrot set (see `descend`), until within 2⁻¹² of the set — the set's skyline seen from above, its outermost contact on that line, from the antenna through the bulb tops and seahorse valley to the cusp.
//! For x past the cusp (0.25) the line misses the set and reaches the real axis just outside it, in the parabolic-implosion region, which is kept.
//! c is the landing point raised by 2^−(4..12) back up the line it came down (see HEIGHT_STOPS). Every point of that line above the landing was proven outside the set by the steps that crossed it, so c is always outside, J(c) is a Cantor dust with no inside to go flat, and near the boundary J(c) resembles the Mandelbrot set around c.
//!
//! The eye: a repelling periodic point of J(c) is the eye of a spiral whose multiplier λ = (f^p)′ says how it repeats (zooming by |λ| and turning by arg λ maps it onto itself), and the ROOT whorl, the largest, is the cycle closest to neutral (smallest ln|λ|).
//! Candidates: the cycle that is attracting in the Mandelbrot component beneath the descent's landing, Newton-closed at c (near-neutral when that component is close); the α fixed point in closed form; and 2⁵ sprayed cycles. The eye is one of the winner's own points (the hash picks which), never a preimage, which would be a sub-spiral.
//!
//! The view: dead centre on the eye, the whorl's radius R filling the plain window (`render`), or spherized into a disc (`render_disc`): a screen point p in the unit disc shows z = eye + R·p/√(1 − |p|²).
//! That is the plane wrapped onto a hemisphere seen from above (gnomonic onto the sphere, orthographic to the eye): the centre is nearly the plain view, and the rim is the plane's infinity, the outer field fading to black, so J(c) closes into a ball with no edge to crop.
//! It cannot also be conformal (no conformal map takes the disc onto the whole plane), so near the rim shapes stretch radially; the distance estimate converts to pixels through the map's area scale, R/(1 − |p|²) per disc unit, so the rim of the set keeps about its width.
//! A thumbnail still rejects views that show too little of the set, redrawing with the next counter.
//!
//! Precision: placement and the eye are f64 (a few dozen points per key, and the descent, the Newton classifier and the landing work near 2⁻¹²); the per-pixel render is f32 (the whorl's radius is kept at least 2⁻⁸, so even at the screen's edge, where the inversion packs z tightest, a pixel spans many f32 steps).
//!
//! Shading: the distance estimate (|z|·ln|z| over the derivative's magnitude, in pixels) separates the set from the field around it, because near the set the escape count changes chaotically from pixel to pixel and reads as static.
//! The set's body is dark with the colouring faintly inside it, its edge is a luminous rim a few pixels wide, and the field outside is the colouring method, dimmed and fading to black far out.
//! The colouring method is one of three: smooth iteration bands, the angle of the orbit at escape (a cellular mosaic), or an orbit trap (distance to a hash-chosen point: rings and petals).
//! Nothing compares the text against anything, so the picture says nothing about whether the text is right.

use fluor::paint::pack_argb;
use rayon::prelude::*;
use std::f64::consts::TAU;

/// Iteration cap for the render: deep enough for the windows used here.
const MAX_ITER: u32 = 1 << 10;
/// Escape when |z|² passes 2⁸ (|z| = 16), far enough out for smooth colouring.
const BAIL_SQ: f64 = (1 << 8) as f64;
/// A jump of 2² iterations between neighbouring thumbnail pixels marks an edge of the set.
const JUMP: i32 = 1 << 2;
/// A framing is kept when at least a quarter of the thumbnail is edge.
const DETAIL_MIN: f64 = 1. / (1 << 2) as f64;
/// Stops of height for c above its landing point, from 2⁻⁴ down: much above 2⁻⁴ J(c) thins into sparse dust, and below 2⁻¹² the landing's own tolerance (2⁻¹²) dominates, so the 2³ stops between are the ones that matter.
const HEIGHT_STOPS: u32 = 1 << 3;
/// Periods sprayed for a spiral's cycle: 1 to 2⁴.
const PERIODS: u32 = 1 << 4;
/// Cycles sprayed as candidates for the root whorl.
const SPIRAL_TRIES: u32 = 1 << 5;
/// The smallest window half-span kept, so the f32 render still has many representable steps per pixel.
const MIN_HALF: f64 = 1. / (1 << 8) as f64;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// The Julia set of z² + c.
    Julia,
    /// The parameter plane of z² + c, z starting at 0 (the locator panes).
    Plane,
}

/// Everything one picture needs, in the plane's terms (f64: placement and framing).
#[derive(Clone, Copy)]
pub struct Params {
    kind: Kind,
    c: (f64, f64),
    centre: (f64, f64),
    /// Half the window's span, in the plane.
    half: f64,
    /// The view's rotation as (cos, sin): plane = centre + rot·(u + iv) for screen offsets (u, v), v downward.
    rot: (f64, f64),
    /// Per-channel palette phases.
    phase: [f64; 3],
    /// Colouring method: 0 smooth bands, 1 escape angle, 2 orbit trap.
    mode: u8,
    /// The orbit trap's point, for mode 2.
    trap: (f64, f64),
    /// The root whorl's radius in the plane: the plain window's half-span, and the disc's scale.
    radius: f64,
}

/// The same picture in the render's precision (f32).
#[derive(Clone, Copy)]
struct View {
    kind: Kind,
    c: (f32, f32),
    centre: (f32, f32),
    rot: (f32, f32),
    phase: [f32; 3],
    mode: u8,
    trap: (f32, f32),
}

impl Params {
    fn view(&self) -> View {
        let f = |v: (f64, f64)| (v.0 as f32, v.1 as f32);
        View { kind: self.kind, c: f(self.c), centre: f(self.centre), rot: f(self.rot), phase: [self.phase[0] as f32, self.phase[1] as f32, self.phase[2] as f32], mode: self.mode, trap: f(self.trap) }
    }
}

/// 32 bits of a hash as a fraction in [0, 1).
fn unit(b: &[u8]) -> f64 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / (1u64 << 32) as f64
}

/// A keyed BLAKE3 of the salt over a domain byte, a counter and any extra bytes.
fn draw(salt: &[u8; 32], domain: u8, counter: u32, extra: &[u8]) -> [u8; 32] {
    *blake3::Hasher::new_keyed(salt).update(&[domain]).update(&counter.to_le_bytes()).update(extra).finalize().as_bytes()
}

fn cmul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// What one orbit did: iterations until escape (MAX_ITER if it never did), z at escape, the derivative's magnitude, and the closest approach to the trap point (squared).
struct Orbit {
    n: u32,
    x: f32,
    y: f32,
    dm: f32,
    trap_sq: f32,
}

/// Iterate z² + c from the plane point (px, py): z₀ = that point in a Julia set, c = that point in the parameter plane.
/// The derivative is tracked by magnitude only (|dz| ← 2·|z|·|dz|, plus 1 in the parameter plane), which is all the distance estimate needs.
/// It can overflow to infinity near the set, which the distance estimate reads, correctly, as "on the set".
#[inline]
fn orbit(px: f32, py: f32, v: &View) -> Orbit {
    let (mut z, c, plus, mut dm) = match v.kind {
        Kind::Plane => ((0f32, 0f32), (px, py), 1f32, 0f32),
        Kind::Julia => ((px, py), v.c, 0., 1.),
    };
    let bail = BAIL_SQ as f32;
    let mut trap_sq = f32::MAX;
    for n in 0..MAX_ITER {
        let (x2, y2) = (z.0 * z.0, z.1 * z.1);
        let r2 = x2 + y2;
        if r2 > bail {
            return Orbit { n, x: z.0, y: z.1, dm, trap_sq };
        }
        let (tx, ty) = (z.0 - v.trap.0, z.1 - v.trap.1);
        trap_sq = trap_sq.min(tx * tx + ty * ty);
        dm = 2. * r2.sqrt() * dm + plus;
        z = (x2 - y2 + c.0, 2. * z.0 * z.1 + c.1);
    }
    Orbit { n: MAX_ITER, x: z.0, y: z.1, dm, trap_sq }
}

/// The plane point under pixel (px, py) of a `w × h` window, `k` plane units per pixel: centre + rot·(u + is).
#[inline]
fn plane(v: &View, px: usize, py: usize, w: usize, h: usize, k: f32) -> (f32, f32) {
    let (u, s) = ((px as f32 + 0.5 - w as f32 * 0.5) * k, (py as f32 + 0.5 - h as f32 * 0.5) * k);
    (v.centre.0 + u * v.rot.0 - s * v.rot.1, v.centre.1 + u * v.rot.1 + s * v.rot.0)
}

/// Plane units per pixel for a `w × h` window: half the span covers `half`.
fn scale(half: f64, w: usize, h: usize) -> f32 {
    let span = 2. * (w * h) as f64 / (w + h) as f64;
    (half / (span * 0.5)) as f32
}

/// How much edge a 2⁵ × 2⁶ thumbnail shows: the share of pixels whose iteration count jumps by at least 2² to the neighbour on the right or below.
/// Only filaments of the set make such jumps; the smooth field between them makes none, so the score reads "how much of the view is set" at every zoom.
fn detail(p: &Params) -> f64 {
    const W: usize = 1 << 5;
    const H: usize = 1 << 6;
    let v = p.view();
    let k = scale(p.half, W, H);
    let mut n = [0i32; W * H];
    for py in 0..H {
        for px in 0..W {
            let (x, y) = plane(&v, px, py, W, H, k);
            n[py * W + px] = orbit(x, y, &v).n as i32;
        }
    }
    let mut edges = 0;
    for py in 0..H - 1 {
        for px in 0..W - 1 {
            let i = py * W + px;
            edges += ((n[i] - n[i + 1]).abs().max((n[i] - n[i + W]).abs()) >= JUMP) as u32;
        }
    }
    edges as f64 / ((W - 1) * (H - 1)) as f64
}

fn framed(p: &Params) -> bool {
    detail(p) >= DETAIL_MIN
}

/// One inverse branch of z² + c: ±√(z − c), the principal root, negated when `negate`.
fn inverse(c: (f64, f64), z: (f64, f64), negate: bool) -> (f64, f64) {
    let (ax, ay) = (z.0 - c.0, z.1 - c.1);
    let r = (ax * ax + ay * ay).sqrt();
    let (mut sx, mut sy) = (((r + ax) * 0.5).sqrt(), ((r - ax) * 0.5).sqrt());
    if ay < 0. {
        sy = -sy;
    }
    if negate {
        (sx, sy) = (-sx, -sy);
    }
    (sx, sy)
}

/// Address bit i (bit 0 is the coarsest: which half of J).
fn bit(bits: &[u8; 32], i: usize) -> bool {
    (bits[i >> 3] >> (i & 7)) & 1 == 1
}

/// The point of J(c) whose address is `bits`: inverse iteration applies the branches finest first, so bit 0, applied last, picks the half of J.
fn address_point(c: (f64, f64), bits: &[u8; 32]) -> (f64, f64) {
    let mut z = (0.5f64, 0.);
    for i in (0..(1 << 8)).rev() {
        z = inverse(c, z, bit(bits, i));
    }
    z
}

/// J(c)'s half-extent along its principal axis, from 2⁷ address points (J = −J, so its centre is 0).
fn j_extent(c: (f64, f64)) -> f64 {
    let pts: Vec<(f64, f64)> = (0u32..(1 << 7)).map(|i| address_point(c, blake3::hash(&i.to_le_bytes()).as_bytes())).collect();
    let (mut sxx, mut syy, mut sxy) = (0., 0., 0.);
    for p in &pts {
        sxx += p.0 * p.0;
        syy += p.1 * p.1;
        sxy += p.0 * p.1;
    }
    let axis = 0.5 * (2. * sxy).atan2(sxx - syy);
    let (ca, sa) = (axis.cos(), axis.sin());
    pts.iter().map(|p| (p.0 * ca + p.1 * sa).abs()).fold(0., f64::max)
}

/// Newton on f^period(w) = w from `guess`: the cycle point it closes on, with ln|λ| and arg λ of the multiplier λ = (f^period)′ there, kept only if f^period carries it back onto itself and it repels (every cycle of a Cantor Julia set does; a non-finite derivative means Newton wandered off to infinity).
fn close_cycle(c: (f64, f64), guess: (f64, f64), period: u32) -> Option<((f64, f64), f64, f64)> {
    // f^period and its derivative at w.
    let run = |w: (f64, f64)| {
        let (mut u, mut du) = (w, (1f64, 0f64));
        for _ in 0..period {
            du = cmul((2. * du.0, 2. * du.1), u);
            let uu = cmul(u, u);
            u = (uu.0 + c.0, uu.1 + c.1);
        }
        (u, du)
    };
    let mut w = guess;
    for _ in 0..NEWTON_STEPS {
        let (u, du) = run(w);
        // Newton step: w ← w − (u − w) / (du − 1).
        let (nr, ni) = (u.0 - w.0, u.1 - w.1);
        let (dr, di) = (du.0 - 1., du.1);
        let dd = dr * dr + di * di;
        let step = ((nr * dr + ni * di) / dd, (ni * dr - nr * di) / dd);
        w = (w.0 - step.0, w.1 - step.1);
        if step.0 * step.0 + step.1 * step.1 < (-80f64).exp2() {
            break;
        }
    }
    let (u, du) = run(w);
    let closed = (u.0 - w.0).abs() + (u.1 - w.1).abs() < 1. / (1 << 24) as f64;
    let mag_sq = du.0 * du.0 + du.1 * du.1;
    (closed && mag_sq > 1. && mag_sq.is_finite()).then(|| (w, 0.5 * mag_sq.ln(), du.1.atan2(du.0)))
}

/// A repelling cycle point of J(c) of period dividing `period`, Newton-closed from the point whose address repeats the first `period` bits of `bits`.
/// The repeated address is only the starting guess: the bits choose principal or negated square roots, and the principal root's cut can cross J, so a repeating bit pattern is not a true itinerary there.
fn cycle_point(c: (f64, f64), bits: &[u8; 32], period: u32) -> Option<((f64, f64), f64, f64)> {
    let mut rep = [0u8; 32];
    for i in 0..(1usize << 8) {
        if bit(bits, i % period as usize) {
            rep[i >> 3] |= 1 << (i & 7);
        }
    }
    close_cycle(c, address_point(c, &rep), period)
}

/// The root whorl of J(c): the cycle that is attracting inside the hyperbolic component beneath c's landing, now just repelling at c (|λ| a little over 1, arg λ near the component's internal angle), which makes it the slowest and largest spiral in the picture.
/// Found from the Mandelbrot side: keep going down the descent's line past the landing, which is usually a hair or decoration, until the Newton classifier proves a point inside; the component found is the one the hairs hang from. Below the landing the line crosses gaps (stepped by the same proven distance bound as the descent, at least 2⁻¹²) and hair the classifier cannot decide (stepped 2⁻¹⁰); at most 2¹⁰ steps, and not past the real axis.
/// The attracting cycle the classifier finds there is then Newton-closed at c itself. None when nothing inside is reached.
pub fn root_cycle(c: (f64, f64), land: (f64, f64)) -> Option<((f64, f64), f64, f64, u32)> {
    let x = land.0;
    let mut y = land.1;
    for _ in 0..(1 << 10) {
        match classify((x, y)) {
            Class::Inside { period, attractor } => {
                let (w, ln_mag, arg) = close_cycle(c, attractor, period)?;
                // The classifier's period can be a multiple of the cycle's own: walk the cycle until it returns.
                let mut z = w;
                let mut own = period;
                for q in 1..=period {
                    let zz = cmul(z, z);
                    z = (zz.0 + c.0, zz.1 + c.1);
                    if (z.0 - w.0).abs() + (z.1 - w.1).abs() < 1. / (1 << 20) as f64 {
                        own = q;
                        break;
                    }
                }
                return Some((w, ln_mag, arg, own));
            }
            Class::Outside { n, r, dm } => {
                let d = r * r.ln() / dm;
                let g = r.ln() * (-(n as f64)).exp2();
                let factor = if g > 0. { -(-2. * g).exp_m1() / (4. * g) } else { 0.5 };
                // WHY/PROOF: below the landing the line runs between hairs, where the proven step can shrink without bound; the floor keeps the probe moving, and at 2⁻¹² it is the descent's own landing tolerance.
                y -= (d * factor).max(1. / (1 << 12) as f64);
            }
            Class::Undecided => y -= 1. / (1 << 10) as f64,
        }
        if y < 0. {
            return None;
        }
    }
    None
}

/// Newton steps when solving for an attracting cycle.
const NEWTON_STEPS: u32 = 1 << 5;
/// Iteration cap for classifying a point of the parameter plane.
const CLASSIFY_ITER: u32 = 1 << 12;

/// Is there an attracting cycle of period `period` near `guess`? Newton on f^period(w) = w; a converged cycle with |multiplier| < 1 proves c is inside the set.
/// Ported from mandelbrot-exploder (compute/mandelbrot_f64.rs, find_interior_attractor).
fn attracting_cycle(c: (f64, f64), guess: (f64, f64), period: u32) -> Option<(f64, f64)> {
    let mut w = guess;
    for _ in 0..NEWTON_STEPS {
        let (mut u, mut du) = (w, (1., 0.));
        for _ in 0..period {
            du = cmul((2. * du.0, 2. * du.1), u);
            let uu = cmul(u, u);
            u = (uu.0 + c.0, uu.1 + c.1);
        }
        // Newton step: w ← w − (u − w) / (du − 1).
        let (nr, ni) = (u.0 - w.0, u.1 - w.1);
        let (dr, di) = (du.0 - 1., du.1);
        let dd = dr * dr + di * di;
        let step = ((nr * dr + ni * di) / dd, (ni * dr - nr * di) / dd);
        w = (w.0 - step.0, w.1 - step.1);
        if step.0 * step.0 + step.1 * step.1 < (-53f64).exp2() {
            return (du.0 * du.0 + du.1 * du.1 < 1.).then_some(w);
        }
    }
    None
}

/// Where c is with respect to the Mandelbrot set.
enum Class {
    /// Escaped at iteration n with |z| = r and |dz/dc| = dm.
    Outside { n: u32, r: f64, dm: f64 },
    /// Proven inside: an attracting cycle of this period was found, through this point.
    Inside { period: u32, attractor: (f64, f64) },
    /// Neither within the cap.
    Undecided,
}

/// Classify c, ported from mandelbrot-exploder's classify_point: iterate from z = c, and whenever |z|² falls below a quarter of its previous minimum, try Newton for an attracting cycle of the period reached so far.
fn classify(c: (f64, f64)) -> Class {
    let (mut z, mut dz) = (c, (1f64, 0f64));
    let mut smallest = f64::MAX;
    for n in 1..CLASSIFY_ITER {
        let r2 = z.0 * z.0 + z.1 * z.1;
        if r2 > BAIL_SQ {
            return Class::Outside { n, r: r2.sqrt(), dm: (dz.0 * dz.0 + dz.1 * dz.1).sqrt() };
        }
        if r2 < smallest {
            let ratio = r2 / smallest;
            smallest = r2;
            if ratio < 0.25 {
                if let Some(attractor) = attracting_cycle(c, z, n) {
                    return Class::Inside { period: n, attractor };
                }
            }
        }
        let zdz = cmul(z, dz);
        dz = (2. * zdz.0 + 1., 2. * zdz.1);
        let zz = cmul(z, z);
        z = (zz.0 + c.0, zz.1 + c.1);
    }
    Class::Undecided
}

/// The descent: down the line Re c = x from Im c = 2, until within 2⁻¹² of the set, onto it (proven inside or undecided), or onto the real axis; at most 2⁹ steps.
/// Each step is the PROVEN lower bound on the distance to the set (Koebe, via Milnor): with G = ln|z|/2ⁿ the Green's function and d = |z|·ln|z|/|dc| the estimate (= G/|G′|), the true distance is at least sinh G / (2·e^G·|G′|) = d·(1 − e^(−2G))/(4G).
/// That factor is ½ next to the set and smaller far out (about 0.22 at G = 1), so the step never jumps past anything, not even a hair-thin filament, and the descent stops at the first, outermost contact along the line.
/// Returns the landing point, whether it is on the set (false when the line missed it and reached the axis), and every point it checked, in order.
pub fn descend(x: f64) -> ((f64, f64), bool, Vec<(f64, f64)>) {
    let mut y = 2.;
    let mut trail = Vec::new();
    for _ in 0..(1 << 9) {
        trail.push((x, y));
        let Class::Outside { n, r, dm } = classify((x, y)) else {
            break;
        };
        let d = r * r.ln() / dm;
        if d < 1. / (1 << 12) as f64 {
            break;
        }
        // G underflows to 0 for orbits longer than about 2¹⁰ steps, where the factor's limit, ½, is exact enough.
        let g = r.ln() * (-(n as f64)).exp2();
        let factor = if g > 0. { -(-2. * g).exp_m1() / (4. * g) } else { 0.5 };
        y -= d * factor;
        if y <= 0. {
            trail.push((x, 0.));
            return ((x, 0.), false, trail);
        }
    }
    ((x, y), true, trail)
}

impl Params {
    /// The picture for this device salt and this textbox content.
    pub fn new(salt: &[u8; 32], text: &str) -> Self {
        let f = draw(salt, 1, 0, &[]);
        let phase = [unit(&f[0..4]), unit(&f[4..8]), unit(&f[8..12])];
        let mode = f[12] % 3;
        let trap = ((unit(&f[16..20]) - 0.5) * 2., (unit(&f[20..24]) - 0.5) * 2.);
        let base = Params { kind: Kind::Julia, c: (0., 0.), centre: (0., 0.), half: 2., rot: (1., 0.), phase, mode, trap, radius: 1. };

        // Each try draws a fresh c and framing from (salt, text, counter); the first framing that shows enough of the set is taken, and after 2⁵ tries the last one stands.
        let mut n = 0;
        loop {
            let t = draw(salt, 3, n, text.as_bytes());
            n += 1;
            let (land, _, _) = descend(-2. + unit(&t[16..20]) * 2.375);
            let c = (land.0, land.1 + (-(4. + unit(&t[20..24]) * HEIGHT_STOPS as f64)).exp2());
            let along = j_extent(c);
            // Every key sits on the ROOT whorl's eye, dead centre, never a preimage of it (a preimage is a smaller copy: a sub-spiral).
            // The root is the cycle closest to neutral (smallest ln|λ|): it turns slowest and grows slowest, so its whorl is the largest in the picture. Candidates: the cycle of the component beneath the landing (`root_cycle`, near-neutral when that component is close, and losing on its own when it is far), the α fixed point in closed form (the root near the main cardioid), and 2⁵ sprayed cycles of periods 1 to 2⁴.
            // The hash picks which of the cycle's own points to sit on: each is a root, mapped onto the next by f.
            let spiral = {
                // The α fixed point, z = (1 − √(1 − 4c))/2 with λ = 2z: the root near the main cardioid.
                let alpha = {
                    let s = {
                        let (ax, ay) = (1. - 4. * c.0, -4. * c.1);
                        let r = (ax * ax + ay * ay).sqrt();
                        let (sx, sy) = (((r + ax) * 0.5).sqrt(), ((r - ax) * 0.5).sqrt());
                        (sx, if ay < 0. { -sy } else { sy })
                    };
                    let z = ((1. - s.0) * 0.5, -s.1 * 0.5);
                    close_cycle(c, z, 1).map(|(w, l, g)| (w, l, g, 1))
                };
                let sprayed = (0..SPIRAL_TRIES).filter_map(|i| {
                    let period = 1 + (i & (PERIODS - 1));
                    cycle_point(c, &draw(salt, 4, (n << 5) | i, text.as_bytes()), period).map(|(w, l, g)| (w, l, g, period))
                });
                root_cycle(c, land).into_iter().chain(alpha).chain(sprayed).fold(None, |best: Option<((f64, f64), f64, f64, u32)>, e| if best.map_or(true, |b| e.1 < b.1) { Some(e) } else { best })
            };
            // α always closes for c outside the set (it repels there); if even that failed, the next try draws a new c, and after 2⁵ tries the whole of J(c), straight, stands.
            let Some((first, _, _, period)) = spiral else {
                if n >= 1 << 5 {
                    return Params { c, half: along * 1.125, radius: along * 1.125, ..base };
                }
                continue;
            };
            {
                let mut eye = first;
                for _ in 0..(t[6] as u32 % period) {
                    let ee = cmul(eye, eye);
                    eye = (ee.0 + c.0, ee.1 + c.1);
                }
                // The whorl's radius R: 2⁻¹ to 2⁻³ of J's half-extent, or larger until the f32 render resolves it.
                let mut stops = 1. + unit(&t[24..28]) * 2.;
                let radius = loop {
                    let radius = along * (-stops).exp2();
                    if radius >= MIN_HALF || stops <= 1. {
                        break radius;
                    }
                    stops -= 1.;
                };
                let turn = unit(&t[28..32]) * TAU;
                let p = Params { c, centre: eye, half: radius, radius, rot: (turn.cos(), turn.sin()), ..base };
                if framed(&p) || n >= 1 << 5 {
                    return p;
                }
            }
        }
    }
}

/// The palette: a cosine per channel, each with its own phase, in [0, 1].
#[inline]
fn palette(t: f32, phase: &[f32; 3]) -> [f32; 3] {
    let tau = TAU as f32;
    [
        0.5 + 0.5 * (tau * (t + phase[0])).cos(),
        0.5 + 0.5 * (tau * (t + phase[1])).cos(),
        0.5 + 0.5 * (tau * (t + phase[2])).cos(),
    ]
}

/// One point of the plane, as an opaque α + darkness pixel; `k` is plane units per pixel.
#[inline]
fn shade(x: f32, y: f32, v: &View, k: f32) -> [f32; 3] {
    let o = orbit(x, y, v);
    let r = (o.x * o.x + o.y * o.y).sqrt();
    let escaped = o.n < MAX_ITER;
    // Smooth iteration count.
    let nu = if escaped { o.n as f32 + 1. - r.ln().log2() } else { MAX_ITER as f32 };
    let t = match v.mode {
        0 => nu * (1. / (1 << 4) as f32),
        // The orbit's angle at escape, turned by the smooth count: cells that tile each level of the set.
        1 => o.y.atan2(o.x) / TAU as f32 + nu * (1. / (1 << 5) as f32),
        // How close the orbit came to the trap, on a log scale: rings and petals around each preimage of the trap.
        _ => -(o.trap_sq.ln()) * (1. / (1 << 3) as f32),
    };
    let col = palette(t, &v.phase);
    // Distance to the set in pixels (0 on it).
    let dist_px = if escaped { r * r.ln() / o.dm / k } else { 0. };
    // Body: within about a pixel of the set, falling smoothly (C∞) to nothing a few pixels out.
    let body = 1. / (1. + dist_px * dist_px);
    // Rim: brightest about 2¹ pixels out, a smooth bump in log distance.
    let l = (dist_px + 1. / (1 << 4) as f32).log2() - 1.;
    let rim = (-(l * l)).exp2();
    // Field: the colouring, dimmed, fading to black far from the set.
    let field = (1. - (-nu * 0.125).exp2()) * 0.375;
    let gain = body * 0.1875 + rim * 0.875 + (1. - body) * field;
    [col[0] * gain, col[1] * gain, col[2] * gain]
}

/// A visible colour in [0, 1] per channel at opacity α in [0, 1], as a fluor α + darkness pixel: darkness is premultiplied by α (an empty pixel is 0, opaque black is all ones).
#[inline]
fn pack(rgb: [f32; 3], alpha: f32) -> u32 {
    let a = (alpha * 255.) as u32;
    let d = |c: f32| ((1. - c) * alpha * 255.) as u32;
    (a << 24) | (d(rgb[0]) << 16) | (d(rgb[1]) << 8) | d(rgb[2])
}

/// Render the mark into `buf` (`w × h`, α + darkness), rows across the cores, in f32.
pub fn render(buf: &mut [u32], w: usize, h: usize, p: &Params) {
    let k = scale(p.half, w, h);
    let v = p.view();
    buf.par_chunks_mut(w).enumerate().for_each(|(py, row)| {
        for px in 0..w {
            let (x, y) = plane(&v, px, py, w, h, k);
            row[px] = pack(shade(x, y, &v, k), 1.);
        }
    });
}

/// Render the mark spherized into a disc of radius `r` pixels centred at (`cx`, `cy`) in `buf` (rows of `w`, α + darkness); pixels outside the disc are left empty (α = 0).
/// A disc point p (in units of r) shows z = eye + R·rot·p/√(1 − |p|²): the plane wrapped onto a hemisphere seen from above, its infinity at the rim.
/// The disc's own edge is anti-aliased with a C∞ step, ½ + ½·tanh(2·(r − |pixel − centre|)), about a pixel wide.
pub fn render_disc(buf: &mut [u32], w: usize, cx: f32, cy: f32, r: f32, p: &Params) {
    let v = p.view();
    let rad = p.radius as f32;
    buf.par_chunks_mut(w).enumerate().for_each(|(py, row)| {
        for px in 0..w {
            let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
            let dist = (dx * dx + dy * dy).sqrt();
            let cover = 0.5 + 0.5 * (2. * (r - dist)).tanh();
            if cover <= 0. {
                row[px] = 0;
                continue;
            }
            let (qx, qy) = (dx / r, dy / r);
            let q2 = qx * qx + qy * qy;
            // At and past the rim (inside the anti-aliasing band) the plane is at infinity: the outer field's farthest, black.
            if q2 >= 1. {
                row[px] = pack([0.; 3], cover);
                continue;
            }
            let g = rad / (1. - q2).sqrt();
            let (a, b) = (qx * g, qy * g);
            let (x, y) = (v.centre.0 + a * v.rot.0 - b * v.rot.1, v.centre.1 + a * v.rot.1 + b * v.rot.0);
            // Plane units per pixel: the map's area scale, √(radial · tangential stretch) = R/(1 − |p|²) per disc unit.
            let k = rad / (r * (1. - q2));
            row[px] = pack(shade(x, y, &v, k), cover);
        }
    });
}

/// Where this picture came from in the Mandelbrot set: the Julia set's c.
pub fn locus(p: &Params) -> (f64, f64) {
    p.c
}

/// The whole-set locator: the Mandelbrot set from Re −2.25 to 0.75 and Im −1.3 to 2.1 (high enough to show where the descent starts), fitted to the pane on both axes.
pub fn render_whole(buf: &mut [u32], w: usize, h: usize, p: &Params) {
    let (ext_w, ext_h) = (3., 3.4);
    // The larger of the two scales is the one at which both extents fit the pane; this picks a fit, it does not clamp anything.
    let k = (ext_w / w as f64).max(ext_h / h as f64);
    render_locator(buf, w, h, p, (-0.75, 0.4), k);
}

/// The near locator: a 2⁵× zoom around the picture's c, 2⁻⁴ across the pane's span.
pub fn render_near(buf: &mut [u32], w: usize, h: usize, p: &Params) {
    let span = 2. * (w * h) as f64 / (w + h) as f64;
    render_locator(buf, w, h, p, locus(p), (1. / (1 << 5) as f64) / (span * 0.5));
}

/// A locator pane at `k` plane units per pixel about `centre`, imaginary axis up, in this device's colouring: every point the descent checked as a pixel in its own random colour, and a crosshair on c.
fn render_locator(buf: &mut [u32], w: usize, h: usize, p: &Params, centre: (f64, f64), k: f64) {
    let span = 2. * (w * h) as f64 / (w + h) as f64;
    let q = Params { kind: Kind::Plane, c: (0., 0.), centre, half: k * span * 0.5, rot: (1., 0.), ..*p };
    render(buf, w, h, &q);
    // Rows run downward and the imaginary axis runs up: flip.
    let (top, bottom) = buf.split_at_mut(w * (h / 2));
    for row in 0..h / 2 {
        let b = (h - h / 2 - 1 - row) * w;
        top[row * w..(row + 1) * w].swap_with_slice(&mut bottom[b..b + w]);
    }
    let at = locus(p);
    let to_px = |c: (f64, f64)| ((c.0 - centre.0) / k + w as f64 * 0.5 - 0.5, h as f64 * 0.5 - 0.5 - (c.1 - centre.1) / k);
    // WHY/PROOF: the points come from the plane and can lie off this pane or on its edge; a write outside it would land in the neighbouring pane's rows or past the buffer.
    let mut put = |x: f64, y: f64, colour: u32| {
        if x >= 0. && y >= 0. && (x as usize) < w && (y as usize) < h {
            buf[y as usize * w + x as usize] = colour;
        }
    };
    let (_, _, trail) = descend(at.0);
    for (i, &pt) in trail.iter().enumerate() {
        let hsh = blake3::hash(&(i as u32).to_le_bytes());
        let b = hsh.as_bytes();
        let (x, y) = to_px(pt);
        put(x, y, pack_argb(b[0] | (1 << 6), b[1] | (1 << 6), b[2] | (1 << 6), 255));
    }
    let (mx, my) = to_px(at);
    let white = pack_argb(255, 255, 255, 255);
    // Crosshair arms from 2² to 2⁴ pixels out, leaving the point itself clear.
    for i in (1 << 2)..(1 << 4) {
        let f = i as f64;
        put(mx + f, my, white);
        put(mx - f, my, white);
        put(mx, my + f, white);
        put(mx, my - f, white);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stand-in device draws a picture with structure, and a change in the text changes it.
    #[test]
    fn pictures_have_structure_and_follow_the_text() {
        let (w, h) = (1 << 7, 1 << 8);
        for device in 0u32..(1 << 3) {
            let salt = *blake3::hash(&device.to_le_bytes()).as_bytes();
            let mut a = vec![0u32; w * h];
            let mut b = vec![0u32; w * h];
            render(&mut a, w, h, &Params::new(&salt, "ab"));
            render(&mut b, w, h, &Params::new(&salt, "abc"));
            let mut seen: Vec<u32> = a.clone();
            seen.sort_unstable();
            seen.dedup();
            assert!(seen.len() > 1 << 6, "device {device}: only {} distinct pixels", seen.len());
            assert!(a != b, "device {device}: the text did not change the picture");
        }
    }

    /// A spiral's eye really is periodic: f^p carries it back onto itself.
    #[test]
    fn a_spiral_eye_is_periodic() {
        let c = (-0.8, 0.2);
        let bits = *blake3::hash(b"eye").as_bytes();
        for p in 1..=PERIODS {
            let (eye, _, _) = cycle_point(c, &bits, p).expect("Newton closes a cycle from the repeated address");
            let mut z = eye;
            for _ in 0..p {
                let zz = cmul(z, z);
                z = (zz.0 + c.0, zz.1 + c.1);
            }
            assert!((z.0 - eye.0).abs() + (z.1 - eye.1).abs() < 1. / (1 << 16) as f64, "period {p}: f^p(eye) − eye = {:?}", (z.0 - eye.0, z.1 - eye.1));
        }
    }


}
