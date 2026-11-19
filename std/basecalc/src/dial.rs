//! basecalc's dial (owner, 2026-11-18: "Think single layer, Warrington or combined wire rope patterns", then "forgot about the middle being the actual readout... I like the staggered two row. Let's go for an optimized warrington so it should be fairly even on the two rows").
//!
//! - **The strand.** The readout is the core; around it an inner row of n keys and an outer row of 2n staggered by half a key: every other outer key sits over an inner key, the ones between over its seams (a Warrington strand's large and small outer wires). n is the base from 10 to 16: the dozenal dial is 1 + 12 + 24, 36 keys.
//! - **Optimized to even.** The keys are annular cells, drawn and hit as their regions, so evenness is area: the rows' boundary lies where the inner row holds a third of the ring (m² = c² + (1 − c²)/3), so all 3n keys have the same area; the readout's radius (1/4 + n/64 of the puck's) keeps the inner keys about as deep as they are wide.
//! - **The keys.** The outer row's keys over the inner ones are the base's digits, 0 at the top and clockwise, so the dial shows its base; the keys between them the arithmetic. The inner row holds Shift (top), AC, = (bottom) and del, its other keys paging thru the rest of basecalc (functions, inverses and parts, constants, the complex keys, the settings) at each Shift.
//! - **The readout.** A tap there copies the value (owner: "tapping in the middle should copy to clipboard"; the clipboard is a capability handed on a gesture and only partly built, so it is said for now).
//! - **The words** are ferros's face (ferros_glyphs, Oxanium with tatau's dozenal digits at G#10-G#1B), so a dozenal digit is its own glyph (owner: "use the lower control set of ascii, not 0-B").
//! - **The base** follows the device's until the settings keys override it; following comes back by its key.

use basecalc_kist::keys::{self, Key, Unit};
use ferros_glyphs::Glyphs;
use fluor::pixel::{Blend, BlendMode, G2t16, Pixel};
use fluor::theme::{dark, fmt, vsf};

/// An authored VSF RGB g2t colour, opaque, as fluor stores it (α and darkness, 8 bits).
const fn colour(t: u32) -> u32 {
    dark(fmt(vsf(t)))
}
const GROUND: u32 = colour(0x00_06_08_0C);
const CORE: u32 = colour(0x00_0C_10_16);
const DIGIT: u32 = colour(0x00_22_2A_36);
const OPERATOR: u32 = colour(0x00_1A_36_44);
const FUNCTION: u32 = colour(0x00_2C_24_3C);
const CONTROL: u32 = colour(0x00_16_1A_22);
const EQUALS: u32 = colour(0x00_8C_54_10);
const SHIFTED: u32 = colour(0x00_2A_62_7C);
const INK: u32 = colour(0x00_C2_C2_C2);
const INK_DIM: u32 = colour(0x00_6C_70_78);
const INK_ERROR: u32 = colour(0x00_D8_40_38);

/// What a key of the dial does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    /// A key of the calculator itself.
    Calc(Key),
    /// The inner row's next layer.
    Shift,
    /// The next of the quick bases (2, 8, 10, 12, 16, 36), overriding the device's.
    BaseNext,
    /// Following the device's base again.
    BaseDevice,
    /// The next page of digits (a base past the row's count).
    DigitPage,
    /// The readout: copy its value.
    Copy,
    /// Nothing (a cell past the base's digits, or a layer's last cells).
    None,
}

/// The keys between the digits on the outer row, in order of want (the row takes the first n).
const BETWEEN: [Act; 16] = [
    Act::Calc(Key::Write(keys::ADD)),
    Act::Calc(Key::Write(keys::SUB)),
    Act::Calc(Key::Write(keys::MUL)),
    Act::Calc(Key::Write(keys::DIV)),
    Act::Calc(Key::Write(keys::OPEN)),
    Act::Calc(Key::Write(keys::CLOSE)),
    Act::Calc(Key::Write(keys::POW)),
    Act::Calc(Key::Write(keys::POINT)),
    Act::Calc(Key::Write(keys::MOD)),
    Act::Calc(Key::Write(keys::LOGB)),
    Act::Calc(Key::Write(keys::PREV)),
    Act::Calc(Key::Square),
    Act::Calc(Key::Write(keys::SQRT)),
    Act::Calc(Key::Write(keys::RECIP)),
    Act::Calc(Key::Write(keys::PI)),
    Act::Calc(Key::Write(keys::E)),
];

/// Everything basecalc has, paged on the inner row by Shift; a key the outer row already holds is left out.
const MORE: [Act; 44] = [
    Act::Calc(Key::Write(keys::SIN)),
    Act::Calc(Key::Write(keys::COS)),
    Act::Calc(Key::Write(keys::TAN)),
    Act::Calc(Key::Write(keys::LN)),
    Act::Calc(Key::Write(keys::LOG)),
    Act::Calc(Key::Write(keys::SQRT)),
    Act::Calc(Key::Write(keys::PI)),
    Act::Calc(Key::Write(keys::E)),
    Act::Calc(Key::Write(keys::ABS)),
    Act::Calc(Key::Write(keys::RECIP)),
    Act::Calc(Key::Write(keys::ASIN)),
    Act::Calc(Key::Write(keys::ACOS)),
    Act::Calc(Key::Write(keys::ATAN)),
    Act::Calc(Key::Write(keys::CEIL)),
    Act::Calc(Key::Write(keys::FLOOR)),
    Act::Calc(Key::Write(keys::ROUND)),
    Act::Calc(Key::Write(keys::INT)),
    Act::Calc(Key::Write(keys::FRAC)),
    Act::Calc(Key::Write(keys::SIGN)),
    Act::Calc(Key::Write(keys::ERF)),
    Act::Calc(Key::Write(keys::PHI)),
    Act::Calc(Key::Write(keys::GAMMA)),
    Act::Calc(Key::Write(keys::CATALAN)),
    Act::Calc(Key::Write(keys::RAND)),
    Act::Calc(Key::Write(keys::GRAND)),
    Act::Calc(Key::Write(keys::I)),
    Act::Calc(Key::Write(keys::CPLX_OPEN)),
    Act::Calc(Key::Write(keys::CPLX_COMMA)),
    Act::Calc(Key::Write(keys::CPLX_CLOSE)),
    Act::Calc(Key::Write(keys::RE)),
    Act::Calc(Key::Write(keys::IM)),
    Act::Calc(Key::Write(keys::ARG)),
    Act::Calc(Key::Write(keys::MOD)),
    Act::Calc(Key::Write(keys::LOGB)),
    Act::Calc(Key::Write(keys::PREV)),
    Act::Calc(Key::Square),
    Act::Calc(Key::Write(keys::POW)),
    Act::Calc(Key::Write(keys::OPEN)),
    Act::Calc(Key::Write(keys::CLOSE)),
    Act::Calc(Key::Angle),
    Act::Calc(Key::Fewer),
    Act::Calc(Key::More),
    Act::BaseNext,
    Act::BaseDevice,
];

/// The quick bases the settings layer steps thru.
pub const QUICK: [u8; 6] = [2, 8, 10, 12, 16, 36];

/// The row's count for a base: the base itself from 10 to 16 (the dial is its base); a smaller base keeps ten (its digits and blank keys), a larger one sixteen with its digits paged. The bounds are the design's (fewer than ten leaves the arithmetic no room, more than sixteen makes keys too small to hit), not a guard.
pub fn ring_of(base: u8) -> usize {
    (base as usize).clamp(10, 16)
}

/// The dial for a base, a layer and a page of digits.
pub struct Dial {
    pub n: usize,
    /// The readout's radius and the rows' boundary, of the puck's radius.
    pub core: f32,
    pub mid: f32,
    /// The inner row (n keys, key k centred k/n of a turn from the top, clockwise) and the outer row (2n, key j centred j/2n).
    pub inner: Vec<Act>,
    pub outer: Vec<Act>,
    /// How many layers Shift steps thru.
    pub layers: usize,
}

impl Dial {
    pub fn new(base: u8, layer: usize, page: usize) -> Dial {
        let n = ring_of(base);
        let core = 0.25 + n as f32 / 64.;
        let mid = f32::sqrt(core * core + (1. - core * core) / 3.);
        // The outer row: the digits on the keys over the inner ones (even j), a base past the row paging them with its last such key; the arithmetic between.
        let paged = base as usize > n;
        let per = if paged { n - 1 } else { n };
        let outer: Vec<Act> = (0..2 * n)
            .map(|j| {
                let k = j / 2;
                if j % 2 == 1 {
                    BETWEEN[k]
                } else if paged && k == n - 1 {
                    Act::DigitPage
                } else {
                    let d = page * per + k;
                    if d < base as usize { Act::Calc(Key::Digit(d as u8)) } else { Act::None }
                }
            })
            .collect();
        // The inner row: Shift at the top, = at the bottom with AC before it and del after it, the layer's keys round the rest.
        let pool: Vec<Act> = MORE.iter().copied().filter(|a| !outer.contains(a)).collect();
        let free = n - 4;
        let layers = pool.len().div_ceil(free);
        let mut it = pool.into_iter().skip(layer % layers * free);
        let bottom = n / 2;
        let inner = (0..n)
            .map(|k| {
                if k == 0 {
                    Act::Shift
                } else if k == bottom - 1 {
                    Act::Calc(Key::Clear)
                } else if k == bottom {
                    Act::Calc(Key::Equals)
                } else if k == bottom + 1 {
                    Act::Calc(Key::Back)
                } else {
                    it.next().unwrap_or(Act::None)
                }
            })
            .collect();
        Dial { n, core, mid, inner, outer, layers }
    }

    /// The key at a point of the unit circle (y down, the dial upright): Copy in the readout, None past the rim.
    pub fn hit(&self, x: f32, y: f32) -> Option<Act> {
        let r = f32::sqrt(x * x + y * y);
        if r > 1. {
            return None;
        }
        if r < self.core {
            return Some(Act::Copy);
        }
        let (outer, k) = self.cell(r, turn_of(x, y));
        Some(if outer { self.outer[k] } else { self.inner[k] })
    }

    /// The cell at radius r and turn t: its row (true for the outer) and its key.
    fn cell(&self, r: f32, t: f32) -> (bool, usize) {
        let outer = r >= self.mid;
        let count = if outer { 2 * self.n } else { self.n };
        // Key k spans half a key either side of k/count: shifted by half a key, the floor is the key (t below 1, so the sum is below count + 1/2 and only count itself wraps).
        let k = (t * count as f32 + 0.5) as usize;
        (outer, if k == count { 0 } else { k })
    }
}

/// A point's turn from the top, clockwise, in [0, 1).
fn turn_of(x: f32, y: f32) -> f32 {
    let t = f32::atan2(x, -y) * (1. / core::f32::consts::TAU);
    if t < 0. { t + 1. } else { t }
}

/// Digits as ferros's face draws them: in base 12, tatau's twelve glyphs at G#10-G#1B (owner: "use the lower control set of ascii, not 0-B"); every other base as basecalc writes them. For digit runs only (a function's name keeps its letters).
pub fn digits(s: &str, base: u8) -> String {
    if base != 12 {
        return s.to_string();
    }
    s.chars()
        .map(|c| match c {
            '0'..='9' => char::from(c as u8 - b'0' + 0x10),
            'A' => '\u{1A}',
            'B' => '\u{1B}',
            _ => c,
        })
        .collect()
}

/// A unit's label in the characters ferros's face has (Oxanium has no superscript n, subscript n, phi or gamma).
pub fn label(u: Unit) -> &'static str {
    match u.text {
        "^" => "x^y",
        "$" => "logb",
        "@phi" => "phi",
        "@gamma" => "gamma",
        _ => u.label,
    }
}

/// A key's face and its body's colour.
fn face(act: Act, base: u8, layer: usize, radians: bool, following: bool) -> (String, u32) {
    match act {
        Act::Calc(Key::Digit(d)) => (digits(&keys::digit_char(d).to_string(), base), DIGIT),
        Act::Calc(Key::Equals) => ("=".into(), EQUALS),
        Act::Calc(Key::Back) => ("del".into(), CONTROL),
        Act::Calc(Key::Clear) => ("AC".into(), CONTROL),
        Act::Calc(Key::Angle) => ((if radians { "rad" } else { "deg" }).into(), CONTROL),
        Act::Calc(Key::Fewer) => ("\u{2212}dig".into(), CONTROL),
        Act::Calc(Key::More) => ("+dig".into(), CONTROL),
        Act::Calc(Key::Write(u)) => (label(u).into(), kind_colour(u)),
        Act::Calc(k) => (k.label(), FUNCTION),
        // WHY the floor on the base: the layer is written in the base in use, and a base under 2 has no digits (never: the engine holds 2 to 36).
        Act::Shift => (if layer == 0 { "\u{2191}".into() } else { format!("\u{2191}{}", digits(&keys::in_base(layer, base.max(2)), base)) }, if layer == 0 { CONTROL } else { SHIFTED }),
        Act::BaseNext => (keys::base_label(base), if following { CONTROL } else { SHIFTED }),
        Act::BaseDevice => ("dev#".into(), if following { SHIFTED } else { CONTROL }),
        Act::DigitPage => ("\u{2026}".into(), CONTROL),
        Act::Copy | Act::None => (String::new(), GROUND),
    }
}

fn kind_colour(u: Unit) -> u32 {
    match u.kind {
        keys::Kind::Infix => OPERATOR,
        keys::Kind::Digit => DIGIT,
        _ => FUNCTION,
    }
}

/// C¹ coverage across an edge from a signed distance in pixels (positive inside): smoothstep over the pixel straddling it.
fn edge(d: f32) -> f32 {
    let t = d + 0.5;
    if t <= 0. {
        0.
    } else if t >= 1. {
        1.
    } else {
        t * t * (3. - 2. * t)
    }
}

/// A smooth minimum of two distances, `−k·ln(e^(−a/k) + e^(−b/k))` (C∞): rounds a cell's corners by about k with no kink where the nearer edge changes.
fn soft_min(a: f32, b: f32, k: f32) -> f32 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    lo - k * f32::ln(1. + f32::exp((lo - hi) / k))
}

/// What the readout shows, every digit already in ferros's glyphs (`digits`).
pub struct Screen<'a> {
    pub expr: &'a str,
    pub value: &'a str,
    pub error: bool,
    pub dim: bool,
    pub base: u8,
    pub radians: bool,
    pub following: bool,
    /// The readout was tapped (its value copied) since the last key.
    pub copied: bool,
}

/// Words centred on (cx, cy) in ferros's face under what is there, `size` pixels, shrunk to `room` wide if wider.
fn words(px: &mut [G2t16], d: usize, g: &mut Glyphs, s: &str, cx: f32, cy: f32, size: f32, room: f32, ink: G2t16) {
    if s.is_empty() {
        return;
    }
    // WHY the floor: words with no advance in the face would give a width of 0 to divide by.
    let w = g.width(s, size).max(1.);
    let size = if w > room { size * room / w } else { size };
    let w = g.width(s, size);
    let di = d as i64;
    g.cover(s, cx - w * 0.5, cy + size * 0.36, size, (1., 0.), |x, y, a| {
        // WHY the bounds: a glyph's pixels come from its outline, and words near the rim can reach past the square.
        if x >= 0 && y >= 0 && x < di && y < di {
            let i = y as usize * d + x as usize;
            px[i] = px[i].under(G2t16::from_cov(ink, a), BlendMode::Normal);
        }
    });
}

/// The dial into `px`, a square `d` pixels a side (cleared), upright: front to back, the words first (the readout's, then each key's face), then each cell under them, then the ground.
pub fn paint(px: &mut [G2t16], d: usize, dial: &Dial, layer: usize, screen: &Screen, g: &mut Glyphs) {
    let rp = d as f32 * 0.5;
    let n = dial.n;
    let tau = core::f32::consts::TAU;
    let base = screen.base;
    let (ink, dim, error) = (G2t16::from_g2t8(INK), G2t16::from_g2t8(INK_DIM), G2t16::from_g2t8(INK_ERROR));
    // The readout: the expression small above the value, the base (a dot when overridden) or "copied" under it, each fitted to the core's chord where it sits.
    let core = dial.core * rp;
    // WHY the floor: a line's height is inside the core by these fractions, so the root's argument stays positive; the floor only spares a NaN if a fraction is ever changed past the core.
    let chord = |y: f32| 2. * f32::sqrt((core * core - y * y).max(0.)) * 0.86;
    words(px, d, g, screen.expr, rp, rp - core * 0.45, core * 0.2, chord(core * 0.55), dim);
    words(px, d, g, screen.value, rp, rp, core * 0.36, chord(core * 0.2), if screen.error { error } else if screen.dim { dim } else { ink });
    let under = if screen.copied { "copied".to_string() } else { format!("{}{}", keys::base_label(base), if screen.following { "" } else { " \u{2022}" }) };
    words(px, d, g, &under, rp, rp + core * 0.5, core * 0.15, chord(core * 0.6), dim);
    // Each key's face at its cell's middle, as big as the cell allows.
    for (outer, r0, r1, count) in [(false, dial.core, dial.mid, n), (true, dial.mid, 1., 2 * n)] {
        let rm = (r0 + r1) * 0.5;
        let depth = (r1 - r0) * rp;
        let width = tau * rm * rp / count as f32;
        for k in 0..count {
            let act = if outer { dial.outer[k] } else { dial.inner[k] };
            let (s, _) = face(act, base, layer, screen.radians, screen.following);
            let t = k as f32 / count as f32 * tau;
            let size = f32::min(depth, width) * if s.chars().count() > 2 { 0.4 } else { 0.55 };
            words(px, d, g, &s, rp + rm * rp * f32::sin(t), rp - rm * rp * f32::cos(t), size, width * 0.8, ink);
        }
    }
    // The cells under the words: each pixel's row and key by radius and turn, its distance in from the cell's edges (across the row to the nearer circle, along it to the nearer seam as an arc at this radius, the corners rounded by the soft minimum), a gutter's width in; the readout's disc; the ground under all.
    let body = |outer: bool, k: usize| G2t16::from_g2t8(face(if outer { dial.outer[k] } else { dial.inner[k] }, base, layer, screen.radians, screen.following).1);
    let inner_c: Vec<G2t16> = (0..n).map(|k| body(false, k)).collect();
    let outer_c: Vec<G2t16> = (0..2 * n).map(|k| body(true, k)).collect();
    let (core_px, ground) = (G2t16::from_g2t8(CORE), G2t16::from_g2t8(GROUND));
    let gutter = rp * (1. / (1 << 6) as f32);
    let round = gutter * 2.;
    let inv = 1. / rp;
    for y in 0..d {
        let uy = (y as f32 + 0.5 - rp) * inv;
        for x in 0..d {
            let ux = (x as f32 + 0.5 - rp) * inv;
            let r = f32::sqrt(ux * ux + uy * uy);
            let rim = edge((1. - r) * rp);
            if rim <= 0. {
                continue;
            }
            let (cell, inside) = if r < dial.core {
                (core_px, (dial.core - r) * rp - gutter * 0.5)
            } else {
                let t = turn_of(ux, uy);
                let (outer, k) = dial.cell(r, t);
                let (count, r0, r1) = if outer { (2 * n, dial.mid, 1.) } else { (n, dial.core, dial.mid) };
                let across = f32::min(r - r0, r1 - r) * rp;
                let off = t * count as f32 - k as f32;
                // The wrapped key 0 sits at t near 1: its offset is past half a key, so one key back.
                let off = if off > 0.5 { off - count as f32 } else { off };
                let along = (0.5 - off.abs()) * (tau / count as f32) * r * rp;
                (if outer { outer_c[k] } else { inner_c[k] }, soft_min(across, along, round) - gutter * 0.5)
            };
            let i = y * d + x;
            let cov = edge(inside) * rim;
            if cov > 0. {
                px[i] = px[i].under(G2t16::from_cov(cell, (cov * 65535.) as u32), BlendMode::Normal);
            }
            px[i] = px[i].under(G2t16::from_cov(ground, (rim * 65535.) as u32), BlendMode::Normal);
        }
    }
}
