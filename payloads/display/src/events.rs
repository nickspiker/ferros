//! A window's events (WINDOWS.md "The helm"; owner, 2026-11-11: "Home backgrounds the kist so depending on how it was designed, like Photon, it doesn't die but it stops rendering everything and stops playing anything except for notifications", "back should signal escape to the app and go up one page").
//!
//! - **What.** Per surface, the window on it: whether it is behind, and a ring of its last `EVENTS` events (ferros_sys::window `EV_BEHIND`, `EV_FRONT`, `EV_KEY` with a USB HID usage, `EV_TEXT` and `EV_INSET` with a payload), counted.
//! - **How an app hears them.** Op 62 `[base u64][seen u32]`, the app's own surface and the count it last saw, held by the receive thread until the count moves past it (a doorbell, never a poll: owner, 2026-11-10, "No polling!"), then `[0][count u32][state u8: bit 0 behind][k u8][k events u16][payloads]`, the events after `seen`, oldest first, then the payload of each of them whose kind carries one (`PAYLOAD_KINDS` and up: typed text, the keyboard's place), in the events' order, each as long as its event's low byte says. A watch is answered at once when the count already moved; a second watch from the same surface answers the first as things stand and is held in its place.
//! - **Who posts.** The render thread, as it puts a window behind (op 60), brings one to the front (op 59), hands the front one a key (op 61) or what the keyboard typed (op 70), or the keyboard moves (WINDOWS.md "The keyboard"); it answers a held watch there and then.
//! - **A new owner.** Each surface's events belong to the generation its process was given it under (the kernel's preference byte 8 + k): a watch or an event under another starts that surface's from nothing, and a watch held for the last owner is dropped unanswered (its process has ended).
//!
//! Both threads touch the same table, under one spin lock held for a few loads and stores and, at most, one reply.

use super::*;
use core::sync::atomic::AtomicBool;
use ferros_sys::window::{EVENTS, EVENTS_REPLY, PAYLOAD_KINDS, PIECE};

/// The payloads' ring: every event the ring of events still holds has its payload here whole, since each is at most `PIECE` bytes (`post_with` asserts it) and the ring holds `EVENTS` of them.
const BYTES: usize = EVENTS * PIECE;

struct Queue {
    /// The generation these events belong to (0: none yet).
    generation: u8,
    /// Events posted, and whether the window is behind now.
    n: u32,
    behind: bool,
    /// The last `EVENTS`, event i at `i % EVENTS`.
    ring: [u16; EVENTS],
    /// Payload bytes posted (byte j at `j % BYTES`), and where event i's payload starts in that count, at `i % EVENTS`.
    nb: u32,
    bytes: [u8; BYTES],
    at: [u32; EVENTS],
    /// A watch held: its reply token and the count its caller had seen.
    held: Option<([u8; 32], u32)>,
}

const EMPTY: Queue = Queue { generation: 0, n: 0, behind: false, ring: [0; EVENTS], nb: 0, bytes: [0; BYTES], at: [0; EVENTS], held: None };
static mut QUEUES: [Queue; SURFACES] = [EMPTY; SURFACES];
static LOCK: AtomicBool = AtomicBool::new(false);

fn lock() -> &'static mut [Queue; SURFACES] {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    unsafe { &mut *(&raw mut QUEUES) }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// Surface k's queue for `generation`: started from nothing when it held another's (a held watch of the last owner's dropped: its caller has ended).
fn own(q: &mut Queue, generation: u8) {
    if q.generation != generation {
        *q = EMPTY;
        q.generation = generation;
    }
}

/// Answer `tok` with what a caller who saw `seen` has not: the count, the state, the events after `seen`, their payloads.
fn answer(q: &Queue, seen: u32, tok: &[u8; 32]) {
    let mut out = [0u8; EVENTS_REPLY];
    out[1..5].copy_from_slice(&q.n.to_le_bytes());
    out[5] = q.behind as u8;
    // WHY the bounds: the ring holds the last EVENTS, and a count from before this owner's start (a display started again, `seen` past `n`) wraps to a huge difference; either way only what the ring holds and was posted goes, and the state byte says where the window stands.
    let k = q.n.wrapping_sub(seen).min(EVENTS as u32).min(q.n);
    out[6] = k as u8;
    let mut p = 7 + 2 * k as usize;
    for i in 0..k {
        let at = ((q.n - k + i) as usize) % EVENTS;
        let e = q.ring[at];
        out[7 + 2 * i as usize..9 + 2 * i as usize].copy_from_slice(&e.to_le_bytes());
        // PROOF the bytes are still there: these k events are among the last EVENTS posted, whose payloads together are at most EVENTS × PIECE = BYTES, the ring's own length.
        if (e >> 8) as u8 >= PAYLOAD_KINDS {
            let from = q.at[at];
            for j in 0..(e & 0xFF) as u32 {
                out[p] = q.bytes[(from.wrapping_add(j) as usize) % BYTES];
                p += 1;
            }
        }
    }
    ferros_sys::reply(tok, &out[..p]);
}

/// An event for the window on surface k under `generation` (the render thread, ops 59-61): kept, the state followed, a held watch answered.
pub fn post(k: usize, generation: u8, ev: u16) {
    post_with(k, generation, ev, &[]);
}

/// As `post`, a kind that carries a payload (`PAYLOAD_KINDS` and up: typed text, the keyboard's place) with it, its length in the event's low byte. A payload past `PIECE` bytes is the caller's bug.
pub fn post_with(k: usize, generation: u8, ev: u16, payload: &[u8]) {
    assert!(payload.len() <= PIECE);
    let ev = if (ev >> 8) as u8 >= PAYLOAD_KINDS { ev & 0xFF00 | payload.len() as u16 } else { ev };
    let qs = lock();
    let q = &mut qs[k];
    own(q, generation);
    match (ev >> 8) as u8 {
        ferros_sys::window::EV_BEHIND => q.behind = true,
        ferros_sys::window::EV_FRONT => q.behind = false,
        _ => {}
    }
    let i = q.n as usize % EVENTS;
    q.ring[i] = ev;
    q.at[i] = q.nb;
    for &b in payload {
        q.bytes[q.nb as usize % BYTES] = b;
        q.nb = q.nb.wrapping_add(1);
    }
    q.n = q.n.wrapping_add(1);
    if let Some((tok, seen)) = q.held.take() {
        answer(q, seen, &tok);
    }
    unlock();
}

/// Op 62 from surface k's process, its surface live under `generation` (the receive thread): answered at once when there is something it has not seen, else held.
pub fn watch(k: usize, generation: u8, seen: u32, tok: &[u8; 32]) {
    let qs = lock();
    let q = &mut qs[k];
    own(q, generation);
    if let Some((old, s)) = q.held.take() {
        answer(q, s, &old);
    }
    if seen != q.n {
        answer(q, seen, tok);
    } else {
        q.held = Some((*tok, seen));
    }
    unlock();
}
