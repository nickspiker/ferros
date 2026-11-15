//! The camera's geometry, kept apart from the map's state so the host tests it (tools/deck-fit): the deck's fit at a turn and the turn that fits largest (WINDOWS.md "Deck's turn"), and a flight's one motion, the spiral about the one screen point its start and its end show alike.
//! f64 and libm only, no state, no allocation.

use core::f64::consts::PI;

/// The most circles a fit takes (the map's `MAX`).
pub const MAX: usize = 1 << 4;

/// Golden-section steps in the fit's centre search, each keeping 0.618 of the bracket: 2⁵ steps leave 2⁻²² of the circles' extent, a thousandth of a pixel on any screen.
const FIT_STEPS: usize = 1 << 5;

/// The deck swipe's turn search (`best_turn`): turns sampled across a half turn, then golden-section steps between a sample's neighbours (each step keeps 0.618 of the bracket: 2⁵ steps take the two samples either side, under six degrees, to about a millionth of one).
pub const TURN_SAMPLES: usize = 1 << 6;
const TURN_REFINE: usize = 1 << 5;

/// Two turns fit alike when their zooms differ by under this fraction (2⁻¹², a quarter of a pixel across the widest screen): the swipe takes the one nearer the camera's own turn, so a layout that fits as large at many turns does not spin past the first of them.
pub const TURN_TIE: f64 = 1. / ((1 << 12) as f64);

/// (√5 − 1) / 2, the golden section's kept fraction.
const GOLD: f64 = 0.618_033_988_749_894_9;

/// Where the deck's circles must land, in panel pixels from the fit's middle: within half the screen's width across (`half_w`), and within the big circle (ferros_sys::active_circle) of its centre (`radius`), so none lands past an edge or under the console in a top corner.
#[derive(Clone, Copy)]
pub struct Room {
    pub half_w: f64,
    pub radius: f64,
}

/// One circle: its world centre and radius, all ×256 (world units ×256, as the map keeps positions).
pub type Disc = (f64, f64, f64);

/// The least of `f` on [lo, hi] by golden sections: where, and its value. `f` must be convex there (each use says why it is).
fn golden_min(mut lo: f64, mut hi: f64, f: impl Fn(f64) -> f64) -> (f64, f64) {
    let (mut p, mut q) = (hi - GOLD * (hi - lo), lo + GOLD * (hi - lo));
    let (mut fp, mut fq) = (f(p), f(q));
    for _ in 0..FIT_STEPS {
        if fp > fq {
            lo = p;
            p = q;
            fp = fq;
            q = lo + GOLD * (hi - lo);
            fq = f(q);
        } else {
            hi = q;
            q = p;
            fq = fp;
            p = hi - GOLD * (hi - lo);
            fp = f(p);
        }
    }
    if fp < fq { (p, fp) } else { (q, fq) }
}

/// The deck's fit at the turn (c, s) (the map's Q14 turn over its one, as `Cam::screen` turns: u = c·x − s·y, v = s·x + c·y): the largest zoom at which every circle is in the room, and where the room's centre stands in the world then (×256). The zoom is Q16 pixels a world unit, before the map's zoom limits.
///
/// In the turned frame, with the room's centre on (cu, cv), the circles need a scale (world ×256 a pixel) of at least G = max over the circles of (|u − cu| + r) / half_w and (√((u − cu)² + (v − cv)²) + r) / radius; the zoom is 2²⁴ / G.
/// G is a maximum of convex functions of the centre, so convex: its least is found exactly, not guessed.
/// Across, the width's part is least at the circles' middle (`um`), where it is half their extent over half the width, and no centre does better; when the big circle's part is no more there at the middle of their extent both ways (`vm`), that is the fit, as it was before 2026-11-15.
/// Else the big circle binds (a layout tall in the turned frame, or lopsided: a big circle at one end, small ones fanned at the other) and the middle of the extent is not the best centre: up to 3.7% of zoom lost over 200 random layouts, a quarter of them short (tools/deck-fit).
/// The centre is then searched: across, the least over cu of max(width's part, the big circle's least over cv), each convex, so golden sections nested; the least lies between the two parts' own leasts, both within the circles' extent.
pub fn fit_at(discs: &[Disc], c: f64, s: f64, room: Room) -> (f64, f64, f64) {
    let mut t = [(0f64, 0f64, 0f64); MAX];
    let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (i, &(x, y, r)) in discs.iter().enumerate() {
        let (u, v) = (c * x - s * y, s * x + c * y);
        t[i] = (u, v, r);
        u0 = u0.min(u - r);
        v0 = v0.min(v - r);
        u1 = u1.max(u + r);
        v1 = v1.max(v + r);
    }
    let t = &t[..discs.len()];
    let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
    // The width's part: the farthest reach across, from either end of the extent.
    let across = |cu: f64| (u1 - cu).max(cu - u0) / room.half_w;
    // The big circle's part: the farthest reach from the centre.
    let round = |cu: f64, cv: f64| t.iter().fold(0f64, |g, &(u, v, r)| g.max((libm::sqrt((u - cu) * (u - cu) + (v - cv) * (v - cv)) + r) / room.radius));
    let least_across = across(um);
    let at_middle = least_across.max(round(um, vm));
    let (cu, cv, g) = if round(um, vm) <= least_across {
        (um, vm, least_across)
    } else {
        // For a fixed cu, the big circle's part is convex in cv and least within the circles' extent (outside it every reach grows outward).
        let best_v = |cu: f64| golden_min(v0, v1, |cv| round(cu, cv));
        let (cu, _) = golden_min(u0, u1, |cu| across(cu).max(best_v(cu).1));
        let (cv, r) = best_v(cu);
        let g = across(cu).max(r);
        // The search ends within a thousandth of a pixel; where the extent's middle was as good, it stays (no drift for nothing).
        if g < at_middle { (cu, cv, g) } else { (um, vm, at_middle) }
    };
    // The centre back into the world: the turn undone (its inverse, so a Q14 turn a hair off unit length lands exactly).
    let d = c * c + s * s;
    ((c * cu + s * cv) / d, (c * cv - s * cu) / d, ((1u64 << 24) as f64) / g)
}

/// The deck swipe's turn: the turn whose fit (`fit`, a zoom for a turn in radians) is largest, the nearest to the camera's own (`a0`) of those that fit alike (`TURN_TIE`), as `reach` sees each zoom (the map's zoom limit as a soft minimum, so a turn that only gains past the limit gains nothing).
/// The fit has period a half turn (turning by π mirrors the circles thru the fit's middle, which fits alike) and is continuous and piecewise smooth.
/// A bounded search, no timer: `TURN_SAMPLES` turns across the half turn centred on `a0` (so of each pair a half turn apart the nearer is taken, and the flight spins the short way), golden-section steps between each sample peak's neighbours for the best zoom; then the camera's own turn if it fits alike, else of the refined peaks that fit alike the one nearest it.
/// Returns the turn, its zoom, and the best zoom found.
pub fn best_turn(a0: f64, fit: impl Fn(f64) -> f64, reach: impl Fn(f64) -> f64) -> (f64, f64, f64) {
    let step = PI / TURN_SAMPLES as f64;
    let at = |j: usize| a0 - PI * 0.5 + step * (j as f64 + 0.5);
    let mut z = [0f64; TURN_SAMPLES];
    let mut top = 0;
    for j in 0..TURN_SAMPLES {
        z[j] = fit(at(j));
        if z[j] > z[top] {
            top = j;
        }
    }
    // A sample's neighbourhood, refined: the largest of the sample and the golden section's last two (the fit is unimodal at that scale: each sample is a 64th of a half turn, under three degrees).
    let refine = |j: usize| {
        let (mut lo, mut hi) = (at(j) - step, at(j) + step);
        let (mut p, mut q) = (hi - GOLD * (hi - lo), lo + GOLD * (hi - lo));
        let (mut zp, mut zq) = (fit(p), fit(q));
        for _ in 0..TURN_REFINE {
            if zp < zq {
                lo = p;
                p = q;
                zp = zq;
                q = lo + GOLD * (hi - lo);
                zq = fit(q);
            } else {
                hi = q;
                q = p;
                zq = zp;
                p = hi - GOLD * (hi - lo);
                zp = fit(p);
            }
        }
        let (a, za) = if zp > zq { (p, zp) } else { (q, zq) };
        if za > z[j] { (a, za) } else { (at(j), z[j]) }
    };
    // The samples' peaks (as large as both neighbours, round the half turn) within a sixteenth of the largest sample, each refined: a peak narrower than the samples' spacing can top the largest sample's once refined (on random layouts by up to a twentieth of a thousandth, tools/deck-fit).
    let n = TURN_SAMPLES;
    let mut peak = [(0f64, 0f64); TURN_SAMPLES];
    let mut is_peak = [false; TURN_SAMPLES];
    let (mut best_a, mut best_z) = (at(top), z[top]);
    for j in 0..n {
        if z[j] >= z[(j + n - 1) % n] && z[j] >= z[(j + 1) % n] && z[j] >= z[top] * (1. - PEAK_NEAR) {
            peak[j] = refine(j);
            is_peak[j] = true;
            if peak[j].1 > best_z {
                (best_a, best_z) = peak[j];
            }
        }
    }
    let enough = reach(best_z) * (1. - TURN_TIE);
    let z0 = fit(a0);
    if reach(z0) >= enough {
        return (a0, z0, best_z);
    }
    // Nearest the camera's own turn first: sample j sits (j + ½ − N/2) steps from it.
    for k in 0..n / 2 {
        for j in [n / 2 - 1 - k, n / 2 + k] {
            if is_peak[j] && reach(peak[j].1) >= enough {
                return (peak[j].0, peak[j].1, best_z);
            }
        }
    }
    (best_a, best_z, best_z)
}

/// The peaks `best_turn` refines: those within this fraction (a sixteenth) of the largest sample.
const PEAK_NEAR: f64 = 1. / ((1 << 4) as f64);

/// The critically damped spring at rate `w`, `t` display frames after it started at offset `x0` with speed `v0` (a frame): x(t) = (x0 + (v0 + w·x0)·t)·e^(−w·t), its offset and speed then.
pub fn spring_at(x0: f64, v0: f64, t: i32, w: f64) -> (f64, f64) {
    let t = t as f64;
    let e = libm::exp(-w * t);
    let a = v0 + w * x0;
    ((x0 + a * t) * e, (v0 - w * a * t) * e)
}

fn mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// e^(−w) − 1 for the complex w = l + iθ, exact near nought: e^(−l)·cos θ − 1 = (e^(−l) − 1)·cos θ − 2·sin²(θ/2).
fn em1(l: f64, a: f64) -> (f64, f64) {
    let e = libm::expm1(-l);
    let h = libm::sin(a * 0.5);
    (e * libm::cos(a) - 2. * h * h, -(e + 1.) * libm::sin(a))
}

/// Under this |e^(−w) − 1| (2⁻⁴⁰) a flight has no zoom or turn to go and its centre rides its own spring: the spiral it would otherwise take differs from that line by under 2⁻⁴⁰ of the way.
const NO_SPIRAL: f64 = 1. / ((1u64 << 40) as f64);

/// A flight's one motion (owner, 2026-11-15: "the spin to fit needs to be one motion ... both at same rate"; "maybe the spin is correct but it's the centerpoint").
/// A camera is a similarity of the plane: the world point under a screen offset f is x + f·q, with q = e^(−(ln z + iθ)) (the turn and the zoom as one complex number, the plane as complex numbers).
/// From any camera to any other there is one screen point both show the same world point under, unless only the centre moves: the spiral's pivot, f = (x₁ − x₀) / (q₀ − q₁).
/// The world point under it (P) stays put while w = ln z + iθ springs to the target's, so every point on the glass turns and grows about it, the turn and the zoom one motion at one rate; the centre is then x = P − f·q, which is the target's plus the pivot's term k·(e^(−w) − 1), w now the offset from the target and k = −f·q₁ = (x₀ − x₁) / (e^(−w₀) − 1).
/// P keeps a spring of its own, nought from rest, so a flight turned round mid-way (a second tap) keeps the speed the camera has: that speed is P's plus the pivot term's.
#[derive(Clone, Copy)]
pub struct Spiral {
    /// The pivot term's factor (world ×256, complex).
    pub k: (f64, f64),
}

impl Spiral {
    /// The spiral for a flight starting with the centre at `d` from the target (world ×256) moving at `dv` (a frame), and w = ln z + iθ at (l, a) from the target's moving at (lv, av); and P's spring then, its offset from where it lands and its speed, x and y (nought from rest).
    pub fn start(d: (f64, f64), dv: (f64, f64), l: f64, a: f64, lv: f64, av: f64) -> (Spiral, (f64, f64), (f64, f64)) {
        let e = em1(l, a);
        let m = e.0 * e.0 + e.1 * e.1;
        // No zoom or turn to go: no pivot, the centre on P's spring alone.
        if m < NO_SPIRAL * NO_SPIRAL {
            return (Spiral { k: (0., 0.) }, d, dv);
        }
        let sp = Spiral { k: mul(d, (e.0 / m, -e.1 / m)) };
        let (o, v) = sp.centre((0., 0.), (0., 0.), (l, lv), (a, av));
        (sp, (d.0 - o.0, d.1 - o.1), (dv.0 - v.0, dv.1 - v.1))
    }

    /// The centre's offset from the target and its speed, with P's spring at (offset, speed) for x and y and w's at `l` and `a` (offset, speed): P plus k·(e^(−w) − 1), whose speed is −k·e^(−w)·ẇ.
    pub fn centre(&self, px: (f64, f64), py: (f64, f64), l: (f64, f64), a: (f64, f64)) -> ((f64, f64), (f64, f64)) {
        let e = em1(l.0, a.0);
        let o = mul(self.k, e);
        let v = mul(mul(self.k, (e.0 + 1., e.1)), (l.1, a.1));
        ((px.0 + o.0, py.0 + o.1), (px.1 - v.0, py.1 - v.1))
    }
}
