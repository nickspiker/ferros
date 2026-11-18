//! Pucks (docs/PUCKS.md; owner, 2026-11-17: "Pucks! I'm DOWN!"): the list of drawing ops a puck sends the display for its circle.
//!
//! A puck is a kist that lives on the deck as a live circle and is never opened.
//! It never touches pixels: it sends the display a short list of ops in its circle's own units, and the display draws that list at whatever size the deck's zoom gives the circle, so the face is sharp at every zoom and the puck holds no surface.
//!
//! - **Units.** Every coordinate is Q14 of the circle's radius from its centre (`ONE` is the radius), x to the right and y down the circle's own up, so a face turns with its circle.
//! - **Order.** The first op is the frontmost: the display walks the list front to back and puts each op under what is already there (fluor's `under`; AGENT.md "Compositing"), never over.
//! - **The circle is the clip.** Nothing an op draws reaches past the circle's edge (`draw`): a puck can only ever paint its own circle.
//! - **Time without a timer.** Two kinds of op move on the display's own clock, so a puck wakes only when its words change: a `SWEEP` whose end is at `(now - t0) / period` of a turn, and a `MARKER` at now along a `CURVE` that spans [t0, t1].
//! - **Values the display formats.** A `MOMENT` (Eagle time) is shown as wall time in the person's base and zone, a `NUMBER` (Q16 of its unit) in the person's base, so a change of base or zone redraws every puck with no puck waking.
//!
//! The wire form (a list): `[VERSION][count u8]`, then each op `[kind u8][len u16][body]`, little-endian, at most `OPS_MAX` ops in `LIST_MAX` bytes.
//! [`Writer`] makes one (the puck's side); [`check`] refuses any list with an unknown kind, a body of the wrong length, a bad colour or a bound passed, whole (the display's side: the last good list stays); [`draw::draw`] draws a checked list.
#![no_std]

pub mod draw;
pub mod fmt;
#[cfg(feature = "sys")]
pub mod rt;
#[cfg(test)]
mod tests;

/// The list's form; a list of another version is refused whole.
pub const VERSION: u8 = 0;
/// The most bytes a list may hold: one call to the display, with room for the kernel's 64-byte stamp and the call's own header inside a 4 KiB message.
pub const LIST_MAX: usize = (1 << 12) - (1 << 7);
/// The most ops in one list (PUCKS.md "The ops": 2^6).
pub const OPS_MAX: usize = 1 << 6;
/// The radius, in the ops' Q14 units.
pub const ONE: i32 = 1 << 14;
/// Eagle oscillations a second (vsf's OSCILLATIONS_PER_SECOND): every time in a list is Eagle time in these.
pub const OSC: i64 = 1_420_407_826;
/// Where the Eagle epoch (1969-07-20 20:17:40 UTC) falls in POSIX seconds, for the wall time a `MOMENT` shows.
pub const EAGLE_EPOCH_UNIX_SECS: i64 = -14_182_940;
/// The most curve samples one `CURVE` carries (ferros_tide `CURVE`).
pub const SAMPLES_MAX: usize = 1 << 6;
/// The most bytes of text one `TEXT` carries.
pub const TEXT_MAX: usize = 1 << 6;

/// The op kinds, by their byte.
pub mod kind {
    /// `[cx i16][cy i16][r u16][colour]`: a filled circle.
    pub const DISC: u8 = 0;
    /// `[cx][cy][outer u16][inner u16][colour]`: an annulus.
    pub const RING: u8 = 1;
    /// `[cx][cy][r u16][width u16][from u16][sweep u16][colour]`: a band of `width` along the circle of radius r, from `from` (Q16 of a turn, clockwise from the circle's up) for `sweep` (Q16), round ended.
    pub const ARC: u8 = 2;
    /// `[ax i16][ay i16][bx i16][by i16][r u16][colour]`: every point within r of the segment (the slider's track).
    pub const CAPSULE: u8 = 3;
    /// `[x0 i16][x1 i16][yb i16][yt i16][vlo i32][vhi i32][width u16][line colour][fill colour][n u8][turn u16]` then n × `[value i32][slope i32]`: n samples of a value and its slope (value units per the whole span x0..x1) at even steps from x0 to x1, joined by cubic Hermite pieces (C¹, each sample at its true slope); vlo maps to yb and vhi to yt; drawn as a line of `width` and, unless the fill's tag is `NONE`, filled from the line to yb.
    pub const CURVE: u8 = 4;
    /// `[cx i16][cy i16][size u16][colour][utf8]`: words a human reads, centred on (cx, cy), `size` the glyphs' height in Q14 of the radius, turned with the circle.
    pub const TEXT: u8 = 5;
    /// `[cx][cy][r u16][width u16][from u16][t0 i64][period u32][colour]`: an `ARC` whose sweep is `(now - t0) / period` of a turn (period in seconds), wrapping each period.
    pub const SWEEP: u8 = 6;
    /// `[curve u8][t0 i64][t1 i64][r u16][colour]`: a dot of radius r on the `CURVE` op at index `curve` (an earlier op of this list), at the x that now takes on [t0, t1] mapped to [x0, x1]; nothing outside it.
    pub const MARKER: u8 = 7;
    /// `[cx][cy][size u16][colour][t i64][places u8]`: the moment t (Eagle oscillations) as wall time in the person's base and zone, `places` its digits after the first (the display's `fmt::moment`).
    pub const MOMENT: u8 = 8;
    /// `[cx][cy][size u16][colour][value i64][places u8][unit u8]`: a value in Q16 of its unit, in the person's base with `places` places after the point, its unit's symbol after it (`fmt::UNITS`).
    pub const NUMBER: u8 = 9;
    /// One past the last kind.
    pub const END: u8 = 10;
}

/// A colour, five bytes on the wire: `[tag][alpha][x][y][z]`.
/// - tag `RGB`: x, y, z are VSF RGB gamma-2 codes (R, G, B), as the display's region ops take them (AGENT.md: colours are authored in VSF RGB only).
/// - tag `THEME`: x is a slot of the person's theme (`theme`), so a puck follows the theme; y and z are zero.
/// - tag `NONE` (a curve's fill only): no fill.
/// Alpha is the colour's opacity, 255 opaque.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colour {
    Rgb { r: u8, g: u8, b: u8, a: u8 },
    Theme { slot: u8, a: u8 },
}

/// A colour's tags.
pub const RGB: u8 = 0;
pub const THEME: u8 = 1;
pub const NONE: u8 = 2;

/// The theme's slots a `THEME` colour names (ferros_sys::theme::Theme's fields of those names).
pub mod theme {
    /// The text on the deck.
    pub const TEXT: u8 = 0;
    /// The quieter text.
    pub const TEXT2: u8 = 1;
    /// A plain circle's fill.
    pub const PLANE: u8 = 2;
    /// A circle's ring.
    pub const RING: u8 = 3;
    /// What is lit (changed since last seen).
    pub const LIT: u8 = 4;
    /// One past the last slot.
    pub const END: u8 = 5;
}

impl Colour {
    /// An opaque VSF RGB gamma-2 colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Colour {
        Colour::Rgb { r, g, b, a: 255 }
    }
    /// A theme slot, opaque.
    pub const fn theme(slot: u8) -> Colour {
        Colour::Theme { slot, a: 255 }
    }
    /// The same colour at opacity `a`.
    pub const fn alpha(self, a: u8) -> Colour {
        match self {
            Colour::Rgb { r, g, b, .. } => Colour::Rgb { r, g, b, a },
            Colour::Theme { slot, .. } => Colour::Theme { slot, a },
        }
    }
    pub const fn opacity(self) -> u8 {
        match self {
            Colour::Rgb { a, .. } | Colour::Theme { a, .. } => a,
        }
    }
    fn put(self, out: &mut [u8]) {
        let b = match self {
            Colour::Rgb { r, g, b, a } => [RGB, a, r, g, b],
            Colour::Theme { slot, a } => [THEME, a, slot, 0, 0],
        };
        out[..5].copy_from_slice(&b);
    }
    fn read(b: &[u8]) -> Option<Option<Colour>> {
        match b[0] {
            RGB => Some(Some(Colour::Rgb { r: b[2], g: b[3], b: b[4], a: b[1] })),
            THEME if b[2] < theme::END && b[3] == 0 && b[4] == 0 => Some(Some(Colour::Theme { slot: b[2], a: b[1] })),
            NONE if b[1..5] == [0; 4] => Some(None),
            _ => None,
        }
    }
}

/// Why a list was refused (the display answers it to the puck; the last good list stays on the glass).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// Shorter than its header, or another version.
    Header,
    /// More than `OPS_MAX` ops or `LIST_MAX` bytes.
    TooBig,
    /// Op `i`'s kind is unknown.
    Kind(u8),
    /// Op `i`'s body is cut short or its length is not its kind's.
    Length(u8),
    /// Op `i` carries a bad value: a colour's tag, a curve's span or sample count, a marker's curve, text that is not UTF-8.
    Value(u8),
    /// Bytes after the last op the count names.
    Trailing,
}

/// A curve as an op carries it.
#[derive(Clone, Copy, Debug)]
pub struct Curve<'a> {
    pub x0: i16,
    pub x1: i16,
    pub yb: i16,
    pub yt: i16,
    pub vlo: i32,
    pub vhi: i32,
    pub width: u16,
    pub line: Colour,
    /// The curve's frame turned clockwise about the circle's centre, Q16 of a turn (0: x along the circle's right): a moon's terminator toward its lit limb, a dial's scale.
    pub turn: u16,
    pub fill: Option<Colour>,
    /// n × 8 bytes: value i32, slope i32.
    samples: &'a [u8],
}

impl Curve<'_> {
    pub fn n(&self) -> usize {
        self.samples.len() / 8
    }
    /// Sample k: (value, slope per the whole span).
    pub fn sample(&self, k: usize) -> (i32, i32) {
        let s = &self.samples[8 * k..8 * k + 8];
        (i32::from_le_bytes(s[0..4].try_into().unwrap()), i32::from_le_bytes(s[4..8].try_into().unwrap()))
    }
    /// The value and its slope (per the whole span) at s in [0, 1] of the span: the cubic Hermite piece between the two samples around it.
    pub fn at(&self, s: f32) -> (f32, f32) {
        let n = self.n();
        let segs = (n - 1) as f32;
        let f = s * segs;
        // WHY the bound: s = 1 exactly lands on the last sample, which ends the last piece rather than starting one past it.
        let k = (libm::floorf(f) as usize).min(n - 2);
        let u = f - k as f32;
        let h = 1. / segs;
        let (v0, m0) = self.sample(k);
        let (v1, m1) = self.sample(k + 1);
        let (v0, m0, v1, m1) = (v0 as f32, m0 as f32 * h, v1 as f32, m1 as f32 * h);
        let (u2, u3) = (u * u, u * u * u);
        let v = (2. * u3 - 3. * u2 + 1.) * v0 + (u3 - 2. * u2 + u) * m0 + (-2. * u3 + 3. * u2) * v1 + (u3 - u2) * m1;
        let dv = (6. * u2 - 6. * u) * v0 + (3. * u2 - 4. * u + 1.) * m0 + (-6. * u2 + 6. * u) * v1 + (3. * u2 - 2. * u) * m1;
        (v, dv / h)
    }
}

/// One op, decoded.
#[derive(Clone, Copy, Debug)]
pub enum Op<'a> {
    Disc { cx: i16, cy: i16, r: u16, c: Colour },
    Ring { cx: i16, cy: i16, outer: u16, inner: u16, c: Colour },
    Arc { cx: i16, cy: i16, r: u16, width: u16, from: u16, sweep: u16, c: Colour },
    Capsule { ax: i16, ay: i16, bx: i16, by: i16, r: u16, c: Colour },
    Curve(Curve<'a>),
    Text { cx: i16, cy: i16, size: u16, c: Colour, s: &'a str },
    Sweep { cx: i16, cy: i16, r: u16, width: u16, from: u16, t0: i64, period: u32, c: Colour },
    Marker { curve: u8, t0: i64, t1: i64, r: u16, c: Colour },
    Moment { cx: i16, cy: i16, size: u16, c: Colour, t: i64, places: u8 },
    Number { cx: i16, cy: i16, size: u16, c: Colour, value: i64, places: u8, unit: u8 },
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}
fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn i32_at(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn i64_at(b: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The fixed body length of each kind (the curve's and the text's are their head; their tails vary).
const fn fixed_len(k: u8) -> usize {
    match k {
        kind::DISC => 11,
        kind::RING => 13,
        kind::ARC => 17,
        kind::CAPSULE => 15,
        kind::CURVE => 31,
        kind::TEXT => 11,
        kind::SWEEP => 27,
        kind::MARKER => 24,
        kind::MOMENT => 20,
        kind::NUMBER => 21,
        _ => 0,
    }
}

impl<'a> Op<'a> {
    /// The op of `kind` with body `b`, or None when the body is not that kind's.
    pub fn decode(k: u8, b: &'a [u8]) -> Option<Op<'a>> {
        let need = fixed_len(k);
        if need == 0 || b.len() < need {
            return None;
        }
        let col = |at: usize| Colour::read(&b[at..at + 5]).flatten();
        let exact = |op: Op<'a>| (b.len() == need).then_some(op);
        match k {
            kind::DISC => exact(Op::Disc { cx: i16_at(b, 0), cy: i16_at(b, 2), r: u16_at(b, 4), c: col(6)? }),
            kind::RING => exact(Op::Ring { cx: i16_at(b, 0), cy: i16_at(b, 2), outer: u16_at(b, 4), inner: u16_at(b, 6), c: col(8)? }).filter(|o| matches!(o, Op::Ring { outer, inner, .. } if inner < outer)),
            kind::ARC => exact(Op::Arc { cx: i16_at(b, 0), cy: i16_at(b, 2), r: u16_at(b, 4), width: u16_at(b, 6), from: u16_at(b, 8), sweep: u16_at(b, 10), c: col(12)? }),
            kind::CAPSULE => exact(Op::Capsule { ax: i16_at(b, 0), ay: i16_at(b, 2), bx: i16_at(b, 4), by: i16_at(b, 6), r: u16_at(b, 8), c: col(10)? }),
            kind::CURVE => {
                let n = b[28] as usize;
                if !(2..=SAMPLES_MAX).contains(&n) || b.len() != need + 8 * n {
                    return None;
                }
                let c = Curve { x0: i16_at(b, 0), x1: i16_at(b, 2), yb: i16_at(b, 4), yt: i16_at(b, 6), vlo: i32_at(b, 8), vhi: i32_at(b, 12), width: u16_at(b, 16), line: col(18)?, fill: Colour::read(&b[23..28])?, turn: u16_at(b, 29), samples: &b[need..] };
                (c.x0 < c.x1 && c.vlo < c.vhi && c.yb != c.yt).then_some(Op::Curve(c))
            }
            kind::TEXT => {
                let s = core::str::from_utf8(&b[need..]).ok()?;
                (!s.is_empty() && s.len() <= TEXT_MAX).then_some(Op::Text { cx: i16_at(b, 0), cy: i16_at(b, 2), size: u16_at(b, 4), c: col(6)?, s })
            }
            kind::SWEEP => exact(Op::Sweep { cx: i16_at(b, 0), cy: i16_at(b, 2), r: u16_at(b, 4), width: u16_at(b, 6), from: u16_at(b, 8), t0: i64_at(b, 10), period: u32_at(b, 18), c: col(22)? }).filter(|o| matches!(o, Op::Sweep { period, .. } if *period > 0)),
            kind::MARKER => exact(Op::Marker { curve: b[0], t0: i64_at(b, 1), t1: i64_at(b, 9), r: u16_at(b, 17), c: col(19)? }).filter(|o| matches!(o, Op::Marker { t0, t1, .. } if t0 < t1)),
            kind::MOMENT => exact(Op::Moment { cx: i16_at(b, 0), cy: i16_at(b, 2), size: u16_at(b, 4), c: col(6)?, t: i64_at(b, 11), places: b[19] }).filter(|o| matches!(o, Op::Moment { places, .. } if *places <= fmt::PLACES_MAX)),
            kind::NUMBER => exact(Op::Number { cx: i16_at(b, 0), cy: i16_at(b, 2), size: u16_at(b, 4), c: col(6)?, value: i64_at(b, 11), places: b[19], unit: b[20] }).filter(|o| matches!(o, Op::Number { places, unit, .. } if *places <= fmt::PLACES_MAX && (*unit as usize) < fmt::UNITS.len())),
            _ => None,
        }
    }
    /// Whether the op moves on the display's clock (a sweep, a marker) or shows a moment: the display redraws such a list at `draw::next_change`.
    pub fn anchored(&self) -> bool {
        matches!(self, Op::Sweep { .. } | Op::Marker { .. })
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The ops of a list in order, front first; stops at the first op it cannot read (a checked list has none).
pub struct Ops<'a> {
    list: &'a [u8],
    at: usize,
    left: u8,
}

impl<'a> Iterator for Ops<'a> {
    type Item = Op<'a>;
    fn next(&mut self) -> Option<Op<'a>> {
        if self.left == 0 || self.at + 3 > self.list.len() {
            return None;
        }
        let k = self.list[self.at];
        let len = u16_at(self.list, self.at + 1) as usize;
        let body = self.list.get(self.at + 3..self.at + 3 + len)?;
        self.at += 3 + len;
        self.left -= 1;
        Op::decode(k, body)
    }
}

/// A list's ops (see `check` first: an unchecked list may end early).
pub fn ops(list: &[u8]) -> Ops<'_> {
    if list.len() < 2 || list[0] != VERSION {
        return Ops { list, at: 0, left: 0 };
    }
    Ops { list, at: 2, left: list[1] }
}

/// What a checked list holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Checked {
    pub count: u8,
    /// Any op moves on the display's clock (`Op::anchored`).
    pub anchored: bool,
}

/// The display's check of a list a puck sent, before it keeps it: the header, every op's kind, length and values, a marker's curve, and nothing after the last op; refused whole on the first fault.
pub fn check(list: &[u8]) -> Result<Checked, Refused> {
    if list.len() < 2 || list[0] != VERSION {
        return Err(Refused::Header);
    }
    let count = list[1];
    if list.len() > LIST_MAX || count as usize > OPS_MAX {
        return Err(Refused::TooBig);
    }
    let mut at = 2;
    let mut anchored = false;
    let mut curves = 0u64;
    for i in 0..count {
        if at + 3 > list.len() {
            return Err(Refused::Length(i));
        }
        let k = list[at];
        if k >= kind::END {
            return Err(Refused::Kind(i));
        }
        let len = u16_at(list, at + 1) as usize;
        let Some(body) = list.get(at + 3..at + 3 + len) else { return Err(Refused::Length(i)) };
        let Some(op) = Op::decode(k, body) else {
            return Err(if body.len() < fixed_len(k) { Refused::Length(i) } else { Refused::Value(i) });
        };
        match op {
            Op::Curve(_) => curves |= 1 << i,
            Op::Marker { curve, .. } if curve >= i || curves >> curve & 1 == 0 => return Err(Refused::Value(i)),
            _ => {}
        }
        anchored |= op.anchored();
        at += 3 + len;
    }
    if at != list.len() {
        return Err(Refused::Trailing);
    }
    Ok(Checked { count, anchored })
}

/// The curve op at index `i` of a list.
pub fn curve_at(list: &[u8], i: u8) -> Option<Curve<'_>> {
    match ops(list).nth(i as usize)? {
        Op::Curve(c) => Some(c),
        _ => None,
    }
}

/// The list is full: the op did not fit and was not added.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Full;

/// A puck's list, written op by op into the puck's own buffer, front first.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    n: usize,
    count: u8,
}

impl<'a> Writer<'a> {
    /// A list in `buf` (at least `LIST_MAX` bytes is enough for any list).
    pub fn new(buf: &'a mut [u8]) -> Writer<'a> {
        buf[0] = VERSION;
        buf[1] = 0;
        Writer { buf, n: 2, count: 0 }
    }
    /// The ops so far.
    pub fn count(&self) -> u8 {
        self.count
    }
    /// Op `k` with a body of `len` bytes, filled by `f`; its index in the list.
    fn op(&mut self, k: u8, len: usize, f: impl FnOnce(&mut [u8])) -> Result<u8, Full> {
        // WHY both bounds: the list's own limit, and the caller's buffer, which may be smaller than LIST_MAX; past either the op is refused here rather than written out of bounds or sent and refused by the display.
        if self.count as usize >= OPS_MAX || self.n + 3 + len > LIST_MAX.min(self.buf.len()) {
            return Err(Full);
        }
        self.buf[self.n] = k;
        self.buf[self.n + 1..self.n + 3].copy_from_slice(&(len as u16).to_le_bytes());
        f(&mut self.buf[self.n + 3..self.n + 3 + len]);
        self.n += 3 + len;
        self.count += 1;
        self.buf[1] = self.count;
        Ok(self.count - 1)
    }
    pub fn disc(&mut self, cx: i16, cy: i16, r: u16, c: Colour) -> Result<u8, Full> {
        self.op(kind::DISC, 11, |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, r);
            c.put(&mut b[6..]);
        })
    }
    pub fn ring(&mut self, cx: i16, cy: i16, outer: u16, inner: u16, c: Colour) -> Result<u8, Full> {
        self.op(kind::RING, 13, |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, outer);
            put_u16(b, 6, inner);
            c.put(&mut b[8..]);
        })
    }
    pub fn arc(&mut self, cx: i16, cy: i16, r: u16, width: u16, from: u16, sweep: u16, c: Colour) -> Result<u8, Full> {
        self.op(kind::ARC, 17, |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, r);
            put_u16(b, 6, width);
            put_u16(b, 8, from);
            put_u16(b, 10, sweep);
            c.put(&mut b[12..]);
        })
    }
    pub fn capsule(&mut self, a: (i16, i16), b2: (i16, i16), r: u16, c: Colour) -> Result<u8, Full> {
        self.op(kind::CAPSULE, 15, |b| {
            put_i16(b, 0, a.0);
            put_i16(b, 2, a.1);
            put_i16(b, 4, b2.0);
            put_i16(b, 6, b2.1);
            put_u16(b, 8, r);
            c.put(&mut b[10..]);
        })
    }
    /// A curve of `samples` (value, slope per the whole span) from x0 to x1; vlo maps to yb, vhi to yt; a line of `width` (0: none) and the fill to yb (None: none).
    #[allow(clippy::too_many_arguments)]
    pub fn curve(&mut self, x0: i16, x1: i16, yb: i16, yt: i16, vlo: i32, vhi: i32, width: u16, line: Colour, fill: Option<Colour>, turn: u16, samples: &[(i32, i32)]) -> Result<u8, Full> {
        assert!((2..=SAMPLES_MAX).contains(&samples.len()), "a curve takes 2 to SAMPLES_MAX samples");
        self.op(kind::CURVE, 31 + 8 * samples.len(), |b| {
            put_i16(b, 0, x0);
            put_i16(b, 2, x1);
            put_i16(b, 4, yb);
            put_i16(b, 6, yt);
            b[8..12].copy_from_slice(&vlo.to_le_bytes());
            b[12..16].copy_from_slice(&vhi.to_le_bytes());
            put_u16(b, 16, width);
            line.put(&mut b[18..]);
            match fill {
                Some(f) => f.put(&mut b[23..]),
                None => b[23..28].copy_from_slice(&[NONE, 0, 0, 0, 0]),
            }
            b[28] = samples.len() as u8;
            put_u16(b, 29, turn);
            for (k, (v, m)) in samples.iter().enumerate() {
                b[31 + 8 * k..35 + 8 * k].copy_from_slice(&v.to_le_bytes());
                b[35 + 8 * k..39 + 8 * k].copy_from_slice(&m.to_le_bytes());
            }
        })
    }
    /// Words a human reads (at most `TEXT_MAX` bytes; longer is cut at a character boundary).
    pub fn text(&mut self, cx: i16, cy: i16, size: u16, c: Colour, s: &str) -> Result<u8, Full> {
        let mut end = s.len().min(TEXT_MAX);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let s = &s[..end];
        if s.is_empty() {
            return Err(Full);
        }
        self.op(kind::TEXT, 11 + s.len(), |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, size);
            c.put(&mut b[6..]);
            b[11..].copy_from_slice(s.as_bytes());
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn sweep(&mut self, cx: i16, cy: i16, r: u16, width: u16, from: u16, t0: i64, period_s: u32, c: Colour) -> Result<u8, Full> {
        assert!(period_s > 0);
        self.op(kind::SWEEP, 27, |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, r);
            put_u16(b, 6, width);
            put_u16(b, 8, from);
            b[10..18].copy_from_slice(&t0.to_le_bytes());
            b[18..22].copy_from_slice(&period_s.to_le_bytes());
            c.put(&mut b[22..]);
        })
    }
    pub fn marker(&mut self, curve: u8, t0: i64, t1: i64, r: u16, c: Colour) -> Result<u8, Full> {
        self.op(kind::MARKER, 24, |b| {
            b[0] = curve;
            b[1..9].copy_from_slice(&t0.to_le_bytes());
            b[9..17].copy_from_slice(&t1.to_le_bytes());
            put_u16(b, 17, r);
            c.put(&mut b[19..]);
        })
    }
    pub fn moment(&mut self, cx: i16, cy: i16, size: u16, c: Colour, t: i64, places: u8) -> Result<u8, Full> {
        self.op(kind::MOMENT, 20, |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, size);
            c.put(&mut b[6..]);
            b[11..19].copy_from_slice(&t.to_le_bytes());
            b[19] = places;
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn number(&mut self, cx: i16, cy: i16, size: u16, c: Colour, value_q16: i64, places: u8, unit: u8) -> Result<u8, Full> {
        self.op(kind::NUMBER, 21, |b| {
            put_i16(b, 0, cx);
            put_i16(b, 2, cy);
            put_u16(b, 4, size);
            c.put(&mut b[6..]);
            b[11..19].copy_from_slice(&value_q16.to_le_bytes());
            b[19] = places;
            b[20] = unit;
        })
    }
    /// The list as written.
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.n]
    }
}

fn put_i16(b: &mut [u8], at: usize, v: i16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// A turn as Q16 (the ops' angles): `t` turns, any real (wrapped into [0, 1)).
pub fn turn_q16(t: f64) -> u16 {
    (libm::floor((t - libm::floor(t)) * 65536.) as u32 & 0xFFFF) as u16
}

/// A length in the ops' Q14 units from a fraction of the radius.
pub fn q14(f: f32) -> i16 {
    libm::roundf(f * ONE as f32) as i16
}

/// The puck endpoint's calls (the display serves them on `puck`; the kernel puts the caller's kist identity ahead of each, so a puck can only ever name its own circle).
pub mod call {
    /// `[LIST][variant u8][generation u32][list…]` → `[status]`: the puck's list for its resting circle (variant 0) or its circle in focus (variant 1, the fuller face; an empty list there means the resting one).
    pub const LIST: u8 = 0;
    /// `[WATCH][seen u32]` → `[0][count u32][shown u8][focused u8][radius px u32]`: held until the circle's view moves past `seen` (shown on the glass or not, in focus or not, its size), a doorbell, never a poll.
    pub const WATCH: u8 = 1;
    /// LIST's statuses.
    pub const OK: u8 = 0;
    /// The list was refused (`check`); the last good one stays.
    pub const REFUSED: u8 = 1;
    /// No circle on the deck is this puck's (it is not stowed here, or the deck has not bound it yet): kept for when it is.
    pub const NOT_ON_DECK: u8 = 2;
    /// The variant for the circle at rest and in focus.
    pub const REST: u8 = 0;
    pub const FOCUS: u8 = 1;
}
