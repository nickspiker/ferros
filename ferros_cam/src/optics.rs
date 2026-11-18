//! Each camera's optics as the list carries them (owner, 2026-11-17: "Missing lens stats on the Lumis page"; docs/CAMERA.md "The endpoint"): the sensor's physical size, the lens's focal length and f-number, by camera id (ferros_cam cams).
//! Every value is Android's Camera2 report, recorded by Lumis on the Pro, said per field below; none is a ferros measurement.
//! - Sensor size: SENSOR_INFO_PHYSICAL_SIZE, enumerated by Lumis on the Pro (Lumis docs/google-bug-tele-maxres-fov.md, its table of physical cameras 2, 3, 4); the front camera's is not in that table and is derived from its recorded 35 mm equivalent (below).
//! - Focal length and f-number: LENS_FOCAL_LENGTH and LENS_APERTURE as Lumis wrote them into each camera's calibration capture (`pixel 8 pro calibration data/*average.vsf`, fields `focal_mm`, `f_number`, 2026-10-21; local, only these numbers read).
//! - The 35 mm equivalents in the same captures agree with the sizes: 43.27 mm x focal / diagonal gives the main 24.36 (recorded 24.358), the ultrawide 12.06 (12.062), the tele 110.4 (110.38).
//! Not recorded anywhere found: the nearest focus distance (Camera2 LENS_INFO_MINIMUM_FOCUS_DISTANCE: Lumis's captures and DNGs carry no focus field) and whether the lens is stabilised for ferros (the main's OIS part sits on its bus, G#24, and ferros does not drive it).

/// One camera's optics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Optics {
    /// The sensor's active size in millimetres, width and height (for the readout's whole field).
    pub sensor_mm: (f32, f32),
    pub focal_mm: f32,
    pub f_number: f32,
}

/// By camera id: the main (Camera2 physical camera 2), the ultrawide (3), the front (7), the tele (4).
pub const OPTICS: [Optics; 4] = [
    Optics { sensor_mm: (9.792, 7.3728), focal_mm: 6.9, f_number: 1.68 },
    Optics { sensor_mm: (6.4, 4.8), focal_mm: 2.23, f_number: 1.95 },
    // The front's size from its 35 mm equivalent (20.6108 at 2.74 mm): a diagonal of 43.2666 x 2.74 / 20.6108 = 5.7519 mm, split at its readout's 3840 : 2736.
    Optics { sensor_mm: (4.6846, 3.3378), focal_mm: 2.74, f_number: 2.2 },
    Optics { sensor_mm: (5.6448, 4.2336), focal_mm: 18.0, f_number: 2.8 },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The 35 mm equivalents the captures recorded follow from the sizes and focal lengths (the full frame's diagonal 43.2666 mm).
    #[test]
    fn the_35mm_equivalents_agree() {
        for (o, recorded) in OPTICS.iter().zip([24.357971f32, 12.061513, 20.610819, 110.382652]) {
            let diag = (o.sensor_mm.0 * o.sensor_mm.0 + o.sensor_mm.1 * o.sensor_mm.1).sqrt();
            let eq = 43.2666 * o.focal_mm / diag;
            assert!((eq - recorded).abs() < 0.05, "{o:?}: {eq} against {recorded}");
        }
    }
}
