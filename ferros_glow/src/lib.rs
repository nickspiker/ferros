//! A kist circle's state as light (owner, 2026-11-11: "Ideas for glow indicating kist states, pending notifications, running or not, etc."; then "I pick spectrum colours as they are colourblind agnostic as long as we always choose monochromatic primaries ... i'd like to cover it all with wavelengths and brightnesses ... Spectral lines for states").
//! One maths for the device (payloads/display draws it on the home plane) and the desktop (tools/tune's glow page), no I/O, no std.
//!
//! Three channels a circle can show, each at once:
//! - **glow**: light outside the rim, a layer of its line's colour at a falling α, composed under the circles (`Glow`, `at_rim`, `falloff`, `reach`);
//! - **rim**: a lit arc on the circle's own edge, in front of its icon (`Rim`, `rim_light`, `arc_cover`);
//! - the circle itself is left as it is.
//!
//! | state | line | λ nm | at the rim |
//! |---|---|---|---|
//! | running (background) | H-beta | 486.1 | glow, low, short reach |
//! | the Back-swipe target | H-beta | 486.1 | glow, medium |
//! | notifications from people | sodium D | 589.0 | glow, high, reach growing with log2(1 + n) |
//! | notifications from the system | mercury | 546.1 | glow, medium |
//! | needs you (a question, a permission, an approval) | H-gamma | 434.0 | the rim's twelve o'clock arc, high |
//! | working (install, update, fetch) | mercury | 546.1 | the rim's progress arc, medium |
//! | stopped with a reason | H-alpha | 656.3 | glow as a multiply, low |
//! | not running | none | - | a neutral multiply under the circle |
//!
//! Colour: a line is its cone response (vsf's Stockman & Sharpe 10° table, 1 nm, linear between samples) in VSF RGB (`LMS2VSF_RGB`), scaled to unit photopic luminance by VSF's own weights (`vsf_rgb_to_photopic_f32`).
//! A spectral line lies outside the primaries' triangle, so one channel comes out negative (owner: "map any negative coefficients when computing glow colour to a multiply on the negative channels").
//! The positive part is the light; the negative part becomes a filter that darkens what is behind, `MULT_SHARE` of its photopic share.
//! The display's canvas carries one α a pixel, so the filter is neutral there (it darkens every channel of the wallpaper behind, by the negative part's photopic share): a per-channel multiply needs three opacities a pixel, the owner's call.
//!
//! Every knob is one constant here, a power of two where it is a choice.

#![no_std]

use vsf::colour::convert::{apply_matrix_3x3_f32, vsf_rgb_to_photopic_f32};
use vsf::colour::spectral::constants::{LMS2VSF_RGB, LMS_2000_10DEG_1NM};

/// The lines, nm in air: hydrogen's Balmer beta, gamma and alpha, sodium's D (the D1/D2 pair's centre), mercury's green.
pub const H_BETA: f32 = 486.1;
pub const SODIUM_D: f32 = 589.0;
pub const MERCURY: f32 = 546.1;
pub const H_GAMMA: f32 = 434.0;
pub const H_ALPHA: f32 = 656.3;

/// Brightness at the rim, as photopic luminance relative to white (1): low, medium, high.
pub const LOW: f32 = 1. / (1 << 4) as f32;
pub const MEDIUM: f32 = 1. / (1 << 3) as f32;
pub const HIGH: f32 = 1. / (1 << 2) as f32;
/// How far a glow reaches past the rim, in the circle's radius: low, medium, high (so a glow reads at every zoom: it is part of the circle's shape).
pub const REACH_LOW: f32 = 1. / (1 << 1) as f32;
pub const REACH_MEDIUM: f32 = 3. / (1 << 2) as f32;
pub const REACH_HIGH: f32 = 1.;
/// A light glow's opacity at the rim at `HIGH`, a dimmer one's in proportion to its level (medium 1/4, low 1/8): how much of what is behind (the wallpaper) it takes away before its own light goes in, so the colour reads vivid on a bright plane, while a faint glow stays mostly light (owner: the multiply is for the kists not running, "so the glow is vivid anyway").
pub const VEIL: f32 = 1. / (1 << 1) as f32;
/// The negative part's filter, as a share of its photopic weight against the positive part's (owner: "share 1/2 to start").
pub const MULT_SHARE: f32 = 1. / (1 << 1) as f32;
/// Not running: the neutral multiply's depth at the rim (1/8 darker than the plane there).
pub const IDLE_DEPTH: f32 = 1. / (1 << 3) as f32;
/// Stopped with a reason: the multiply's depth at the rim, with its line's light at `LOW` in it (the H-alpha tint the darkening carries).
pub const STOP_DEPTH: f32 = 1. / (1 << 2) as f32;
/// The rim's arcs: their light at full coverage (relative luminance), high and medium; an arc lies over the circle's icon, so it is brighter than a glow on the plane.
pub const RIM_HIGH: f32 = 1. / (1 << 1) as f32;
pub const RIM_MEDIUM: f32 = 1. / (1 << 2) as f32;
/// The arc's band, as shares of the circle's radius: outer and inner edge (the quick row's rings, display quick.rs `RING_BAND`).
pub const RIM_BAND: (f32, f32) = (0.97, 0.84);
/// Needs you: the arc's length about twelve o'clock, a share of the turn.
pub const NEEDS_SHARE: f32 = 1. / (1 << 2) as f32;
/// Working: the progress arc's unlit rest, as a share of the lit part's coverage (a faint track, so the share left reads).
pub const RIM_REST: f32 = 1. / (1 << 3) as f32;

/// A circle's glow (the map sends it, display op 57).
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
}

impl Glow {
    /// Every glow that draws, in its wire order.
    pub const ALL: [Glow; 6] = [Glow::Running, Glow::Back, Glow::People, Glow::System, Glow::Stopped, Glow::Idle];
    /// From the wire; an unknown byte is no glow.
    pub fn from_u8(b: u8) -> Glow {
        match b {
            1 => Glow::Running,
            2 => Glow::Back,
            3 => Glow::People,
            4 => Glow::System,
            5 => Glow::Stopped,
            6 => Glow::Idle,
            _ => Glow::None,
        }
    }
    /// Its line (nm), none for the neutral multiply.
    pub fn line(self) -> Option<f32> {
        match self {
            Glow::Running | Glow::Back => Some(H_BETA),
            Glow::People => Some(SODIUM_D),
            Glow::System => Some(MERCURY),
            Glow::Stopped => Some(H_ALPHA),
            Glow::Idle | Glow::None => None,
        }
    }
    /// Its brightness at the rim.
    pub fn level(self) -> f32 {
        match self {
            Glow::Running | Glow::Stopped => LOW,
            Glow::Back | Glow::System => MEDIUM,
            Glow::People => HIGH,
            Glow::Idle | Glow::None => 0.,
        }
    }
}

/// A circle's rim arc (display op 57).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Rim {
    None = 0,
    NeedsYou = 1,
    Working = 2,
}

impl Rim {
    pub fn from_u8(b: u8) -> Rim {
        match b {
            1 => Rim::NeedsYou,
            2 => Rim::Working,
            _ => Rim::None,
        }
    }
}

/// A wavelength (nm, inside the table's 390 to 830) as linear VSF RGB at unit photopic luminance: its cone response, linear between the table's 1 nm samples, thru `LMS2VSF_RGB`, divided by its luminance. One channel is negative for any line between the primaries (703, 523, 462 nm), as a spectral colour lies outside their triangle.
pub fn line(nm: f32) -> [f32; 3] {
    let s = &LMS_2000_10DEG_1NM;
    let f = nm - s.start_nm;
    let (i, w) = (f as usize, f - (f as usize) as f32);
    let lms = [0, 1, 2].map(|c| s.data[3 * i + c] * (1. - w) + s.data[3 * i + 3 + c] * w);
    let rgb = apply_matrix_3x3_f32(&LMS2VSF_RGB, &lms);
    let y = vsf_rgb_to_photopic_f32(rgb[0], rgb[1], rgb[2]);
    rgb.map(|v| v / y)
}

/// A line's light and filter: the positive part scaled to `level` of white's photopic luminance, and the negative part's photopic weight against the positive's (the filter's share before `MULT_SHARE`).
pub fn split(nm: f32, level: f32) -> ([f32; 3], f32) {
    let c = line(nm);
    let p = c.map(|v| if v > 0. { v } else { 0. });
    let n = c.map(|v| if v < 0. { -v } else { 0. });
    let yp = vsf_rgb_to_photopic_f32(p[0], p[1], p[2]);
    let yn = vsf_rgb_to_photopic_f32(n[0], n[1], n[2]);
    (p.map(|v| v * level / yp), yn / yp)
}

/// A glow at its rim: linear VSF RGB light (each channel ≥ 0) and its opacity (0 to 1). Both fall by `falloff` together out to the reach.
#[derive(Clone, Copy, Debug)]
pub struct AtRim {
    pub light: [f32; 3],
    pub alpha: f32,
}

/// Glow `g` at its rim: a light (running, back, people, system) is its line's positive part at its level, at α = `VEIL` scaled by its level against `HIGH`, plus the negative part's filter; stopped is a multiply of `STOP_DEPTH` carrying its line's light at `LOW`; not running is the neutral multiply, no light at all.
pub fn at_rim(g: Glow) -> AtRim {
    match (g, g.line()) {
        (Glow::Stopped, Some(nm)) => AtRim { light: split(nm, g.level()).0, alpha: STOP_DEPTH },
        (_, Some(nm)) => {
            let (light, neg) = split(nm, g.level());
            AtRim { light, alpha: VEIL * g.level() / HIGH + MULT_SHARE * neg }
        }
        (Glow::Idle, None) => AtRim { light: [0.; 3], alpha: IDLE_DEPTH },
        _ => AtRim { light: [0.; 3], alpha: 0. },
    }
}

/// The falloff from the rim (s = 0) to the reach (s = 1): the bump exp(1 − 1/(1 − s²)), 1 at the rim with every odd derivative zero there, and zero at the reach with every derivative zero, so the glow meets the plane with no edge at all (C∞).
/// `s` is the squared distance past the rim as a share of the reach's, q / q_max with q = d² − r² (the display walks q, no root), itself C∞ in the pixel's place. The test is the bump's definition past its end, not a clamp.
pub fn falloff(s: f32) -> f32 {
    if s >= 1. {
        return 0.;
    }
    libm::expf(1. - 1. / (1. - s * s))
}

/// How far glow `g` reaches past the rim, in the circle's radius; people's grows with the count n, from `REACH_LOW` toward `REACH_HIGH` as L / (1 + L) with L = log2(1 + n) (one: 3/4, twenty: 0.91), C∞ in n and never past high.
pub fn reach(g: Glow, n: u16) -> f32 {
    match g {
        Glow::Running | Glow::Stopped | Glow::Idle => REACH_LOW,
        Glow::Back | Glow::System => REACH_MEDIUM,
        Glow::People => {
            let l = libm::log2f(1. + n as f32);
            REACH_LOW + (REACH_HIGH - REACH_LOW) * l / (1. + l)
        }
        Glow::None => 0.,
    }
}

/// Rim arc `r`'s light at full coverage (linear VSF RGB, its positive part: an arc is opaque where lit, so the negative part has nothing behind it to filter).
pub fn rim_light(r: Rim) -> [f32; 3] {
    match r {
        Rim::NeedsYou => split(H_GAMMA, RIM_HIGH).0,
        Rim::Working => split(MERCURY, RIM_MEDIUM).0,
        Rim::None => [0.; 3],
    }
}

/// Rim arc `r`'s lit part, in turns clockwise from twelve o'clock: its centre and half its length. Needs you: `NEEDS_SHARE` about twelve. Working: from twelve round to `share` (0 to 1), as the quick row's rings fill.
pub fn arc(r: Rim, share: f32) -> (f32, f32) {
    match r {
        Rim::NeedsYou => (0., NEEDS_SHARE * 0.5),
        Rim::Working => (share * 0.5, share * 0.5),
        Rim::None => (0., 0.),
    }
}

/// A C∞ step from 0 at t ≤ 0 to 1 at t ≥ 1 (display draw.rs `bump_step`): the bump exp(−1/t) against its mirror; the two tests are its definition.
pub fn bump_step(t: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let (a, b) = (libm::expf(-1. / t), libm::expf(-1. / (1. - t)));
    a / (a + b)
}

/// A pixel's coverage by rim arc `r` on a circle of radius `ri` pixels: (dx, dy) the pixel's centre from the circle's, `share` the working arc's progress.
/// The band's two edges and the lit part's two ends are each the C∞ step across a pixel (the quick row's `ring`), so the lit end moves smoothly with the share; past the lit part a working arc keeps `RIM_REST` of its light (the track still to go), a needs-you arc nothing.
pub fn arc_cover(r: Rim, dx: f32, dy: f32, ri: f32, share: f32) -> f32 {
    let (r1, r0) = (ri * RIM_BAND.0, ri * RIM_BAND.1);
    let d = libm::sqrtf(dx * dx + dy * dy);
    let band = bump_step(r1 - d + 0.5) * bump_step(d - r0 + 0.5);
    if band <= 0. {
        return 0.;
    }
    let tau = core::f32::consts::TAU;
    let (centre, half) = arc(r, share);
    // The turn from the arc's centre, -1/2 to 1/2: the seam opposite the centre is where the lit part never is (short of a full share).
    let a = libm::atan2f(dx, -dy) * (1. / tau) - centre;
    let g = a - libm::floorf(a + 0.5);
    let lit = bump_step((g + half) * tau * d + 0.5) * bump_step((half - g) * tau * d + 0.5);
    let rest = if r == Rim::Working { RIM_REST } else { 0. };
    band * (lit + (1. - lit) * rest)
}
