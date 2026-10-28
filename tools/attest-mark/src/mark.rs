//! The mark: a view into an escape-time fractal, chosen by the device's salt and moved by what is in the textbox.
//!
//! The salt (on the phone, the device's own, sealed under its ira; here a stand-in) fixes the device's look: the family, the colouring method, the palette and the family's own seed.
//! Only the quadratic map z² + c, in three families:
//! 0. Julia sets with c chosen symbolically: a bulb p/q on the main cardioid (q from 2 to 2⁴; J(c) near it spirals with q arms), sometimes a sub-bulb p′/q′ of it (spirals of spirals), and a point just OUTSIDE that disc, so J(c) is a Cantor dust with no inside to go flat; bulbs are taken as discs of radius sin(πp/q)/q², sub-bulbs at angle 2πp′/q′ from the root;
//! 1. Julia sets with c from the descent (below), a little above the boundary point it lands on — near the boundary J(c) resembles the Mandelbrot set around c, so the two rhyme;
//! 2. zooms into the Mandelbrot set itself, centred on the descent's landing point.
//!
//! The descent: pick x in [−2, 0.375] and come straight down the line Re c = x from Im c = 2, each step half the distance estimate (it never overshoots), until within 2⁻¹² of the set — the set's skyline seen from above, one dial from the antenna through the bulb tops and seahorse valley to the cusp.
//! For x past the cusp (0.25) the line misses the set and lands on the real axis just outside it, in the parabolic-implosion region, which is kept for the Julia family.
//!
//! The textbox changes both the shape and the view, so every key looks new while the device's look stays:
//! in family 0 the key moves c around the device's bulb (the angle away from its root and how far outside), and the view lands on J(c) by its symbolic address: for c outside the Mandelbrot set, z² + c on J(c) is conjugate to the full shift on two symbols, so a hash read as a 256-bit address names one point of J, built by inverse iteration (z → ±√(z − c)) with the coarsest bit applied last; uniform bits give the balanced measure (Brolin), so every key lands independently over the whole set;
//! in family 1 the key nudges the descent's x and sets how far above the boundary c sits, and the view lands by a spray: 2⁶ points drawn from the key's hash across the region, and the one that escapes latest, the one nearest the boundary, is the centre;
//! in family 2 the key nudges x and picks the zoom depth, and a fine spray at that zoom picks the centre.
//! Every family then takes a log-uniform zoom, a rotation and a Gaussian offset of the centre from the key, and a thumbnail rejects framings that show too little of the set, redrawing with the next counter.
//!
//! Shading: the distance estimate (|z|·ln|z| over the derivative's magnitude, in pixels) separates the set from the field around it, because near the set the escape count changes chaotically from pixel to pixel and reads as static.
//! The set's body is dark with the colouring faintly inside it, its edge is a luminous rim a few pixels wide, and the field outside is the colouring method, dimmed and fading to black far out.
//! The colouring method is one of three: smooth iteration bands, the angle of the orbit at escape (a cellular mosaic), or an orbit trap (distance to a hash-chosen point: rings and petals).
//! Nothing compares the text against anything, so the picture says nothing about whether the text is right.

use fluor::paint::pack_argb;
use rayon::prelude::*;
use std::f64::consts::{PI, TAU};

/// Iteration cap: deep enough for the zooms used here.
const MAX_ITER: u32 = 1 << 10;
/// Escape when |z|² passes 2⁸ (|z| = 16), far enough out for smooth colouring.
const BAIL_SQ: f64 = (1 << 8) as f64;
/// A jump of 2² iterations between neighbouring thumbnail pixels marks an edge of the set.
const JUMP: i32 = 1 << 2;
/// A framing is kept when at least a quarter of the thumbnail is edge.
const DETAIL_MIN: f64 = 1. / (1 << 2) as f64;
/// Points in a landing spray.
const SPRAY: u32 = 1 << 6;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Julia set of z^power + c.
    Julia,
    /// The parameter plane of z^power + c, z starting at 0.
    Plane,
}

/// Everything one picture needs, in the plane's terms.
#[derive(Clone, Copy)]
pub struct Params {
    kind: Kind,
    power: u32,
    c: (f64, f64),
    centre: (f64, f64),
    /// Half the window's span, in the plane.
    half: f64,
    /// The view's rotation as (cos, sin).
    rot: (f64, f64),
    /// Per-channel palette phases.
    phase: [f64; 3],
    /// Colouring method: 0 smooth bands, 1 escape angle, 2 orbit trap.
    mode: u8,
    /// The orbit trap's point, for mode 2.
    trap: (f64, f64),
}

/// 32 bits of a hash as a fraction in [0, 1).
fn unit(b: &[u8]) -> f64 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / (1u64 << 32) as f64
}

/// A keyed BLAKE3 of the salt over a domain byte, a counter and any extra bytes.
fn draw(salt: &[u8; 32], domain: u8, counter: u32, extra: &[u8]) -> [u8; 32] {
    *blake3::Hasher::new_keyed(salt).update(&[domain]).update(&counter.to_le_bytes()).update(extra).finalize().as_bytes()
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// A numerator p coprime to q, chosen by `h`.
fn coprime(q: u32, h: u8) -> u32 {
    let ps: Vec<u32> = (1..q).filter(|&p| gcd(p, q) == 1).collect();
    ps[h as usize % ps.len()]
}

fn rotate(v: (f64, f64), a: f64) -> (f64, f64) {
    let (c, s) = (a.cos(), a.sin());
    (v.0 * c - v.1 * s, v.0 * s + v.1 * c)
}

fn cmul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// z^n for a small whole n, by repeated multiplication.
fn cpow(z: (f64, f64), n: u32) -> (f64, f64) {
    let mut r = (1., 0.);
    for _ in 0..n {
        r = cmul(r, z);
    }
    r
}

/// A standard normal pair from two uniforms (Box–Muller).
fn gauss(u0: f64, u1: f64) -> (f64, f64) {
    let r = (-2. * (1. - u0).ln()).sqrt();
    (r * (TAU * u1).cos(), r * (TAU * u1).sin())
}

/// What one orbit did: iterations until escape (MAX_ITER if it never did), z at escape, the derivative's magnitude, and the closest approach to the trap point (squared).
struct Orbit {
    n: u32,
    x: f64,
    y: f64,
    dm: f64,
    trap_sq: f64,
}

/// Iterate the family's map from the plane point (px, py).
/// The derivative is tracked by magnitude only (|dz| ← d·|z|^(d−1)·|dz|, plus 1 in the parameter plane), which is all the distance estimate needs.
/// It can overflow to infinity near the set, which the distance estimate reads, correctly, as "on the set".
#[inline]
fn orbit(px: f64, py: f64, p: &Params) -> Orbit {
    let (mut z, c, plus) = match p.kind {
        Kind::Plane => ((0., 0.), (px, py), 1.),
        _ => ((px, py), p.c, 0.),
    };
    let d = p.power as f64;
    let mut dm = if p.kind == Kind::Plane { 0. } else { 1. };
    let mut trap_sq = f64::MAX;
    for n in 0..MAX_ITER {
        let r2 = z.0 * z.0 + z.1 * z.1;
        if r2 > BAIL_SQ {
            return Orbit { n, x: z.0, y: z.1, dm, trap_sq };
        }
        let (tx, ty) = (z.0 - p.trap.0, z.1 - p.trap.1);
        trap_sq = trap_sq.min(tx * tx + ty * ty);
        dm = d * r2.sqrt().powi(p.power as i32 - 1) * dm + plus;
        let zp = cpow(z, p.power);
        z = (zp.0 + c.0, zp.1 + c.1);
    }
    Orbit { n: MAX_ITER, x: z.0, y: z.1, dm, trap_sq }
}

/// The plane point under pixel (px, py) of a `w × h` window, `k` plane units per pixel.
#[inline]
fn plane(p: &Params, px: usize, py: usize, w: usize, h: usize, k: f64) -> (f64, f64) {
    let (u, v) = ((px as f64 + 0.5 - w as f64 * 0.5) * k, (py as f64 + 0.5 - h as f64 * 0.5) * k);
    (p.centre.0 + u * p.rot.0 - v * p.rot.1, p.centre.1 + u * p.rot.1 + v * p.rot.0)
}

/// Plane units per pixel for a `w × h` window: half the span covers `half`.
fn scale(p: &Params, w: usize, h: usize) -> f64 {
    let span = 2. * (w * h) as f64 / (w + h) as f64;
    p.half / (span * 0.5)
}

/// How much edge a 2⁵ × 2⁶ thumbnail shows: the share of pixels whose iteration count jumps by at least 2² to the neighbour on the right or below.
/// Only filaments of the set make such jumps; the smooth field between them makes none, so the score reads "how much of the view is set" at every zoom.
fn detail(p: &Params) -> f64 {
    const W: usize = 1 << 5;
    const H: usize = 1 << 6;
    let k = scale(p, W, H);
    let mut n = [0i32; W * H];
    for py in 0..H {
        for px in 0..W {
            let (x, y) = plane(p, px, py, W, H, k);
            n[py * W + px] = orbit(x, y, p).n as i32;
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

/// The point of the quadratic J(c) whose address is `bits`: inverse iteration applies the branches finest first, so `bits[0]`'s low bit, applied last, picks the half of J.
fn address_point(c: (f64, f64), bits: &[u8; 32]) -> (f64, f64) {
    let (mut x, mut y) = (0.5f64, 0.);
    for i in (0..(1 << 8)).rev() {
        let (ax, ay) = (x - c.0, y - c.1);
        let r = (ax * ax + ay * ay).sqrt();
        let (mut sx, mut sy) = (((r + ax) * 0.5).sqrt(), ((r - ax) * 0.5).sqrt());
        if ay < 0. {
            sy = -sy;
        }
        if (bits[i >> 3] >> (i & 7)) & 1 == 1 {
            (sx, sy) = (-sx, -sy);
        }
        (x, y) = (sx, sy);
    }
    (x, y)
}

/// The spray: SPRAY points drawn from `seed` uniformly in the square of half-side `half` about `centre`; the one that escapes latest (nearest the boundary) wins. Points that never escape are inside and do not count.
fn spray(p: &Params, seed: &[u8; 32], centre: (f64, f64), half: f64) -> (f64, f64) {
    let mut bytes = [0u8; (SPRAY as usize) << 3];
    blake3::Hasher::new_keyed(seed).update(b"spray").finalize_xof().fill(&mut bytes);
    let mut best = (centre, 0u32);
    for i in 0..SPRAY as usize {
        let b = &bytes[i << 3..(i + 1) << 3];
        let pt = (centre.0 + (unit(&b[0..4]) * 2. - 1.) * half, centre.1 + (unit(&b[4..8]) * 2. - 1.) * half);
        let n = orbit(pt.0, pt.1, p).n;
        if n < MAX_ITER && n >= best.1 {
            best = (pt, n);
        }
    }
    best.0
}

/// The device's quadratic bulb: centre, radius and the unit direction from its centre toward its root.
fn bulb(salt: &[u8; 32], counter: u32) -> ((f64, f64), f64, (f64, f64)) {
    let h = draw(salt, 0, counter, &[]);
    // The p/q bulb's root on the cardioid c(t) = e^{it}/2 − e^{2it}/4, and the outward normal there (the tangent turned a quarter clockwise).
    let q = 2 + (h[0] % 15) as u32;
    let p = coprime(q, h[1]);
    let t = TAU * p as f64 / q as f64;
    let root = (t.cos() * 0.5 - (2. * t).cos() * 0.25, t.sin() * 0.5 - (2. * t).sin() * 0.25);
    let tangent = (-(t.sin()) * 0.5 + (2. * t).sin() * 0.5, t.cos() * 0.5 - (2. * t).cos() * 0.5);
    let tl = (tangent.0 * tangent.0 + tangent.1 * tangent.1).sqrt();
    let normal = (tangent.1 / tl, -tangent.0 / tl);
    let mut radius = (PI * p as f64 / q as f64).sin() / (q * q) as f64;
    let mut centre = (root.0 + normal.0 * radius, root.1 + normal.1 * radius);
    let mut to_root = (-normal.0, -normal.1);
    if h[2] & 1 == 1 {
        let q2 = 2 + (h[3] % 7) as u32;
        let p2 = coprime(q2, h[4]);
        let dir = rotate(to_root, TAU * p2 as f64 / q2 as f64);
        let sub_root = (centre.0 + dir.0 * radius, centre.1 + dir.1 * radius);
        radius *= (PI * p2 as f64 / q2 as f64).sin() / (q2 * q2) as f64;
        centre = (sub_root.0 + dir.0 * radius, sub_root.1 + dir.1 * radius);
        to_root = (-dir.0, -dir.1);
    }
    (centre, radius, to_root)
}

/// A point just outside the bulb, within three eighths of a turn of straight away from its root, by 2⁻² to 2⁻⁶ of its radius, both chosen by `t`.
fn outside_bulb(b: ((f64, f64), f64, (f64, f64)), t: &[u8; 32]) -> (f64, f64) {
    let (centre, radius, to_root) = b;
    let away = rotate((-to_root.0, -to_root.1), (unit(&t[0..4]) - 0.5) * TAU * 0.75);
    let out = radius * (1. + (-(2. + unit(&t[4..8]) * 4.)).exp2());
    (centre.0 + away.0 * out, centre.1 + away.1 * out)
}

/// The descent: down the line Re c = x from Im c = 2 in the parameter plane of z² + c, until within 2⁻¹² of the set or on the real axis; at most 2⁹ steps.
/// Each step is the PROVEN lower bound on the distance to the set (Koebe, via Milnor): with G = ln|z|/2ⁿ the Green's function and d = |z|·ln|z|/|dc| the estimate (= G/|G′|), the true distance is at least sinh G / (2·e^G·|G′|) = d·(1 − e^(−2G))/(4G).
/// That factor is ½ next to the set and smaller far out (about 0.22 at G = 1), so the step never jumps past anything, not even a hair-thin filament, and the descent stops at the first, outermost contact along the line.
/// Returns the landing point and whether it is on the set (false when the line missed it and reached the axis).
fn descend(x: f64, power: u32) -> ((f64, f64), bool) {
    let probe = Params { kind: Kind::Plane, power, c: (0., 0.), centre: (0., 0.), half: 2., rot: (1., 0.), phase: [0.; 3], mode: 0, trap: (0., 0.) };
    let mut y = 2.;
    for _ in 0..(1 << 9) {
        let o = orbit(x, y, &probe);
        if o.n == MAX_ITER {
            break;
        }
        let r = (o.x * o.x + o.y * o.y).sqrt();
        let d = r * r.ln() / o.dm;
        if d < 1. / (1 << 12) as f64 {
            break;
        }
        // G underflows to 0 for orbits longer than about 2¹⁰ steps, where the factor's limit, ½, is exact enough.
        let g = r.ln() * (-(o.n as f64)).exp2();
        let factor = if g > 0. { -(-2. * g).exp_m1() / (4. * g) } else { 0.5 };
        y -= d * factor;
        if y <= 0. {
            return ((x, 0.), false);
        }
    }
    ((x, y), true)
}

impl Params {
    /// The picture for this device salt and this textbox content.
    pub fn new(salt: &[u8; 32], text: &str) -> Self {
        let f = draw(salt, 1, 0, &[]);
        let phase = [unit(&f[0..4]), unit(&f[4..8]), unit(&f[8..12])];
        let mode = f[12] % 3;
        let trap = ((unit(&f[16..20]) - 0.5) * 2., (unit(&f[20..24]) - 0.5) * 2.);
        let family = f[13] % 3;
        let base = Params { kind: Kind::Julia, power: 2, c: (0., 0.), centre: (0., 0.), half: 2., rot: (1., 0.), phase, mode, trap };

        // Family 0's device bulb: the first whose J(c), at the bulb's central exit, shows enough of the set at 2³× zoom in at least half of 2² trial landings (a thin dendrite fails at every zoom and is passed over).
        let device_bulb = || {
            let mut counter = 0;
            loop {
                let b = bulb(salt, counter);
                counter += 1;
                let c = outside_bulb(b, &[0x80; 32]);
                let probe = Params { c, half: 2. / (1 << 3) as f64, ..base };
                let good = (0..(1u32 << 2)).filter(|&n| framed(&Params { centre: address_point(c, &draw(salt, 4, n, &[])), ..probe })).count();
                if good >= 1 << 1 {
                    return b;
                }
            }
        };
        // Families 1 and 2: the descent's x for this device. The Julia family takes the whole span, misses past the cusp included; a zoom needs a line that hits the set, so its x is redrawn until the descent lands.
        let x0 = if family == 2 {
            let mut counter = 0;
            loop {
                let h = draw(salt, 7, counter, &[]);
                counter += 1;
                let x = -2. + unit(&h[0..4]) * 2.375;
                if descend(x, 2).1 {
                    break x;
                }
            }
        } else {
            -2. + unit(&f[25..29]) * 2.375
        };
        let bulb_b = if family == 0 { Some(device_bulb()) } else { None };
        let fixed = match family {
            0 => base,
            1 => base,
            _ => Params { kind: Kind::Plane, ..base },
        };

        // Each try draws a fresh shape and view from (salt, text, counter); the first framing that shows enough of the set is taken, and after 2⁵ tries the last one stands.
        let mut n = 0;
        loop {
            let a = draw(salt, 2, n, text.as_bytes());
            let t = draw(salt, 3, n, text.as_bytes());
            n += 1;
            let mut p = fixed;
            let zoom_stops = if family == 2 { 2. + unit(&t[0..4]) * 10. } else { unit(&t[0..4]) * 6. };
            p.half = 2. / zoom_stops.exp2();
            let point = match family {
                0 => {
                    p.c = outside_bulb(bulb_b.expect("family 0 has a bulb"), &a);
                    address_point(p.c, &a)
                }
                // c a little above the descent's landing point: 2^−(4..10) on up the line it came down.
                1 => {
                    let (b, _) = descend(x0 + (unit(&a[8..12]) - 0.5) / (1 << 5) as f64, 2);
                    p.c = (b.0, b.1 + (-(4. + unit(&a[12..16]) * 6.)).exp2());
                    spray(&p, &a, (0., 0.), 2.)
                }
                // The parameter plane: the descent's landing point, then a fine spray at the zoom about it.
                _ => {
                    let (b, _) = descend(x0 + (unit(&a[8..12]) - 0.5) / (1 << 5) as f64, p.power);
                    spray(&p, &draw(salt, 5, n, text.as_bytes()), b, p.half)
                }
            };
            let turn = unit(&t[4..8]) * TAU;
            let (gx, gy) = gauss(unit(&t[8..12]), unit(&t[12..16]));
            let sigma = p.half * 0.25;
            p.centre = (point.0 + gx * sigma, point.1 + gy * sigma);
            p.rot = (turn.cos(), turn.sin());
            if framed(&p) || n >= 1 << 5 {
                return p;
            }
        }
    }
}

/// The palette: a cosine per channel, each with its own phase, in [0, 1].
fn palette(t: f64, phase: &[f64; 3]) -> [f64; 3] {
    [
        0.5 + 0.5 * (TAU * (t + phase[0])).cos(),
        0.5 + 0.5 * (TAU * (t + phase[1])).cos(),
        0.5 + 0.5 * (TAU * (t + phase[2])).cos(),
    ]
}

/// One point of the plane, as an opaque α + darkness pixel; `k` is plane units per pixel.
#[inline]
fn shade(x: f64, y: f64, p: &Params, k: f64) -> u32 {
    let o = orbit(x, y, p);
    let r = (o.x * o.x + o.y * o.y).sqrt();
    let escaped = o.n < MAX_ITER;
    // Smooth iteration count for a map of degree d.
    let nu = if escaped { o.n as f64 + 1. - r.ln().ln() / (p.power as f64).ln() } else { MAX_ITER as f64 };
    let t = match p.mode {
        0 => nu * (1. / (1 << 4) as f64),
        // The orbit's angle at escape, turned by the smooth count: cells that tile each level of the set.
        1 => o.y.atan2(o.x) / TAU + nu * (1. / (1 << 5) as f64),
        // How close the orbit came to the trap, on a log scale: rings and petals around each preimage of the trap.
        _ => -(o.trap_sq.ln()) * (1. / (1 << 3) as f64),
    };
    let col = palette(t, &p.phase);
    // Distance to the set in pixels (0 on it).
    let dist_px = if escaped { r * r.ln() / o.dm / k } else { 0. };
    // Body: within about a pixel of the set, falling smoothly (C∞) to nothing a few pixels out.
    let body = 1. / (1. + dist_px * dist_px);
    // Rim: brightest about 2¹ pixels out, a smooth bump in log distance.
    let l = (dist_px + 1. / (1 << 4) as f64).log2() - 1.;
    let rim = (-(l * l)).exp2();
    // Field: the colouring, dimmed, fading to black far from the set.
    let field = (1. - (-nu * 0.125).exp2()) * 0.375;
    let ch = |i: usize| (col[i] * (body * 0.1875 + rim * 0.875 + (1. - body) * field) * 255.) as u8;
    pack_argb(ch(0), ch(1), ch(2), 255)
}

/// Render the mark into `buf` (`w × h`, α + darkness), rows across the cores.
pub fn render(buf: &mut [u32], w: usize, h: usize, p: &Params) {
    let k = scale(p, w, h);
    let p = *p;
    buf.par_chunks_mut(w).enumerate().for_each(|(py, row)| {
        for px in 0..w {
            let (x, y) = plane(&p, px, py, w, h, k);
            row[px] = shade(x, y, &p, k);
        }
    });
}

/// Where this picture came from in the Mandelbrot set: the c of a Julia family, or the centre of a Mandelbrot zoom; and, for a zoom, its window's half-span.
pub fn locus(p: &Params) -> ((f64, f64), Option<f64>) {
    if p.kind == Kind::Plane { (p.centre, Some(p.half)) } else { (p.c, None) }
}

/// A locator pane: the Mandelbrot set around `centre` (half-span `half`), in this device's colouring, with a crosshair on the picture's locus and, for a zoom, a ring the size of its window.
pub fn render_locator(buf: &mut [u32], w: usize, h: usize, p: &Params, centre: (f64, f64), half: f64) {
    let q = Params { kind: Kind::Plane, power: 2, c: (0., 0.), centre, half, rot: (1., 0.), ..*p };
    render(buf, w, h, &q);
    let k = scale(&q, w, h);
    let (at, window) = locus(p);
    let (mx, my) = ((at.0 - centre.0) / k + w as f64 * 0.5, (at.1 - centre.1) / k + h as f64 * 0.5);
    let white = pack_argb(255, 255, 255, 255);
    // WHY/PROOF: the locus can lie anywhere in the plane, off this pane or near its edge, and the marker's pixels are computed from it; a write outside the pane would land in the neighbouring pane's rows or past the buffer.
    let mut put = |x: f64, y: f64| {
        if x >= 0. && y >= 0. && (x as usize) < w && (y as usize) < h {
            buf[y as usize * w + x as usize] = white;
        }
    };
    // Crosshair arms from 2² to 2⁴ pixels out, leaving the point itself clear.
    for i in (1 << 2)..(1 << 4) {
        let f = i as f64;
        put(mx + f, my);
        put(mx - f, my);
        put(mx, my + f);
        put(mx, my - f);
    }
    if let Some(wh) = window {
        let r = wh / k;
        let steps = (r * TAU) as u32 + (1 << 4);
        for i in 0..steps {
            let a = i as f64 * TAU / steps as f64;
            put(mx + r * a.cos(), my + r * a.sin());
        }
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
}
