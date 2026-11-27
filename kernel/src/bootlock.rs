//! The bootloader's lock state as ABL told this boot (owner, 2026-11-13: "please. I like."; FLASHER.md step 7): leaving it unlocked is the person's known choice, so ferros shows it, a settings line and a console line, never a nag.
//!
//! ABL writes two keys into the bootconfig it appends to the initrd (modem.rs `bootconfig`), read on both boards (husky 2026-11-13: `diag bootconfig`):
//! - `androidboot.vbmeta.device_state`: `locked` or `unlocked`.
//! - `androidboot.verifiedbootstate`: `green` (locked, the maker's key), `yellow` (locked, the custom key set with `avb_custom_key`: for a ferros boot that is the key this ferros is signed with), `orange` (unlocked), `red` (verification failed).
//!
//! A reload (`bridge reload`) has no ABL handoff: the state stays unknown. The jump from Android and the restart read the bootconfig the hv kept from ABL (restart.rs, LOW_BOOTCONFIG), so they know it.

use core::sync::atomic::{AtomicU64, Ordering};

/// `androidboot.vbmeta.device_state` (fact 87 bits 0-7): 0 not given, 1 locked, 2 unlocked, 3 something else.
pub const DEVICE_LOCKED: u64 = 1;
pub const DEVICE_UNLOCKED: u64 = 2;
/// `androidboot.verifiedbootstate` (fact 87 bits 8-15): 0 not given, 1 green, 2 yellow, 3 orange, 4 red, 5 something else.
pub const VBS_GREEN: u64 = 1;
pub const VBS_YELLOW: u64 = 2;
pub const VBS_ORANGE: u64 = 3;
pub const VBS_RED: u64 = 4;

/// The state as read (fact 87): the two bytes above, `FACT_UNKNOWN` until a bootconfig has been read (a reload, or none found).
pub static STATE: AtomicU64 = AtomicU64::new(crate::process::FACT_UNKNOWN);
/// The first four bytes of `androidboot.vbmeta.public_key_digest` (the key ABL verified this boot's vbmeta with), for the console line alone; 0 when not given.
static KEY: AtomicU64 = AtomicU64::new(0);

/// Read the two keys from ABL's bootconfig text (`modem::keep_bootconfig`, a real boot only).
pub fn read(text: &[u8]) {
    let device = match crate::modem::bootconfig_value(text, b"androidboot.vbmeta.device_state").as_deref() {
        None => 0,
        Some(b"locked") => DEVICE_LOCKED,
        Some(b"unlocked") => DEVICE_UNLOCKED,
        Some(_) => 3,
    };
    let vbs = match crate::modem::bootconfig_value(text, b"androidboot.verifiedbootstate").as_deref() {
        None => 0,
        Some(b"green") => VBS_GREEN,
        Some(b"yellow") => VBS_YELLOW,
        Some(b"orange") => VBS_ORANGE,
        Some(b"red") => VBS_RED,
        Some(_) => 5,
    };
    STATE.store(device | vbs << 8, Ordering::Relaxed);
    if let Some(d) = crate::modem::bootconfig_value(text, b"androidboot.vbmeta.public_key_digest") {
        // Eight hex digits are four bytes; a digest shorter than that, or not hex, is left out (external data: ABL's).
        let hex = |c: u8| (c as char).to_digit(16).map(|v| v as u64);
        if d.len() >= 8 && d[..8].iter().all(|&c| hex(c).is_some()) {
            KEY.store(d[..8].iter().fold(0, |a, &c| a << 4 | hex(c).unwrap()), Ordering::Relaxed);
        }
    }
}

/// The console's line for this boot (developer output: words fixed, G# numbers).
pub fn console_line() -> alloc::string::String {
    let s = STATE.load(Ordering::Relaxed);
    if s == crate::process::FACT_UNKNOWN {
        return alloc::string::String::from("bootloader: lock state not known (no bootconfig from ABL this boot)\n");
    }
    let device = ["not given", "locked", "unlocked", "not understood"][(s & 0xFF) as usize];
    let vbs = ["not given", "green", "yellow", "orange", "red", "not understood"][(s >> 8 & 0xFF) as usize];
    let key = KEY.load(Ordering::Relaxed);
    let key = if key == 0 { alloc::string::String::from("not given") } else { alloc::format!("G#{key:08X}...") };
    alloc::format!("bootloader: {device}, verified boot {vbs}, vbmeta key {key}\n")
}
