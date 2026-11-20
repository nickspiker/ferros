//! The device's themes (preference 5, an index into `THEMES`): the one table the map, the display, the pucks and every kist's fluor look (ferros_fluor) draw from, its colours G#RRGGBB VSF RGB codes at gamma 2 as every display op takes them.
//! Until 2026-11-20 inside ferros_sys::theme, which re-exports it with `current` and `index` (the preference read): here so it builds for any target, and a kist's host tests read the very table the device draws in.
#![no_std]

#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    /// A circle without an icon, and the clock's face.
    pub plane: u32,
    /// The glow of an ordinary circle.
    pub ring: u32,
    /// What changed since last seen: a lit circle's fill and glow (the map's only colour).
    pub lit: u32,
    /// A radio's and a checkbox's outline and fill: white in the default theme (owner, 2026-11-08: "white for the checkbox/radial too, no amber, unless of course the theme wants something else but that's for the non default"); the others their own text colour (night stays red).
    pub mark: u32,
    /// Text: primary, secondary, faint.
    pub text: u32,
    pub text2: u32,
    pub text3: u32,
    /// The clock: a ring's swept fill, the hour ring's fill after noon (decimal's AM/PM), the strokes between rings, a tick still ahead of the fill, a tick the fill has passed.
    pub clock_fill: u32,
    pub clock_fill_pm: u32,
    pub clock_stroke: u32,
    pub clock_tick: u32,
    pub clock_tick_passed: u32,
    /// The glow's brightness one pixel out, by importance (none, ordinary, heavy).
    pub glow_peak: [u32; 3],
    /// A button's colours, G#RRGGBB VSF RGB codes at gamma 2 as every display op's: its fill, its fill while held (or armed, the first tap of two), and its edge on the top-left and the bottom-right of the anti-diagonal, light then shadow, lit from the top left as the keys are (owner, 2026-11-09).
    /// On grey they are fluor's BUTTON_FILL, BUTTON_HELD, TEXTBOX_LIGHT_EDGE and TEXTBOX_SHADOW_EDGE (the same codes at gamma 2); the other themes' in their own hues.
    /// Until 2026-11-20 the map's BUTTON_LOOK; here so fluor's ferros host draws every kist's controls in the device's theme from this one table.
    pub button: [u32; 4],
}

pub const THEMES: [Theme; 4] = [
    Theme { name: "grey", plane: 0x2A2A2A, ring: 0x8A8A8A, lit: 0xFFA000, mark: 0xFFFFFF, text: 0xF0F0F0, text2: 0x9A9A9A, text3: 0x5C5C5C, clock_fill: 0x707070, clock_fill_pm: 0x9C9C9C, clock_stroke: 0, clock_tick: 0xFFFFFF, clock_tick_passed: 0, glow_peak: [0, 0x77, 0xFF], button: [0x1A214C, 0x3C4CA8, 0x423F36, 0x2A3336] },
    Theme { name: "slate", plane: 0x222830, ring: 0x7A8CA0, lit: 0x3CC8FF, mark: 0xE8F0F8, text: 0xE8F0F8, text2: 0x8EA0B4, text3: 0x506070, clock_fill: 0x5A6C80, clock_fill_pm: 0x8AA0B8, clock_stroke: 0, clock_tick: 0xE8F0FF, clock_tick_passed: 0, glow_peak: [0, 0x77, 0xFF], button: [0x162238, 0x2E4C78, 0x48525E, 0x1A2028] },
    Theme { name: "ember", plane: 0x2C241E, ring: 0xA08060, lit: 0xFF7A30, mark: 0xF4E8DC, text: 0xF4E8DC, text2: 0xB09880, text3: 0x6A5848, clock_fill: 0x7C6450, clock_fill_pm: 0xA88C70, clock_stroke: 0, clock_tick: 0xFFF0E0, clock_tick_passed: 0, glow_peak: [0, 0x77, 0xFF], button: [0x3A2414, 0x6E4424, 0x5A4A3A, 0x241A12] },
    Theme { name: "night", plane: 0x1E0806, ring: 0x802018, lit: 0xFF2810, mark: 0xD04030, text: 0xD04030, text2: 0x902818, text3: 0x501008, clock_fill: 0x601810, clock_fill_pm: 0x902418, clock_stroke: 0, clock_tick: 0xC03020, clock_tick_passed: 0, glow_peak: [0, 0x50, 0xB0], button: [0x380A06, 0x6A160C, 0x4A120C, 0x180404] },
];
