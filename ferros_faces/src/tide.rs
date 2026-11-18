//! The tide puck's face (PUCKS.md "The tide puck"), after the owner's e-ink tide clock (inksurf: tide-display, the Android widget's smoothed edition): the day's water as a filled curve on the station's fixed scale, now riding it, the next high's time and height below the curve and the next low's above it (each opposite the water, as inksurf puts its labels), the nights as dim bands behind it.
//!
//! The scale is the station's own, fixed: mean sea level plus and less the sum of its constituents' amplitudes, the most its tide can ever reach either way (inksurf draws on a fixed LAT..HAT axis for the same reason: a day's water is read against the station's range, never stretched to fill the box).

use crate::{band, osc, samples, sky, u, x_of, MARK, NIGHT, TEXT, TEXT2, WATER, WATER_LINE};
use ferros_puck::{fmt::unit, q14, Full, Writer};
use ferros_sky::{crossings, Body, Crossing, Place, SUNRISE};
use ferros_tide::{Face, Tide, CURVE, CURVE_BEFORE};

/// Where the curve sits: x from −X to X, the station's range from y = Y (its lowest) to −Y (its highest).
const X: f32 = 0.86;
const Y: f32 = 0.34;
/// Heights in the ops' values: Q16 of a metre.
const Q16: f32 = 65536.;

/// The station's fixed scale: (lowest, highest) its tide can reach, metres above its datum.
pub fn scale(tide: &Tide) -> (f32, f32) {
    let reach: f64 = tide.amp.iter().sum();
    ((tide.z0 - reach) as f32, (tide.z0 + reach) as f32)
}

/// What the face needs besides the tide's own face: the station's scale, the place (for its nights) and, in focus, the station's name and how far it is (metres).
pub struct About<'a> {
    pub scale: (f32, f32),
    pub place: Option<Place>,
    pub station: &'a str,
    pub distance_m: f64,
}

/// The face at rest (`focus` false) or in focus: the station's name and its distance added at the top and bottom.
pub fn face(w: &mut Writer, f: &Face, about: &About, focus: bool) -> Result<(), Full> {
    let span = crate::tide::span();
    let (lo, hi) = about.scale;
    // Words first (the frontmost): the height now at the top, the next high below the water and the next low above it, each at its own time across the curve.
    w.number(0, q14(-0.62), u(0.15), TEXT, (f.height as f32 * Q16) as i64, 1, unit::METRE)?;
    for (e, y) in [(f.next_high, 0.5), (f.next_low, -0.44)] {
        // Drawn where it happens across the curve, drawn in toward the middle so its words stay inside the circle: 0.6 tanh(x / 0.6), smooth (no clamp, AGENT.md "GUI Continuity") and near x itself near the middle.
        let x = 0.6 * libm::tanhf(x_of(e.t, f.start, span, -X, X) / 0.6);
        w.moment(q14(x), q14(y), u(0.11), if e.high { TEXT } else { TEXT2 }, osc(e.t), 1)?;
        w.number(q14(x), q14(y + 0.12), u(0.08), TEXT2, (e.h as f32 * Q16) as i64, 1, unit::METRE)?;
    }
    if focus {
        w.text(0, q14(-0.8), u(0.07), TEXT2, about.station)?;
        w.number(0, q14(0.8), u(0.07), TEXT2, (about.distance_m / 1000. * 65536.) as i64, 0, unit::KM)?;
    }
    // Now on the water, from the moment the face was made to the end of its curve: the display moves it.
    let curve_index = 2 + 4 + 2 * focus as u8;
    w.marker(curve_index, osc(f.start), osc(f.start + span), u(0.04), MARK)?;
    // The high and the low, each a hairline down thru the water (inksurf's red lines).
    let mut values = [(0i32, 0i32); CURVE];
    samples(&f.heights, &f.rates, Q16, &mut values);
    let i = w.curve(q14(-X), q14(X), q14(Y), q14(-Y), (lo * Q16) as i32, (hi * Q16) as i32, u(0.012), WATER_LINE, Some(WATER), 0, &values)?;
    debug_assert_eq!(i, curve_index);
    // The nights in the curve's day: the sun under the horizon, from the crossings of the sunrise level (a polar day has none and no band; a polar night is one band).
    if let Some(p) = about.place {
        let mut cs = [Crossing { t: 0., rising: false }; 1 << 3];
        let n = crossings(Body::Sun, f.start, f.start + span, p, SUNRISE, &mut cs);
        let below_at_start = ferros_sky::altitude(Body::Sun, f.start, p) < SUNRISE;
        let mut from = below_at_start.then_some(f.start);
        for c in &cs[..n] {
            match (c.rising, from) {
                (true, Some(a)) => {
                    band(w, x_of(a, f.start, span, -X, X), x_of(c.t, f.start, span, -X, X), NIGHT)?;
                    from = None;
                }
                (false, _) => from = Some(c.t),
                _ => {}
            }
        }
        if let Some(a) = from {
            band(w, x_of(a, f.start, span, -X, X), X, NIGHT)?;
        }
    }
    sky(w)?;
    Ok(())
}

/// The curve's span: a day, its 64 samples a 64th of a day apart, the last at 63/64 (ferros_tide `Face`).
pub fn span() -> f64 {
    ferros_sky::DAY * (CURVE - 1) as f64 / CURVE as f64
}

/// The face's next deadline (Eagle seconds): when its words go stale (the next high or low), or when now would ride off its curve, whichever is first.
pub fn deadline(f: &Face) -> f64 {
    f.until.min(f.made - CURVE_BEFORE + span())
}
