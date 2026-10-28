//! The mark: a view into a Julia set, chosen by the device's salt and moved by what is in the textbox.
//!
//! Seeding is generated, not curated, and every step is a deterministic function of the salt:
//! 0. c is sampled from the plane until it lies just OUTSIDE the Mandelbrot set (its distance estimate between 2⁻¹⁰ and 2⁻⁶), so the Julia set is a dust of spirals and dendrites with no inside to go flat;
//! 1. the view centres on a point OF that Julia set, reached by inverse iteration (z → ±√(z − c)) with salt-chosen branches, at a salt-chosen zoom and rotation;
//! 2. a thumbnail scores how much of the view lies near the set, and a framing that is too empty or too full is resampled with the next counter.
//! The salt (on the phone, the device's own, sealed under its ira; here a stand-in) therefore fixes the family: the texture, the palette and roughly where the view sits.
//! The textbox then moves the view a little — a pan of up to a quarter of the window, a turn of up to an eighth, half a stop of zoom — rescored the same way, so each change in the box visibly moves the picture while the device's look stays.
//! Nothing compares the text against anything, so the picture says nothing about whether the text is right.

use fluor::paint::pack_argb;
use rayon::prelude::*;
use std::f64::consts::TAU;

/// Iteration cap: deep enough for the zooms used here (up to 2⁵).
const MAX_ITER: u32 = 1 << 10;
/// Escape when |z|² passes 2⁸ (|z| = 16), far enough out for smooth colouring.
const BAIL_SQ: f64 = (1 << 8) as f64;
/// c is accepted when its distance to the Mandelbrot set lies in [2⁻¹⁰, 2⁻⁶].
const DE_NEAR: f64 = 1. / (1 << 10) as f64;
const DE_FAR: f64 = 1. / (1 << 6) as f64;
/// A thumbnail pixel counts as near the set once its orbit survives 2⁴ iterations.
const NEAR_ITER: u32 = 1 << 4;
/// A framing is kept when between 3/8 and 7/8 of the thumbnail lies near the set.
const FILL_LO: f64 = 3. / (1 << 3) as f64;
const FILL_HI: f64 = 7. / (1 << 3) as f64;

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

/// The salt's framing number `counter`: c, the point of J(c) to centre on, the zoom, the rotation and the palette.
fn framing(salt: &[u8; 32], counter: &mut u32) -> Params {
    let c = loop {
        let h = draw(salt, 0, *counter, &[]);
        *counter += 1;
        let c = (-2. + unit(&h[0..4]) * 2.5, -1.25 + unit(&h[4..8]) * 2.5);
        if let Some(d) = m_distance(c) {
            if d > DE_NEAR && d < DE_FAR {
                break c;
            }
        }
    };
    let f = draw(salt, 1, *counter, &[]);
    // Inverse iteration lands on J(c) whatever it starts from; 2⁷ steps, each branch from one bit of the hash.
    let (mut x, mut y) = (0.5f64, 0.);
    for i in 0..(1 << 7) {
        let (ax, ay) = (x - c.0, y - c.1);
        let r = (ax * ax + ay * ay).sqrt();
        let (mut sx, mut sy) = (((r + ax) * 0.5).sqrt(), ((r - ax) * 0.5).sqrt());
        if ay < 0. {
            sy = -sy;
        }
        if (f[(i >> 3) & 31] >> (i & 7)) & 1 == 1 {
            (sx, sy) = (-sx, -sy);
        }
        (x, y) = (sx, sy);
    }
    let zoom = (unit(&f[0..4]) * 5.).exp2();
    let a = unit(&f[4..8]) * TAU;
    Params { c, centre: (x, y), half: 2. / zoom, rot: (a.cos(), a.sin()), phase: [unit(&f[8..12]), unit(&f[12..16]), unit(&f[16..20])] }
}

/// The plane point under pixel (px, py) of a `w × h` window, `k` plane units per pixel.
#[inline]
fn plane(p: &Params, px: usize, py: usize, w: usize, h: usize, k: f64) -> (f64, f64) {
    let (u, v) = ((px as f64 + 0.5 - w as f64 * 0.5) * k, (py as f64 + 0.5 - h as f64 * 0.5) * k);
    (p.centre.0 + u * p.rot.0 - v * p.rot.1, p.centre.1 + u * p.rot.1 + v * p.rot.0)
}

/// Iterations until escape, or MAX_ITER, with |z|² at escape.
#[inline]
fn orbit(mut x: f64, mut y: f64, c: (f64, f64)) -> (u32, f64) {
    for n in 0..MAX_ITER {
        let (x2, y2) = (x * x, y * y);
        if x2 + y2 > BAIL_SQ {
            return (n, x2 + y2);
        }
        y = 2. * x * y + c.1;
        x = x2 - y2 + c.0;
    }
    (MAX_ITER, 0.)
}

/// Plane units per pixel for a `w × h` window: half the span covers `half`.
fn scale(p: &Params, w: usize, h: usize) -> f64 {
    let span = 2. * (w * h) as f64 / (w + h) as f64;
    p.half / (span * 0.5)
}

/// How much of a 2⁵ × 2⁶ thumbnail lies near the set.
fn fill(p: &Params) -> f64 {
    let (w, h) = (1usize << 5, 1usize << 6);
    let k = scale(p, w, h);
    let mut near = 0;
    for py in 0..h {
        for px in 0..w {
            let (x, y) = plane(p, px, py, w, h, k);
            near += (orbit(x, y, p.c).0 >= NEAR_ITER) as u32;
        }
    }
    near as f64 / (w * h) as f64
}

fn framed(p: &Params) -> bool {
    let s = fill(p);
    s >= FILL_LO && s <= FILL_HI
}

impl Params {
    /// The picture for this device salt and this textbox content.
    pub fn new(salt: &[u8; 32], text: &str) -> Self {
        let mut counter = 0;
        let mut base = framing(salt, &mut counter);
        while !framed(&base) {
            base = framing(salt, &mut counter);
        }
        // The text's move, rescored like a framing; the first move that keeps the view full enough is taken, and after 2⁴ tries the last one stands.
        let mut n = 0;
        loop {
            let t = draw(salt, 2, n, text.as_bytes());
            n += 1;
            let mut p = base;
            let (ox, oy) = ((unit(&t[0..4]) - 0.5) * base.half * 0.5, (unit(&t[4..8]) - 0.5) * base.half * 0.5);
            let turn = (unit(&t[8..12]) - 0.5) * TAU * 0.25;
            let (c1, s1) = (turn.cos(), turn.sin());
            p.rot = (base.rot.0 * c1 - base.rot.1 * s1, base.rot.0 * s1 + base.rot.1 * c1);
            p.centre = (base.centre.0 + ox, base.centre.1 + oy);
            p.half = base.half * (unit(&t[12..16]) - 0.5).exp2();
            if framed(&p) || n >= 1 << 4 {
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

/// One point of the plane, as an opaque α + darkness pixel.
#[inline]
fn shade(x: f64, y: f64, p: &Params) -> u32 {
    let (n, r2) = orbit(x, y, p.c);
    if n == MAX_ITER {
        return pack_argb(8, 8, 8, 255);
    }
    // Smooth iteration count; near the set it is large and bright, far out it fades to black.
    let nu = n as f64 + 1. - (0.5 * r2.ln()).log2();
    let glow = 1. - (-nu * 0.125).exp2();
    let rgb = palette(nu * (1. / (1 << 4) as f64), &p.phase);
    pack_argb((rgb[0] * glow * 255.) as u8, (rgb[1] * glow * 255.) as u8, (rgb[2] * glow * 255.) as u8, 255)
}

/// Render the mark into `buf` (`w × h`, α + darkness), rows across the cores.
pub fn render(buf: &mut [u32], w: usize, h: usize, p: &Params) {
    let k = scale(p, w, h);
    let p = *p;
    buf.par_chunks_mut(w).enumerate().for_each(|(py, row)| {
        for px in 0..w {
            let (x, y) = plane(&p, px, py, w, h, k);
            row[px] = shade(x, y, &p);
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
