//! basecalc's dial (owner, 2026-11-18: "Think single layer, Warrington or combined wire rope patterns. How many buttons we want How can we tessellate/honeycomb it?" then "Go!"): the puck's keys as a Seale strand, a core and two rings of n, n the base.
//!
//! - **The strand.** The display is the core; an inner ring of n smaller keys touches it and each other; an outer ring of n larger keys sits in the inner ring's valleys, touching two inner keys, each other and the puck's rim (`seale`). n is the base from 10 to 16 (the dozenal dial is 1 + 12 + 12).
//! - **The cells.** Every pixel belongs to the key whose circle it is nearest by power (|p − c|² − r²: a power diagram, the honeycomb keyboard's nearest-centre rule with each key's own size, so two touching keys meet exactly where their circles touch), the core's pixels to the display, the whole clipped by the rim. A key's cell is drawn with a C¹ edge a gutter's width inside its boundary, and a tap is the cell it lands in: drawing and hitting share one rule.
//! - **The outer ring** holds the base's digits, 0 at the top and clockwise, so the dial shows the base itself. A base above the ring's count pages its digits (the last cell).
//! - **The inner ring** holds the arithmetic (layer 0); Shift pages it through the rest of basecalc (its functions, constants, the complex keys and the settings), Shift, = and ⌫ staying where they are.
//! - **The base** follows the device's (Settings, Units: arabic, dozenal or hex) until the settings layer overrides it; following comes back by its key.

use basecalc_kist::keys::{self, Key, Unit};
use fluor::canvas::Canvas;
use fluor::pixel::{Blend, BlendMode, G2t16, Pixel};
use fluor::text::{TextRenderer, TextStyle};
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
    /// One bracket key for both (the decimal dial, ten keys a ring): opens, or closes when more are open than closed and a value ends the expression.
    Paren,
    /// The inner ring's next layer.
    Shift,
    /// The next of the quick bases (2, 8, 10, 12, 16, 36), overriding the device's.
    BaseNext,
    /// Following the device's base again.
    BaseDevice,
    /// The outer ring's next page of digits (a base above the ring's count).
    DigitPage,
    /// Nothing (a cell past the base's digits).
    None,
}

/// The arithmetic layer before its fixed keys, in the order they are wanted when a ring is short (the dozenal ring takes the first nine).
const ARITH: [Act; 13] = [
    Act::Calc(Key::Write(keys::OPEN)),
    Act::Calc(Key::Write(keys::CLOSE)),
    Act::Calc(Key::Write(keys::ADD)),
    Act::Calc(Key::Write(keys::SUB)),
    Act::Calc(Key::Write(keys::MUL)),
    Act::Calc(Key::Write(keys::DIV)),
    Act::Calc(Key::Write(keys::POW)),
    Act::Calc(Key::Write(keys::POINT)),
    Act::Calc(Key::Clear),
    Act::Calc(Key::Write(keys::MOD)),
    Act::Calc(Key::Write(keys::LOGB)),
    Act::Calc(Key::Write(keys::PREV)),
    Act::Calc(Key::Square),
];

/// Everything else basecalc has, paged across the inner ring by Shift: its functions, inverses and parts, constants, the complex keys, then the settings.
const MORE: [Act; 39] = [
    Act::Calc(Key::Write(keys::SIN)),
    Act::Calc(Key::Write(keys::COS)),
    Act::Calc(Key::Write(keys::TAN)),
    Act::Calc(Key::Write(keys::LN)),
    Act::Calc(Key::Write(keys::LOG)),
    Act::Calc(Key::Write(keys::SQRT)),
    Act::Calc(Key::Write(keys::ABS)),
    Act::Calc(Key::Write(keys::LOGB)),
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
    Act::Calc(Key::Write(keys::PI)),
    Act::Calc(Key::Write(keys::E)),
    Act::Calc(Key::Write(keys::PHI)),
    Act::Calc(Key::Write(keys::GAMMA)),
    Act::Calc(Key::Write(keys::CATALAN)),
    Act::Calc(Key::Write(keys::RAND)),
    Act::Calc(Key::Write(keys::GRAND)),
    Act::Calc(Key::Write(keys::PREV)),
    Act::Calc(Key::Write(keys::I)),
    Act::Calc(Key::Write(keys::CPLX_OPEN)),
    Act::Calc(Key::Write(keys::CPLX_COMMA)),
    Act::Calc(Key::Write(keys::CPLX_CLOSE)),
    Act::Calc(Key::Write(keys::MOD)),
    Act::Calc(Key::Square),
    Act::Calc(Key::Clear),
    Act::Calc(Key::Angle),
    Act::Calc(Key::Fewer),
    Act::Calc(Key::More),
    Act::BaseNext,
    Act::BaseDevice,
];

/// The quick bases the settings layer steps thru.
pub const QUICK: [u8; 6] = [2, 8, 10, 12, 16, 36];

/// A Seale strand of n around a core in a circle of radius 1: (the core's radius, the inner keys' ring radius and radius, the outer keys' ring radius and radius). The outer keys touch each other and the rim (b = d₂ sin(π/n), d₂ + b = 1); the inner keys touch each other (a = d₁ sin(π/n)) and the outer ones (a + b = |c₁ − c₂| across the half step), solved for d₁ by bisection; the core touches the inner keys.
pub fn seale(n: usize) -> (f32, f32, f32, f32, f32) {
    let (s, c) = (f32::sin(core::f32::consts::PI / n as f32), f32::cos(core::f32::consts::PI / n as f32));
    let d2 = 1. / (1. + s);
    let b = s * d2;
    let (mut lo, mut hi) = (0f32, d2);
    for _ in 0..1 << 6 {
        let mid = (lo + hi) * 0.5;
        let a = mid * s;
        if (a + b) * (a + b) > mid * mid + d2 * d2 - 2. * mid * d2 * c { hi = mid } else { lo = mid }
    }
    (lo * (1. - s), lo, lo * s, d2, b)
}

/// A key on the dial: its centre and radius (unit circle, y down), what it does, and which ring.
#[derive(Clone, Copy)]
pub struct Site {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub act: Act,
    pub outer: bool,
}

/// The dial for a base and a layer: the core's radius and every key.
pub struct Dial {
    pub n: usize,
    pub core: f32,
    pub sites: Vec<Site>,
    /// How many layers Shift steps thru.
    pub layers: usize,
}

/// The ring's count for a base: the base itself from 10 to 16 (the dial is its base); a smaller base keeps ten cells (its digits and blank ones), a larger one sixteen with its digits paged. The bounds are the design's (a ring of fewer than ten leaves the arithmetic no room, one of more than sixteen makes keys too small to hit), not a guard.
pub fn ring_of(base: u8) -> usize {
    (base as usize).clamp(10, 16)
}

impl Dial {
    pub fn new(base: u8, layer: usize, page: usize) -> Dial {
        let n = ring_of(base);
        let (core, d1, a, d2, b) = seale(n);
        let step = core::f32::consts::TAU / n as f32;
        let mut sites = Vec::with_capacity(2 * n);
        // The outer ring: digits from the top, clockwise; a base past the ring pages them, its last cell turning the page.
        let paged = base as usize > n;
        let per = if paged { n - 1 } else { n };
        for k in 0..n {
            let t = k as f32 * step;
            let act = if paged && k == n - 1 {
                Act::DigitPage
            } else {
                let d = page * per + k;
                if d < base as usize { Act::Calc(Key::Digit(d as u8)) } else { Act::None }
            };
            sites.push(Site { x: d2 * f32::sin(t), y: -d2 * f32::cos(t), r: b, act, outer: true });
        }
        // The inner ring, half a step round (in the outer ring's valleys): Shift at the top, = at the bottom, ⌫ after it, the layer's keys round the rest.
        let fixed = 3;
        let free = n - fixed;
        let layers = 1 + MORE.len().div_ceil(free);
        let mut acts: Vec<Act> = if layer == 0 {
            let mut v: Vec<Act> = ARITH.iter().copied().take(free).collect();
            // Ten keys a ring: one bracket key for both, and ^ in the room the other bracket leaves.
            if n == 10 {
                v.retain(|a| *a != Act::Calc(Key::Write(keys::CLOSE)));
                v[0] = Act::Paren;
                v.push(ARITH[6]);
                v.dedup();
                v.truncate(free);
            }
            v
        } else {
            MORE.iter().copied().skip((layer - 1) * free).take(free).collect()
        };
        acts.resize(free, Act::None);
        let bottom = n / 2;
        let mut ring = Vec::with_capacity(n);
        let mut it = acts.into_iter();
        for k in 0..n {
            ring.push(if k == 0 {
                Act::Shift
            } else if k == bottom {
                Act::Calc(Key::Equals)
            } else if k == bottom + 1 {
                Act::Calc(Key::Back)
            } else {
                it.next().unwrap_or(Act::None)
            });
        }
        for (k, act) in ring.into_iter().enumerate() {
            let t = (k as f32 + 0.5) * step;
            sites.push(Site { x: d1 * f32::sin(t), y: -d1 * f32::cos(t), r: a, act, outer: false });
        }
        Dial { n, core, sites, layers }
    }

    /// The key a point of the unit circle belongs to (its power-diagram cell), None in the display or past the rim.
    pub fn hit(&self, x: f32, y: f32) -> Option<Act> {
        if x * x + y * y > 1. {
            return None;
        }
        let core = x * x + y * y - self.core * self.core;
        let (k, p) = self.nearest(x, y);
        (p < core).then(|| self.sites[k].act)
    }

    /// The site nearest a point by power, and that power.
    fn nearest(&self, x: f32, y: f32) -> (usize, f32) {
        let mut best = (0, f32::MAX);
        for (k, s) in self.sites.iter().enumerate() {
            let p = (x - s.x) * (x - s.x) + (y - s.y) * (y - s.y) - s.r * s.r;
            if p < best.1 {
                best = (k, p);
            }
        }
        best
    }
}

/// A key's face and its body's colour.
fn face(act: Act, base: u8, layer: usize, radians: bool, following: bool) -> (String, u32) {
    match act {
        Act::Calc(Key::Digit(d)) => (keys::digit_char(d).to_string(), DIGIT),
        Act::Calc(Key::Equals) => ("=".into(), EQUALS),
        Act::Calc(Key::Back) => ("\u{232b}".into(), CONTROL),
        Act::Calc(Key::Clear) => ("AC".into(), CONTROL),
        Act::Calc(Key::Angle) => ((if radians { "rad" } else { "deg" }).into(), CONTROL),
        Act::Calc(Key::Fewer) => ("\u{2212}dig".into(), CONTROL),
        Act::Calc(Key::More) => ("+dig".into(), CONTROL),
        Act::Calc(Key::Write(u)) => (u.label.into(), kind_colour(u)),
        Act::Calc(k) => (k.label(), FUNCTION),
        Act::Paren => ("( )".into(), OPERATOR),
        // WHY the floor on the base: the layer is written in the base in use, and a base under 2 has no digits (never: the engine holds 2 to 36).
        Act::Shift => (if layer == 0 { "\u{21e7}".into() } else { format!("\u{21e7}{}", keys::in_base(layer, base.max(2))) }, if layer == 0 { CONTROL } else { SHIFTED }),
        Act::BaseNext => (keys::base_label(base), if following { CONTROL } else { SHIFTED }),
        Act::BaseDevice => ("dev#".into(), if following { SHIFTED } else { CONTROL }),
        Act::DigitPage => ("\u{2026}".into(), CONTROL),
        Act::None => (String::new(), GROUND),
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

/// What the display shows: the expression above, the value large, its base under.
pub struct Screen<'a> {
    pub expr: &'a str,
    pub value: &'a str,
    pub error: bool,
    pub dim: bool,
    pub base: u8,
    pub radians: bool,
    pub following: bool,
}

/// The dial into `px`, a square `d` pixels a side (cleared), upright: front to back, the words first (the display's, then each key's face), then each cell under them, then the ground.
pub fn paint(px: &mut [G2t16], d: usize, dial: &Dial, layer: usize, screen: &Screen, text: &mut TextRenderer, damage: &mut fluor::canvas::Damage) {
    let r_px = d as f32 * 0.5;
    let to_px = |v: f32| r_px + v * r_px;
    {
        let mut canvas = Canvas::new(&mut px[..d * d], d, d, damage);
        // The display in the core: the expression small over the value, the base under, each fitted to the core's chord where it sits.
        let core = dial.core * r_px;
        let mut fit = |s: &str, size: f32, y: f32, ink: u32| {
            if s.is_empty() {
                return;
            }
            // WHY the floors: a line placed past the core (y beyond its radius) has no chord, the root of a negative; a string with no advance would give a width of 0 to divide by.
            let chord = 2. * f32::sqrt((core * core - y * y).max(0.)) * 0.86;
            let w = text.measure_text(s, &TextStyle::new(size, ink)).max(1.);
            let sz = if w > chord { size * chord / w } else { size };
            text.draw_text_center(&mut canvas, s, r_px, r_px + y, &TextStyle::new(sz, ink), None, None);
        };
        fit(screen.expr, core * 0.22, -core * 0.42, INK_DIM);
        fit(screen.value, core * 0.38, 0., if screen.error { INK_ERROR } else if screen.dim { INK_DIM } else { INK });
        let under = format!("{}{}", keys::base_label(screen.base), if screen.following { "" } else { " \u{2022}" });
        fit(&under, core * 0.16, core * 0.5, INK_DIM);
        let _ = screen.radians;
        // Each key's face at its centre, as large as its circle allows.
        for s in &dial.sites {
            let (label, _) = face(s.act, screen.base, layer, screen.radians, screen.following);
            if label.is_empty() {
                continue;
            }
            let room = s.r * r_px * 1.5;
            let size = s.r * r_px * if s.outer { 0.9 } else { 0.8 };
            // WHY the floor: a face with no advance would give a width of 0 to divide by.
            let w = text.measure_text(&label, &TextStyle::new(size, INK)).max(1.);
            let sz = if w > room { size * room / w } else { size };
            text.draw_text_center(&mut canvas, &label, to_px(s.x), to_px(s.y), &TextStyle::new(sz, INK), None, None);
        }
    }
    // The cells under the words: each pixel's key by power, its colour at the coverage of its cell's edge (a gutter's width in from the boundary to the next key, and the rim), the ground under all.
    let colours: Vec<G2t16> = dial.sites.iter().map(|s| G2t16::from_g2t8(face(s.act, screen.base, layer, screen.radians, screen.following).1)).collect();
    let (core_px, ground) = (G2t16::from_g2t8(CORE), G2t16::from_g2t8(GROUND));
    let gutter = r_px * (1. / (1 << 6) as f32);
    let inv = 1. / r_px;
    for y in 0..d {
        let uy = (y as f32 + 0.5 - r_px) * inv;
        for x in 0..d {
            let ux = (x as f32 + 0.5 - r_px) * inv;
            let rr = ux * ux + uy * uy;
            let rim = edge((1. - f32::sqrt(rr)) * r_px);
            if rim <= 0. {
                continue;
            }
            // The best two sites by power, the core among them: the boundary between the best and the second is the line where their powers are equal, its distance from the point (p₂ − p₁) / 2|c₁ − c₂|.
            let pc = rr - dial.core * dial.core;
            let (mut b1, mut p1, mut b2, mut p2) = (usize::MAX, pc, usize::MAX, f32::MAX);
            for (k, s) in dial.sites.iter().enumerate() {
                let p = (ux - s.x) * (ux - s.x) + (uy - s.y) * (uy - s.y) - s.r * s.r;
                if p < p1 {
                    (b2, p2) = (b1, p1);
                    (b1, p1) = (k, p);
                } else if p < p2 {
                    (b2, p2) = (k, p);
                }
            }
            let at = |k: usize| if k == usize::MAX { (0f32, 0f32) } else { (dial.sites[k].x, dial.sites[k].y) };
            let ((ax, ay), (bx, by)) = (at(b1), at(b2));
            // WHY the floor: two sites never share a centre, but the core and a site at the origin would; the distance is the divisor.
            let sep = f32::sqrt((ax - bx) * (ax - bx) + (ay - by) * (ay - by)).max(1e-6);
            let inside = (p2 - p1) / (2. * sep) * r_px - gutter * 0.5;
            let cov = edge(inside) * rim;
            let cell = if b1 == usize::MAX { core_px } else { colours[b1] };
            let i = y * d + x;
            if cov > 0. {
                px[i] = px[i].under(G2t16::from_cov(cell, (cov * 65535.) as u32), BlendMode::Normal);
            }
            px[i] = px[i].under(G2t16::from_cov(ground, (rim * 65535.) as u32), BlendMode::Normal);
        }
    }
}
