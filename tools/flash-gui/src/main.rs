//! ferros-flash-gui: the ferros installer as people see it (docs/FLASHER.md "The GUI", docs/BRAND.md).
//! A fluor window on the same library as the ferros-flash CLI: the same detection over nusb, the same signed release, the same VSF run log.
//! Screen 0 is the welcome: the mark, "Consent.", three dim lines, the box that sends this run's log (unticked; ticking it unfolds its two choices and the plain facts), the device silhouette that is the live detector, the honest wipe line.
//! Screen 1, after "Install ferros", says what this does and the steps ahead.
//! Nothing is sent unless the box is ticked: then the run's report goes to fgtw.org at a failure ("only when something fails") or at the run's end ("the whole run"), and the answer shows in a small line at the bottom of whatever screen is up.
//! Everything is drawn front to back into one frame (fluor's `under`): the words first, then the shapes, the glow behind the title, and the near-black canvas last.
//! The glow breathes and the choices unfold with the shown frames (one step a frame, no clock), the silhouette changes on USB events (no poll), and nothing happens on a timer.
//! Usage: ferros-flash-gui [--release <https base or a staged directory>] [--snap <out.vsf>] [--steps] [--pretend <none|noaccess|android|bootloader|ferros>]
//! `--snap` writes the first settled frame as a VSF visual (snap.rs) and keeps running; `--steps` opens on screen 1 and `--pretend` shows a device state in place of the detector (which is not started), both for looking at the screens without a device on the cable.

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod detect;
mod fetch;
mod mark;
mod snap;
mod tick;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use detect::Device;
use ferros_flash::runlog::{self, Ev, Level, RunLog};
use fluor::canvas::Canvas;
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, ModifiersState, MouseButton};
use fluor::geom::Viewport;
use fluor::host::WakeSender;
use fluor::host::app::{Context, EventResponse, FluorApp, run_app};
use fluor::host::chrome::{ResizeEdge, get_resize_edge, strip_height};
use fluor::host::chrome_widget::DefaultChrome;
use fluor::host::widget::{Container, Widget, apply_hover, apply_pressed, build_overlay_deltas, dispatch_release};
use fluor::paint::{self, HIT_NONE, HitId, draw_squircle_rrect, fill_rect};
use fluor::pixel::{Blend, BlendMode, G2t16, Pixel};
use fluor::text::{TextRenderer, TextStyle};
use fluor::widgets::Button;
use mark::Mark;
use tick::Tick;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where the release lives unless `--release` names another place (tools/flash/publish.sh).
const BASE: &str = "https://get.ferros.org";

/// A colour from three 8-bit VSF RGB g2t codes, as fluor stores it: α full, darkness the codes' complement ("Claude memories/feedback_vsf_rgb_only.md"; never sRGB).
const fn g2t(r: u32, g: u32, b: u32) -> u32 {
    fluor::theme::dark((r << 16) | (g << 8) | b)
}
/// The canvas: near black, a hair warm.
const BG: u32 = g2t(9, 9, 10);
/// The headline and the words inside the silhouette.
const TITLE: u32 = g2t(198, 192, 180);
/// The three dim lines.
const BODY: u32 = g2t(118, 114, 106);
/// The bottom line, the log's facts and the answer line.
const QUIET: u32 = g2t(84, 82, 78);
/// The one warm glow.
const GLOW: u32 = g2t(232, 150, 64);
const GREY: u32 = g2t(98, 98, 100);
const AMBER: u32 = g2t(216, 152, 48);
const WARM: u32 = g2t(228, 178, 120);
const ORANGE: u32 = g2t(238, 120, 28);
/// The button's resting fill; hover and held are derived from it by fluor.
const BUTTON: u32 = g2t(92, 56, 24);
/// The dot of a ticked box or a chosen radio button: the warm light's own colour, as a fill.
const TICK: u32 = GLOW;

/// The glow's breathing period in shown frames: 2^9 frames, a slow breath at a desktop's rate.
const BREATH_FRAMES: u32 = 1 << 9;

/// What the window's threads send it: a detector change, the release, the answer to a sent log.
enum UserEvent {
    Device(Device),
    Release(Result<fetch::Fetched, String>),
    Sent(Result<[u8; 32], String>),
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Welcome,
    Steps,
}

/// The box's two choices (the owner, 2026-11-28): send at each failure, or once at the run's end.
#[derive(Clone, Copy, PartialEq)]
enum When {
    Fails,
    Whole,
}

impl When {
    fn words(self) -> &'static str {
        match self {
            When::Fails => "only when something fails",
            When::Whole => "the whole run",
        }
    }
    fn id(self) -> &'static str {
        match self {
            When::Fails => "fails",
            When::Whole => "whole",
        }
    }
}

/// The person's consent as it stands, read by the panic hook as well as the window: SEND_NONE unless the box is ticked.
static CONSENT: AtomicU8 = AtomicU8::new(SEND_NONE);
const SEND_NONE: u8 = 0;
const SEND_FAILS: u8 = 1;
const SEND_WHOLE: u8 = 2;

/// The last send's state, shown in a small line at the bottom of whatever screen is up.
enum Answer {
    Sending,
    Sent([u8; 32]),
    NotSent(String),
}

/// The words the box says next to it, on screen 0.
const CONSENT_WORDS: &str = "Send this run's log to help fix things";
/// The plain facts under the two choices, one short dim line each (docs/BRAND.md "The voice": what it holds and where it goes, said in full).
const FACTS: [&str; 5] = [
    "It goes to fgtw.org/report: one document of what this tool did and what the device answered.",
    "It never holds a handle, a key, a location or your files.",
    "The device's serial is replaced by a short hash.",
    "fgtw.org keeps it 32 days.",
    "Your computer's address is not kept.",
];

/// Everything that takes a click: the chrome's own controls and this window's buttons and boxes.
struct Ui {
    chrome: DefaultChrome<G2t16>,
    /// "Install ferros", present only while a device is seen on the welcome.
    install: Option<Button<G2t16>>,
    /// The box on screen 0, unticked at the start of every run.
    consent: Tick,
    /// Its two choices, unfolded under it while it is ticked; they take hits only then.
    fails: Tick,
    whole: Tick,
}

impl Container for Ui {
    fn visit(&mut self, f: &mut dyn FnMut(&mut dyn Widget)) {
        self.chrome.visit(f);
        if let Some(b) = &mut self.install {
            f(b);
        }
        for t in [&mut self.consent, &mut self.fails, &mut self.whole] {
            f(t);
        }
    }
}

/// The run's log, shared with the panic hook and the threads.
type Log = Arc<Mutex<RunLog>>;

fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The log, opened as the CLI opens its own: one file a run, the start recorded, a crash recorded before the process ends.
fn open_log() -> Log {
    let start = vsf::eagle_time_oscillations();
    let facts = Ev::info("start", "ferros-flash-gui {} on {}").t(VERSION).t(&platform()).id("tool", VERSION).id("platform", &platform());
    let l = RunLog::create(&runlog::default_dir(), start, facts).unwrap_or_else(|e| {
        eprintln!("ferros-flash-gui: cannot keep a log: {e}");
        std::process::exit(1)
    });
    let path = l.path().to_path_buf();
    let log = Arc::new(Mutex::new(l));
    let hook = log.clone();
    std::panic::set_hook(Box::new(move |p| {
        let at = p.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = p.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        if let Ok(mut l) = hook.lock() {
            let _ = l.append(&Ev::error("crash", "the window crashed at {}: {}").t(&at).t(&msg));
            eprintln!("ferros-flash-gui crashed ({msg} at {at}); the log is {}", l.path().display());
        }
        // A crash is a failure and the run's end at once, so either choice covers it.
        // Sent here, on the crashing thread and before the process ends, since no thread outlives the window.
        if CONSENT.load(Ordering::Relaxed) != SEND_NONE {
            let r = post_report(&path);
            if let Ok(mut l) = hook.lock() {
                let _ = l.append(&match &r {
                    Ok(h) => Ev::info("send", "fgtw.org took it").hash("report", h),
                    Err(e) => Ev::error("send", "not sent: {}").t(e),
                });
            }
        }
    }));
    log
}

/// This run's report as it stands, posted to fgtw.org: the report's BLAKE3 when taken, else why not.
fn post_report(path: &std::path::Path) -> Result<[u8; 32], String> {
    let run = std::fs::read(path).map_err(|e| e.to_string())?;
    let doc = runlog::report(&run, &format!("ferros-flash-gui {VERSION} {}", platform()), vsf::eagle_time_oscillations())?;
    // A deadline on the post: from the panic hook it runs on the crashing thread before the process ends, and a stalled connection must not hold the dying window open. 2^4 s, long enough for the report on any live link.
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(1 << 4)).build();
    let r = agent.post(runlog::REPORT_URL).set("Content-Type", "application/octet-stream").send_bytes(&doc).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut r.into_reader(), &mut out).map_err(|e| e.to_string())?;
    runlog::reply(&out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// `c` at a fraction `f` (ZERO to ONE) of its opacity: fluor's `under` takes a colour's darkness and its α apart, so scaling the α alone is the same colour, fainter.
fn fade(c: u32, f: f32) -> u32 {
    ((((c >> 24) as f32 * f) as u32) << 24) | (c & 0xFF_FFFF)
}

fn note(log: &Log, ev: Ev) {
    if let Ok(mut l) = log.lock() {
        let _ = l.append(&ev);
    }
}

/// The window.
struct App {
    ui: Ui,
    hits: HitId,
    proxy: Option<Arc<dyn WakeSender<UserEvent>>>,
    log: Log,
    base: String,
    screen: Screen,
    device: Device,
    mark: Option<Mark>,
    /// The release fetch has answered, one way or the other.
    release_settled: bool,
    /// Shown frames, the glow's phase.
    frame: u32,
    /// The VSF capture still to write, and whether a frame has been drawn since the release settled.
    snap: Option<String>,
    /// The box's choice; it counts only while the box is ticked.
    when: When,
    /// How far the choices under the box have unfolded, ZERO folded to ONE open: each shown frame halves what is left of the way.
    unfold: f32,
    /// A failure was logged while no consent covered it: ticking "only when something fails" sends the run then.
    unsent_fail: bool,
    answer: Option<Answer>,
    viewport: Viewport,
}

/// Where everything sits, from the window's size alone: fractions of the height under the chrome's strip, sizes from the span and the width, never a pixel count.
struct Layout {
    w: f32,
    y0: f32,
    hh: f32,
    span: f32,
    margin: f32,
    body: f32,
    quiet: f32,
}

impl Layout {
    fn of(v: Viewport) -> Layout {
        let (w, h) = (v.width_px as f32, v.height_px as f32);
        let y0 = strip_height(v);
        Layout { w, y0, hh: h - y0, span: v.effective_span(), margin: w / (1 << 4) as f32, body: w / 50., quiet: w / 48. }
    }
    fn cx(&self) -> f32 {
        self.w * 0.5
    }
    /// A height fraction under the strip.
    fn y(&self, f: f32) -> f32 {
        self.y0 + self.hh * f
    }
    fn title_size(&self) -> f32 {
        self.span / 9.
    }
    fn mark_side(&self) -> usize {
        (self.span / (1 << 3) as f32) as usize
    }
}

impl App {
    fn new(base: String, snap: Option<String>, screen: Screen, device: Device, log: Log) -> App {
        let mut hits = HIT_NONE;
        let chrome = DefaultChrome::with_depth(Viewport::new(1, 1), "ferros", None, None, &mut hits);
        let consent = Tick::new(&mut hits, false, false);
        let fails = Tick::new(&mut hits, true, true);
        let whole = Tick::new(&mut hits, true, false);
        App {
            ui: Ui { chrome, install: None, consent, fails, whole },
            hits,
            proxy: None,
            log,
            base,
            screen,
            device,
            mark: None,
            release_settled: false,
            frame: 0,
            snap,
            when: When::Fails,
            unfold: 0.,
            unsent_fail: false,
            answer: None,
            viewport: Viewport::new(1, 1),
        }
    }

    /// The buttons for this screen and this device, at this size: made again whenever any of those change (a Button keeps its caches by size, so a fresh one at a new size is the plain way).
    fn place_buttons(&mut self) {
        let l = Layout::of(self.viewport);
        let bh = l.body * 2.4;
        self.ui.install = (self.screen == Screen::Welcome && self.device.seen()).then(|| {
            let mut b = Button::with_depth(&mut self.hits, l.cx(), l.y(0.845), l.body * 10., bh, l.body * 1.1, "Install ferros");
            b.set_fill(Some(BUTTON));
            b
        });
    }

    /// The silhouette's colour for the device's state.
    fn device_colour(&self) -> u32 {
        match self.device {
            Device::None | Device::Failed { .. } => GREY,
            Device::NoAccess { .. } => AMBER,
            Device::Android { .. } | Device::Bootloader { .. } => WARM,
            Device::Ferros => ORANGE,
        }
    }

    /// The consent as it stands: none unless the box is ticked, else its choice.
    fn consent(&self) -> Option<When> {
        self.ui.consent.on.then_some(self.when)
    }

    /// A record into the run's log, and a failure (an error of any step but the send's own, so a send that fails does not send again) sent when the person chose "only when something fails".
    fn record(&mut self, ev: Ev) {
        let fail = ev.level() == Level::Error && ev.step() != "send";
        note(&self.log, ev);
        if fail {
            match self.consent() {
                Some(When::Fails) => self.send_log("a failure"),
                _ => self.unsent_fail = true,
            }
        }
    }

    /// The box or a choice changed: the radio buttons follow it, the panic hook sees it, and a failure no consent covered yet goes now if the choice is the failures.
    fn consent_changed(&mut self) {
        self.ui.fails.on = self.when == When::Fails;
        self.ui.whole.on = self.when == When::Whole;
        let c = match self.consent() {
            None => SEND_NONE,
            Some(When::Fails) => SEND_FAILS,
            Some(When::Whole) => SEND_WHOLE,
        };
        CONSENT.store(c, Ordering::Relaxed);
        if self.unsent_fail && self.consent() == Some(When::Fails) {
            self.send_log("a failure before the box was ticked");
        }
    }

    /// The report sent to fgtw.org on its own thread, the answer coming back as a user event; why it is sent goes into the run's own log first.
    fn send_log(&mut self, why: &str) {
        let path = self.log.lock().map(|l| l.path().to_path_buf()).unwrap();
        note(&self.log, Ev::info("send", "sending this run's log to fgtw.org: {}").t(why).id("when", self.when.id()));
        self.unsent_fail = false;
        let Some(proxy) = self.proxy.clone() else { return };
        self.answer = Some(Answer::Sending);
        std::thread::spawn(move || {
            let _ = proxy.send(UserEvent::Sent(post_report(&path)));
        });
    }
}

/// Greedy word wrap by measured width: lines of `words` no wider than `max`, a word wider than that on a line of its own.
fn wrap(text: &mut TextRenderer, words: &str, style: &TextStyle, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in words.split_whitespace() {
        let trial = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && text.measure_text(&trial, style) > max {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = trial;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A soft warm light centred at `(cx, cy)`, `peak` of ONE at its centre, falling as 1 / (1 + d²) with d the elliptical distance in `(sx, sy)`: C∞ everywhere, no edge, no clamp ("GUI continuity" in AGENT.md).
/// Composed under what is already in the canvas, so it lights the words in front of it and nothing behind an opaque pixel.
fn glow(canvas: &mut Canvas<G2t16>, cx: f32, cy: f32, sx: f32, sy: f32, peak: f32, colour: u32) {
    let (w, h) = (canvas.width, canvas.height);
    let light = G2t16::from_g2t8(colour).rgb();
    let peak = peak * G2t16::ONE as f32;
    let (isx, isy) = (1. / sx, 1. / sy);
    canvas.damage.add_bounds(0, 0, w, h);
    for y in 0..h {
        let dy = (y as f32 + 0.5 - cy) * isy;
        let row = &mut canvas.pixels[y * w..(y + 1) * w];
        for (x, px) in row.iter_mut().enumerate() {
            let dx = (x as f32 + 0.5 - cx) * isx;
            // The cast truncates toward zero: a peak below ONE keeps every value inside the α field.
            let a = (peak / (1. + dx * dx + dy * dy)) as u32;
            if a != 0 {
                *px = px.under(light.with_alpha(a), BlendMode::Normal);
            }
        }
    }
}

/// A height the layout leaves for the silhouette, kept positive: `(d + √(d² + e²)) / 2`, which is `d` itself once `d` is a few `e` above zero and falls smoothly toward zero below it, C∞ everywhere (no clamp, AGENT.md "GUI continuity").
/// WHY: the room left is the layout's remainder, and a window much wider than tall (the fonts scale with the width) or the unfolded facts take more than there is, so `d` goes negative.
/// PROOF: at a width three times the height the three lines alone reach past three quarters of it; a negative height would hand the text renderer a negative font size for the silhouette's words.
fn soft_height(d: f32, e: f32) -> f32 {
    (d + (d * d + e * e).sqrt()) * 0.5
}

/// The device silhouette: a soft squircle outline, `stroke` wide, in `colour`, with the state's word and its line inside it.
/// Front to back: the words, then the inside in the canvas colour (so the outline is a ring), then the outline.
fn silhouette(canvas: &mut Canvas<G2t16>, text: &mut TextRenderer, cx: f32, cy: f32, h: f32, colour: u32, word: &str, under: &str, word_size: f32) {
    let w = h * 0.5;
    let stroke = h / (1 << 6) as f32;
    let under_size = word_size * 0.62;
    if under.is_empty() {
        text.draw_text_center(canvas, word, cx, cy, &TextStyle::new(word_size, colour).weight(600), None, None);
    } else {
        text.draw_text_center(canvas, word, cx, cy - under_size * 0.7, &TextStyle::new(word_size, colour).weight(600), None, None);
        text.draw_text_center(canvas, under, cx, cy + word_size * 0.75, &TextStyle::new(under_size, BODY), None, None);
    }
    // Each edge placed from the centre in floats and cast once, so the ring is the same width on both sides (an inset of a cast width was a pixel thicker on the right).
    let (x0, x1, y0, y1) = ((cx - w * 0.5) as isize, (cx + w * 0.5) as isize, (cy - h * 0.5) as isize, (cy + h * 0.5) as isize);
    let (ix0, ix1, iy0, iy1) = ((cx - w * 0.5 + stroke) as isize, (cx + w * 0.5 - stroke) as isize, (cy - h * 0.5 + stroke) as isize, (cy + h * 0.5 - stroke) as isize);
    let r = w * 0.22;
    draw_squircle_rrect(canvas, ix0, iy0, ix1 - ix0, iy1 - iy0, BG, [r - stroke; 4], paint::WINDOW_SHAPE_POWER);
    draw_squircle_rrect(canvas, x0, y0, x1 - x0, y1 - y0, colour, [r; 4], paint::WINDOW_SHAPE_POWER);
}

impl FluorApp<G2t16> for App {
    type UserEvent = UserEvent;

    fn title(&self) -> &str {
        "ferros"
    }

    fn initial_size(&self, monitor: (u32, u32)) -> (u32, u32) {
        // A tall card: three quarters of the screen's height, nine tenths as wide as tall.
        let h = monitor.1 * 3 / 4;
        (h * 9 / 10, h)
    }

    fn set_event_proxy(&mut self, proxy: Arc<dyn WakeSender<UserEvent>>) {
        self.proxy = Some(proxy);
    }

    fn init(&mut self, ctx: &mut Context) {
        self.viewport = ctx.viewport;
        self.ui.chrome.resize(ctx.viewport);
        self.place_buttons();
        let Some(proxy) = self.proxy.clone() else { return };
        // A pretended device (`--pretend`) stands in for the detector; otherwise the detector runs for the life of the window.
        if self.device == Device::None {
            let p = proxy.clone();
            detect::start(move |d| p.send(UserEvent::Device(d)).is_ok());
        }
        fetch::start(self.base.clone(), move |r| {
            let _ = proxy.send(UserEvent::Release(r));
        });
    }

    fn on_resize(&mut self, _w: u32, _h: u32, ctx: &mut Context) {
        self.viewport = ctx.viewport;
        self.ui.chrome.resize(ctx.viewport);
        self.place_buttons();
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        match event {
            Event::CursorMoved { x, y } => {
                let hit = self.ui.chrome.hit_at(*x, *y);
                if self.ui.chrome.set_hover(hit) {
                    apply_hover(&mut self.ui, hit);
                    ctx.window.request_redraw();
                }
                EventResponse::Handled
            }
            Event::CursorLeft => {
                if self.ui.chrome.set_hover(HIT_NONE) {
                    apply_hover(&mut self.ui, HIT_NONE);
                    ctx.window.request_redraw();
                }
                EventResponse::Handled
            }
            Event::MouseInput { state: ElementState::Pressed, button: MouseButton::Left } => {
                // Every press goes in the log with what was under it: the one record that tells a dead button from a missed one.
                let hit = self.ui.chrome.hit_at(ctx.cursor_x, ctx.cursor_y);
                note(&self.log, Ev::info("press", "press at {} {} on hit {}").n(ctx.cursor_x as u64).n(ctx.cursor_y as u64).n(hit as u64));
                let edge = get_resize_edge(ctx.viewport, ctx.cursor_x, ctx.cursor_y);
                if edge != ResizeEdge::None {
                    EventResponse::StartResize(edge)
                } else if self.ui.chrome.hit_at(ctx.cursor_x, ctx.cursor_y) == HIT_NONE {
                    EventResponse::StartWindowDrag
                } else {
                    EventResponse::Handled
                }
            }
            _ => EventResponse::Pass,
        }
    }

    fn on_activate(&mut self, hit: HitId, x: Coord, y: Coord, mods: ModifiersState, _ctx: &mut Context) -> EventResponse {
        let r = dispatch_release(&mut self.ui, hit, x, y, mods);
        if self.ui.install.as_mut().is_some_and(|b| b.take_click()) {
            note(&self.log, Ev::info("screen", "Install ferros: the steps screen").id("device", self.device.word()));
            self.screen = Screen::Steps;
            self.place_buttons();
            // Today the run ends here: nothing past screen 1 is built ("This build goes no further yet"), so reaching it is the end "the whole run" waits for.
            // When the steps after it are built, this send moves to the real end.
            if self.consent() == Some(When::Whole) {
                self.send_log("the run's end");
            }
        }
        // The consent, each tick and untick recorded in the run's own log as it happens.
        if self.ui.consent.take_click() {
            self.ui.consent.on = !self.ui.consent.on;
            note(
                &self.log,
                if self.ui.consent.on {
                    Ev::info("consent", "the person ticked the box: send this run's log {}").t(self.when.words()).flag("send", true).id("when", self.when.id())
                } else {
                    Ev::info("consent", "the person unticked the box: nothing is sent").flag("send", false)
                },
            );
            self.consent_changed();
        }
        for (picked, when) in [(self.ui.fails.take_click(), When::Fails), (self.ui.whole.take_click(), When::Whole)] {
            if picked && self.when != when {
                self.when = when;
                note(&self.log, Ev::info("consent", "the person chose to send this run's log {}").t(when.words()).flag("send", true).id("when", when.id()));
                self.consent_changed();
            }
        }
        r
    }

    fn on_user_event(&mut self, event: UserEvent, _ctx: &mut Context) -> EventResponse {
        match event {
            UserEvent::Device(d) => {
                let ev = match &d {
                    Device::None => Ev::info("device", "no device on USB"),
                    Device::NoAccess { why } => Ev::warn("device", "a Google device this computer cannot open: {}").t(why),
                    Device::Android { model } => Ev::info("device", "{} in Android").t(model).id("model", model),
                    Device::Bootloader { model } => Ev::info("device", "{} in its bootloader").t(model).id("model", model),
                    Device::Ferros => Ev::info("device", "a device running ferros"),
                    Device::Failed { why } => Ev::error("device", "USB cannot be read: {}").t(why),
                };
                self.record(ev);
                self.device = d;
                self.place_buttons();
            }
            UserEvent::Release(r) => {
                match r {
                    Ok(f) => {
                        let commit: String = f.release.commit.iter().map(|b| format!("{b:02x}")).collect();
                        note(&self.log, Ev::info("release", "the release is signed: channel {}, commit {}, A#{} files").t(&f.release.channel).t(&commit).n(f.release.artefacts.len() as u64).id("channel", &f.release.channel).id("commit", &commit));
                        match f.logo.as_deref().map(Mark::read) {
                            Some(Ok(m)) => self.mark = Some(m),
                            Some(Err(e)) => note(&self.log, Ev::warn("release", "the mark does not read: {}").t(&e)),
                            None => note(&self.log, Ev::info("release", "no mark in this release")),
                        }
                    }
                    Err(e) => note(&self.log, Ev::warn("release", "no release: {}").t(&e)),
                }
                self.release_settled = true;
            }
            UserEvent::Sent(r) => {
                self.answer = Some(match r {
                    Ok(h) => {
                        self.record(Ev::info("send", "fgtw.org took it").hash("report", &h));
                        Answer::Sent(h)
                    }
                    Err(e) => {
                        self.record(Ev::error("send", "not sent: {}").t(&e));
                        Answer::NotSent(e)
                    }
                });
            }
        }
        EventResponse::Handled
    }

    /// Every shown frame: the glow's phase.
    fn wake_at(&self) -> Option<Instant> {
        Some(Instant::now())
    }

    fn tick(&mut self, _ctx: &mut Context) -> bool {
        true
    }

    fn render(&mut self, target: &mut [G2t16], ctx: &mut Context) {
        let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
        self.frame = self.frame.wrapping_add(1);
        let l = Layout::of(ctx.viewport);
        // The chrome first: the window's edge, then its strip and controls, which take their hits before anything of ours.
        self.ui.chrome.rasterize_perimeter(target, w, h, ctx.clip_mask);
        self.ui.chrome.rasterize_chrome(ctx.damage, ctx.text, ctx.clip_mask);
        self.ui.chrome.flatten_chrome_into(target, w, h, None);
        self.ui.chrome.settle_bg();
        apply_pressed(&mut self.ui, ctx.pressed_hit);

        let text = &mut *ctx.text;
        let mut canvas = Canvas::new(target, w, h, &mut *ctx.damage);
        let (colour, word, under) = (self.device_colour(), self.device.word().to_string(), self.device.under().to_string());
        // The choices under the box unfold while it is ticked and fold while it is not: each shown frame halves what is left of the way, so the motion is smooth and needs no clock.
        let target_unfold = if self.ui.consent.on { 1. } else { 0. };
        self.unfold += (target_unfold - self.unfold) * 0.5;
        let u = self.unfold;
        let Ui { chrome, install, consent, fails, whole } = &mut self.ui;
        let hit_map = &mut chrome.hit_test_map;

        // The send's answer, in front, a small line at the bottom of whatever screen is up.
        if let Some(a) = &self.answer {
            let words = match a {
                Answer::Sending => "Sending this run's log.".to_string(),
                Answer::Sent(h) => format!("Sent. Its name at fgtw.org: {}. Quote it to us.", hex(h)),
                Answer::NotSent(e) => format!("Not sent. {}. The log stays on this computer.", e.trim_end_matches('.')),
            };
            let style = TextStyle::new(l.quiet * 0.8, BODY);
            let lines = wrap(text, &words, &style, l.w - 2. * l.margin);
            let step = l.quiet * 0.8 * 1.4;
            // The last line sits a quiet size above the window's bottom edge and the others stack up from it; `words` is never empty, so `lines` is not.
            let mut y = h as f32 - l.quiet * 1.1 - step * (lines.len() as f32 - 1.);
            for line in &lines {
                text.draw_text_center(&mut canvas, line, l.cx(), y, &style, None, None);
                y += step;
            }
        }

        let title_size = match self.screen {
            Screen::Welcome => l.title_size(),
            Screen::Steps => l.title_size() * 0.8,
        };
        // Unfolding lifts the title and its lines by a thirty-second of the height, linear in the unfold, to give the choices room.
        let lift = l.hh * (1. / (1 << 5) as f32) * u;
        let title_cy = match self.screen {
            Screen::Welcome => l.y(0.26) - lift,
            Screen::Steps => l.y(0.16),
        };
        match self.screen {
            Screen::Welcome => {
                text.draw_text_center(&mut canvas, "Consent.", l.cx(), title_cy, &TextStyle::new(title_size, TITLE).weight(600), None, None);
                let body = TextStyle::new(l.body, BODY);
                let mut y = l.y(0.36) - lift;
                // The three lines as the owner wrote them; a window too narrow for one folds it, nothing else.
                for words in ["Nothing happens on this device unless you say so.", "Nobody's buying your data. If you ever choose to sell it, that's yours to decide too.", "And when something breaks, the only person to call is you."] {
                    for line in wrap(text, words, &body, l.w - 2. * l.margin) {
                        text.draw_text_center(&mut canvas, &line, l.cx(), y, &body, None, None);
                        y += l.body * 1.7;
                    }
                }

                // The box, centred as one with its words, a line under the three.
                let q = l.quiet;
                let (side, gap) = (q * 1.1, q * 0.6);
                let box_y = y + q * 0.3;
                let style = TextStyle::new(q, BODY);
                let x0 = l.cx() - (side + gap + text.measure_text(CONSENT_WORDS, &style)) * 0.5;
                consent.draw(&mut canvas, text, x0, box_y, side, gap, CONSENT_WORDS, &style, 1., Some(&mut hit_map[..]));

                // The two choices on one row and the facts under them, spreading out from under the box and fading in with the unfold; the choices take hits only while the box is ticked.
                let radio = TextStyle::new(q * 0.95, fade(BODY, u));
                let (fw, ww) = (text.measure_text(When::Fails.words(), &radio), text.measure_text(When::Whole.words(), &radio));
                let between = q * 2.;
                let rx0 = l.cx() - (2. * (side + gap) + fw + between + ww) * 0.5;
                let radio_y = box_y + u * q * 1.9;
                let ticked = consent.on;
                fails.draw(&mut canvas, text, rx0, radio_y, side, gap, When::Fails.words(), &radio, u, ticked.then_some(&mut hit_map[..]));
                whole.draw(&mut canvas, text, rx0 + side + gap + fw + between, radio_y, side, gap, When::Whole.words(), &radio, u, ticked.then_some(&mut hit_map[..]));
                let fact = TextStyle::new(q * 0.85, fade(QUIET, u));
                let mut fy = radio_y + u * q * 1.6;
                for words in FACTS {
                    for line in wrap(text, words, &fact, l.w - 2. * l.margin) {
                        text.draw_text_center(&mut canvas, &line, l.cx(), fy, &fact, None, None);
                        fy += u * q * 0.85 * 1.45;
                    }
                }

                if let Some(b) = install {
                    let id = b.hit_id();
                    b.render_content_into(&mut canvas, 0., 0., text, None, Some(&mut hit_map[..]), id);
                    // The button's own glow, under it: the same warm light as the title's, small and steady.
                    glow(&mut canvas, b.center_x, b.center_y, b.width * 0.5, b.height * 0.9, 1. / (1 << 3) as f32, GLOW);
                }
                // The silhouette fills what is left between the box's facts and three quarters of the way down, so it gives way as they unfold.
                let top = fy + q * 1.2;
                let sh = soft_height(l.y(0.76) - top, l.body * 2.);
                silhouette(&mut canvas, text, l.cx(), top + sh * 0.5, sh, colour, &word, &under, l.body * 1.3 * sh / (l.hh * 0.25));
                text.draw_text_center(&mut canvas, "This replaces Android and erases the device. You're choosing that.", l.cx(), l.y(0.935), &TextStyle::new(l.quiet, QUIET), None, None);
            }
            Screen::Steps => {
                text.draw_text_center(&mut canvas, "Welcome!", l.cx(), title_cy, &TextStyle::new(title_size, TITLE).weight(600), None, None);
                let body = TextStyle::new(l.body, BODY);
                let mut y = l.y(0.27);
                let max = l.w - 2. * l.margin;
                for words in [
                    "This completely removes Android and as much google as we can and replaces it with an operating system that respects you. You'll need a Pixel 8a or a Pixel 8 Pro as that's all there is right now, oh and a fast cable to transfer the goods.",
                    "This tool fetches the signed ferros release, named by its hash and checked before anything is written. The steps ahead:",
                ] {
                    for line in wrap(text, words, &body, max) {
                        text.draw_text_center(&mut canvas, &line, l.cx(), y, &body, None, None);
                        y += l.body * 1.6;
                    }
                    y += l.body * 0.8;
                }
                let step = TextStyle::new(l.body, TITLE);
                for line in ["plug in", "turn on unlocking", "unlock on the device (this erases it)", "install", "lock it back", "done"] {
                    text.draw_text_center(&mut canvas, line, l.cx(), y, &step, None, None);
                    y += l.body * 1.5;
                }
                // The owner's line under the steps (2026-11-28), in the quiet style.
                let quiet = TextStyle::new(l.quiet, QUIET);
                y += l.body * 0.4;
                for line in wrap(text, "A fresh OS, a bumpy road. Don't expect it to pair with your DJI goggles and fly your drone today.", &quiet, max) {
                    text.draw_text_center(&mut canvas, &line, l.cx(), y, &quiet, None, None);
                    y += l.quiet * 1.5;
                }
                // The silhouette fills what is left under the words, down to nine tenths of the height.
                let top = y + l.body * 0.4;
                let sh = soft_height(l.y(0.9) - top, l.body * 2.);
                silhouette(&mut canvas, text, l.cx(), top + sh * 0.5, sh, colour, &word, &under, l.body * sh / (l.hh * 0.17));
                text.draw_text_center(&mut canvas, "This build goes no further yet: nothing is written to the device.", l.cx(), l.y(0.935), &TextStyle::new(l.quiet, QUIET), None, None);
            }
        }

        // The mark, under the words, from the release's own logo document.
        if let Some(m) = &mut self.mark {
            let side = l.mark_side();
            m.draw(side);
            // Inside the frame by the layout's own numbers: the side is span/8 and span is at most twice the shorter edge, so half of it is at most an eighth of the height, and the centre sits at the strip (a sixteenth of the span) plus 8.5 % of what is under it, at half the width.
            let (x0, y0) = ((l.cx() - side as f32 * 0.5) as usize, (l.y(0.085) - side as f32 * 0.5) as usize);
            for y in 0..side {
                for x in 0..side {
                    let i = (y0 + y) * w + x0 + x;
                    canvas.pixels[i] = canvas.pixels[i].under(m.pixels[y * side + x], BlendMode::Normal);
                }
            }
            canvas.damage.add_bounds(x0, y0, x0 + side, y0 + side);
        }

        // The one warm glow behind the title, breathing with the shown frames: its peak swings between 1/8 and 1/4 on a sine of the frame count, C∞ in time as in space.
        let phase = self.frame as f32 * (core::f32::consts::TAU / BREATH_FRAMES as f32);
        let peak = (1. / (1 << 3) as f32) * (1.5 + 0.5 * phase.sin());
        glow(&mut canvas, l.cx(), title_cy, l.w * 0.3, title_size * 2., peak, GLOW);

        // The canvas itself, last: opaque, so every pixel is finished.
        fill_rect(&mut canvas, 0, 0, w as isize, h as isize, BG, None, None);

        if self.release_settled {
            if let Some(path) = self.snap.take() {
                match snap::write(&path, target, w, h) {
                    Ok(()) => eprintln!("ferros-flash-gui: the frame is in {path}"),
                    Err(e) => eprintln!("ferros-flash-gui: {e}"),
                }
            }
        }
    }

    fn cursor_for(&self, x: Coord, y: Coord, _ctx: &Context) -> CursorIcon {
        if self.ui.chrome.hit_at(x, y) != HIT_NONE { CursorIcon::Pointer } else { CursorIcon::Default }
    }

    fn hit_test_map(&mut self) -> Option<(&mut [HitId], usize, usize)> {
        Some(self.ui.chrome.hit_map_mut())
    }

    fn overlay_deltas(&mut self) -> Vec<u32> {
        build_overlay_deltas(&mut self.ui, self.hits as usize + 1)
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut base = BASE.to_string();
    let mut snap = None;
    let mut screen = Screen::Welcome;
    let mut device = Device::None;
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--release" if i + 1 < a.len() => base = a[i + 1].clone(),
            "--snap" if i + 1 < a.len() => snap = Some(a[i + 1].clone()),
            "--steps" => {
                screen = Screen::Steps;
                i += 1;
                continue;
            }
            "--pretend" if i + 1 < a.len() => {
                device = match a[i + 1].as_str() {
                    "noaccess" => Device::NoAccess { why: "pretended".into() },
                    "android" => Device::Android { model: "Pixel 8a".into() },
                    "bootloader" => Device::Bootloader { model: "Pixel 8a".into() },
                    "ferros" => Device::Ferros,
                    _ => Device::None,
                }
            }
            x => {
                eprintln!("ferros-flash-gui: `{x}`? usage: ferros-flash-gui [--release <base>] [--snap <out.vsf>] [--steps]");
                std::process::exit(2)
            }
        }
        i += 2;
    }
    let log = open_log();
    if let Ok(l) = log.lock() {
        eprintln!("ferros-flash-gui: this run's log is {}", l.path().display());
    }
    note(&log, Ev::info("release", "fetching the release from {}").t(&base).id("base", &base));
    run_app(App::new(base, snap, screen, device, log)).expect("the window's event loop");
}
