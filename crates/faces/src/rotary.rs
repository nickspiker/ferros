//! The rotary readout (owner, 2026-11-28: "would be nice to get some rotary goodness around the temp readout, magnetic strength, stuff like that"): one face for every gauge-like puck, a value as an arc round the rim and as a number in the middle.
//! The battery is its first reader; the timer, the thermometer's dial and the compass's field-strength ring are to share it.
//!
//! - **The track**: three quarters of the circle near the rim, open at the bottom, dim.
//! - **The fill**: from the track's start to the value's angle, in the face's colour, with a short needle across the track at the value.
//! - **The second value** (optional): a thin arc inside the track on the same scale (a trail, a target, a second reading).
//! - **The numbers**: the value large in the middle, its notes smaller under it, each in the person's base (`NUMBER`).
//! - **The angle** is a C² function of the value (`turns`): linear over the range, and past either end it runs on into the gap along a tanh, never reaching the gap's middle, so a value outside the range is still seen moving.

use crate::{u, TEXT, TEXT2};
use ferros_puck::{q14, turn_q16, Colour, Full, Writer};

/// The track's share of a turn (three quarters), and the gap at the bottom (the rest).
pub const TRACK: f32 = 0.75;
pub const GAP: f32 = 1. - TRACK;
/// How far past either end of the range the fill can run, as a share of a turn: half the gap, so its far end stays on its own side of the gap's middle (in f32 a value far enough out lands on the middle itself, as tanh rounds to 1), never round onto the track's start.
const RUN: f32 = GAP * 0.5;
/// The track's radius and width, the second value's, and the needle's ends and half-width, as fractions of the circle's radius.
const R_TRACK: f32 = 0.8;
const W_TRACK: f32 = 0.1;
const R_SECOND: f32 = 0.68;
const W_SECOND: f32 = 1. / (1 << 5) as f32;
const NEEDLE: (f32, f32) = (0.7, 0.92);
const W_NEEDLE: f32 = 1. / (1 << 6) as f32;

/// A smaller number under the large one (a reading beside the main one: the battery's current and temperature).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Note {
    pub value: f32,
    pub places: u8,
    /// `ferros_puck::fmt::unit`.
    pub unit: u8,
}

/// What the face shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rotary<'a> {
    /// The reading; None before the first, when the track alone is drawn.
    pub value: Option<f32>,
    /// The range the track spans, lo at its start and hi at its end; lo < hi is the caller's (an empty range divides by zero, and the fill and needle come out at the track's start).
    pub lo: f32,
    pub hi: f32,
    /// A second value on the same scale, drawn as a thin arc inside the track.
    pub second: Option<f32>,
    /// Which way the value runs: clockwise from the lower left (a gauge's way), or counterclockwise from the lower right.
    pub clockwise: bool,
    /// The large number's unit (`ferros_puck::fmt::unit`) and its places after the point.
    pub unit: u8,
    pub places: u8,
    /// The fill's and the needle's colour (`TEXT` for a plain reading; a puck tints a state, as the battery its charging).
    pub colour: Colour,
    /// The smaller numbers under the large one, top first; each carries one more place in focus.
    pub notes: &'a [Note],
}

/// The fill's length in turns from the track's start for `x`, the value as a share of its range (0 at lo, 1 at hi): x · TRACK on [0, 1]; past hi, TRACK + RUN · tanh((x − 1) · TRACK / RUN), and below lo, RUN · tanh(x · TRACK / RUN).
/// The joins are C² (the GUI continuity rule asks C¹): at each, value and slope match (tanh(0) = 0, tanh′(0) = 1, so the slope is TRACK on both sides) and so does the second derivative (tanh″(0) = 0, the line's is 0).
/// Bounded by its own form, not by a clamp: in (−RUN, TRACK + RUN), reaching the bound itself only once f32's tanh rounds to 1, so the fill is at most 7/8 of a turn and its Q16 sweep (57344 at most) fits a u16.
pub fn turns(x: f32) -> f32 {
    if x > 1. {
        TRACK + RUN * libm::tanhf((x - 1.) * (TRACK / RUN))
    } else if x < 0. {
        RUN * libm::tanhf(x * (TRACK / RUN))
    } else {
        x * TRACK
    }
}

/// Where the value starts and which way it runs: the angle the track starts at (a turn clockwise from the circle's up), and which way the value runs (+1 clockwise, −1 counterclockwise).
fn start(clockwise: bool) -> (f32, f32) {
    // Clockwise starts at the gap's left edge (the lower left), counterclockwise at its right edge (the lower right); the track is the same three quarters either way.
    if clockwise { (0.5 + GAP * 0.5, 1.) } else { (0.5 - GAP * 0.5, -1.) }
}

/// The point at turn `a` (clockwise from the circle's up) and radius r, in Q14.
fn at(a: f32, r: f32) -> (i16, i16) {
    let th = a * core::f32::consts::TAU;
    (q14(r * libm::sinf(th)), q14(-r * libm::cosf(th)))
}

/// An arc from the track's start along the value's way for `len` turns (negative: back past the start), with a disc at the start as wide as the band: as `len` shrinks to 0 the arc's round end shrinks onto that disc, so nothing vanishes at once at the range's bottom (the arc op draws nothing for a sweep of 0).
fn from_start(w: &mut Writer, a0: f32, dir: f32, len: f32, r: f32, width: f32, c: Colour) -> Result<(), Full> {
    let end = a0 + dir * len;
    // The arc op runs clockwise from `from` for `sweep`: the earlier of the two ends (clockwise) is where it starts, and the distance between them its length. This orders the two ends, it bounds nothing: both ends move smoothly with the value, and so does the band between them.
    let (from, sweep) = if end >= a0 { (a0, end - a0) } else { (end, a0 - end) };
    w.arc(0, 0, u(r), u(width), turn_q16(from as f64), (sweep * 65536.) as u16, c)?;
    let (x, y) = at(a0, r);
    w.disc(x, y, u(width * 0.5), c)?;
    Ok(())
}

/// The value as Q16 of its unit, as `NUMBER` takes it.
fn q16(v: f32) -> i64 {
    (v * (1 << 16) as f32) as i64
}

pub fn face(w: &mut Writer, r: &Rotary, focus: bool) -> Result<(), Full> {
    let (a0, dir) = start(r.clockwise);
    let share = |v: f32| (v - r.lo) / (r.hi - r.lo);
    if let Some(v) = r.value {
        w.number(0, q14(0.02), u(0.24), TEXT, q16(v), r.places, r.unit)?;
    }
    for (k, n) in r.notes.iter().enumerate() {
        w.number(0, q14(0.32 + 0.16 * k as f32), u(0.1), TEXT2, q16(n.value), n.places + focus as u8, n.unit)?;
    }
    if let Some(v) = r.value {
        let len = turns(share(v));
        let a = a0 + dir * len;
        w.capsule(at(a, NEEDLE.0), at(a, NEEDLE.1), u(W_NEEDLE), r.colour)?;
        from_start(w, a0, dir, len, R_TRACK, W_TRACK, r.colour)?;
    }
    if let Some(s) = r.second {
        from_start(w, a0, dir, turns(share(s)), R_SECOND, W_SECOND, TEXT2)?;
    }
    // In focus, the range's quarters as ticks outside the track.
    if focus {
        for k in 0..=4 {
            let a = a0 + dir * TRACK * k as f32 * 0.25;
            w.capsule(at(a, R_TRACK + W_TRACK * 0.5 + 0.03), at(a, R_TRACK + W_TRACK * 0.5 + 0.08), u(1. / (1 << 7) as f32), TEXT2)?;
        }
    }
    // The track, dim: the same three quarters whichever way the value runs.
    w.arc(0, 0, u(R_TRACK), u(W_TRACK), turn_q16((0.5 + GAP * 0.5) as f64), (TRACK * 65536.) as u16, TEXT.alpha(1 << 6))?;
    crate::sky(w)?;
    Ok(())
}
