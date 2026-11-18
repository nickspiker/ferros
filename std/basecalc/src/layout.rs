//! Where everything goes: the window fills the glass, and the keys and text stay inside the app's circle (ferros WINDOWS.md "Drawn under the controls"), each row as wide as the circle's chord lets it be where the system holds the frame outside the circle.
//! Every length is a fraction of the circle's diameter or of the frame's width (AGENT.md "Dimensional Units"), so the pad is the same shape on the Pro, the 8a and anything else.

use crate::keys::{self, Key};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// The app's circle in the frame's pixels, and which halves of the frame outside it the system holds (fluor's `SafeArea`, told by the display as `EV_SAFE`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    /// The frame outside the circle above its centre is the system's (the console's circles).
    pub top: bool,
    /// The frame outside the circle below its centre is the system's (the helm's wedges).
    pub bottom: bool,
}

impl Circle {
    /// Whether (x, y) is the app's to put a control at (fluor `SafeArea::contains`).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let held = if y < self.cy { self.top } else { self.bottom };
        dx * dx + dy * dy <= self.r * self.r || !held
    }
}

/// The circle the display gives a window filling a `w` × `h` panel, worked out its way until it says (ferros_sys `active_circle`): tangent to the bottom of the camera cutout and to the bottom of the glass, centred, both halves held.
/// The cutout's bottom is a fact of each panel (ferros_sys `CUTOUT_BOTTOM`, measured on the glass); a panel not known here takes the Pro's as a fraction of its height.
pub fn panel_circle(w: f32, h: f32) -> Circle {
    let cutout = match (w as u32, h as u32) {
        (1344, 2992) => 118.5610,
        (1080, 2400) => 101.8303,
        _ => h * (118.5610 / 2992.),
    };
    let r = (h - cutout) * 0.5;
    Circle { cx: w * 0.5, cy: cutout + r, r, top: true, bottom: true }
}

/// A smooth minimum: below both `a` and `b`, by about `k` / 2 where they meet, C-infinity everywhere (AGENT.md "GUI Continuity": a row's width follows the chord into the frame's width with no kink as the window's size moves).
pub fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    (a + b - ((a - b) * (a - b) + k * k).sqrt()) * 0.5
}

/// The digit block's grid for a base: columns across, rows down, digit d at row d / cols and column d % cols, read like text from 0 at the top left.
/// The bases come in four families, and each base's digits sit where they sit in its family's largest member, so a digit never moves as the base changes within a family; rows only come and go:
/// - two across (2, 4): binary's two keys side by side, the largest of any base; quaternary 2 x 2, the row its high bit and the column its low bit.
/// - three across, the dozenal family (3, 5, 6, 9, 10, 11, 12): dozenal 3 x 4, each row a quarter of the dozen (0-2, 3-5, 6-8, 9-B) and three across as every decimal keypad is; decimal is dozenal less A and B (9 alone opens the last row), so the two share every digit's place; senary is two rows of three, nonary 3 x 3 (the row its high trit, the column its low one); ternary, quinary and undecimal are the family's rows cut short.
/// - four across, the hex family (7, 8, 13 to 16): hex 4 x 4, the row the digit's high two bits and the column its low two (a nibble read as two base-4 digits); octal is hex's top two rows (the row its high bit), and septenary and 13 to 15 are hex cut short.
/// - six across, the base-36 family (17 to 36): base 36 is 6 x 6, the row d / 6 and the column d % 6 (two senary digits); every base past hex keeps base 36's places, a row of six at a time.
pub fn digit_grid(base: u8) -> (u8, u8) {
    let cols = match base {
        2 | 4 => 2,
        3 | 5 | 6 | 9 | 10 | 11 | 12 => 3,
        7 | 8 | 13..=16 => 4,
        _ => 6,
    };
    (cols, base.div_ceil(cols))
}

/// What a placed key is, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Digit,
    Operator,
    Function,
    Equals,
    /// The top strip, the settings row and the picker.
    Control,
    /// The base now in use, or the layer now shown.
    Chosen,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub key: Key,
    pub rect: Rect,
    pub face: Face,
}

/// The pad's bands, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub circle: Circle,
    /// The gap between keys.
    pub gap: f32,
    /// A key row's height, its gap below it included.
    pub pitch: f32,
    /// The quick bases.
    pub bases: Rect,
    /// History, the expression and the result.
    pub display: Rect,
    /// The layer key, the angle unit and the precision.
    pub settings: Rect,
    /// The function rows (three), and with them the main block: the picker's place.
    pub functions: Rect,
    /// The digit block and the operator columns (five rows).
    pub main: Rect,
    /// The equals key, alone in the circle's bottom.
    pub equals: Rect,
}

impl Layout {
    /// The bands for a `w` × `h` frame and its circle.
    /// The circle's diameter L, top to bottom: a pad of L / 2^5 (the circle's tip is too narrow for a key), the quick bases (one row), the display (5L / 2^4), the settings row, three function rows, the main block's five rows, the equals row and a pad of L / 2^5: eleven rows in the 10L / 2^4 left.
    pub fn new(w: f32, h: f32, circle: Circle) -> Layout {
        let top = circle.cy - circle.r;
        let l = circle.r * 2.;
        let pad = l * (1. / (1 << 5) as f32);
        let display_h = l * (5. / (1 << 4) as f32);
        let pitch = (l - 2. * pad - display_h) / 11.;
        let gap = pitch * (1. / (1 << 3) as f32);
        let margin = w * (1. / (1 << 5) as f32);
        let band = |y: f32, rows: f32| -> Rect {
            let (y0, y1) = (y, y + rows * pitch - gap);
            let frame = w - 2. * margin;
            // The distance from the centre to the band's outer edge, where the chord is narrowest; a band across the centre, or on a half the system leaves to the app, takes the frame's width.
            let outer = if y1 <= circle.cy && circle.top {
                circle.cy - y0
            } else if y0 >= circle.cy && circle.bottom {
                y1 - circle.cy
            } else {
                0.
            };
            // One formula for every band, so nothing steps as a band's edge crosses the centre while the frame resizes; at the centre the chord is the diameter, wider than any frame here, and the smooth minimum gives the frame's width less a hair.
            let chord = 2. * ((circle.r * circle.r - outer * outer).sqrt() - margin);
            let width = smooth_min(frame, chord, margin);
            Rect { x: circle.cx - width * 0.5, y: y0, w: width, h: y1 - y0 }
        };
        let mut y = top + pad;
        let bases = band(y, 1.);
        y += pitch;
        let display = band(y, display_h / pitch);
        y += display_h;
        let settings = band(y, 1.);
        y += pitch;
        let functions = band(y, 3.);
        y += 3. * pitch;
        let main = band(y, 5.);
        y += 5. * pitch;
        let equals = band(y, 1.);
        Layout { w, h, circle, gap, pitch, bases, display, settings, functions, main, equals }
    }

    /// `n` equal cells across a row.
    pub fn row(&self, row: Rect, n: usize) -> Vec<Rect> {
        let cw = (row.w - (n - 1) as f32 * self.gap) / n as f32;
        (0..n).map(|i| Rect { x: row.x + i as f32 * (cw + self.gap), y: row.y, w: cw, h: row.h }).collect()
    }

    /// A `cols` × `rows` grid over `area`, row by row.
    pub fn grid(&self, area: Rect, cols: usize, rows: usize) -> Vec<Rect> {
        let cw = (area.w - (cols - 1) as f32 * self.gap) / cols as f32;
        let ch = (area.h - (rows - 1) as f32 * self.gap) / rows as f32;
        (0..rows * cols).map(|i| Rect { x: area.x + (i % cols) as f32 * (cw + self.gap), y: area.y + (i / cols) as f32 * (ch + self.gap), w: cw, h: ch }).collect()
    }

    /// The main block's six columns: the digit block over the first four, the operators over the last two.
    pub fn digit_area(&self) -> Rect {
        let unit = (self.main.w - 5. * self.gap) / 6.;
        Rect { w: 4. * unit + 3. * self.gap, ..self.main }
    }

    pub fn operator_area(&self) -> Rect {
        let unit = (self.main.w - 5. * self.gap) / 6.;
        Rect { x: self.main.x + 4. * (unit + self.gap), w: 2. * unit + self.gap, ..self.main }
    }

    /// The precision's own cell in the settings row (not a key: the count shown between its two keys).
    pub fn precision_label(&self) -> Rect {
        self.row(self.settings, 5)[3]
    }

    /// Every key on the glass for this base, layer and picker, each with its rect and face.
    /// The picker of every base takes the function rows and the main block (6 x 6: the 35 bases, 2# to 10#, then the key that closes it).
    pub fn place(&self, base: u8, second: bool, picking: bool) -> Vec<Placed> {
        let mut out = Vec::new();
        let chosen = |on: bool, face: Face| if on { Face::Chosen } else { face };
        let quick = keys::QUICK_BASES.contains(&base);
        for (i, cell) in self.row(self.bases, 7).into_iter().enumerate() {
            let key = if i < 6 { Key::Base(keys::QUICK_BASES[i]) } else { Key::Bases };
            let on = if i < 6 { keys::QUICK_BASES[i] == base } else { !quick || picking };
            out.push(Placed { key, rect: cell, face: chosen(on, Face::Control) });
        }
        let settings = self.row(self.settings, 5);
        for (i, key) in keys::SETTINGS {
            out.push(Placed { key, rect: settings[i], face: chosen(key == Key::Second && second, Face::Control) });
        }
        if picking {
            let area = Rect { h: self.main.y + self.main.h - self.functions.y, ..self.functions };
            for (i, cell) in self.grid(area, 6, 6).into_iter().enumerate() {
                let b = i as u8 + 2;
                let key = if b <= 36 { Key::Base(b) } else { Key::Bases };
                out.push(Placed { key, rect: cell, face: chosen(b == base, Face::Control) });
            }
        } else {
            for (r, row) in keys::function_rows(second).iter().enumerate() {
                let band = Rect { y: self.functions.y + r as f32 * self.pitch, h: self.pitch - self.gap, ..self.functions };
                for (key, cell) in row.iter().zip(self.row(band, 6)) {
                    out.push(Placed { key: *key, rect: cell, face: Face::Function });
                }
            }
            let (cols, rows) = digit_grid(base);
            let cells = self.grid(self.digit_area(), cols as usize, rows as usize);
            for d in 0..base {
                out.push(Placed { key: Key::Digit(d), rect: cells[d as usize], face: Face::Digit });
            }
            let ops = self.grid(self.operator_area(), 2, 5);
            for (i, key) in keys::OPERATORS.iter().flatten().enumerate() {
                out.push(Placed { key: *key, rect: ops[i], face: Face::Operator });
            }
        }
        out.push(Placed { key: Key::Equals, rect: self.equals, face: Face::Equals });
        out
    }
}
