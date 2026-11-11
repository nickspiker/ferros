//! init: the first process of a cold boot, started by the kernel once the boot ring completes (the USB link up, or 1 s after the press without a cable). It holds exactly the capabilities the shell needs and starts every process of the home from vault capsules, each with its share (SYS_SPAWN: copies of init's own capabilities, never more), then ends. Policy lives here, in a signed and replaceable capsule; the kernel only starts init (owner, 2026-10-22).
//!
//! The shell (SHELL.md): `display` owns the panel and keeps the map's scene, `touch` serves contacts, `ahau` is the attest screen, `map` places the circles and turns touch into the camera. Started in that order; a spawn only queues, so init waits for the attest screen's window before it starts the map (`wait_front`).
//!
//! Then the network (WIFI.md), after the shell so the screen never waits for it: `pcie1`, the WiFi driver, brings up PCIe channel 1, loads the chip's firmware from the vault and joins the network it holds the credential for, then serves Ethernet frames on `wlan`; `ip` (ARP, IPv4 + DHCP, IPv6, UDP) serves `udp` over it. Both are processes with only their own grants; the kernel only hands them out.
//!
//! Grant slots, as the kernel hands them to init (service.rs `start_home`, same order):
//!   1 serve display, 2-7 decon0-main, decon0-win, decon0-wincon, dpu-dma0, dpp0, gpio-gpp0, 8-9 dma display-fb, display-heap, 10-13 dsim0, dphy0, sramc0, hdrcomm0,
//!   14 serve touch, 15-16 usi2-spi, gpio-gpn0, 23 the touch interrupt line, 17 use display, 18 use touch, 19 settings (the map holds it: the device's settings are the device's own circle), 20 the vault scope `map.` (the arrangement made by hand; the display reads the person's own wallpaper from it, `map.wallpaper`), 21 dma display-phases (the display's four temporal dither frames), 22 dma surfaces (the four apps' surfaces, WINDOWS.md: the display composes them; an app the map starts gets one of its own). 26 dma display-sprites (the display's sprite cache), 27 dma display-canvas (its 16-bit back buffer).
//!   28 serve wlan, 29-40 the WiFi driver's hardware (pcie1-elbi … pmu), 41 entropy, 42 dma pcie1-dma, 43 the vault scope `wifi.`, 44 use wlan, 45 serve udp,
//!   46 serve fgtw, 47 use fgtw, 48 the vault scope `fgtw.`, 49 the ira key, 50 use udp (payloads/fgtw, the identity service).
//!   51 the vault scope `ahau.`, 52 use rmnet (the cellular link, the IP process's second), 53 serve rmnet, 54-68 the modem's hardware, 69-71 its shared memory, 72-76 its partitions (payloads/modem).
//!   77-79 the NFC driver's, 80 use nfc, 81-83 the Bluetooth driver's, 84 use bt, 85 the identity unlock right, 86-90 the USB driver's (registers, DMA, interrupt, PT commands, power). 91 the touch controller's power. 92-96 the sensors process's (the AoC's IPC block and mailboxes, the vault scope `aoc.`, the Sensors right, its switch).
//!   104 the Kists right (launching a kist with exactly its granted rights, KISTS.md), 105 serve launch, 106 use launch (the map's), 107 the vault scope `kists.` (the person's list); 104-108 were the GPU service's, which the kernel starts itself since 2026-11-08.
//!   108 use kists (the map's: the installer, payloads/kists, which the kernel starts itself with its own grants once a person has attested, as it does GPS).
#![no_std]
#![no_main]

extern crate alloc;

use ferros_sys::{call_live, cap, console, freq, now, sleep_until, spawn, ERR};

/// Grant slot 104: the Kists right (KISTS.md); 105: serve launch; 107: the vault scope `kists.`.
const S_KISTS: u64 = 104;
const S_LAUNCH: u64 = 105;
const S_KIST_LIST: u64 = 107;

/// The heap the person's list of kists is read in (ferros_kist), a launch at a time.
const HEAP_SIZE: usize = 1 << 18;
#[repr(C, align(64))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);
#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();
/// The launcher's stack (`launcher`): a list parsed through vsf wants more than a keeper's.
const LAUNCH_STACK: usize = 1 << 16;
#[repr(C, align(16))]
struct LaunchStack([u8; LAUNCH_STACK]);
static mut LSTACK: LaunchStack = LaunchStack([0; LAUNCH_STACK]);

/// Grant slot 17: use display (the header's list).
const S_USE_DISPLAY: u64 = 17;
/// Grant slot 85: the identity unlock right (asked only).
const S_UNLOCK: u64 = 85;

/// Each process: its name, its vault key, and which of init's slots it receives, in the order its own grant slots expect them (payloads/display, payloads/touch, payloads/map headers).
const SHELL: [(&[u8], &[u8], &[u64]); 12] = [
    // What runs before a person is present (AUTH.md, the device class): the screen, touch and the attest screen, then the network and the radios.
    (b"display", b"display", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 21, 22, 25, 26, 27, 97, 98, 99, 100, 101, 102, 103, 20]),
    (b"touch", b"touch", &[14, 15, 16, 23, 91]),
    // The attest screen, before anything else draws: 1 use display, 2 use touch, 3 a surface (init's `surfaces` passes as one of its own), 4 the vault scope `ahau.`, 5 the vault scope `fgtw.`, 6 the ira key, 7 settings (its layout switch writes the device's layout), 8 use fgtw (the identity service's verdict on the identity, and the mint), 9 the identity unlock right, 10 a second surface (the keyboard's back buffer; `surfaces` again) (payloads/ahau).
    (b"ahau", b"ahau", &[17, 18, 22, 51, 48, 49, 19, 47, 85, 22, 24, 68, 44]),
    // The USB driver (payloads/usb, USB.md): it runs the controller and serves Photon Transport to the bridge, handing each command to the kernel. 1 the controller's registers, 2 its DMA memory, 3 its interrupt, 4 the kernel's PT commands, 5 the `usb` power switch. The kernel has no USB of its own: should the driver end, the keeper below starts it again.
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
    (b"modem", b"modem", &[53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 40, 69, 70, 71, 72, 73, 74, 75, 76]),
    // What reads the person's space (the identity class), each when it may (`wait_person`). fgtw, the identity service, once an identity is unlocked: it is part of attest, looking the identity up and joining its fleet (its chain and key; not the Photon app, which a person installs): 1 use udp, 2 the vault scope `fgtw.`, 3 the ira key, 4 serve fgtw, 5 entropy (the fleet-key minting), 6 use nfc, 7 use bt, 8 the identity unlock right, 9 the vault scope `map.` and 10 the vault scope `kists.` (the person's identity space, joined to their fleet's: payloads/fgtw space.rs). The map only once the device is also bound to that identity (owner, 2026-11-04: "if it's not attached to an identity, not usable"): its arrangement, 1 use display, 2 use touch, 3 settings, 4 the vault scope `map.`, 5 use fgtw (the Fleet page).
    (b"fgtw", b"fgtw", &[50, 48, 49, 46, 41, 80, 84, 85, 20, 107]),
    (b"map", b"map", &[17, 18, 19, 20, 47, 107, 106, 108]),
];

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The table's names and slot lists are `&'static` slices, absolute addresses fixed at link time: apply the payload's relocations before reading them (the first run faulted at address 0 without this, 2026-10-22, as the map once did).
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw mut HEAP) as usize, HEAP_SIZE) };
    let mut failed = 0u64;
    // The keepers' stacks: in this frame, which never returns.
    let mut stacks = Stacks([[0u8; KEEPER_STACK]; KEPT.len()]);
    // After the jump from a running Android (fact 31 = 6) the radios and the AoC are still in Android's state, and their drivers know only the bootloader's: the WiFi driver's first register write hung the bus (the fifth jump, 2026-11-01), as the modem's read and the AoC start had. They wait for drivers that bring their block up from any state; the shell, the screen and USB start as always.
    // Fact 31 = 7, ferros's own restart (power + volume up, the kernel's restart.rs), meets the state the last session left the same way: these stand down until each has a bring-up from any state.
    let jumped = matches!(ferros_sys::facts().0[31], 6 | 7);
    // The GPU too: Android ran it, and whether the hv cuts it off at the jump is still the owner's open decision (KERNEL-OUT.md); the attest screen draws on the CPU without it.
    let after_jump = |n: &[u8]| jumped && matches!(n, b"pcie1" | b"nfc" | b"bt" | b"sensors" | b"modem" | b"gpu");
    for (name, key, slots) in SHELL.iter() {
        if switched_off(name) {
            console(grants, b"init: ");
            console(grants, name);
            console(grants, b" not started: switched off (a status circle)\n");
            continue;
        }
        if after_jump(name) {
            console(grants, b"init: ");
            console(grants, name);
            console(grants, b" not started: the jump from Android left its hardware in Android's state\n");
            continue;
        }

        if *name == b"fgtw" {
            // Every driver the keepers watch is started by now.
            for (k, s) in stacks.0.iter_mut().enumerate() {
                if !after_jump(KEPT[k].0) {
                    ferros_sys::thread_spawn(keeper, s, (grants << 2) | k as u64);
                }
            }
            wait_person(grants, false);
        }
        if *name == b"map" {
            wait_person(grants, true);
        }
        // fgtw again until it starts (2026-11-10, the Pro after a release: the attest screen holds one of the big pool's two arcs and wallgen, remaking a wiped person's wallpaper, the other for about 20 s; fgtw's one try found the pool full and the device sat with no identity service, so nothing of the fleet came back). Each try a second apart, said once.
        if *name == b"fgtw" {
            let mut said = false;
            while spawn(grants, name, key, slots) != 0 {
                if !said {
                    said = true;
                    console(grants, b"init: fgtw could not start yet (no arc free); trying again each second\n");
                }
                sleep_until(now() + freq());
            }
            if said {
                console(grants, b"init: fgtw started\n");
            }
            continue;
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
        console(grants, b"init: shell, network, radios and the person's space started (display, touch, ahau, usb, pcie1, ip, sensors, nfc, bt, modem, fgtw, map)\n");
    }
    // Kists launch from here on (the map's kist circles): the person is present and the device bound by now (`wait_person` before the map).
    ferros_sys::thread_spawn(launcher, unsafe { &mut (*(&raw mut LSTACK)).0 }, grants);
    // init stays: its keepers and its launcher run in this process. The main thread only waits.
    let _ = failed;
    loop {
        sleep_until(now() + freq() * 1024);
    }
}

/// The `launch` endpoint (KISTS.md, owner 2026-11-08): the map asks for a kist by its capsule's address and init launches it with exactly the rights the person granted, read from the person's own list, never the map's word; the map holds none of a kist's rights. A want never answered launches nothing: the map asks first (init never asks, and no kist can). One process per kist: a second tap finds it running.
extern "C" fn launcher(grants: u64) -> ! {
    use ferros_kist::launch::*;
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    loop {
        let n = ferros_sys::recv(cap(grants, S_LAUNCH), &mut msg, &mut token, false);
        if n == ERR {
            continue;
        }
        let answer = if n >= 33 && (msg[0] == OP_LAUNCH || msg[0] == OP_RELAUNCH) { launch(grants, msg[1..33].try_into().unwrap(), msg[0] == OP_RELAUNCH) } else { REFUSED };
        ferros_sys::reply(&token, &[answer]);
    }
}

/// One launch (or, `relaunch`, a running kist killed and launched again after the person changed an answer): the kist found in the person's list by its capsule, its slots from their answers (a right granted, or an empty slot), handed to the kernel, which checks them against the capsule itself.
fn launch(grants: u64, capsule: [u8; 32], relaunch: bool) -> u8 {
    use ferros_kist::launch::*;
    let Some(list) = ferros_kist::store::read(cap(grants, S_KIST_LIST)) else { return UNREADABLE };
    let Some(k) = list.by_capsule(&capsule) else { return NOT_INSTALLED };
    let Some(codes) = k.launch_codes() else { return ASK };
    // WHY: the name is the installer's ASCII; a process name and a request hold 64 bytes, a slot list 64 slots (a longer one is a capsule no request can launch).
    let name = &k.name.as_bytes()[..k.name.len().min(64)];
    if codes.len() > 64 {
        return REFUSED;
    }
    match ferros_sys::spawn_kist(grants, S_KISTS, name, &capsule, &codes, relaunch) {
        0 => {
            console(grants, if relaunch { b"init: relaunching ".as_slice() } else { b"init: launching " });
            console(grants, name);
            console(grants, b"\n");
            STARTED
        }
        ferros_sys::NOTHING => RUNNING,
        _ => REFUSED,
    }
}

const KEEPER_STACK: usize = 8 << 10;
#[repr(C, align(16))]
struct Stacks([[u8; KEEPER_STACK]; KEPT.len()]);

/// The drivers init keeps running (restart policy is init's, not the kernel's: KERNEL-OUT.md; until 2026-11-01 the kernel kept the WiFi driver and took USB back itself). Each: its name in SHELL, the first wait (seconds) between starts, the most the wait doubles to.
/// - usb: the bridge's only way in, so started again within 4 s of going, at most every 64 s.
/// - pcie1: it exits when a join fails, and an access point that throttles repeated joins refuses them all evening (2026-10-29), so at most every 64 s, doubling to 1024 s (a join refused again only adds to a throttle).
/// - sensors: the kernel has no USF of its own since 2026-11-01 (KERNEL-OUT.md decision 2), so a sensors process that ends (the AoC not online in 16 s, USF not opening) is started again like usb; its switch asks the kernel for the AoC's start again.
const KEPT: [(&[u8], u64, u64); 3] = [(b"usb", 4, 64), (b"pcie1", 64, 1024), (b"sensors", 4, 64)];

/// One keeper, its own thread. Every 4 s, once the wait since its driver's last start has passed, it asks the kernel to start the driver only if no process by that name is alive (`spawn_unique`): "already running" is the answer most of the time, and nothing starts; otherwise the driver starts again from the vault with the slots it booted with, and the wait doubles. Running as long as the longest wait, the wait goes back to the first. Asking the driver itself would not do: one alive but not serving yet (the WiFi driver waits for the person's network credential) looks gone to a call, and a second would start beside it. `diag "kill init pcie1"` stops the keepers with the driver.
extern "C" fn keeper(arg: u64) -> ! {
    let (grants, (name, first, most)) = (arg >> 2, KEPT[(arg & 3) as usize]);
    let f = freq();
    let (mut wait, mut started) = (first, now());
    loop {
        sleep_until(now() + 4 * f);
        if now() - started < wait * f {
            continue;
        }
        let Some((n, key, slots)) = SHELL.iter().find(|s| s.0 == name) else { continue };
        // A radio switched off by its status circle stays off: the kernel stopped it on purpose.
        if switched_off(n) {
            continue;
        }
        match ferros_sys::spawn_unique(grants, n, key, slots) {
            ferros_sys::NOTHING => {
                if wait > first && now() - started > most * f {
                    wait = first;
                }
            }
            r => {
                console(grants, if r == 0 { b"init: keeper started " } else { b"init: keeper could not start " });
                console(grants, name);
                console(grants, b" again\n");
                started = now();
                // The policy's ceiling, not a guard: a driver that keeps failing is tried every `most` seconds from then on.
                if wait < most {
                    wait *= 2;
                }
            }
        }
    }
}


/// Whether a radio's driver is switched off (preference 15, the status circles; owner 2026-11-07): the bit in the facts' order, cellular, WiFi, Bluetooth, NFC, GPS. Not a radio: never.
fn switched_off(name: &[u8]) -> bool {
    const RADIOS: [&[u8]; 5] = [b"modem", b"pcie1", b"bt", b"nfc", b"gps"];
    RADIOS.iter().position(|r| *r == name).is_some_and(|i| ferros_sys::pref(ferros_sys::PREF_RADIOS_OFF) >> i & 1 == 1)
}

/// The person's space opens only with them (AUTH.md). fgtw, the identity service, starts once an identity is unlocked (`bound` false): it is the attest's own, looking the identity up and joining its fleet. The map starts only once the device is also bound to that identity (`bound` true): a device not attached to an identity is not usable, nothing to save settings into and no point to it (owner, 2026-11-04). At once with unlock at boot (the kernel unlocked a bound device before init started), else when someone attests and, for the map, the fleet binds the device or a mint makes it the identity's first. No timeout: without a person there is nothing of theirs to show. Fails closed: an answer the kernel cannot give (no Unlock right) waits forever, where it once started everything.
fn wait_person(grants: u64, bound: bool) {
    let mut said = false;
    loop {
        let ready = match bound {
            false => ferros_sys::identity_unlocked(cap(grants, S_UNLOCK)) == Some(true),
            true => ferros_sys::identity_unlocked(cap(grants, S_UNLOCK)) == Some(true) && ferros_sys::identity_bound(cap(grants, S_UNLOCK)) == Some(true),
        };
        if ready {
            return;
        }
        if !said {
            console(grants, if bound { b"init: waiting for this device to be bound to the identity before the map\n" } else { b"init: waiting for a person (the attest screen) before the identity service\n" });
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

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
