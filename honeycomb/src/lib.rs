//! The honeycomb keyboard (Claude memories/input_rhe_on_glass.md): a flush honeycomb of rows alternating 9 and 10 keys, an extent of rows stacked upward over the letters (digits, then symbols), and a fixed bar under it (map, shift, space, del, enter). Pure: it draws into a 64-bit surface (R, G, B working values in bits 0-15, 16-31, 32-47, gamma 2, white = Q14) and hands each label to the host, which owns the display, the touches and the text. Hive (payloads/hive) and the attest screen (std/ahau) host it.
//!
//! Drawing without a square root per pixel: the hexes are pointy-topped, so their sides are vertical and their caps are 30° from horizontal. Each is drawn column by column: a column's top and bottom are straight-line crossings (one multiply per column), the span between them is filled flat, and only the two end pixels (and the side columns) are partly covered. A partly covered pixel gets the key's value times the square root of its coverage, because the surface holds gamma-2 working values and light adds linearly (the integer root of a 16-bit coverage is the 8-bit factor). The keys never overlap, so nothing is blended and nothing needs a clamp: the layout keeps every key inside the screen.
//!
//! Hit testing: a point belongs to the nearest key centre (the honeycomb is exactly the nearest-centre map of its centres, the gaps included). The point's row and the row below are the only candidates, and on each row the nearest column is one division; the closer of the two by squared distance wins. A key at a row's end owns the margin beyond it.
//!
//! The extent scrolls in whole rows (no animation, owner 2026-10-22): two fingers drag it 1:1, a row per row pitch, and the page key on the symbol row moves it a page (four rows) at a time, back to the letters after the last. The whole keyboard sits inside the circle an app lives in (owner, 2026-10-27): the bar's bottom is where the circle's chord first spans the screen, so the corners stay the system's.
#![no_std]

/// White, as a working value (gamma 2, Q14).
pub const WHITE: u64 = 16384;
/// A key at rest, lit by a finger, and the bar's keys at rest (working values: about 2.5 %, 18 % and 1.2 % of white's light).
pub const KEY: u64 = 2600;
pub const LIT: u64 = 7000;
pub const BAR_KEY: u64 = 1800;
pub const LABEL: u16 = 13000;
/// 1/√3 and 2/√3 in Q16: a pointy-topped hex's cap slope, and its circumradius over its half width. √3/2 in Q16: the row pitch over the column pitch.
const INV_SQRT3: i64 = 37837;
const TWO_OVER_SQRT3: i64 = 75674;
const HALF_SQRT3: i64 = 56756;

/// The page key's cell.
pub const PAGE: char = '\u{0}';
/// The extent, bottom row first, alternating 9 and 10 keys so the honeycomb stays flush at every scroll: the letters (QWERTY with Q moved to the bottom row, owner 2026-10-27), the digits from 0, then the symbols. On the symbol row the commonest sit at the ends, where a thumb reaches easiest, with the page key between them (owner: the most common symbols on the symbol row, placed by reach).
pub const EXTENT: [&[char]; 8] = [
    &['z', 'x', 'c', 'v', 'b', 'n', 'm', 'q', ','],
    &['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', '.'],
    &['w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'],
    &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'],
    &['\'', '?', '-', '(', PAGE, ')', '/', '!', '"'],
    &[':', ';', '@', '#', '&', '*', '_', '+', '=', '%'],
    &['$', '~', '`', '^', '<', '>', '[', ']', '|'],
    &['{', '}', '\\', '€', '£', '¥', '°', '§', '•', '…'],
];
/// Rows on show at once.
pub const VIS: usize = 4;
/// The highest view: the extent's top row at the top.
pub const MAX_VIEW: usize = EXTENT.len() - VIS;
/// The bar's keys and their right edges in half columns (the bar is 20 half columns wide).
pub const BAR_KEYS: [&str; 5] = ["map", "shift", "space", "del", "enter"];
const BAR_EDGE: [i64; 5] = [3, 6, 14, 17, 20];
pub const B_MAP: usize = 0;
pub const B_SHIFT: usize = 1;
pub const B_SPACE: usize = 2;
pub const B_DEL: usize = 3;
pub const B_ENTER: usize = 4;

/// A key: a hex on show (numbered top row first), a bar key, or none (above the keyboard).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Hex(usize),
    Bar(usize),
    None,
}

/// No key, as a constant for hosts that keep a key per finger.
pub const NONE_KEY: Key = Key::None;

/// What a tap did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// A character typed.
    Char(char),
    /// Delete the last character.
    Back,
    Enter,
    /// The bar's left key (`Keyboard::left` names it: map in Hive, where the host leaves; show in the attest screen).
    Leave,
    /// The keys changed (shift, a page): the host redraws the keyboard.
    Redraw,
}

/// The keyboard: its geometry (Q8 pixels unless named) and its state.
pub struct Keyboard {
    w: usize,
    /// The column pitch and the row pitch.
    px: i64,
    py: i64,
    /// The top visible row's centre line.
    cy0: i64,
    /// A drawn key's half width and circumradius (the cell's, less half the gap).
    a: i64,
    r: i64,
    gap: i64,
    /// The bar's top and bottom.
    bar0: i64,
    bar1: i64,
    /// The keyboard's top edge, in pixels: everything from here down is the keyboard's.
    pub top: usize,
    /// The extent row at the bottom of the view.
    pub view: usize,
    pub shift: bool,
    /// The bar's left key's label (the host decides what it does: `Event::Leave`).
    pub left: &'static str,
}

/// The rows of the circle's corners: how far the circle (tangent to the screen's top and bottom) stays narrower than the screen at the bottom.
pub fn corner_rows(w: usize, h: usize) -> usize {
    let (cx, r) = (w / 2, h / 2);
    r - (r * r - cx * cx).isqrt() + 1
}

impl Keyboard {
    /// A keyboard for a `w` × `h` screen: ten columns across, the bar on the circle's full-width line, the honeycomb over it.
    pub fn new(w: usize, h: usize) -> Keyboard {
        let px = ((w as i64) << 8) / 10;
        let py = px * HALF_SQRT3 >> 16;
        let cell_r = px * INV_SQRT3 >> 16;
        let gap = px >> 4;
        let bar1 = ((h - corner_rows(w, h)) as i64) << 8;
        let bar0 = bar1 - py;
        let cy3 = bar0 - cell_r;
        let cy0 = cy3 - (VIS as i64 - 1) * py;
        let a = px / 2 - gap / 2;
        Keyboard { w, px, py, cy0, a, r: a * TWO_OVER_SQRT3 >> 16, gap, bar0, bar1, top: ((cy0 - cell_r) >> 8) as usize, view: 0, shift: false, left: BAR_KEYS[B_MAP] }
    }

    /// The row pitch in pixels: a two-finger drag of this far scrolls one row.
    pub fn row_pitch(&self) -> i64 {
        self.py >> 8
    }

    /// Visible row `row` (0 at the top): its extent row.
    fn extent_row(&self, row: usize) -> &'static [char] {
        EXTENT[self.view + VIS - 1 - row]
    }

    /// Hex `k` on show: its row and column.
    fn place(&self, k: usize) -> (usize, usize) {
        let mut start = 0;
        for row in 0..VIS {
            let n = self.extent_row(row).len();
            if k < start + n {
                return (row, k - start);
            }
            start += n;
        }
        panic!("hex {k} is not on show")
    }

    /// How many hexes are on show.
    pub fn hexes(&self) -> usize {
        (0..VIS).map(|r| self.extent_row(r).len()).sum()
    }

    fn centre(&self, row: usize, col: usize) -> (i64, i64) {
        let n = self.extent_row(row).len() as i64;
        ((10 - n) * self.px / 2 + self.px / 2 + col as i64 * self.px, self.cy0 + row as i64 * self.py)
    }

    /// The character on hex `k`, as typed now (shift capitalises letters).
    pub fn cell(&self, k: usize) -> char {
        let (row, col) = self.place(k);
        let c = self.extent_row(row)[col];
        if self.shift { c.to_uppercase().next().unwrap_or(c) } else { c }
    }

    /// Bar key `b`'s left and right edges, drawn (inset by half the gap).
    fn bar_x(&self, b: usize) -> (i64, i64) {
        let left = if b == 0 { 0 } else { BAR_EDGE[b - 1] * self.px / 2 };
        (left + self.gap / 2, BAR_EDGE[b] * self.px / 2 - self.gap / 2)
    }

    /// The key under a finger at pixel (x, y).
    pub fn hit(&self, x: i64, y: i64) -> Key {
        let (fx, fy) = ((x << 8) + 128, (y << 8) + 128);
        if fy >= self.bar1 {
            return Key::None;
        }
        if fy >= self.bar0 {
            let half = fx * 2 / self.px;
            return Key::Bar(BAR_EDGE.iter().position(|&e| half < e).unwrap_or(BAR_KEYS.len() - 1));
        }
        if y < self.top as i64 {
            return Key::None;
        }
        let rf = (fy - self.cy0).div_euclid(self.py);
        let mut best = (i64::MAX, Key::None);
        let mut start = 0;
        for row in 0..VIS as i64 {
            let n = self.extent_row(row as usize).len() as i64;
            if row == rf || row == rf + 1 {
                let ox = (10 - n) * self.px / 2;
                // The nearest column on this row; a row's end key owns the margin beyond it.
                let c = (fx - ox).div_euclid(self.px);
                let c = if c < 0 { 0 } else if c >= n { n - 1 } else { c };
                let (dx, dy) = (fx - (ox + self.px / 2 + c * self.px), fy - (self.cy0 + row * self.py));
                let d = dx * dx + dy * dy;
                if d < best.0 {
                    best = (d, Key::Hex(start + c as usize));
                }
            }
            start += n as usize;
        }
        best.1
    }

    /// A tap on `key`: what it typed, and the keyboard's own changes (shift, the page).
    pub fn tap(&mut self, key: Key) -> Option<Event> {
        match key {
            Key::Hex(k) => {
                let c = self.cell(k);
                if c == PAGE {
                    // The next page up the extent, and from the top back to the letters.
                    self.view = if self.view == MAX_VIEW { 0 } else { (self.view + VIS).min(MAX_VIEW) };
                    return Some(Event::Redraw);
                }
                if self.shift && c.is_alphabetic() {
                    self.shift = false;
                }
                Some(Event::Char(c))
            }
            Key::Bar(B_MAP) => Some(Event::Leave),
            Key::Bar(B_SHIFT) => {
                self.shift = !self.shift;
                Some(Event::Redraw)
            }
            Key::Bar(B_SPACE) => Some(Event::Char(' ')),
            Key::Bar(B_DEL) => Some(Event::Back),
            Key::Bar(B_ENTER) => Some(Event::Enter),
            _ => None,
        }
    }

    /// Scroll to `view` (a two-finger drag: the host passes where the drag started plus whole row pitches moved). True when the view changed. The drag is a finger's travel, so it can run past either end of the extent; the view stops there.
    pub fn scroll_to(&mut self, view: i64) -> bool {
        let v = if view < 0 { 0 } else if view > MAX_VIEW as i64 { MAX_VIEW } else { view as usize };
        let changed = v != self.view;
        self.view = v;
        changed
    }

    /// The whole keyboard: its area cleared to black (the surface's rows from `top` to the bar's bottom), every key, every label through `label(x, baseline, size px, value, text)`.
    pub fn draw(&self, s: &mut [u64], label: &mut dyn FnMut(i64, i64, u16, u16, &str)) {
        s[self.top * self.w..((self.bar1 >> 8) as usize) * self.w].fill(0);
        for k in 0..self.hexes() {
            self.draw_key(s, Key::Hex(k), false, label);
        }
        for b in 0..BAR_KEYS.len() {
            self.draw_key(s, Key::Bar(b), false, label);
        }
    }

    /// One key, lit or at rest, with its label.
    pub fn draw_key(&self, s: &mut [u64], key: Key, lit: bool, label: &mut dyn FnMut(i64, i64, u16, u16, &str)) {
        let size = (self.px * 107 >> 16) as u16;
        match key {
            Key::Hex(k) => {
                let (row, col) = self.place(k);
                let (cx, cy) = self.centre(row, col);
                let (a, r) = (self.a, self.r);
                shape(s, self.w, cx - a, cx + a, if lit { LIT } else { KEY }, |fx| {
                    let t = (fx - cx).abs() * INV_SQRT3 >> 16;
                    (cy - r + t, cy + r - t)
                });
                let c = self.cell(k);
                let mut b = [0u8; 4];
                let (t, size) = if c == PAGE { ("page", size * 5 / 8) } else { (&*c.encode_utf8(&mut b), size) };
                centred(cx, cy, size, t, label);
            }
            Key::Bar(b) => {
                let on = lit || (b == B_SHIFT && self.shift);
                let (x0, x1) = self.bar_x(b);
                let (y0, y1) = (self.bar0 + self.gap / 2, self.bar1 - self.gap / 2);
                shape(s, self.w, x0, x1, if on { LIT } else { BAR_KEY }, |_| (y0, y1));
                centred((x0 + x1) / 2, (self.bar0 + self.bar1) / 2, size * 5 / 7, if b == B_MAP { self.left } else { BAR_KEYS[b] }, label);
            }
            Key::None => {}
        }
    }
}

/// What the fingers did to the keyboard in one poll.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Touch {
    /// A finger landed on this key: draw it lit.
    Light(Key),
    /// The finger left the key or moved off as a drag: draw it at rest.
    Unlight(Key),
    /// A finger lifted from this key without moving and outside a scroll: a tap (pass it to `Keyboard::tap`).
    Tap(Key),
    /// A two-finger drag moved the view: redraw the keyboard.
    Scrolled,
}

/// Fingers on the keyboard, each on its own (owner, 2026-10-27): a second finger landing before the first lifts is two taps, not a gesture; a key is lit while a finger is on it and is a tap when that finger lifts without moving past the slop; two of the host's fingers moving together scroll the extent a row per row pitch. A finger that moves alone does nothing yet: shape writing goes there.
pub struct Touches {
    /// Per contact id: down, where it landed, the key under it, moved past the slop, the host's (not another window's).
    f: [(bool, i64, i64, Key, bool, bool); 16],
    /// A scroll: the view and the fingers' mean height when it began.
    scroll: Option<(usize, i64)>,
    slop: i64,
}

impl Touches {
    /// For a screen `w` pixels wide: the slop is a 56th of it (the map's 24 pixels on this panel).
    pub fn new(w: usize) -> Touches {
        Touches { f: [(false, 0, 0, Key::None, false, false); 16], scroll: None, slop: (w / 56) as i64 }
    }

    /// One poll: `held` is every contact down now (id, x, y). `mine` is asked once per contact as it lands (is the point the host's window's?). Each change goes to `out`.
    pub fn poll(&mut self, kb: &mut Keyboard, held: &[(u8, i64, i64)], mine: &mut dyn FnMut(i64, i64) -> bool, out: &mut dyn FnMut(Touch)) {
        let mut here = 0u16;
        let (mut sum_y, mut ours) = (0i64, 0i64);
        for &(id, x, y) in held {
            let id = (id & 15) as usize;
            here |= 1 << id;
            let f = &mut self.f[id];
            if !f.0 {
                let m = mine(x, y);
                *f = (true, x, y, if m { kb.hit(x, y) } else { Key::None }, false, m);
                if f.3 != Key::None {
                    out(Touch::Light(f.3));
                }
            } else if !f.4 && (x - f.1).pow(2) + (y - f.2).pow(2) > self.slop * self.slop {
                f.4 = true;
                if f.3 != Key::None {
                    out(Touch::Unlight(f.3));
                }
            }
            if f.5 {
                sum_y += y;
                ours += 1;
            }
        }
        // Two of the host's fingers: a scroll from where they stood when the second landed; dragging down shows the rows above.
        if ours == 2 {
            let mean = sum_y / 2;
            match self.scroll {
                None => self.scroll = Some((kb.view, mean)),
                Some((v0, y0)) => {
                    if kb.scroll_to(v0 as i64 + (mean - y0) / kb.row_pitch()) {
                        out(Touch::Scrolled);
                    }
                }
            }
        }
        for id in 0..16 {
            let f = self.f[id];
            if f.0 && here >> id & 1 == 0 {
                self.f[id].0 = false;
                if f.3 != Key::None && !f.4 {
                    out(Touch::Unlight(f.3));
                    if self.scroll.is_none() {
                        out(Touch::Tap(f.3));
                    }
                }
            }
        }
        if self.f.iter().all(|f| !f.0) {
            self.scroll = None;
        }
    }
}

/// Text centred on (cx, cy) in Q8, by an estimate of the typeface's advance (0.56 em) and cap height (0.7 em): the display does not say how wide a string is yet.
fn centred(cx: i64, cy: i64, px: u16, s: &str, label: &mut dyn FnMut(i64, i64, u16, u16, &str)) {
    let (cx, cy, p) = (cx >> 8, cy >> 8, px as i64);
    label(cx - s.chars().count() as i64 * p * 28 / 100, cy + p * 35 / 100, px, LABEL, s);
}

fn grey(v: u64) -> u64 {
    v | v << 16 | v << 32
}

/// One column of a shape: pixel column `x`, covered from `top` to `bot` (Q8), `cov_x` of its width (0 to 256). The ends get the value times √coverage; the span between them the column's value flat.
fn column(s: &mut [u64], w: usize, x: usize, top: i64, bot: i64, cov_x: u32, v: u64) {
    let (yt, yb) = ((top >> 8) as usize, ((bot - 1) >> 8) as usize);
    let (ct, cb) = (256 - (top & 255) as u32, ((bot - 1) & 255) as u32 + 1);
    let shade = |c: u32| grey(v * (c * cov_x).isqrt() as u64 >> 8);
    let flat = grey(v * (cov_x << 8).isqrt() as u64 >> 8);
    s[yt * w + x] = shade(ct);
    for y in yt + 1..yb {
        s[y * w + x] = flat;
    }
    s[yb * w + x] = shade(cb);
}

/// A shape `left` to `right` (Q8) wide, each pixel column's top and bottom from `span` at the column's centre.
fn shape(s: &mut [u64], w: usize, left: i64, right: i64, v: u64, span: impl Fn(i64) -> (i64, i64)) {
    let (xl, xr) = ((left >> 8) as usize, ((right - 1) >> 8) as usize);
    for x in xl..=xr {
        let cov_x = if x == xl {
            256 - (left & 255) as u32
        } else if x == xr {
            ((right - 1) & 255) as u32 + 1
        } else {
            256
        };
        let (top, bot) = span(((x as i64) << 8) + 128);
        column(s, w, x, top, bot, cov_x, v);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// Every lit pixel's nearest key is the key that drew it, at every view.
    #[test]
    fn drawing_matches_hit_testing() {
        let (w, h) = (1344usize, 2992usize);
        let mut kb = Keyboard::new(w, h);
        let mut none = |_: i64, _: i64, _: u16, _: u16, _: &str| {};
        for view in 0..=MAX_VIEW {
            kb.view = view;
            for k in 0..kb.hexes() + BAR_KEYS.len() {
                let key = if k < kb.hexes() { Key::Hex(k) } else { Key::Bar(k - kb.hexes()) };
                let mut one = std::vec![0u64; w * h];
                kb.draw_key(&mut one, key, false, &mut none);
                for (i, &p) in one.iter().enumerate() {
                    if p != 0 {
                        assert_eq!(kb.hit((i % w) as i64, (i / w) as i64), key, "view {view}");
                    }
                }
            }
        }
    }

    /// The bar sits inside the circle: its corners are inside the circle tangent to the screen's top and bottom.
    #[test]
    fn bar_inside_the_circle() {
        let (w, h) = (1344usize, 2992usize);
        let kb = Keyboard::new(w, h);
        let (cx, cy, r) = (w as i64 / 2, h as i64 / 2, h as i64 / 2);
        let y = (kb.bar1 >> 8) - 1;
        for x in [0i64, w as i64 - 1] {
            assert!((x - cx).pow(2) + (y - cy).pow(2) <= r * r);
        }
    }

    #[test]
    fn page_cycles() {
        let mut kb = Keyboard::new(1344, 2992);
        kb.view = 1;
        let page = (0..kb.hexes()).find(|&k| kb.cell(k) == PAGE).unwrap();
        assert_eq!(kb.tap(Key::Hex(page)), Some(Event::Redraw));
        assert_eq!(kb.view, MAX_VIEW);
        let page = (0..kb.hexes()).find(|&k| kb.cell(k) == PAGE).unwrap();
        kb.tap(Key::Hex(page));
        assert_eq!(kb.view, 0);
    }
}
