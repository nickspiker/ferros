//! Which language the shell speaks, and whose choice it is (SETTINGS.md; mirrors the zone, zone.rs: owner, 2026-11-10, "it's per identity, you can uncheck and set per device tho").
//!
//! - **Two choices.** The identity's (written here by its identity service, payloads/fgtw, from the person's space: settings flow identity to device only) and this device's own. The device follows the identity's while "follow my identity" is on (the default) and the identity has one; else its own.
//! - **A language is its ISO 639-3 code**, packed as vsf's `wl` packs it (three letters base 26, `vsf::Language::packed`), so the kernel keeps any language it is given and knows no list: which languages the shell can show is the shell's business (ferros_lang, fgtw::text). 0 (`aaa`, which no shell offers) is none: English.
//! - **Published** in every process's second preferences block (`process::PREFS_HI`): the language in effect, this device's, the identity's (each two bytes, little-endian) and whether the device keeps its own (a byte), so a process reads them from its own memory at draw time with no call.
//! - **Kept** in the device settings document (settings.rs) as `language`, `language_follow` and `language_identity`.

use crate::process::{PREF_LANG, PREF_LANG_DEVICE, PREF_LANG_IDENTITY, PREF_LANG_OWN};

/// The packed codes: three letters base 26 are below 26³.
const CODES: u16 = 26 * 26 * 26;

/// Whether `code` is a packed ISO 639-3 code (or 0, none).
pub fn valid(code: u64) -> bool {
    code < CODES as u64
}

struct State {
    device: u16,
    identity: u16,
    own: bool,
}

static mut STATE: State = State { device: 0, identity: 0, own: false };

fn state() -> &'static mut State {
    unsafe { &mut *(&raw mut STATE) }
}

/// The language in effect: the identity's while following it and it has one, else this device's.
pub fn effective() -> u16 {
    let s = state();
    if !s.own && s.identity != 0 { s.identity } else { s.device }
}

/// This device's own, the identity's as last written, and whether the device keeps its own (settings.rs writes them).
pub fn device() -> u16 {
    state().device
}
pub fn identity() -> u16 {
    state().identity
}
pub fn own() -> bool {
    state().own
}

/// The state into every process's preferences (`held`: the kernel lock is already held).
fn publish(held: bool) {
    let s = state();
    let e = effective().to_le_bytes();
    let d = s.device.to_le_bytes();
    let i = s.identity.to_le_bytes();
    for (index, v) in [(PREF_LANG, e[0]), (PREF_LANG + 1, e[1]), (PREF_LANG_DEVICE, d[0]), (PREF_LANG_DEVICE + 1, d[1]), (PREF_LANG_IDENTITY, i[0]), (PREF_LANG_IDENTITY + 1, i[1]), (PREF_LANG_OWN, s.own as u8)] {
        if held {
            crate::process::set_pref_held(index, v);
        } else {
            crate::process::set_pref(index, v);
        }
    }
}

/// Loaded from the settings document at boot, or a document stored from the bridge (settings.rs `apply`); each code already checked.
pub fn load(device: u16, identity: u16, own: bool, held: bool) {
    let s = state();
    s.device = device;
    s.identity = identity;
    s.own = own;
    publish(held);
}

/// The three things that change it, each kept in the settings document by `keep` (SYS_LANG's store, registered by the vault's owner) and put back when the vault refuses it. With the kernel lock held (from SYS_LANG).
pub fn set_device(code: u16, keep: impl FnOnce() -> bool) -> bool {
    let was = state().device;
    state().device = code;
    settle(keep, |s| s.device = was)
}

pub fn set_own(own: bool, keep: impl FnOnce() -> bool) -> bool {
    let was = state().own;
    state().own = own;
    settle(keep, |s| s.own = was)
}

pub fn set_identity(code: u16, keep: impl FnOnce() -> bool) -> bool {
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
