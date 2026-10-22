//! The map (SHELL.md): the phone is a map of your life. Every person, message, photo and app is a circle on one plane; the view is a camera over it, and the camera is the only thing that moves.
//!
//! The circles: the device itself first, at the centre (its front is the dashboard, its back the settings), then the apps — Photon, Mahere, Lumis, Opsin — placed once by a seeded random fit with generous air, so they land in the same place every time.
//!
//! The gestures (owner, 2026-10-22: "two finger scale/move for edits and back to a normal two finger vs one finger for the back"):
//! - One finger drags the plane; two fingers pinch to zoom and twist to turn it, about the point between them.
//! - A one-finger tap on a circle arrives at it: the next frame has the circle filling the screen, its front shown. A tap in the air around an arrived circle leaves it.
//! - Nothing is animated and nothing is timed (owner, 2026-10-22: "no animations ... instant, always, no timers"): a view is simply there on the next frame, and a tap is fingers down and up without moving, however long they rested.
//! - A two-finger tap turns a circle over: its back, the facts about it and its few choices. Another two-finger tap turns it back.
//! - Arranging by hand (from the device's back): two fingers that land on a circle move and size that circle instead of the view; a tap in the air ends it.
//!
//! All of it integer: the camera centre in world units ×256, the zoom in 16.16, pinches by integer square roots.
//!
//! What is still illustrative, and why: area is the share of the owner's time each thing takes, set from the owner's own ordering until time is measured. The rings (importance) and the one lit circle (changed since last seen) are set by hand here; importance becomes the owner's to declare and lit becomes Photon's to report. Positions are deterministic and an arrangement by hand lasts until the process ends: the map holds no vault capability yet.
//!
//! Started by init (payloads/init). Grant slots: 0 console, 1 display (use), 2 touch (use), 3 settings.

#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{call, cap, console, freq, now, sleep_until};
use tatau::Style;

const S_DISPLAY: u64 = 1;
const S_TOUCH: u64 = 2;
const S_SETTINGS: u64 = 3;

/// One circle: its label, its weight of ring (0 none, 1 thin, 2 heavy — importance), whether it is lit (changed since last seen), its share of the owner's time in thousandths (its area is that share of the map), its icon in the display server's bundle (tools/app-icons order), and what it waits for before it can be more than a circle.
struct Spec {
    label: &'static str,
    ring: u8,
    lit: u8,
    share: u32,
    icon: u8,
    needs: &'static str,
}

/// The device first: it is placed at the centre and never lights (lit means a person sent you something). The apps' shares are the owner's ordering until time is measured (2026-10-22: "Photon the most, then Mahere, then Lumis").
const CIRCLES: [Spec; 5] = [
    Spec { label: "ferros", ring: 1, lit: 0, share: 120, icon: 5, needs: "" },
    Spec { label: "Photon", ring: 2, lit: 1, share: 400, icon: 1, needs: "WiFi and your enrollment" },
    Spec { label: "Mahere", ring: 1, lit: 0, share: 250, icon: 2, needs: "a position (GNSS)" },
    Spec { label: "Lumis", ring: 1, lit: 0, share: 200, icon: 3, needs: "the camera as a process" },
    Spec { label: "Opsin", ring: 1, lit: 0, share: 150, icon: 4, needs: "Lumis's frames" },
];
const N: usize = CIRCLES.len();
const DEVICE: usize = 0;

/// The map's total circle area, as four circles of radius 110 world units: each circle gets its share of it, so area is time.
const BASE_RADIUS: i64 = 110;

// Colours, as the display server takes them (G#00RRGGBB): the map's own greys, white for what is current.
const WHITE: u32 = 0xF0F0F0;
const GREY: u32 = 0x9A9A9A;
const DIM: u32 = 0x5C5C5C;
const FILL: u32 = 0x2A2A2A;

/// Movement under this many panel pixels is a finger resting, not a drag: a tap may wobble this much.
const SLOP: i64 = 24;
/// One, in the rotation's Q14.
const ONE: i64 = 1 << 14;

/// A circle's radius in world units: area = share x the map's total, so r = 110 x sqrt(4 x share). Integer square root, no floats.
fn radius_of(share: u32) -> i64 {
    isqrt(BASE_RADIUS * BASE_RADIUS * 4 * share as i64 / 1000)
}

fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// A small, fixed-seed generator: the same seed places the same circles in the same places on every boot, which is what "nothing ever moves" asks of a layout that is not yet stored.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [-span, span).
    fn signed(&mut self, span: i64) -> i64 {
        ((self.next() >> 33) as i64 % (2 * span.max(1))) - span
    }
}

/// The device at the centre; then the rest one at a time, in table order (biggest first), each at the first random spot that keeps clear of every circle already placed by at least the larger of the two radii again: more air is better, and no two ever overlap.
fn place(rs: &[i64], xs: &mut [i64], ys: &mut [i64]) {
    let mut rng = Rng(0x6D61_7020_6C69_6665);
    for i in 1..rs.len() {
        let mut spread = rs[i] + rs[0];
        loop {
            let (x, y) = (rng.signed(spread), rng.signed(spread * 8 / 5));
            let clear = (0..i).all(|j| {
                let gap = rs[i] + rs[j] + rs[i].max(rs[j]);
                let (dx, dy) = (x - xs[j], y - ys[j]);
                dx * dx + dy * dy >= gap * gap
            });
            if clear {
                xs[i] = x;
                ys[i] = y;
                break;
            }
            // Widen the search slowly, so the circles stay as close as the air allows rather than scattering.
            spread += 4;
        }
    }
}

/// A line of text being written, for tatau's formatters.
struct Line {
    b: [u8; 120],
    n: usize,
}
impl Line {
    fn new() -> Line {
        Line { b: [0; 120], n: 0 }
    }
    fn s(&mut self, t: &str) -> &mut Line {
        let _ = self.write_str(t);
        self
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + n].copy_from_slice(&b[..n]);
        self.n += n;
        Ok(())
    }
}

/// The ops of one frame, sent to the display server as one batch (op 15): one call per frame however much is on it.
struct Batch {
    b: [u8; 4096],
    n: usize,
}
impl Batch {
    fn new() -> Batch {
        let mut b = Batch { b: [0; 4096], n: 1 };
        b.b[0] = 15;
        b
    }
    fn op(&mut self, bytes: &[u8]) {
        if self.n + 2 + bytes.len() > self.b.len() {
            return;
        }
        self.b[self.n..self.n + 2].copy_from_slice(&(bytes.len() as u16).to_le_bytes());
        self.b[self.n + 2..self.n + 2 + bytes.len()].copy_from_slice(bytes);
        self.n += 2 + bytes.len();
    }
    /// Text with its baseline at y; align 0 left, 1 centred on x, 2 right edge at x.
    fn text(&mut self, x: i64, y: i64, px: u16, colour: u32, align: u8, t: &str) {
        let mut m = [0u8; 140];
        m[0] = 2;
        m[1..3].copy_from_slice(&(x.clamp(0, 65535) as u16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(0, 65535) as u16).to_le_bytes());
        m[5..7].copy_from_slice(&px.to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        m[11] = align;
        let t = t.as_bytes();
        let k = t.len().min(m.len() - 12);
        m[12..12 + k].copy_from_slice(&t[..k]);
        self.op(&m[..12 + k]);
    }
    fn disc(&mut self, x: i64, y: i64, r: i64, colour: u32) {
        let mut m = [0u8; 11];
        m[0] = 16;
        m[1..3].copy_from_slice(&(x.clamp(-32768, 32767) as i16).to_le_bytes());
        m[3..5].copy_from_slice(&(y.clamp(-32768, 32767) as i16).to_le_bytes());
        m[5..7].copy_from_slice(&(r.clamp(0, 65535) as u16).to_le_bytes());
        m[7..11].copy_from_slice(&colour.to_le_bytes());
        self.op(&m);
    }
    fn send(&self, display: u64) {
        let mut r = [0u8; 4];
        call(display, &self.b[..self.n], &mut r);
    }
}

/// The camera: the centre in world units ×256, the zoom in panel pixels per world unit, 16.16, and the turn as a unit vector (cos, sin) in Q14 — world to screen is that rotation, then the zoom. A vector rather than an angle: a twist composes by one complex multiply and nothing needs a sine.
#[derive(Clone, Copy, PartialEq)]
struct Cam {
    x: i64,
    y: i64,
    z: i64,
    c: i64,
    s: i64,
}
const Z_MIN: i64 = 9830; // 0.15
const Z_MAX: i64 = 12 << 16;

impl Cam {
    /// A panel offset from the centre, in pixels, to world units ×256: the rotation undone, then the zoom.
    fn delta(&self, dx: i64, dy: i64) -> (i64, i64) {
        (((self.c * dx + self.s * dy) << 10) / self.z, ((self.c * dy - self.s * dx) << 10) / self.z)
    }
    /// A panel pixel to world units ×256.
    fn world(&self, w: i64, h: i64, sx: i64, sy: i64) -> (i64, i64) {
        let (dx, dy) = self.delta(sx - w / 2, sy - h / 2);
        (self.x + dx, self.y + dy)
    }
    fn op(&self, keep: bool) -> [u8; 18] {
        let mut m = [0u8; 18];
        m[0] = 14;
        m[1..5].copy_from_slice(&(self.x as i32).to_le_bytes());
        m[5..9].copy_from_slice(&(self.y as i32).to_le_bytes());
        m[9..13].copy_from_slice(&(self.z as u32).to_le_bytes());
        m[13] = keep as u8;
        m[14..16].copy_from_slice(&(self.c as i16).to_le_bytes());
        m[16..18].copy_from_slice(&(self.s as i16).to_le_bytes());
        m
    }
}

/// What the screen is showing: the plane, or one circle's front or back, arrived at.
#[derive(Clone, Copy, PartialEq)]
enum View {
    Map,
    Front(usize),
    Back(usize),
}

/// The device's back: what each row does when tapped.
#[derive(Clone, Copy)]
enum Choice {
    Base(u8),
    Labels,
    Arrange,
}

/// The four bases in the order the chooser shows them: the house base first.
const BASES: [u8; 4] = [12, 0x8C, 10, 16];

fn base_name(code: u8) -> &'static str {
    match code {
        10 => "arabic",
        16 => "hexadecimal",
        0x8C => "dozenal linear (DLS)",
        _ => "dozenal magnitude (DMS)",
    }
}

struct Map {
    grants: u64,
    display: u64,
    w: i64,
    h: i64,
    /// Circles in world units ×256, and radii in world units.
    xs: [i64; N],
    ys: [i64; N],
    rs: [i64; N],
    cam: Cam,
    view: View,
    editing: bool,
    /// The device back's rows, as laid out by the last frame: (top, bottom, what).
    rows: [(i64, i64, Choice); 8],
    nrows: usize,
}

impl Map {
    /// The arrived circle's radius on the panel.
    fn arrived_r(&self) -> i64 {
        self.w * 15 / 32
    }

    /// The circle under a panel pixel, smallest first when they overlap (the smaller is drawn on top).
    fn hit(&self, sx: i64, sy: i64) -> Option<usize> {
        let (wx, wy) = self.cam.world(self.w, self.h, sx, sy);
        let mut best: Option<usize> = None;
        for i in 0..N {
            let (dx, dy) = ((wx - self.xs[i]) >> 8, (wy - self.ys[i]) >> 8);
            if dx * dx + dy * dy <= self.rs[i] * self.rs[i] && best.is_none_or(|b| self.rs[i] < self.rs[b]) {
                best = Some(i);
            }
        }
        best
    }

    /// The whole scene to the display server: clear, then every circle in one call.
    fn send_scene(&self) {
        let labels = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
        let mut r = [0u8; 4];
        call(self.display, &[12], &mut r);
        let mut m = [0u8; 2 + 30 * N];
        m[0] = 13;
        m[1] = N as u8;
        for (i, a) in CIRCLES.iter().enumerate() {
            let at = 2 + i * 30;
            m[at..at + 4].copy_from_slice(&((self.xs[i] >> 8) as i32).to_le_bytes());
            m[at + 4..at + 8].copy_from_slice(&((self.ys[i] >> 8) as i32).to_le_bytes());
            m[at + 8..at + 10].copy_from_slice(&(self.rs[i] as u16).to_le_bytes());
            m[at + 10] = a.ring;
            m[at + 11] = a.lit;
            m[at + 12] = a.icon;
            let label = a.label.as_bytes();
            let len = if labels { label.len().min(16) } else { 0 };
            m[at + 13] = len as u8;
            m[at + 14..at + 14 + len].copy_from_slice(&label[..len]);
        }
        call(self.display, &m, &mut r);
    }

    /// One circle moved or resized in the display server's scene (op 17).
    fn send_move(&self, i: usize) {
        let mut m = [0u8; 12];
        m[0] = 17;
        m[1] = i as u8;
        m[2..6].copy_from_slice(&((self.xs[i] >> 8) as i32).to_le_bytes());
        m[6..10].copy_from_slice(&((self.ys[i] >> 8) as i32).to_le_bytes());
        m[10..12].copy_from_slice(&(self.rs[i] as u16).to_le_bytes());
        let mut r = [0u8; 4];
        call(self.display, &m, &mut r);
    }

    /// The camera that shows every circle with air around them, never closer than one world unit to a pixel.
    fn home(&self) -> Cam {
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for i in 0..N {
            let r = self.rs[i] << 8;
            x0 = x0.min(self.xs[i] - r);
            y0 = y0.min(self.ys[i] - r);
            x1 = x1.max(self.xs[i] + r);
            y1 = y1.max(self.ys[i] + r);
        }
        // Zoom so the bounds take seven eighths of the panel either way.
        let (bw, bh) = (((x1 - x0) >> 8).max(1), ((y1 - y0) >> 8).max(1));
        let z = ((self.w * 7 / 8) << 16) / bw;
        let z = z.min(((self.h * 7 / 8) << 16) / bh).clamp(Z_MIN, 1 << 16);
        Cam { x: (x0 + x1) / 2, y: (y0 + y1) / 2, z, c: self.cam.c, s: self.cam.s }
    }

    /// The camera that has arrived at circle i: it fills the panel's width, less a margin.
    fn at(&self, i: usize) -> Cam {
        Cam { x: self.xs[i], y: self.ys[i], z: ((self.arrived_r() << 16) / self.rs[i].max(1)).clamp(Z_MIN, Z_MAX), c: self.cam.c, s: self.cam.s }
    }

    /// Go to `to`: the camera is simply there (no animation).
    fn fly(&mut self, to: Cam) {
        self.cam = to;
    }

    /// Draw the frame: the overlay for the view first (what is sent first is on top), then the scene under it through the camera.
    fn draw(&mut self) {
        let mut b = Batch::new();
        b.op(&[1]);
        self.nrows = 0;
        match self.view {
            View::Map => {
                if self.editing {
                    b.text(self.w / 2, self.h / 16, 34, GREY, 1, "arranging by hand");
                    b.text(self.w / 2, self.h / 16 + 52, 28, DIM, 1, "two fingers on a circle move and size it \u{2022} tap the air to finish");
                }
            }
            View::Front(DEVICE) => self.dashboard(&mut b),
            View::Back(DEVICE) => self.settings(&mut b),
            View::Front(i) => {
                let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
                b.text(cx, cy + r + 96, 52, WHITE, 1, CIRCLES[i].label);
                b.text(cx, cy + r + 160, 32, GREY, 1, "not here yet \u{2022} two fingers for its back");
            }
            View::Back(i) => self.app_back(&mut b, i),
        }
        b.op(&self.cam.op(true));
        b.send(self.display);
    }

    /// Label and value on a row: the label right of centre's left, the value left of centre's right.
    fn row(b: &mut Batch, cx: i64, y: i64, label: &str, value: &str) {
        b.text(cx - 20, y, 36, GREY, 2, label);
        b.text(cx + 20, y, 36, WHITE, 0, value);
    }

    /// The device's front: the state of the device in plain facts, every number in the device base.
    fn dashboard(&self, b: &mut Batch) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        let f = ferros_sys::numbers(Style::Glyphs);
        let facts = ferros_sys::facts();
        let mut y = cy - r * 9 / 16;
        b.text(cx, y, 72, WHITE, 1, "ferros");
        y += 120;
        let mut v = Line::new();
        match facts.battery_pct() {
            Some(p) => {
                let _ = tatau::share(&mut v, f, p, 100, 2);
                if let Some(ua) = facts.battery_ua() {
                    v.s(if ua > 0 { "  charging" } else { "  on battery" });
                }
            }
            None => {
                v.s("not read");
            }
        }
        Map::row(b, cx, y, "battery", v.as_str());
        y += 64;
        let mut v = Line::new();
        match facts.boot_us() {
            Some(us) => {
                let _ = tatau::duration_ns(&mut v, f, us * 1000, 1);
            }
            None => {
                v.s("-");
            }
        }
        Map::row(b, cx, y, "press to ready", v.as_str());
        y += 64;
        let mut v = Line::new();
        // The counter runs from the SoC's reset: power on, near enough (the bootloader's seconds included).
        let (t, hz) = (now(), freq());
        let _ = tatau::duration_ns(&mut v, f, t / hz * 1_000_000_000 + t % hz * 1_000_000_000 / hz, 1);
        Map::row(b, cx, y, "since power on", v.as_str());
        y += 64;
        Map::row(b, cx, y, "USB", match facts.usb_link() { Some(true) => "linked", Some(false) => "no link", None => "-" });
        y += 64;
        let mut v = Line::new();
        if let Some(n) = facts.processes() {
            let _ = tatau::count(&mut v, f, n, 1);
        }
        Map::row(b, cx, y, "processes", v.as_str());
        y += 64;
        let mut v = Line::new();
        if let Some(n) = facts.vault_keys() {
            let _ = tatau::count(&mut v, f, n, 1);
        }
        Map::row(b, cx, y, "in the vault", v.as_str());
        y += 64;
        Map::row(b, cx, y, "numbers", base_name(ferros_sys::device_base()));
        b.text(cx, cy + r * 3 / 4, 28, DIM, 1, "two fingers: settings");
        b.disc(cx, cy, r - r / 25 - 1, FILL);
    }

    /// The device's back: its few choices. The number base is chosen by seeing — each row is two seconds and a mebibyte (2^23 bits, a whole number of doublings) written in that base — with its name small beneath.
    fn settings(&mut self, b: &mut Batch) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        let mut y = cy - r * 5 / 8;
        b.text(cx, y, 56, WHITE, 1, "settings");
        y += 96;
        let now_base = ferros_sys::device_base();
        for code in BASES {
            let f = tatau::Fmt::new(tatau::Base::from_code(code).unwrap_or(tatau::Base::Dms), Style::Glyphs);
            let mut v = Line::new();
            let _ = tatau::duration_ns(&mut v, f, 2_000_000_000, 1);
            v.s("   ");
            let _ = tatau::size_bits(&mut v, f, 1 << 23, 1);
            let current = code == now_base;
            b.text(cx, y, 44, if current { WHITE } else { GREY }, 1, v.as_str());
            b.text(cx, y + 38, 24, if current { GREY } else { DIM }, 1, base_name(code));
            if current {
                b.disc(cx - r * 5 / 8, y - 14, 10, WHITE);
            }
            self.push_row(y - 56, y + 52, Choice::Base(code));
            y += 112;
        }
        y += 24;
        let labels = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
        b.text(cx, y, 36, GREY, 1, if labels { "names under circles: on" } else { "names under circles: off" });
        self.push_row(y - 44, y + 24, Choice::Labels);
        y += 80;
        b.text(cx, y, 36, GREY, 1, "arrange by hand");
        self.push_row(y - 44, y + 24, Choice::Arrange);
        b.disc(cx, cy, r - r / 25 - 1, FILL);
    }

    fn push_row(&mut self, top: i64, bottom: i64, what: Choice) {
        if self.nrows < self.rows.len() {
            self.rows[self.nrows] = (top, bottom, what);
            self.nrows += 1;
        }
    }

    /// An app's back: the facts about it, the same for every circle.
    fn app_back(&self, b: &mut Batch, i: usize) {
        let (cx, cy, r) = (self.w / 2, self.h / 2, self.arrived_r());
        let f = ferros_sys::numbers(Style::Glyphs);
        let a = &CIRCLES[i];
        let mut y = cy - r / 2;
        b.text(cx, y, 64, WHITE, 1, a.label);
        y += 112;
        let mut v = Line::new();
        let _ = tatau::share(&mut v, f, a.share as u64, 1000, 2);
        Map::row(b, cx, y, "your time", v.as_str());
        y += 64;
        Map::row(b, cx, y, "installed", "not yet");
        y += 64;
        Map::row(b, cx, y, "waits for", a.needs);
        y += 64;
        Map::row(b, cx, y, "capabilities", "none");
        b.text(cx, cy + r * 3 / 4, 28, DIM, 1, "time is set by hand until it is measured");
        b.disc(cx, cy, r - r / 25 - 1, FILL);
    }

    /// A tap: one finger or two, at a panel point. Logged with what it did, so a session can be followed from `diag console`.
    fn tap(&mut self, fingers: usize, sx: i64, sy: i64) {
        let before = (self.view, self.editing);
        self.tap_inner(fingers, sx, sy);
        let mut l = Line::new();
        let name = |v: View| match v {
            View::Map => ("map", ""),
            View::Front(i) => ("front of ", CIRCLES[i].label),
            View::Back(i) => ("back of ", CIRCLES[i].label),
        };
        let (a, b) = name(self.view);
        let _ = write!(l, "map: {fingers}-finger tap at {sx},{sy} -> {a}{b}{}{}\n", if self.editing { ", arranging" } else { "" }, if before == (self.view, self.editing) { " (no change)" } else { "" });
        console(self.grants, l.as_str().as_bytes());
    }

    fn tap_inner(&mut self, fingers: usize, sx: i64, sy: i64) {
        let inside = {
            let (dx, dy) = (sx - self.w / 2, sy - self.h / 2);
            dx * dx + dy * dy <= self.arrived_r() * self.arrived_r()
        };
        if self.editing {
            if fingers == 1 && self.hit(sx, sy).is_none() {
                self.editing = false;
                self.draw();
            }
            return;
        }
        match (fingers, self.view) {
            (1, View::Map) => {
                if let Some(i) = self.hit(sx, sy) {
                    let to = self.at(i);
                    self.fly(to);
                    self.view = View::Front(i);
                    self.draw();
                }
            }
            (1, View::Front(_)) | (1, View::Back(_)) => {
                if !inside {
                    self.view = View::Map;
                    let to = self.home();
                    self.fly(to);
                    self.draw();
                } else if self.view == View::Back(DEVICE) {
                    let row = self.rows[..self.nrows].iter().find(|r| sy >= r.0 && sy < r.1).map(|r| r.2);
                    match row {
                        Some(Choice::Base(code)) => {
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_BASE, code);
                            self.draw();
                        }
                        Some(Choice::Labels) => {
                            let on = ferros_sys::pref(ferros_sys::PREF_LABELS) == 1;
                            ferros_sys::set_pref(cap(self.grants, S_SETTINGS), ferros_sys::PREF_LABELS, (!on) as u8);
                            self.send_scene();
                            self.draw();
                        }
                        Some(Choice::Arrange) => {
                            self.view = View::Map;
                            self.editing = true;
                            let to = self.home();
                            self.fly(to);
                            self.draw();
                        }
                        None => {}
                    }
                }
            }
            (2, View::Map) => {
                if let Some(i) = self.hit(sx, sy) {
                    let to = self.at(i);
                    self.fly(to);
                    self.view = View::Back(i);
                    self.draw();
                }
            }
            (2, View::Front(i)) => {
                self.view = View::Back(i);
                self.draw();
            }
            (2, View::Back(i)) => {
                self.view = View::Front(i);
                self.draw();
            }
            _ => {}
        }
    }
}

/// "map: numbers in <base>", with two seconds written in it, in words (the console is a terminal surface).
fn say_base(grants: u64, code: u8) {
    let mut out = Line::new();
    let f = ferros_sys::numbers(Style::Words);
    let _ = write!(out, "map: numbers in {:?} (G#{:02X}); two seconds reads ", f.base, code);
    let _ = tatau::duration_ns(&mut out, f, 2_000_000_000, 1);
    out.s("\n");
    console(grants, out.as_str().as_bytes());
}

/// The distance between two panel points in sixteenths of a pixel, for the pinch.
fn dist16(a: (i64, i64), b: (i64, i64)) -> i64 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    isqrt((dx * dx + dy * dy) << 8)
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The circles' labels are `&'static str`, absolute addresses fixed at link time; linked position-independent, they are relocations this applies before anything reads them. Without it the first label read faulted at its link-time address, G#D9B (2026-10-21).
    unsafe { ferros_sys::relocate() };
    let display = cap(grants, S_DISPLAY);
    let touch = cap(grants, S_TOUCH);
    let mut r = [0u8; 256];

    // The panel's size, from the display server.
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"map: no display server\n");
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as i64, u16::from_le_bytes([r[2], r[3]]) as i64);

    // The circles, sized by share and placed once.
    let rs: [i64; N] = core::array::from_fn(|i| radius_of(CIRCLES[i].share));
    let mut xs = [0i64; N];
    let mut ys = [0i64; N];
    place(&rs, &mut xs, &mut ys);
    let mut map = Map {
        grants,
        display,
        w,
        h,
        xs: xs.map(|x| x << 8),
        ys: ys.map(|y| y << 8),
        rs,
        cam: Cam { x: 0, y: 0, z: 1 << 16, c: ONE, s: 0 },
        view: View::Map,
        editing: false,
        rows: [(0, 0, Choice::Labels); 8],
        nrows: 0,
    };
    map.send_scene();
    map.cam = map.home();
    // A development build can start arrived somewhere, to see a front or a back without a finger: FERROS_MAP_START=f<n> or b<n> (circle n).
    if let Some(v) = option_env!("FERROS_MAP_START").map(str::as_bytes) {
        if v.len() == 2 && (v[1] as usize).wrapping_sub(b'0' as usize) < N {
            let i = (v[1] - b'0') as usize;
            map.cam = map.at(i);
            map.view = if v[0] == b'b' { View::Back(i) } else { View::Front(i) };
        }
    }
    map.draw();
    console(grants, b"map: home drawn; tap arrives, two fingers turn a circle over\n");
    // The device's number base, as every app reads it (SETTINGS.md): logged now and whenever it changes, which is the proof the preference reaches a running process.
    let mut base = ferros_sys::device_base();
    say_base(grants, base);
    let mut labels = ferros_sys::pref(ferros_sys::PREF_LABELS);

    // The previous frame's held contacts, by contact id: present, x, y.
    let mut prev = [(false, 0i64, 0i64); 16];
    // The touch session: from the first finger down to the last one up.
    let mut down = false;
    let mut most = 0usize;
    let mut moved = false;
    let mut start = [(false, 0i64, 0i64); 16];
    // Arranging: the circle two fingers took hold of this session (decided once, when the second finger moves), and whether it has been decided.
    let mut grab: Option<usize> = None;
    let mut grab_decided = false;
    let mut last_dashboard = now();
    loop {
        // A preference changed (here or from the bridge): redraw in it.
        let b = ferros_sys::device_base();
        if b != base {
            base = b;
            say_base(grants, base);
            map.draw();
        }
        let l = ferros_sys::pref(ferros_sys::PREF_LABELS);
        if l != labels {
            labels = l;
            map.send_scene();
            map.draw();
        }
        // The dashboard is live: once a second while it is showing.
        if map.view == View::Front(DEVICE) && now().wrapping_sub(last_dashboard) > freq() {
            last_dashboard = now();
            map.draw();
        }

        let n = call(touch, &[1], &mut r) as usize;
        let mut cur = [(false, 0i64, 0i64); 16];
        let mut ids = [0usize; 16];
        let mut held = 0usize;
        if n >= 1 && n != ferros_sys::ERR as usize {
            let count = (r[0] as usize).min(16);
            for k in 0..count {
                if 1 + (k + 1) * 12 > n {
                    break;
                }
                let rec = &r[1 + k * 12..1 + (k + 1) * 12];
                let id = (rec[0] as usize) & 15;
                cur[id] = (true, u16::from_le_bytes([rec[4], rec[5]]) as i64, u16::from_le_bytes([rec[6], rec[7]]) as i64);
                ids[held] = id;
                held += 1;
            }
        }

        // The session: when it began, how many fingers it ever held, where each landed, and whether any went further than a resting finger does.
        if held > 0 && !down {
            down = true;
            most = 0;
            moved = false;
            start = [(false, 0, 0); 16];
            grab = None;
            grab_decided = false;
        }
        if down {
            most = most.max(held);
            for &id in ids[..held].iter() {
                if !start[id].0 {
                    start[id] = cur[id];
                }
                let (dx, dy) = (cur[id].1 - start[id].1, cur[id].2 - start[id].2);
                if dx * dx + dy * dy > SLOP * SLOP {
                    moved = true;
                }
            }
        }
        if held == 0 && down {
            down = false;
            if !moved && most <= 2 {
                // Where the fingers landed, on average.
                let (mut sx, mut sy, mut k) = (0i64, 0i64, 0i64);
                for s in start.iter().filter(|s| s.0) {
                    sx += s.1;
                    sy += s.2;
                    k += 1;
                }
                if k > 0 {
                    map.tap(most, sx / k, sy / k);
                }
            }
        }

        // Drags and pinches, once the fingers have moved past the slop: the contacts present both now and last frame are the ones that moved.
        if moved {
            let mut both = [0usize; 2];
            let mut nb = 0usize;
            for &id in ids[..held].iter() {
                if prev[id].0 && nb < 2 {
                    both[nb] = id;
                    nb += 1;
                }
            }
            let before = map.cam;
            let mut scene_moved = false;
            if nb >= 1 && map.view != View::Map {
                // Moving away from an arrived circle leaves it: the overlay goes, the plane is free again.
                map.view = View::Map;
            }
            if nb == 1 {
                // One finger: the plane follows it.
                let id = both[0];
                let (dx, dy) = map.cam.delta(cur[id].1 - prev[id].1, cur[id].2 - prev[id].2);
                map.cam.x -= dx;
                map.cam.y -= dy;
            } else if nb == 2 {
                let (a, b) = (both[0], both[1]);
                let (pa, pb, ca, cb) = ((prev[a].1, prev[a].2), (prev[b].1, prev[b].2), (cur[a].1, cur[a].2), (cur[b].1, cur[b].2));
                let (d0, d1) = (dist16(pa, pb), dist16(ca, cb));
                let (m0x, m0y) = ((pa.0 + pb.0) / 2, (pa.1 + pb.1) / 2);
                let (m1x, m1y) = ((ca.0 + cb.0) / 2, (ca.1 + cb.1) / 2);
                if map.editing && !grab_decided {
                    grab_decided = true;
                    grab = map.hit(m0x, m0y);
                }
                if let (true, Some(i)) = (map.editing, grab) {
                    // Arranging: the circle follows the point between the fingers and grows or shrinks with their spread.
                    let (dx, dy) = map.cam.delta(m1x - m0x, m1y - m0y);
                    map.xs[i] += dx;
                    map.ys[i] += dy;
                    if d0 > 8 * 16 {
                        map.rs[i] = (map.rs[i] * d1 / d0).clamp(24, 1200);
                    }
                    map.send_move(i);
                    scene_moved = true;
                } else if d0 > 8 * 16 && d1 > 8 * 16 {
                    // Two fingers: zoom by how much they spread and turn by how much they twist, about the point between them, so what is under your fingers stays under your fingers.
                    let (wx, wy) = map.cam.world(w, h, m0x, m0y);
                    map.cam.z = (map.cam.z * d1 / d0).clamp(Z_MIN, Z_MAX);
                    // The twist between the two finger vectors, as (cos, sin) = (dot, cross) / (|a| |b|), composed onto the camera's own turn and brought back to unit length.
                    let (ax, ay, bx, by) = (pb.0 - pa.0, pb.1 - pa.1, cb.0 - ca.0, cb.1 - ca.1);
                    let (dot, cross) = (ax * bx + ay * by, ax * by - ay * bx);
                    // |a| |b| from the two spreads (sixteenths of a pixel each), so the product comes back near Q14 before the exact renormalising square root.
                    let ab = (d0 * d1 >> 8).max(1);
                    let (c, s) = ((map.cam.c * dot - map.cam.s * cross) / ab, (map.cam.s * dot + map.cam.c * cross) / ab);
                    let len = isqrt(c * c + s * s).max(1);
                    map.cam.c = c * ONE / len;
                    map.cam.s = s * ONE / len;
                    let (dx, dy) = map.cam.delta(m1x - w / 2, m1y - h / 2);
                    map.cam.x = wx - dx;
                    map.cam.y = wy - dy;
                }
            }
            if map.cam != before || scene_moved {
                map.draw();
            }
        }
        prev = cur;
        // A hundred-and-twentieth of a second while a finger is down, a thirtieth at rest.
        sleep_until(now() + if held > 0 { freq() / 120 } else { freq() / 30 });
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
