//! A puck's side on the device (feature `sys`): its calls to the display, its view's doorbell, its square, Eagle time, a GPS fix when granted, and its place kept as VSF.
//!
//! The shape of every puck: while its circle is on the glass, build its face's list from its engine at now, draw it into its square at the size and turn the display asked for (`show`), hand the square over; then sleep in one wait until the earliest of its content's own next deadline, the next moment its time-anchored ops move half a pixel, and a change of its circle's view (`View`), never on an interval.
//! The view comes from a held WATCH on the display (a thread of its own, `watch`): the display answers it when the circle is shown or hidden, changes size or turns, when a tap passes thru to it or a finger drags on it while it is centred, and, for a puck that asked for them, at every frame the deck shows.
//! A puck whose circle is not on the glass draws nothing.
//! The place: a GPS fix taken only when the person granted GPS (a claim, released at the first fix or after `FIX_GIVE_UP_S`), rounded to the ~10 km cell (ferros_weather's grid) before it is kept, and kept in the puck's own vault domain as a VSF document; the finer fix never leaves the process's memory.

use crate::{call, OSC};
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::{cap, freq, now};

/// Eagle oscillations now, from the kernel's clock; None until it has read the PMIC's.
pub fn eagle_now() -> Option<i64> {
    let (secs, ticks, hz) = ferros_sys::utc_time()?;
    let osc = OSC as i128;
    Some(((secs as i128 - crate::EAGLE_EPOCH_UNIX_SECS as i128) * osc + ticks as i128 * osc / hz as i128) as i64)
}

/// The counter tick at which Eagle time `at` falls (now when it has passed).
pub fn ticks_at(at: i64) -> u64 {
    let Some(t) = eagle_now() else { return now() };
    if at <= t {
        return now();
    }
    let ahead = ((at - t) as i128 * freq() as i128 / OSC as i128) as u64;
    now() + ahead
}

/// The puck's link to the display: the slot of its `puck` right, and the slot of its square (its surface's, `ferros_sys::kist::SURFACE`).
#[derive(Clone, Copy)]
pub struct Link {
    pub grants: u64,
    pub slot: u64,
    pub square: u64,
}

/// The circle's view as the display last said it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub count: u32,
    /// On the glass.
    pub shown: bool,
    /// At least half the panel's width across: room for the fuller face.
    pub large: bool,
    /// Its diameter in pixels, the side of the square to draw.
    pub d: u16,
    /// Its turn, Q14 (cos, sin).
    pub c: i16,
    pub s: i16,
}

/// The view, packed: count in bits 0-31, d in 32-47, shown bit 48, large bit 49; the turn beside it (c in 0-15, s in 16-31).
static VIEW: AtomicU64 = AtomicU64::new(0);
static TURN: AtomicU64 = AtomicU64::new(1 << 14);
/// Rung by anything the main loop should wake for besides the view (a fix, a fetch): its count moves.
static BELL: AtomicU64 = AtomicU64::new(0);
/// What the watch asks for (`call::FRAMES`), set before `watch`.
static WANTS: AtomicU64 = AtomicU64::new(0);

/// The view now.
pub fn view() -> View {
    let v = VIEW.load(Ordering::Acquire);
    let t = TURN.load(Ordering::Acquire);
    View { count: v as u32, d: (v >> 32) as u16, shown: v >> 48 & 1 == 1, large: v >> 49 & 1 == 1, c: t as u16 as i16, s: (t >> 16) as u16 as i16 }
}

/// Taps that passed thru to the circle and are not yet taken: (x, y) in Q14 of the radius, in the face's own frame. A ring the watch thread fills and `taps` empties; the oldest go when it is full.
static TAPS: [AtomicU64; call::TAPS] = [const { AtomicU64::new(0) }; call::TAPS];
static TAPS_IN: AtomicU64 = AtomicU64::new(0);
static TAPS_OUT: AtomicU64 = AtomicU64::new(0);

/// The taps since the last call, oldest first, into `out`; how many.
pub fn taps(out: &mut [(i16, i16); call::TAPS]) -> usize {
    let end = TAPS_IN.load(Ordering::Acquire);
    let mut at = TAPS_OUT.load(Ordering::Acquire);
    // The ring holds the last `TAPS`: anything older was overwritten.
    if end - at > call::TAPS as u64 {
        at = end - call::TAPS as u64;
    }
    let mut n = 0;
    while at < end {
        let t = TAPS[at as usize % call::TAPS].load(Ordering::Acquire);
        out[n] = (t as u16 as i16, (t >> 16) as u16 as i16);
        n += 1;
        at += 1;
    }
    TAPS_OUT.store(end, Ordering::Release);
    n
}

/// Drag points on the circle not yet taken (display op 90: a finger dragged on it while it is the centred one): (x, y) as a tap's, and the phase (`call::DRAG_DOWN`, `DRAG_MOVE`, `DRAG_UP`), packed x in bits 0-15, y in 16-31, the phase in 32-39. A ring as the taps', the oldest going when it is full.
static DRAGS: [AtomicU64; call::DRAGS] = [const { AtomicU64::new(0) }; call::DRAGS];
static DRAGS_IN: AtomicU64 = AtomicU64::new(0);
static DRAGS_OUT: AtomicU64 = AtomicU64::new(0);

/// The drag points since the last call, oldest first, into `out`; how many.
/// Every point, not only the latest: a puck that turns a value by the finger's angle about the centre unwraps it point to point, and a lagging puck that heard only the latest could lose a turn.
pub fn drags(out: &mut [(i16, i16, u8); call::DRAGS]) -> usize {
    let end = DRAGS_IN.load(Ordering::Acquire);
    let mut at = DRAGS_OUT.load(Ordering::Acquire);
    // The ring holds the last `DRAGS`: anything older was overwritten.
    if end - at > call::DRAGS as u64 {
        at = end - call::DRAGS as u64;
    }
    let mut n = 0;
    while at < end {
        let t = DRAGS[at as usize % call::DRAGS].load(Ordering::Acquire);
        out[n] = (t as u16 as i16, (t >> 16) as u16 as i16, (t >> 32) as u8);
        n += 1;
        at += 1;
    }
    DRAGS_OUT.store(end, Ordering::Release);
    n
}

/// Wake the main loop (a worker thread finished something the face shows).
pub fn ring() {
    BELL.fetch_add(1, Ordering::Release);
    // Every thread that waits on it: the main loop and the place thread.
    ferros_sys::wake(&BELL, 1 << 4);
}

static mut WATCH_STACK: [u8; 1 << 13] = [0; 1 << 13];
static mut WATCH_LINK: Link = Link { grants: 0, slot: 0, square: 0 };

extern "C" fn watcher(_: u64) -> ! {
    let link = unsafe { WATCH_LINK };
    let mut seen = 0u32;
    let mut failed = false;
    loop {
        let mut m = [0u8; 6];
        m[0] = call::WATCH;
        m[1..5].copy_from_slice(&seen.to_le_bytes());
        m[5] = WANTS.load(Ordering::Acquire) as u8;
        let mut r = [0u8; call::WATCH_REPLY];
        let n = ferros_sys::call(cap(link.grants, link.slot), &m, &mut r);
        // A display that ended holding the watch fails it once (the kernel answers ERR for a server gone); the next call waits for the display that comes after it, which answers at once (its count starts again), and the deck binds this puck to its circle there again (docs/PUCKS.md "When the display starts again"). Two failures in a row are no display behind the grant: asking again would only spin.
        if n == ferros_sys::ERR && !failed {
            failed = true;
            continue;
        }
        failed = false;
        // No display, or none that knows this puck: the view stays as it was and the puck runs on its deadlines alone.
        if n == ferros_sys::ERR || n < 14 || r[0] != 0 {
            ferros_sys::console(link.grants, b"puck: the display does not answer the view's watch; following deadlines only\n");
            loop {
                ferros_sys::sleep_until(u64::MAX);
            }
        }
        seen = u32::from_le_bytes(r[1..5].try_into().unwrap());
        let d = u16::from_le_bytes([r[7], r[8]]);
        TURN.store(u16::from_le_bytes([r[9], r[10]]) as u64 | (u16::from_le_bytes([r[11], r[12]]) as u64) << 16, Ordering::Release);
        VIEW.store(seen as u64 | (d as u64) << 32 | (r[5] as u64 & 1) << 48 | (r[6] as u64 & 1) << 49, Ordering::Release);
        // WHY the bounds: the counts are the display's word and the reply holds at most `TAPS` taps and `DRAGS` drag points, in the `n` bytes it says it wrote.
        let n = n as usize;
        let k = (r[13] as usize).min(call::TAPS).min((n - 14) / 4);
        for j in 0..k {
            let at = 14 + 4 * j;
            let t = u16::from_le_bytes([r[at], r[at + 1]]) as u64 | (u16::from_le_bytes([r[at + 2], r[at + 3]]) as u64) << 16;
            let i = TAPS_IN.load(Ordering::Acquire);
            TAPS[i as usize % call::TAPS].store(t, Ordering::Release);
            TAPS_IN.store(i + 1, Ordering::Release);
        }
        // The drags follow the taps the reply holds, only when there are any.
        let dat = 14 + 4 * k;
        if n > dat {
            let m = (r[dat] as usize).min(call::DRAGS).min((n - dat - 1) / 5);
            for j in 0..m {
                let at = dat + 1 + 5 * j;
                let t = u16::from_le_bytes([r[at], r[at + 1]]) as u64 | (u16::from_le_bytes([r[at + 2], r[at + 3]]) as u64) << 16 | (r[at + 4] as u64) << 32;
                let i = DRAGS_IN.load(Ordering::Acquire);
                DRAGS[i as usize % call::DRAGS].store(t, Ordering::Release);
                DRAGS_IN.store(i + 1, Ordering::Release);
            }
        }
        ring();
    }
}

/// Start the view's watch (once); `frames` asks for every frame the deck shows while the circle is on the glass (`call::FRAMES`).
pub fn watch(link: Link, frames: bool) {
    unsafe { WATCH_LINK = link };
    WANTS.store(if frames { call::FRAMES as u64 } else { 0 }, Ordering::Release);
    if ferros_sys::thread_spawn(watcher, unsafe { &mut *(&raw mut WATCH_STACK) }, 0) != 0 {
        ferros_sys::console(link.grants, b"puck: no watch thread; following deadlines only\n");
    }
}

/// Sleep until the bell moves past `seen` (a view change, a tap, a frame, a worker's result) or Eagle time `deadline` (None: no deadline). Returns the bell's count.
pub fn wait(seen: u64, deadline: Option<i64>) -> u64 {
    let until = deadline.map_or(u64::MAX, ticks_at);
    if BELL.load(Ordering::Acquire) == seen {
        ferros_sys::wait(&BELL, seen, until);
    }
    BELL.load(Ordering::Acquire)
}

/// The bell's count now (the `seen` the next `wait` takes).
pub fn bell() -> u64 {
    BELL.load(Ordering::Acquire)
}

/// The square: the bytes its slot holds (past the porch), the square the display has now (offset, bytes), which is never drawn into; and the scratch the face is drawn in first, at the top of the puck's own arc (arc offset, bytes).
/// A masked puck reaches its slot only thru its handle (SANDBOX.md "Handles": every plain address is folded into the arc, so a pointer to the slot's address writes the arc's own first page), so the face is drawn in the arc and copied in (`Handle::write_words`); native code runs the same instructions.
struct Held {
    held: u64,
    shown: Option<(u64, u64)>,
    scratch: (usize, usize),
}
static mut HELD: Held = Held { held: 0, shown: None, scratch: (0, 0) };
/// The square's slot as a handle: a puck's surface slot (ferros_kist `slots`).
type Square = ferros_sys::Handle<{ ferros_sys::kist::SURFACE }>;
/// The porch every slot starts with (cachemap SURFACE_PORCH): offsets in a SHOW are past it.
const PORCH: u64 = 64;

/// The scratch at least `bytes` long: the arc grown at its top (the scratch is always the arc's last bytes, nothing else of the puck grows it), or given back down to twice what is needed when it holds more than eight times it. None when the kernel refuses the growth.
fn scratch(h: &mut Held, bytes: usize) -> Option<&'static mut [u64]> {
    if h.scratch.1 < bytes {
        let (top, was) = ferros_sys::arc::grow(bytes - h.scratch.1).ok()?;
        // The first growth starts the scratch; a later one extends it in place (the arc's top was the scratch's end).
        if h.scratch.1 == 0 || h.scratch.0 + h.scratch.1 != was {
            h.scratch = (was, top - was);
        } else {
            h.scratch.1 = top - h.scratch.0;
        }
    } else if h.scratch.1 > 8 * bytes {
        if let Ok((top, _)) = ferros_sys::arc::shrink(h.scratch.0 + 2 * bytes) {
            h.scratch.1 = top - h.scratch.0;
        }
    }
    // SAFETY: [scratch.0, scratch.0 + scratch.1) is the arc's own top, grown for this alone and zeroed by the kernel; nothing else of the process holds it.
    Some(unsafe { core::slice::from_raw_parts_mut((ferros_sys::arc_base() as usize + h.scratch.0) as *mut u64, bytes / 8) })
}

/// Where the next square of `bytes` goes: at the slot's start when that clears the one the display has, else just past it (the two never overlap, so the display reads a whole square while the next is drawn).
fn next_offset(shown: Option<(u64, u64)>, bytes: u64) -> u64 {
    match shown {
        Some((off, len)) if off < bytes => (off + len + 63) & !63,
        _ => 0,
    }
}

/// Draw `list` into the square at the view's size and turn and hand it to the display. Nothing while the circle is not on the glass. The display's status (`call::OK`, `REFUSED`, `NOT_ON_DECK`), or None when nothing was drawn.
pub fn show(link: Link, v: &View, list: &[u8], theme: &crate::square::Theme, glyphs: &mut ferros_glyphs::Glyphs) -> Option<u8> {
    // The base's digits: tatau's code without its high bit (G#8C, DLS, is dozenal digits with its own time keeping; 12). Unmasked, DLS reached `char::from_digit` as radix 140 and every puck with a number on its face panicked on the 8a (2026-11-18, GPS and Compass).
    let ctx = crate::draw::Ctx { now: eagle_now().unwrap_or(0), zone: ferros_sys::zone(), base: ferros_sys::device_base() & 0x7F };
    show_with(link, v, |px, d, turn| crate::square::paint(px, d, turn, list, &ctx, theme, glyphs))
}

/// `show` for a puck that draws its square itself: `paint(pixels, d, turn)` fills the `d` × `d` square (cleared to nothing first) at the view's turn.
pub fn show_with(link: Link, v: &View, paint: impl FnOnce(&mut [u64], usize, (f32, f32))) -> Option<u8> {
    if !v.shown || v.d == 0 {
        return None;
    }
    let h = unsafe { &mut *(&raw mut HELD) };
    // WHY the bound: the side is the display's word; past `SIDE_MAX` two squares would not fit the slot.
    let d = v.d.min(crate::SIDE_MAX) as u64;
    let bytes = d * d * 8;
    let off = next_offset(h.shown, bytes);
    if off + bytes > h.held {
        match ferros_sys::puck_size(cap(link.grants, link.square), off + bytes) {
            Some(n) => h.held = n,
            None => return Some(call::REFUSED),
        }
    }
    let Some(px) = scratch(h, bytes as usize) else {
        ferros_sys::console(link.grants, b"puck: no room in its arc to draw a square that size\n");
        return Some(call::REFUSED);
    };
    let turn = (v.c as f32 / 16384., v.s as f32 / 16384.);
    px.fill(0);
    paint(px, d as usize, turn);
    // Into the slot thru its handle, past the porch: [off, off + bytes) lies inside the frames the slot holds (`held`, the kernel's answer), which nothing else writes, and clear of the square the display reads (`next_offset`).
    Square::write_words(PORCH + off, px);
    let mut m = [0u8; 11];
    m[0] = call::SHOW;
    m[1..5].copy_from_slice(&(off as u32).to_le_bytes());
    m[5..7].copy_from_slice(&(d as u16).to_le_bytes());
    m[7..9].copy_from_slice(&v.c.to_le_bytes());
    m[9..11].copy_from_slice(&v.s.to_le_bytes());
    let mut r = [0u8; 1];
    let n = ferros_sys::call(cap(link.grants, link.slot), &m, &mut r);
    let status = if n == ferros_sys::ERR || n == 0 { call::REFUSED } else { r[0] };
    if status == call::OK {
        h.shown = Some((off, bytes));
        // The slot back down once the display holds a square at its start and the slot is more than four times what two squares of this size need: a circle pinched small gives its frames back, without a resize at every step of the pinch.
        if off == 0 && h.held > 8 * bytes {
            if let Some(n) = ferros_sys::puck_size(cap(link.grants, link.square), 2 * bytes) {
                h.held = n;
            }
        }
    }
    Some(status)
}

/// The theme's colours a list may name, from the person's theme now (text, text2, plane, ring, lit).
pub fn theme() -> crate::square::Theme {
    let t = ferros_sys::theme::current();
    [t.text, t.text2, t.plane, t.ring, t.lit]
}

/// How long a fix is waited for before the claim is given up (WHY a deadline: indoors the chip has gone 300 s with no fix, GPS.md, and a puck's refresh must not hold the radio for that; LOCATION.md's 128 s).
pub const FIX_GIVE_UP_S: u64 = 1 << 7;

/// A GPS fix (latitude, longitude in radians, its error in metres) through the `gps` right in `gps_slot`: claimed, the driver's own doorbell held (its op 3 WATCH, answered as its fixes and messages move, about once a second while it tracks), its FIX read at each ring, released at the first valid fix or after `FIX_GIVE_UP_S`. None when refused (not granted, nobody present, GPS off) or nothing came.
pub fn gps_fix(grants: u64, gps_slot: u64) -> Option<(f64, f64, u32)> {
    let proof = cap(grants, gps_slot);
    if !ferros_sys::claim::claim(ferros_sys::claim::GPS, proof) {
        return None;
    }
    let give_up = now() + FIX_GIVE_UP_S * freq();
    let mut seen = 0u32;
    let mut got = None;
    while now() < give_up {
        let mut f = [0u8; 33];
        let n = ferros_sys::call_live(proof, &[0], &mut f);
        if n != ferros_sys::ERR && n >= 33 && f[0] == 1 {
            let deg = |at: usize| i32::from_le_bytes(f[at..at + 4].try_into().unwrap()) as f64 / (1u64 << 23) as f64;
            let (lat, lon) = (deg(9), deg(13));
            got = Some((lat.to_radians(), lon.to_radians(), u32::from_le_bytes(f[21..25].try_into().unwrap())));
            break;
        }
        // The driver's doorbell (a driver still replaying its session answers ERR at once: a breath, then ask again, bounded by the give-up).
        let mut w = [0u8; 5];
        w[0] = 3;
        w[1..5].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 4];
        let n = ferros_sys::call_live(proof, &w, &mut r);
        if n == 4 {
            seen = u32::from_le_bytes(r);
        } else {
            ferros_sys::sleep_until(now() + (freq() >> 3));
        }
    }
    ferros_sys::claim::release(ferros_sys::claim::GPS);
    got
}

/// The place a puck keeps: the ~10 km cell of its last fix (ferros_weather's grid and its `place` document), never finer.
pub mod place {
    use ferros_weather::Cell;

    /// The domain key it is kept under.
    pub const KEY: &[u8] = b"place";

    /// Keep the cell in the puck's own domain (`domain` its slot's token), stamped now.
    pub fn keep(domain: u64, cell: Cell, when: i64) -> bool {
        let made = super::eagle_now().unwrap_or(when);
        ferros_weather::kept_build(cell, when, made).is_ok_and(|d| ferros_sys::store_set(domain, KEY, &d))
    }

    /// How old a kept place may grow, while the circle is shown, before a fresh fix is taken: 2^12 s, 68 minutes (LOCATION.md's "at most once an hour", the owner's "maybe we at least lock once an hour"). A deadline the place's own age sets, armed only when a fix is wanted and granted.
    pub const STALE_S: i64 = 1 << 12;

    /// The place now: its cell, its centre (latitude, longitude in degrees), when its fix was; packed for the threads (row + 2^11 in bits 0-15, column in 16-31, a valid bit at 63) beside its time.
    static CELL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static WHEN: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

    fn set(cell: Cell, when: i64) {
        WHEN.store(when, core::sync::atomic::Ordering::Release);
        CELL.store(1 << 63 | ((cell.column as u64) << 16) | (cell.row + (1 << 11)) as u64, core::sync::atomic::Ordering::Release);
    }

    /// The place the puck has: its cell and when its fix was; None before any.
    pub fn current() -> Option<(Cell, i64)> {
        let v = CELL.load(core::sync::atomic::Ordering::Acquire);
        (v >> 63 == 1).then(|| (Cell { row: (v & 0xFFFF) as i32 - (1 << 11), column: ((v >> 16) & 0xFFFF) as u32 }, WHEN.load(core::sync::atomic::Ordering::Acquire)))
    }

    static mut PLACER_STACK: [u8; 1 << 14] = [0; 1 << 14];
    static mut PLACER: (u64, u64, Option<u64>) = (0, 0, None);

    extern "C" fn placer(_: u64) -> ! {
        let (grants, domain, gps) = unsafe { PLACER };
        let Some(gps) = gps else {
            loop {
                ferros_sys::sleep_until(u64::MAX);
            }
        };
        let mut seen = super::bell();
        loop {
            let v = super::view();
            let age_ok = current().is_some_and(|(_, when)| super::eagle_now().is_some_and(|now| now - when < STALE_S * crate::OSC));
            if v.shown && !age_ok {
                if let Some((lat, lon, _err)) = super::gps_fix(grants, gps) {
                    if let (Some(cell), Some(now)) = (Cell::of(lat.to_degrees(), lon.to_degrees()), super::eagle_now()) {
                        set(cell, now);
                        keep(domain, cell, now);
                        super::ring();
                    }
                }
            }
            // Until the circle's view changes or the place grows stale; with no place yet and the circle hidden, until it is shown.
            // A place already stale (the circle hidden, or a fix that failed) arms no deadline: one in the past returns the wait at once, and the thread spun a core (2026-11-26: Tide and Compass each held an A715 busy with the screen off, 2 W on the Pro); the next view change asks again.
            let now = super::eagle_now();
            let deadline = current().map(|(_, when)| when + STALE_S * crate::OSC).filter(|&d| now.is_some_and(|t| d > t));
            seen = super::wait(seen, deadline);
        }
    }

    /// The place at start (the cell kept by the last run), and a thread that takes a fresh fix when the person granted GPS (`gps` the slot of that want), the circle is shown, and the kept one is older than `STALE_S`.
    pub fn start(grants: u64, domain: u64, gps: Option<u64>) {
        if let Some((cell, when)) = load(domain) {
            set(cell, when);
        }
        unsafe { PLACER = (grants, domain, gps) };
        if ferros_sys::thread_spawn(placer, unsafe { &mut *(&raw mut PLACER_STACK) }, 0) != 0 {
            ferros_sys::console(grants, b"puck: no place thread; the kept place only\n");
        }
    }

    /// The cell kept, if any, and when its fix was.
    pub fn load(domain: u64) -> Option<(Cell, i64)> {
        let mut b = [0u8; 1 << 9];
        let n = ferros_sys::store_get(domain, KEY, &mut b)?;
        // WHY: a place document is under 2^8 bytes; a longer value is no place of this puck's.
        if n > b.len() {
            return None;
        }
        ferros_weather::kept_parse(&b[..n]).ok()
    }
}
