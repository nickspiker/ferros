//! Pucks on the glass (docs/PUCKS.md; owner, 2026-11-18: "supplying a diameter and having the kist return a square vector in sixteen bit that ferros will trim and place"): each puck draws its own face into a square of G2t16 pixels in its slot of the pucks' window, and this server trims the square to the puck's circle and places it, turned and scaled to the circle as it is now.
//!
//! - **Who sends.** A puck calls the `puck` endpoint (grant slot 9), and the kernel puts its kist identity (developer key and option) and its square (slot, generation, bytes held) ahead of every call (process.rs `puck_stamp`): a square is kept under that identity, and read only where and as far as the kernel says the puck holds one.
//! - **Which circle.** The deck (the map) binds each puck's identity to a scene icon, `PUCK0 + j` for its j-th puck (op 73, the system's alone).
//! - **The view.** Each scene, every bound puck's circle as it shows: on the glass or not, large (at least half the panel across), its diameter, its turn (gravity's, else the camera's and the circle's own). A puck's held WATCH is answered when the view moves (a size by a pixel, a turn by half a pixel at the rim), when a tap passes thru to it (op 74 from the deck), and, for a puck that asked (`call::FRAMES`), at every flip the deck shows. A doorbell, never a poll.
//! - **The square.** SHOW names a square in the puck's slot; the render thread takes it at the top of its next pass (`due`) and answers then, so from that answer on the puck draws only elsewhere, and this server never reads a square while its puck writes it.
//! - **Drawing.** The circle's sprite is made from the square when the square, the circle's size or its turn changes: each pixel of the circle sampled from the square (bilinear, the turn between the square's and the circle's undone), trimmed by the circle's C¹ edge, baked (bake.rs `g2t_row`). A circle shown larger than the square or turned since is drawn from it scaled and turned until the puck's next square lands.
//!
//! Both threads (the puck receiver and the render thread) touch one table under one spin lock, held for copies and a reply at most.

use super::*;
use core::sync::atomic::AtomicBool;
use ferros_puck::call;

/// The scene icon of a deck's first puck; its j-th is `PUCK0 + j`.
pub const PUCK0: u8 = 0x90;
/// How many pucks the deck can hold (and how many squares are kept).
pub const N: usize = 1 << 4;
/// The `puck` endpoint (init's slot 9, the old heap region's: kernel service.rs `home_grants`).
pub const S_PUCK: u64 = 9;
/// The kernel's stamp ahead of every call: identity 64, then the square: slot, generation, two zeros, bytes held u32.
const STAMP: usize = 72;

/// Whether a scene icon is a puck's.
pub fn is_puck(icon: u8) -> bool {
    (PUCK0..PUCK0 + N as u8).contains(&icon)
}

/// A square handed over: where in the puck's slot (slot, generation), how far past its porch, its side and the turn it was drawn at (Q14).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Square {
    slot: u8,
    generation: u8,
    off: u32,
    d: u16,
    c: i16,
    s: i16,
}

/// A circle's view: on the glass, large, diameter in pixels, turn (Q14).
#[derive(Clone, Copy, PartialEq, Eq)]
struct View {
    shown: bool,
    large: bool,
    d: u16,
    c: i16,
    s: i16,
}
const HIDDEN: View = View { shown: false, large: false, d: 0, c: 1 << 14, s: 0 };

struct Entry {
    /// Its kist identity (developer key, option); zeros: free.
    id: [u8; 64],
    /// The square this server reads, and one handed over that the render thread has not taken yet (its reply token).
    cur: Option<Square>,
    pending: Option<(Square, [u8; 32])>,
    /// Counts every square taken: the sprite's key.
    seq: u32,
    /// Its view as the last scene left it, and how many times it changed (or a tap or a frame came).
    view: View,
    view_n: u32,
    /// Asked for every shown frame (`call::FRAMES`).
    frames: bool,
    /// Taps not yet heard: (x, y) Q14 of the radius in the face's frame.
    taps: [(i16, i16); call::TAPS],
    ntaps: usize,
    /// A watch held: its reply token.
    held: Option<[u8; 32]>,
}

const FREE: Entry = Entry { id: [0; 64], cur: None, pending: None, seq: 0, view: HIDDEN, view_n: 0, frames: false, taps: [(0, 0); call::TAPS], ntaps: 0, held: None };
static mut ENTRIES: [Entry; N] = [FREE; N];
/// The deck's binding: icon `PUCK0 + j` is the puck with identity `BOUND[j]` (zeros: none).
static mut BOUND: [[u8; 64]; N] = [[0; 64]; N];
/// What the scene being drawn has shown so far, per bound puck j: its view, and where (centre x, y and radius Q8, turn Q14) for the deck's taps.
static mut SEEN: [View; N] = [HIDDEN; N];
static mut GEOM: [(i64, i64, i64, i64, i64); N] = [(0, 0, 0, 1 << 14, 0); N];
/// Each scene circle's puck, by scene index (G#FF none), for op 74.
static mut BY_SCENE: [u8; 256] = [0xFF; 256];
static LOCK: AtomicBool = AtomicBool::new(false);
/// A new square or binding since the render thread last looked: the frame is drawn again.
static DIRTY: AtomicBool = AtomicBool::new(false);
/// The squares' owners, from the kernel (SYS_BELL `BELL_PUCKS`): bits 4j to 4j + 3 slot j's generation while its puck lives.
static LIVE: AtomicU64 = AtomicU64::new(0);
static LIVE_BELL: AtomicBool = AtomicBool::new(false);

fn lock() -> &'static mut [Entry; N] {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    unsafe { &mut *(&raw mut ENTRIES) }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// Whether slot `slot` still holds generation `g` for a puck that lives: always, on a kernel without the squares' bell (none could have handed a square then).
fn live(sq: &Square) -> bool {
    !LIVE_BELL.load(Ordering::Acquire) || ferros_sys::surface_generation(LIVE.load(Ordering::Acquire), sq.slot as usize) == sq.generation
}

/// The entry for identity `id`, made from a free one (or one no circle shows) when it has none.
fn entry<'a>(es: &'a mut [Entry; N], id: &[u8; 64]) -> Option<&'a mut Entry> {
    let bound = unsafe { &*(&raw const BOUND) };
    let k = es.iter().position(|e| &e.id == id).or_else(|| es.iter().position(|e| e.id == [0; 64])).or_else(|| es.iter().position(|e| !bound.contains(&e.id)))?;
    let e = &mut es[k];
    if &e.id != id {
        if let Some(tok) = e.held.take() {
            ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
        }
        if let Some((_, tok)) = e.pending.take() {
            ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
        }
        *e = FREE;
        e.id = *id;
    }
    Some(e)
}

/// Answer a watch with the view now and the taps not yet heard (which go with it).
fn answer(e: &mut Entry, tok: &[u8; 32]) {
    let mut out = [0u8; 14 + 4 * call::TAPS];
    out[1..5].copy_from_slice(&e.view_n.to_le_bytes());
    out[5] = e.view.shown as u8;
    out[6] = e.view.large as u8;
    out[7..9].copy_from_slice(&e.view.d.to_le_bytes());
    out[9..11].copy_from_slice(&e.view.c.to_le_bytes());
    out[11..13].copy_from_slice(&e.view.s.to_le_bytes());
    out[13] = e.ntaps as u8;
    for (k, &(x, y)) in e.taps[..e.ntaps].iter().enumerate() {
        out[14 + 4 * k..16 + 4 * k].copy_from_slice(&x.to_le_bytes());
        out[16 + 4 * k..18 + 4 * k].copy_from_slice(&y.to_le_bytes());
    }
    let n = 14 + 4 * e.ntaps;
    e.ntaps = 0;
    ferros_sys::reply(tok, &out[..n]);
}

/// Something the puck should hear: its count moves and a held watch is answered.
fn tell(e: &mut Entry) {
    e.view_n = e.view_n.wrapping_add(1);
    if let Some(tok) = e.held.take() {
        answer(e, &tok);
    }
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

/// Op 74 from the deck (the system's alone): `[scene index u8][x u16][y u16]`, a tap at that screen pixel on that circle, passed thru to its puck in the face's own frame, Q14 of the radius from the centre. Answers 0, or 1 when the circle is no puck's.
pub fn tap(m: &[u8]) -> u8 {
    if m.len() < 5 {
        return 1;
    }
    let i = m[0] as usize;
    let j = unsafe { BY_SCENE[i] } as usize;
    if j >= N {
        return 1;
    }
    let (x, y) = (u16::from_le_bytes([m[1], m[2]]) as i64, u16::from_le_bytes([m[3], m[4]]) as i64);
    let (sx, sy, sr, fc, fs) = unsafe { GEOM[j] };
    // From the circle's centre in Q8 pixels, into the face's frame (its turn undone), then Q14 of the radius.
    let (dx, dy) = ((x << 8) + 128 - sx, (y << 8) + 128 - sy);
    let (fx, fy) = ((dx * fc + dy * fs) >> 14, (dy * fc - dx * fs) >> 14);
    // WHY the floor: a circle under a pixel is never drawn, so never tapped, but its radius is the divisor.
    let r = sr.max(256);
    let q = |v: i64| ((v << 14) / r) as i16;
    let id = unsafe { (*(&raw const BOUND))[j] };
    let es = lock();
    if let Some(e) = es.iter_mut().find(|e| e.id == id && id != [0; 64]) {
        // The oldest go when the puck has not heard the last `TAPS`.
        if e.ntaps == call::TAPS {
            e.taps.copy_within(1.., 0);
            e.ntaps -= 1;
        }
        e.taps[e.ntaps] = (q(fx), q(fy));
        e.ntaps += 1;
        tell(e);
    }
    unlock();
    0
}

static mut PUCK_STACK: [u8; 1 << 15] = [0; 1 << 15];

/// The puck receiver: one call at a time from the `puck` endpoint, each behind its caller's identity and square.
extern "C" fn receive(grants: u64) -> ! {
    let ep = cap(grants, S_PUCK);
    let mut msg = [0u8; STAMP + 64];
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
        if n < STAMP + 1 {
            ferros_sys::reply(&tok, &[call::REFUSED]);
            continue;
        }
        let id: [u8; 64] = msg[..64].try_into().unwrap();
        // A caller that is no kist carries zeros: it has no circle and nothing is kept for it.
        if id == [0; 64] {
            ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
            continue;
        }
        let (slot, generation, held) = (msg[64], msg[65], u32::from_le_bytes(msg[68..72].try_into().unwrap()) as u64);
        let m = &msg[STAMP..n];
        match m[0] {
            call::WATCH if m.len() >= 5 => {
                let seen = u32::from_le_bytes(m[1..5].try_into().unwrap());
                let frames = m.get(5).is_some_and(|w| w & call::FRAMES != 0);
                let es = lock();
                let Some(e) = entry(es, &id) else {
                    unlock();
                    ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
                    continue;
                };
                e.frames = frames;
                if let Some(old) = e.held.take() {
                    answer(e, &old);
                }
                if seen != e.view_n || e.ntaps > 0 {
                    answer(e, &tok);
                } else {
                    e.held = Some(tok);
                }
                unlock();
            }
            call::SHOW if m.len() >= 11 => {
                let sq = Square {
                    slot,
                    generation,
                    off: u32::from_le_bytes(m[1..5].try_into().unwrap()),
                    d: u16::from_le_bytes([m[5], m[6]]),
                    c: i16::from_le_bytes([m[7], m[8]]),
                    s: i16::from_le_bytes([m[9], m[10]]),
                };
                // The square must lie inside what the kernel says the puck holds (the stamp), whole pixels, a side within the slot's two: anything else is never read.
                let bytes = sq.d as u64 * sq.d as u64 * 8;
                if slot as usize >= ferros_sys::PUCK_SLOTS || sq.off % 8 != 0 || sq.d == 0 || sq.d > ferros_puck::SIDE_MAX || sq.off as u64 + bytes > held {
                    ferros_sys::reply(&tok, &[call::REFUSED]);
                    continue;
                }
                let es = lock();
                let Some(e) = entry(es, &id) else {
                    unlock();
                    ferros_sys::reply(&tok, &[call::NOT_ON_DECK]);
                    continue;
                };
                // A call waits for its answer, so a puck has at most one square handed over at a time; an earlier one still pending was never taken (the render thread was away): answered now, the newer replaces it.
                if let Some((_, old)) = e.pending.take() {
                    ferros_sys::reply(&old, &[call::OK]);
                }
                e.pending = Some((sq, tok));
                unlock();
                // The render thread takes it at its next pass (`due`); drawn into the phases at the flips, not a whole frame.
                ring_bell();
            }
            _ => {
                ferros_sys::reply(&tok, &[call::REFUSED]);
            }
        }
    }
}

/// Start the receiver and the squares' bell (display_main, once).
pub fn start(grants: u64) {
    LIVE_BELL.store(ferros_sys::bell(ferros_sys::BELL_PUCKS, &LIVE), Ordering::Release);
    if ferros_sys::thread_spawn(receive, unsafe { &mut *(&raw mut PUCK_STACK) }, grants) != 0 {
        console(grants, b"display: the puck receiver did not start; pucks keep their last squares\n");
    }
}

/// A scene is about to be drawn whole: what it shows is gathered afresh.
pub fn scene_start() {
    unsafe {
        SEEN = [HIDDEN; N];
        BY_SCENE = [0xFF; 256];
    }
}

/// Puck j's circle is on the glass this scene as scene circle `i`, centred at (sx, sy) radius sr (Q8 pixels), its face turned (fc, fs) Q14; the panel `width` pixels across.
pub fn seen(j: usize, i: usize, sx: i64, sy: i64, sr: i64, fc: i64, fs: i64, width: usize) {
    // WHY the bound: a circle zoomed past the slot's room is drawn from the largest square the slot holds, scaled.
    let d = ((2 * sr) >> 8).min(ferros_puck::SIDE_MAX as i64) as u16;
    unsafe {
        SEEN[j] = View { shown: true, large: 2 * d as usize >= width, d, c: fc as i16, s: fs as i16 };
        GEOM[j] = (sx, sy, sr, fc, fs);
        BY_SCENE[i] = j as u8;
    }
}

/// The scene is drawn: each bound puck's view compared with what it heard last, and told on a change: shown or hidden, large or not, a pixel of diameter, or a turn of half a pixel at its rim.
pub fn scene_end() {
    let seen = unsafe { SEEN };
    let es = lock();
    let bound = unsafe { &*(&raw const BOUND) };
    for (j, b) in bound.iter().enumerate().filter(|(_, b)| **b != [0; 64]) {
        let Some(e) = es.iter_mut().find(|e| &e.id == b) else { continue };
        let (a, n) = (e.view, seen[j]);
        // sin of the angle between the two turns, Q28, times the radius in pixels: past half a pixel at the rim (2^27 · d, in the same units) the face would draw differently.
        let sin = (a.c as i64 * n.s as i64 - a.s as i64 * n.c as i64).abs();
        let turned = sin * n.d as i64 >= 1 << 28;
        if a.shown != n.shown || a.large != n.large || a.d != n.d || turned {
            e.view = n;
            tell(e);
            count(2);
        }
    }
    unlock();
}

/// The deck showed a frame (a flip): every puck on the glass that asked for frames hears it.
pub fn frame() {
    let es = lock();
    for e in es.iter_mut().filter(|e| e.frames && e.view.shown && e.held.is_some()) {
        tell(e);
        count(1);
    }
    unlock();
}

/// The top of a render pass: every square handed over is taken (from here on this server reads it, and its puck may draw elsewhere: answered), and a square whose puck has ended is dropped. Whether the deck must be drawn again whole (a binding, a puck gone, or `whole`: a new square too, for a display with no flips to carry it); a new square alone is otherwise drawn into each phase at its flip (`phase_due`), the puck's box alone.
pub fn due(whole: bool) -> bool {
    let mut any = DIRTY.swap(false, Ordering::AcqRel);
    let es = lock();
    for e in es.iter_mut() {
        if let Some((sq, tok)) = e.pending.take() {
            e.cur = Some(sq);
            e.seq = e.seq.wrapping_add(1);
            ferros_sys::reply(&tok, &[call::OK]);
            count(0);
            any |= whole;
        }
        if e.cur.is_some_and(|sq| !live(&sq)) {
            e.cur = None;
            e.seq = e.seq.wrapping_add(1);
            any = true;
        }
    }
    unlock();
    any
}

/// Puck j's sprite key: the square it was drawn from (each taken square counts).
pub fn key(j: usize) -> i64 {
    let id = unsafe { &*(&raw const BOUND) }[j];
    let es = lock();
    let k = es.iter().find(|e| e.id == id && id != [0; 64]).map_or(0, |e| e.seq as i64 + 1);
    unlock();
    k
}

/// The largest radius of a puck on the glass, Q8: how finely the gravity turn is worth following.
pub fn largest_r() -> i64 {
    unsafe { (*(&raw const GEOM)).iter().zip((*(&raw const SEEN)).iter()).filter(|(_, v)| v.shown).map(|(g, _)| g.2).max().unwrap_or(0) }
}

/// `draw_into`'s rows: a circle's box is at most the panel's width plus its edge, inside the clip; the widest panel ferros drives is under 2^11.
const ROW_MAX: usize = 1 << 11;
/// The threads that draw the scene's list (the publish workers and the render thread, `draw::thread_slot`): each its own rows, since a puck's square is drawn by bands across them (`Item::Puck`, 2026-11-20).
const THREADS: usize = super::PUB_WORKERS + 1;
static mut ROW: [[u64; ROW_MAX]; THREADS] = [[0; ROW_MAX]; THREADS];
static mut BAKED: [[u64; ROW_MAX]; THREADS] = [[0; ROW_MAX]; THREADS];
static mut COV: [[f32; ROW_MAX]; THREADS] = [[0.; ROW_MAX]; THREADS];

/// The square of each puck the canvas holds whole (its `seq`), at the geometry of the last scene; None: not known (never drawn whole since the last whole frame, or drawn in part from a newer square).
/// A phase that lacks a puck's newest square while the canvas holds it is only published from the canvas, never drawn again (2026-11-20, the Pro: a new square was sampled once for each of the four phases, 16 ms a box).
/// Only the render thread touches it, between draws.
static mut CANVAS: [Option<u32>; N] = [None; N];

/// Puck j's newest square's `seq`, while it has one that lives.
fn seq_of(j: usize) -> Option<u32> {
    let id = unsafe { &*(&raw const BOUND) }[j];
    let es = lock();
    let s = es.iter().find(|e| e.id == id && id != [0; 64]).and_then(|e| e.cur.filter(live).map(|_| e.seq));
    unlock();
    s
}

/// Whether puck j has a square to draw (else its circle shows its name on the plane).
pub fn has_square(j: usize) -> bool {
    seq_of(j).is_some()
}

/// A whole frame is drawn into the canvas: what it held of each puck is forgotten, and the frame's own draws say it again (`canvas_drew`).
pub fn canvas_forget() {
    unsafe { CANVAS = [None; N] };
}

/// Puck j was drawn into the canvas from its newest square: its whole box (`whole`), or a part of it (a clip that cut it), which leaves the canvas mixed unless it held that square already.
pub fn canvas_drew(j: usize, whole: bool) {
    let s = seq_of(j);
    unsafe {
        if whole {
            CANVAS[j] = s;
        } else if CANVAS[j] != s {
            CANVAS[j] = None;
        }
    }
}

/// Whether the canvas holds puck j's newest square whole: a phase that lacks it needs only the box published.
pub fn canvas_holds(j: usize) -> bool {
    let s = seq_of(j);
    s.is_some() && unsafe { CANVAS[j] } == s
}

/// Draw puck j's square into `px` (w × h, inside `clip`): the circle at (cx, cy) radius r (Q8 pixels), its face turned (fc, fs) Q14, each pixel sampled from the square (bilinear, the turn between the square's and the face's undone, its size to the circle's), trimmed by the circle's edge and baked, under what is there. False when the puck has handed no square yet (the caller shows its plane).
#[allow(clippy::too_many_arguments)]
pub fn draw_into(px: &mut [Px], w: usize, clip: draw::Rect, j: usize, cx: i64, cy: i64, r: i64, fc: i64, fs: i64) -> bool {
    let id = unsafe { &*(&raw const BOUND) }[j];
    let sq = {
        let es = lock();
        let sq = es.iter().find(|e| e.id == id && id != [0; 64]).and_then(|e| e.cur);
        unlock();
        sq
    };
    let Some(sq) = sq.filter(live) else { return false };
    let d = sq.d as usize;
    // SAFETY: the pucks' window maps every block of a held slot (a frame of its puck's, or the blank frame), and the square was checked against the bytes the kernel says the puck holds when it was handed over; a puck that ended or shrank since leaves the blank frame there, never a fault.
    let src = unsafe { core::slice::from_raw_parts((ferros_sys::PUCKS_WINDOW as usize + sq.slot as usize * ferros_sys::PUCK_BYTES + 64 + sq.off as usize) as *const u64, d * d) };
    let (rf, cxf, cyf) = (r as f32 / 256., cx as f32 / 256., cy as f32 / 256.);
    // The turn from the face to the square: the face's turn undone, the square's applied.
    let (fc, fs) = (fc as f32 / 16384., fs as f32 / 16384.);
    let (sc, ss) = (sq.c as f32 / 16384., sq.s as f32 / 16384.);
    let (kc, ks) = (sc * fc + ss * fs, ss * fc - sc * fs);
    // Square pixels per screen pixel.
    let k = d as f32 / (2. * rf);
    let half = d as f32 * 0.5;
    // The circle's box, a pixel wider for its edge, inside the clip (the screen, or the sprite, or the region being redrawn): pixels outside it are not this draw's.
    let x0 = (((cx - r) >> 8) as i32 - 1).max(clip.x0);
    let x1 = (((cx + r) >> 8) as i32 + 2).min(clip.x1);
    let y0 = (((cy - r) >> 8) as i32 - 1).max(clip.y0);
    let y1 = (((cy + r) >> 8) as i32 + 2).min(clip.y1);
    if x0 >= x1 || y0 >= y1 {
        return true;
    }
    let n = (x1 - x0) as usize;
    // The rows in fixed buffers, nothing on the heap: this process's heap is a bump that only the text draws reset, so a deck with no words on it (no labels) never gave back three rows a puck redraw, and the display ran out of memory 16 minutes into the 8a's first pucks (2026-11-18, "memory allocation of 5272 bytes failed").
    // Each drawing thread its own (`THREADS`): the scene's list draws a puck by bands across the publish workers and the render thread.
    let t = draw::thread_slot();
    // SAFETY: row set t is this thread's alone (`draw::thread_slot` is unique to each drawing thread), so nothing else holds these.
    let (row, baked, cov): (&mut [u64; ROW_MAX], &mut [u64; ROW_MAX], &mut [f32; ROW_MAX]) = unsafe { (&mut (*(&raw mut ROW))[t], &mut (*(&raw mut BAKED))[t], &mut (*(&raw mut COV))[t]) };
    let (row, baked, cov) = (&mut row[..n], &mut baked[..n], &mut cov[..n]);
    // WHY the bounds: a pixel at the circle's rim, turned and scaled into the square, lands up to a pixel past its side (and anywhere past it while the circle is turned or sized since the square was drawn); past the side is nothing.
    let at = |x: i32, y: i32| -> u64 { if x >= 0 && y >= 0 && (x as usize) < d && (y as usize) < d { src[y as usize * d + x as usize] } else { 0 } };
    let bake = crate::bake::now();
    // The square's coordinates step along a row by fixed amounts (the turn and the scale are the row's), Q16: one multiply-add a pixel, as `draw::icon` samples (2026-11-18, the 8a: the float sampler with a square root a pixel took 30 ms for the clock's 606-pixel circle at every box).
    let q16 = |v: f32| (v * 65536.) as i64;
    let (du, dv) = (q16(kc * k), q16(ks * k));
    let (rin, rout) = (rf - 1., rf + 1.);
    for y in y0..y1 {
        let dy = y as f32 + 0.5 - cyf;
        // The row's reach outside the edge and its span wholly inside it, from the circle: two roots a row, not one a pixel.
        let reach2 = rout * rout - dy * dy;
        if reach2 <= 0. {
            continue;
        }
        let reach = libm::sqrtf(reach2);
        let inner2 = rin * rin - dy * dy;
        let inner = if inner2 > 0. { libm::sqrtf(inner2) } else { -1. };
        let xa = (libm::floorf(cxf - reach) as i32).max(x0);
        let xb = (libm::ceilf(cxf + reach) as i32).min(x1);
        if xa >= xb {
            continue;
        }
        let (qa, qb) = ((xa - x0) as usize, (xb - x0) as usize);
        let dx0 = xa as f32 + 0.5 - cxf;
        let mut u = q16((dx0 * kc - dy * ks) * k + half - 0.5);
        let mut v = q16((dx0 * ks + dy * kc) * k + half - 0.5);
        for q in qa..qb {
            let dx = (x0 + q as i32) as f32 + 0.5 - cxf;
            // Wholly inside: coverage 1, no root; near the edge, the C¹ edge of the distance.
            cov[q] = if dx.abs() < inner { 1. } else { ferros_puck::draw::edge(libm::sqrtf(dx * dx + dy * dy) - rf) };
            if cov[q] <= 0. {
                row[q] = 0;
            } else {
                let (iu, iv) = ((u >> 16) as i32, (v >> 16) as i32);
                let (fu, fv) = ((u >> 8 & 0xFF) as u64, (v >> 8 & 0xFF) as u64);
                row[q] = bilerp(at(iu, iv), at(iu + 1, iv), at(iu, iv + 1), at(iu + 1, iv + 1), fu, fv);
            }
            u += du;
            v += dv;
        }
        bake.g2t_row(&row[qa..qb], &mut baked[qa..qb]);
        let base = y as usize * w;
        for q in qa..qb {
            let a = row[q] >> 48;
            if a == 0 || cov[q] <= 0. {
                continue;
            }
            // Baked light with the square's own α (the light of a premultiplied code is premultiplied, as the canvas holds it: the windows' decode, main.rs `layer_row`), then every lane trimmed by the circle's edge where it is partly covered.
            let p = a << 48 | (baked[q] & 0xFFFF_FFFF_FFFF);
            let t = if cov[q] >= 1. {
                p
            } else {
                let e = (cov[q] * 65535.) as u64;
                (0..4).fold(0u64, |v, k| v | ((p >> (16 * k) & 0xFFFF) * e + 0x7FFF) / 0xFFFF << (16 * k))
            };
            let i = base + x0 as usize + q;
            px[i] = draw::under(px[i], t);
        }
    }
    true
}

/// Bilinear between four G2t16 pixels, two lanes per multiply (α and G, then R and B, each pair riding one word's two 32-bit halves); `fu`, `fv` in 256ths (as draw.rs `bilerp` for the canvas).
#[inline(always)]
fn bilerp(p00: u64, p10: u64, p01: u64, p11: u64, fu: u64, fv: u64) -> u64 {
    let (w00, w10, w01, w11) = ((256 - fu) * (256 - fv) >> 8, fu * (256 - fv) >> 8, (256 - fu) * fv >> 8, fu * fv >> 8);
    const M: u64 = 0x0000_FFFF_0000_FFFF;
    let rb = (p00 & M) * w00 + (p10 & M) * w10 + (p01 & M) * w01 + (p11 & M) * w11;
    let ga = (p00 >> 16 & M) * w00 + (p10 >> 16 & M) * w10 + (p01 >> 16 & M) * w01 + (p11 >> 16 & M) * w11;
    (rb >> 8 & M) | (ga >> 8 & M) << 16
}

/// Whether the deck is covered (an app in front, the screen off) as last said: while it is, no puck's circle is on the glass.
static COVERED: AtomicBool = AtomicBool::new(false);

/// The deck covered or shown again: covered, every puck hears its circle is off the glass (it draws nothing until it is back); shown again, the next scene says where each is.
pub fn covered(now: bool) {
    if COVERED.swap(now, Ordering::AcqRel) == now || !now {
        return;
    }
    let es = lock();
    for e in es.iter_mut().filter(|e| e.view.shown) {
        e.view = HIDDEN;
        tell(e);
    }
    unlock();
}

/// What the pucks did since the last read (`counts`): squares taken, frames told, views told, whole redraws they caused, boxes drawn into a phase at its flip. For the display's ten-second line.
static COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// One more of count `k`.
pub fn count(k: usize) {
    COUNTS[k].fetch_add(1, Ordering::Relaxed);
}

/// The counts since the last read, each back to zero.
pub fn counts() -> [u64; 6] {
    core::array::from_fn(|k| COUNTS[k].swap(0, Ordering::Relaxed))
}

/// The square each dither phase holds of each puck (its `seq`, 0 nothing known): what a flip still owes a phase.
static mut PHASE_SEQ: [[u32; N]; 4] = [[0; N]; 4];

/// A circle's box at (sx, sy) radius sr (Q8 pixels), with a margin for its edge, on the screen: what a puck's redraw covers, and what a draw must cover to hold its square whole (`canvas_drew`).
pub fn box_of(sx: i64, sy: i64, sr: i64, width: usize, height: usize) -> draw::Rect {
    draw::Rect { x0: ((sx - sr) >> 8) as i32 - 3, y0: ((sy - sr) >> 8) as i32 - 3, x1: ((sx + sr) >> 8) as i32 + 4, y1: ((sy + sr) >> 8) as i32 + 4 }.clip(width, height)
}

/// The boxes phase `k` lacks before it is shown: each bound puck on the glass whose newest square that phase does not hold yet, by the puck (j) and its circle's box with a margin for its edge (Q8 geometry of the last scene; `box_of`). How many, into `out`.
pub fn phase_due(k: usize, width: usize, height: usize, out: &mut [(usize, draw::Rect); N]) -> usize {
    let bound = unsafe { &*(&raw const BOUND) };
    let seen = unsafe { &*(&raw const SEEN) };
    let geom = unsafe { &*(&raw const GEOM) };
    let es = lock();
    let mut n = 0;
    for j in 0..N {
        if bound[j] == [0; 64] || !seen[j].shown {
            continue;
        }
        let Some(e) = es.iter().find(|e| e.id == bound[j]) else { continue };
        if e.cur.is_some() && unsafe { PHASE_SEQ[k][j] } != e.seq {
            let (sx, sy, sr, _, _) = geom[j];
            out[n] = (j, box_of(sx, sy, sr, width, height));
            n += 1;
        }
    }
    unlock();
    n
}

/// Phase `k` now holds every bound puck's newest square (drawn into it by the flip), or every phase does (`all`: a whole frame was published).
pub fn phase_drew(k: usize, all: bool) {
    let bound = unsafe { &*(&raw const BOUND) };
    let es = lock();
    for j in 0..N {
        let s = es.iter().find(|e| e.id == bound[j] && bound[j] != [0; 64]).map_or(0, |e| e.seq);
        unsafe {
            if all {
                for p in 0..4 {
                    PHASE_SEQ[p][j] = s;
                }
            } else {
                PHASE_SEQ[k][j] = s;
            }
        }
    }
    unlock();
}

/// `us` more of the boxes' drawing time (count 5).
pub fn count_us(us: u64) {
    COUNTS[5].fetch_add(us, Ordering::Relaxed);
}
