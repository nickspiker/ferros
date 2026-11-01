//! init: the first process of a cold boot, started by the kernel once the boot ring completes (the USB link up, or 1 s after the press without a cable). It holds exactly the capabilities the shell needs and starts every process of the home from vault capsules, each with its share (SYS_SPAWN: copies of init's own capabilities, never more), then ends. Policy lives here, in a signed and replaceable capsule; the kernel only starts init (owner, 2026-10-22).
//!
//! The shell (SHELL.md): `display` owns the panel and keeps the map's scene, `touch` serves contacts, `ahau` is the attest screen, `map` places the circles and turns touch into the camera. Started in that order; a spawn only queues, so init waits for the attest screen's window before it starts the map (`wait_front`).
//!
//! Then the network (WIFI.md), after the shell so the screen never waits for it: `pcie1`, the WiFi driver, brings up PCIe channel 1, loads the chip's firmware from the vault and joins the network it holds the credential for, then serves Ethernet frames on `wlan`; `ip` (ARP, IPv4 + DHCP, IPv6, UDP) serves `udp` over it. Both are processes with only their own grants; the kernel only hands them out.
//!
//! Grant slots, as the kernel hands them to init (service.rs `start_home`, same order):
//!   1 serve display, 2-7 decon0-main, decon0-win, decon0-wincon, dpu-dma0, dpp0, gpio-gpp0, 8-9 dma display-fb, display-heap, 10-13 dsim0, dphy0, sramc0, hdrcomm0,
//!   14 serve touch, 15-16 usi2-spi, gpio-gpn0, 23 the touch interrupt line, 17 use display, 18 use touch, 19 settings (the map holds it: the device's settings are the back of the device's own circle), 20 the vault scope `map.` (the arrangement made by hand), 21 dma display-phases (the display's four temporal dither frames), 22 dma surfaces (the four apps' surfaces, WINDOWS.md: the display composes them; an app the map starts gets one of its own). 26 dma display-sprites (the display's sprite cache), 27 dma display-canvas (its 16-bit back buffer).
//!   28 serve wlan, 29-40 the WiFi driver's hardware (pcie1-elbi … pmu), 41 entropy, 42 dma pcie1-dma, 43 the vault scope `wifi.`, 44 use wlan, 45 serve udp,
//!   46 serve photon, 47 use photon, 48 the vault scope `photon.`, 49 the ira key, 50 use udp (PHOTON.md).
//!   51 the vault scope `ahau.`, 52 use rmnet (the cellular link, the IP process's second), 53 serve rmnet, 54-68 the modem's hardware, 69-71 its shared memory, 72-76 its partitions (payloads/modem).
//!   77-79 the NFC driver's, 80 use nfc, 81-83 the Bluetooth driver's, 84 use bt, 85 the identity unlock right, 86-90 the USB driver's (registers, DMA, interrupt, PT commands, power). 91 the touch controller's power. 92-96 the sensors process's (the AoC's IPC block and mailboxes, the vault scope `aoc.`, the Sensors right, its switch).
#![no_std]
#![no_main]

use ferros_sys::{call_live, cap, console, exit, freq, now, sleep_until, spawn, ERR};

/// Grant slot 17: use display (the header's list).
const S_USE_DISPLAY: u64 = 17;
/// Grant slot 85: the identity unlock right (asked only).
const S_UNLOCK: u64 = 85;

/// Each process: its name, its vault key, and which of init's slots it receives, in the order its own grant slots expect them (payloads/display, payloads/touch, payloads/map headers).
const SHELL: [(&[u8], &[u8], &[u64]); 12] = [
    // What runs before a person is present (AUTH.md, the device class): the screen, touch and the attest screen, then the network and the radios.
    (b"display", b"display", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22, 25, 26, 27]),
    (b"touch", b"touch", &[14, 15, 16, 23, 91]),
    // The attest screen, before anything else draws: 1 use display, 2 use touch, 3 a surface (init's `surfaces` passes as one of its own), 4 the vault scope `ahau.`, 5 the vault scope `photon.`, 6 the ira key, 7 settings (its layout switch writes the device's layout), 8 use photon (the Photon client's verdict on the identity, and the mint), 9 the identity unlock right (payloads/ahau).
    (b"ahau", b"ahau", &[17, 18, 22, 51, 48, 49, 19, 47, 85]),
    // The USB driver (payloads/usb, USB.md): it takes the controller from the kernel and serves Photon Transport to the bridge, handing each command to the kernel. 1 the controller's registers, 2 its DMA memory, 3 its interrupt, 4 the kernel's PT commands, 5 the `usb` power switch. Should it fail, it gives the controller back and the kernel serves USB as before.
    (b"usb", b"usb", &[86, 87, 88, 89, 90]),
    // The WiFi driver's slots in its own order: 1-12 hardware, 13 entropy, 14 DMA, 15 serve wlan, 16 the vault scope, 17 the ira key (its per-network MAC), 18 Bluetooth's power (off before WL_REG_ON is cycled) (payloads/pcie1 header). It brings the chip up now and joins once the identity's network credential can be read.
    (b"pcie1", b"pcie1", &[29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 28, 43, 49, 83]),
    // 1 use wlan, 2 serve udp, 3 entropy, 4 use rmnet (payloads/ip).
    (b"ip", b"ip", &[44, 45, 41, 52]),
    // The radios: the NFC driver 1 serve nfc, 2 hsi2c_0, 3 gpio-alive, 4 its power (payloads/nfc); the Bluetooth driver 1 serve bt, 2 the APM UART, 3 vendor_a (the patch), 4 its power, 5 entropy (payloads/bt).
    // The sensors process (payloads/sensors): the USF conversation with the AoC once the kernel has the AoC online; it publishes the samples the kernel answers SYS_FACTS and SYS_LIGHT from. 1 the AoC's IPC block, 2 its mailboxes, 3 the vault scope `aoc.`, 4 the Sensors right, 5 the `sensors` switch. Should it fail, the kernel keeps the conversation.
    (b"sensors", b"sensors", &[92, 93, 94, 95, 96]),
    (b"nfc", b"nfc", &[77, 78, 67, 79]),
    (b"bt", b"bt", &[81, 82, 76, 83, 41]),
    // The cellular modem's driver (payloads/modem): it powers the CP, registers and serves the data call on rmnet; ip takes it as its second link.
    (b"modem", b"modem", &[53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76]),
    // What reads the person's space (the identity class), started once an identity is unlocked (`wait_unlocked`): the map (its arrangement), then the Photon client (its chain and key): 1 use udp, 2 the vault scope `photon.`, 3 the ira key, 4 serve photon, 5 entropy (fgtw's fleet-key minting), 6 use nfc, 7 use bt, 8 the identity unlock right (payloads/photon).
    (b"map", b"map", &[17, 18, 19, 20, 22, 47]),
    (b"photon", b"photon", &[50, 48, 49, 46, 41, 80, 84, 85]),
];

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The table's names and slot lists are `&'static` slices, absolute addresses fixed at link time: apply the payload's relocations before reading them (the first run faulted at address 0 without this, 2026-10-22, as the map once did).
    unsafe { ferros_sys::relocate() };
    let mut failed = 0u64;
    for (name, key, slots) in SHELL.iter() {
        if *name == b"map" {
            wait_unlocked(grants);
        }
        if spawn(grants, name, key, slots) != 0 {
            console(grants, b"init: could not start ");
            console(grants, name);
            console(grants, b"\n");
            failed += 1;
        } else if *name == b"ahau" {
            wait_front(grants);
        }
    }
    if failed == 0 {
        console(grants, b"init: shell, network, radios and the person's space started (display, touch, ahau, usb, pcie1, ip, sensors, nfc, bt, modem, map, photon)\n");
    }
    exit(failed)
}

/// The person's space opens only with them (AUTH.md): the map and the Photon client read the identity class, so they start once an identity is unlocked — at once with unlock at boot, else when someone attests. No timeout: without a person there is nothing of theirs to show. A kernel without the Unlock right (an older one) does not hold this up.
fn wait_unlocked(grants: u64) {
    let mut said = false;
    loop {
        match ferros_sys::identity_unlocked(cap(grants, S_UNLOCK)) {
            Some(false) => {}
            _ => return,
        }
        if !said {
            console(grants, b"init: waiting for a person (the attest screen) before the map and Photon\n");
            said = true;
        }
        sleep_until(now() + freq() / 64);
    }
}

/// The attest screen before the map: a spawn only queues the process, so without this the map's few calls reached the glass before ahau's keyboard and window, and its circles showed before Attest (2026-10-31). Waits until an app window is in front (the display's op 5, byte 12), at most 4 s, so an attest screen that fails cannot keep the rest of the shell off. A display not serving yet fails the call at once (CALL_LIVE) and is asked again.
fn wait_front(grants: u64) {
    let end = now() + freq() * 4;
    while now() < end {
        let mut r = [0u8; 16];
        let n = call_live(cap(grants, S_USE_DISPLAY), &[5], &mut r);
        if n != ERR && n >= 13 && r[12] == 1 {
            return;
        }
        sleep_until(now() + freq() / 64);
    }
    console(grants, b"init: the attest screen's window was not up after 4 s; starting the rest\n");
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
