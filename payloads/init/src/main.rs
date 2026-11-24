//! init: the first process of a cold boot, started by the kernel once the boot ring completes (the USB link up, or 1 s after the press without a cable). It holds exactly the capabilities the shell needs and starts every process of the home from vault capsules, each with its share (SYS_SPAWN: copies of init's own capabilities, never more), then ends. Policy lives here, in a signed and replaceable capsule; the kernel only starts init (owner, 2026-10-22).
//!
//! Started again by the kernel at its death by a panic or a fault (docs/KERNEL.md "Restarts", process.rs `KEPT`): every shell process is started with `spawn_unique`, so an init started again starts only what is missing.
//!
//! The shell (SHELL.md): `display` owns the panel and keeps the map's scene, `touch` serves contacts, `ahau` is the attest screen, `map` places the circles and turns touch into the camera. Started in that order; a spawn only queues, so init waits for the attest screen's window before the drivers (`wait_front`, a doorbell from the display), and for a person before the identity service and the map (`wait_person`, the kernel's identity doorbell).
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
//!   108 use kists (the map's: the stower, payloads/kists, which the kernel starts itself with its own grants once a person has attested, as it does GPS).
#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::{cap, console, freq, now, sleep_until, spawn_unique, ERR, NOTHING};

/// Grant slot 104: the Kists right (KISTS.md); 105: serve launch; 107: the vault scope `kists.`.
const S_KISTS: u64 = 104;
const S_LAUNCH: u64 = 105;
const S_KIST_LIST: u64 = 107;

/// The heap the person's list of kists is read in (ferros_kist), a launch at a time.
/// The heap's ceiling in the image header: what the device holds (ferros_sys::HEAP_DEVICE, docs/NO-CEILINGS.md step 3; the kernel takes the least of it and its pool), refused only when the device is out. Addresses only: the kernel maps the start at spawn and the rest as the heap grows (SYS_ARC, ferros_heap::ArcHeap).
const HEAP_CEILING: usize = ferros_sys::HEAP_DEVICE;
/// The heap the kernel maps at spawn, as a log2 (the header's start-size word, process.rs `image_start`): 1 << 18, what the static array held until step 1, so the arc starts as it did and grows from there as the heap runs out. A kernel from before step 2 maps its 4 MiB instead.
const HEAP_START_LOG2: usize = 18;
unsafe extern "C" {
    /// The end of .bss (payload.ld): the heap starts there, in the zeroed tail the kernel maps after it.
    static __bss_end: u8;
}

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);
/// The launcher's stack (`launcher`): a list parsed through vsf wants more than a keeper's.
const LAUNCH_STACK: usize = 1 << 16;
#[repr(C, align(16))]
struct LaunchStack([u8; LAUNCH_STACK]);
static mut LSTACK: LaunchStack = LaunchStack([0; LAUNCH_STACK]);

/// Grant slot 17: use display (the header's list).
const S_USE_DISPLAY: u64 = 17;
/// Grant slot 85: the identity unlock right (asked only).
const S_UNLOCK: u64 = 85;

/// The shell's processes and their grants: ferros_shell, one table with the kernel's restarts (`shell_grants`).
use ferros_shell::SHELL;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    // The table's names and slot lists are `&'static` slices, absolute addresses fixed at link time: apply the payload's relocations before reading them (the first run faulted at address 0 without this, 2026-10-22, as the map once did).
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw const __bss_end) as usize, _input as usize, _input_len as usize) };
    let mut failed = 0u64;
    // The keepers' stacks: in this frame, which never returns.
    let mut stacks = Stacks([[0u8; KEEPER_STACK]; KEPT.len()]);
    // After the jump from a running Android (fact 31 = 6) the AoC is still in Android's state, and its start knows only the bootloader's (the AoC start stalled the service loop, 2026-11-01): sensors waits for a start from any state; the shell, the screen and USB start as always.
    // Fact 31 = 7, ferros's own restart (power + volume up, the kernel's restart.rs), meets the state the last session left the same way.
    // The radios start since 2026-11-15: WiFi and the modem take Android's chip and link down first (each driver's `off` at the head of its bring-up, never the controller's ELBI or DBI before), Bluetooth and NFC power-cycle their chips at every start (the kernel's `bt` and `nfc` switches; the NFC bus is brought up fresh after a jump). Measured on the Pro's jump b878: WiFi joined and leased, Bluetooth and NFC up, Android's live CP taken down.
    let jumped = matches!(ferros_sys::facts().0[31], 6 | 7);
    // Not the GPU: the kernel starts its service itself, after a jump too since 2026-11-22 (owner, 2026-11-21: "if we can bring up GPU and all hardware after a jump"), once it has crowbarred the GPU to its reset (the kernel's pd.rs `gpu_fresh`).
    let after_jump = |n: &[u8]| jumped && n == b"sensors";
    // Whether the map was started ahead of fgtw (the device bound already at the attest).
    let mut map_first = false;
    // The identity's doorbell (SYS_UNLOCK op 11): the kernel writes the identity's state into `IDENTITY` now and at every change, and `wait_person` sleeps on it.
    // Without it (no Unlock right, or a kernel from before 2026-11-16) the word stays 0 and nothing of the person's starts: fail closed, as `wait_person` always has.
    if !ferros_sys::identity_doorbell(cap(grants, S_UNLOCK), &IDENTITY) {
        console(grants, b"init: the kernel gave no identity doorbell; nothing of the person's will start\n");
    }
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
            // A device already bound to the person who just attested (every attest but its first): the deck first, then the identity service. Each spawn holds the kernel lock while it reads its capsule (fgtw 47 ms, the map 70 ms on the 8a, 2026-11-12), and the map waited behind fgtw's; nothing of the map's start needs fgtw, which answers its Fleet page once it serves.
            if IDENTITY.load(Ordering::Acquire) & ferros_sys::ID_BOUND != 0 {
                if let Some((n, k, s)) = SHELL.iter().find(|s| s.0 == b"map") {
                    map_first = matches!(spawn_unique(grants, n, k, s), 0 | NOTHING);
                }
            }
        }
        if *name == b"map" {
            if map_first {
                continue;
            }
            wait_person(grants, true);
        }
        // fgtw again until it starts (2026-11-10, the Pro after a release: the attest screen holds one of the big pool's two arcs and wallgen, remaking a wiped person's wallpaper, the other for about 20 s; fgtw's one try found the pool full and the device sat with no identity service, so nothing of the fleet came back). Each try a second apart, said once.
        if *name == b"fgtw" {
            let mut said = false;
            while !matches!(spawn_unique(grants, name, key, slots), 0 | NOTHING) {
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
        let r = spawn_unique(grants, name, key, slots);
        if r == NOTHING {
            // Running already: this init was started again (the kernel's restart at its death, process.rs `KEPT`), and starts only what of the shell is missing.
            console(grants, b"init: ");
            console(grants, name);
            console(grants, b" running already\n");
        } else if r != 0 {
            console(grants, b"init: could not start ");
            console(grants, name);
            console(grants, b"\n");
            failed += 1;
        } else if *name == b"ahau" {
            // Unlocked and bound at boot (`device.unlock_at_boot`, AUTH.md): ahau shows no attest screen, only stays as the keyboard, so there is no window to wait for (until 2026-11-16 init waited out the 4 s for it, the drivers after it held off that long).
            // The one exception is not waited for either: the identity service holding no roots, which ahau learns from fgtw once init has started it, and its attest screen then comes up over whatever has started (AUTH.md "Never stuck").
            let v = IDENTITY.load(Ordering::Acquire);
            if v & ferros_sys::ID_UNLOCKED == 0 || v & ferros_sys::ID_BOUND == 0 {
                wait_front(grants);
            }
        }
    }
    if failed == 0 {
        console(grants, b"init: shell, network, radios and the person's space started (display, touch, ahau, usb, pcie1, ip, sensors, nfc, bt, modem, fgtw, map)\n");
    }
    // Kists launch from here on (the map's kist circles): the person is present and the device bound by now (`wait_person` before the map).
    ferros_sys::thread_spawn(launcher, unsafe { &mut (*(&raw mut LSTACK)).0 }, grants);
    // init stays: its keepers and its launcher run in this process. The main thread only waits, on a word nothing rings (until 2026-11-16 a sleep woke it every 1024 s for nothing).
    let _ = failed;
    loop {
        ferros_sys::wait_world(&PARKED, 0, 0);
    }
}

/// The main thread's word once the shell is started: never rung, so its wait has no end.
static PARKED: AtomicU64 = AtomicU64::new(0);

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
        let answer = if n >= 33 && (msg[0] == OP_LAUNCH || msg[0] == OP_RELAUNCH) {
            launch(grants, msg[1..33].try_into().unwrap(), msg[0] == OP_RELAUNCH)
        } else if n >= 33 && msg[0] == OP_STOP {
            [stop(grants, msg[1..33].try_into().unwrap()), 0, 0, 1]
        } else {
            [REFUSED, 0, 0, 1]
        };
        ferros_sys::reply(&token, &answer[..answer[3] as usize]);
    }
}

/// One launch (or, `relaunch`, a running kist killed and launched again after the person changed an answer): the kist found in the person's list by its capsule, a code per declared want from their answers (a right granted, or an empty slot; an implicit right always, ferros_kist `launch_codes`), handed to the kernel, which checks them against the capsule itself and lays the slots out (ferros_kist `slots`). The reply's bytes and, last, how many of them go: the answer alone, or RUNNING with the surface and generation the running kist shows on (ferros_kist::launch::RUNNING).
fn launch(grants: u64, capsule: [u8; 32], relaunch: bool) -> [u8; 4] {
    use ferros_kist::launch::*;
    let one = |a: u8| [a, 0, 0, 1];
    let Some(list) = ferros_kist::store::read(cap(grants, S_KIST_LIST)) else { return one(UNREADABLE) };
    let Some(k) = list.by_capsule(&capsule) else { return one(NOT_STOWED) };
    let Some(codes) = k.launch_codes() else { return one(ASK) };
    // WHY: the name is the stower's ASCII; a process name and a request hold 64 bytes, a slot list 64 slots (a longer one is a capsule no request can launch).
    let name = &k.name.as_bytes()[..k.name.len().min(64)];
    if codes.len() > 64 {
        return one(REFUSED);
    }
    match ferros_sys::spawn_kist(grants, S_KISTS, name, &capsule, &codes, relaunch) {
        0 => {
            console(grants, if relaunch { b"init: relaunching ".as_slice() } else { b"init: launching " });
            console(grants, name);
            console(grants, b"\n");
            one(STARTED)
        }
        ferros_sys::NOTHING => one(RUNNING),
        // Running on a surface (the kernel's `RUNNING_ON`): which, so the map can bring its window to the front.
        r if r != ferros_sys::ERR && r & ferros_sys::RUNNING_ON != 0 => [RUNNING, (r >> 8) as u8, r as u8, 3],
        _ => one(REFUSED),
    }
}

/// The person's Stop on a kist's card (owner, 2026-11-12; docs/KISTS.md "The deck and the card"): the kist found in their list by its capsule, its running process killed by the kernel, one shot dead, nothing launched. Answers alone (ferros_kist::launch): STOPPED, NOT_RUNNING, NOT_STOWED, UNREADABLE or REFUSED.
fn stop(grants: u64, capsule: [u8; 32]) -> u8 {
    use ferros_kist::launch::*;
    let Some(list) = ferros_kist::store::read(cap(grants, S_KIST_LIST)) else { return UNREADABLE };
    let Some(k) = list.by_capsule(&capsule) else { return NOT_STOWED };
    // WHY: as a launch's: the name is the stower's ASCII, and a request holds 64 bytes of it.
    let name = &k.name.as_bytes()[..k.name.len().min(64)];
    match ferros_sys::stop_kist(grants, S_KISTS, name, &capsule) {
        0 => {
            console(grants, b"init: stopped ");
            console(grants, name);
            console(grants, b"\n");
            STOPPED
        }
        ferros_sys::NOTHING => NOT_RUNNING,
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

/// The identity's doorbell word (SYS_UNLOCK op 11, `ferros_sys::identity_doorbell`): bit 0 an identity unlocked, bit 1 the device bound, bit 2 the person key held, a change count above bit 8.
/// The kernel writes it at registration and at every change of the identity's state, and wakes whatever waits here; a static, since it writes it for as long as init lives.
static IDENTITY: AtomicU64 = AtomicU64::new(0);

/// The person's space opens only with them (AUTH.md).
/// fgtw, the identity service, starts once an identity is unlocked (`bound` false): it is the attest's own, looking the identity up and joining its fleet.
/// The map starts only once the device is also bound to that identity (`bound` true): a device not attached to an identity is not usable, nothing to save settings into and no point to it (owner, 2026-11-04).
/// At once with unlock at boot (the kernel unlocked a bound device before init started), else when someone attests and, for the map, the fleet binds the device or a mint makes it the identity's first.
/// A doorbell, not a poll (2026-11-16; until then op 0 was asked 64 times a second, up to 16 ms behind the tap): asleep on `IDENTITY` until the kernel rings it, then the state read from the word itself.
/// No timeout: without a person there is nothing of theirs to show.
/// Fails closed: with no doorbell (no Unlock right) the word stays 0 and this waits forever, where it once started everything.
fn wait_person(grants: u64, bound: bool) {
    let mut said = false;
    loop {
        let v = IDENTITY.load(Ordering::Acquire);
        let ready = v & ferros_sys::ID_UNLOCKED != 0 && (!bound || v & ferros_sys::ID_BOUND != 0);
        if ready {
            return;
        }
        if !said {
            console(grants, if bound { b"init: waiting for this device to be bound to the identity before the map\n" } else { b"init: waiting for a person (the attest screen) before the identity service\n" });
            said = true;
        }
        ferros_sys::wait_world(&IDENTITY, v, 0);
    }
}

/// `wait_front`'s word: 1 once its thread heard an app window is in front.
static FRONT: AtomicU64 = AtomicU64::new(0);
/// `front_thread`'s stack: one call's few bytes.
const FRONT_STACK: usize = 1 << 13;
#[repr(C, align(16))]
struct FrontStack([u8; FRONT_STACK]);
static mut FSTACK: FrontStack = FrontStack([0; FRONT_STACK]);

/// The attest screen before the drivers: a spawn only queues the process, and ahau's window should be on the glass before the drivers' starts (each holding the kernel lock while it reads its capsule) come between it and its first frame.
/// Until 2026-11-04 it also kept the map off the glass before Attest (2026-10-31: the map's circles showed first); `wait_person` does that now.
/// Waits until an app window is in front (the display's op 75, the doorbell of op 5's byte 12, held by `front_thread`), asleep on `FRONT` meanwhile.
/// The 4 s bound is a real deadline, not a pace: the USB driver (the bridge's only way in), WiFi and the radios start after this, so an attest screen that fails before its window must not keep them off.
fn wait_front(grants: u64) {
    ferros_sys::thread_spawn(front_thread, unsafe { &mut (*(&raw mut FSTACK)).0 }, grants);
    let end = now() + freq() * 4;
    while FRONT.load(Ordering::Acquire) == 0 {
        if ferros_sys::wait_world(&FRONT, 0, end) == 2 {
            console(grants, b"init: the attest screen's window was not up after 4 s; starting the rest\n");
            return;
        }
    }
}

/// Holds the display's op 75 until an app window is in front, then rings `FRONT` and ends.
/// The display takes the call when it first receives, so one not serving yet needs no asking again.
/// A display without the op, or one that ended, answers None: the thread ends unrung and `wait_front` keeps its deadline.
extern "C" fn front_thread(grants: u64) -> ! {
    let display = cap(grants, S_USE_DISPLAY);
    // No answer gave this count yet (the display counts from 0), so the first call is answered at once with the state now.
    let mut seen = u32::MAX;
    loop {
        match ferros_sys::window::front_watch(display, seen) {
            Some((_, true)) => {
                FRONT.store(1, Ordering::Release);
                ferros_sys::wake(&FRONT, 1);
                break;
            }
            Some((n, false)) => seen = n,
            None => break,
        }
    }
    ferros_sys::thread_exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main, grows HEAP_CEILING, starts HEAP_START_LOG2);
