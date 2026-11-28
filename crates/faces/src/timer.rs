//! The timer's face (owner, 2026-11-28): a timer and a stopwatch in one puck, on the rotary face (`rotary`), the ring draining counterclockwise as the time runs out (the stopwatch's filling counterclockwise with the minute), the time large in the middle.
//!
//! - **The log dial**: the time is set by turning a finger about the circle's centre, T = 1 s · 60^θ with θ in turns, so one turn is a minute, two an hour, three 60 hours. The turn is relative: each step of the finger's angle multiplies the time by 60^Δθ from wherever it is (`turned`), clockwise up and counterclockwise down; nothing snaps, nothing is bounded.
//! - **The decades**: one thin ring inside the track per turn of the dial reached (seconds to a minute, to an hour, to 60 hours, ...), each filling clockwise over its turn along a C∞ step (`ring_fill`), so the person sees which scale a drag is on.
//! - **In focus**: the presets (1, 3, 5, 10, 15 and 30 minutes, each a number of minutes in the person's base) along the bottom inside the track, and the mode marks at the top (the timer's a dial with a quarter left, the stopwatch's a dial with its crown), the one shown lit.
//! - **The digits** are written by the puck in the person's base (ferros_puck::fmt `duration`) and drawn as the face's label: digits and colons, no words.

use crate::rotary::{self, Rotary};
use crate::{u, TEXT, TEXT2};
use core::f64::consts::TAU;
use ferros_puck::{fmt::unit, q14, turn_q16, Colour, Full, Writer};

/// The dial's factor a turn: one turn of the finger multiplies the time by 60.
pub const PER_TURN: f64 = 60.;

/// The time set at θ turns of the log dial: 60^θ seconds (θ = 1 a minute, 2 an hour, 3 sixty hours).
pub fn seconds(theta: f64) -> f64 {
    libm::exp(theta * libm::log(PER_TURN))
}

/// The dial's turns for a time of `secs` seconds (more than zero): log₆₀ of it, `seconds`'s inverse.
pub fn theta(secs: f64) -> f64 {
    libm::log(secs) / libm::log(PER_TURN)
}

/// A time of `secs` turned by `dtheta` turns of the finger: multiplied by 60^Δθ, clockwise (positive) up, counterclockwise down. Relative: the finger turns the time from where it is, never to where the finger is.
pub fn turned(secs: f64, dtheta: f64) -> f64 {
    secs * seconds(dtheta)
}

/// A point's angle about the circle's centre in turns clockwise from the face's up, in (−½, ½]: the face's frame has x to the right and y down, as ferros_puck's taps and drags come.
pub fn angle(x: i16, y: i16) -> f64 {
    libm::atan2(x as f64, -(y as f64)) / TAU
}

/// The finger's turn from angle `from` to angle `to` (each in turns): the smallest signed difference, in [−½, ½), so a step across the seam at the bottom (½ to −½) is the short way round, never nearly a whole turn back.
pub fn unwrap(from: f64, to: f64) -> f64 {
    let d = to - from;
    d - libm::floor(d + 0.5)
}

/// The fill a timer shows: the time left as a share of the time set, 1 full at the start and 0 empty at the end (left runs from set down to 0, so the share is in [0, 1] by its own form; `set` more than zero is the caller's).
pub fn drain(left: f64, set: f64) -> f32 {
    (left / set) as f32
}

/// How long the fill's end takes to move half a pixel on a circle `d` pixels across when its share moves `rate` a second (the timer's 1 / the time set, the stopwatch's 1 / 60): the share's slope on the track is `TRACK` turns (rotary `turns`, linear over the range), at the track's radius. Infinite for a circle of no pixels, on which nothing moves.
pub fn half_pixel(rate: f64, d: u16) -> f64 {
    let px = rate * rotary::TRACK as f64 * TAU * rotary::R_TRACK as f64 * d as f64 * 0.5;
    0.5 / px
}

/// The C∞ step from 0 to 1 over x in [0, 1], 0 at and below 0 and 1 at and above 1, every derivative 0 at both joins: b(x) / (b(x) + b(1 − x)) with the bump b(t) = e^(−1/t) for t > 0, else 0 (AGENT.md's reference shapes). No clamp: past either end it is its own constant.
pub fn ring_fill(x: f64) -> f64 {
    let b = |t: f64| if t > 0. { libm::exp(-1. / t) } else { 0. };
    let (a, c) = (b(x), b(1. - x));
    a / (a + c)
}

/// The decades' rings: the first's radius, the step inward between them, their width, and the least radius one is drawn at (the presets' numbers sit just inside it in focus), as fractions of the radius.
/// WHY the least radius: six rings fit between the needle's inner end (0.7) and the presets; the sixth is 60^6 s, about 1500 years, so a dial turned past it draws no more rings and loses nothing a timer reads.
const RING0: f32 = 0.68;
const RING_STEP: f32 = 1. / (1 << 6) as f32;
const RING_W: f32 = 1. / (1 << 7) as f32;
const RING_LOW: f32 = 0.6;

/// The presets in minutes, along the bottom inside the track in focus.
pub const PRESETS: [u32; 6] = [1, 3, 5, 10, 15, 30];
/// Their radius, the turn between two, their numbers' height and how near a tap must land to take one, as fractions of the radius and of a turn.
const PRESET_R: f32 = 0.56;
const PRESET_STEP: f32 = 1. / 20.;
const PRESET_SIZE: f32 = 1. / (1 << 4) as f32 + 1. / (1 << 6) as f32;
// The hit's radius under half the presets' spacing (2 · 0.56 · sin(π / 20) = 0.175), so no point is near two.
const PRESET_HIT: f32 = 0.085;
/// The mode marks: their centres' radius and turn either side of the top, their size and how near a tap must land.
const MODE_R: f32 = 0.52;
const MODE_TURN: f32 = 0.06;
const MODE_SIZE: f32 = 0.06;
const MODE_HIT: f32 = 0.1;
/// The middle a tap starts or pauses in: inside the digits' reach.
const CENTRE: f32 = 0.45;

/// The point at turn `a` (clockwise from the face's up) and radius r, in the face's units.
fn at(a: f32, r: f32) -> (f32, f32) {
    let th = a * core::f32::consts::TAU;
    (r * libm::sinf(th), -r * libm::cosf(th))
}

/// Preset k's centre: evenly along the bottom, the shortest on the left.
fn preset_at(k: usize) -> (f32, f32) {
    at(0.5 + (k as f32 - (PRESETS.len() - 1) as f32 * 0.5) * -PRESET_STEP, PRESET_R)
}

/// The mode mark's centre: the timer's left of the top, the stopwatch's right.
fn mode_at(stopwatch: bool) -> (f32, f32) {
    at(if stopwatch { MODE_TURN } else { -MODE_TURN }, MODE_R)
}

/// What a tap at (x, y) (Q14 of the radius, the face's frame) is on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    /// The middle: start or pause.
    Centre,
    /// A preset, in minutes (in focus only).
    Preset(u32),
    /// A mode mark: the stopwatch (true) or the timer (in focus only).
    Mode(bool),
    /// Anywhere else on the circle: two such taps near each other clear.
    Rim,
}

/// The mark a tap lands on: the marks first in focus (they are drawn only then), then the middle, else the rim.
pub fn hit(x: i16, y: i16, focus: bool) -> Hit {
    let (x, y) = (x as f32 / (1 << 14) as f32, y as f32 / (1 << 14) as f32);
    let near = |(px, py): (f32, f32), r: f32| (x - px) * (x - px) + (y - py) * (y - py) < r * r;
    if focus {
        if let Some(k) = (0..PRESETS.len()).find(|&k| near(preset_at(k), PRESET_HIT)) {
            return Hit::Preset(PRESETS[k]);
        }
        for sw in [false, true] {
            if near(mode_at(sw), MODE_HIT) {
                return Hit::Mode(sw);
            }
        }
    }
    if x * x + y * y < CENTRE * CENTRE { Hit::Centre } else { Hit::Rim }
}

/// What the face shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Timer<'a> {
    /// The stopwatch is shown (else the timer).
    pub stopwatch: bool,
    /// The fill: the time left over the time set (`drain`), or the stopwatch's share of its current minute.
    pub share: f32,
    /// Running: the fill in the text's colour, else the quieter one.
    pub running: bool,
    /// The time in the middle, as the puck wrote it in the person's base.
    pub label: &'a str,
    /// The time set in turns of the dial (`theta`), drawn as the decades' rings; None draws none (the stopwatch, which no finger sets).
    pub theta: Option<f64>,
}

fn quiet(on: bool) -> Colour {
    if on { TEXT } else { TEXT2.alpha(1 << 7) }
}

/// The timer's mark: a dial with its last quarter left (as a timer's drains).
fn timer_mark(w: &mut Writer, on: bool) -> Result<(), Full> {
    let (x, y) = mode_at(false);
    let c = quiet(on);
    w.ring(q14(x), q14(y), u(MODE_SIZE), u(MODE_SIZE * 0.8), c)?;
    w.arc(q14(x), q14(y), u(MODE_SIZE * 0.4), u(MODE_SIZE * 0.8), turn_q16(0.75), 1 << 14, c)?;
    Ok(())
}

/// The stopwatch's mark: a dial with its crown on top and its hand up.
fn stopwatch_mark(w: &mut Writer, on: bool) -> Result<(), Full> {
    let (x, y) = mode_at(true);
    let c = quiet(on);
    w.capsule((q14(x), q14(y - MODE_SIZE * 1.25)), (q14(x), q14(y - MODE_SIZE * 1.6)), u(MODE_SIZE * 0.2), c)?;
    w.capsule((q14(x), q14(y)), (q14(x), q14(y - MODE_SIZE * 0.6)), u(MODE_SIZE * 0.12), c)?;
    w.ring(q14(x), q14(y), u(MODE_SIZE), u(MODE_SIZE * 0.8), c)?;
    Ok(())
}

/// The decades' rings for a dial at θ turns: ring k fills clockwise from the top as θ goes from k to k + 1 (`ring_fill`), its opacity with it, so it comes out of nothing; full once the dial has passed that turn.
fn rings(w: &mut Writer, theta: f64) -> Result<(), Full> {
    let mut k = 0;
    loop {
        let r = RING0 - k as f32 * RING_STEP;
        // A ring for each turn the dial has started (θ past k), while one fits (`RING_LOW`).
        if theta <= k as f64 || r < RING_LOW {
            return Ok(());
        }
        let f = ring_fill(theta - k as f64);
        w.arc(0, 0, u(r), u(RING_W), 0, (f * 65535.) as u16, TEXT2.alpha((f * 255.) as u8))?;
        k += 1;
    }
}

pub fn face(w: &mut Writer, t: &Timer, focus: bool) -> Result<(), Full> {
    if focus {
        for (k, &m) in PRESETS.iter().enumerate() {
            let (x, y) = preset_at(k);
            w.number(q14(x), q14(y), u(PRESET_SIZE), TEXT2, (m as i64) << 16, 0, unit::NONE)?;
        }
        timer_mark(w, !t.stopwatch)?;
        stopwatch_mark(w, t.stopwatch)?;
    }
    if let Some(th) = t.theta {
        rings(w, th)?;
    }
    let r = Rotary { value: Some(t.share), lo: 0., hi: 1., second: None, clockwise: false, unit: unit::NONE, places: 0, colour: if t.running { TEXT } else { TEXT2 }, notes: &[], label: Some(t.label) };
    rotary::face(w, &r, focus)
}
