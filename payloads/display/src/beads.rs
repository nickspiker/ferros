//! The privacy indicators' geometry (docs/QUICK.md "The privacy indicators"; owner, 2026-11-26: "be classy about it").
//!
//! - **The camera** is a solid disc behind the hole, the hole's radius and a stroke more; the hole's knockout in front of everything hides its inside, so what shows is a ring of the stroke's width round the lens ("a simple solid filled circle and the camera trim will automatically make it a nice stroke").
//! - **The beads** are the other indicators, one size, threaded round the hole on a ring tangent outside the camera's disc, so they stand in the same place whether the camera is lit or not.
//! - **Each side** holds three: the lowest tangent to the app's circle from outside (it never reaches into the app's room), each next one up tangent to the one below.
//!
//! Pure: the hole and the app's circle in, the disc and the beads out, nothing of the display's, so it is tested on the host (`tests` below).

/// The camera's stroke, the hole's radius shifted down by this (an eighth of it): the lit disc behind the hole is that much wider than the hole, and the knockout leaves a ring that wide.
pub const STROKE_LOG2: u32 = 3;
/// A bead's radius, the hole's over this (owner, 2026-11-26: "hole radius / 3").
pub const BEAD_PARTS: f32 = 3.;
/// The beads a side.
pub const SLOTS: usize = 3;
/// Bisection steps for the lowest bead's angle: a quarter turn halved 32 times is under a billionth of a radian, past what f32 keeps of it.
const FINE: u32 = 1 << 5;

/// The solved indicators.
#[derive(Clone, Copy, Debug)]
pub struct Beads {
    /// The camera's disc: the hole's centre, its radius plus the stroke.
    pub cam: (f32, f32, f32),
    /// A bead's radius.
    pub bead: f32,
    /// The beads' centres' distance from the hole's centre: the camera's radius and a bead's.
    pub ring: f32,
    /// Each side's lowest bead's angle from straight down, in radians, left then right.
    pub low: [f32; 2],
    /// The angle between neighbouring beads: tangent beads on the ring are 2 asin(bead / ring) apart.
    pub step: f32,
    /// The beads' centres, `[side][slot]`: side 0 left and 1 right, slot 0 the lowest.
    pub at: [[(f32, f32); SLOTS]; 2],
}

/// The indicators about the camera hole `hole` (centre and radius, `ferros_sys::cutout_hole`) beside the app's circle `app` (`ferros_sys::active_circle`, tangent to the hole's bottom from below).
/// Each side's lowest bead sits where it touches the app's circle from outside: its centre's distance from the app's centre less the two radii, as a function of the angle from straight down, is below nought straight down (the hole touches the app's circle there, and the ring reaches past the hole) and above it level with the hole's centre (there the bead is at most a bead's radius below the hole's centre line, and the hole's radius is three beads'), and it rises between, so bisection finds where it is nought.
pub fn solve(hole: (f32, f32, f32), app: (f32, f32, f32)) -> Beads {
    let (hx, hy, r) = hole;
    let cam_r = r + r * (1. / (1 << STROKE_LOG2) as f32);
    let bead = r * (1. / BEAD_PARTS);
    let ring = cam_r + bead;
    // In f64: the app's centre is about fifteen hundred pixels away, and the tangency is a difference of two such distances.
    let (ax, ay, ar) = (app.0 as f64, app.1 as f64, app.2 as f64);
    let reach = ar + bead as f64;
    let centre = |side: f32, phi: f32| (hx + side * ring * libm::sinf(phi), hy + ring * libm::cosf(phi));
    let gap = |side: f32, phi: f64| {
        let (x, y) = (hx as f64 + side as f64 * ring as f64 * libm::sin(phi), hy as f64 + ring as f64 * libm::cos(phi));
        libm::sqrt((x - ax) * (x - ax) + (y - ay) * (y - ay)) - reach
    };
    let step = 2. * libm::asinf(bead / ring);
    let mut low = [0f32; 2];
    let mut at = [[(0f32, 0f32); SLOTS]; 2];
    for (k, side) in [-1f32, 1.].into_iter().enumerate() {
        let (mut lo, mut hi) = (0f64, core::f64::consts::FRAC_PI_2);
        for _ in 0..FINE {
            let mid = (lo + hi) * 0.5;
            if gap(side, mid) < 0. { lo = mid } else { hi = mid }
        }
        low[k] = hi as f32;
        for (j, c) in at[k].iter_mut().enumerate() {
            *c = centre(side, low[k] + j as f32 * step);
        }
    }
    Beads { cam: (hx, hy, cam_r), bead, ring, low, step, at }
}

impl Beads {
    /// The highest row any bead reaches, its top edge (the spare's slot counted too, so the check holds whatever is lit).
    pub fn top(&self) -> f32 {
        self.at.iter().flatten().map(|c| c.1 - self.bead).fold(f32::MAX, f32::min)
    }

    /// The least room between a bead's edge and the circle `c`'s (x, y, r): below nought they overlap.
    pub fn gap(&self, c: (f32, f32, f32)) -> f32 {
        self.at.iter().flatten().map(|b| libm::sqrtf((b.0 - c.0) * (b.0 - c.0) + (b.1 - c.1) * (b.1 - c.1)) - c.2 - self.bead).fold(f32::MAX, f32::min)
    }

    /// The rows the disc and the beads cover, [top, bottom), out to `reach` past their radii: the indicators' band.
    pub fn rows(&self, reach: f32) -> (i64, i64) {
        let (mut a, mut b) = (self.cam.1 - self.cam.2, self.cam.1 + self.cam.2);
        // WHY the min and max: the band is the union of the disc's rows and every bead's, an extent, never a clamp.
        for c in self.at.iter().flatten() {
            a = a.min(c.1 - self.bead);
            b = b.max(c.1 + self.bead);
        }
        (libm::floorf(a - reach) as i64, libm::ceilf(b + reach) as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's circle as ferros_sys `active_circle` makes it: tangent to the cutout's bottom and the screen's, centred.
    fn app(w: f32, h: f32, bottom: f32) -> (f32, f32, f32) {
        let r = (h - bottom) * 0.5;
        (w * 0.5, bottom + r, r)
    }

    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        libm::sqrtf((a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1))
    }

    /// Each board: the hole (ferros_sys `CUTOUT_HOLE`, `CUTOUT_HOLE_AKITA`), the cutout's bottom, the screen, and the top console circles either side of the hole, GPS and Bluetooth (x, y, r): husky's as the display logged them on the Pro (`display: corners`, 2026-11-24), akita's as `corner_layout` solves them for its glass (main.rs's solve run on the host, 2026-11-26, which gives husky's logged circles to a hundredth).
    const BOARDS: [((f32, f32, f32), f32, f32, f32, (f32, f32, f32), (f32, f32, f32)); 2] = [
        ((671.5, 75.0, 43.5610), 118.5610, 1344., 2992., (465.38, 67.42, 65.41), (894.66, 68.57, 66.57)),
        ((539.5, 69.0, 32.8303), 101.8303, 1080., 2400., AKITA_GPS, AKITA_BT),
    ];
    const AKITA_GPS: (f32, f32, f32) = (419.28, 55.11, 52.80);
    const AKITA_BT: (f32, f32, f32) = (705.30, 57.61, 55.61);
    /// The glass's hairline, its outermost pixel row (main.rs `EDGE_HAIRLINE`), and a console circle's drawn reach past its radius (`console_draw`'s REACH), with half a pixel of a bead's own antialiasing.
    const HAIRLINE: f32 = 1.;
    const CONSOLE_REACH: f32 = 2.5 + 0.5;
    /// A thousandth of a pixel: what f32 keeps of positions near a thousand pixels, with room.
    const EPS: f32 = 1. / (1 << 10) as f32;

    #[test]
    fn beads_fit() {
        for &(hole, bottom, w, h, gps, bt) in &BOARDS {
            let a = app(w, h, bottom);
            let b = solve(hole, a);
            for side in 0..2 {
                // The lowest bead touches the app's circle from outside.
                let low = b.at[side][0];
                assert!((dist(low, (a.0, a.1)) - a.2 - b.bead).abs() < EPS, "lowest bead not tangent to the app's circle: {:?}", b);
                for j in 0..SLOTS {
                    let c = b.at[side][j];
                    // Every bead outside the app's circle, and tangent to the camera's disc.
                    assert!(dist(c, (a.0, a.1)) - a.2 - b.bead > -EPS, "a bead reaches into the app's circle: {:?}", b);
                    assert!((dist(c, (hole.0, hole.1)) - b.cam.2 - b.bead).abs() < EPS, "a bead off the camera's disc: {:?}", b);
                    // Each bead touches the one below.
                    if j > 0 {
                        assert!((dist(c, b.at[side][j - 1]) - 2. * b.bead).abs() < EPS, "beads not tangent: {:?}", b);
                    }
                }
                // The middle bead about level with the lens: within a bead's radius of the hole's centre line.
                assert!((b.at[side][1].1 - hole.1).abs() < b.bead, "the middle bead is not level with the lens: {:?}", b);
            }
            // Inside the glass: the top bead's top below the hairline.
            assert!(b.top() >= HAIRLINE, "a bead reaches the glass's edge: top {}", b.top());
            // Clear of the console's GPS and Bluetooth circles, their drawn reach included.
            assert!(b.gap(gps) > CONSOLE_REACH, "a bead meets GPS: {}", b.gap(gps));
            assert!(b.gap(bt) > CONSOLE_REACH, "a bead meets Bluetooth: {}", b.gap(bt));
        }
    }
}
