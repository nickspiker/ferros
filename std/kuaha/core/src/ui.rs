//! Kūaha on the glass: everything inside the app's circle (WINDOWS.md "Drawn under the controls": the window fills the glass, the corners are the system's), drawn front to back with fluor's `under` (AGENT.md "Compositing"), every length a fraction of the circle's radius (AGENT.md "Dimensional Units").
//! Two views, the person's things and this device, in one column that scrolls: a finger drags it 1:1, and after the lift it keeps the largest of the last three frames' movements and loses a little each frame (feedback_no_animations: frame-counted physics, never a clock); past either end it comes back by halving the excess each frame.
//! A tap is a press and a lift without moving past the slop, however long it took; taps are answered by this module's own rects, so a drag that began on a card never opens it.

use fluor::canvas::{Canvas, Damage};
use fluor::paint::{self, AlphaMask};
use fluor::paint::squircle::{asymmetric_radii, draw_squircle_rrect, SHAPE_POWER};
use fluor::pixel::G2t16;
use fluor::text::{TextRenderer, TextStyle};
use ferros_lang::{Msg, Say};

use crate::kinds::{self, Mark};
use crate::look::{flavour, Colours, Locale};
use crate::model::{order_words, Answer, Ask, Handle, Load, Model, Prefs, View};
use crate::source::{Problem, Usage, CLIP_BLOCK};

/// The face every word is drawn in, as the deck draws its own (the display's Oxanium +glyphs: Latin with Māori's macrons, and the dozenal digits at G#10 to G#1B that tatau's glyphs style writes).
pub const FACE: &[u8] = include_bytes!("../../../../assets/fonts/Oxanium-Regular+glyphs.ttf");

/// The app's circle in the frame's pixels (fluor's `SafeArea`, as the display tells it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

/// The circle the display gives a window filling a `w` x `h` panel until it says (ferros_sys `active_circle`): tangent to the bottom of the camera cutout and the bottom of the glass. The cutout's bottom is each panel's measured fact; another panel takes the Pro's as a share of its height.
pub fn panel_circle(w: f32, h: f32) -> Circle {
    let cutout = match (w as u32, h as u32) {
        (1344, 2992) => 118.5610,
        (1080, 2400) => 101.8303,
        _ => h * (118.5610 / 2992.),
    };
    let r = (h - cutout) * 0.5;
    Circle { cx: w * 0.5, cy: cutout + r, r }
}

/// A smooth minimum: under both `a` and `b` by about `k` / 2 where they meet, C-infinity (AGENT.md "GUI Continuity": a width that follows the frame never kinks).
fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    (a + b - ((a - b) * (a - b) + k * k).sqrt()) * 0.5
}

/// What a tap lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Tab(View),
    Search,
    Kind(Option<[u8; 32]>),
    When,
    Order,
    Maker,
    Item(Handle),
    Open(Handle),
    Remove(Handle),
    More,
}

/// A row of the column, in the order it is drawn top to bottom, its height in units (`Ui::u`).
#[derive(Clone, Debug)]
enum Row {
    Tabs,
    Search,
    Kinds(usize),
    Conditions,
    /// Words alone, one line each, centred.
    Note(Msg, bool),
    Item(Handle),
    More,
    Heading(Msg),
    KindTotal { kind: [u8; 32], items: u64, bytes: u64 },
    Fact(Msg, String),
    Clip { owner: String, bytes: u64, sealed: bool },
    /// A stowed kist: its own domain's bytes (None until sized) and its clips in the clips area.
    Kist { name: String, bytes: Option<u64>, clip_bytes: u64 },
}

/// A press in flight.
#[derive(Clone, Copy, Debug)]
struct Press {
    x: f32,
    y: f32,
    last: f32,
    /// Past the slop: a drag of the column, never a tap.
    moved: bool,
}

/// Kūaha: the model and its face.
pub struct Kuaha {
    pub model: Model,
    size: (usize, usize),
    circle: Option<Circle>,
    /// The column's offset, pixels: 0 shows its top.
    scroll: f32,
    /// The column's height as the last frame laid it out.
    content: f32,
    /// After a lift: pixels a frame, losing `decay()` each frame.
    velocity: f32,
    /// The finger's movement in this frame so far, and in the last three frames.
    this_frame: f32,
    recent: [f32; 3],
    press: Option<Press>,
    hits: Vec<(f32, f32, f32, f32, Hit)>,
    /// The circle's coverage of each pixel, built once a size.
    mask: Vec<u8>,
    face: Option<String>,
    /// The search line has the keyboard.
    typing: bool,
    /// A change of wish for the keyboard, not yet told to the platform.
    keyboard: Option<bool>,
}

impl Kuaha {
    pub fn new(prefs: Option<Prefs>, now: i64) -> Kuaha {
        Kuaha {
            model: Model::new(prefs, now),
            size: (0, 0),
            circle: None,
            scroll: 0.,
            content: 0.,
            velocity: 0.,
            this_frame: 0.,
            recent: [0.; 3],
            press: None,
            hits: Vec::new(),
            mask: Vec::new(),
            face: None,
            typing: false,
            keyboard: None,
        }
    }

    /// The display's circle (`on_safe_area`); until it says, the display's way of working it out.
    pub fn set_circle(&mut self, c: Circle) {
        self.circle = Some(c);
        self.mask.clear();
    }

    pub fn take_asks(&mut self) -> Vec<Ask> {
        self.model.take_asks()
    }

    pub fn answer(&mut self, a: Answer) {
        self.model.answer(a);
    }

    pub fn take_prefs(&mut self) -> Option<Prefs> {
        self.model.take_prefs()
    }

    /// The keyboard wanted or let go, once per change (fluor's `wants_keyboard`).
    pub fn wants_keyboard(&mut self) -> Option<bool> {
        self.keyboard.take()
    }

    /// Back in front: the hold may have changed meanwhile.
    pub fn focus(&mut self, front: bool) {
        if front {
            self.model.refresh();
        }
    }

    fn circle(&self) -> Circle {
        self.circle.unwrap_or_else(|| panel_circle(self.size.0 as f32, self.size.1 as f32))
    }

    /// The unit every length is a multiple of: a 32nd of the circle's radius (45 pixels on the Pro).
    fn u(&self) -> f32 {
        self.circle().r * (1. / (1 << 5) as f32)
    }

    /// What a lifted fling loses each frame: a 1024th of the radius (about a pixel on the Pro, the settings pages' rule).
    fn decay(&self) -> f32 {
        self.circle().r * (1. / (1 << 10) as f32)
    }

    /// How far the column may scroll.
    fn most(&self) -> f32 {
        let c = self.circle();
        // WHY max: a column shorter than the circle does not scroll at all; this is the target the physics settles to, never a clamp on motion.
        (self.content - (c.r * 2. - self.top_inset())).max(0.)
    }

    /// The column starts this far below the circle's top, clear of where its chord is narrower than the glass.
    fn top_inset(&self) -> f32 {
        self.circle().r * (1. / (1 << 3) as f32)
    }

    // ---- input ----

    pub fn press(&mut self, x: f32, y: f32) {
        self.velocity = 0.;
        self.recent = [0.; 3];
        self.press = Some(Press { x, y, last: y, moved: false });
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        let slop = self.u() * 0.5;
        let Some(p) = &mut self.press else { return };
        if !p.moved && ((x - p.x).abs() > slop || (y - p.y).abs() > slop) {
            p.moved = true;
        }
        if p.moved {
            let dy = y - p.last;
            self.scroll -= dy;
            self.this_frame -= dy;
        }
        p.last = y;
    }

    /// The finger lifted at (x, y): a tap if it never moved past the slop, else the column flies on.
    pub fn release(&mut self, x: f32, y: f32) {
        let Some(p) = self.press.take() else { return };
        if p.moved {
            // The largest of the last three frames' movements, with its sign.
            self.velocity = self.recent.iter().chain(core::iter::once(&self.this_frame)).copied().fold(0., |a: f32, b| if b.abs() > a.abs() { b } else { a });
            return;
        }
        let hit = self.hit_at(x, y);
        if hit.is_some() && hit == self.hit_at(p.x, p.y) {
            self.tap(hit.unwrap());
        } else {
            self.model.disarm();
            self.set_typing(false);
        }
    }

    /// The finger left without a lift (the window went behind): nothing fires.
    pub fn leave(&mut self) {
        self.press = None;
    }

    /// The centre of the first thing the last frame drew that `which` picks: where a test (or a desktop's keyboard) taps it.
    pub fn where_is(&self, which: impl Fn(&Hit) -> bool) -> Option<(f32, f32)> {
        self.hits.iter().find(|h| which(&h.4)).map(|h| ((h.0 + h.2) * 0.5, (h.1 + h.3) * 0.5))
    }

    /// The column's offset and how far it may go, for a test of the physics.
    pub fn scrolled(&self) -> (f32, f32) {
        (self.scroll, self.most())
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        let c = self.circle();
        let (dx, dy) = (x - c.cx, y - c.cy);
        if dx * dx + dy * dy > c.r * c.r {
            return None;
        }
        self.hits.iter().find(|h| x >= h.0 && x < h.2 && y >= h.1 && y < h.3).map(|h| h.4)
    }

    fn set_typing(&mut self, on: bool) {
        if self.typing != on {
            self.typing = on;
            self.keyboard = Some(on);
        }
    }

    fn tap(&mut self, hit: Hit) {
        if !matches!(hit, Hit::Remove(_)) {
            self.model.disarm();
        }
        if hit != Hit::Search {
            self.set_typing(false);
        }
        match hit {
            Hit::Tab(v) => {
                self.model.set_view(v);
                self.scroll = 0.;
            }
            Hit::Search => self.set_typing(true),
            Hit::Kind(k) => self.model.tap_kind(k),
            Hit::When => self.model.next_when(),
            Hit::Order => self.model.next_order(),
            Hit::Maker => self.model.next_maker(),
            Hit::Item(h) => self.model.tap_item(h),
            // The hand-off is not built (STORE.md step 1): the open only says which kist will take it.
            Hit::Open(_) => {}
            Hit::Remove(h) => self.model.tap_remove(h),
            Hit::More => self.model.more(),
        }
    }

    /// Text the keyboard typed (an IME commit).
    pub fn typed(&mut self, s: &str) {
        if self.typing {
            self.model.type_words(s);
        }
    }

    pub fn backspace(&mut self) {
        if self.typing {
            self.model.delete_word_char();
        }
    }

    /// Enter: the keyboard goes; the find already follows every key.
    pub fn enter(&mut self) {
        self.set_typing(false);
    }

    /// Back (the helm's Escape, owner 2026-11-18: "back continues up a level until stopping at the trunk of the kist"): the keyboard put away, an armed remove disarmed, an open card closed, the device's view left for the things; false at the trunk, where Back goes on to the deck.
    pub fn back(&mut self) -> bool {
        if self.typing {
            self.set_typing(false);
        } else if self.model.armed.is_some() {
            self.model.disarm();
        } else if self.model.open.is_some() {
            self.model.open = None;
        } else if self.model.view == View::Device {
            self.model.set_view(View::Things);
            self.scroll = 0.;
        } else {
            return false;
        }
        true
    }

    /// One shown frame: the physics stepped once. True while anything still moves.
    pub fn tick(&mut self) -> bool {
        self.recent = [self.recent[1], self.recent[2], self.this_frame];
        self.this_frame = 0.;
        if self.press.is_some() {
            return false;
        }
        let most = self.most();
        let mut moving = false;
        if self.velocity != 0. {
            self.scroll += self.velocity;
            let d = self.decay();
            // Toward zero by `d`, landing on zero: a step that would cross it stops there.
            self.velocity = if self.velocity.abs() <= d { 0. } else { self.velocity - d * self.velocity.signum() };
            moving = true;
        }
        // Past an end: the fling ends there and the excess halves each frame, landing on the end (the settings pages' snap).
        let excess = if self.scroll < 0. { self.scroll } else if self.scroll > most { self.scroll - most } else { 0. };
        if excess != 0. {
            self.velocity = 0.;
            let half = (excess * 0.5).trunc();
            self.scroll -= excess - half;
            moving = true;
        }
        moving
    }

    // ---- layout ----

    fn rows(&self, loc: &Locale) -> Vec<Row> {
        let m = &self.model;
        let mut rows = vec![Row::Tabs];
        match m.view {
            View::Things => {
                rows.push(Row::Search);
                let kinds = m.totals.ready().map_or(0, |t| t.kinds.len());
                rows.push(Row::Kinds(kinds + 1));
                rows.push(Row::Conditions);
                match &m.found {
                    Load::Asking => {}
                    Load::Failed(p) => rows.push(Row::Note(problem(*p), true)),
                    Load::Ready(items) if items.is_empty() => {
                        let empty = !m.query.narrowed() && m.totals.ready().is_none_or(|t| t.items == 0);
                        rows.push(Row::Note(if empty { Msg::KuahaEmpty } else { Msg::KuahaNothingMatches }, !empty));
                    }
                    Load::Ready(items) => {
                        rows.extend(items.iter().map(|(h, _)| Row::Item(*h)));
                        if (items.len() as u64) < m.total {
                            rows.push(Row::More);
                        }
                    }
                }
            }
            View::Device => {
                rows.push(Row::Heading(Msg::KuahaInTheHold));
                // The usage call counts every item whoever made it; until it answers (or on a hold without it) the totals of what Kūaha may open.
                let kinds = match (&m.usage, &m.totals) {
                    (Load::Ready(Some(u)), _) => Ok(&u.kinds),
                    (_, Load::Ready(t)) => Ok(&t.kinds),
                    (_, Load::Failed(p)) => Err(Some(*p)),
                    _ => Err(None),
                };
                match kinds {
                    Err(None) => {}
                    Err(Some(p)) => rows.push(Row::Note(problem(p), true)),
                    Ok(kinds) => {
                        for k in kinds {
                            rows.push(Row::KindTotal { kind: k.kind, items: k.items, bytes: k.bytes });
                        }
                        let (items, bytes) = kinds.iter().fold((0, 0), |(n, b), k| (n + k.items, b + k.bytes));
                        rows.push(Row::Fact(Msg::KuahaTotal, format!("{}  {}", loc.count(items), loc.size(bytes))));
                    }
                }
                device_rows(&mut rows, &m.usage, loc);
            }
        }
        rows
    }

    fn height(&self, row: &Row, cols: usize, loc: &Locale) -> f32 {
        match row {
            Row::Tabs => 3.,
            Row::Search => 2.75,
            Row::Kinds(n) => 4.5 * n.div_ceil(cols) as f32,
            Row::Conditions => 2.75,
            Row::Note(m, _) => 1.5 * (loc.t(*m).matches('\n').count() + 1) as f32 + 1.,
            Row::Item(h) => {
                if self.model.open == Some(*h) {
                    11.75 + if self.model.refused.is_some_and(|(r, _)| r == *h) { 1.25 } else { 0. }
                } else {
                    3.25
                }
            }
            Row::More => 2.5,
            Row::Heading(_) => 2.25,
            Row::KindTotal { .. } | Row::Clip { .. } => 2.25,
            Row::Kist { clip_bytes, .. } => if *clip_bytes > 0 { 2.75 } else { 2.25 },
            Row::Fact(..) => 1.75,
        }
    }

    // ---- drawing ----

    /// The whole frame into `target` (`w` x `h`), front to back: each row's words in front of its marks and fills, then the ground behind everything.
    pub fn draw(&mut self, target: &mut [G2t16], w: usize, h: usize, text: &mut TextRenderer, loc: &Locale, damage: &mut Damage) {
        if (w, h) != self.size {
            self.size = (w, h);
            self.mask.clear();
        }
        self.model.now = loc.now;
        if self.face.is_none() {
            self.face = Some(text.load_font_data_named(FACE.to_vec()).unwrap_or_else(|| "Open Sans".into()));
        }
        let c = self.circle();
        if self.mask.len() != w * h {
            self.mask = circle_mask(w, h, c);
        }
        let mask = core::mem::take(&mut self.mask);
        let colours = loc.colours();
        let u = self.u();
        // The column's width: the glass's, eased under the circle's diameter, less a unit each side.
        let width = smooth_min(w as f32, c.r * 2., u) - 2. * u;
        let left = (w as f32 - width) * 0.5;
        // WHY max: a frame narrower than one kind's slot (a desktop window dragged thin) still lays the kinds out one a line; zero columns would divide by zero in the kinds' rows.
        let cols = ((width / (4.5 * u)) as usize).max(1);
        let rows = self.rows(loc);
        self.hits.clear();
        let face = self.face.clone().unwrap_or_default();
        let mut canvas = Canvas::new(target, w, h, damage);
        let mut d = Draw { canvas: &mut canvas, text, mask: AlphaMask::new(&mask, w, h), face: &face, u, colours, loc };
        let top = c.cy - c.r + self.top_inset();
        let mut y = top - self.scroll;
        let (shown_top, shown_bottom) = (c.cy - c.r, c.cy + c.r);
        for row in &rows {
            let rh = self.height(row, cols, loc) * u;
            if y + rh > shown_top && y < shown_bottom {
                let hits = self.draw_row(&mut d, row, left, y, width, rh, cols);
                self.hits.extend(hits);
            }
            y += rh;
        }
        self.content = y + self.scroll - top + u * 2.;
        paint::fill_rect(d.canvas, 0, 0, w as isize, h as isize, colours.ground, None, None);
        self.mask = mask;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(&self, d: &mut Draw, row: &Row, x: f32, y: f32, w: f32, h: f32, cols: usize) -> Vec<(f32, f32, f32, f32, Hit)> {
        let (u, m, c) = (d.u, &self.model, d.colours);
        let mut hits = Vec::new();
        match row {
            Row::Tabs => {
                let half = (w - u) * 0.5;
                for (i, (v, words)) in [(View::Things, Msg::KuahaThings), (View::Device, Msg::KuahaDevice)].into_iter().enumerate() {
                    let px = x + i as f32 * (half + u);
                    let chosen = m.view == v;
                    let (py, ph) = (y + u * 0.5, u * 2.);
                    d.words_centred(d.loc.t(words), px + half * 0.5, py + ph * 0.5, u * 1.125, if chosen { c.text } else { c.text2 }, half - u);
                    if chosen {
                        d.pill(px, py, half, ph, c.plane);
                    }
                    hits.push((px, py, px + half, py + ph, Hit::Tab(v)));
                }
            }
            Row::Search => {
                let (py, ph) = (y + u * 0.25, u * 2.);
                let words = &m.query.words;
                let (shown, ink) = if words.is_empty() { (d.loc.t(Msg::KuahaSearch).to_string(), c.text3) } else { (words.clone(), c.text) };
                let typed = d.words_left(&shown, x + u, py + ph * 0.5, u, ink, w - 2. * u);
                if self.typing {
                    // Where the next letter goes: a still bar, never a blink (no timers).
                    let bx = if words.is_empty() { x + u } else { x + u + typed + u * 0.125 };
                    paint::fill_rect(d.canvas, bx as isize, (py + u * 0.5) as isize, (u * 0.125) as isize, u as isize, c.text, None, Some(&d.mask));
                }
                d.pill(x, py, w, ph, if self.typing { c.ring } else { c.plane });
                hits.push((x, py, x + w, py + ph, Hit::Search));
            }
            Row::Kinds(n) => {
                let totals = m.totals.ready();
                let slot = w / cols as f32;
                for i in 0..*n {
                    let (kind, count) = match i {
                        0 => (None, totals.map(|t| t.items)),
                        i => totals.and_then(|t| t.kinds.get(i - 1)).map_or((None, None), |k| (Some(k.kind), Some(k.items))),
                    };
                    let (col, line) = (i % cols, i / cols);
                    let cx = x + slot * (col as f32 + 0.5);
                    let cy = y + line as f32 * 4.5 * u + u * 1.75;
                    let r = u * 1.375;
                    let chosen = m.query.kind == kind;
                    if let Some(n) = count {
                        d.words_centred(&d.loc.count(n), cx, cy, u * 0.875, c.text, r * 1.75);
                    }
                    let name = kind.map_or(d.loc.t(Msg::KuahaEverything), |k| d.loc.t(kinds::words(&k)));
                    d.words_centred(name, cx, cy + r + u * 0.75, u * 0.6875, if chosen { c.text } else { c.text2 }, slot - u * 0.25);
                    let ring = if chosen { c.lit } else { kind.map_or(c.ring, |k| flavour(&k)) };
                    d.ringed(cx, cy, r, ring, c.plane, Mark::Ring);
                    hits.push((cx - slot * 0.5, cy - r - u * 0.25, cx + slot * 0.5, cy + r + u * 1.5, Hit::Kind(kind)));
                }
            }
            Row::Conditions => {
                let maker = match m.query.maker {
                    None => d.loc.t(Msg::KuahaAnyMaker).to_string(),
                    Some(tag) => m.totals.ready().and_then(|t| t.makers.iter().find(|x| x.tag == tag)).map_or_else(|| d.loc.t(Msg::KuahaAnyMaker).to_string(), |x| x.name.clone()),
                };
                let parts = [(d.loc.t(m.query.when.words()).to_string(), Hit::When, m.query.when != crate::model::When::Any), (d.loc.t(order_words(m.query.order)).to_string(), Hit::Order, false), (maker, Hit::Maker, m.query.maker.is_some())];
                let third = (w - u) / 3.;
                for (i, (words, hit, narrowed)) in parts.into_iter().enumerate() {
                    let px = x + i as f32 * (third + u * 0.5);
                    let (py, ph) = (y + u * 0.375, u * 1.875);
                    d.words_centred(&words, px + third * 0.5, py + ph * 0.5, u * 0.75, if narrowed { c.text } else { c.text2 }, third - u * 0.5);
                    d.pill(px, py, third, ph, if narrowed { c.ring } else { c.plane });
                    hits.push((px, py, px + third, py + ph, hit));
                }
            }
            Row::Note(msg, dim) => {
                let lines: Vec<&str> = d.loc.t(*msg).split('\n').collect();
                for (i, line) in lines.iter().enumerate() {
                    let ink = if i == 0 && !dim { c.text } else { c.text2 };
                    d.words_centred(line, x + w * 0.5, y + u * (1.25 + 1.5 * i as f32), u * if i == 0 && !dim { 1. } else { 0.8125 }, ink, w);
                }
            }
            Row::Item(handle) => {
                if let Some(card) = m.card(handle) {
                    hits.extend(self.draw_item(d, *handle, card, x, y, w, h));
                }
            }
            Row::More => {
                let (py, ph) = (y + u * 0.25, u * 2.);
                let shown = m.found.ready().map_or(0, |v| v.len() as u64);
                let words = format!("{}  {} / {}", d.loc.t(Msg::KuahaMore), d.loc.count(shown), d.loc.count(m.total));
                d.words_centred(&words, x + w * 0.5, py + ph * 0.5, u * 0.875, c.text2, w - 2. * u);
                d.pill(x, py, w, ph, c.plane);
                hits.push((x, py, x + w, py + ph, Hit::More));
            }
            Row::Heading(msg) => {
                d.words_left(d.loc.t(*msg), x + u * 0.5, y + h * 0.6, u * 0.875, c.text2, w - u);
            }
            Row::KindTotal { kind, items, bytes } => {
                let cy = y + h * 0.5;
                let r = u * 0.75;
                d.words_left(d.loc.t(kinds::words(kind)), x + u * 2.25, cy, u * 0.9375, c.text, w * 0.5);
                d.words_right(&format!("{}  {}", d.loc.count(*items), d.loc.size(*bytes)), x + w - u * 0.5, cy, u * 0.875, c.text2, w * 0.4);
                d.ringed(x + u * 1.125, cy, r, flavour(kind), c.plane, kinds::mark(kind));
            }
            Row::Fact(msg, value) => {
                let cy = y + h * 0.5;
                d.words_left(d.loc.t(*msg), x + u * 0.5, cy, u * 0.8125, c.text2, w * 0.5);
                d.words_right(value, x + w - u * 0.5, cy, u * 0.875, c.text, w * 0.5);
            }
            Row::Clip { owner, bytes, sealed } => {
                let cy = y + h * 0.5;
                let right = if *sealed { d.loc.size(*bytes) } else { format!("{}  {}", d.loc.t(Msg::KuahaRecording), d.loc.size(*bytes)) };
                d.words_left(owner, x + u * 2.25, cy, u * 0.9375, c.text, w * 0.5);
                d.words_right(&right, x + w - u * 0.5, cy, u * 0.875, if *sealed { c.text2 } else { c.lit }, w * 0.45);
                d.ringed(x + u * 1.125, cy, u * 0.75, flavour(&ferros_items::kinds::RAW_VIDEO), c.plane, Mark::Dot);
            }
            Row::Kist { name, bytes, clip_bytes } => {
                let cy = if *clip_bytes > 0 { y + h * 0.375 } else { y + h * 0.5 };
                d.words_left(name, x + u * 2.25, cy, u * 0.9375, c.text, w * 0.55);
                // Not yet sized: the row stands with no number until its size arrives.
                if let Some(b) = bytes {
                    d.words_right(&d.loc.size(*b), x + w - u * 0.5, cy, u * 0.875, c.text2, w * 0.4);
                }
                if *clip_bytes > 0 {
                    let line = format!("{}  {}", d.loc.t(Msg::KuahaClipsArea), d.loc.size(*clip_bytes));
                    d.words_right(&line, x + w - u * 0.5, y + h * 0.75, u * 0.6875, c.text2, w * 0.7);
                }
                d.ringed(x + u * 1.125, y + h * 0.5, u * 0.75, if *clip_bytes > 0 { flavour(&ferros_items::kinds::RAW_VIDEO) } else { c.ring }, c.plane, if *clip_bytes > 0 { Mark::Dot } else { Mark::Ring });
            }
        }
        hits
    }

    /// One item's card: its circle, its title and a line of facts; opened, its facts one a line and its two actions.
    #[allow(clippy::too_many_arguments)]
    fn draw_item(&self, d: &mut Draw, h: Handle, card: &ferros_items::Card, x: f32, y: f32, w: f32, rh: f32) -> Vec<(f32, f32, f32, f32, Hit)> {
        let (u, c, loc) = (d.u, d.colours, d.loc);
        let mut hits = Vec::new();
        let kind_words = loc.t(kinds::words(&card.kind));
        let title = card.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| kind_words.to_string());
        let recording = card.extent.is_some() && card.size == 0;
        let open = self.model.open == Some(h);
        let (cx, cy, r) = (x + u * 1.75, y + u * 1.625, u * 1.125);
        let text_x = x + u * 3.5;
        let text_w = w - u * 4.;
        d.words_left(&title, text_x, y + u * 1.125, u, c.text, text_w);
        let mut facts = vec![kind_words.to_string(), card.kist.name.clone(), loc.when(card.made)];
        facts.push(if recording { loc.t(Msg::KuahaRecording).to_string() } else { loc.size(card.size) });
        d.words_left(&facts.join("  \u{b7}  "), text_x, y + u * 2.25, u * 0.6875, if recording { c.lit } else { c.text2 }, text_w);
        d.ringed(cx, cy, r, flavour(&card.kind), c.plane, kinds::mark(&card.kind));
        hits.push((x, y, x + w, y + u * 3.25, Hit::Item(h)));
        if open {
            let mut fy = y + u * 3.75;
            let size = if recording { loc.t(Msg::KuahaRecording).to_string() } else { loc.size(card.size) };
            for (label, value) in [(Msg::KuahaKind, kind_words.to_string()), (Msg::MadeBy, card.kist.name.clone()), (Msg::KuahaMade, loc.when(card.made)), (Msg::Size, size)] {
                d.words_left(loc.t(label), text_x, fy, u * 0.6875, c.text3, text_w * 0.4);
                d.words_right(&value, x + w - u, fy, u * 0.75, c.text, text_w * 0.6);
                fy += u * 1.25;
            }
            // The actions: open, greyed, naming the kist that will take it; remove, two taps.
            let (ay, ah) = (fy - u * 0.25, u * 1.75);
            let half = (w - u * 3.) * 0.5;
            let (ox, rx) = (x + u, x + u * 2. + half);
            let open_words = kinds::opener(&card.kind).map_or_else(|| loc.t(Msg::KuahaNoOpener).to_string(), |k| loc.say(Say::OpenIn(k)));
            d.words_centred(&open_words, ox + half * 0.5, ay + ah * 0.5, u * 0.75, c.text3, half - u * 0.5);
            d.pill(ox, ay, half, ah, c.ground);
            d.words_centred(loc.t(Msg::KuahaHandOffLater), ox + half * 0.5, ay + ah + u * 0.625, u * 0.5625, c.text3, half);
            hits.push((ox, ay, ox + half, ay + ah, Hit::Open(h)));
            let armed = self.model.armed == Some(h);
            let remove_words = loc.t(if armed { Msg::TapAgainRemove } else { Msg::KuahaRemove });
            d.words_centred(remove_words, rx + half * 0.5, ay + ah * 0.5, u * 0.75, c.text, half - u * 0.5);
            d.pill(rx, ay, half, ah, if armed { c.armed } else { c.ground });
            hits.push((rx, ay, rx + half, ay + ah, Hit::Remove(h)));
            if let Some((_, p)) = self.model.refused.filter(|(r, _)| *r == h) {
                let why = if p == Problem::Refused { Msg::KuahaNotRemoved } else { problem(p) };
                d.words_centred(loc.t(why).split('\n').next().unwrap_or_default(), x + w * 0.5, ay + ah + u * 1.75, u * 0.6875, c.lit, w - u);
            }
        }
        // An opened card's own plane, behind its words and circle; a closed one stands on the ground.
        if open {
            let radii = asymmetric_radii(u * 0.75);
            draw_squircle_rrect(d.canvas, x as isize, (y + u * 0.125) as isize, w as isize, (rh - u * 0.25) as isize, c.plane, radii, SHAPE_POWER);
        }
        hits
    }
}

/// The device view's rows past the hold's totals: the RAW clips area and each kist's own storage, or that this device does not count them yet.
fn device_rows(rows: &mut Vec<Row>, usage: &Load<Option<Usage>>, loc: &Locale) {
    let usage = match usage {
        Load::Asking => return,
        Load::Failed(p) => {
            rows.push(Row::Heading(Msg::KuahaClipsArea));
            rows.push(Row::Note(problem(*p), true));
            return;
        }
        Load::Ready(u) => u,
    };
    rows.push(Row::Heading(Msg::KuahaClipsArea));
    match usage.as_ref().and_then(|u| u.clips.as_ref()) {
        None => rows.push(Row::Note(Msg::KuahaNotCounted, true)),
        Some(a) => {
            // WHY saturating: both counts are the vault's words; a table that says more taken than it has reads as none free, never as a wrapped huge one.
            let free = a.blocks.saturating_sub(a.taken);
            rows.push(Row::Fact(Msg::KuahaUsed, format!("{}  {}", loc.size(a.taken * CLIP_BLOCK), loc.share(a.taken, a.blocks))));
            rows.push(Row::Fact(Msg::KuahaFree, loc.size(free * CLIP_BLOCK)));
            rows.push(Row::Fact(Msg::KuahaLargestStretch, loc.size(a.largest_free * CLIP_BLOCK)));
            for clip in &a.clips {
                rows.push(Row::Clip { owner: clip.owner.clone(), bytes: clip.bytes, sealed: clip.sealed });
            }
        }
    }
    rows.push(Row::Heading(Msg::KuahaKistsKeep));
    match usage {
        None => rows.push(Row::Note(Msg::KuahaNotCounted, true)),
        Some(u) => {
            for k in &u.kists {
                rows.push(Row::Kist { name: k.name.clone(), bytes: k.bytes, clip_bytes: k.clip_bytes });
            }
        }
    }
}

/// What the person is told for a refusal.
fn problem(p: Problem) -> Msg {
    match p {
        Problem::NoRight => Msg::KuahaNoRight,
        _ => Msg::KuahaHoldSilent,
    }
}

/// The circle's coverage of every pixel, one antialiased pixel at its edge.
fn circle_mask(w: usize, h: usize, c: Circle) -> Vec<u8> {
    let mut m = vec![0u8; w * h];
    let (inner, outer) = ((c.r - 0.5) * (c.r - 0.5), (c.r + 0.5) * (c.r + 0.5));
    for y in 0..h {
        let dy = y as f32 + 0.5 - c.cy;
        for x in 0..w {
            let dx = x as f32 + 0.5 - c.cx;
            let d2 = dx * dx + dy * dy;
            m[y * w + x] = if d2 <= inner {
                255
            } else if d2 >= outer {
                0
            } else {
                ((c.r + 0.5 - d2.sqrt()) * 255.) as u8
            };
        }
    }
    m
}

/// One frame's drawing state: the canvas, the words, the circle's mask, the unit and the theme.
struct Draw<'a, 'b> {
    canvas: &'a mut Canvas<'b, G2t16>,
    text: &'a mut TextRenderer,
    mask: AlphaMask<'a>,
    face: &'a str,
    u: f32,
    colours: Colours,
    loc: &'a Locale,
}

impl Draw<'_, '_> {
    /// `s` cut to `room` pixels at `size`, an ellipsis where it was cut.
    fn fit(&mut self, s: &str, size: f32, room: f32) -> String {
        let style = TextStyle::new(size, 0).font(self.face);
        if self.text.measure_text(s, &style) <= room {
            return s.to_string();
        }
        let mut chars: Vec<char> = s.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t: String = chars.iter().collect::<String>() + "\u{2026}";
            if self.text.measure_text(&t, &style) <= room {
                return t;
            }
        }
        String::new()
    }

    /// Left-aligned at `x`, centred on `y`: the width drawn.
    fn words_left(&mut self, s: &str, x: f32, y: f32, size: f32, ink: u32, room: f32) -> f32 {
        let s = self.fit(s, size, room);
        let style = TextStyle::new(size, ink).font(self.face);
        self.text.draw_text_left(self.canvas, &s, x, y, &style, None, Some(&self.mask))
    }

    fn words_right(&mut self, s: &str, x: f32, y: f32, size: f32, ink: u32, room: f32) {
        let s = self.fit(s, size, room);
        let style = TextStyle::new(size, ink).font(self.face);
        self.text.draw_text_right(self.canvas, &s, x, y, &style, None, Some(&self.mask));
    }

    fn words_centred(&mut self, s: &str, x: f32, y: f32, size: f32, ink: u32, room: f32) {
        let s = self.fit(s, size, room);
        let style = TextStyle::new(size, ink).font(self.face);
        self.text.draw_text_center(self.canvas, &s, x, y, &style, None, Some(&self.mask));
    }

    /// A pill behind what was drawn over it: the house squircle, its corners from its own height.
    fn pill(&mut self, x: f32, y: f32, w: f32, h: f32, fill: u32) {
        let radii = asymmetric_radii(h * 0.5);
        draw_squircle_rrect(self.canvas, x as isize, y as isize, w as isize, h as isize, fill, radii, SHAPE_POWER);
    }

    /// A kind's circle: its mark in front, then its plane, then its ring behind (a ring is a disc under a smaller one).
    fn ringed(&mut self, cx: f32, cy: f32, r: f32, ring: u32, plane: u32, mark: Mark) {
        let stroke = r * (1. / (1 << 4) as f32);
        match mark {
            Mark::Lens => {
                paint::draw_circle(self.canvas, cx, cy, r * 0.25, ring, None);
                paint::draw_circle(self.canvas, cx, cy, r * 0.5, plane, None);
                paint::draw_circle(self.canvas, cx, cy, r * 0.5 + stroke, ring, None);
            }
            Mark::Dot => paint::draw_circle(self.canvas, cx, cy, r * 0.375, ring, None),
            Mark::Lines => {
                let (lw, lh) = (r, stroke * 1.5);
                for k in 0..3 {
                    let ly = cy - r * 0.375 + k as f32 * r * 0.375;
                    let len = if k == 2 { lw * 0.625 } else { lw };
                    paint::fill_rect(self.canvas, (cx - lw * 0.5) as isize, (ly - lh * 0.5) as isize, len as isize, lh as isize, ring, None, Some(&self.mask));
                }
            }
            Mark::Ring => {}
        }
        paint::draw_circle(self.canvas, cx, cy, r - stroke, plane, None);
        paint::draw_circle(self.canvas, cx, cy, r, ring, None);
    }
}
