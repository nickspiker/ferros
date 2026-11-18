//! A puck's side on the device (feature `sys`): its calls to the display, its view's doorbell, Eagle time, a GPS fix when granted, and its place kept as VSF.
//!
//! The shape of every puck: build its lists from its engine at now, send them, then sleep in one wait until the earliest of its content's own next deadline and a change of its circle's view (`View`), never on an interval.
//! The view comes from a held WATCH on the display (a thread of its own, `watch`): the display answers it when the circle is shown or hidden, gains or loses the focus, or changes size class.
//! The place: a GPS fix taken only when the person granted GPS (a claim, released at the first fix or after `FIX_GIVE_UP_S`), rounded to the ~10 km cell (ferros_weather's grid) before it is kept, and kept in the puck's own vault domain as a VSF document so the next start has it without the radio; the finer fix never leaves the process's memory and is never logged.

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

/// The puck's link to the display: the slot of its `puck` right.
#[derive(Clone, Copy)]
pub struct Link {
    pub grants: u64,
    pub slot: u64,
}

impl Link {
    /// Send a list for the circle at rest (`call::REST`) or in focus (`call::FOCUS`); the display's status (`call::OK`, `REFUSED`, `NOT_ON_DECK`).
    pub fn send(&self, variant: u8, generation: u32, list: &[u8]) -> u8 {
        let mut m = [0u8; 6 + crate::LIST_MAX];
        m[0] = call::LIST;
        m[1] = variant;
        m[2..6].copy_from_slice(&generation.to_le_bytes());
        m[6..6 + list.len()].copy_from_slice(list);
        let mut r = [0u8; 1];
        let n = ferros_sys::call(cap(self.grants, self.slot), &m[..6 + list.len()], &mut r);
        if n == ferros_sys::ERR || n == 0 { call::REFUSED } else { r[0] }
    }
}

/// The circle's view as the display last said it: (count, shown, focused, radius in pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub count: u32,
    pub shown: bool,
    pub focused: bool,
    pub r_px: u32,
}

/// The view, packed: count in the low 32 bits, shown bit 32, focused bit 33; the radius beside it.
static VIEW: AtomicU64 = AtomicU64::new(0);
static R_PX: AtomicU64 = AtomicU64::new(0);
/// Rung by anything the main loop should wake for besides the view (a fix, a fetch): its count moves.
static BELL: AtomicU64 = AtomicU64::new(0);

/// The view now.
pub fn view() -> View {
    let v = VIEW.load(Ordering::Acquire);
    View { count: v as u32, shown: v >> 32 & 1 == 1, focused: v >> 33 & 1 == 1, r_px: R_PX.load(Ordering::Acquire) as u32 }
}

/// Wake the main loop (a worker thread finished something the face shows).
pub fn ring() {
    BELL.fetch_add(1, Ordering::Release);
    // Every thread that waits on it: the main loop and the place thread.
    ferros_sys::wake(&BELL, 1 << 4);
}

static mut WATCH_STACK: [u8; 1 << 13] = [0; 1 << 13];
static mut WATCH_LINK: Link = Link { grants: 0, slot: 0 };

extern "C" fn watcher(_: u64) -> ! {
    let link = unsafe { WATCH_LINK };
    let mut seen = 0u32;
    loop {
        let mut m = [0u8; 5];
        m[0] = call::WATCH;
        m[1..5].copy_from_slice(&seen.to_le_bytes());
        let mut r = [0u8; 11];
        let n = ferros_sys::call(cap(link.grants, link.slot), &m, &mut r);
        // No display, or none that knows this puck: the view stays as it was and the puck runs on its deadlines alone; asking again would only spin.
        if n == ferros_sys::ERR || n < 11 || r[0] != 0 {
            ferros_sys::console(link.grants, b"puck: the display does not answer the view's watch; following deadlines only\n");
            loop {
                ferros_sys::sleep_until(u64::MAX);
            }
        }
        seen = u32::from_le_bytes(r[1..5].try_into().unwrap());
        R_PX.store(u32::from_le_bytes(r[7..11].try_into().unwrap()) as u64, Ordering::Release);
        VIEW.store(seen as u64 | (r[5] as u64 & 1) << 32 | (r[6] as u64 & 1) << 33, Ordering::Release);
        ring();
    }
}

/// Start the view's watch (once).
pub fn watch(link: Link) {
    unsafe { WATCH_LINK = link };
    if ferros_sys::thread_spawn(watcher, unsafe { &mut *(&raw mut WATCH_STACK) }, 0) != 0 {
        ferros_sys::console(link.grants, b"puck: no watch thread; following deadlines only\n");
    }
}

/// Sleep until the bell moves past `seen` (a view change, a worker's result) or Eagle time `deadline` (None: no deadline). Returns the bell's count.
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
            let deadline = current().map(|(_, when)| when + STALE_S * crate::OSC);
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
