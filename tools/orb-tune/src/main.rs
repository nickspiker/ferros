//! orb tune — the console's radio circles on the desktop, every dimension a slider.
//!
//! Left: one console circle drawn large, and beside it the same circle at its true size on husky's 1344 × 2992 panel (the radius `corner_layout` solves for that radio's corner) and at twice that, so the look is judged at real size.
//! Right: one slider row per knob, its value in its own unit, in the style of chirp's bell tune.
//! The circle is the vessel (Lumis's camera-button profile made radial: three channel rings at their own depths, a dark tinted body) and, behind the O toggle, the orb inside it: a Julia set whose c every dimension of the radio's link picks.
//! GPS (radio 5) has its own orb, the sky dome (sky.rs): its satellites, their fix and the privacy turn on the last fifteen sliders; switched off it shows its map pin.
//! Keys: R rolls every dimension of the link, S prints every knob (paste-ready as --dump arguments), 0-6 pick the radio, O shows or hides the orb, Space makes it alive (one frame step a redraw, counted in frames, no clock).
//! `orb-tune --dump <out.vsf> [key=value ...]` writes the left side as a VSF visual (VSF RGB, gamma2tanh at 16 bits) instead of opening a window; `--nuclei` prints each bulb's nucleus.

mod look;
mod sky;

use look::{Look, Prep, RADIOS};

use fluor::canvas::{Canvas, PixelRect};
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, Key, KeyEvent, MouseButton, NamedKey};
use fluor::geom::Viewport;
use fluor::host::app::{Context, EventResponse, FluorApp};
use fluor::host::chrome::{self, HIT_NONE, HitId, ResizeEdge};
use fluor::host::chrome_widget::DefaultChrome;
use fluor::host::widget::{self as widget, Container, TabDir, Widget};
use fluor::pixel::{Blend, BlendMode, G2t16, Pixel};
use fluor::text::{TextRenderer, TextStyle};
use fluor::widgets::Slider;

/// One knob: its key (for --dump and S), its label, its range and its step (0 continuous).
struct Knob {
    key: &'static str,
    label: &'static str,
    lo: f32,
    hi: f32,
    step: f32,
}

const KNOBS: [Knob; 40] = [
    Knob { key: "radio", label: "radio", lo: 0., hi: 6., step: 1. },
    Knob { key: "depth", label: "rim depth", lo: 0., hi: 1. / (1 << 2) as f32, step: 0. },
    Knob { key: "width", label: "rim tail λ", lo: 1. / (1 << 9) as f32, hi: 1. / (1 << 4) as f32, step: 0. },
    Knob { key: "sep", label: "channel separation", lo: 0., hi: 1. / (1 << 4) as f32, step: 0. },
    Knob { key: "rim", label: "rim light", lo: -4., hi: 4., step: 0. },
    Knob { key: "body", label: "body darkness", lo: 0., hi: 12., step: 0. },
    Knob { key: "fr", label: "flavour R", lo: 0., hi: 1., step: 0. },
    Knob { key: "fg", label: "flavour G", lo: 0., hi: 1., step: 0. },
    Knob { key: "fb", label: "flavour B", lo: 0., hi: 1., step: 0. },
    Knob { key: "orb", label: "orb (O)", lo: 0., hi: 1., step: 1. },
    Knob { key: "state", label: "state", lo: 0., hi: 2., step: 1. },
    Knob { key: "period", label: "streams → bulb", lo: 1., hi: 8., step: 1. },
    Knob { key: "snr", label: "quality (SNR)", lo: 0., hi: 40., step: 0. },
    Knob { key: "rssi", label: "signal", lo: -100., hi: -20., step: 0. },
    Knob { key: "noise", label: "noise floor", lo: -100., hi: -60., step: 0. },
    Knob { key: "band", label: "band", lo: 0., hi: 2., step: 1. },
    Knob { key: "bw", label: "bandwidth", lo: 0., hi: 9., step: 0. },
    Knob { key: "qam", label: "constellation (MCS)", lo: 2., hi: 12., step: 2. },
    Knob { key: "loss", label: "loss", lo: 0., hi: 32., step: 0. },
    Knob { key: "delay", label: "delay spread", lo: 0., hi: (1 << 9) as f32, step: 0. },
    Knob { key: "frame", label: "frame (Space)", lo: 0., hi: 1023., step: 1. },
    Knob { key: "orbr", label: "orb size", lo: 1. / (1 << 2) as f32, hi: 1., step: 0. },
    Knob { key: "plane", label: "plane half-width", lo: 1. / (1 << 1) as f32, hi: 4., step: 0. },
    Knob { key: "knee", label: "soft knee", lo: -4., hi: 2., step: 0. },
    Knob { key: "sharp", label: "rim sharpness s", lo: 1. / (1 << 4) as f32, hi: 1., step: 0. },
    // GPS (radio 5): the sky dome's synthetic sky, its fix, and the privacy turn.
    Knob { key: "gps", label: "GPS satellites", lo: 0., hi: 16., step: 1. },
    Knob { key: "gal", label: "Galileo satellites", lo: 0., hi: 16., step: 1. },
    Knob { key: "glo", label: "GLONASS satellites", lo: 0., hi: 16., step: 1. },
    Knob { key: "bds", label: "BeiDou satellites", lo: 0., hi: 16., step: 1. },
    Knob { key: "qzss", label: "QZSS satellites", lo: 0., hi: 8., step: 1. },
    Knob { key: "navic", label: "NavIC satellites", lo: 0., hi: 8., step: 1. },
    Knob { key: "seed", label: "sky seed", lo: 0., hi: 1023., step: 1. },
    Knob { key: "cn0", label: "C/N0 spread", lo: 0., hi: 16., step: 0. },
    Knob { key: "used", label: "used in fix", lo: 0., hi: 32., step: 1. },
    Knob { key: "l5", label: "L5-capable", lo: 0., hi: 1., step: 0. },
    Knob { key: "herr", label: "horizontal error", lo: 1., hi: (1 << 7) as f32, step: 0. },
    Knob { key: "sky", label: "open sky (DOP)", lo: 1. / (1 << 4) as f32, hi: 1., step: 0. },
    Knob { key: "turn", label: "secret turn", lo: 0., hi: 1., step: 0. },
    Knob { key: "mirror", label: "mirror", lo: 0., hi: 1., step: 1. },
    Knob { key: "stretch", label: "L1 stretch γ", lo: 1., hi: (1 << 4) as f32, step: 0. },
];
const N_KNOBS: usize = KNOBS.len();
const K_RADIO: usize = 0;
const K_FRAME: usize = 20;

fn get(k: &Look, i: usize) -> f32 {
    match i {
        0 => k.radio as f32,
        1 => k.depth,
        2 => k.width,
        3 => k.sep,
        4 => k.rim_stops,
        5 => k.body_stops,
        6..=8 => k.flavour[i - 6],
        9 => k.orb as u32 as f32,
        10 => k.state as f32,
        11 => k.period as f32,
        12 => k.snr_db,
        13 => k.rssi_dbm,
        14 => k.noise_dbm,
        15 => k.band as f32,
        16 => k.bw_log2,
        17 => k.qam_bits as f32,
        18 => k.loss_pct,
        19 => k.delay_ns,
        20 => k.frame as f32,
        21 => k.orb_r,
        22 => k.plane,
        23 => k.knee_stops,
        24 => k.sharp,
        25..=30 => k.sats[i - 25] as f32,
        31 => k.seed as f32,
        32 => k.cn0_spread,
        33 => k.used as f32,
        34 => k.l5_frac,
        35 => k.herr_m,
        36 => k.sector,
        37 => k.secret_turn,
        38 => k.mirror as u32 as f32,
        _ => k.stretch,
    }
}

/// Set knob `i`; a new radio brings its own flavour, channel width and constellation.
fn set(k: &mut Look, i: usize, v: f32) {
    match i {
        0 => {
            k.radio = v as usize;
            k.flavour = RADIOS[k.radio].flavour;
            k.bw_log2 = RADIOS[k.radio].bw_mhz.log2();
            k.qam_bits = RADIOS[k.radio].qam_bits;
        }
        1 => k.depth = v,
        2 => k.width = v,
        3 => k.sep = v,
        4 => k.rim_stops = v,
        5 => k.body_stops = v,
        6..=8 => k.flavour[i - 6] = v,
        9 => k.orb = v != 0.,
        10 => k.state = v as u32,
        11 => k.period = v as u32,
        12 => k.snr_db = v,
        13 => k.rssi_dbm = v,
        14 => k.noise_dbm = v,
        15 => k.band = v as u32,
        16 => k.bw_log2 = v,
        17 => k.qam_bits = v as u32,
        18 => k.loss_pct = v,
        19 => k.delay_ns = v,
        20 => k.frame = v as u32,
        21 => k.orb_r = v,
        22 => k.plane = v,
        23 => k.knee_stops = v,
        24 => k.sharp = v,
        25..=30 => k.sats[i - 25] = v as u32,
        31 => k.seed = v as u32,
        32 => k.cn0_spread = v,
        33 => k.used = v as u32,
        34 => k.l5_frac = v,
        35 => k.herr_m = v,
        36 => k.sector = v,
        37 => k.secret_turn = v,
        38 => k.mirror = v != 0.,
        _ => k.stretch = v,
    }
}

/// Knob `i`'s value as a person reads it, in its unit.
fn show(k: &Look, i: usize) -> String {
    let v = get(k, i);
    match i {
        0 => RADIOS[k.radio].name.to_string(),
        1..=3 => format!("{v:.4} r"),
        4 => format!("{v:+.2} stops"),
        5 => format!("{v:.2} stops under"),
        6..=8 => format!("{v:.3} √L"),
        9 => if k.orb { "on" } else { "off" }.to_string(),
        10 if k.radio == look::GPS => ["off", "searching", "fixed"][k.state as usize].to_string(),
        10 => ["off", "searching", "connected"][k.state as usize].to_string(),
        11 => format!("period {}", k.period),
        12 => format!("{v:.1} dB"),
        13 | 14 => format!("{v:.1} dBm"),
        15 => RADIOS[k.radio].bands[k.band as usize].to_string(),
        16 => format!("{:.1} MHz", v.exp2()),
        17 => format!("{}-QAM", 1u32 << k.qam_bits),
        18 => format!("{v:.1} %"),
        19 => format!("{v:.0} ns"),
        20 => format!("{}", k.frame),
        21 => format!("{v:.3} r"),
        22 => format!("{v:.3}"),
        23 => format!("{v:+.2} stops"),
        24 => format!("{v:.3}"),
        25..=30 => format!("{}", v as u32),
        31 => format!("{}", k.seed),
        32 => format!("±{v:.1} dB"),
        33 => {
            let s = sky::Sky::new(k);
            format!("{} of {} tracked", s.n_used, s.n_tracked)
        }
        34 => format!("{:.0} %", v * 100.),
        35 => format!("±{v:.1} m"),
        36 => format!("{:.0}°, HDOP {:.2}", v * 360., sky::Sky::new(k).hdop),
        37 => format!("{:.0}°", v * 360.),
        38 => if k.mirror { "mirrored" } else { "as seen" }.to_string(),
        _ => format!("γ {v:.2}"),
    }
}

/// A slider's position for knob `i`'s value.
fn to_unit(k: &Look, i: usize) -> f32 {
    (get(k, i) - KNOBS[i].lo) / (KNOBS[i].hi - KNOBS[i].lo)
}

/// Knob `i`'s value at slider position `u`, on its step.
fn from_unit(i: usize, u: f32) -> f32 {
    let kn = &KNOBS[i];
    let v = kn.lo + u * (kn.hi - kn.lo);
    if kn.step == 0. { v } else { kn.lo + ((v - kn.lo) / kn.step).round() * kn.step }
}

/// Every knob as `key=value`, paste-ready after `orb-tune --dump <out.vsf>`.
fn print_knobs(k: &Look) {
    let args: Vec<String> = (0..N_KNOBS).map(|i| format!("{}={}", KNOBS[i].key, get(k, i))).collect();
    println!("{}", args.join(" "));
    let c = Prep::new(k).c();
    println!("  c = {:+.6} {:+.6}i", c.re, c.im);
    if k.radio == look::GPS {
        println!("{}", sky::Sky::new(k).report());
    }
}

/// A smooth (C∞) minimum, (a + b − √((a − b)² + δ²))/2 with δ = 2⁻⁵ of a + b: a hair under the smaller, with no corner where the two cross.
fn smooth_min(a: f32, b: f32) -> f32 {
    let d = (a + b) / (1 << 5) as f32;
    0.5 * (a + b - ((a - b) * (a - b) + d * d).sqrt())
}

/// A 16-bit composite pixel as the 8-bit one the window shows: each field's top byte (a code's bucket is its top bits).
fn narrow(p: G2t16) -> u32 {
    let [r, g, b] = p.channels();
    (p.alpha() >> 8) << 24 | (r >> 8) << 16 | (g >> 8) << 8 | b >> 8
}

/// The three circles of a look: large (radius `big`, `ss` × `ss` samples), twice the console's size and the console's (4 × 4 each).
fn render_all(k: &Look, text: &mut TextRenderer, big: f32, ss: u32) -> [(Vec<G2t16>, usize); 3] {
    let p = Prep::new(k);
    let r0 = RADIOS[k.radio].console_r;
    [(big, ss), (2. * r0, 4), (r0, 4)].map(|(r, ss)| {
        let side = 2 * r.ceil() as usize + 4;
        let name = look::name_layer(text, k, r, side);
        look::render(&p, r, ss, &name)
    })
}

const LABEL: u32 = 0xFF00_0000 | (0x00C0_C0C0 ^ 0x00FF_FFFF);
const DIM: u32 = 0xFF00_0000 | (0x0080_8080 ^ 0x00FF_FFFF);
/// The slider column's ground, a dark blue-grey (as darkness), and the circles' black.
const COLUMN_BG: u32 = 0xFF00_0000 | (0x0014_161C ^ 0x00FF_FFFF);
const BLACK8: u32 = 0xFFFF_FFFF;

struct App {
    chrome: DefaultChrome,
    hit_counter: HitId,
    sliders: Vec<Slider>,
    current_focus: Option<HitId>,
    dragging: Option<usize>,
    modifiers: fluor::event::ModifiersState,
    look: Look,
    alive: bool,
    rng: u64,
    dirty: bool,
    /// The three circles and where each goes: (pixels, side, x0, y0).
    images: Vec<(Vec<G2t16>, usize, usize, usize)>,
    /// The left region's width, the big circle's centre and radius, the small circles' centres.
    split: Coord,
    big: (Coord, Coord, Coord),
    small: [(Coord, Coord); 2],
    row_pitch: Coord,
}

impl App {
    fn new() -> Self {
        let viewport = Viewport::new(1600, 1000);
        let mut hit_counter: HitId = HIT_NONE;
        let chrome = DefaultChrome::new(viewport, "orb tune".to_string(), None, None, &mut hit_counter);
        let look = Look::new(0);
        let sliders = (0..N_KNOBS).map(|i| Slider::new(&mut hit_counter, 0., 0., 1., 1., to_unit(&look, i))).collect();
        Self {
            chrome,
            hit_counter,
            sliders,
            current_focus: None,
            dragging: None,
            modifiers: fluor::event::ModifiersState::default(),
            look,
            alive: false,
            rng: 0x6F72_6220_7475_6E65,
            dirty: true,
            images: Vec::new(),
            split: 0.,
            big: (0., 0., 1.),
            small: [(0., 0.); 2],
            row_pitch: 1.,
        }
    }

    fn update_layout(&mut self, vp: Viewport) {
        let (w, h) = (vp.width_px as Coord, vp.height_px as Coord);
        let span = vp.effective_span();
        let top = h / (1 << 4) as Coord;
        let margin = span / (1 << 6) as Coord;
        self.split = w * (1. - 3. / (1 << 3) as Coord);
        // The small circles in a column at the left region's right edge: twice the console's size above, the console's below.
        let r0 = RADIOS[self.look.radio].console_r;
        let small_cx = self.split - margin - 2. * r0;
        self.small = [(small_cx, top + margin + 2. * r0), (small_cx, top + 2. * margin + 4. * r0 + r0)];
        let room_w = small_cx - 2. * r0 - 2. * margin;
        let room_h = h - top - margin;
        let r = 0.5 * smooth_min(room_w, room_h) * (1. - 1. / (1 << 4) as Coord);
        self.big = (margin + room_w * 0.5, top + room_h * 0.5, r);
        // One slider row per knob, between the top strip and the bottom.
        let rows_top = top;
        self.row_pitch = (h - top - margin) / N_KNOBS as Coord;
        let cw = w - self.split;
        let band_h = self.row_pitch * (1. - 1. / (1 << 2) as Coord);
        let (left, right) = (self.split + cw * 0.34, self.split + cw * 0.76);
        for (i, s) in self.sliders.iter_mut().enumerate() {
            let cy = rows_top + (i as Coord + 0.5) * self.row_pitch;
            s.set_rect((left + right) * 0.5, cy, right - left, band_h * 0.5);
        }
        self.dirty = true;
    }

    /// Slider moves into the look; a new radio brings its flavour (and its true size: the layout moves).
    fn poll_slider_changes(&mut self) -> bool {
        let mut changed = false;
        let mut radio_moved = false;
        for i in 0..N_KNOBS {
            if self.sliders[i].take_change() {
                let v = from_unit(i, self.sliders[i].value());
                if get(&self.look, i) != v {
                    set(&mut self.look, i, v);
                    changed = true;
                    radio_moved |= i == K_RADIO;
                }
            }
        }
        if radio_moved {
            self.sync_sliders();
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// The sliders to the look's values (after keys or a roll).
    fn sync_sliders(&mut self) {
        for i in 0..N_KNOBS {
            self.sliders[i].set_value(to_unit(&self.look, i));
            self.sliders[i].take_change();
        }
        self.dirty = true;
    }

    fn next_rand(&mut self) -> f32 {
        self.rng = look_mix(self.rng);
        (self.rng >> 40) as f32 / (1u32 << 24) as f32
    }

    /// R: every dimension of the link rolled (the vessel's knobs stay), the orb shown; mostly connected, a sixteenth searching, a sixteenth off.
    fn roll(&mut self) {
        let radio = (self.next_rand() * 7.) as usize;
        set(&mut self.look, K_RADIO, radio as f32);
        for i in 11..=19 {
            let u = self.next_rand();
            set(&mut self.look, i, from_unit(i, u));
        }
        let s = self.next_rand();
        self.look.state = if s < 1. / (1 << 4) as f32 { 0 } else if s < 1. / (1 << 3) as f32 { 1 } else { 2 };
        self.look.orb = true;
        self.sync_sliders();
        print_knobs(&self.look);
    }

    fn change_focus(&mut self, new_focus: Option<HitId>, ctx: &mut Context) {
        if new_focus == self.current_focus {
            return;
        }
        let prior = self.current_focus;
        widget::apply_focus_change(self as &mut dyn Container, prior, new_focus);
        self.current_focus = new_focus;
        ctx.window.request_redraw();
    }

    fn handle_key(&mut self, kev: &KeyEvent, ctx: &mut Context) -> EventResponse {
        if kev.state != ElementState::Pressed {
            return EventResponse::Pass;
        }
        match &kev.logical_key {
            Key::Named(NamedKey::Tab) => {
                let dir = if self.modifiers.shift_key() { TabDir::Backward } else { TabDir::Forward };
                let current = self.current_focus;
                let next = widget::linear_tab_next(self as &mut dyn Container, current, dir);
                self.change_focus(next, ctx);
                return EventResponse::Handled;
            }
            Key::Named(NamedKey::Escape) => {
                self.change_focus(None, ctx);
                return EventResponse::Handled;
            }
            Key::Named(NamedKey::Space) => {
                self.alive = !self.alive;
                println!("alive: {}", if self.alive { "on" } else { "off" });
                self.dirty = true;
                ctx.window.request_redraw();
                return EventResponse::Handled;
            }
            Key::Character(c) => {
                let c = c.to_ascii_lowercase();
                let handled = match c.as_str() {
                    "r" => {
                        self.roll();
                        self.update_layout(ctx.viewport);
                        true
                    }
                    "s" => {
                        print_knobs(&self.look);
                        true
                    }
                    "o" => {
                        self.look.orb = !self.look.orb;
                        self.sync_sliders();
                        true
                    }
                    d if d.len() == 1 && (b'0'..=b'6').contains(&d.as_bytes()[0]) => {
                        set(&mut self.look, K_RADIO, (d.as_bytes()[0] - b'0') as f32);
                        self.sync_sliders();
                        self.update_layout(ctx.viewport);
                        true
                    }
                    _ => false,
                };
                if handled {
                    ctx.window.request_redraw();
                    return EventResponse::Handled;
                }
            }
            _ => {}
        }
        let Some(focus_id) = self.current_focus else {
            return EventResponse::Pass;
        };
        let mods = self.modifiers;
        let response = widget::dispatch_key(self as &mut dyn Container, focus_id, kev, mods, ctx.text);
        if matches!(response, EventResponse::Handled) && self.poll_slider_changes() {
            self.update_layout(ctx.viewport);
            ctx.window.request_redraw();
        }
        response
    }
}

fn look_mix(k: u64) -> u64 {
    let mut z = k.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Container for App {
    fn visit(&mut self, f: &mut dyn FnMut(&mut dyn Widget)) {
        for s in &mut self.sliders {
            f(s);
        }
        self.chrome.visit(f);
    }
}

impl FluorApp for App {
    type UserEvent = ();

    fn title(&self) -> &str {
        "orb tune"
    }

    fn initial_size(&self, monitor: (u32, u32)) -> (u32, u32) {
        let h = monitor.1 * 7 / 8;
        (h * 8 / 5, h)
    }

    fn init(&mut self, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.update_layout(ctx.viewport);
    }

    fn on_resize(&mut self, _w: u32, _h: u32, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.chrome.set_full_edge(ctx.is_maximized);
        self.update_layout(ctx.viewport);
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        match event {
            Event::ModifiersChanged(m) => {
                self.modifiers = *m;
                EventResponse::Pass
            }
            Event::CursorMoved { .. } => {
                let (x, y) = (ctx.cursor_x, ctx.cursor_y);
                if let Some(i) = self.dragging {
                    self.sliders[i].set_value_from_x(x);
                    if self.poll_slider_changes() {
                        if i == K_RADIO {
                            self.update_layout(ctx.viewport);
                        }
                        ctx.window.request_redraw();
                    }
                    return EventResponse::Handled;
                }
                if self.chrome.set_hover(self.chrome.hit_at(x, y)) {
                    ctx.window.request_redraw();
                }
                EventResponse::Pass
            }
            Event::CursorLeft => {
                if self.chrome.set_hover(HIT_NONE) {
                    ctx.window.request_redraw();
                }
                EventResponse::Pass
            }
            Event::MouseInput { state: ElementState::Pressed, button: MouseButton::Left } => {
                let (x, y) = (ctx.cursor_x, ctx.cursor_y);
                let hit = self.chrome.hit_at(x, y);
                if self.chrome.owns_hit(hit) {
                    if hit == self.chrome.close_btn.id() {
                        return EventResponse::Close;
                    } else if hit == self.chrome.max_btn.id() {
                        return EventResponse::ToggleMaximized;
                    } else if hit == self.chrome.min_btn.id() {
                        return EventResponse::Minimize;
                    }
                    return EventResponse::Handled;
                }
                if hit != HIT_NONE {
                    let mods = self.modifiers;
                    let response = widget::dispatch_click(self as &mut dyn Container, hit, x, y, mods);
                    self.change_focus(Some(hit), ctx);
                    self.dragging = self.sliders.iter().position(|s| s.hit_id() == hit);
                    if self.poll_slider_changes() {
                        self.update_layout(ctx.viewport);
                    }
                    ctx.window.request_redraw();
                    return response;
                }
                let edge = chrome::get_resize_edge(ctx.viewport, x, y);
                if edge != ResizeEdge::None {
                    return EventResponse::StartResize(edge);
                }
                self.change_focus(None, ctx);
                EventResponse::StartWindowDrag
            }
            Event::MouseInput { state: ElementState::Released, .. } => {
                if self.dragging.take().is_some() {
                    // The drag drew the big circle at one sample a pixel; now the full four.
                    self.dirty = true;
                    ctx.window.request_redraw();
                    return EventResponse::Handled;
                }
                EventResponse::Pass
            }
            Event::KeyboardInput { event: kev } => self.handle_key(kev, ctx),
            Event::Focused(f) => {
                if self.chrome.set_focused(*f) {
                    ctx.window.request_redraw();
                }
                EventResponse::Pass
            }
            _ => EventResponse::Pass,
        }
    }

    fn damage_rect(&mut self, viewport: Viewport) -> Option<PixelRect> {
        Some(PixelRect::new(0, 0, viewport.width_px as usize, viewport.height_px as usize))
    }

    fn hit_test_map(&self) -> Option<(&[HitId], usize, usize)> {
        let (w, h) = self.chrome.dims();
        Some((self.chrome.hit_test_map(), w, h))
    }

    fn overlay_deltas(&mut self) -> Vec<u32> {
        let count = self.hit_counter as usize + 1;
        widget::build_overlay_deltas(self, count)
    }

    fn render(&mut self, target: &mut [u32], ctx: &mut Context) {
        let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
        // Alive: one frame step a redraw, then ask for the next.
        if self.alive {
            self.look.frame = (self.look.frame + 1) & 1023;
            self.sliders[K_FRAME].set_value(to_unit(&self.look, K_FRAME));
            self.sliders[K_FRAME].take_change();
            self.dirty = true;
        }
        if self.dirty {
            // While dragging or alive, one sample a pixel for the big circle; four when still.
            let ss = if self.dragging.is_some() || self.alive { 1 } else { 2 };
            let imgs = render_all(&self.look, ctx.text, self.big.2, ss);
            let centres = [(self.big.0, self.big.1), self.small[0], self.small[1]];
            self.images = imgs
                .into_iter()
                .zip(centres)
                .map(|((px, side), (cx, cy))| (px, side, (cx - side as Coord * 0.5) as usize, (cy - side as Coord * 0.5) as usize))
                .collect();
            self.dirty = false;
        }

        self.chrome.rasterize_perimeter(target, w, h, ctx.clip_mask);
        self.chrome.rasterize_chrome(ctx.damage, ctx.text, ctx.clip_mask);

        // Front to back: the sliders and their text, the chrome, the circles, then the grounds.
        let span = ctx.viewport.effective_span();
        let font = span / (1 << 6) as Coord * 1.25;
        let cw = w as Coord - self.split;
        for i in 0..N_KNOBS {
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            let id = self.sliders[i].hit_id();
            self.sliders[i].render_content_into(&mut canvas, Some(&mut self.chrome.hit_test_map), id);
            let b = self.sliders[i].bbox();
            let cy = b.y + b.h * 0.5;
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            ctx.text.draw_text_left(&mut canvas, KNOBS[i].label, self.split + cw / (1 << 5) as Coord, cy, &TextStyle::new(font, LABEL), None, None);
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            ctx.text.draw_text_left(&mut canvas, &show(&self.look, i), self.split + cw * 0.78, cy, &TextStyle::new(font, LABEL), None, None);
        }
        {
            let r0 = RADIOS[self.look.radio].console_r;
            let notes = [(format!("2× ({} px)", 2. * r0), self.small[0], 2. * r0), (format!("true size ({r0} px)"), self.small[1], r0)];
            for (t, (cx, cy), r) in notes {
                let mut canvas = Canvas::new(target, w, h, ctx.damage);
                ctx.text.draw_text_center(&mut canvas, &t, cx, cy + r + font, &TextStyle::new(font * 0.8, DIM), None, None);
            }
            let c = Prep::new(&self.look).c();
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            let line = format!("c = {:+.5} {:+.5}i{}", c.re, c.im, if self.alive { "   alive" } else { "" });
            ctx.text.draw_text_left(&mut canvas, &line, span / (1 << 6) as Coord, h as Coord - font, &TextStyle::new(font * 0.8, DIM), None, None);
        }
        self.chrome.flatten_into(target, w, h, None);
        for (px, side, x0, y0) in &self.images {
            for (j, row) in px.chunks(*side).enumerate() {
                let y = y0 + j;
                // WHY/PROOF: a circle's square can reach past the window's edge on a small window (its side follows the layout, not the window), and a row or column past it would index another row or past the buffer.
                if y >= h {
                    break;
                }
                for (i, &p) in row.iter().enumerate() {
                    let x = x0 + i;
                    if x >= w {
                        break;
                    }
                    target[y * w + x] = target[y * w + x].under(narrow(p), BlendMode::Normal);
                }
            }
        }
        let split = self.split as usize;
        for row in target.chunks_mut(w) {
            for (x, d) in row.iter_mut().enumerate() {
                *d = d.under(if x < split { BLACK8 } else { COLUMN_BG }, BlendMode::Normal);
            }
        }
        ctx.damage.add_bounds(0, 0, w, h);
        if self.alive {
            ctx.window.request_redraw();
        }
    }

    fn cursor_for(&self, x: Coord, y: Coord, ctx: &Context) -> CursorIcon {
        let hit = self.chrome.hit_at(x, y);
        if self.chrome.owns_hit(hit) || self.sliders.iter().any(|s| s.hit_id() == hit) {
            return CursorIcon::Pointer;
        }
        match chrome::get_resize_edge(ctx.viewport, x, y) {
            ResizeEdge::Top | ResizeEdge::Bottom => CursorIcon::NsResize,
            ResizeEdge::Left | ResizeEdge::Right => CursorIcon::EwResize,
            ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorIcon::NwseResize,
            ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorIcon::NeswResize,
            ResizeEdge::None => CursorIcon::Default,
        }
    }
}

/// --dump: the big circle (radius 2⁸ + 2⁷ px, 2 × 2 samples), the 2× and the true size beside it, on black, written as a VSF visual.
fn dump(out: &str, args: &[String]) {
    let mut k = Look::new(0);
    for a in args {
        let Some((key, val)) = a.split_once('=') else { fail(&format!("not key=value: {a}")) };
        let Some(i) = KNOBS.iter().position(|kn| kn.key == key) else { fail(&format!("no knob {key}")) };
        let Ok(v) = val.parse::<f32>() else { fail(&format!("{key}: not a number: {val}")) };
        // A radio brings its flavour, so fr, fg and fb after it override that.
        set(&mut k, i, v);
    }
    let mut text = TextRenderer::new();
    let big = ((1 << 8) + (1 << 7)) as f32;
    let imgs = render_all(&k, &mut text, big, 2);
    let gap = 1 << 4;
    let (s0, s1, s2) = (imgs[0].1, imgs[1].1, imgs[2].1);
    let (w, h) = (s0 + gap + s1 + gap, s0.max(s1 + gap + s2));
    let mut px = vec![u64::MAX; w * h];
    let places = [(0, 0), (s0 + gap, 0), (s0 + gap + (s1 - s2) / 2, s1 + gap)];
    for ((img, side), (x0, y0)) in imgs.iter().zip(places) {
        for j in 0..*side {
            px[(y0 + j) * w + x0..(y0 + j) * w + x0 + side].copy_from_slice(&img[j * side..(j + 1) * side]);
        }
    }
    let n = w * h;
    let mut samples = vec![0u16; 3 * n];
    for (i, p) in px.iter().enumerate() {
        let ch = p.channels();
        for c in 0..3 {
            samples[c * n + i] = (G2t16::ONE - ch[c]) as u16;
        }
    }
    // Clipping check: the brightest code of any channel, and how many samples sit within 2⁻⁸ of the top.
    let top = samples.iter().copied().max().unwrap_or(0);
    let near = samples.iter().filter(|&&s| s as u32 >= G2t16::ONE - (G2t16::ONE >> 8)).count();
    println!("max code {top} of {} ({:.1} %), {near} samples within 2⁻⁸ of the top", G2t16::ONE, 100. * top as f32 / G2t16::ONE as f32);
    use vsf::types::BitPackedTensor;
    use vsf::visual::{Encoding, Plane, Primaries, Visual};
    let mut plane = Plane::encoded("display", "orb", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "main".into();
    let args_line: Vec<String> = (0..N_KNOBS).map(|i| format!("{}={}", KNOBS[i].key, get(&k, i))).collect();
    let v = Visual { title: "orb tune".into(), text: args_line.join(" "), planes: vec![plane], ..Default::default() };
    let bytes = v.write().unwrap_or_else(|e| fail(&format!("visual: {e}")));
    std::fs::write(out, &bytes).unwrap_or_else(|e| fail(&format!("{out}: {e}")));
    if k.radio == look::GPS {
        println!("{}", sky::Sky::new(&k).report());
    }
    let c = Prep::new(&k).c();
    println!("{out}: {w}x{h}; big circle r {big} at (0, 0) side {s0}, 2x at ({}, 0) side {s1}, true size at ({}, {}) side {s2}; c = {:+.6} {:+.6}i", s0 + gap, places[2].0, places[2].1, c.re, c.im);
}

fn fail(m: &str) -> ! {
    eprintln!("orb-tune: {m}");
    std::process::exit(1)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(|s| s.as_str()) {
        Some("--dump") => {
            let Some(out) = a.get(2) else { fail("usage: orb-tune --dump <out.vsf> [key=value ...]") };
            dump(out, &a[3..]);
        }
        Some("--nuclei") => {
            for p in 1..=8 {
                let c = look::nucleus(p);
                // Its exact period: the first k with f_c^k(0) back at 0.
                let mut z = look::C::new(0., 0.);
                let mut period = 0;
                for k in 1..=p {
                    z = z.sq() + c;
                    if z.abs() < 1e-9 {
                        period = k;
                        break;
                    }
                }
                println!("period {p}: nucleus {:+.9} {:+.9}i (returns to 0 after {period})", c.re, c.im);
            }
        }
        _ => fluor::host::app::run_app(App::new()).expect("event loop"),
    }
}
