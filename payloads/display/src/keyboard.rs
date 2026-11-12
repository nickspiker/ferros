//! The keyboard's two ends (WINDOWS.md "The keyboard", 2026-11-12): which window wants it, and the system keyboard's doorbell.
//!
//! - **Who wants it.** The app window in front asks with op 68 as a text field of its gains focus, and lets go as it loses it. One window at a time: the surface and the generation it was shown under. Set and cleared by the render thread alone (ops 68, 70, 71, Back's Escape, and `follow` as the windows change), which keeps it the app in front's or nobody's.
//! - **Who hears it.** The system keyboard (the attest screen's process after attest, payloads/ahau) holds op 69 `[seen u32]`, answered by the receive thread when the count already moved past `seen`, else held until the render thread changes the wanting window: `[0][count u32][wanted u8]`. A doorbell, never a poll.
//!
//! Both threads touch it under one spin lock held for a few loads and stores and, at most, one reply, as the events' table (events.rs).

use super::*;
use core::sync::atomic::AtomicBool;

struct State {
    /// The window that wants the keyboard: its surface and generation.
    want: Option<(usize, u8)>,
    /// Changes to `want`, for the doorbell.
    n: u32,
    /// The keyboard's watch held: its reply token and the count its caller had seen.
    held: Option<([u8; 32], u32)>,
    /// The window last told the keyboard's place, and what it was told (`EV_INSET`).
    told: Option<(usize, u8, u16)>,
}

static mut STATE: State = State { want: None, n: 0, held: None, told: None };
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
    out[5] = s.want.is_some() as u8;
    ferros_sys::reply(tok, &out);
}

/// The window that wants the keyboard now.
pub fn want() -> Option<(usize, u8)> {
    let s = lock();
    let w = s.want;
    unlock();
    w
}

/// The window that wants the keyboard is `w` from now (the render thread): when that changed, counted and the held watch answered.
pub fn set(w: Option<(usize, u8)>) {
    let s = lock();
    if s.want != w {
        s.want = w;
        s.n = s.n.wrapping_add(1);
        if let Some((tok, _)) = s.held.take() {
            answer(s, &tok);
        }
    }
    unlock();
}

/// Op 69 from the system (the receive thread): answered at once when the count moved past `seen`, else held. A second watch answers the first as things stand and is held in its place.
pub fn watch(seen: u32, tok: &[u8; 32]) {
    let s = lock();
    if let Some((old, _)) = s.held.take() {
        answer(s, &old);
    }
    if seen != s.n {
        answer(s, tok);
    } else {
        s.held = Some((*tok, seen));
    }
    unlock();
}

/// The window last told the keyboard's place and what it was told, and the same set to `t` (the render thread).
pub fn told(t: Option<(usize, u8, u16)>) -> Option<(usize, u8, u16)> {
    let s = lock();
    let was = s.told;
    s.told = t;
    unlock();
    was
}
