//! tune: one desktop tuning utility for ferros, a page per thing tuned, every dimension a slider (owner, 2026-11-10: "what's the command again? lightscope, orb tune? do we have a wallpaper one? it should be one utility").
//!
//! Pages, switched with Page Up and Page Down or a click on a page's name at the top left:
//! - orbs (orbs.rs, orb tune until 2026-11-10): the console's radio circles, the vessel and the orb inside it, judged large and at the panel's true size.
//! - wallpaper (wall.rs): a person's default wallpaper, drawn by the device's own crate from a seed under the `wallpaper::Knobs` the sliders set, at husky's or akita's frame.
//! Each page's keys are in its own header; Tab and Shift+Tab walk the sliders, the arrow keys nudge the one in focus, Escape lets go.
//!
//! Usage (cd tools/tune && cargo build --release; the binary is target/release/tune):
//!   tune [orbs | wallpaper] [--seed <64 hex digits | a counter>]   the window, on that page (orbs by default); the seed is the wallpaper page's first
//!   tune orbs --dump <out.vsf> [key=value ...]                    the orbs page's circles as a VSF visual, no window (`S` on the page prints the arguments)
//!   tune orbs --nuclei                                            each bulb's nucleus
//!   tune wallpaper --report [<seed>] [akita]                      no window: the attempts a seed's survey looked at, the one kept, and the time each pass takes on this host

mod look;
mod orbs;
mod wall;

use fluor::canvas::{Canvas, PixelRect};
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, Key, KeyEvent, MouseButton, NamedKey};
use fluor::geom::Viewport;
use fluor::host::app::{Context, EventResponse, FluorApp};
use fluor::host::chrome::{self, HIT_NONE, HitId, ResizeEdge};
use fluor::host::chrome_widget::DefaultChrome;
use fluor::host::widget::{self as widget, Container, TabDir, Widget};
use fluor::pixel::{Blend, BlendMode};
use fluor::text::TextStyle;
use fluor::widgets::Slider;

/// Text: a label, and dimmer notes (8-bit α + darkness).
pub const LABEL: u32 = 0xFF00_0000 | (0x00C0_C0C0 ^ 0x00FF_FFFF);
pub const DIM: u32 = 0xFF00_0000 | (0x0080_8080 ^ 0x00FF_FFFF);
/// The slider column's ground, a dark blue-grey (as darkness), and the picture side's black.
const COLUMN_BG: u32 = 0xFF00_0000 | (0x0014_161C ^ 0x00FF_FFFF);
const BLACK8: u32 = 0xFFFF_FFFF;

pub fn fail(m: &str) -> ! {
    eprintln!("tune: {m}");
    std::process::exit(1)
}

/// A smooth (C∞) minimum, (a + b − √((a − b)² + δ²))/2 with δ = 2⁻⁵ of a + b: a hair under the smaller, with no corner where the two cross.
pub fn smooth_min(a: f32, b: f32) -> f32 {
    let d = (a + b) / (1 << 5) as f32;
    0.5 * (a + b - ((a - b) * (a - b) + d * d).sqrt())
}

/// The text size for a window of span `span`: 1/64 of it, and a quarter again.
pub fn font(span: Coord) -> Coord {
    span / (1 << 6) as Coord * (1. + 1. / (1 << 2) as Coord)
}

/// The slider column right of `split` (the window `w` wide), one row per slider between `top` and `bottom`: the label at the left, the band across `band` (fractions of the column), the value right of it.
pub fn column(sliders: &mut [Slider], split: Coord, w: Coord, top: Coord, bottom: Coord, band: [Coord; 2]) {
    let pitch = (bottom - top) / sliders.len() as Coord;
    let cw = w - split;
    let band_h = pitch * (1. - 1. / (1 << 2) as Coord);
    let (left, right) = (split + cw * band[0], split + cw * band[1]);
    for (i, s) in sliders.iter_mut().enumerate() {
        let cy = top + (i as Coord + 0.5) * pitch;
        s.set_rect((left + right) * 0.5, cy, right - left, band_h * 0.5);
    }
}

/// The column's sliders, each with its label and its value as a person reads it (`shown`), into `target`.
pub fn draw_column(sliders: &mut [Slider], target: &mut [u32], ctx: &mut Context, hit_map: &mut [HitId], split: Coord, labels: &[&str], shown: &[String]) {
    let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
    let font = font(ctx.viewport.effective_span());
    let cw = w as Coord - split;
    for (i, s) in sliders.iter_mut().enumerate() {
        let mut canvas = Canvas::new(target, w, h, ctx.damage);
        let id = s.hit_id();
        s.render_content_into(&mut canvas, Some(&mut *hit_map), id);
        let b = s.bbox();
        let cy = b.y + b.h * 0.5;
        let mut canvas = Canvas::new(target, w, h, ctx.damage);
        ctx.text.draw_text_left(&mut canvas, labels[i], split + cw / (1 << 5) as Coord, cy, &TextStyle::new(font, LABEL), None, None);
        let mut canvas = Canvas::new(target, w, h, ctx.damage);
        ctx.text.draw_text_left(&mut canvas, &shown[i], b.x + b.w + cw * 0.02, cy, &TextStyle::new(font, LABEL), None, None);
    }
}

/// The grounds, last and opaque: black left of `split`, the column's blue-grey right of it.
pub fn grounds(target: &mut [u32], w: usize, split: usize) {
    for row in target.chunks_mut(w) {
        for (x, d) in row.iter_mut().enumerate() {
            *d = d.under(if x < split { BLACK8 } else { COLUMN_BG }, BlendMode::Normal);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Orbs,
    Wall,
}

const PAGES: [(Page, &str); 2] = [(Page::Orbs, "orbs"), (Page::Wall, "wallpaper")];

struct App {
    chrome: DefaultChrome,
    hit_counter: HitId,
    page: Page,
    orbs: orbs::Orbs,
    wall: wall::Wall,
    current_focus: Option<HitId>,
    /// The slider of the page shown that a press is dragging.
    dragging: Option<usize>,
    modifiers: fluor::event::ModifiersState,
    /// The top strip's height, and each page name's x range in it.
    top: Coord,
    tabs: [(Coord, Coord); 2],
}

impl App {
    fn new(page: Page, seed: Option<wall::Seed>) -> Self {
        let viewport = Viewport::new(1600, 1000);
        let mut hit_counter: HitId = HIT_NONE;
        let chrome = DefaultChrome::new(viewport, "tune".to_string(), None, None, &mut hit_counter);
        let orbs = orbs::Orbs::new(&mut hit_counter);
        let wall = wall::Wall::new(&mut hit_counter, seed);
        Self { chrome, hit_counter, page, orbs, wall, current_focus: None, dragging: None, modifiers: Default::default(), top: 0., tabs: [(0., 0.); 2] }
    }

    fn sliders(&mut self) -> &mut [Slider] {
        match self.page {
            Page::Orbs => &mut self.orbs.sliders,
            Page::Wall => &mut self.wall.sliders,
        }
    }

    /// The page shown takes its sliders' moves; true if anything changed.
    fn poll(&mut self) -> bool {
        match self.page {
            Page::Orbs => self.orbs.poll(),
            Page::Wall => self.wall.poll(),
        }
    }

    fn update_layout(&mut self, vp: Viewport, text: &mut fluor::text::TextRenderer) {
        let h = vp.height_px as Coord;
        let span = vp.effective_span();
        self.top = h / (1 << 4) as Coord;
        // The page names along the top strip's left, each as wide as its text and a margin each side.
        let margin = span / (1 << 6) as Coord;
        let style = TextStyle::new(font(span), LABEL);
        // They start a quarter span in, clear of the chrome's title at the top left.
        let mut x = span / (1 << 2) as Coord;
        for (i, (_, name)) in PAGES.iter().enumerate() {
            let wide = text.measure_text(name, &style);
            self.tabs[i] = (x, x + wide + 2. * margin);
            x += wide + 2. * margin;
        }
        self.orbs.layout(vp, self.top);
        self.wall.layout(vp, self.top);
    }

    /// To page `p`: focus and drag let go, and the chrome's hit map wiped (`resize` marks it dirty) so the other page's sliders leave no hit area behind.
    fn switch(&mut self, p: Page, ctx: &mut Context) {
        if p == self.page {
            return;
        }
        self.change_focus(None, ctx);
        self.dragging = None;
        self.page = p;
        self.chrome.resize(ctx.viewport);
        self.chrome.set_full_edge(ctx.is_maximized);
        ctx.window.request_redraw();
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
        // The wallpaper page's hex entry takes every key while it is open.
        let page_first = self.page == Page::Wall && self.wall.typing();
        if !page_first {
            match &kev.logical_key {
                Key::Named(NamedKey::PageUp) => {
                    self.switch(Page::Orbs, ctx);
                    return EventResponse::Handled;
                }
                Key::Named(NamedKey::PageDown) => {
                    self.switch(Page::Wall, ctx);
                    return EventResponse::Handled;
                }
                Key::Named(NamedKey::Tab) => {
                    let dir = if self.modifiers.shift_key() { TabDir::Backward } else { TabDir::Forward };
                    let current = self.current_focus;
                    let next = widget::linear_tab_next(self as &mut dyn Container, current, dir);
                    self.change_focus(next, ctx);
                    return EventResponse::Handled;
                }
                Key::Named(NamedKey::Escape) if self.current_focus.is_some() => {
                    self.change_focus(None, ctx);
                    return EventResponse::Handled;
                }
                _ => {}
            }
        }
        // The page's own keys, unless a slider holds focus and the key is one a slider takes (the arrows, Home, End).
        let slider_key = self.current_focus.is_some() && matches!(kev.logical_key, Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::Home | NamedKey::End));
        if !slider_key {
            let took = match self.page {
                Page::Orbs => self.orbs.key(kev),
                Page::Wall => self.wall.key(kev, self.modifiers.shift_key()),
            };
            if took {
                ctx.window.request_redraw();
                return EventResponse::Handled;
            }
        }
        let Some(focus_id) = self.current_focus else {
            return EventResponse::Pass;
        };
        let mods = self.modifiers;
        let response = widget::dispatch_key(self as &mut dyn Container, focus_id, kev, mods, ctx.text);
        if matches!(response, EventResponse::Handled) && self.poll() {
            ctx.window.request_redraw();
        }
        response
    }
}

impl Container for App {
    fn visit(&mut self, f: &mut dyn FnMut(&mut dyn Widget)) {
        for s in self.sliders() {
            f(s);
        }
        self.chrome.visit(f);
    }
}

impl FluorApp for App {
    type UserEvent = wall::Done;

    fn title(&self) -> &str {
        "tune"
    }

    fn initial_size(&self, monitor: (u32, u32)) -> (u32, u32) {
        let h = monitor.1 * 7 / 8;
        (h * 8 / 5, h)
    }

    fn set_event_proxy(&mut self, proxy: std::sync::Arc<dyn fluor::host::WakeSender<wall::Done>>) {
        self.wall.start(proxy);
    }

    fn init(&mut self, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.update_layout(ctx.viewport, ctx.text);
    }

    fn on_resize(&mut self, _w: u32, _h: u32, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.chrome.set_full_edge(ctx.is_maximized);
        self.update_layout(ctx.viewport, ctx.text);
    }

    fn on_user_event(&mut self, done: wall::Done, ctx: &mut Context) -> EventResponse {
        if self.wall.accept(done) && self.page == Page::Wall {
            ctx.window.request_redraw();
        }
        EventResponse::Handled
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
                    self.sliders()[i].set_value_from_x(x);
                    if self.poll() {
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
                if y < self.top {
                    if let Some(i) = self.tabs.iter().position(|&(a, b)| x >= a && x < b) {
                        self.switch(PAGES[i].0, ctx);
                        return EventResponse::Handled;
                    }
                }
                if hit != HIT_NONE {
                    let mods = self.modifiers;
                    let response = widget::dispatch_click(self as &mut dyn Container, hit, x, y, mods);
                    self.change_focus(Some(hit), ctx);
                    self.dragging = self.sliders().iter().position(|s| s.hit_id() == hit);
                    self.poll();
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
                    match self.page {
                        Page::Orbs => self.orbs.released(),
                        Page::Wall => {}
                    }
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
        self.chrome.rasterize_perimeter(target, w, h, ctx.clip_mask);
        self.chrome.rasterize_chrome(ctx.damage, ctx.text, ctx.clip_mask);

        // Front to back: the page names, the page's front (sliders, text), the chrome, the page's back (its pictures), then the grounds.
        let font = font(ctx.viewport.effective_span());
        for (i, (p, name)) in PAGES.iter().enumerate() {
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            let colour = if *p == self.page { LABEL } else { DIM };
            ctx.text.draw_text_center(&mut canvas, name, (self.tabs[i].0 + self.tabs[i].1) * 0.5, self.top * 0.5, &TextStyle::new(font, colour), None, None);
        }
        let dragging = self.dragging.is_some();
        match self.page {
            Page::Orbs => self.orbs.render_front(target, ctx, &mut self.chrome.hit_test_map, dragging),
            Page::Wall => self.wall.render_front(target, ctx, &mut self.chrome.hit_test_map),
        }
        self.chrome.flatten_into(target, w, h, None);
        match self.page {
            Page::Orbs => self.orbs.render_back(target, w, h),
            Page::Wall => self.wall.render_back(target, w, h),
        }
        ctx.damage.add_bounds(0, 0, w, h);
        if self.page == Page::Orbs && self.orbs.alive() {
            ctx.window.request_redraw();
        }
    }

    fn cursor_for(&self, x: Coord, y: Coord, ctx: &Context) -> CursorIcon {
        let hit = self.chrome.hit_at(x, y);
        let sliders = match self.page {
            Page::Orbs => &self.orbs.sliders,
            Page::Wall => &self.wall.sliders,
        };
        if self.chrome.owns_hit(hit) || sliders.iter().any(|s| s.hit_id() == hit) || (y < self.top && self.tabs.iter().any(|&(a, b)| x >= a && x < b)) {
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

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let page = match a.get(1).map(|s| s.as_str()) {
        Some("orbs") => Page::Orbs,
        Some("wallpaper") => Page::Wall,
        None => Page::Orbs,
        Some(other) => fail(&format!("no page {other}: orbs or wallpaper")),
    };
    let rest = if a.len() > 1 { &a[2..] } else { &a[1..] };
    match (page, rest.first().map(|s| s.as_str())) {
        (Page::Orbs, Some("--dump")) => {
            let Some(out) = rest.get(1) else { fail("usage: tune orbs --dump <out.vsf> [key=value ...]") };
            orbs::dump(out, &rest[2..]);
        }
        (Page::Orbs, Some("--nuclei")) => orbs::nuclei(),
        (Page::Wall, Some("--report")) => wall::report(&rest[1..]),
        (_, Some("--seed")) => {
            let Some(s) = rest.get(1) else { fail("usage: tune [orbs | wallpaper] --seed <64 hex digits | a counter>") };
            let seed = wall::Seed::parse(s).unwrap_or_else(|e| fail(&e));
            fluor::host::app::run_app(App::new(page, Some(seed))).expect("event loop");
        }
        (_, None) => fluor::host::app::run_app(App::new(page, None)).expect("event loop"),
        (_, Some(other)) => fail(&format!("unknown argument {other}")),
    }
}
