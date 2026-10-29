//! ferros-attest-mark — the attest recognition mark, on the desktop first.
//!
//! A phone-shaped window with one textbox: whatever is in the box, together with the device's salt, picks a Julia set that fills the window behind it, and every change to the box changes the picture.
//! On the phone this becomes ahau's attest screen, where the salt is the device's own, sealed under its ira, so a swapped phone cannot draw the owner's picture for the owner's handle.
//! Here the salt is a stand-in: Ctrl+R steps to the next stand-in device, to see how different devices' families look.
//! Ctrl+arrows nudge the colouring's complex offset and Ctrl+PageUp/PageDown double or halve the relief's depth (both printed), to tune the look for the final sphere; Ctrl+L toggles one fixed light against each device's own, Ctrl+B toggles the descent's x between banded per device (1/12 of the real axis) and free.
//! Layout: the textbox at the top, the mark's sphere centred between it and the bottom, and a tiny grey Mandelbrot set in the top-right gap with a white pixel at the Julia set's c.
//! The text is never printed or stored.

use tapuwae as mark;

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
    /// The descent's x banded per device (1/12 of the real axis) or free.
    banded: bool,
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
            banded: true,
            salt: salt_for(0),
            mark: Vec::new(),
            mark_key: (0, 0, [0; 32]),
        }
    }

    /// The textbox at the top, a little narrower than the window, sized from the span.
    fn layout(&mut self, vp: Viewport, text: &mut TextRenderer) {
        let (w, _) = (vp.width_px as Coord, vp.height_px as Coord);
        let span = vp.effective_span();
        let fs = span / (1 << 5) as Coord;
        self.textbox.set_rect(w * 0.5, textbox_cy(span), w - span / (1 << 2) as Coord, fs * 1.6);
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

/// The textbox's centre height: clear of the window's title strip, 2⁻³ of the span down.
fn textbox_cy(span: Coord) -> Coord {
    span / (1 << 3) as Coord
}

/// The textbox's bottom edge.
fn textbox_bottom(span: Coord) -> Coord {
    textbox_cy(span) + span / (1 << 5) as Coord * 0.8
}

/// A smooth (C∞) minimum, (a + b − √((a − b)² + δ²))/2 with δ = 2⁻⁴ of the larger: a hair under the smaller, with no corner where the two cross.
fn smooth_min(a: f32, b: f32) -> f32 {
    let d = a.max(b) * (1. / (1 << 4) as f32);
    0.5 * (a + b - ((a - b) * (a - b) + d * d).sqrt())
}

/// A tiny grey Mandelbrot set in the box (x0, y0, bw, bh) of `buf` (rows of `w`, α + darkness): the set's whole extent [−2, X_RIGHT] × [−1.15, 1.15], imaginary axis up, fitted inside the box; the set mid grey, the field around it a faint grey by escape speed, and a white pixel at `c`.
fn render_minibrot(buf: &mut [u32], w: usize, x0: usize, y0: usize, bw: usize, bh: usize, c: (f64, f64)) {
    let grey = |g: u32| 0xFF00_0000 | ((255 - g) << 16) | ((255 - g) << 8) | (255 - g);
    let (re0, re1, im_half) = (-2.0f64, mark::X_RIGHT, 1.15f64);
    // Plane units per pixel at which both extents fit the box: the larger of the two needed picks the fit, it clamps nothing.
    let k = ((re1 - re0) / bw as f64).max(2. * im_half / bh as f64);
    let (cre, cim) = (0.5 * (re0 + re1), 0.);
    for py in 0..bh {
        for px in 0..bw {
            let cr = cre + (px as f64 + 0.5 - bw as f64 * 0.5) * k;
            let ci = cim - (py as f64 + 0.5 - bh as f64 * 0.5) * k;
            let (mut x, mut y, mut n) = (0f64, 0f64, 0u32);
            while n < 1 << 8 && x * x + y * y <= 4. {
                (x, y) = (x * x - y * y + cr, 2. * x * y + ci);
                n += 1;
            }
            let g = if n == 1 << 8 { 0x90 } else { (72. * (1. - (-(n as f32) / 8.).exp2())) as u32 };
            buf[(y0 + py) * w + x0 + px] = grey(g);
        }
    }
    let px = ((c.0 - cre) / k + bw as f64 * 0.5) as usize;
    let py = ((cim - c.1) / k + bh as f64 * 0.5) as usize;
    // WHY/PROOF: c lies on [−2, X_RIGHT] × [0, 2], inside the fitted extent, but the box's integer edges can round its pixel one past the last row or column.
    if px < bw && py < bh {
        buf[(y0 + py) * w + x0 + px] = grey(0xFF);
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
                // Ctrl+L: one fixed light or each device's own. Ctrl+B: the descent's x banded per device or free.
                if ctx.modifiers.control_key() && matches!(&kev.logical_key, Key::Character(s) if s.eq_ignore_ascii_case("l") || s.eq_ignore_ascii_case("b")) {
                    if matches!(&kev.logical_key, Key::Character(s) if s.eq_ignore_ascii_case("l")) {
                        self.look.fixed_light = !self.look.fixed_light;
                        println!("light: {}", if self.look.fixed_light { "fixed" } else { "per device" });
                    } else {
                        self.banded = !self.banded;
                        println!("x: {}", if self.banded { "banded per device (1/12 of the axis)" } else { "free" });
                    }
                    self.mark_key.2 = [0; 32];
                    ctx.window.request_redraw();
                    return EventResponse::Handled;
                }
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
                    // Ctrl+PageUp / PageDown: the relief twice or half as deep as the device's own.
                    let height = match &kev.logical_key {
                        Key::Named(NamedKey::PageUp) => Some(2f32),
                        Key::Named(NamedKey::PageDown) => Some(0.5),
                        _ => None,
                    };
                    if let Some(f) = height {
                        self.look.relief *= f;
                        println!("relief × {}", self.look.relief);
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
            let p = mark::Params::new_placed(&self.salt, &self.text(), self.banded);
            // For tuning: where this picture came from (coordinates only, never the text).
            let c = mark::locus(&p);
            let (land, hit, trail) = mark::descend(c.0);
            println!("device {:2}  x {:+.6}  landing {:+.6} {:+.6}i ({} after {} checks)  c {:+.6} {:+.6}i", self.device, c.0, land.0, land.1, if hit { "on the set" } else { "missed, on the axis" }, trail.len(), c.0, c.1);
            // The sphere, centred between the textbox and the bottom: as large as fits that space and the window's width, a hair inside both.
            let span = 2. * (w * h) as f32 / (w + h) as f32;
            let top = textbox_bottom(span);
            let room = h as f32 - top;
            let r = 0.5 * smooth_min(w as f32, room) * (1. - 1. / (1 << 4) as f32);
            mark::render_disc(&mut self.mark, w, w as f32 * 0.5, top + room * 0.5, r, &p, &self.look);
            // A tiny grey Mandelbrot in the top-right gap between the textbox and the sphere, with a white pixel at c.
            let mb_w = (span / (1 << 2) as f32) as usize;
            let mb_h = (mb_w as f32 * 2.3 / (mark::X_RIGHT as f32 + 2.)) as usize + 1;
            let margin = (span / (1 << 6) as f32) as usize;
            render_minibrot(&mut self.mark, w, w - margin - mb_w, top as usize + margin, mb_w, mb_h, c);
            for px in self.mark.iter_mut() {
                *px = px.under(0xFF_FF_FF_FF, BlendMode::Normal);
            }
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
