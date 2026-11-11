//! A kist circle's state as the colour of its glow (owner, 2026-11-11: "I pick spectrum colours as they are colourblind agnostic as long as we always choose monochromatic primaries ... map any negative coefficients when computing glow colour to a multiply on the negative channels"; then "for the four states, nix the rest of the circles and we'll do black-none, running, 500nm, stopped, 600 and for you 400").
//! The glow itself is the display's own (draw.rs `glow_log`: its falloff, its reach, its peaks by the theme); a state changes only its colour, its peak and the multiply behind it (owner, on the device: "we just need to change the colour and add the multiply for the negative channel").
//! No I/O, no std.
//!
//! | state | λ nm | glow |
//! |---|---|---|
//! | not running | - | no light: a ring of darkness, the neutral multiply at `NOT_RUNNING_DEPTH`, reaching `waiting_reach(1)` |
//! | running | 500 | the theme's thin peak |
//! | stopped | 600 | the theme's thin peak |
//! | for you (anything waiting on the person: a notification from anyone, a question, an approval) | 400 | the theme's heavy peak at `FOR_YOU_LIGHT` of its light, reaching `FOR_YOU_REACH` farther and farther still as the count grows |
//!
//! Colour: a wavelength is its cone response (vsf's Stockman & Sharpe 10° table, 1 nm, linear between samples) in VSF RGB (`LMS2VSF_RGB`). A single wavelength lies outside the primaries' triangle, so a channel comes out negative: the positive channels are the glow's colour, each negative channel a multiply of what is behind (`MULT_SHARE` of it at the glow's full intensity).

#![no_std]

use vsf::colour::convert::{apply_matrix_3x3_f32, vsf_rgb_to_photopic_f32};
use vsf::colour::spectral::constants::{LMS2VSF_RGB, LMS_2000_10DEG_1NM};

/// The states' wavelengths, nm (the owner's).
pub const RUNNING_NM: f32 = 500.;
pub const STOPPED_NM: f32 = 600.;
pub const FOR_YOU_NM: f32 = 400.;

/// A negative channel's multiply at the glow's full intensity: that channel of what is behind darkened by this share (owner: "share 1/2 to start").
pub const MULT_SHARE: f32 = 1. / (1 << 1) as f32;
/// Not running: the darkening of what is behind at the rim, every channel alike (owner, 2026-11-11, on the Pro: "Glow states are perfect except for the black, needs more cowbell"). The one knob; it falls with the glow's profile to nothing at its reach.
pub const NOT_RUNNING_DEPTH: f32 = 1. / (1 << 1) as f32;
/// For you: its reach against the waiting count's, and its light against the theme's ring (owner, 2026-11-11, on the Pro: "1.5x on the for you radius, brightness can go down 0.75x").
pub const FOR_YOU_REACH: f32 = 1. + 1. / (1 << 1) as f32;
pub const FOR_YOU_LIGHT: f32 = 1. - 1. / (1 << 2) as f32;

/// A circle's state (the map sends it, display op 57).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Glow {
    /// No state: the theme's glow as ever.
    None = 0,
    NotRunning = 1,
    Running = 2,
    Stopped = 3,
    ForYou = 4,
}

impl Glow {
    /// From the wire; an unknown byte is no state.
    pub fn from_u8(b: u8) -> Glow {
        match b {
            1 => Glow::NotRunning,
            2 => Glow::Running,
            3 => Glow::Stopped,
            4 => Glow::ForYou,
            _ => Glow::None,
        }
    }
}

/// A glow's peak, mapped by the display onto the theme's glow peaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    /// No light (not running: its multiply alone).
    Off,
    Thin,
    Heavy,
}

/// A wavelength (nm, inside the table's 390 to 830) as linear VSF RGB: its cone response, linear between the table's 1 nm samples, thru `LMS2VSF_RGB`. A channel is negative for any wavelength but the primaries' own (703, 523, 462 nm).
pub fn line(nm: f32) -> [f32; 3] {
    let s = &LMS_2000_10DEG_1NM;
    let f = nm - s.start_nm;
    let (i, w) = (f as usize, f - (f as usize) as f32);
    let lms = [0, 1, 2].map(|c| s.data[3 * i + c] * (1. - w) + s.data[3 * i + 3 + c] * w);
    apply_matrix_3x3_f32(&LMS2VSF_RGB, &lms)
}

/// What a state's glow is: its colour (linear VSF RGB, the wavelength's positive channels), its peak, and its multiply (per channel, the share of what is behind darkened at the glow's full intensity).
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub colour: [f32; 3],
    pub level: Level,
    pub mult: [f32; 3],
}

/// State `g`'s glow, or None for no state (the theme's glow as ever). Not running is no light, its neutral multiply alone (owner, 2026-11-11, on the Pro: "Glow states are perfect except for the black, needs more cowbell").
pub fn spec(g: Glow) -> Option<Spec> {
    let (nm, level) = match g {
        Glow::None => return None,
        Glow::NotRunning => return Some(Spec { colour: [0.; 3], level: Level::Off, mult: [NOT_RUNNING_DEPTH; 3] }),
        Glow::Running => (RUNNING_NM, Level::Thin),
        Glow::Stopped => (STOPPED_NM, Level::Thin),
        Glow::ForYou => (FOR_YOU_NM, Level::Heavy),
    };
    let c = line(nm);
    Some(Spec { colour: c.map(|v| if v > 0. { v } else { 0. }), level, mult: c.map(|v| if v < 0. { MULT_SHARE } else { 0. }) })
}

/// A multiply's photopic share: the light it takes from a grey (equal-energy) plane, by VSF's own weights (R 0.2584, G 0.6346, B 0.1070). The display's canvas has one opacity a pixel, so a per-channel multiply goes in as a neutral one of this strength (display main.rs `state_glow`).
pub fn photopic(m: [f32; 3]) -> f32 {
    vsf_rgb_to_photopic_f32(m[0], m[1], m[2])
}

/// Linear VSF RGB `l` as light of the same photopic luminance as `y` (relative to white, 1), or as bright as the primaries allow when a channel would pass white (its brightest channel then full, as the console's battery flavour does).
pub fn at_luminance(l: [f32; 3], y: f32) -> [f32; 3] {
    let k = y / vsf_rgb_to_photopic_f32(l[0], l[1], l[2]);
    let top = l[0].max(l[1]).max(l[2]) * k;
    // WHY the cap: a wavelength of low photopic weight (400 nm, violet) cannot reach the theme's light within white; its brightest channel stops at full.
    let k = if top > 1. { k / top } else { k };
    l.map(|v| v * k)
}

/// How much farther a for-you glow reaches with n things waiting: 1 + L / (1 + L), L = log2(1 + n) (one: 1.5, twenty: 1.81), never past twice.
pub fn waiting_reach(n: u16) -> f32 {
    let l = libm::log2f(1. + n as f32);
    1. + l / (1. + l)
}

/// How much farther state `g`'s glow reaches than the theme's, with `n` things waiting: for you by its count (times `FOR_YOU_REACH`), not running `waiting_reach(1)`, as far as for you with one reached before `FOR_YOU_REACH` (so its darkness reads as a ring), the rest not at all.
pub fn reach(g: Glow, n: u16) -> f32 {
    match g {
        Glow::ForYou => waiting_reach(n) * FOR_YOU_REACH,
        Glow::NotRunning => waiting_reach(1),
        _ => 1.,
    }
}

/// State `g`'s light against the theme's ring: for you `FOR_YOU_LIGHT`, the rest as the ring.
pub fn light(g: Glow) -> f32 {
    if g == Glow::ForYou { FOR_YOU_LIGHT } else { 1. }
}
