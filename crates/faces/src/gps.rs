//! The GPS puck's face: in focus, while it holds GPS, the sky as the chip sees it (each signal at its azimuth and elevation, green when used in the fix), the fix's accuracy and the time of the fix; at rest, what the last session found.
//! The position itself is never on the face: a puck never shows or keeps where the person is, only how well the device knows it.

use crate::{osc, u, RING, TEXT, TEXT2, TRACKED, USED};
use ferros_puck::{fmt::unit, q14, Full, Writer};

/// One signal: azimuth and elevation in degrees, used in the fix or only tracked, its carrier to noise in dB-Hz; or a bird only predicted from the orbits (GPS.md "Predicted birds": C/N0 0, not used).
#[derive(Clone, Copy, Debug, Default)]
pub struct Signal {
    pub az: u16,
    pub el: i8,
    pub used: bool,
    pub cn0: u8,
    pub predicted: bool,
}

/// A heard signal's weight by its C/N0 (dB-Hz): a logistic step about 28 dB-Hz (a signal a receiver can hold) four wide, at most half, as the console's GPS rings weigh it: about 0.06 at 20 dB-Hz, 0.48 at 40.
pub fn heard_weight(cn0: u8) -> f32 {
    0.5 / (1. + libm::expf(-(cn0 as f32 - 28.) * (1. / (1 << 2) as f32)))
}

/// A predicted bird's opacity at radius `r` of the plot (its horizon at 0.82): an eighth, faded toward the zenith by 1 − e^(−(r/r₀)²) with r₀ a quarter of the horizon's radius (C∞, 0 at the zenith), so the middle stays dark until there is real signal.
pub fn predicted_alpha(r: f32) -> u8 {
    let r0 = 0.82 / 4.;
    let fade = 1. - libm::expf(-(r * r) / (r0 * r0));
    (255. / (1 << 3) as f32 * fade) as u8
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
    let heard = s.signals.iter().filter(|g| !g.predicted).count() as i64;
    match s.fix {
        Some((err, t)) => {
            w.number(0, q14(-0.08), u(0.18), TEXT, (err as i64) << 16, 0, unit::METRE)?;
            w.moment(0, q14(0.16), u(0.08), TEXT2, osc(t), 2)?;
        }
        None => {}
    }
    w.number(q14(-0.3), q14(0.62), u(0.1), TEXT, used << 16, 0, unit::NONE)?;
    w.number(q14(0.3), q14(0.62), u(0.1), TEXT2, heard << 16, 0, unit::NONE)?;
    if focus {
        // The sky plot: the zenith at the middle, the horizon at 0.82 of the radius. Used in the fix: a full dot in the used colour; heard: a dot sized and weighted by its C/N0; predicted: a hollow ring at an eighth, faded toward the zenith (2026-11-18). The used first: the list is front to back.
        let mut order = [0usize; SIGNALS];
        let n = s.signals.len().min(SIGNALS);
        let mut k = 0;
        for pass in 0..3 {
            for (i, g) in s.signals[..n].iter().enumerate() {
                if (pass == 0 && g.used) || (pass == 1 && !g.used && !g.predicted) || (pass == 2 && g.predicted) {
                    order[k] = i;
                    k += 1;
                }
            }
        }
        for &i in &order[..k] {
            let g = &s.signals[i];
            let r = 0.82 * (90. - g.el as f32) / 90.;
            let th = g.az as f32 * (core::f32::consts::PI / 180.);
            let (x, y) = (r * libm::sinf(th), -r * libm::cosf(th));
            if g.used {
                w.disc(q14(x), q14(y), u(0.035), USED)?;
            } else if g.predicted {
                w.ring(q14(x), q14(y), u(0.03), u(0.022), TRACKED.alpha(predicted_alpha(r)))?;
            } else {
                let wt = heard_weight(g.cn0);
                w.disc(q14(x), q14(y), u(0.02 + 0.03 * wt), TRACKED.alpha((255. * (0.5 + wt)) as u8))?;
            }
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
