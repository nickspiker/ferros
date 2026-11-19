//! The faces of ferros's own pucks (docs/PUCKS.md; owner, 2026-11-17: "plus a tide puck", "GPS, compass, shit that's useful!"): each engine's values written as a ferros_puck list, at rest and in focus (the fuller face).
//!
//! - **Words.** No face writes a human word: every time is a `MOMENT` and every measure a `NUMBER`, which the display formats in the person's base and zone; what a value means is said by where it sits and what it is drawn with (the tide's high below its curve and low above it, as the owner's e-ink tide clock draws them; sunrise on the left, sunset on the right). The one exception is a tide station's name in focus, which is a place's name as its source publishes it.
//! - **Colours.** VSF RGB gamma-2 codes (AGENT.md, memory feedback_vsf_rgb_only), or the person's theme; the water's, the sky's and the marker's are provisional picks for the owner (PUCKS.md question 9).
//! - **Order.** Front first: the words, then the markers, then the curves, then the bands, then the sky, as ferros_puck draws them (`under`).
//! - **Sizes.** Every coordinate a fraction of the circle's radius (`q14`), so a face is the same at every zoom.
//! - **Time.** Engines count Eagle seconds; ops take Eagle oscillations (`osc`).

#![no_std]

pub mod compass;
pub mod gps;
pub mod sky;
pub mod tide;
pub mod weather;
#[cfg(test)]
mod tests;

use ferros_puck::{q14, Colour, Full, Writer, OSC};

/// Eagle seconds (the engines') as Eagle oscillations (the ops').
pub fn osc(secs: f64) -> i64 {
    (secs * OSC as f64) as i64
}

/// A length as a fraction of the radius, unsigned (radii, widths, sizes).
pub fn u(f: f32) -> u16 {
    q14(f) as u16
}

/// The deep sky every face sits on: VSF RGB gamma 2, a blue so dark it reads as night on a lit panel and does not glow on a dark one (provisional, the owner's pick).
pub const SKY: Colour = Colour::rgb(6, 10, 26);
/// Night on a curve (the sun below the horizon): the sky's black at a third, laid behind the curve so the water and the line stay as they are (inksurf inverts its night columns; a dim band is the same reading on an emitting panel).
pub const NIGHT: Colour = Colour::Rgb { r: 0, g: 0, b: 0, a: 1 << 7 };
/// The water (provisional): a sea blue, and its surface line lighter.
pub const WATER: Colour = Colour::rgb(24, 92, 168);
pub const WATER_LINE: Colour = Colour::rgb(120, 196, 236);
/// A marker at now (the tide's, the sun's when no sun colour is wanted).
pub const MARK: Colour = Colour::rgb(250, 250, 250);
/// The sun, and its golden and blue hours as translucent bands.
pub const SUN: Colour = Colour::rgb(255, 196, 40);
pub const GOLDEN: Colour = Colour::Rgb { r: 255, g: 150, b: 30, a: 1 << 6 };
pub const BLUE_HOUR: Colour = Colour::Rgb { r: 40, g: 80, b: 255, a: 1 << 6 };
/// The moon's lit face.
pub const MOONLIGHT: Colour = Colour::rgb(232, 230, 214);
/// North on a dial.
pub const NORTH: Colour = Colour::rgb(230, 40, 28);
/// A satellite used in the fix, and one only tracked.
pub const USED: Colour = Colour::rgb(60, 220, 120);
pub const TRACKED: Colour = Colour::rgb(120, 120, 120);
/// Rain and snow.
pub const RAIN: Colour = Colour::rgb(70, 140, 255);
pub const SNOW: Colour = Colour::rgb(240, 244, 255);
pub const CLOUD: Colour = Colour::rgb(150, 156, 170);

/// The theme's text, its quieter text, and its ring.
pub const TEXT: Colour = Colour::theme(ferros_puck::theme::TEXT);
pub const TEXT2: Colour = Colour::theme(ferros_puck::theme::TEXT2);
pub const RING: Colour = Colour::theme(ferros_puck::theme::RING);

/// A curve's samples from an engine's (value, rate per sample step), as the op takes them: values scaled by `scale` to i32, slopes per the whole span (n − 1 steps).
pub fn samples<const N: usize>(values: &[f32; N], rates: &[f32; N], scale: f32, out: &mut [(i32, i32); N]) {
    let steps = (N - 1) as f32;
    for k in 0..N {
        out[k] = ((values[k] * scale) as i32, (rates[k] * scale * steps) as i32);
    }
}

/// x on a curve spanning [x0, x1] for time `t` in [start, start + span] (Eagle seconds).
pub fn x_of(t: f64, start: f64, span: f64, x0: f32, x1: f32) -> f32 {
    x0 + ((t - start) / span) as f32 * (x1 - x0)
}

/// A vertical band across the circle from x = a to x = b (a thick capsule, its round ends beyond the circle, which clips them).
pub fn band(w: &mut Writer, a: f32, b: f32, c: Colour) -> Result<u8, Full> {
    let (m, hw) = ((a + b) * 0.5, (b - a).abs() * 0.5);
    w.capsule((q14(m), q14(-1.5)), (q14(m), q14(1.5)), u(hw), c)
}

/// The background every face ends with: its sky.
pub fn sky(w: &mut Writer) -> Result<u8, Full> {
    w.disc(0, 0, u(1.), SKY)
}
