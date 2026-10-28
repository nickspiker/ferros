//! The mark: a Julia set chosen by the device's salt and nudged by what is in the textbox.
//!
//! The salt (on the phone, the device's salt sealed under its ira; here a stand-in) picks the family: which component of the Mandelbrot set the parameter sits in, the rational rotation p/q its spirals turn by, the palette and the picture's rotation.
//! The textbox picks where in that family the parameter sits: how close to the edge (ρ) and how far off the rational angle (ε), so each change in the box moves the picture while the device's look stays.
//! Nothing compares the text against anything, so the picture says nothing about whether the text is right.
//!
//! The parameter comes in closed form from a multiplier λ inside the unit disc, so there is no search for a good spot and the Julia set is always connected:
//! main cardioid, c = λ/2 − λ²/4, attracting fixed point z* = λ/2;
//! period-two disc, c = −1 + λ/4, attracting cycle z = (−1 ± √(1 − λ))/2.
//! Knowing the attractor lets interior pixels stop as soon as they reach it.

use fluor::paint::pack_argb;
use rayon::prelude::*;
use std::f32::consts::TAU;

/// Iteration cap: 2⁸ is plenty at 1× zoom.
const MAX_ITER: u32 = 1 << 8;
/// Escape when |z|² passes 2⁸ (|z| = 16), far enough out for smooth colouring.
const BAIL_SQ: f32 = (1 << 8) as f32;
/// An interior orbit has arrived when it is within 2⁻¹⁰ of the attracting cycle.
const ARRIVE_SQ: f32 = 1. / ((1u32 << 20) as f32);
/// Half the picture's extent in the plane, mapped to half the window's span: 2 − 1/4 holds the widest of these Julia sets.
const HALF_EXTENT: f32 = 2. - 1. / ((1 << 2) as f32);

/// Everything one picture needs, in the plane's terms.
#[derive(Clone, Copy)]
pub struct Params {
    c: (f32, f32),
    /// The attracting cycle's points; the cardioid's single fixed point is stored twice so every pixel runs the same two tests.
    attractor: [(f32, f32); 2],
    /// The picture's rotation as (cos, sin).
    rot: (f32, f32),
    /// Per-channel palette phases.
    phase: [f32; 3],
}

/// 32 bits of a hash as a fraction in [0, 1).
fn unit(b: &[u8]) -> f32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / (1u64 << 32) as f32
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn mul(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// Principal square root of a complex number.
fn sqrt(a: (f32, f32)) -> (f32, f32) {
    let r = (a.0 * a.0 + a.1 * a.1).sqrt();
    let re = ((r + a.0) * 0.5).sqrt();
    let im = ((r - a.0) * 0.5).sqrt();
    (re, if a.1 < 0. { -im } else { im })
}

impl Params {
    /// The picture for this device salt and this textbox content.
    pub fn new(salt: &[u8; 32], text: &str) -> Self {
        // Domain byte 0: the device's family; byte 1 followed by the text: where in it.
        let fam = blake3::Hasher::new_keyed(salt).update(&[0]).finalize();
        let fam = fam.as_bytes();
        let txt = blake3::Hasher::new_keyed(salt).update(&[1]).update(text.as_bytes()).finalize();
        let txt = txt.as_bytes();

        let disc = fam[0] & 1 == 1;
        let q = 2 + (fam[1] % 8) as u32;
        let coprime: Vec<u32> = (1..q).filter(|&p| gcd(p, q) == 1).collect();
        let p = coprime[fam[2] as usize % coprime.len()];
        let angle = unit(&fam[4..8]) * TAU;
        let phase = [unit(&fam[8..12]), unit(&fam[12..16]), unit(&fam[16..20])];

        // ε: a small step off the rational angle, well inside the p/q neighbourhood; ρ: 1 − 2^−(3..7), from smooth to intricate.
        let eps = (unit(&txt[0..4]) - 0.5) * TAU / ((q * q) as f32 * (1 << 3) as f32);
        let rho = 1. - (-(3. + unit(&txt[4..8]) * 4.)).exp2();
        let theta = TAU * p as f32 / q as f32 + eps;
        let lambda = (rho * theta.cos(), rho * theta.sin());

        let (c, attractor) = if disc {
            let c = (-1. + lambda.0 * 0.25, lambda.1 * 0.25);
            let s = sqrt((1. - lambda.0, -lambda.1));
            let z0 = ((-1. + s.0) * 0.5, s.1 * 0.5);
            let z1 = ((-1. - s.0) * 0.5, -s.1 * 0.5);
            (c, [z0, z1])
        } else {
            let l2 = mul(lambda, lambda);
            let c = (lambda.0 * 0.5 - l2.0 * 0.25, lambda.1 * 0.5 - l2.1 * 0.25);
            let z = (lambda.0 * 0.5, lambda.1 * 0.5);
            (c, [z, z])
        };
        Params { c, attractor, rot: (angle.cos(), angle.sin()), phase }
    }
}

/// The palette: a cosine per channel, each with its own phase, in [0, 1].
fn palette(t: f32, phase: &[f32; 3]) -> [f32; 3] {
    [
        0.5 + 0.5 * (TAU * (t + phase[0])).cos(),
        0.5 + 0.5 * (TAU * (t + phase[1])).cos(),
        0.5 + 0.5 * (TAU * (t + phase[2])).cos(),
    ]
}

fn dark(rgb: [f32; 3], k: f32) -> u32 {
    pack_argb((rgb[0] * k * 255.) as u8, (rgb[1] * k * 255.) as u8, (rgb[2] * k * 255.) as u8, 255)
}

/// One point of the plane, as an opaque α + darkness pixel.
fn shade(mut x: f32, mut y: f32, p: &Params) -> u32 {
    let [a0, a1] = p.attractor;
    for n in 0..MAX_ITER {
        let (x2, y2) = (x * x, y * y);
        let r2 = x2 + y2;
        if r2 > BAIL_SQ {
            // Smooth iteration count; near the set it is large and bright, far out it fades to black.
            let nu = n as f32 + 1. - (0.5 * r2.ln()).log2();
            let glow = 1. - (-nu * 0.25).exp2();
            return dark(palette(nu * (1. / (1 << 4) as f32), &p.phase), glow);
        }
        let (d0x, d0y, d1x, d1y) = (x - a0.0, y - a0.1, x - a1.0, y - a1.1);
        if (d0x * d0x + d0y * d0y).min(d1x * d1x + d1y * d1y) < ARRIVE_SQ {
            // Interior: shaded by how long the orbit took to arrive, half as bright as the outside.
            return dark(palette(0.5 + n as f32 * (1. / (1 << 5) as f32), &p.phase), 0.5);
        }
        y = 2. * x * y + p.c.1;
        x = x2 - y2 + p.c.0;
    }
    // Neither escaped nor arrived within the cap: the set's own edge.
    pack_argb(8, 8, 8, 255)
}

/// Render the mark into `buf` (`w × h`, α + darkness).
/// A Julia set is symmetric through the origin (J = −J) and the origin sits at the window's centre, so only the top half is iterated and the bottom half is its point reflection.
pub fn render(buf: &mut [u32], w: usize, h: usize, p: &Params) {
    let span = 2. * (w * h) as f32 / (w + h) as f32;
    let k = HALF_EXTENT / (span * 0.5);
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    let half = (h + 1) / 2;
    let p = *p;
    buf[..half * w].par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let py = (y as f32 + 0.5 - cy) * k;
        for x in 0..w {
            let px = (x as f32 + 0.5 - cx) * k;
            let zx = px * p.rot.0 - py * p.rot.1;
            let zy = px * p.rot.1 + py * p.rot.0;
            row[x] = shade(zx, zy, &p);
        }
    });
    // Pixel (x, y) is the reflection of (w − 1 − x, h − 1 − y): with pixel centres at +½ the two sit at opposite points about the centre exactly.
    let (top, bottom) = buf.split_at_mut(half * w);
    for y in half..h {
        let src = (h - 1 - y) * w;
        let dst = (y - half) * w;
        for x in 0..w {
            bottom[dst + x] = top[src + (w - 1 - x)];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stand-in device draws a picture with an inside, an outside and an edge, and a change in the text changes it.
    #[test]
    fn pictures_have_structure_and_follow_the_text() {
        let (w, h) = (1 << 7, 1 << 8);
        for device in 0u32..(1 << 4) {
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
