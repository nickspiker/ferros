//! Each camera's optics as the list carries them (owner, 2026-11-17: "Missing lens stats on the Lumis page"; docs/CAMERA.md "The endpoint"): the sensor's physical size, the lens's focal length and f-number, by board and camera id (ferros_cam cams `Cam::optics`: the Pro's `OPTICS`, the 8a's `AKITA_OPTICS`).
//! Every value is Android's Camera2 report, recorded by Lumis on the Pro, said per field below (the 8a's at `AKITA_OPTICS`); none is a ferros measurement.
//! - Sensor size: SENSOR_INFO_PHYSICAL_SIZE, enumerated by Lumis on the Pro (Lumis docs/google-bug-tele-maxres-fov.md, its table of physical cameras 2, 3, 4); the front camera's is not in that table and is derived from its recorded 35 mm equivalent (below).
//! - Focal length and f-number: LENS_FOCAL_LENGTH and LENS_APERTURE as Lumis wrote them into each camera's calibration capture (`pixel 8 pro calibration data/*average.vsf`, fields `focal_mm`, `f_number`, 2026-10-21; local, only these numbers read).
//! - The 35 mm equivalents in the same captures agree with the sizes: 43.27 mm x focal / diagonal gives the main 24.36 (recorded 24.358), the ultrawide 12.06 (12.062), the tele 110.4 (110.38).
//! Not recorded anywhere found: the nearest focus distance (Camera2 LENS_INFO_MINIMUM_FOCUS_DISTANCE: Lumis's captures and DNGs carry no focus field) and whether the lens is stabilised for ferros (the main's OIS part sits on its bus, G#24, and ferros does not drive it).
//!
//! A readout's field (`Optics::field`, docs/CAMERA.md "A readout's field of view"): the part of the sensor it reads, in pixels of the array `sensor_mm` measures (`array_px`), scaled to millimetres, and the angles the lens gives that part. Two readouts of one lens with the same size tell apart by it (the tele's 4032 x 3024 at 22 degrees, the whole array binned, and at 11, its middle quarter at 1:1).

/// One camera's optics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Optics {
    /// The sensor's active size in millimetres, width and height (for the readout's whole field).
    pub sensor_mm: (f32, f32),
    /// The pixels `sensor_mm` spans, width and height: the rear sensors' maximum resolution (Camera2 PIXEL_ARRAY_MAXIMUM_RESOLUTION: 8160 x 6144 at 1.2 um gives 9.792 x 7.3728 mm), the front's 10.5 MP readout (the size its millimetres were split at).
    pub array_px: (u32, u32),
    pub focal_mm: f32,
    pub f_number: f32,
}

/// By camera id: the main (Camera2 physical camera 2), the ultrawide (3), the front (7), the tele (4).
pub const OPTICS: [Optics; 4] = [
    Optics { sensor_mm: (9.792, 7.3728), array_px: (8160, 6144), focal_mm: 6.9, f_number: 1.68 },
    Optics { sensor_mm: (6.4, 4.8), array_px: (8000, 6000), focal_mm: 2.23, f_number: 1.95 },
    // The front's size from its 35 mm equivalent (20.6108 at 2.74 mm): a diagonal of 43.2666 x 2.74 / 20.6108 = 5.7519 mm, split at its readout's 3840 : 2736.
    Optics { sensor_mm: (4.6846, 3.3378), array_px: (3840, 2736), focal_mm: 2.74, f_number: 2.2 },
    Optics { sensor_mm: (5.6448, 4.2336), array_px: (8064, 6048), focal_mm: 18.0, f_number: 2.8 },
];

/// The 8a's (akita) by camera id (ferros_cam cams `AKITA`): the main (Camera2 physical camera 2), the ultrawide (3), the front (logical camera 1).
/// Each is Android's Camera2 report on the 8a (`dumpsys media.camera`, 2026-11-18, keys/akita/captures-2026-11-18/dumpsys-media-camera.txt): SENSOR_INFO_PHYSICAL_SIZE over SENSOR_INFO_PIXEL_ARRAY_SIZE, LENS_INFO_AVAILABLE_FOCAL_LENGTHS, LENS_INFO_AVAILABLE_APERTURES.
/// The 8a lists no maximum resolution, so each array is the readout ferros streams (the main's 1.6 um pixels, the IMX712s' 1.12 um), and every readout gathers one array pixel a pixel.
/// The main's 2x (physical camera 4) is the same lens with half the main's size, 3.6992 x 2.7776 mm: its middle quarter, not a readout of ferros's.
pub const AKITA_OPTICS: [Optics; 3] = [
    Optics { sensor_mm: (7.3984, 5.5552), array_px: (4624, 3472), focal_mm: 5.43, f_number: 1.89 },
    Optics { sensor_mm: (4.71296, 3.4944), array_px: (4208, 3120), focal_mm: 1.835, f_number: 2.2 },
    Optics { sensor_mm: (4.71296, 3.44064), array_px: (4208, 3072), focal_mm: 2.74, f_number: 2.2 },
];

/// The full frame's diagonal (36 x 24 mm), what a 35 mm equivalent focal length is measured against.
pub const FULL_FRAME_DIAGONAL_MM: f64 = 43.266615305567875;

/// What a readout sees: the sensor's part it reads (mm, width and height), the angles across its width, its height and its diagonal (degrees), and the focal length that gives the same diagonal angle on the full frame (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub mm: (f32, f32),
    pub fov_deg: (f32, f32, f32),
    pub focal_equiv_mm: f32,
}

impl Optics {
    /// The field of a readout that reads `covered` pixels of the array (`array_px`), width and height: its output size times the array pixels each output pixel gathers (2 binned, 1 at 1:1), so a crop narrows it and binning does not.
    pub fn field(&self, covered: (u32, u32)) -> Field {
        let w = self.sensor_mm.0 as f64 * covered.0 as f64 / self.array_px.0 as f64;
        let h = self.sensor_mm.1 as f64 * covered.1 as f64 / self.array_px.1 as f64;
        let d = sqrt(w * w + h * h);
        let f = self.focal_mm as f64;
        let angle = |size: f64| (2. * atan(size / (2. * f)) * (180. / core::f64::consts::PI)) as f32;
        Field { mm: (w as f32, h as f32), fov_deg: (angle(w), angle(h), angle(d)), focal_equiv_mm: (FULL_FRAME_DIAGONAL_MM * f / d) as f32 }
    }
}

/// The arctangent of `x` (radians), for `x` at 0 or more (a size over a distance): core has none without std.
/// Folded onto [0, 1] by atan(x) = pi/2 - atan(1/x), then onto |u| <= tan(pi/8) by atan(x) = pi/4 + atan((x - 1) / (x + 1)), where the series u - u^3/3 + u^5/5 ... loses a factor of tan(pi/8)^2 (about 1/6) a term: 2^5 terms take it far past f64's 53 bits.
pub fn atan(x: f64) -> f64 {
    use core::f64::consts::{FRAC_PI_2, FRAC_PI_4};
    // tan(pi/8) = sqrt(2) - 1, the fold's point: both halves of [0, 1] come within it.
    const TAN_PI_8: f64 = core::f64::consts::SQRT_2 - 1.;
    let (far, x) = if x > 1. { (true, 1. / x) } else { (false, x) };
    let (base, u) = if x > TAN_PI_8 { (FRAC_PI_4, (x - 1.) / (x + 1.)) } else { (0., x) };
    let u2 = u * u;
    let mut term = u;
    let mut sum = 0.;
    for k in 0..1u32 << 5 {
        sum += term / (2 * k + 1) as f64;
        term *= -u2;
    }
    let r = base + sum;
    if far { FRAC_PI_2 - r } else { r }
}

/// The square root of `v` (`v` above 0: a diagonal's square), likewise: half the exponent from the bits, then Newton's steps, each doubling the correct bits (the first guess within about 6 %: 2^2 steps reach f64's 53).
pub fn sqrt(v: f64) -> f64 {
    let mut x = f64::from_bits((v.to_bits() >> 1) + (1023 << 51));
    for _ in 0..1 << 2 {
        x = (x + v / x) / 2.;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 35 mm equivalents the captures recorded follow from the sizes and focal lengths (the full frame's diagonal 43.2666 mm).
    #[test]
    fn the_35mm_equivalents_agree() {
        for (o, recorded) in OPTICS.iter().zip([24.357971f32, 12.061513, 20.610819, 110.382652]) {
            let f = o.field(o.array_px);
            assert!((f.focal_equiv_mm - recorded).abs() < 0.05, "{o:?}: {} against {recorded}", f.focal_equiv_mm);
        }
    }

    /// The fold and the series against std's, over sizes from a hundredth to a hundred times the distance; the square root likewise.
    #[test]
    fn atan_and_sqrt_match_std() {
        let mut x = 1. / ((1 << 7) as f64);
        while x < (1 << 7) as f64 {
            assert!((atan(x) - x.atan()).abs() < 1e-14, "atan {x}: {} against {}", atan(x), x.atan());
            assert!((sqrt(x) - x.sqrt()).abs() < 1e-14 * x.sqrt(), "sqrt {x}");
            x *= 1.0625;
        }
        assert_eq!((atan(0.), atan(1.)), (0., core::f64::consts::FRAC_PI_4));
    }

    /// The whole arrays' diagonal angles as Android computes them from the same sizes (Lumis docs/google-bug-tele-maxres-fov.md: the main 83.2, the ultrawide 121.7, the tele 22.2; the tele's virtual camera, its middle quarter, 11.2).
    #[test]
    fn the_diagonal_angles_are_androids() {
        for (k, want) in [(0usize, 83.2f32), (1, 121.7), (3, 22.2)] {
            let o = OPTICS[k];
            assert!((o.field(o.array_px).fov_deg.2 - want).abs() < 0.05, "camera {k}: {:?}", o.field(o.array_px));
        }
        let t = OPTICS[3];
        assert!((t.field((t.array_px.0 / 2, t.array_px.1 / 2)).fov_deg.2 - 11.2).abs() < 0.05);
    }

    /// The 8a's sizes hold together: the main's middle quarter is its 2x's reported size (physical camera 4, 3.6992 x 2.7776 mm), and the pixels are the parts' own (the main 1.6 um, the IMX712s 1.12 um).
    #[test]
    fn the_8a_sizes_agree() {
        let m = AKITA_OPTICS[0];
        let q = m.field((m.array_px.0 / 2, m.array_px.1 / 2));
        assert!((q.mm.0 - 3.6992).abs() < 1e-4 && (q.mm.1 - 2.7776).abs() < 1e-4, "{q:?}");
        for (o, um) in AKITA_OPTICS.iter().zip([1.6f32, 1.12, 1.12]) {
            assert!((o.sensor_mm.0 * 1000. / o.array_px.0 as f32 - um).abs() < 1e-4 && (o.sensor_mm.1 * 1000. / o.array_px.1 as f32 - um).abs() < 1e-4, "{o:?}");
        }
    }
}
