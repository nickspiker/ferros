//! Pucks on the glass (docs/PUCKS.md; owner, 2026-11-17: "Pucks! I'm DOWN!"): the lists of drawing ops the pucks send, kept here and drawn into their circles on the deck.
//!
//! - **Who sends.** A puck calls the `puck` endpoint (grant slot 9), and the kernel puts its kist identity (developer key and option) ahead of every call (process.rs `puck_stamp`): a list is kept under that identity, never under anything the puck says.
//! - **Which circle.** The deck (the map) binds each puck's identity to a scene icon, `PUCK0 + j` for its j-th puck (op 73, the system's alone); the scene draws icon `PUCK0 + j` from the list kept under the identity bound to j. A puck the deck has not bound yet keeps its list here for when it is.
//! - **What is drawn.** ferros_puck checks a list whole before it is kept (the last good one stays), and draws it front to back thru `under` into the circle, at the circle's size and turn; the circle is the clip. Two variants: at rest, and in focus (the deck's focus op 25 with the puck's icon), the fuller face.
//! - **When it is drawn again.** When a new list arrives, when the focus moves to it or away, and when its time-anchored ops (a sweep, a marker) move half a pixel at the size it was last drawn (`ferros_puck::draw::next_change`): `due`, asked each pass of the render thread, which wakes at least once a second. Nothing is polled from a puck.
//! - **What a puck hears.** WATCH, held until its circle's view changes (shown on the glass or not, in focus or not, its radius in pixels): the compass follows its sensor only while it is shown, the GPS puck claims GPS only while it is in focus. A doorbell, never a poll.
//!
//! Both threads (the puck receiver and the render thread) touch one table under one spin lock, held for copies and a reply at most.

use super::*;
use core::sync::atomic::AtomicBool;
use ferros_puck::{call, draw::Ctx, draw::Geom, draw::Target, Colour, LIST_MAX};

/// The scene icon of a deck's first puck; its j-th is `PUCK0 + j` (the clock is G#80, the scope G#82).
pub const PUCK0: u8 = 0x90;
/// How many pucks the deck can hold (and how many lists are kept).
pub const N: usize = 1 << 4;
/// The `puck` endpoint (init's slot 9, the old heap region's: kernel service.rs `home_grants`).
pub const S_PUCK: u64 = 9;

/// Whether a scene icon is a puck's.
pub fn is_puck(icon: u8) -> bool {
    (PUCK0..PUCK0 + N as u8).contains(&icon)
}

struct Entry {
    /// Its kist identity (developer key, option); zeros: free.
    id: [u8; 64],
    /// Its lists: at rest and in focus, and their lengths (0: none sent).
    list: [[u8; LIST_MAX]; 2],
    len: [usize; 2],
    /// Counts every new list: the sprite's key.
    seq: u32,
    /// Counts each time its time-anchored ops moved (`due`): the sprite's key.
    tick: u32,
    /// When its time-anchored ops next move, Eagle oscillations (None: nothing moves).
    next: Option<i64>,
    /// Its view as the last scene left it: shown, focused, radius in pixels; and how many times it changed.
    view: (bool, bool, u32),
    view_n: u32,
    /// A watch held: its reply token and the count its caller had seen.
    held: Option<([u8; 32], u32)>,
}

const FREE: Entry = Entry { id: [0; 64], list: [[0; LIST_MAX]; 2], len: [0; 2], seq: 0, tick: 0, next: None, view: (false, false, 0), view_n: 0, held: None };
static mut ENTRIES: [Entry; N] = [FREE; N];
/// The deck's binding: icon `PUCK0 + j` is the puck with identity `BOUND[j]` (zeros: none).
static mut BOUND: [[u8; 64]; N] = [[0; 64]; N];
/// What the scene being drawn has shown so far: per bound puck j, (shown, focused, radius px).
static mut SEEN: [(bool, bool, u32); N] = [(false, false, 0); N];
static LOCK: AtomicBool = AtomicBool::new(false);
/// A new list or binding since the render thread last looked: the frame is drawn again.
static DIRTY: AtomicBool = AtomicBool::new(false);

fn lock() -> &'static mut [Entry; N] {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    unsafe { &mut *(&raw mut ENTRIES) }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// The entry for identity `id`, made from a free one (or the oldest unbound one) when it has none.
fn entry<'a>(es: &'a mut [Entry; N], id: &[u8; 64]) -> Option<&'a mut Entry> {
    let bound = unsafe { &*(&raw const BOUND) };
    // Its own; else a free one; else one no circle shows (a puck unstowed since). Every entry bound to another identity: none, and the caller hears it is not on the deck.
    let k = es.iter().position(|e| &e.id == id).or_else(|| es.iter().position(|e| e.id == [0; 64])).or_else(|| es.iter().position(|e| !bound.contains(&e.id)))?;
    let e = &mut es[k];
    if &e.id != id {
        if let Some((tok, _)) = e.held.take() {
            ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
        }
        *e = FREE;
        e.id = *id;
    }
    Some(e)
}

/// Answer a watch with the view now.
fn answer(e: &Entry, tok: &[u8; 32]) {
    let mut out = [0u8; 11];
    out[1..5].copy_from_slice(&e.view_n.to_le_bytes());
    out[5] = e.view.0 as u8;
    out[6] = e.view.1 as u8;
    out[7..11].copy_from_slice(&e.view.2.to_le_bytes());
    ferros_sys::reply(tok, &out);
}

/// Op 73 from the deck (the system's alone): `[count u8][count × (developer 32, option 32)]`, its pucks in order, the j-th drawn for icon `PUCK0 + j`. Answers the status.
pub fn bind(m: &[u8]) -> u8 {
    let Some(&n) = m.first() else { return 1 };
    let n = n as usize;
    if n > N || m.len() != 1 + 64 * n {
        return 1;
    }
    let es = lock();
    let bound = unsafe { &mut *(&raw mut BOUND) };
    for (j, b) in bound.iter_mut().enumerate() {
        *b = if j < n { m[1 + 64 * j..65 + 64 * j].try_into().unwrap() } else { [0; 64] };
    }
    let _ = es;
    unlock();
    DIRTY.store(true, Ordering::Release);
    ring_bell();
    0
}

static mut PUCK_STACK: [u8; 1 << 15] = [0; 1 << 15];

/// The puck receiver: one call at a time from the `puck` endpoint, each behind its caller's identity.
extern "C" fn receive(grants: u64) -> ! {
    let ep = cap(grants, S_PUCK);
    let mut msg = [0u8; 64 + 6 + LIST_MAX];
    let mut tok = [0u8; 32];
    let mut served = false;
    loop {
        let n = ferros_sys::recv(ep, &mut msg, &mut tok, false);
        // A kernel from before 2026-11-17 hands slot 9 as a memory region, which no recv can serve: said once, and this thread sleeps for good rather than spin on the refusal.
        if n == ferros_sys::ERR && !served {
            console(grants, b"display: slot 9 is not the pucks' endpoint (a kernel from before pucks); no pucks this boot\n");
            loop {
                ferros_sys::sleep_until(u64::MAX);
            }
        }
        if n == ferros_sys::ERR || n > msg.len() as u64 {
            continue;
        }
        served = true;
        let n = n as usize;
        if n < 65 {
            ferros_sys::reply(&tok, &[call::REFUSED]);
            continue;
        }
        let id: [u8; 64] = msg[..64].try_into().unwrap();
        // A caller that is no kist carries zeros: it has no circle and nothing is kept for it.
        if id == [0; 64] {
            ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
            continue;
        }
        let m = &msg[64..n];
        match m[0] {
            call::LIST if m.len() >= 6 && m[1] <= call::FOCUS => {
                let list = &m[6..];
                // An empty focus list means the resting one; any other list is checked whole before it replaces the last good one.
                if !(m[1] == call::FOCUS && list.is_empty()) && ferros_puck::check(list).is_err() {
                    ferros_sys::reply(&tok, &[call::REFUSED]);
                    continue;
                }
                let es = lock();
                let Some(e) = entry(es, &id) else {
                    unlock();
                    ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
                    continue;
                };
                let v = m[1] as usize;
                e.list[v][..list.len()].copy_from_slice(list);
                e.len[v] = list.len();
                e.seq = e.seq.wrapping_add(1);
                let on = unsafe { &*(&raw const BOUND) }.contains(&id);
                unlock();
                DIRTY.store(true, Ordering::Release);
                ring_bell();
                ferros_sys::reply(&tok, &[if on { call::OK } else { call::NOT_ON_DECK }]);
            }
            call::WATCH if m.len() >= 5 => {
                let seen = u32::from_le_bytes(m[1..5].try_into().unwrap());
                let es = lock();
                let Some(e) = entry(es, &id) else {
                    unlock();
                    ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
                    continue;
                };
                if let Some((old, _)) = e.held.take() {
                    answer(e, &old);
                }
                if seen != e.view_n {
                    answer(e, &tok);
                } else {
                    e.held = Some((tok, seen));
                }
                unlock();
            }
            _ => {
                ferros_sys::reply(&tok, &[call::REFUSED]);
            }
        }
    }
}

/// Start the receiver (display_main, once).
pub fn start(grants: u64) {
    if ferros_sys::thread_spawn(receive, unsafe { &mut *(&raw mut PUCK_STACK) }, grants) != 0 {
        console(grants, b"display: the puck receiver did not start; pucks keep their last lists\n");
    }
}

/// Eagle oscillations now, from the kernel's clock (0 until it has read the PMIC's).
pub fn eagle_now() -> i64 {
    let Some((secs, ticks, hz)) = ferros_sys::utc_time() else { return 0 };
    let osc = ferros_puck::OSC as i128;
    ((secs as i128 - ferros_puck::EAGLE_EPOCH_UNIX_SECS as i128) * osc + ticks as i128 * osc / hz as i128) as i64
}

/// A scene is about to be drawn whole: what it shows is gathered afresh.
pub fn scene_start() {
    unsafe { SEEN = [(false, false, 0); N] };
}

/// Puck j's circle is on the glass this scene, at radius `r_px`.
pub fn seen(j: usize, r_px: i64) {
    let focused = unsafe { FOCUS } == PUCK0 + j as u8;
    // WHY the cast is sound: a radius on the glass is under 2^31 pixels at any zoom the deck allows, and a negative one is never drawn (the scene skips circles under a pixel).
    unsafe { SEEN[j] = (true, focused, r_px as u32) };
}

/// The scene is drawn: each bound puck's view compared with what it heard last, and its held watch answered on a change.
pub fn scene_end() {
    let seen = unsafe { SEEN };
    let es = lock();
    let bound = unsafe { &*(&raw const BOUND) };
    for (j, b) in bound.iter().enumerate().filter(|(_, b)| **b != [0; 64]) {
        let Some(e) = es.iter_mut().find(|e| &e.id == b) else { continue };
        // The radius is heard by powers of two only: a pinch changes it every frame, and a puck needs to know its size class, not each pixel of a flight.
        let class = |v: (bool, bool, u32)| (v.0, v.1, if v.2 == 0 { 0 } else { 1u32 << (31 - v.2.leading_zeros()) });
        if class(e.view) != class(seen[j]) {
            e.view = seen[j];
            e.view_n = e.view_n.wrapping_add(1);
            if let Some((tok, _)) = e.held.take() {
                answer(e, &tok);
            }
        }
    }
    unlock();
}

/// The focus moved to or from a puck (op 25): its other face is due.
pub fn changed() {
    DIRTY.store(true, Ordering::Release);
}

/// Whether a puck's picture changed since the last frame: a new list or binding, or a shown puck's time-anchored ops moved (each bumps its tick). The render thread draws the frame again when so.
pub fn due() -> bool {
    let mut any = DIRTY.swap(false, Ordering::AcqRel);
    let now = eagle_now();
    let es = lock();
    for e in es.iter_mut().filter(|e| e.view.0) {
        if e.next.is_some_and(|t| t <= now) {
            e.next = None;
            e.tick = e.tick.wrapping_add(1);
            any = true;
        }
    }
    unlock();
    any
}

/// Puck j's sprite key: what it was drawn from (its list, the moves of its anchored ops, the person's base and zone, whether in focus); a change draws it afresh.
pub fn key(j: usize) -> i64 {
    let focused = unsafe { FOCUS } == PUCK0 + j as u8;
    let id = unsafe { &*(&raw const BOUND) }[j];
    let es = lock();
    let k = es.iter().find(|e| e.id == id && id != [0; 64]).map_or(0, |e| (e.seq as i64) << 24 ^ (e.tick as i64) << 44);
    unlock();
    k ^ (ferros_sys::device_base() as i64) << 8 ^ ferros_sys::zone() << 2 ^ focused as i64
}

/// The display's pixels as ferros_puck draws into them: each covered pixel put under what is there.
struct Pixels<'a> {
    px: &'a mut [Px],
    w: usize,
    h: usize,
    clip: (i32, i32, i32, i32),
    th: &'a ferros_sys::theme::Theme,
    /// The last colour baked, and its opaque pixel.
    last: (Option<Colour>, Px),
}

impl Pixels<'_> {
    fn rgb(&self, c: Colour) -> (u32, u8) {
        match c {
            Colour::Rgb { r, g, b, a } => ((r as u32) << 16 | (g as u32) << 8 | b as u32, a),
            Colour::Theme { slot, a } => {
                let t = self.th;
                let v = match slot {
                    ferros_puck::theme::TEXT => t.text,
                    ferros_puck::theme::TEXT2 => t.text2,
                    ferros_puck::theme::PLANE => t.plane,
                    ferros_puck::theme::RING => t.ring,
                    _ => t.lit,
                };
                (v & 0x00FF_FFFF, a)
            }
        }
    }
}

impl Target for Pixels<'_> {
    fn clip(&self) -> (i32, i32, i32, i32) {
        self.clip
    }
    fn under(&mut self, x: i32, y: i32, cov: f32, c: Colour) {
        let i = y as usize * self.w + x as usize;
        if self.px[i] >> 48 == 0xFFFF {
            return;
        }
        // The baked colour once per colour, not per pixel: an op covers many pixels in one colour.
        if self.last.0 != Some(c) {
            self.last = (Some(c), draw::opaque(self.rgb(c).0));
        }
        let solid = self.last.1;
        let p = &mut self.px[i];
        // The colour's own opacity is already in `cov` (ferros_puck's `shade`); coverage in [0, 1] to Q16.
        *p = draw::under(*p, draw::cover(solid, (cov * 65535.) as u32));
    }
    fn text(&mut self, cx: f32, cy: f32, px: f32, turn: (f32, f32), c: Colour, s: &str) {
        let (rgb, a) = self.rgb(c);
        text_turned_centre(self.px, self.w, self.h, s, cx, cy, px, turn, (a as u32) << 24 | rgb);
        ALLOC.reset();
    }
}

/// Draw puck j's list (its focus list while in focus, when it sent one) into `px` (w × h, clipped to `clip`), the circle at (cx, cy) radius r in Q8 pixels, turned (tc, ts) Q14; its next move kept for `due`. Nothing when it has no list yet: the circle shows only its glow.
#[allow(clippy::too_many_arguments)]
pub fn draw_into(px: &mut [Px], w: usize, h: usize, clip: draw::Rect, j: usize, cx: i64, cy: i64, r: i64, tc: i64, ts: i64) {
    let mut list = [0u8; LIST_MAX];
    let focused = unsafe { FOCUS } == PUCK0 + j as u8;
    let id = unsafe { &*(&raw const BOUND) }[j];
    let n = {
        let es = lock();
        let n = match es.iter().find(|e| e.id == id && id != [0; 64]) {
            Some(e) => {
                let v = if focused && e.len[1] > 0 { 1 } else { 0 };
                list[..e.len[v]].copy_from_slice(&e.list[v][..e.len[v]]);
                e.len[v]
            }
            None => 0,
        };
        unlock();
        n
    };
    if n == 0 {
        return;
    }
    let g = Geom { cx: cx as f32 / 256., cy: cy as f32 / 256., r: r as f32 / 256., cos: tc as f32 / 16384., sin: ts as f32 / 16384. };
    let now = eagle_now();
    let ctx = Ctx { now, zone: ferros_sys::zone(), base: ferros_sys::device_base() };
    let th = ferros_sys::theme::current();
    let mut t = Pixels { px, w, h, clip: (clip.x0, clip.y0, clip.x1, clip.y1), th, last: (None, 0) };
    ferros_puck::draw::draw(&list[..n], &g, &ctx, &mut t);
    let next = ferros_puck::draw::next_change(&list[..n], g.r, now);
    let es = lock();
    if let Some(e) = es.iter_mut().find(|e| e.id == id) {
        e.next = next;
    }
    unlock();
}
