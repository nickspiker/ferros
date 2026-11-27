//! Which number base the device draws in, and whose choice it is (SETTINGS.md "The number base"; 2026-11-22, as the language, lang.rs): the identity's, written here by its identity service (payloads/fgtw, from the person's space: settings flow identity to device only), or this device's own.
//!
//! - **Two choices.** The device follows the identity's while "follow my identity" is on (the default) and the identity has one; else its own.
//! - **A base is tatau's code** (10 arabic, 12 DMS, 16 hex, G#8C DLS); 0 is the identity having none.
//! - **Published** as preference 0 (`PREF_BASE`), the base in effect, which tatau's process-wide base follows (process.rs `set_pref`) and every process reads at draw time. The block has no room for the rest, so this device's, the identity's and the switch are read thru SYS_BASE op 3 (the settings page alone needs them).
//! - **Kept** in the device settings document (settings.rs) as `base` (this device's own), `base_follow` and `base_identity`.

use crate::process::PREF_BASE;

struct State {
    device: u8,
    identity: u8,
    own: bool,
}

/// DMS, tatau's house base, until the document says otherwise.
static mut STATE: State = State { device: 12, identity: 0, own: false };

fn state() -> &'static mut State {
    unsafe { &mut *(&raw mut STATE) }
}

/// Whether `code` is a base tatau knows.
pub fn valid(code: u64) -> bool {
    code <= u8::MAX as u64 && tatau::Base::from_code(code as u8).is_some()
}

/// The base in effect: the identity's while following it and it has one, else this device's.
pub fn effective() -> u8 {
    let s = state();
    if !s.own && s.identity != 0 { s.identity } else { s.device }
}

/// This device's own, the identity's as last written (0 none), and whether the device keeps its own (settings.rs writes them).
pub fn device() -> u8 {
    state().device
}
pub fn identity() -> u8 {
    state().identity
}
pub fn own() -> bool {
    state().own
}

/// SYS_BASE op 3's answer: the base in effect, this device's, the identity's (0 none) and whether the device keeps its own, a byte each from the lowest.
pub fn word() -> u64 {
    let s = state();
    effective() as u64 | (s.device as u64) << 8 | (s.identity as u64) << 16 | (s.own as u64) << 24
}

/// The base in effect into every process's preferences (`held`: the kernel lock is already held).
fn publish(held: bool) {
    if held {
        crate::process::set_pref_held(PREF_BASE, effective());
    } else {
        crate::process::set_pref(PREF_BASE, effective());
    }
}

/// Loaded from the settings document at boot, or a document stored from the bridge (settings.rs `apply`); each code already checked.
pub fn load(device: u8, identity: u8, own: bool, held: bool) {
    let s = state();
    s.device = device;
    s.identity = identity;
    s.own = own;
    publish(held);
}

/// The three things that change it, each kept in the settings document by `keep` and put back when the vault refuses it. With the kernel lock held (from SYS_BASE, SYS_SET_PREF, or `with_lock`).
pub fn set_device(code: u8, keep: impl FnOnce() -> bool) -> bool {
    let was = state().device;
    state().device = code;
    settle(keep, |s| s.device = was)
}

pub fn set_own(own: bool, keep: impl FnOnce() -> bool) -> bool {
    let was = state().own;
    state().own = own;
    settle(keep, |s| s.own = was)
}

pub fn set_identity(code: u8, keep: impl FnOnce() -> bool) -> bool {
    let was = state().identity;
    if was == code {
        return true;
    }
    state().identity = code;
    settle(keep, |s| s.identity = was)
}

fn settle(keep: impl FnOnce() -> bool, undo: impl FnOnce(&mut State)) -> bool {
    if keep() {
        publish(true);
        true
    } else {
        undo(state());
        false
    }
}
