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
//! | system (the settings circle, built in: never a kist) | 703 | the theme's thin peak, the VSF red primary itself, so no multiply |
//!
//! What the kist states mean, as the map sends them (payloads/map `send_states`, docs/KISTS.md "Kist states"): running, a live process of that kist (in front or behind); stopped, its last process ended without the person ending it (a fault, a panic, a kill from outside) or its last launch did not start, and it has not started since; not running, installed and neither; for you, something waiting on the person for that kist (nothing sets it yet: there are no notifications).
//!
//! Colour: a wavelength is its cone response (vsf's Stockman & Sharpe 10° table, 1 nm, linear between samples) in VSF RGB (`LMS2VSF_RGB`). A single wavelength lies outside the primaries' triangle, so a channel comes out negative: the positive channels are the glow's colour, each negative channel a multiply of what is behind (`MULT_SHARE` of it at the glow's full intensity). A primary's own wavelength (703, 523, 462 nm) is inside the triangle, at its corner: its other two channels are zero but for the f32 matrix's rounding (`ROUNDING`), so it has no multiply.

#![no_std]

use vsf::colour::convert::{apply_matrix_3x3_f32, vsf_rgb_to_photopic_f32};
use vsf::colour::spectral::constants::{LMS2VSF_RGB, LMS_2000_10DEG_1NM};

/// The states' wavelengths, nm (the owner's).
pub const RUNNING_NM: f32 = 500.;
pub const STOPPED_NM: f32 = 600.;
pub const FOR_YOU_NM: f32 = 400.;
/// The settings circle's own (owner, 2026-11-11, on the Pro: "For the ferros settings specifically, I think 703nm (I think that's VSF red?)"): VSF RGB's red primary (vsf colour::spectral, "Red (R): 703nm").
pub const SYSTEM_NM: f32 = 703.;

/// A channel nearer zero than this share of the line's brightest channel is zero: the cone table and `LMS2VSF_RGB` are f32, and a primary's own wavelength leaves its other channels at about 1/2^29 of its own (703 nm: green -1.2e-9 of red, blue 2.2e-12; 462 nm: red -8.2e-9 of blue), where the multiply would otherwise take a whole `MULT_SHARE` for nothing. The nearest real negative of the lines in use is 600 nm's blue, 1/2^8 of its red, far above.
pub const ROUNDING: f32 = 1. / (1 << 16) as f32;

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
    /// The settings circle (the map's circle 0): built in, the system's own, never a kist's state.
    System = 5,
}

impl Glow {
    /// From the wire; an unknown byte is no state.
    pub fn from_u8(b: u8) -> Glow {
        match b {
            1 => Glow::NotRunning,
            2 => Glow::Running,
            3 => Glow::Stopped,
            4 => Glow::ForYou,
            5 => Glow::System,
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
        // Thin, as running: a mark of what the circle is, not a call for the person's eye.
        Glow::System => (SYSTEM_NM, Level::Thin),
    };
    let c = line(nm);
    // WHY the floor: `ROUNDING`, a primary's other channels are the matrix's rounding, not light or a multiply.
    let floor = c[0].max(c[1]).max(c[2]) * ROUNDING;
    Some(Spec { colour: c.map(|v| if v > floor { v } else { 0. }), level, mult: c.map(|v| if v < -floor { MULT_SHARE } else { 0. }) })
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

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// The display's colour for state `g` on a theme whose ring is `ring` (G#RRGGBB), as payloads/display `state_glow` makes it: 8-bit gamma 2 codes.
    fn code(g: Glow, ring: u32) -> u32 {
        let s = spec(g).unwrap();
        let lin = |v: u32| {
            let c = (v & 0xFF) as f32 * (1. / 255.);
            c * c
        };
        let y = photopic([lin(ring >> 16), lin(ring >> 8), lin(ring)]);
        let l = at_luminance(s.colour, y).map(|v| v * light(g));
        let c = |v: f32| (libm::sqrtf(v) * 255. + 0.5) as u32;
        c(l[0]) << 16 | c(l[1]) << 8 | c(l[2])
    }

    /// The settings circle's 703 nm is the red primary: red alone, no multiply, and red full on the grey theme (its photopic weight, 0.2584, is under the ring's 0.293, so the brightest channel stops at full).
    #[test]
    fn system_is_the_red_primary() {
        let l = line(SYSTEM_NM);
        let s = spec(Glow::System).unwrap();
        std::println!("703 nm line {l:?}; colour {:?}, multiply {:?} (photopic {}), grey theme G#{:06X}", s.colour, s.mult, photopic(s.mult), code(Glow::System, 0x8A8A8A));
        assert!(l[1].abs() < l[0] * ROUNDING && l[2].abs() < l[0] * ROUNDING);
        assert_eq!(s.mult, [0.; 3]);
        assert_eq!(s.level, Level::Thin);
        assert!(s.colour[0] > 0. && s.colour[1] == 0. && s.colour[2] == 0.);
        assert_eq!(code(Glow::System, 0x8A8A8A), 0xFF0000);
        assert_eq!(Glow::from_u8(5), Glow::System);
    }

    /// `ROUNDING` leaves the owner's three lines as they were: each keeps its one negative channel's multiply.
    #[test]
    fn lines_keep_their_multiply() {
        let h = MULT_SHARE;
        for (g, m) in [(Glow::Running, [h, 0., 0.]), (Glow::Stopped, [0., 0., h]), (Glow::ForYou, [0., h, 0.])] {
            let s = spec(g).unwrap();
            std::println!("{g:?}: colour {:?}, multiply {:?} (photopic {}), grey theme G#{:06X}", s.colour, s.mult, photopic(s.mult), code(g, 0x8A8A8A));
            assert_eq!(s.mult, m);
        }
        assert_eq!(code(Glow::Running, 0x8A8A8A), 0x00AA4C);
        assert_eq!(code(Glow::Stopped, 0x8A8A8A), 0xDA6700);
    }
}
