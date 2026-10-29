//! ferros-attest-mark — the attest recognition mark, on the desktop first.
//!
//! A phone-shaped window with one textbox: whatever is in the box, together with the device's salt, picks a Julia set that fills the window behind it, and every change to the box changes the picture.
//! On the phone this becomes ahau's attest screen, where the salt is the device's own, sealed under its ira, so a swapped phone cannot draw the owner's picture for the owner's handle.
//! Here the salt is a stand-in: Ctrl+R steps to the next stand-in device, to see how different devices' families look.
//! Ctrl+arrows nudge the colouring's complex offset and Ctrl+PageUp/PageDown double or halve the relief's depth (both printed), to tune the look for the final sphere.
//! For tuning, the window is split: the mark in the top half, and below it two locator strips, imaginary axis up — the whole Mandelbrot set with every point the descent checked as a pixel in its own random colour and a crosshair on the Julia set's c, and a 2⁵× zoom around c.
//! The text is never printed or stored.

mod mark;

use fluor::canvas::Canvas;
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, Key, MouseButton, NamedKey};
use fluor::geom::Viewport;
use fluor::host::app::{Context, EventResponse, FluorApp};
use fluor::host::chrome::{self, HIT_NONE, HitId, ResizeEdge};
use fluor::host::chrome_widget::DefaultChrome;
use fluor::host::widget::{self as widget, Container, Widget};
use fluor::pixel::{Blend, BlendMode};
use fluor::text::TextRenderer;
use fluor::widgets::Textbox;

/// The stand-in for a device's sealed salt; device k's salt is BLAKE3 of this and k.
const DEV_SALT_SEED: &[u8; 32] = b"ferros attest mark dev salt 0000";

/// husky's panel, for the window's shape.
const PANEL_W: u32 = 1344;
const PANEL_H: u32 = 2992;

struct App {
    chrome: DefaultChrome,
    hit_counter: HitId,
    textbox: Textbox,
    device: u32,
    look: mark::Look,
    salt: [u8; 32],
    /// The rendered mark (α + darkness) and what it was rendered for: (width, height, hash of salt and text).
    mark: Vec<u32>,
    mark_key: (usize, usize, [u8; 32]),
}

fn salt_for(device: u32) -> [u8; 32] {
    *blake3::Hasher::new().update(DEV_SALT_SEED).update(&device.to_le_bytes()).finalize().as_bytes()
}

impl App {
    fn new() -> Self {
        let viewport = Viewport::new(PANEL_W / 4, PANEL_H / 4);
        let mut hit_counter: HitId = HIT_NONE;
        let chrome = DefaultChrome::new(viewport, "attest mark".to_string(), None, None, &mut hit_counter);
        let textbox = Textbox::new(&mut hit_counter, 0., 0., 1., 1., 12.);
        Self {
            chrome,
            hit_counter,
            textbox,
            device: 0,
            look: mark::Look::default(),
            salt: salt_for(0),
            mark: Vec::new(),
            mark_key: (0, 0, [0; 32]),
        }
    }

    /// The textbox sits near the bottom of the mark (the top half), a little narrower than the window, sized from the span.
    fn layout(&mut self, vp: Viewport, text: &mut TextRenderer) {
        let (w, h) = (vp.width_px as Coord, vp.height_px as Coord);
        let span = vp.effective_span();
        let fs = span / (1 << 5) as Coord;
        self.textbox.set_rect(w * 0.5, h * 0.4, w - span / (1 << 2) as Coord, fs * 1.6);
        self.textbox.set_font_size(fs, text);
    }

    /// The box always holds the keyboard; its cursor is shown steady, with no blink timer.
    fn focus(&mut self) {
        let id = self.textbox.hit_id();
        if !self.textbox.is_focused() {
            widget::apply_focus_change(self as &mut dyn Container, None, Some(id));
            self.textbox.flip_blinkey();
        }
    }

    fn text(&self) -> String {
        self.textbox.chars.iter().collect()
    }
}

impl Container for App {
    fn visit(&mut self, f: &mut dyn FnMut(&mut dyn Widget)) {
        f(&mut self.textbox);
        self.chrome.visit(f);
    }
}

impl FluorApp for App {
    type UserEvent = ();

    fn title(&self) -> &str {
        "attest mark"
    }

    fn initial_size(&self, monitor: (u32, u32)) -> (u32, u32) {
        let h = monitor.1 * 3 / 4;
        (h * PANEL_W / PANEL_H, h)
    }

    fn init(&mut self, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.layout(ctx.viewport, ctx.text);
        self.focus();
    }

    fn on_resize(&mut self, _w: u32, _h: u32, ctx: &mut Context) {
        self.chrome.resize(ctx.viewport);
        self.chrome.set_full_edge(ctx.is_maximized);
        self.layout(ctx.viewport, ctx.text);
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        match event {
            Event::CursorMoved { .. } => {
                let hit = self.chrome.hit_at(ctx.cursor_x, ctx.cursor_y);
                if self.chrome.set_hover(hit) {
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
                if hit == self.textbox.hit_id() {
                    let mods = ctx.modifiers;
                    let r = widget::dispatch_click(self as &mut dyn Container, hit, x, y, mods);
                    ctx.window.request_redraw();
                    return r;
                }
                let edge = chrome::get_resize_edge(ctx.viewport, x, y);
                if edge != ResizeEdge::None {
                    return EventResponse::StartResize(edge);
                }
                EventResponse::StartWindowDrag
            }
            Event::KeyboardInput { event: kev } if kev.state == ElementState::Pressed => {
                // Ctrl+arrows nudge the colouring's complex offset by 2⁻⁴ (left/right the real part, up/down the imaginary).
                if ctx.modifiers.control_key() {
                    let step = 1. / (1 << 4) as f32;
                    let nudge = match &kev.logical_key {
                        Key::Named(NamedKey::ArrowLeft) => Some((-step, 0.)),
                        Key::Named(NamedKey::ArrowRight) => Some((step, 0.)),
                        Key::Named(NamedKey::ArrowUp) => Some((0., step)),
                        Key::Named(NamedKey::ArrowDown) => Some((0., -step)),
                        _ => None,
                    };
                    // Ctrl+PageUp / PageDown: the relief twice or half as deep.
                    let height = match &kev.logical_key {
                        Key::Named(NamedKey::PageUp) => Some(2f32),
                        Key::Named(NamedKey::PageDown) => Some(0.5),
                        _ => None,
                    };
                    if let Some(f) = height {
                        self.look.relief *= f;
                        println!("relief {} px", self.look.relief);
                        self.mark_key.2 = [0; 32];
                        ctx.window.request_redraw();
                        return EventResponse::Handled;
                    }
                    if let Some((dr, di)) = nudge {
                        self.look.offset = (self.look.offset.0 + dr, self.look.offset.1 + di);
                        println!("colour offset {:+.4} {:+.4}i", self.look.offset.0, self.look.offset.1);
                        self.mark_key.2 = [0; 32];
                        ctx.window.request_redraw();
                        return EventResponse::Handled;
                    }
                }
                if ctx.modifiers.control_key() && matches!(&kev.logical_key, Key::Character(s) if s.eq_ignore_ascii_case("r")) {
                    self.device += 1;
                    self.salt = salt_for(self.device);
                    ctx.window.request_redraw();
                    return EventResponse::Handled;
                }
                let (id, mods) = (self.textbox.hit_id(), ctx.modifiers);
                let r = widget::dispatch_key(self as &mut dyn Container, id, kev, mods, ctx.text);
                ctx.window.request_redraw();
                r
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

        // Re-render the mark only when the window or the (salt, text) pair changed.
        let key = (w, h, *blake3::Hasher::new().update(&self.salt).update(self.text().as_bytes()).finalize().as_bytes());
        if key != self.mark_key {
            self.mark.resize(w * h, 0);
            let p = mark::Params::new(&self.salt, &self.text());
            // For tuning: where this picture came from (coordinates only, never the text).
            let c = mark::locus(&p);
            let (land, hit, trail) = mark::descend(c.0);
            println!("device {:2}  x {:+.6}  landing {:+.6} {:+.6}i ({} after {} checks)  c {:+.6} {:+.6}i", self.device, c.0, land.0, land.1, if hit { "on the set" } else { "missed, on the axis" }, trail.len(), c.0, c.1);
            // Top half: the mark; then the whole set, then a 2⁵× zoom around the locus, a quarter of the height each.
            let (h0, h1) = (h / 2, h / 4);
            let (top, rest) = self.mark.split_at_mut(w * h0);
            let (whole, near) = rest.split_at_mut(w * h1);
            // The mark spherized into a disc above the textbox: radius a quarter of the pane's span, centred at 0.45 of its height; the rest of the pane is black.
            let span = 2. * (w * h0) as f32 / (w + h0) as f32;
            mark::render_disc(top, w, w as f32 * 0.5, h0 as f32 * 0.45, span * 0.25, &p, &self.look);
            for px in top.iter_mut() {
                *px = px.under(0xFF_FF_FF_FF, BlendMode::Normal);
            }
            mark::render_whole(whole, w, h1, &p);
            mark::render_near(near, w, h - h0 - h1, &p);
            self.mark_key = key;
        }

        // Front to back: the window's edge and controls, the textbox, then the mark under everything.
        self.chrome.rasterize_perimeter(target, w, h, ctx.clip_mask);
        self.chrome.rasterize_chrome(ctx.damage, ctx.text, ctx.clip_mask);
        {
            let mut canvas = Canvas::new(target, w, h, ctx.damage);
            self.textbox.render_blinkey_into(&mut canvas, 0., 0.);
            let id = self.textbox.hit_id();
            self.textbox.render_content_into(&mut canvas, 0., 0., ctx.text, None, None, Some(&mut self.chrome.hit_test_map), id);
        }
        self.chrome.flatten_into(target, w, h, None);
        for (dst, &src) in target.iter_mut().zip(self.mark.iter()) {
            *dst = dst.under(src, BlendMode::Normal);
        }
        ctx.damage.add_bounds(0, 0, w, h);
    }

    fn cursor_for(&self, x: Coord, y: Coord, ctx: &Context) -> CursorIcon {
        let hit = self.chrome.hit_at(x, y);
        if self.chrome.owns_hit(hit) {
            return CursorIcon::Pointer;
        }
        if hit == self.textbox.hit_id() {
            return CursorIcon::Text;
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
    fluor::host::app::run_app(App::new()).expect("event loop");
}
