//! The sun and moon pucks' faces: the body's altitude over the local day as a curve against the horizon, now riding it, the day's times as moments where they happen; the sun's golden and blue hours as bands, the moon drawn lit as it is.

use crate::{band, osc, samples, sky, u, x_of, BLUE_HOUR, GOLDEN, MOONLIGHT, RING, SKY, SUN, TEXT, TEXT2};
use ferros_puck::{fmt::unit, q14, turn_q16, Full, Writer};
use ferros_sky::{Curve, MoonDay, Pass, SunDay, CURVE};

/// The curve's box: x from −X to X, the altitude's range over the box's height about the horizon line.
const X: f32 = 0.86;
/// Altitudes in the ops' values: radians times 2^20.
const RAD: f32 = (1 << 20) as f32;
/// A right angle, the most an altitude reaches either way: the scale's fixed ends, so every day is read against the same sky (a low winter sun stays low on the face).
const QUARTER: f32 = core::f32::consts::FRAC_PI_2;

/// The altitude curve (into the list, with a marker riding it at now), the horizon across it, between y = 0.34 (the nadir) and y = −0.34 (the zenith).
fn altitude(w: &mut Writer, c: &Curve, line: ferros_puck::Colour, marker: ferros_puck::Colour, curve_index: u8) -> Result<(), Full> {
    let span = c.step * (CURVE - 1) as f64;
    w.marker(curve_index, osc(c.start), osc(c.start + span), u(0.055), marker)?;
    let mut values = [(0i32, 0i32); CURVE];
    samples(&c.alt, &c.rate, RAD, &mut values);
    let i = w.curve(q14(-X), q14(X), q14(0.34), q14(-0.34), (-QUARTER * RAD) as i32, (QUARTER * RAD) as i32, u(0.014), line, None, 0, &values)?;
    debug_assert_eq!(i, curve_index);
    // The horizon: the middle of the scale.
    w.capsule((q14(-X), 0), (q14(X), 0), u(0.006), RING)?;
    Ok(())
}

/// The rise and set of a pass as moments at the bottom left and right (sunrise where the curve starts climbing, sunset where it ends), none for a polar day or night.
fn rise_set(w: &mut Writer, pass: &Pass, y: f32, size: f32, colour: ferros_puck::Colour) -> Result<(), Full> {
    if let Pass::Crosses { rise, set } = pass {
        if let Some(r) = rise {
            w.moment(q14(-0.42), q14(y), u(size), colour, osc(*r), 1)?;
        }
        if let Some(s) = set {
            w.moment(q14(0.42), q14(y), u(size), colour, osc(*s), 1)?;
        }
    }
    Ok(())
}

/// The sun's face: at rest the noon altitude at the top, sunrise and sunset at the bottom, the sun on its curve, golden and blue hours as bands; in focus the civil dawn and dusk under them.
pub fn sun(w: &mut Writer, d: &SunDay, focus: bool) -> Result<(), Full> {
    w.number(0, q14(-0.62), u(0.12), TEXT, (d.noon_altitude.to_degrees() as f32 * 65536.) as i64, 0, unit::DEGREE)?;
    rise_set(w, &d.sun, 0.52, 0.12, TEXT)?;
    if focus {
        rise_set(w, &d.civil, 0.68, 0.08, TEXT2)?;
    }
    let ops_before = w.count() + 1;
    altitude(w, &d.curve, SUN, SUN, ops_before)?;
    let span = d.curve.step * (CURVE - 1) as f64;
    for (bands, c) in [(&d.golden, GOLDEN), (&d.blue, BLUE_HOUR)] {
        for s in bands.spans() {
            band(w, x_of(s.from, d.curve.start, span, -X, X), x_of(s.to, d.curve.start, span, -X, X), c)?;
        }
    }
    sky(w)?;
    Ok(())
}

/// The moon drawn lit as it is: its disc in moonlight, the dark side the sky's own colour in front of it. The terminator is an ellipse's half, so the lit part between the bright limb and it is exact: with the lit limb up, x across the disc and s(x) = sqrt(r² − x²), the lit part is −s ≤ y ≤ k·s with k = 2·fraction − 1 (full at k = 1, new at k = −1, its area fraction·πr²).
/// That dark side is a curve filled from k·s down to the disc's bottom, turned so the lit limb points where the moon's does in the sky (`zenith_limb`, from straight up), in the sky's colour, so outside the disc it is sky on sky.
fn moon_disc(w: &mut Writer, cx: f32, cy: f32, r: f32, fraction: f64, zenith_limb: f64) -> Result<(), Full> {
    const N: usize = 1 << 6;
    let k = 2. * fraction as f32 - 1.;
    // The ends just inside the limb, where s's slope is finite.
    let e = r * (1. - 1. / (1 << 7) as f32);
    let mut values = [(0i32, 0i32); N];
    for (j, v) in values.iter_mut().enumerate() {
        let x = -e + 2. * e * j as f32 / (N - 1) as f32;
        let s = libm::sqrtf(r * r - x * x);
        // The terminator at y = k·s on the glass (y down), which is value −k·s on the curve's scale (value v sits at y = −v); its slope d(−k·s)/dx = k·x / s, per the whole span (2e).
        *v = ((-k * s * RAD) as i32, (k * x / s * 2. * e * RAD) as i32);
    }
    // The turned frame is about the circle's centre: the moon sits there.
    debug_assert!(cx == 0. && cy == 0.);
    // Lit limb up is the curve's frame turned by zero. χ − q is a position angle, counted from the zenith toward the east, which on the sky as seen facing it is anticlockwise: on the glass the drawing turns the other way.
    let turn = turn_q16(-zenith_limb / core::f64::consts::TAU);
    // The box: value −r at y = r (the disc's bottom, where the dark fill ends) and r at y = −r (its top).
    w.curve(q14(-e), q14(e), q14(r), q14(-r), (-r * RAD) as i32, (r * RAD) as i32, 0, SKY, Some(SKY), turn, &values)?;
    w.disc(q14(cx), q14(cy), u(r), MOONLIGHT)?;
    Ok(())
}

/// The moon's face: its disc lit as it is now (made at `d.made`, redrawn at the puck's deadlines), the illuminated share under it, the next principal phase's moment over it; in focus moonrise and moonset at the bottom and its altitude curve behind.
pub fn moon(w: &mut Writer, d: &MoonDay, focus: bool) -> Result<(), Full> {
    w.number(0, q14(0.6), u(0.11), TEXT, (d.light.fraction * 100. * 65536.) as i64, 0, unit::PERCENT)?;
    w.moment(0, q14(-0.62), u(0.09), TEXT2, osc(d.next_phase_at), 1)?;
    if focus {
        rise_set(w, &d.moon, 0.8, 0.08, TEXT2)?;
    }
    moon_disc(w, 0., 0., 0.4, d.light.fraction, d.light.zenith_limb)?;
    if focus {
        let ops_before = w.count() + 1;
        altitude(w, &d.curve, TEXT2, MOONLIGHT, ops_before)?;
    }
    sky(w)?;
    Ok(())
}
