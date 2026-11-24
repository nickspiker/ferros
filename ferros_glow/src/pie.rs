//! A loading circle's pie (owner, 2026-11-24: "Would also be nice to have a pacman/pie fill for kists and pucks whilst they are loading. I think blue channel for download, green for the optimization and verification step and the red channel for any final stuffs, which should be multiplied by the kist icon or in the case of a puck be black/grey.").
//! Three sweeps from the circle's top, clockwise, one per channel: blue the download, green the optimisation and verification (signature, hashes, the code's check, a compile), red the final steps (seal, store, register).
//! Each sweep multiplies its own channel of the circle's layer (its icon, or grey where it has none): where a sweep has passed, that channel of the icon shows; where it has not, that channel is dark.
//! So the icon comes up channel by channel as its stages finish, and once all three are whole the layer is the icon itself.
//!
//! In darkness space (fluor's pixels: α and premultiplied darkness D, composited front to back with `under`), at a pixel with sweep coverages s_c:
//! the loading layer is (α, D'_c) with D'_c = α − s_c · (α − D_c);
//! its light over black is α − D'_c = s_c · (α − D_c), the icon's light in that channel times the sweep;
//! its α is the icon's, unchanged, so an unswept channel is darkness, never a hole;
//! then the layer goes `under` what is in front of it like any other.
//! The display's canvas holds premultiplied light (α − D, baked), where the same multiply is s_c times each channel (`multiply`); `multiply_darkness` is the same thing on α and darkness, and the tests hold the two equal.
//!
//! The sweep's edge and centre are antialiased analytically and C∞ in the share, with no clamp and no kink at 0 or whole (`sweep`).
//! No I/O, no std.

use core::f32::consts::TAU;

/// A stage's share as the stower reports it (docs/KISTS.md "The loading pie") and the display takes it: unsigned Q15, `WHOLE` = 1 << 15 the stage done.
pub const WHOLE: u16 = 1 << 15;

/// The three stages' shares, each 0 to `WHOLE`, in the order the wire carries them.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Shares {
    pub blue: u16,
    pub green: u16,
    pub red: u16,
}

impl Shares {
    /// Every stage done: the layer is the icon itself.
    pub const DONE: Shares = Shares { blue: WHOLE, green: WHOLE, red: WHOLE };

    /// From the wire's six bytes (blue, green, red, each u16 LE); None when a share is past `WHOLE`.
    /// WHY the test: the bytes are another process's, and a share past whole is a turn past the circle's top, outside the range `sweep`'s bounds are proven over.
    pub fn read(b: &[u8; 6]) -> Option<Shares> {
        let s = Shares { blue: u16::from_le_bytes([b[0], b[1]]), green: u16::from_le_bytes([b[2], b[3]]), red: u16::from_le_bytes([b[4], b[5]]) };
        (s.blue <= WHOLE && s.green <= WHOLE && s.red <= WHOLE).then_some(s)
    }

    /// The wire's six bytes.
    pub fn bytes(&self) -> [u8; 6] {
        let (b, g, r) = (self.blue.to_le_bytes(), self.green.to_le_bytes(), self.red.to_le_bytes());
        [b[0], b[1], g[0], g[1], r[0], r[1]]
    }

    /// Each share as a fraction of a turn, in the pixel's channel order: red, green, blue.
    pub fn turns(&self) -> [f32; 3] {
        let q = 1. / WHOLE as f32;
        [self.red as f32 * q, self.green as f32 * q, self.blue as f32 * q]
    }

    /// The three in one word (16 bits each), for a cached drawing's key.
    pub fn key(&self) -> u64 {
        self.blue as u64 | (self.green as u64) << 16 | (self.red as u64) << 32
    }
}

/// A C∞ step from 0 at t ≤ 0 to 1 at t ≥ 1: the bump exp(−1/t) against its mirror (the display's `draw::bump_step`).
/// Every derivative is zero at both ends; the two tests are its definition, not a clamp.
#[inline]
fn bump_step(t: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let (a, b) = (libm::expf(-1. / t), libm::expf(-1. / (1. - t)));
    a / (a + b)
}

/// An edge's coverage at signed distance `d` pixels inside it: the C∞ step across two pixels centred on the edge, slope 1 there (as the display's ferros mark has its edges).
#[inline]
fn edge(d: f32) -> f32 {
    bump_step((d + 1.) * 0.5)
}

/// Where the centre's coverage gives way to the edges': from half a pixel out to two and a half, by the same C∞ step.
const CENTRE_FROM: f32 = 0.5;
const CENTRE_OVER: f32 = 2.;

/// One sweep's coverage at polar (rho pixels, phi radians clockwise from the top, in (−π, π]) for a share `t` of a turn, 0 to 1.
/// The wedge from the top to the angle Θ = 2πt, smoothed across each of its two straight edges by `edge` of the arc length to it, ρ·Δφ (the distance to the ray within the step's two pixels).
/// The angle is periodic, so the wedge is summed over its images a turn apart: Σ_k [edge(ρ(Θ − φ − 2πk)) − edge(−ρ(φ + 2πk))].
/// For φ in (−π, π] and Θ in [0, 2π], once πρ ≥ 1 (everywhere the edges are used: ρ ≥ `CENTRE_FROM` > 1/π), only three of those steps are not exactly 0 or cancelled: k = −1 puts both of its edges more than πρ away on the covered side (its two steps 1, their difference 0), k ≥ 2 more than 3πρ away on the other (both 0), and k = 1's second step lies past πρ (0).
/// What is left is edge(a) − (edge(b) − edge(c)) with a = ρ(Θ − φ), b = ρ(0 − φ), c = ρ((Θ − 2π) − φ), written so it is exact at both ends in floats too: at Θ = 0, a is b to the bit and c's step is 0; at Θ = 2π, c is b to the bit and a's step is 1.
/// It lies in [0, 1]: a ≥ b and b ≥ c, and edge rises, so 0 ≤ edge(b) − edge(c) ≤ edge(b) ≤ edge(a) ≤ 1.
/// Between the ends it is C∞ in Θ, so the sweep has no kink as it starts or as it closes.
/// At the centre every edge meets and the angle means nothing: a pixel there is covered by the share itself, the wedge's area, and the edges take over by a C∞ step from `CENTRE_FROM` to `CENTRE_FROM + CENTRE_OVER` pixels out: t + c(ρ)·(edges − t), a convex mix of two values in [0, 1], exactly t or exactly the edges at either end of the step.
pub fn sweep(rho: f32, phi: f32, t: f32) -> f32 {
    let theta = t * TAU;
    let (a, b, c) = (rho * (theta - phi), rho * (0. - phi), rho * ((theta - TAU) - phi));
    let edges = edge(a) - (edge(b) - edge(c));
    let m = bump_step((rho - CENTRE_FROM) * (1. / CENTRE_OVER));
    t + m * (edges - t)
}

/// The three sweeps' coverages at (u, v), pixels from the centre in the circle's own frame (u right, v down: its top is v < 0), for shares of a turn `t` (red, green, blue; `Shares::turns`).
/// A share of 0 or whole is that coverage everywhere (`sweep` is exactly 0 or 1 there), so the angle is worked out only while one is between.
pub fn cover(u: f32, v: f32, t: [f32; 3]) -> [f32; 3] {
    if t.iter().all(|&x| x == 0. || x == 1.) {
        return t;
    }
    let rho = libm::sqrtf(u * u + v * v);
    // Clockwise from the top: atan2 of (right, up).
    let phi = libm::atan2f(u, -v);
    t.map(|x| if x == 0. || x == 1. { x } else { sweep(rho, phi, x) })
}

/// A premultiplied light pixel (the display's canvas: R, G, B in bits 0-15, 16-31, 32-47, α in 48-63) with each colour channel multiplied by its coverage `s` (red, green, blue, each 0 to 1), α unchanged.
#[inline]
pub fn multiply(p: u64, s: [f32; 3]) -> u64 {
    // s is at most 1 (`sweep`'s bound), so a channel stays inside its 16 bits.
    let m = |sh: u32, k: f32| (((p >> sh & 0xFFFF) as f32 * k + 0.5) as u64) << sh;
    (p & 0xFFFF << 48) | m(0, s[0]) | m(16, s[1]) | m(32, s[2])
}

/// The same multiply on α and premultiplied darkness (fluor's pixels, 16 bits each, `D ≤ α`): D' = α − s·(α − D), α unchanged.
#[inline]
pub fn multiply_darkness(alpha: u16, d: [u16; 3], s: [f32; 3]) -> [u16; 3] {
    let a = alpha as f32;
    [0, 1, 2].map(|c| (a - s[c] * (a - d[c] as f32) + 0.5) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Polar from the circle's frame, as `cover` takes it.
    fn at(u: f32, v: f32, t: f32) -> f32 {
        cover(u, v, [t, 0., 0.])[0]
    }

    #[test]
    fn nothing_at_zero_everything_at_whole() {
        for y in -40..=40 {
            for x in -40..=40 {
                let (u, v) = (x as f32 + 0.5, y as f32 + 0.5);
                let (rho, phi) = (libm::sqrtf(u * u + v * v), libm::atan2f(u, -v));
                assert_eq!(sweep(rho, phi, 0.), 0., "at ({u}, {v})");
                assert_eq!(sweep(rho, phi, 1.), 1., "at ({u}, {v})");
            }
        }
    }

    #[test]
    fn half_covers_the_right_side() {
        // Half a turn from the top, clockwise: the right half lit, the left dark, away from the edges.
        assert_eq!(at(10., 0., 0.5), 1.);
        assert_eq!(at(-10., 0., 0.5), 0.);
        // A quarter: the top-right quadrant only.
        assert_eq!(at(7., -7., 0.25), 1.);
        assert_eq!(at(7., 7., 0.25), 0.);
        assert_eq!(at(-7., -7., 0.25), 0.);
        // Three quarters: all but the top-left.
        assert_eq!(at(-7., 7., 0.75), 1.);
        assert_eq!(at(-7., -7., 0.75), 0.);
    }

    #[test]
    fn edges_are_half_on_the_line() {
        // On the start ray (straight up) and on the moving ray, the coverage is the step's middle.
        assert!((at(0., -20., 0.25) - 0.5).abs() < 1e-5);
        assert!((at(20., 0., 0.25) - 0.5).abs() < 1e-5);
        // Two pixels off a ray is past the step.
        assert_eq!(at(-2., -20., 0.25), 0.);
        assert_eq!(at(2., -20., 0.25), 1.);
    }

    #[test]
    fn centre_is_the_share() {
        for t in [0.01, 0.25, 0.5, 0.9, 0.999] {
            assert!((at(0., 0., t) - t).abs() < 1e-6, "{t}");
        }
    }

    #[test]
    fn within_unit_everywhere() {
        let mut t = 0.;
        while t <= 1. {
            for y in -12..=12 {
                for x in -12..=12 {
                    let c = at(x as f32 * 0.5, y as f32 * 0.5, t);
                    assert!((0. ..=1.).contains(&c), "{c} at ({x}, {y}) t {t}");
                }
            }
            t += 1. / 64.;
        }
    }

    /// C¹ in the share, the formula on both sides of 0, a half and whole: the jump in the first difference across each point shrinks with the step (a kink, a clamp's, would keep it at the slope's size), at every pixel near the top, where the sweep starts and closes.
    #[test]
    fn smooth_in_the_share_at_both_ends() {
        for y in -6..=6 {
            for x in -6..=6 {
                let (u, v) = (x as f32 + 0.5, y as f32 - 8.);
                let (rho, phi) = (libm::sqrtf(u * u + v * v), libm::atan2f(u, -v));
                let f = |t: f32| sweep(rho, phi, t);
                let jump = |t0: f32, h: f32| ((f(t0 + h) - f(t0)) / h - (f(t0) - f(t0 - h)) / h).abs();
                for t0 in [0., 0.5, 1.] {
                    let (coarse, fine) = (jump(t0, 1. / 1024.), jump(t0, 1. / 8192.));
                    assert!(fine < coarse * 0.25 + 0.5, "kink at t {t0}, ({u}, {v}): {coarse} then {fine}");
                }
            }
        }
    }

    /// No seam where atan2 wraps (straight down).
    #[test]
    fn no_seam_at_the_bottom() {
        for t in [0.3, 0.5, 0.51, 0.7] {
            let a = at(-1e-4, 20., t);
            let b = at(1e-4, 20., t);
            assert!((a - b).abs() < 1e-3, "t {t}: {a} {b}");
        }
    }

    #[test]
    fn light_and_darkness_agree() {
        // A few premultiplied pixels: light L = α − D per channel.
        for &(alpha, d) in &[(65535u16, [0u16, 30000, 65535]), (40000, [10000, 20000, 39000]), (12345, [0, 0, 12345])] {
            for s in [[0., 0., 0.], [1., 1., 1.], [0.5, 0.25, 0.75], [1., 0., 0.3]] {
                let p = (alpha as u64) << 48 | ((alpha - d[0]) as u64) | ((alpha - d[1]) as u64) << 16 | ((alpha - d[2]) as u64) << 32;
                let q = multiply(p, s);
                let dd = multiply_darkness(alpha, d, s);
                assert_eq!(q >> 48, alpha as u64);
                for c in 0..3 {
                    let light = (q >> (16 * c) & 0xFFFF) as i32;
                    assert!((light - (alpha - dd[c]) as i32).abs() <= 1, "α {alpha} d {d:?} s {s:?} channel {c}");
                }
            }
        }
    }

    #[test]
    fn shares_read_refuses_past_whole() {
        assert_eq!(Shares::read(&Shares::DONE.bytes()), Some(Shares::DONE));
        let mut b = Shares::DONE.bytes();
        b[2] = 1;
        b[3] = 0x80;
        assert_eq!(Shares::read(&b), None);
        let s = Shares { blue: 1, green: 2, red: 3 };
        assert_eq!(Shares::read(&s.bytes()), Some(s));
    }
}
