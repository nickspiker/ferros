//! Whether an app window is in front (op 5's byte 12, `app_live`) as a doorbell (op 75; 2026-11-16, init's wait for the attest screen's window asked op 5 64 times a second).
//!
//! - **What rings it.** The two things `app_live` reads: the windows up (`WIN_GENS`, which the render thread writes in `wins_mirror` after every change to its table) and their owners alive (`SURF_LIVE`, the kernel's bell word, which wakes `reaper`). Each calls `changed`, which counts a change only when the answer differs from the one counted last.
//! - **Who hears it.** Whoever holds the display's endpoint, as op 5 answers anyone: op 75 `[seen u32]` is answered by the receive thread when the count already moved past `seen`, else held until it does: `[0][count u32][front u8]`. One watch is held at a time; a second answers the first as things stand and is held in its place. A doorbell, never a poll.
//!
//! The state is touched under one spin lock held for a few loads and stores and, at most, one reply, as the keyboard's (keyboard.rs).

use super::*;
use core::sync::atomic::AtomicBool;

struct State {
    /// The answer counted last.
    front: bool,
    /// Changes to `front`, for the doorbell.
    n: u32,
    /// The watch held: its reply token.
    held: Option<[u8; 32]>,
}

static mut STATE: State = State { front: false, n: 0, held: None };
static LOCK: AtomicBool = AtomicBool::new(false);

fn lock() -> &'static mut State {
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    unsafe { &mut *(&raw mut STATE) }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

fn answer(s: &State, tok: &[u8; 32]) {
    let mut out = [0u8; 6];
    out[1..5].copy_from_slice(&s.n.to_le_bytes());
    out[5] = s.front as u8;
    ferros_sys::reply(tok, &out);
}

/// What `app_live` reads may have changed (`wins_mirror`, `reaper`): when its answer did, counted and the held watch answered.
pub fn changed() {
    let f = app_live();
    let s = lock();
    if s.front != f {
        s.front = f;
        // WHY wrapping: the count is compared by inequality only, so its wrap after 2^32 changes is harmless, never an overflow to trap.
        s.n = s.n.wrapping_add(1);
        if let Some(tok) = s.held.take() {
            answer(s, &tok);
        }
    }
    unlock();
}

/// Op 75 (the receive thread): answered at once when the count moved past `seen`, else held. A second watch answers the first as things stand and is held in its place.
pub fn watch(seen: u32, tok: &[u8; 32]) {
    let s = lock();
    if let Some(old) = s.held.take() {
        answer(s, &old);
    }
    if seen != s.n {
        answer(s, tok);
    } else {
        s.held = Some(*tok);
    }
    unlock();
}
