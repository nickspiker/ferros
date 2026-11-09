//! lightscope — a live scope on the PC's audio input, for a photodiode against a panel's glass (owner, 2026-11-08: "can you give me a live scope in fluor ... so I can pick the right diode, bias, etc?"). The emission's timing per panel, per rate, per brightness: the duty, the notch, pulses per frame, PWM.
//!
//! The input is AC-coupled (line in or mic in), so the light's DC level is gone: everything is read from the waveform's shape. The threshold sits between the window's lowest and highest sample (the trigger slider moves it); above it is "on". Rising crossings, with a sixteenth of the swing as hysteresis, give the frequency; the time above and below the threshold, the duty; the shortest on and off stretches, the notch's width. A sample at full scale marks the readout CLIPPING: lower the load resistor or the light.
//!
//! Keys: space freezes and thaws; `l` prints the readout to the terminal.
//! Usage: tools/lightscope/target/release/lightscope [part of the input device's name]   (default: the system's default input; every input is listed at start)
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use fluor::canvas::{Canvas, PixelRect};
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, Key, KeyEvent, MouseButton, NamedKey};
use fluor::geom::Viewport;
use fluor::host::app::{Context, EventResponse, FluorApp};
use fluor::host::chrome::{self, HIT_NONE, HitId, ResizeEdge};
use fluor::host::chrome_widget::DefaultChrome;
use fluor::host::widget::{self as widget, Container, Widget};
use fluor::paint::pack_argb;
use fluor::text::TextStyle;
use fluor::widgets::Slider;

/// The codec's own top rate: a sound server offers more by resampling, which adds no timing.
const NATIVE: u32 = 192_000;
/// The capture ring: about 5 s at 192 kHz.
const RING: usize = 1 << 20;
/// The screen's time span is 2^(LO + v × OCTAVES) seconds for the timebase slider's v in 0..1: 1/16 ms to 128 ms.
const SPAN_LO: f64 = -14.;
const SPAN_OCTAVES: f64 = 11.;
/// The analysis window: at least this many samples, and at least four screens.
const ANALYSIS_MIN: usize = 1 << 14;

struct Ring {
    buf: Vec<f32>,
    /// Samples written since the start (the newest is at `(written - 1) % RING`).
    written: usize,
}

/// What one analysis window says.
#[derive(Default, Clone, Copy)]
struct Reading {
    lo: f32,
    hi: f32,
    threshold: f32,
    freq: f64,
    duty: f64,
    on_min_us: f64,
    off_min_us: f64,
    clipped: usize,
}

struct Scope {
    title: String,
    chrome: DefaultChrome,
    timebase: Slider,
    trigger: Slider,
    hit_counter: HitId,
    dragging: Option<usize>,
    ring: Arc<Mutex<Ring>>,
    rate: u32,
    device: String,
    _stream: Option<cpal::Stream>,
    frozen: Option<Vec<f32>>,
    reading: Reading,
    plot: (usize, usize, usize, usize),
}

/// The input stream into the ring: the first channel of the device's best rate.
fn open_input(want: Option<&str>, ring: Arc<Mutex<Ring>>) -> (Option<cpal::Stream>, u32, String) {
    let host = cpal::default_host();
    let mut chosen = None;
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let name = d.description().map(|x| x.name().to_string()).unwrap_or_default();
            println!("input: {name}");
            if chosen.is_none() && want.is_some_and(|w| name.to_lowercase().contains(&w.to_lowercase())) {
                chosen = Some(d);
            }
        }
    }
    let Some(dev) = chosen.or_else(|| host.default_input_device()) else {
        eprintln!("lightscope: no input device");
        return (None, 48_000, "no input".into());
    };
    let name = dev.description().map(|x| x.name().to_string()).unwrap_or_default();
    // A real input first: two channels or fewer (a sound server's 64-channel config is a virtual mix), then the rate it can reach up to the codec's 192 kHz, then f32.
    let rate_of = |r: &cpal::SupportedStreamConfigRange| if r.min_sample_rate() <= NATIVE && NATIVE <= r.max_sample_rate() { NATIVE } else { r.max_sample_rate() };
    let Some(range) = dev.supported_input_configs().ok().and_then(|c| c.max_by_key(|r| (r.channels() <= 2, rate_of(r) <= NATIVE, rate_of(r), r.sample_format() == cpal::SampleFormat::F32))) else {
        eprintln!("lightscope: {name} offers no input configuration");
        return (None, 48_000, name);
    };
    let cfg = range.with_sample_rate(rate_of(&range));
    let (rate, ch, fmt) = (cfg.sample_rate(), cfg.channels() as usize, cfg.sample_format());
    let config: cpal::StreamConfig = cfg.into();
    println!("lightscope: {name}, {rate} Hz, {ch} channels, {fmt:?}; the first channel shown");
    let err = |e| eprintln!("lightscope: stream error {e}");
    // Each format's samples to f32 in -1..1, the first channel of each frame, into the ring.
    fn push(ring: &Mutex<Ring>, ch: usize, it: impl Iterator<Item = f32>) {
        let mut r = ring.lock().unwrap();
        for (k, s) in it.enumerate() {
            if k % ch == 0 {
                let w = r.written;
                r.buf[w & (RING - 1)] = s;
                r.written = w + 1;
            }
        }
    }
    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_input_stream(config, move |d: &[f32], _: &_| push(&ring, ch, d.iter().copied()), err, None),
        cpal::SampleFormat::I16 => dev.build_input_stream(config, move |d: &[i16], _: &_| push(&ring, ch, d.iter().map(|&s| s as f32 * (1. / (1u32 << 15) as f32))), err, None),
        cpal::SampleFormat::I32 => dev.build_input_stream(config, move |d: &[i32], _: &_| push(&ring, ch, d.iter().map(|&s| (s as f64 * (1. / (1u64 << 31) as f64)) as f32)), err, None),
        f => {
            eprintln!("lightscope: sample format {f:?} not read");
            return (None, rate, name);
        }
    };
    match stream {
        Ok(s) => {
            let _ = s.play();
            (Some(s), rate, name)
        }
        Err(e) => {
            eprintln!("lightscope: {e}");
            (None, rate, name)
        }
    }
}

/// The window's levels, threshold, crossings, duty and shortest stretches.
fn analyse(x: &[f32], rate: u32, trig: f32) -> Reading {
    let mut r = Reading { lo: f32::MAX, hi: f32::MIN, ..Default::default() };
    for &s in x {
        r.lo = r.lo.min(s);
        r.hi = r.hi.max(s);
        // Full scale on either side: the input is clipping.
        if s.abs() >= 1. - 1. / (1 << 8) as f32 {
            r.clipped += 1;
        }
    }
    let swing = r.hi - r.lo;
    r.threshold = r.lo + swing * trig;
    if swing <= 0. {
        return r;
    }
    let hyst = swing * (1. / (1 << 4) as f32);
    let (up, down) = (r.threshold + hyst * 0.5, r.threshold - hyst * 0.5);
    let mut on = x[0] > r.threshold;
    let mut since = 0usize;
    let (mut rises, mut first_rise, mut last_rise) = (0usize, 0usize, 0usize);
    let (mut on_min, mut off_min) = (usize::MAX, usize::MAX);
    let mut above = 0usize;
    // The first stretch is cut by the window's start: not counted as a whole stretch.
    let mut whole = false;
    for (i, &s) in x.iter().enumerate() {
        if on {
            above += 1;
        }
        let flip = if on { s < down } else { s > up };
        if flip {
            if whole {
                let len = i - since;
                if on {
                    on_min = on_min.min(len);
                } else {
                    off_min = off_min.min(len);
                }
            }
            whole = true;
            since = i;
            on = !on;
            if on {
                if rises == 0 {
                    first_rise = i;
                }
                last_rise = i;
                rises += 1;
            }
        }
    }
    let us = 1e6 / rate as f64;
    r.duty = above as f64 / x.len() as f64;
    if rises >= 2 {
        r.freq = (rises - 1) as f64 * rate as f64 / (last_rise - first_rise) as f64;
    }
    r.on_min_us = if on_min == usize::MAX { 0. } else { on_min as f64 * us };
    r.off_min_us = if off_min == usize::MAX { 0. } else { off_min as f64 * us };
    r
}

fn level_db(v: f32) -> f64 {
    if v <= 0. { -144. } else { 20. * (v as f64).log10() }
}

impl Scope {
    fn new() -> Self {
        let mut hit_counter: HitId = HIT_NONE;
        let chrome = DefaultChrome::new(Viewport::new(1200, 700), "lightscope".to_string(), None, Some("".to_string()), &mut hit_counter);
        let timebase = Slider::new(&mut hit_counter, 0., 0., 1., 1., ((-8. - SPAN_LO) / SPAN_OCTAVES) as f32);
        let trigger = Slider::new(&mut hit_counter, 0., 0., 1., 1., 0.5);
        let ring = Arc::new(Mutex::new(Ring { buf: vec![0.; RING], written: 0 }));
        let want = std::env::args().nth(1);
        let (stream, rate, device) = open_input(want.as_deref(), ring.clone());
        Scope { title: "lightscope".into(), chrome, timebase, trigger, hit_counter, dragging: None, ring, rate, device, _stream: stream, frozen: None, reading: Reading::default(), plot: (0, 0, 1, 1) }
    }

    fn span_s(&self) -> f64 {
        (SPAN_LO + self.timebase.value() as f64 * SPAN_OCTAVES).exp2()
    }

    fn layout(&mut self, ctx: &mut Context) {
        let (w, h) = (ctx.viewport.width_px as Coord, ctx.viewport.height_px as Coord);
        let bw = ctx.viewport.effective_span() / 32.;
        let margin = w * (1. / (1 << 5) as Coord);
        let top = h * 0.16;
        let bottom = h * 0.80;
        self.plot = (margin as usize, top as usize, (w - margin * 2.) as usize, (bottom - top) as usize);
        let sw = w * 0.36;
        self.timebase.set_rect(margin + w * 0.12 + sw * 0.5, h * 0.88, sw, bw);
        self.trigger.set_rect(w * 0.58 + sw * 0.5, h * 0.88, sw, bw);
    }

    /// The samples to look at: the frozen copy, or the newest window of the ring.
    fn window(&self, n: usize) -> Vec<f32> {
        if let Some(f) = &self.frozen {
            return f.clone();
        }
        let r = self.ring.lock().unwrap();
        let n = n.min(r.written).min(RING);
        let start = r.written - n;
        (start..r.written).map(|i| r.buf[i & (RING - 1)]).collect()
    }

    fn readout(&self) -> String {
        let r = self.reading;
        let p2p = r.hi - r.lo;
        format!(
            "{:.1} Hz   duty {:.2} %   shortest on {:.0} µs, off {:.0} µs   p-p {:.1} dBFS{}   {} Hz   {}",
            r.freq,
            r.duty * 100.,
            r.on_min_us,
            r.off_min_us,
            level_db(p2p * 0.5),
            if r.clipped > 0 { "   CLIPPING" } else { "" },
            self.rate,
            self.device
        )
    }

    fn draw_trace(&self, target: &mut [u32], buf_w: usize, x: &[f32], damage: &mut fluor::canvas::Damage) {
        let (px, py, pw, ph) = self.plot;
        let mut canvas = Canvas::new(target, buf_w, py + ph, damage);
        let trace = pack_argb(96, 255, 128, 255);
        let thresh = pack_argb(255, 160, 64, 160);
        let grid = pack_argb(48, 56, 72, 255);
        let span = self.span_s();
        let screen = ((span * self.rate as f64) as usize).max(2);
        let r = self.reading;
        let (lo, hi) = (r.lo, r.hi);
        let swing = if hi > lo { hi - lo } else { 1. };
        // A sixteenth of the plot's height above and below the swing.
        let pad = swing * (1. / (1 << 4) as f32);
        let (vlo, vrange) = (lo - pad, swing + pad * 2.);
        let row = |v: f32| -> isize { py as isize + ((1. - (v - vlo) / vrange) * (ph - 1) as f32) as isize };
        // The trigger: the latest rising crossing with a whole screen after it.
        let mut start = x.len().saturating_sub(screen);
        if x.len() > screen + 1 {
            let mut i = x.len() - screen - 1;
            while i > 0 {
                if x[i - 1] <= r.threshold && x[i] > r.threshold {
                    start = i;
                    break;
                }
                i -= 1;
            }
        }
        let seen = &x[start..(start + screen).min(x.len())];
        let per_col = seen.len() as f64 / pw as f64;
        // Front to back: the trace, the threshold, the grid; the plot's black last.
        for col in 0..pw {
            let a = (col as f64 * per_col) as usize;
            let b = (((col + 1) as f64 * per_col).ceil() as usize).max(a + 1);
            // WHY the bound: the last column's end can round past the window.
            let b = b.min(seen.len());
            if a >= b {
                continue;
            }
            // The column's lowest and highest sample, joined to the next column's first so a fast edge stays a line.
            let mut cmin = f32::MAX;
            let mut cmax = f32::MIN;
            for &s in &seen[a..(b + 1).min(seen.len())] {
                cmin = cmin.min(s);
                cmax = cmax.max(s);
            }
            let (y0, y1) = (row(cmax), row(cmin));
            fluor::paint::fill_rect(&mut canvas, (px + col) as isize, y0, 0, y1 - y0 + 1, trace, None, None);
        }
        fluor::paint::fill_rect(&mut canvas, px as isize, row(r.threshold), pw as isize, 0, thresh, None, None);
        // Grid: the screen in eighths of its span.
        for k in 1..8 {
            fluor::paint::fill_rect(&mut canvas, (px + pw * k / 8) as isize, py as isize, 0, ph as isize, grid, None, None);
        }
        fluor::paint::fill_rect(&mut canvas, px as isize, py as isize, pw as isize, ph as isize, pack_argb(0, 0, 0, 255), None, None);
    }
}

impl Container for Scope {
    fn visit(&mut self, f: &mut dyn FnMut(&mut dyn Widget)) {
        f(&mut self.timebase);
        f(&mut self.trigger);
        self.chrome.visit(f);
    }
}

impl FluorApp for Scope {
    type UserEvent = ();

    fn title(&self) -> &str {
        &self.title
    }

    fn init(&mut self, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.layout(ctx);
    }

    fn on_resize(&mut self, _w: u32, _h: u32, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.chrome.set_full_edge(ctx.is_maximized);
        self.layout(ctx);
    }

    /// Every frame while live: the scope follows the input.
    fn wake_at(&self) -> Option<Instant> {
        if self.frozen.is_none() { Some(Instant::now()) } else { None }
    }

    fn tick(&mut self, _ctx: &mut Context) -> bool {
        self.frozen.is_none()
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        match event {
            Event::CursorMoved { .. } => {
                let (x, y) = (ctx.cursor_x, ctx.cursor_y);
                if let Some(i) = self.dragging {
                    if i == 0 { self.timebase.set_value_from_x(x) } else { self.trigger.set_value_from_x(x) }
                    let _ = self.timebase.take_change() | self.trigger.take_change();
                    ctx.window.request_redraw();
                    return EventResponse::Handled;
                }
                if self.chrome.set_hover(self.chrome.hit_at(x, y)) {
                    ctx.window.request_redraw();
                }
                EventResponse::Pass
            }
            Event::MouseInput { state: ElementState::Pressed, button: MouseButton::Left } => {
                let (x, y) = (ctx.cursor_x, ctx.cursor_y);
                let hit = self.chrome.hit_at(x, y);
                if hit != HIT_NONE {
                    let response = widget::dispatch_click(self as &mut dyn Container, hit, x, y, Default::default());
                    self.dragging = if hit == self.timebase.hit_id() { Some(0) } else if hit == self.trigger.hit_id() { Some(1) } else { None };
                    let _ = self.timebase.take_change() | self.trigger.take_change();
                    ctx.window.request_redraw();
                    return response;
                }
                let edge = chrome::get_resize_edge(ctx.viewport, x, y);
                if edge != ResizeEdge::None {
                    return EventResponse::StartResize(edge);
                }
                EventResponse::StartWindowDrag
            }
            Event::MouseInput { state: ElementState::Released, .. } => {
                if self.dragging.take().is_some() {
                    return EventResponse::Handled;
                }
                EventResponse::Pass
            }
            Event::KeyboardInput { event: KeyEvent { state: ElementState::Pressed, logical_key, .. } } => {
                match logical_key {
                    Key::Named(NamedKey::Space) => {
                        self.frozen = if self.frozen.is_some() { None } else { Some(self.window(self.analysis_len())) };
                    }
                    Key::Character(c) if c.eq_ignore_ascii_case("l") => println!("{}", self.readout()),
                    _ => return EventResponse::Pass,
                }
                ctx.window.request_redraw();
                EventResponse::Handled
            }
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
        let (buf_w, buf_h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
        let x = self.window(self.analysis_len());
        if x.len() >= 2 {
            self.reading = analyse(&x, self.rate, self.trigger.value());
        }
        self.chrome.rasterize_bg(ctx.damage, |canvas| canvas.pixels.fill(pack_argb(10, 14, 22, 255)));
        self.chrome.rasterize_perimeter(target, buf_w, buf_h, ctx.clip_mask);
        self.chrome.rasterize_chrome(ctx.damage, ctx.text, ctx.clip_mask);
        let span = ctx.viewport.effective_span();
        let font = span / 40.;
        let label = pack_argb(200, 208, 220, 255);
        let (px, py, _, ph) = self.plot;
        let lines = [
            (self.readout(), py as Coord * 0.62, font),
            (format!("{} per screen, {} per line{}", fmt_time(self.span_s()), fmt_time(self.span_s() / 8.), if self.frozen.is_some() { "   FROZEN (space)" } else { "" }), (py + ph) as Coord + font * 1.2, font * 0.85),
        ];
        for (t, y, f) in lines {
            let mut canvas = Canvas::new(target, buf_w, buf_h, ctx.damage);
            ctx.text.draw_text_left(&mut canvas, &t, px as Coord, y, &TextStyle::new(f, label), None, None);
        }
        for (t, s) in [("timebase", &self.timebase), ("trigger", &self.trigger)] {
            let b = s.bbox();
            let mut canvas = Canvas::new(target, buf_w, buf_h, ctx.damage);
            ctx.text.draw_text_left(&mut canvas, t, b.x - font * 4.5, b.y + b.h * 0.5, &TextStyle::new(font * 0.85, label), None, None);
        }
        for s in [&mut self.timebase, &mut self.trigger] {
            let mut canvas = Canvas::new(target, buf_w, buf_h, ctx.damage);
            let id = s.hit_id();
            s.render_content_into(&mut canvas, Some(&mut self.chrome.hit_test_map), id);
        }
        if x.len() >= 2 {
            self.draw_trace(target, buf_w, &x, ctx.damage);
        }
        self.chrome.flatten_into(target, buf_w, buf_h, None);
    }

    fn cursor_for(&self, x: Coord, y: Coord, ctx: &Context) -> CursorIcon {
        let hit = self.chrome.hit_at(x, y);
        if self.chrome.owns_hit(hit) || hit == self.timebase.hit_id() || hit == self.trigger.hit_id() {
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

impl Scope {
    /// Four screens, at least `ANALYSIS_MIN` samples, at most the ring.
    fn analysis_len(&self) -> usize {
        ((self.span_s() * self.rate as f64) as usize * 4).max(ANALYSIS_MIN).min(RING)
    }
}

fn fmt_time(s: f64) -> String {
    if s >= 1e-3 { format!("{:.2} ms", s * 1e3) } else { format!("{:.0} µs", s * 1e6) }
}

fn main() {
    fluor::host::app::run_app(Scope::new()).expect("event loop");
}
