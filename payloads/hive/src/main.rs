//! Hive (a working name): the honeycomb keyboard's first cut, an app like Ramps (SHELL.md: apps are places). It shows a flush 10-9-10-9 honeycomb at the bottom of the screen, the owner's layout of 2026-10-27, with a fixed control bar under it. A tap types, and the typed line shows above the keys and goes to the console (Claude memories/input_rhe_on_glass.md has the design).
//!
//! The rows, top to bottom: the digits from 0, then `W E R T Y U I O P`, `A S D F G H J K L .`, `Z X C V B N M Q ,` (QWERTY with Q moved to the bottom row so the 9-cell rows are flush with the 10-cell ones). The bar: map (back to the map), shift (the next letter as a capital), space, del, enter.
//!
//! Drawing without a square root per pixel: the hexes are pointy-topped, so their sides are vertical and their caps are 30° from horizontal. Each is drawn column by column: a column's top and bottom are straight-line crossings (one multiply per column), the span between them is filled flat, and only the two end pixels (and the side columns) are partly covered. A partly covered pixel gets the key's value times the square root of its coverage, because the surface holds gamma-2 working values and light adds linearly (the integer root of a 16-bit coverage is the 8-bit factor). The keys never overlap, so nothing is blended and nothing needs a clamp: the layout keeps every key inside the screen.
//!
//! Hit testing: a point belongs to the nearest key centre (the honeycomb is exactly the nearest-centre map of its centres, the gaps included). The point's row and the row below are the only candidates, and on each row the nearest column is found by one division; the closer of the two by squared distance wins. A key at a row's end owns the margin beyond it.
//!
//! Each finger is tracked on its own, so a second finger landing before the first lifts (fast typing) is two taps, not a gesture. A key is lit while a finger is on it and types when that finger lifts without moving past the slop. A finger that moves types nothing yet: that is where shape writing goes.
//!
//! Launched with `ferros-bridge spawn hive.bin --name hive --use display --use touch --dma app-surface`. Grant slots: 0 console, 1 display (use), 2 touch (use), 3 app-surface.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit, freq, now, sleep_until};

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SURF: u64 = 3;
/// White, as a working value (gamma 2, Q14).
const WHITE: u64 = 16384;
/// A key at rest, lit by a finger, and the bar's keys at rest (working values: about 2.5 %, 18 % and 1.2 % of white's light).
const KEY: u64 = 2600;
const LIT: u64 = 7000;
const BAR_KEY: u64 = 1800;
const LABEL: u16 = 13000;
/// The space between neighbouring keys, in pixels.
const GAP: i64 = 8;
/// The bar's height and the margin under it, in pixels (the margin keeps the bar off the glass's edge).
const BAR: i64 = 128;
const MARGIN: i64 = 64;
/// A finger that moves further than this from where it landed is not a tap (the map's slop).
const SLOP: i64 = 24;
/// 1/√3 and 2/√3 in Q16: a pointy-topped hex's cap slope, and its circumradius over its half width. √3/2 in Q16: the row pitch over the column pitch.
const INV_SQRT3: i64 = 37837;
const TWO_OVER_SQRT3: i64 = 75674;
const HALF_SQRT3: i64 = 56756;

const ROWS: [&[u8]; 4] = [b"0123456789", b"WERTYUIOP", b"ASDFGHJKL.", b"ZXCVBNMQ,"];
/// Where each row's keys start in the row-major key number.
const ROW_START: [usize; 4] = [0, 10, 19, 29];
const HEXES: usize = 38;
/// The bar's keys and their right edges in half columns (the bar is 20 half columns wide).
const BAR_KEYS: [&str; 5] = ["map", "shift", "space", "del", "enter"];
const BAR_EDGE: [i64; 5] = [3, 6, 14, 17, 20];
const B_MAP: usize = 0;
const B_SHIFT: usize = 1;
const B_SPACE: usize = 2;
const B_DEL: usize = 3;
const B_ENTER: usize = 4;
/// No key (the text area).
const NONE: usize = usize::MAX;

/// The keyboard's geometry, everything in Q8 pixels except where named.
struct Layout {
    w: usize,
    /// The column pitch and the row pitch.
    px: i64,
    py: i64,
    /// Row 0's centre line.
    cy0: i64,
    /// A drawn key's half width and circumradius (the cell's, less half the gap).
    a: i64,
    r: i64,
    /// The bar's top and bottom.
    bar0: i64,
    bar1: i64,
    /// The honeycomb's top edge, in pixels.
    top: i64,
}

impl Layout {
    fn new(w: usize, h: usize) -> Layout {
        let px = ((w as i64) << 8) / 10;
        let py = px * HALF_SQRT3 >> 16;
        let cell_r = px * INV_SQRT3 >> 16;
        let bar1 = ((h as i64) - MARGIN) << 8;
        let bar0 = bar1 - (BAR << 8);
        let cy3 = bar0 - cell_r;
        let cy0 = cy3 - 3 * py;
        let a = px / 2 - (GAP << 8) / 2;
        Layout { w, px, py, cy0, a, r: a * TWO_OVER_SQRT3 >> 16, bar0, bar1, top: (cy0 - cell_r) >> 8 }
    }

    /// Key `k`'s centre (row-major over the honeycomb).
    fn centre(&self, k: usize) -> (i64, i64) {
        let row = (k >= ROW_START[1]) as usize + (k >= ROW_START[2]) as usize + (k >= ROW_START[3]) as usize;
        let n = ROWS[row].len() as i64;
        let c = (k - ROW_START[row]) as i64;
        ((10 - n) * self.px / 2 + self.px / 2 + c * self.px, self.cy0 + row as i64 * self.py)
    }

    /// Bar key `b`'s left and right edges, drawn (inset by half the gap).
    fn bar_x(&self, b: usize) -> (i64, i64) {
        let left = if b == 0 { 0 } else { BAR_EDGE[b - 1] * self.px / 2 };
        (left + (GAP << 8) / 2, BAR_EDGE[b] * self.px / 2 - (GAP << 8) / 2)
    }

    /// The key under a finger at pixel (x, y): a hex number, 64 + a bar key, or NONE.
    fn hit(&self, x: i64, y: i64) -> usize {
        let (fx, fy) = ((x << 8) + 128, (y << 8) + 128);
        if fy >= self.bar0 {
            let half = fx * 2 / self.px;
            return 64 + BAR_EDGE.iter().position(|&e| half < e).unwrap_or(BAR_KEYS.len() - 1);
        }
        if y < self.top {
            return NONE;
        }
        let rf = (fy - self.cy0).div_euclid(self.py);
        let mut best = (i64::MAX, NONE);
        for row in [rf, rf + 1] {
            if row < 0 || row > 3 {
                continue;
            }
            let n = ROWS[row as usize].len() as i64;
            let ox = (10 - n) * self.px / 2;
            // The nearest column on this row; a row's end key owns the margin beyond it.
            let c = (fx - ox).div_euclid(self.px);
            let c = if c < 0 { 0 } else if c >= n { n - 1 } else { c };
            let (dx, dy) = (fx - (ox + self.px / 2 + c * self.px), fy - (self.cy0 + row * self.py));
            let d = dx * dx + dy * dy;
            if d < best.0 {
                best = (d, ROW_START[row as usize] + c as usize);
            }
        }
        best.1
    }
}

fn grey(v: u64) -> u64 {
    v | v << 16 | v << 32
}

/// One column of a shape: pixel column `x`, covered from `top` to `bot` (Q8), `cov_x` of its width (0 to 256). The ends get the value times √coverage; the span between them the column's value flat.
fn column(surf: &mut [u64], w: usize, x: usize, top: i64, bot: i64, cov_x: u32, v: u64) {
    let (yt, yb) = ((top >> 8) as usize, ((bot - 1) >> 8) as usize);
    let (ct, cb) = (256 - (top & 255) as u32, ((bot - 1) & 255) as u32 + 1);
    let shade = |c: u32| grey(v * (c * cov_x).isqrt() as u64 >> 8);
    let flat = grey(v * (cov_x << 8).isqrt() as u64 >> 8);
    surf[yt * w + x] = shade(ct);
    for y in yt + 1..yb {
        surf[y * w + x] = flat;
    }
    surf[yb * w + x] = shade(cb);
}

/// A shape `left` to `right` (Q8) wide, each pixel column's top and bottom from `span` at the column's centre.
fn shape(surf: &mut [u64], w: usize, left: i64, right: i64, v: u64, span: impl Fn(i64) -> (i64, i64)) {
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
        column(surf, w, x, top, bot, cov_x, v);
    }
}

fn hex(surf: &mut [u64], l: &Layout, k: usize, v: u64) {
    let (cx, cy) = l.centre(k);
    let (a, r) = (l.a, l.r);
    shape(surf, l.w, cx - a, cx + a, v, |fx| {
        let t = (fx - cx).abs() * INV_SQRT3 >> 16;
        (cy - r + t, cy + r - t)
    });
}

fn bar_key(surf: &mut [u64], l: &Layout, b: usize, v: u64) {
    let (x0, x1) = l.bar_x(b);
    let (y0, y1) = (l.bar0 + (GAP << 8) / 2, l.bar1 - (GAP << 8) / 2);
    shape(surf, l.w, x0, x1, v, |_| (y0, y1));
}

/// A line of text into the surface (display op 26): screen coordinates, size in pixels, grey value Q14.
fn label(display: u64, x: i64, baseline: i64, px: u16, value: u16, s: &[u8]) {
    let mut m = [0u8; 160];
    m[0] = 26;
    m[1..3].copy_from_slice(&(x as i16).to_le_bytes());
    m[3..5].copy_from_slice(&(baseline as i16).to_le_bytes());
    m[5..7].copy_from_slice(&px.to_le_bytes());
    m[7..9].copy_from_slice(&value.to_le_bytes());
    let n = s.len().min(m.len() - 9);
    m[9..9 + n].copy_from_slice(&s[..n]);
    let mut r = [0u8; 4];
    let t = now();
    call(display, &m[..9 + n], &mut r);
    unsafe { LABEL_US += ferros_sys::us_since(t) };
}

/// The time spent in the display's text op, for the console's timing lines.
static mut LABEL_US: u64 = 0;

/// Text centred on (cx, cy) in Q8, by an estimate of the typeface's advance (0.56 em) and cap height (0.7 em): the display does not say how wide a string is yet.
fn centred(display: u64, cx: i64, cy: i64, px: u16, value: u16, s: &[u8]) {
    let (cx, cy, p) = (cx >> 8, cy >> 8, px as i64);
    label(display, cx - s.len() as i64 * p * 28 / 100, cy + p * 35 / 100, px, value, s);
}

struct Hive {
    display: u64,
    l: Layout,
    h: usize,
    shift: bool,
    text: [u8; 128],
    len: usize,
}

impl Hive {
    fn surf(&self) -> &'static mut [u64] {
        unsafe { core::slice::from_raw_parts_mut(SURF as *mut u64, self.l.w * self.h) }
    }

    /// Key `k` (a hex number, or 64 + a bar key) drawn lit or at rest, with its label.
    fn key(&self, k: usize, lit: bool) {
        let s = self.surf();
        if k < HEXES {
            hex(s, &self.l, k, if lit { LIT } else { KEY });
            let (cx, cy) = self.l.centre(k);
            let row = (k >= ROW_START[1]) as usize + (k >= ROW_START[2]) as usize + (k >= ROW_START[3]) as usize;
            let ch = ROWS[row][k - ROW_START[row]];
            centred(self.display, cx, cy, 56, LABEL, &[ch]);
        } else {
            let b = k - 64;
            let on = lit || (b == B_SHIFT && self.shift);
            bar_key(s, &self.l, b, if on { LIT } else { BAR_KEY });
            let (x0, x1) = self.l.bar_x(b);
            centred(self.display, (x0 + x1) / 2, (self.l.bar0 + self.l.bar1) / 2, 40, LABEL, BAR_KEYS[b].as_bytes());
        }
    }

    /// The typed line's band above the keys: cleared, then the line's tail and a cursor.
    fn line(&self) {
        let s = self.surf();
        let w = self.l.w;
        let base = self.l.top - 96;
        for y in (base - 96) as usize..(base + 40) as usize {
            s[y * w..(y + 1) * w].fill(0);
        }
        let mut t = [0u8; 33];
        let from = self.len - self.len.min(32);
        let n = self.len - from;
        t[..n].copy_from_slice(&self.text[from..self.len]);
        t[n] = b'_';
        label(self.display, 48, base, 64, WHITE as u16, &t[..n + 1]);
    }

    fn all(&self, grants: u64) {
        let t = now();
        self.surf().fill(0);
        let cleared = ferros_sys::us_since(t);
        for k in 0..HEXES {
            self.key(k, false);
        }
        for b in 0..BAR_KEYS.len() {
            self.key(64 + b, false);
        }
        label(self.display, 48, 200, 36, 5000, b"hive: the honeycomb keyboard, first cut (map: back)");
        self.line();
        let mut l = Line::new();
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!("hive: cleared in {cleared} us; text {} us of {} us\n", unsafe { LABEL_US }, ferros_sys::us_since(t)));
        console(grants, l.as_bytes());
    }

    fn show(&self) -> bool {
        let mut m = [0u8; 9];
        m[0] = 22;
        m[5..7].copy_from_slice(&(self.l.w as u16).to_le_bytes());
        m[7..9].copy_from_slice(&(self.h as u16).to_le_bytes());
        let mut r = [0u8; 4];
        call(self.display, &m, &mut r);
        r[0] == 0
    }

    fn push(&mut self, c: u8) {
        if self.len == self.text.len() {
            self.text.copy_within(1.., 0);
            self.len -= 1;
        }
        self.text[self.len] = c;
        self.len += 1;
    }

    /// A tap on key `k` typed.
    fn tap(&mut self, grants: u64, k: usize) {
        if k < HEXES {
            let row = (k >= ROW_START[1]) as usize + (k >= ROW_START[2]) as usize + (k >= ROW_START[3]) as usize;
            let ch = ROWS[row][k - ROW_START[row]];
            let ch = if ch.is_ascii_uppercase() && !self.shift { ch.to_ascii_lowercase() } else { ch };
            if ch.is_ascii_alphabetic() && self.shift {
                self.shift = false;
                self.key(64 + B_SHIFT, false);
            }
            self.push(ch);
        } else {
            match k - 64 {
                B_MAP => {
                    let mut q = [0u8; 4];
                    call(self.display, &[23], &mut q);
                    console(grants, b"hive: back to the map\n");
                    exit(0);
                }
                B_SHIFT => {
                    self.shift = !self.shift;
                    self.key(64 + B_SHIFT, false);
                }
                B_SPACE => self.push(b' '),
                B_DEL => self.len -= (self.len > 0) as usize,
                B_ENTER => {
                    let mut m = [0u8; 160];
                    let head = b"hive: entered: ";
                    m[..head.len()].copy_from_slice(head);
                    m[head.len()..head.len() + self.len].copy_from_slice(&self.text[..self.len]);
                    m[head.len() + self.len] = b'\n';
                    console(grants, &m[..head.len() + self.len + 1]);
                    self.len = 0;
                }
                _ => {}
            }
        }
        self.line();
    }
}

static mut SURF: u64 = 0;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let (display, touch) = (cap(grants, S_DISPLAY), cap(grants, S_TOUCH));
    let mut r = [0u8; 256];
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"hive: no display server\n");
        exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as usize, u16::from_le_bytes([r[2], r[3]]) as usize);
    let base = ferros_sys::dma(cap(grants, S_SURF));
    if base == 0 || base == u64::MAX || w * h * 8 > 32 << 20 {
        console(grants, b"hive: no app surface\n");
        exit(2);
    }
    unsafe { SURF = base };
    // Spawned with any input: the text op's cost, empty strings against one glyph at three sizes, 32 calls each (a development measurement).
    if _input_len > 0 {
        for (px, txt) in [(56u16, &b""[..]), (56, b"W"), (24, b"W"), (112, b"W"), (56, b"WWWWWWWW")] {
            unsafe { LABEL_US = 0 };
            for k in 0..32 {
                label(display, 100 + k * 30, 600, px, 8000, txt);
            }
            let mut l = Line::new();
            let _ = core::fmt::Write::write_fmt(&mut l, format_args!("hive: 32 text calls at {px} px, {} bytes: {} us\n", txt.len(), unsafe { LABEL_US }));
            console(grants, l.as_bytes());
        }
        for (name, ep, op) in [("display info", display, 5u8), ("touch held", touch, 1)] {
            let t = now();
            for _ in 0..32 {
                call(ep, &[op], &mut r);
            }
            let mut l = Line::new();
            let _ = core::fmt::Write::write_fmt(&mut l, format_args!("hive: 32 calls of {name}: {} us\n", ferros_sys::us_since(t)));
            console(grants, l.as_bytes());
        }
        exit(0);
    }
    let mut hive = Hive { display, l: Layout::new(w, h), h, shift: false, text: [0; 128], len: 0 };
    let t0 = now();
    hive.all(grants);
    let drawn = ferros_sys::us_since(t0);
    if !hive.show() {
        console(grants, b"hive: the display is not in 10 bits\n");
        exit(3);
    }
    let shown = ferros_sys::us_since(t0) - drawn;
    {
        use core::fmt::Write;
        let mut l = Line::new();
        let _ = write!(l, "hive: on the panel ({w}x{h}; drawn in {drawn} us, published in {shown} us)\n");
        console(grants, l.as_bytes());
    }
    // Each finger on its own: where it landed, the key under it, whether it has moved past the slop.
    let mut fingers = [(false, 0i64, 0i64, NONE, false); 16];
    loop {
        let n = call(touch, &[1], &mut r) as usize;
        let count = if n >= 1 && n != ferros_sys::ERR as usize { (r[0] as usize).min(16) } else { 0 };
        let mut here = [false; 16];
        let mut dirty = false;
        for k in 0..count {
            if 1 + (k + 1) * 12 > n {
                break;
            }
            let rec = &r[1 + k * 12..1 + (k + 1) * 12];
            let id = (rec[0] as usize) & 15;
            let (x, y) = (u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
            here[id] = true;
            let f = &mut fingers[id];
            if !f.0 {
                *f = (true, x, y, hive.l.hit(x, y), false);
                if f.3 != NONE {
                    hive.key(f.3, true);
                    dirty = true;
                }
            } else if !f.4 && (x - f.1).pow(2) + (y - f.2).pow(2) > SLOP * SLOP {
                f.4 = true;
                if f.3 != NONE {
                    hive.key(f.3, false);
                    dirty = true;
                }
            }
        }
        for id in 0..16 {
            let f = fingers[id];
            if f.0 && !here[id] {
                fingers[id].0 = false;
                if f.3 != NONE && !f.4 {
                    let t = now();
                    unsafe { LABEL_US = 0 };
                    hive.key(f.3, false);
                    hive.tap(grants, f.3);
                    hive.show();
                    let mut l = Line::new();
                    let tail = &hive.text[hive.len - hive.len.min(24)..hive.len];
                    let _ = core::fmt::Write::write_fmt(&mut l, format_args!("hive: tap in {} us (text {} us): {}\n", ferros_sys::us_since(t), unsafe { LABEL_US }, core::str::from_utf8(tail).unwrap_or("")));
                    console(grants, l.as_bytes());
                }
            }
        }
        if dirty {
            hive.show();
        }
        sleep_until(now() + freq() / 240);
    }
}

/// A console line in a fixed buffer.
struct Line {
    b: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Line {
        Line { b: [0; 160], n: 0 }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
