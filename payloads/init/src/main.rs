//! init: the first process of a cold boot, started by the kernel once the boot ring completes (the USB link up, or 1 s after the press without a cable). It holds exactly the capabilities the shell needs and starts the shell's three processes from vault capsules, each with its share (SYS_SPAWN: copies of init's own capabilities, never more), then ends. Policy lives here, in a signed and replaceable capsule; the kernel only starts init (owner, 2026-10-22).
//!
//! The shell (SHELL.md): `display` owns the panel and keeps the map's scene, `touch` serves contacts, `map` places the circles and turns touch into the camera. Started in that order: each runs until it waits for work, so the display is serving before the map asks it for the panel.
//!
//! Grant slots, as the kernel hands them to init (service.rs `start_home`, same order):
//!   1 serve display, 2-7 decon0-main, decon0-win, decon0-wincon, dpu-dma0, dpp0, gpio-gpp0, 8-9 dma display-fb, display-heap, 10-13 dsim0, dphy0, sramc0, hdrcomm0,
//!   14 serve touch, 15-16 usi2-spi, gpio-gpn0, 17 use display, 18 use touch, 19 settings (the map holds it: the device's settings are the back of the device's own circle), 20 the vault scope `map.` (the arrangement made by hand), 21 dma display-phases (the display's four temporal dither frames), 22 dma app-surface (the app in front draws there; the map passes it on).
#![no_std]
#![no_main]

use ferros_sys::{console, exit, spawn};

/// Each process: its name, its vault key, and which of init's slots it receives, in the order its own grant slots expect them (payloads/display, payloads/touch, payloads/map headers).
const SHELL: [(&[u8], &[u8], &[u64]); 3] = [
    (b"display", b"display", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22]),
    (b"touch", b"touch", &[14, 15, 16]),
    (b"map", b"map", &[17, 18, 19, 20, 22]),
];

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The table's names and slot lists are `&'static` slices, absolute addresses fixed at link time: apply the payload's relocations before reading them (the first run faulted at address 0 without this, 2026-10-22, as the map once did).
    unsafe { ferros_sys::relocate() };
    let mut failed = 0u64;
    for (name, key, slots) in SHELL.iter() {
        if spawn(grants, name, key, slots) != 0 {
            console(grants, b"init: could not start ");
            console(grants, name);
            console(grants, b"\n");
            failed += 1;
        }
    }
    if failed == 0 {
        console(grants, b"init: shell started (display, touch, map)\n");
    }
    exit(failed)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
