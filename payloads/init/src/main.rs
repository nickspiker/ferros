//! init: the first process of a cold boot, started by the kernel once the boot ring completes (the USB link up, or 1 s after the press without a cable). It holds exactly the capabilities the shell needs and starts the shell's three processes from vault capsules, each with its share (SYS_SPAWN: copies of init's own capabilities, never more), then ends. Policy lives here, in a signed and replaceable capsule; the kernel only starts init (owner, 2026-10-22).
//!
//! The shell (SHELL.md): `display` owns the panel and keeps the map's scene, `touch` serves contacts, `map` places the circles and turns touch into the camera. Started in that order: each runs until it waits for work, so the display is serving before the map asks it for the panel.
//!
//! Then the network (WIFI.md), after the shell so the screen never waits for it: `pcie1`, the WiFi driver, brings up PCIe channel 1, loads the chip's firmware from the vault and joins the network it holds the credential for, then serves Ethernet frames on `wlan`; `ip` (ARP, IPv4 + DHCP, IPv6, UDP) serves `udp` over it. Both are processes with only their own grants; the kernel only hands them out.
//!
//! Grant slots, as the kernel hands them to init (service.rs `start_home`, same order):
//!   1 serve display, 2-7 decon0-main, decon0-win, decon0-wincon, dpu-dma0, dpp0, gpio-gpp0, 8-9 dma display-fb, display-heap, 10-13 dsim0, dphy0, sramc0, hdrcomm0,
//!   14 serve touch, 15-16 usi2-spi, gpio-gpn0, 23 the touch interrupt line, 17 use display, 18 use touch, 19 settings (the map holds it: the device's settings are the back of the device's own circle), 20 the vault scope `map.` (the arrangement made by hand), 21 dma display-phases (the display's four temporal dither frames), 22 dma surfaces (the four apps' surfaces, WINDOWS.md: the display composes them; an app the map starts gets one of its own). 26 dma display-sprites (the display's sprite cache), 27 dma display-canvas (its 16-bit back buffer).
//!   28 serve wlan, 29-40 the WiFi driver's hardware (pcie1-elbi … pmu), 41 entropy, 42 dma pcie1-dma, 43 the vault scope `wifi.`, 44 use wlan, 45 serve udp,
//!   46 serve photon, 47 use photon, 48 the vault scope `photon.`, 49 the ira key, 50 use udp (PHOTON.md).
#![no_std]
#![no_main]

use ferros_sys::{console, exit, spawn};

/// Each process: its name, its vault key, and which of init's slots it receives, in the order its own grant slots expect them (payloads/display, payloads/touch, payloads/map headers).
const SHELL: [(&[u8], &[u8], &[u64]); 6] = [
    (b"display", b"display", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22, 25, 26, 27]),
    (b"touch", b"touch", &[14, 15, 16, 23]),
    (b"map", b"map", &[17, 18, 19, 20, 22, 47]),
    // The WiFi driver's slots in its own order: 1-12 hardware, 13 entropy, 14 DMA, 15 serve wlan, 16 the vault scope (payloads/pcie1 header).
    (b"pcie1", b"pcie1", &[29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 28, 43]),
    // 1 use wlan, 2 serve udp, 3 entropy (payloads/ip).
    (b"ip", b"ip", &[44, 45, 41]),
    // 1 use udp, 2 the vault scope `photon.`, 3 the ira key, 4 serve photon (payloads/photon).
    (b"photon", b"photon", &[50, 48, 49, 46]),
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
        console(grants, b"init: shell and network started (display, touch, map, pcie1, ip)\n");
    }
    exit(failed)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
