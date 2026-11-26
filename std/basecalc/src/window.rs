//! The window: the pad drawn front to back (the display's text, each key's label in front of its body, then the ground behind them all), every key's tap area its declared shape claimed in the hit map (fluor 3512d08's claims), and the vault written after each change it keeps.

use crate::sys;
use basecalc::{get_base_name, spans_text, Role, Span};
use basecalc_kist::calc::{Calc, Shown};
use basecalc_kist::keys::{self, Key, Kind};
use basecalc_kist::layout::{panel_circle, smooth_min, Circle, Face, Layout, Placed};
use basecalc_kist::store::{self, Saved};
use fluor::canvas::Canvas;
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, ModifiersState, MouseButton};
use fluor::host::app::{Context, FluorApp};
use fluor::host::EventResponse;
use fluor::paint::squircle::{asymmetric_radii, draw_squircle_rrect, CornerRadii, SHAPE_POWER};
use fluor::paint::{self, HitId, HIT_NONE};
use fluor::pixel::G2t16;
use fluor::text::{TextRenderer, TextStyle};
use fluor::theme::{dark, fmt, vsf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

const DISPLAY: u64 = 1;
const TOUCH: u64 = 2;
const SURFACE: u64 = 3;
const IRA: u64 = 4;

/// An authored VSF RGB `g2t` colour (diffuse white 0xC2), opaque, as the buffer stores it (fluor's theme: α and darkness).
const fn colour(t: u32) -> u32 {
    dark(fmt(vsf(t)))
}

/// The same colour a step lighter, for a key held down.
const fn lit(t: u32) -> u32 {
    colour(t + 0x00_18_18_18)
}

const GROUND: u32 = 0x00_06_08_0C;
const KEY_DIGIT: u32 = 0x00_22_2A_36;
const KEY_OPERATOR: u32 = 0x00_1A_36_44;
const KEY_FUNCTION: u32 = 0x00_2C_24_3C;
const KEY_EQUALS: u32 = 0x00_8C_54_10;
const KEY_CONTROL: u32 = 0x00_16_1A_22;
const KEY_CHOSEN: u32 = 0x00_2A_62_7C;
const INK: u32 = colour(0x00_C2_C2_C2);
const INK_DIM: u32 = colour(0x00_6C_70_78);
const INK_ERROR: u32 = colour(0x00_D8_40_38);
const INK_OPERATOR: u32 = colour(0x00_7C_B8_C8);
const INK_FUNCTION: u32 = colour(0x00_B4_9C_D8);
const INK_BRACKET: u32 = colour(0x00_90_B8_58);

/// A result's piece in its role's colour (basecalc's terminal colours its output by the same roles; these are its hues, authored in VSF RGB).
fn role_ink(role: Role) -> u32 {
    colour(match role {
        Role::LoneInteger => 0x00_94_C4_98,
        Role::LoneFraction => 0x00_74_C2_AC,
        Role::RealInteger => 0x00_8C_C0_C2,
        Role::RealFraction => 0x00_A8_B4_C2,
        Role::ImaginaryInteger => 0x00_C2_A8_98,
        Role::ImaginaryFraction => 0x00_C2_94_B0,
        Role::Exponent => 0x00_A0_5C_C0,
        Role::Tilde => 0x00_78_88_B8,
        Role::Error => 0x00_D8_40_38,
        Role::Nan => 0x00_B0_40_C2,
        Role::Brackets => 0x00_90_B8_58,
        Role::Comma | Role::Colon => 0x00_8C_90_98,
        Role::Carat => 0x00_C2_A0_10,
        Role::Plain | Role::Decimal | Role::Sign | Role::Message => 0x00_C2_C2_C2,
    })
}

fn body(face: Face, pressed: bool) -> u32 {
    let t = match face {
        Face::Digit => KEY_DIGIT,
        Face::Operator => KEY_OPERATOR,
        Face::Function => KEY_FUNCTION,
        Face::Equals => KEY_EQUALS,
        Face::Control => KEY_CONTROL,
        Face::Chosen => KEY_CHOSEN,
    };
    if pressed {
        lit(t)
    } else {
        colour(t)
    }
}

/// Whether the pixel (x, y) lies in a key's declared shape: its box with each corner cut on the house squircle (|dx|^3 + |dy|^3 <= r^3 about the corner's centre, fluor's `SHAPE_POWER`).
fn in_shape(x0: f32, y0: f32, x1: f32, y1: f32, radii: CornerRadii, x: usize, y: usize) -> bool {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    if px < x0 || px >= x1 || py < y0 || py >= y1 {
        return false;
    }
    // The corner whose quadrant holds the point, clockwise from the top left as `CornerRadii` is.
    let (left, top) = (px < (x0 + x1) * 0.5, py < (y0 + y1) * 0.5);
    let r = radii[match (left, top) {
        (true, true) => 0,
        (false, true) => 1,
        (false, false) => 2,
        (true, false) => 3,
    }];
    let cx = if left { x0 + r } else { x1 - r };
    let cy = if top { y0 + r } else { y1 - r };
    let dx = if left { cx - px } else { px - cx };
    let dy = if top { cy - py } else { py - cy };
    dx <= 0. || dy <= 0. || dx * dx * dx + dy * dy * dy <= r * r * r
}

/// A run of text in one colour, for the display's lines.
struct Run {
    text: String,
    ink: u32,
}

fn run(text: impl Into<String>, ink: u32) -> Run {
    Run { text: text.into(), ink }
}

/// The expression as the display shows it: each unit the keys wrote, by its label, in its kind's colour; a worded operator spaced.
fn expression_runs(expr: &str, dim: bool) -> Vec<Run> {
    keys::units(expr)
        .into_iter()
        .map(|p| {
            let ink = if dim {
                INK_DIM
            } else {
                match p.kind {
                    Kind::Digit => INK,
                    Kind::Infix => INK_OPERATOR,
                    Kind::Opens | Kind::Value if p.text.starts_with(['#', '@', '&']) || p.label == "i" => INK_FUNCTION,
                    _ => INK_BRACKET,
                }
            };
            let text = if p.kind == Kind::Infix && p.label.chars().count() > 1 { format!(" {} ", p.label) } else { p.label.to_string() };
            run(text, ink)
        })
        .collect()
}

/// A result's runs: its base, then each span in its role's colour (all dim for a preview), the engine's leading spaces dropped.
fn result_runs(base: u8, spans: &[Span], dim: bool) -> Vec<Run> {
    let mut out = vec![run(keys::base_label(base), INK_DIM)];
    let mut leading = true;
    for s in spans {
        let text = if leading { s.text.trim_start() } else { s.text.as_str() };
        if text.is_empty() {
            continue;
        }
        leading = false;
        out.push(run(text, if dim { INK_DIM } else { role_ink(s.role) }));
    }
    out
}

/// The runs cut into lines no wider than `width` at `size`, breaking after a space (basecalc writes digits in groups of three), at most `most` lines, the last ending in an ellipsis when more were left.
fn wrap(text: &mut TextRenderer, runs: Vec<Run>, width: f32, size: f32, most: usize) -> Vec<Vec<(Run, f32)>> {
    let mut lines: Vec<Vec<(Run, f32)>> = vec![Vec::new()];
    let mut line_w = 0.;
    for r in runs {
        for piece in r.text.split_inclusive(' ') {
            let w = text.measure_text(piece, &TextStyle::new(size, r.ink));
            let last = lines.last_mut().unwrap();
            if line_w + w > width && last.last().is_some_and(|(p, _): &(Run, f32)| p.text.ends_with(' ')) {
                lines.push(Vec::new());
                line_w = 0.;
            }
            lines.last_mut().unwrap().push((run(piece, r.ink), w));
            line_w += w;
        }
    }
    if lines.len() > most {
        lines.truncate(most);
        let ellipsis = "\u{2026}";
        let ew = text.measure_text(ellipsis, &TextStyle::new(size, INK_DIM));
        let last = lines.last_mut().unwrap();
        while last.len() > 1 && last.iter().map(|(_, w)| w).sum::<f32>() + ew > width {
            last.pop();
        }
        last.push((run(ellipsis, INK_DIM), ew));
    }
    lines
}

/// One line's runs, right-aligned at `right`, centred on `y`.
fn draw_line(canvas: &mut Canvas<G2t16>, text: &mut TextRenderer, line: &[(Run, f32)], right: f32, y: f32, size: f32) {
    let mut x = right - line.iter().map(|(_, w)| w).sum::<f32>();
    for (r, w) in line {
        text.draw_text_left(canvas, &r.text, x, y, &TextStyle::new(size, r.ink), None, None);
        x += w;
    }
}

/// One line that keeps its right end: runs dropped from the left until it fits, an ellipsis in their place.
fn tail(text: &mut TextRenderer, runs: Vec<Run>, width: f32, size: f32) -> Vec<(Run, f32)> {
    let mut line: Vec<(Run, f32)> = runs.into_iter().map(|r| {
        let w = text.measure_text(&r.text, &TextStyle::new(size, r.ink));
        (r, w)
    }).collect();
    let total = |l: &[(Run, f32)]| l.iter().map(|(_, w)| w).sum::<f32>();
    if total(&line) > width {
        let ellipsis = "\u{2026}";
        let ew = text.measure_text(ellipsis, &TextStyle::new(size, INK_DIM));
        while line.len() > 1 && total(&line) + ew > width {
            line.remove(0);
        }
        line.insert(0, (run(ellipsis, INK_DIM), ew));
    }
    line
}

/// The pad's window (and the basecalc puck's face, puck.rs).
pub(crate) struct Pad {
    pub(crate) calc: Calc,
    /// The app's circle as the display last told it (`on_safe_area`); until then the display's way of working it out.
    pub(crate) circle: Option<Circle>,
    pub(crate) layout: Option<Layout>,
    pub(crate) placed: Vec<Placed>,
    /// The keys as the last frame drew them: the hit map's ids index this, so a tap answers to the key the person saw even when a key before it in the same pass placed the keys again.
    pub(crate) drawn: Vec<Placed>,
    /// Each placed key's label size, measured once a layout (0: not yet).
    pub(crate) sizes: Vec<f32>,
    pub(crate) hits: Vec<HitId>,
    pub(crate) size: (usize, usize),
    pub(crate) shown: paint::Shown,
    /// The vault's writer thread, when the vault opened.
    pub(crate) vault: Option<Sender<Vec<u8>>>,
}

impl Pad {
    /// A pad with nothing placed yet (the first frame's size places it), its vault's writer when the vault opened.
    pub(crate) fn new(calc: Calc, vault: Option<Sender<Vec<u8>>>) -> Pad {
        Pad { calc, circle: None, layout: None, placed: Vec::new(), drawn: Vec::new(), sizes: Vec::new(), hits: Vec::new(), size: (0, 0), shown: paint::Shown::default(), vault }
    }

    /// The keys placed for the frame's size, the base, the layer and the picker.
    /// The hit map is sized with the frame here, so the host's look-up (fluor ferros `hit_under_cursor`, which takes the map's width and height as given) always indexes a map of the frame's size, even for a touch before the first frame is drawn.
    pub(crate) fn place(&mut self) {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        if self.hits.len() != self.size.0 * self.size.1 {
            self.hits = vec![HIT_NONE; self.size.0 * self.size.1];
        }
        // No frame yet (the size comes with the first resize): nothing to place.
        if w == 0. {
            return;
        }
        let circle = self.circle.unwrap_or_else(|| panel_circle(w, h));
        let layout = Layout::new(w, h, circle);
        self.placed = layout.place(self.calc.state.base, self.calc.second, self.calc.picking);
        self.sizes = vec![0.; self.placed.len()];
        self.layout = Some(layout);
    }

    /// The settings and the history to the vault's writer, stamped now.
    pub(crate) fn save(&self) {
        let Some(vault) = &self.vault else { return };
        match self.calc.saved().build(vsf::types::eagle_time::eagle_time_oscillations()) {
            Ok(doc) => {
                let _ = vault.send(doc);
            }
            Err(e) => println!("basecalc: the state did not build: {e}"),
        }
    }

    /// A key's face, as the moment has it.
    fn label(&self, p: &Placed, layout: &Layout) -> (String, Option<&'static str>) {
        let base = self.calc.state.base;
        let in_picker = self.calc.picking && p.rect.y >= layout.functions.y && p.key != Key::Equals;
        match p.key {
            Key::Angle => ((if self.calc.state.radians { "rad" } else { "deg" }).to_string(), None),
            Key::Bases if in_picker => ("\u{2715}".to_string(), None),
            Key::Bases if !keys::QUICK_BASES.contains(&base) => (keys::base_label(base), None),
            Key::Base(b) if in_picker => (keys::base_label(b), get_base_name(b)),
            key => (key.label(), None),
        }
    }

    fn draw_key(&mut self, i: usize, canvas: &mut Canvas<G2t16>, text: &mut TextRenderer, pressed: HitId, layout: &Layout) {
        let p = self.placed[i];
        let id = (i + 1) as HitId;
        let (w, h) = (canvas.width, canvas.height);
        let r = p.rect;
        let (x0, y0, x1, y1) = (r.x.round(), r.y.round(), (r.x + r.w).round(), (r.y + r.h).round());
        let (kw, kh) = (x1 - x0, y1 - y0);
        // The corners from the key's own span (the harmonic mean of its sides), the house's offset diagonals.
        let radii = asymmetric_radii(2. * kw * kh / (kw + kh) * (1. / (1 << 2) as f32));
        let (label, name) = self.label(&p, layout);
        if self.sizes[i] == 0. {
            // Three eighths of the key's height, eased down to fit four fifths of its width: a smooth minimum, so a label does not kink as the window resizes.
            let full = kh * (3. / (1 << 3) as f32);
            let measured = text.measure_text(&label, &TextStyle::new(full, INK));
            let room = kw * (4. / 5.);
            self.sizes[i] = full * smooth_min(1., room / measured, 1. / (1 << 3) as f32);
        }
        let size = self.sizes[i];
        let reach = paint::reach_rect(x0 as isize, y0 as isize, x1 as isize, y1 as isize, w, h);
        self.shown.before(canvas.pixels, w, h, reach);
        let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        match name {
            Some(name) => {
                text.draw_text_center(canvas, &label, cx, cy - kh * (1. / (1 << 3) as f32), &TextStyle::new(size, INK), None, None);
                let small = kh * (1. / (1 << 3) as f32);
                text.draw_text_center(canvas, name, cx, cy + kh * (1. / (1 << 2) as f32), &TextStyle::new(small, INK_DIM), None, None);
            }
            None => {
                text.draw_text_center(canvas, &label, cx, cy, &TextStyle::new(size, INK), None, None);
            }
        }
        draw_squircle_rrect(canvas, x0 as isize, y0 as isize, kw as isize, kh as isize, body(p.face, pressed == id), radii, SHAPE_POWER);
        self.shown.settle(canvas.pixels);
        self.shown.claim_shape(&mut self.hits, id, |x, y| in_shape(x0, y0, x1, y1, radii, x, y));
    }

    /// The display: the result at the bottom (up to three lines), the expression above it, the history above that as far as it reaches; then the precision in its cell of the settings row.
    fn draw_display(&self, canvas: &mut Canvas<G2t16>, text: &mut TextRenderer, layout: &Layout) {
        let d = layout.display;
        let right = d.x + d.w;
        let (fr, fe, fh) = (layout.pitch * 0.5, layout.pitch * (3. / (1 << 3) as f32), layout.pitch * (1. / (1 << 2) as f32));
        let calc = &self.calc;
        let base = calc.state.base;
        let result = match &calc.shown {
            Shown::Nothing => Vec::new(),
            Shown::Preview(spans) => wrap(text, result_runs(base, spans, true), d.w, fr, 3),
            Shown::Value(spans) => wrap(text, result_runs(base, spans, false), d.w, fr, 3),
            Shown::Error(why) => vec![vec![(run(why.clone(), INK_ERROR), text.measure_text(why, &TextStyle::new(fe, INK_ERROR)))]],
        };
        let line_r = fr * 1.25;
        let mut y = d.y + d.h;
        let result_size = if matches!(calc.shown, Shown::Error(_)) { fe } else { fr };
        for line in result.iter().rev() {
            y -= line_r;
            draw_line(canvas, text, line, right, y + line_r * 0.5, result_size);
        }
        if result.is_empty() {
            y -= line_r;
        }
        let line_e = fe * 1.5;
        y -= line_e;
        let expr = tail(text, expression_runs(&calc.expr, calc.done), d.w, fe);
        draw_line(canvas, text, &expr, right, y + line_e * 0.5, fe);
        let line_h = fh * 1.5;
        for entry in calc.history.iter().rev() {
            if y - line_h < d.y {
                break;
            }
            y -= line_h;
            let mut runs = expression_runs(&entry.line.expr, true);
            runs.push(run("  =  ", INK_DIM));
            runs.push(run(keys::base_label(entry.line.base), INK_DIM));
            runs.push(run(spans_text(&entry.spans).trim_start(), INK_DIM));
            let line = tail(text, runs, d.w, fh);
            draw_line(canvas, text, &line, right, y + line_h * 0.5, fh);
        }
        // The precision, in the base in use, between its two keys.
        let cell = layout.precision_label();
        let digits = format!("{}{}", keys::base_label(base), keys::in_base(calc.state.digits, base));
        let (cx, cy) = (cell.x + cell.w * 0.5, cell.y + cell.h * 0.5);
        text.draw_text_center(canvas, &digits, cx, cy - cell.h * (1. / (1 << 3) as f32), &TextStyle::new(cell.h * (3. / (1 << 3) as f32), INK), None, None);
        text.draw_text_center(canvas, "digits", cx, cy + cell.h * (1. / (1 << 2) as f32), &TextStyle::new(cell.h * (1. / (1 << 3) as f32), INK_DIM), None, None);
    }
}

impl FluorApp<G2t16> for Pad {
    type UserEvent = ();

    fn init(&mut self, _ctx: &mut Context) {}

    fn on_resize(&mut self, width: u32, height: u32, ctx: &mut Context) {
        self.size = (width as usize, height as usize);
        self.place();
        ctx.window.request_redraw();
    }

    #[cfg(feature = "safe-area")]
    fn on_safe_area(&mut self, area: fluor::host::app::SafeArea) {
        self.circle = Some(Circle { cx: area.cx, cy: area.cy, r: area.r, top: area.top, bottom: area.bottom });
        self.place();
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        // A key held shows held: the host keeps which (ctx.pressed_hit), the frame is drawn again to show it.
        if let Event::MouseInput { state: ElementState::Pressed | ElementState::Released, button: MouseButton::Left } = event {
            ctx.window.request_redraw();
        }
        EventResponse::Pass
    }

    fn on_activate(&mut self, hit_id: HitId, _x: Coord, _y: Coord, _mods: ModifiersState, ctx: &mut Context) -> EventResponse {
        // The host fires only a held id, never HIT_NONE (fluor pointer.rs `on_up`), so the id is at least 1, and every id in the map was stamped by the frame that drew `drawn`, one past its index.
        let p = self.drawn[hit_id as usize - 1];
        let change = self.calc.press(p.key);
        if change.relayout {
            self.place();
        }
        if change.save {
            self.save();
        }
        ctx.window.request_redraw();
        EventResponse::Handled
    }

    fn render(&mut self, target: &mut [G2t16], ctx: &mut Context) {
        let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
        let pressed = ctx.pressed_hit;
        self.draw_all(target, w, h, &mut *ctx.text, pressed, ctx.damage);
    }

    fn hit_test_map(&mut self) -> Option<(&mut [HitId], usize, usize)> {
        Some((&mut self.hits, self.size.0, self.size.1))
    }

    fn opaque(&self) -> bool {
        true
    }

    fn cursor_for(&self, _x: Coord, _y: Coord, _ctx: &Context) -> CursorIcon {
        CursorIcon::Default
    }
}

impl Pad {
    /// The whole pad into `target` (`w` × `h`), the keys' tap areas into the hit map: the window's frame, and the basecalc puck's square (puck.rs).
    pub(crate) fn draw_all(&mut self, target: &mut [G2t16], w: usize, h: usize, text: &mut TextRenderer, pressed: HitId, damage: &mut fluor::canvas::Damage) {
        if (w, h) != self.size {
            self.size = (w, h);
            self.place();
        }
        let Some(layout) = self.layout else { return };
        // The map starts each frame empty: the host clears it over the frame's damage before `render`, and the damage is the whole frame (fluor's default).
        let mut canvas = Canvas::new(target, w, h, damage);
        // Front to back (AGENT.md "Compositing"): the text in front of everything, then each key (its label in front of its body), then the ground behind it all.
        self.draw_display(&mut canvas, text, &layout);
        for i in 0..self.placed.len() {
            self.draw_key(i, &mut canvas, text, pressed, &layout);
        }
        paint::fill_rect(&mut canvas, 0, 0, w as isize, h as isize, colour(GROUND), None, None);
        self.drawn.clone_from(&self.placed);
    }
}

/// The kist's vault domain: its key from the kist's ira, the store the kernel scopes to the kist (slot 5). None without either: basecalc then keeps nothing this launch, and says so.
pub(crate) fn open_vault() -> Option<Arc<kete::FlatStorage>> {
    let Some(secret) = sys::ira_key(IRA, "basecalc vault") else {
        println!("basecalc: no ira key (slot {IRA}): nothing is kept this launch");
        return None;
    };
    match kete::FlatStorage::open_device_shared(kete::App { id: "basecalc", dir: "basecalc" }, secret) {
        Ok(v) => Some(v),
        Err(e) => {
            println!("basecalc: the vault did not open ({e:?}): nothing is kept this launch");
            None
        }
    }
}

/// What the vault kept, run again (`Calc::restore`), or a fresh calculator.
pub(crate) fn restore(vault: &kete::FlatStorage) -> Calc {
    match vault.read_device(store::NAME) {
        Ok(Some(doc)) => match Saved::parse(&doc) {
            Ok(saved) => {
                let calc = Calc::restore(&saved);
                println!("basecalc: {} history lines back, base {}", calc.history.len(), keys::base_label(calc.state.base));
                calc
            }
            Err(why) => {
                println!("basecalc: the kept state does not read ({why}): starting fresh");
                Calc::new()
            }
        },
        Ok(None) => Calc::new(),
        Err(e) => {
            println!("basecalc: the kept state did not come back ({e:?}): starting fresh");
            Calc::new()
        }
    }
}

/// The vault's writer: each document as it comes, only the newest when several wait, so a run of quick changes is one write.
pub(crate) fn writer(vault: Arc<kete::FlatStorage>) -> Sender<Vec<u8>> {
    let (tx, rx) = channel::<Vec<u8>>();
    let spawned = std::thread::Builder::new().name("basecalc-vault".into()).spawn(move || {
        while let Ok(mut doc) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                doc = newer;
            }
            if let Err(e) = vault.write_device(store::NAME, &doc) {
                println!("basecalc: the state was not kept: {e:?}");
            }
        }
    });
    if let Err(e) = spawned {
        println!("basecalc: no vault writer ({e}): nothing is kept this launch");
    }
    tx
}

pub fn main() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("basecalc: PANIC at {at}: {info}");
    }));
    println!("basecalc: basecalc {} on ferros, {} grants", env!("CARGO_PKG_VERSION"), sys::grant_count());
    let vault = open_vault();
    let calc = vault.as_deref().map(restore).unwrap_or_default();
    let pad = Pad::new(calc, vault.map(writer));
    let caps = fluor::host::ferros::Caps { display: DISPLAY, touch: TOUCH, surface: SURFACE };
    // In the person's app scale and the device's theme, following both, a pinch written back as the app scale with the display grant (ferros_fluor; owner, 2026-11-20: every kist follows them).
    match ferros_fluor::run_app(pad, caps, None, sys::cap(DISPLAY)) {
        Ok(()) => println!("basecalc: closed"),
        Err(e) => println!("basecalc: {e}"),
    }
}

