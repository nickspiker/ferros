//! The GPS puck's face: in focus, while it holds GPS, the sky as the chip sees it (each signal at its azimuth and elevation, green when used in the fix), the fix's accuracy and the time of the fix; at rest, what the last session found.
//! The position itself is never on the face: a puck never shows or keeps where the person is, only how well the device knows it.

use crate::{osc, u, RING, TEXT, TEXT2, TRACKED, USED};
use ferros_puck::{fmt::unit, q14, Full, Writer};

/// One signal: azimuth and elevation in degrees, used in the fix or only tracked.
#[derive(Clone, Copy, Debug, Default)]
pub struct Signal {
    pub az: u16,
    pub el: i8,
    pub used: bool,
}

/// What the face shows: the signals (at most `SIGNALS` drawn), the fix's error in metres and its time (Eagle seconds), when there is one.
pub struct Sky<'a> {
    pub signals: &'a [Signal],
    pub fix: Option<(u32, f64)>,
    /// Holding GPS now (in focus, claimed).
    pub live: bool,
}

/// The most signals drawn: the list's own bound leaves room for the rest of the face.
pub const SIGNALS: usize = 1 << 5;

pub fn face(w: &mut Writer, s: &Sky, focus: bool) -> Result<(), Full> {
    let used = s.signals.iter().filter(|g| g.used).count() as i64;
    match s.fix {
        Some((err, t)) => {
            w.number(0, q14(-0.08), u(0.18), TEXT, (err as i64) << 16, 0, unit::METRE)?;
            w.moment(0, q14(0.16), u(0.08), TEXT2, osc(t), 2)?;
        }
        None => {}
    }
    w.number(q14(-0.3), q14(0.62), u(0.1), TEXT, used << 16, 0, unit::NONE)?;
    w.number(q14(0.3), q14(0.62), u(0.1), TEXT2, (s.signals.len() as i64) << 16, 0, unit::NONE)?;
    if focus {
        // The sky plot: the zenith at the middle, the horizon at 0.82 of the radius; each signal a dot.
        for g in s.signals.iter().take(SIGNALS) {
            let r = 0.82 * (90. - g.el as f32) / 90.;
            let th = g.az as f32 * (core::f32::consts::PI / 180.);
            let (x, y) = (r * libm::sinf(th), -r * libm::cosf(th));
            w.disc(q14(x), q14(y), u(0.035), if g.used { USED } else { TRACKED })?;
        }
        // Elevations 60 and 30 degrees, and the horizon.
        for r in [0.82 / 3., 0.82 * 2. / 3., 0.82] {
            w.ring(0, 0, u(r + 0.004), u(r - 0.004), RING.alpha(1 << 7))?;
        }
    }
    // Live: a ring of the theme's text round the rim says the radio is on for this puck.
    if s.live {
        w.ring(0, 0, u(0.96), u(0.93), USED)?;
    }
    crate::sky(w)?;
    Ok(())
}
