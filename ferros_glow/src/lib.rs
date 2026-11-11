//! A kist circle's state as the colour of its glow (owner, 2026-11-11: "Ideas for glow indicating kist states, pending notifications, running or not, etc."; "I pick spectrum colours as they are colourblind agnostic as long as we always choose monochromatic primaries ... map any negative coefficients when computing glow colour to a multiply on the negative channels ... Spectral lines for states").
//! The glow itself is the display's own (draw.rs `glow_log`: its falloff, its reach, its peaks by the theme); a state changes only its colour, its brightness step and the multiply behind it (owner, on the device: "we just need to change the colour and add the multiply for the negative channel").
//! No I/O, no std.
//!
//! | state | line | λ nm | brightness |
//! |---|---|---|---|
//! | running (background) | H-beta | 486.1 | low |
//! | the Back-swipe target | H-beta | 486.1 | medium |
//! | notifications from people | sodium D | 589.0 | high (the count widens its reach) |
//! | notifications from the system | mercury | 546.1 | medium |
//! | needs you (a question, a permission, an approval) | H-gamma | 434.0 | high |
//! | working (install, update, fetch) | mercury | 546.1 | medium |
//! | stopped with a reason | H-alpha | 656.3 | low, and a multiply passing its line |
//! | not running | none | - | a neutral multiply only |
//!
//! Colour: a line is its cone response (vsf's Stockman & Sharpe 10° table, 1 nm, linear between samples) in VSF RGB (`LMS2VSF_RGB`). A spectral line lies outside the primaries' triangle, so one channel comes out negative: the positive channels are the glow's colour, each negative channel a multiply of what is behind (`MULT_SHARE` of it at the glow's full intensity).

#![no_std]

use vsf::colour::convert::{apply_matrix_3x3_f32, vsf_rgb_to_photopic_f32};
use vsf::colour::spectral::constants::{LMS2VSF_RGB, LMS_2000_10DEG_1NM};

/// The lines, nm in air: hydrogen's Balmer beta, gamma and alpha, sodium's D (the D1/D2 pair's centre), mercury's green.
pub const H_BETA: f32 = 486.1;
pub const SODIUM_D: f32 = 589.0;
pub const MERCURY: f32 = 546.1;
pub const H_GAMMA: f32 = 434.0;
pub const H_ALPHA: f32 = 656.3;

/// A negative channel's multiply at the glow's full intensity: that channel of what is behind darkened by this share (owner: "share 1/2 to start").
pub const MULT_SHARE: f32 = 1. / (1 << 1) as f32;
/// Not running: the neutral multiply at the glow's full intensity (1/8 darker than the plane there).
pub const IDLE_DEPTH: f32 = 1. / (1 << 3) as f32;
/// Stopped with a reason: the multiply that passes H-alpha, at full intensity, on the channels the line does not light.
pub const STOP_DEPTH: f32 = 1. / (1 << 2) as f32;

/// A glow's brightness step, mapped by the display onto the theme's glow peaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Low,
    Medium,
    High,
}

/// A circle's state (the map sends it, display op 57).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Glow {
    None = 0,
    Running = 1,
    Back = 2,
    People = 3,
    System = 4,
    Stopped = 5,
    Idle = 6,
    NeedsYou = 7,
    Working = 8,
}

impl Glow {
    /// From the wire; an unknown byte is no state.
    pub fn from_u8(b: u8) -> Glow {
        match b {
            1 => Glow::Running,
            2 => Glow::Back,
            3 => Glow::People,
            4 => Glow::System,
            5 => Glow::Stopped,
            6 => Glow::Idle,
            7 => Glow::NeedsYou,
            8 => Glow::Working,
            _ => Glow::None,
        }
    }
}

/// A wavelength (nm, inside the table's 390 to 830) as linear VSF RGB: its cone response, linear between the table's 1 nm samples, thru `LMS2VSF_RGB`. One channel is negative for any line between the primaries (703, 523, 462 nm).
pub fn line(nm: f32) -> [f32; 3] {
    let s = &LMS_2000_10DEG_1NM;
    let f = nm - s.start_nm;
    let (i, w) = (f as usize, f - (f as usize) as f32);
    let lms = [0, 1, 2].map(|c| s.data[3 * i + c] * (1. - w) + s.data[3 * i + 3 + c] * w);
    apply_matrix_3x3_f32(&LMS2VSF_RGB, &lms)
}

/// What a state's glow is: its colour (linear VSF RGB, the line's positive channels, each 0 or more; none for a multiply alone), its brightness step, and its multiply (per channel, the share of what is behind darkened at the glow's full intensity).
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub colour: Option<[f32; 3]>,
    pub level: Level,
    pub mult: [f32; 3],
}

/// State `g`'s glow, or None for no state (the theme's glow as ever).
pub fn spec(g: Glow) -> Option<Spec> {
    let (nm, level) = match g {
        Glow::None => return None,
        Glow::Idle => return Some(Spec { colour: None, level: Level::Low, mult: [IDLE_DEPTH; 3] }),
        Glow::Running => (H_BETA, Level::Low),
        Glow::Back => (H_BETA, Level::Medium),
        Glow::People => (SODIUM_D, Level::High),
        Glow::System | Glow::Working => (MERCURY, Level::Medium),
        Glow::NeedsYou => (H_GAMMA, Level::High),
        Glow::Stopped => (H_ALPHA, Level::Low),
    };
    let c = line(nm);
    let p = c.map(|v| if v > 0. { v } else { 0. });
    let mult = if g == Glow::Stopped {
        // A filter passing the line: each channel darkened as far as the line leaves it unlit (its share of the brightest channel).
        let top = p[0].max(p[1]).max(p[2]);
        p.map(|v| STOP_DEPTH * (1. - v / top))
    } else {
        c.map(|v| if v < 0. { MULT_SHARE } else { 0. })
    };
    Some(Spec { colour: Some(p), level, mult })
}

/// A multiply's photopic share: the light it takes from a grey (equal-energy) plane, by VSF's own weights (R 0.2584, G 0.6346, B 0.1070). The display's canvas has one opacity a pixel, so a per-channel multiply goes in as a neutral one of this strength (display main.rs `state_glow`).
pub fn photopic(m: [f32; 3]) -> f32 {
    vsf_rgb_to_photopic_f32(m[0], m[1], m[2])
}

/// Linear VSF RGB `l` as light of the same photopic luminance as `y` (relative to white, 1), or as bright as the primaries allow when a channel would pass white (its brightest channel then full, as the console's battery flavour does).
pub fn at_luminance(l: [f32; 3], y: f32) -> [f32; 3] {
    let k = y / vsf_rgb_to_photopic_f32(l[0], l[1], l[2]);
    let top = l[0].max(l[1]).max(l[2]) * k;
    // WHY the cap: a line of low photopic weight (H-gamma, deep blue) cannot reach the theme's light within white; its brightest channel stops at full.
    let k = if top > 1. { k / top } else { k };
    l.map(|v| v * k)
}

/// How much farther a people glow reaches with n notifications: 1 + L / (1 + L), L = log2(1 + n) (one: 1.5, twenty: 1.81), never past twice.
pub fn people_reach(n: u16) -> f32 {
    let l = libm::log2f(1. + n as f32);
    1. + l / (1. + l)
}
