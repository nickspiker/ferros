//! The compass puck's face: a dial that turns so its north points north, a fixed mark for the device's top edge, the true heading as a number; in focus the declination and how far each axis's calibration has come.
//! No letters for the points (they are words, and a language's): north is the red tick, the others ticks of the theme.

use crate::{u, NORTH, RING, TEXT, TEXT2};
use ferros_puck::{fmt::unit, q14, turn_q16, Colour, Full, Writer};

/// What the face shows: the true heading of the device's top edge (radians clockwise from true north; None while it is unknown: upright, or no field), the declination (radians, east positive), each axis's calibration coverage (0 to 1).
pub struct Compass {
    pub heading: Option<f64>,
    pub declination: f64,
    pub coverage: [f32; 3],
}

/// The point at turn `a` (clockwise from the circle's up) and radius r, in Q14.
fn at(a: f64, r: f32) -> (i16, i16) {
    let th = a * core::f64::consts::TAU;
    (q14(r * libm::sin(th) as f32), q14(-r * libm::cos(th) as f32))
}

pub fn face(w: &mut Writer, c: &Compass, focus: bool) -> Result<(), Full> {
    if let Some(h) = c.heading {
        w.number(0, q14(0.08), u(0.2), TEXT, (h.to_degrees() * 65536.) as i64, 0, unit::DEGREE)?;
    }
    if focus {
        w.number(0, q14(0.4), u(0.08), TEXT2, (c.declination.to_degrees() * 65536.) as i64, 1, unit::DEGREE)?;
        // Each axis's calibration as an arc's length near the rim, a third of the circle each: full when that axis has been turned thru a whole field.
        for (k, cov) in c.coverage.iter().enumerate() {
            let from = k as f64 / 3. + 1. / 48.;
            w.arc(0, 0, u(0.66), u(0.02), turn_q16(from), (cov * (1. / 3. - 1. / 24.) * 65536.) as u16, TEXT2.alpha(1 << 7))?;
        }
    }
    // The device's top: a fixed bar at the top of the dial.
    w.capsule((0, q14(-0.92)), (0, q14(-0.7)), u(0.025), TEXT)?;
    // The dial, turned by the heading so its north tick points north: 2^4 ticks, north red and longer, the quarters longer than the rest. Unknown heading: the dial as it last was is not shown; the ticks stand at rest, dimmed.
    let (turn, dim) = match c.heading {
        Some(h) => (-h / core::f64::consts::TAU, 255u8),
        None => (0., 1 << 6),
    };
    for k in 0..1 << 4 {
        let a = turn + k as f64 / (1 << 4) as f64;
        let (inner, colour, width): (f32, Colour, f32) = match k {
            0 => (0.62, NORTH, 0.03),
            4 | 8 | 12 => (0.7, TEXT, 0.02),
            _ => (0.76, RING, 0.012),
        };
        w.capsule(at(a, inner), at(a, 0.84), u(width), colour.alpha(dim))?;
    }
    w.ring(0, 0, u(0.9), u(0.88), RING)?;
    crate::sky(w)?;
    Ok(())
}
