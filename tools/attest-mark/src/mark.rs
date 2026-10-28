//! The mark: a view into a Julia set, chosen by the device's salt and moved by what is in the textbox.
//!
//! The salt (on the phone, the device's own, sealed under its ira; here a stand-in) fixes the family: c, the colouring method and the palette.
//! c is chosen symbolically, so devices land in different regimes rather than all near the main cardioid, which is where sampling the plane by area mostly lands: the hash picks a bulb p/q on the main cardioid (q from 2 to 2⁴; J(c) near it spirals with q arms), sometimes a sub-bulb p′/q′ of that bulb (spirals of spirals), and then a point just OUTSIDE that disc at a hash-chosen angle away from its root, so J(c) is a Cantor dust with no inside to go flat.
//! Bulbs are taken as discs: the p/q bulb's radius is close to sin(πp/q)/q², and its sub-bulbs sit at angle 2πp′/q′ from its root.
//! A candidate is kept when it escapes and trial views of J(c) show enough of the set, so a thin dendrite is passed over.
//! The colouring method is one of three: smooth iteration bands, the angle of the orbit at escape (a cellular mosaic), or an orbit trap (distance to a hash-chosen point: rings and petals).
//!
//! The textbox picks where on J(c) the view lands, with maximum diffusion: for c outside the Mandelbrot set, f(z) = z² + c on J(c) is conjugate to the full shift on two symbols, so every point of J(c) has an address, an infinite string of bits (which half of J each forward iterate falls in), and every string names exactly one point.
//! Reading a hash of (salt, text) as that address lands on a point of J(c) deterministically; inverse iteration (z → ±√(z − c)) builds it, the last branch applied being the address's first and coarsest bit.
//! Uniform bits give the balanced (harmonic) measure on J, Brolin's equidistribution: each bit halves the measure, so every change in the box lands somewhere new on the whole set, independent of where the last one landed.
//! The same hash then sets the zoom (1× to 2⁶×, log-uniform), the rotation, and a Gaussian offset of the view's centre (σ a quarter window), and a thumbnail rejects framings that show too little of the set, redrawing with the next counter.
//! Nothing compares the text against anything, so the picture says nothing about whether the text is right.

use fluor::paint::pack_argb;
use rayon::prelude::*;
use std::f64::consts::TAU;

/// Iteration cap: deep enough for the zooms used here (up to 2⁶).
const MAX_ITER: u32 = 1 << 10;
/// Escape when |z|² passes 2⁸ (|z| = 16), far enough out for smooth colouring.
const BAIL_SQ: f64 = (1 << 8) as f64;
/// A jump of 2² iterations between neighbouring thumbnail pixels marks an edge of the set.
const JUMP: i32 = 1 << 2;
/// A framing is kept when at least a quarter of the thumbnail is edge.
const DETAIL_MIN: f64 = 1. / (1 << 2) as f64;

/// Everything one picture needs, in the plane's terms.
#[derive(Clone, Copy)]
pub struct Params {
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

/// The Mandelbrot set's exterior distance estimate at c, or None when c does not escape within the cap (inside, or too close to tell).
fn m_distance(c: (f64, f64)) -> Option<f64> {
    let (mut x, mut y, mut dx, mut dy) = (0., 0., 0., 0.);
    for _ in 0..(1 << 12) {
        let (ndx, ndy) = (2. * (x * dx - y * dy) + 1., 2. * (x * dy + y * dx));
        let (nx, ny) = (x * x - y * y + c.0, 2. * x * y + c.1);
        (x, y, dx, dy) = (nx, ny, ndx, ndy);
        let r2 = x * x + y * y;
        if r2 > (1u64 << 16) as f64 {
            let r = r2.sqrt();
            return Some(r * r.ln() / (dx * dx + dy * dy).sqrt());
        }
    }
    None
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

/// The device's c, chosen symbolically: bulb p/q on the main cardioid, sometimes a sub-bulb, then just outside that disc.
fn family_c(salt: &[u8; 32]) -> (f64, f64) {
    use std::f64::consts::PI;
    let mut counter = 0;
    loop {
        let h = draw(salt, 0, counter, &[]);
        counter += 1;
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
        // From the centre toward the root.
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
        // Just outside the disc, within three eighths of a turn of straight away from the root, by 2⁻² to 2⁻⁶ of its radius.
        let away = rotate((-to_root.0, -to_root.1), (unit(&h[8..12]) - 0.5) * TAU * 0.75);
        let out = radius * (1. + (-(2. + unit(&h[12..16]) * 4.)).exp2());
        let c = (centre.0 + away.0 * out, centre.1 + away.1 * out);
        if m_distance(c).is_some() && full_bodied(salt, c) {
            return c;
        }
    }
}

/// Is J(c) full enough to fill a view? Of 2² trial landings at 2³× zoom, at least half must show enough of the set; a thin dendrite fails at every zoom and is passed over.
fn full_bodied(salt: &[u8; 32], c: (f64, f64)) -> bool {
    let mut good = 0;
    for n in 0..(1 << 2) {
        let a = draw(salt, 4, n, &[]);
        let half = 2. / (1 << 3) as f64;
        let p = Params { c, centre: address_point(c, &a), half, rot: (1., 0.), phase: [0.; 3], mode: 0, trap: (0., 0.) };
        good += framed(&p) as u32;
    }
    good >= 1 << 1
}

/// The point of J(c) whose address is `bits`: inverse iteration applies the branches finest first, so `bits[0]`'s low bit, applied last, picks the half of J.
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

/// A standard normal pair from two uniforms (Box–Muller).
fn gauss(u0: f64, u1: f64) -> (f64, f64) {
    let r = (-2. * (1. - u0).ln()).sqrt();
    (r * (TAU * u1).cos(), r * (TAU * u1).sin())
}

/// The plane point under pixel (px, py) of a `w × h` window, `k` plane units per pixel.
#[inline]
fn plane(p: &Params, px: usize, py: usize, w: usize, h: usize, k: f64) -> (f64, f64) {
    let (u, v) = ((px as f64 + 0.5 - w as f64 * 0.5) * k, (py as f64 + 0.5 - h as f64 * 0.5) * k);
    (p.centre.0 + u * p.rot.0 - v * p.rot.1, p.centre.1 + u * p.rot.1 + v * p.rot.0)
}

/// What one orbit did: iterations until escape (MAX_ITER if it never did), z and its derivative dz/dz₀ at escape, and its closest approach to the trap point (squared).
struct Orbit {
    n: u32,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    trap_sq: f64,
}

#[inline]
fn orbit(mut x: f64, mut y: f64, c: (f64, f64), trap: (f64, f64)) -> Orbit {
    let mut trap_sq = f64::MAX;
    // dz/dz₀ starts at 1 and follows dz ← 2·z·dz; it can overflow to infinity near the set, which the distance estimate reads, correctly, as "on the set".
    let (mut dx, mut dy) = (1f64, 0f64);
    for n in 0..MAX_ITER {
        let (x2, y2) = (x * x, y * y);
        if x2 + y2 > BAIL_SQ {
            return Orbit { n, x, y, dx, dy, trap_sq };
        }
        let (tx, ty) = (x - trap.0, y - trap.1);
        trap_sq = trap_sq.min(tx * tx + ty * ty);
        (dx, dy) = (2. * (x * dx - y * dy), 2. * (x * dy + y * dx));
        y = 2. * x * y + c.1;
        x = x2 - y2 + c.0;
    }
    Orbit { n: MAX_ITER, x, y, dx, dy, trap_sq }
}

/// Plane units per pixel for a `w × h` window: half the span covers `half`.
fn scale(p: &Params, w: usize, h: usize) -> f64 {
    let span = 2. * (w * h) as f64 / (w + h) as f64;
    p.half / (span * 0.5)
}

/// How much edge a 2⁵ × 2⁶ thumbnail shows: the share of pixels whose iteration count jumps by at least 2² to the neighbour on the right or below.
/// Only filaments of the set make such jumps; the smooth glow between them makes none, so the score reads "how much of the view is set" at every zoom.
fn detail(p: &Params) -> f64 {
    const W: usize = 1 << 5;
    const H: usize = 1 << 6;
    let k = scale(p, W, H);
    let mut n = [0i32; W * H];
    for py in 0..H {
        for px in 0..W {
            let (x, y) = plane(p, px, py, W, H, k);
            n[py * W + px] = orbit(x, y, p.c, p.trap).n as i32;
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

impl Params {
    /// The picture for this device salt and this textbox content.
    pub fn new(salt: &[u8; 32], text: &str) -> Self {
        let c = family_c(salt);
        let f = draw(salt, 1, 0, &[]);
        let phase = [unit(&f[0..4]), unit(&f[4..8]), unit(&f[8..12])];
        let mode = f[12] % 3;
        let trap = ((unit(&f[16..20]) - 0.5) * 2., (unit(&f[20..24]) - 0.5) * 2.);
        // Each try draws a fresh address, zoom, turn and offset from (salt, text, counter); the first framing full enough is taken, and after 2⁵ tries the last one stands.
        let mut n = 0;
        loop {
            let a = draw(salt, 2, n, text.as_bytes());
            let t = draw(salt, 3, n, text.as_bytes());
            n += 1;
            let point = address_point(c, &a);
            let half = 2. / (unit(&t[0..4]) * 6.).exp2();
            let turn = unit(&t[4..8]) * TAU;
            let (gx, gy) = gauss(unit(&t[8..12]), unit(&t[12..16]));
            let sigma = half * 0.25;
            let p = Params { c, centre: (point.0 + gx * sigma, point.1 + gy * sigma), half, rot: (turn.cos(), turn.sin()), phase, mode, trap };
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
/// Near the set the escape count changes chaotically from pixel to pixel, which reads as static; so every pixel first asks how far it is from the set, in pixels (the distance estimate ½·|z|·ln|z| / |dz/dz₀|), and anything within about a pixel is the set's own ink, edged smoothly. The colouring method only paints the smooth field around it.
#[inline]
fn shade(x: f64, y: f64, p: &Params, k: f64) -> u32 {
    let o = orbit(x, y, p.c, p.trap);
    let ink = palette(0.5, &p.phase);
    if o.n == MAX_ITER {
        return pack_argb((ink[0] * 255.) as u8, (ink[1] * 255.) as u8, (ink[2] * 255.) as u8, 255);
    }
    let r = (o.x * o.x + o.y * o.y).sqrt();
    let dist_px = 0.5 * r * r.ln() / (o.dx * o.dx + o.dy * o.dy).sqrt() / k;
    // Smooth iteration count; near the set it is large, far out small, and the glow fades the far field to black in every mode.
    let nu = o.n as f64 + 1. - r.ln().log2();
    let glow = 1. - (-nu * 0.125).exp2();
    let t = match p.mode {
        0 => nu * (1. / (1 << 4) as f64),
        // The orbit's angle at escape, turned by the smooth count: cells that tile each level of the set.
        1 => o.y.atan2(o.x) / TAU + nu * (1. / (1 << 5) as f64),
        // How close the orbit came to the trap, on a log scale: rings and petals around each preimage of the trap.
        _ => -(o.trap_sq.ln()) * (1. / (1 << 3) as f64),
    };
    let field = palette(t, &p.phase);
    // The ink's share: 1 on the set, falling smoothly (1 / (1 + d²), C∞) to nothing a few pixels out.
    let a = 1. / (1. + dist_px * dist_px);
    let ch = |i: usize| ((ink[i] * a + field[i] * glow * (1. - a)) * 255.) as u8;
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
