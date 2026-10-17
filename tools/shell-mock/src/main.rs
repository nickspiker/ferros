//! The ferros shell as a picture, before it is a program: four screens at husky's 1344x2992, laid out in fluor's span units so the same code is right on any panel, written as PPM frames (`shell-mock <out dir>`).
//!
//! The screen shows state, not a menu (SHELL.md): one status line, the people who want you, capture one gesture away, and the sealed-app list one step down. Drawn front to back the fluor way: text and marks first, the black background last.

use fluor::canvas::{Canvas, Damage};
use fluor::geom::Viewport;
use fluor::paint;
use fluor::text::{TextRenderer, TextStyle};

const W: usize = 1344;
const H: usize = 2992;

/// An opaque colour in fluor's α+darkness form (pack_argb is not const).
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | ((255 - r) as u32) << 16 | ((255 - g) as u32) << 8 | (255 - b) as u32
}
const WHITE: u32 = rgb(0xF0, 0xF0, 0xF0);
const GREY: u32 = rgb(0x9A, 0x9A, 0x9A);
const DIM: u32 = rgb(0x5C, 0x5C, 0x5C);
const RULE: u32 = rgb(0x28, 0x28, 0x28);
/// Photon's unread colour (theme.rs RING_UNREAD_COLOUR).
const UNREAD: u32 = rgb(0xFF, 0x00, 0xFF);
const BLACK: u32 = rgb(0, 0, 0);

struct Screen<'a> {
    c: Canvas<'a>,
    t: &'a mut TextRenderer,
    span: f32,
}

impl Screen<'_> {
    fn left(&mut self, s: &str, x: f32, y: f32, size: f32, colour: u32, weight: u16) -> f32 {
        self.t.draw_text_left(&mut self.c, s, x, y, &TextStyle::new(size, colour).weight(weight), None, None)
    }
    fn right(&mut self, s: &str, x: f32, y: f32, size: f32, colour: u32, weight: u16) {
        self.t.draw_text_right(&mut self.c, s, x, y, &TextStyle::new(size, colour).weight(weight), None, None);
    }
    fn center(&mut self, s: &str, y: f32, size: f32, colour: u32, weight: u16) {
        self.t.draw_text_center(&mut self.c, s, W as f32 * 0.5, y, &TextStyle::new(size, colour).weight(weight), None, None);
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, colour: u32) {
        paint::fill_rect(&mut self.c, x as isize, y as isize, w as isize, h as isize, colour, None, None);
    }
    /// The one line every screen shares: time left, battery and link right. Plain facts, no badge.
    fn status(&mut self, time: &str) {
        let (m, y, s) = (self.span * 0.04, self.span * 0.05, self.span * 0.021);
        self.left(time, m, y, s, GREY, 500);
        self.right("WiFi \u{2022} 71% \u{2022} slot b", W as f32 - m, y, s, DIM, 400);
    }
    fn footer(&mut self, left: &str, right: &str) {
        let (m, y, s) = (self.span * 0.04, H as f32 - self.span * 0.05, self.span * 0.019);
        self.left(left, m, y, s, DIM, 400);
        self.right(right, W as f32 - m, y, s, GREY, 500);
    }
}

struct Row {
    who: &'static str,
    last: &'static str,
    when: &'static str,
    unread: bool,
}

const PEOPLE: &[Row] = &[
    Row { who: "Emma", last: "ferry's in, fog lifting", when: "14:02", unread: true },
    Row { who: "Scott", last: "\u{25B6} beam missed", when: "13:40", unread: true },
    Row { who: "Brandon", last: "You: pushed, have a look", when: "11:15", unread: false },
    Row { who: "Dad", last: "Sunday still good?", when: "Wed", unread: false },
    Row { who: "Ferry crew", last: "Mo: 5:40 sailing cancelled", when: "Wed", unread: false },
    Row { who: "Azie", last: "\u{1F4F7} 3 photos", when: "Tue", unread: false },
    Row { who: "Dr Lin", last: "Confirmed for the 22nd", when: "Mon", unread: false },
    Row { who: "Notes to self", last: "tanh after the sqrt", when: "Mon", unread: false },
];

fn home(s: &mut Screen) {
    s.status("14:07");
    let (m, top, rh) = (s.span * 0.04, s.span * 0.11, s.span * 0.082);
    let (name, small) = (s.span * 0.027, s.span * 0.0205);
    for (i, r) in PEOPLE.iter().enumerate() {
        let y = top + i as f32 * rh;
        let (c_name, c_last) = if r.unread { (WHITE, GREY) } else { (GREY, DIM) };
        s.left(r.who, m, y + rh * 0.34, name, c_name, if r.unread { 650 } else { 500 });
        s.left(r.last, m, y + rh * 0.68, small, c_last, 400);
        s.right(r.when, W as f32 - m, y + rh * 0.34, small, if r.unread { GREY } else { DIM }, 400);
        if r.unread {
            let d = s.span * 0.009;
            s.rect(W as f32 - m - d, y + rh * 0.68 - d * 0.5, d, d, UNREAD);
        }
        s.rect(m, y + rh, W as f32 - 2. * m, 1., RULE);
    }
    s.footer("Lumis  \u{2014}  hold power + volume down", "Apps \u{203A}");
}

fn apps(s: &mut Screen) {
    s.status("14:07");
    let (m, top, rh) = (s.span * 0.04, s.span * 0.11, s.span * 0.082);
    s.left("Sealed", m, top - rh * 0.35, s.span * 0.021, DIM, 500);
    let apps = [("Photon", "people", "G#5f3a\u{2026}"), ("Lumis", "camera", "G#c01e\u{2026}"), ("Validate", "this phone against known-good", "G#88d2\u{2026}"), ("Settings", "", "G#b140\u{2026}")];
    for (i, (name, what, seal)) in apps.iter().enumerate() {
        let y = top + i as f32 * rh;
        s.left(name, m, y + rh * 0.34, s.span * 0.027, WHITE, 550);
        s.left(what, m, y + rh * 0.68, s.span * 0.0205, DIM, 400);
        s.right(seal, W as f32 - m, y + rh * 0.68, s.span * 0.017, DIM, 400);
        s.rect(m, y + rh, W as f32 - 2. * m, 1., RULE);
    }
    s.left("4 sealed \u{2022} a capsule runs only if this phone compiled and sealed it", m, top + 4.6 * rh, s.span * 0.017, DIM, 400);
    s.footer("\u{2039} People", "");
}

fn first_boot(s: &mut Screen) {
    s.status("14:07");
    let mid = H as f32 * 0.42;
    s.center("No one here yet.", mid - s.span * 0.05, s.span * 0.034, WHITE, 600);
    s.center("Pair with a device you trust.", mid + s.span * 0.01, s.span * 0.024, GREY, 400);
    s.center("Bring a Photon you trust close, and accept there.", mid + s.span * 0.055, s.span * 0.019, DIM, 400);
    s.footer("", "Validate this phone \u{203A}");
}

fn resting(s: &mut Screen) {
    let mid = H as f32 * 0.40;
    s.center("14:07", mid, s.span * 0.13, GREY, 300);
    s.center("Thu 16 Oct", mid + s.span * 0.085, s.span * 0.022, DIM, 400);
    s.center("2 unread \u{2022} Emma, Scott", mid + s.span * 0.13, s.span * 0.021, DIM, 400);
    let d = s.span * 0.009;
    s.rect(W as f32 * 0.5 - d * 0.5, mid + s.span * 0.165, d, d, UNREAD);
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let vp = Viewport::new(W as u32, H as u32);
    let span = vp.span as f32;
    let mut text = TextRenderer::new();
    let screens: [(&str, fn(&mut Screen)); 4] = [("home", home), ("apps", apps), ("first-boot", first_boot), ("resting", resting)];
    for (name, draw) in screens {
        let mut px = vec![0u32; W * H];
        let mut damage = Damage::new();
        let mut s = Screen { c: Canvas::new(&mut px, W, H, &mut damage), t: &mut text, span };
        draw(&mut s);
        // The background last: front to back, it only lands where nothing above covered the pixel.
        s.rect(0., 0., W as f32, H as f32, BLACK);
        let mut ppm = format!("P6\n{W} {H}\n255\n").into_bytes();
        for p in &px {
            ppm.extend_from_slice(&[255 - ((p >> 16) & 0xFF) as u8, 255 - ((p >> 8) & 0xFF) as u8, 255 - (p & 0xFF) as u8]);
        }
        std::fs::write(format!("{out}/{name}.ppm"), ppm).expect("write");
        println!("{name}: {W}x{H}, span {span:.0}");
    }
}
