//! The sensors process (KERNEL-OUT.md move 4, phase 1): the USF conversation with the AoC — Google's sensor framework, spoken directly over the AoC's IPC rings — out of the kernel. It opens ferros's channel, pushes the board's sensor registry, subscribes to gravity, proximity and the rear spectral sensor, tells the AoC the display's state, follows the light-scope preference, and publishes every sample in one block the kernel reads in place: SYS_FACTS and SYS_LIGHT answer from it, and the power key's wake rule reads proximity from it (sensorlink.rs).
//!
//! The kernel keeps the AoC's start — its rails, the firmware through GSA and Trusty: secure-world calls it makes on the `sensors` switch's word (decision 2, 2026-11-01: no FF-A door for EL0). This process turns the switch on, waits for the AoC to come online, then holds the whole conversation: the kernel has no USF of its own. Should it end, init's keeper starts another.
//!
//! Spawn: ferros-bridge spawn sensors.bin --name sensors --dma aoc-ipc --hw aoc-mbox --store aoc --sensors --power sensors
//! Grant slots: 0 console, 1 the AoC's IPC block (`aoc-ipc`), 2 its mailboxes (`aoc-mbox`), 3 the vault scope `aoc.` (the registry scripts), 4 the Sensors right (publish), 5 the `sensors` power switch, 6 the AoC's a32 mailbox line (`aoc-mbox-a32`: the kernel adds it after the Sensors right's holder's last slot, as it adds pcie1's line), 7 the motion mailbox's writer's region (`motion-mailbox`, added the same way since 2026-11-19), 8 the thermometer's (`thermo-mailbox`, since 2026-11-20).
//!
//! The motion mailbox (ferros_sys::mailbox, ferros_mailbox::motion; docs/SENSORS.md "The motion mailbox"; owner, 2026-11-19: "same with accelerometer, mic, etc."): gravity, its last two samples, the field and the accelerometer, published with each sample of any, for any number of readers waiting on it, none of them calling this process.
//! The thermo mailbox (ferros_mailbox::thermo; docs/SENSORS.md "The thermo mailbox"; owner, 2026-11-20): the IR thermometer's object and sensor temperatures and every float of its samples, both ranges, published with each sample of either.
//! The block the kernel reads stays as it was (SYS_FACTS, SYS_LIGHT, the wake rule).
#![no_std]
#![no_main]

extern crate alloc;

mod usf;

use core::arch::global_asm;
use ferros_sys::{cap, console, freq, now, sleep_until};

pub const S_IPC: u64 = 1;
pub const S_MBOX: u64 = 2;
pub const S_STORE: u64 = 3;
pub const S_SENSORS: u64 = 4;
pub const S_POWER: u64 = 5;
/// The AoC's a32 mailbox line, which the kernel hands with the Sensors right (process.rs): com.google.usf's doorbell.
pub const S_DOORBELL: u64 = 6;
/// The motion mailbox's writer's region, which the kernel hands with the Sensors right after the line.
pub const S_MOTION: u64 = 7;
/// The thermometer's mailbox's writer's region, which the kernel hands with the Sensors right after motion's.
pub const S_THERMO: u64 = 8;

global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 1
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad {heap}
    .size _entry, . - _entry
"#,
    main = sym sensors_main,
    heap = const HEAP_CEILING
);

/// The heap: the protocol's messages (a kilobyte or two each), the registry scripts while they are pushed.
/// The heap's ceiling in the image header (docs/NO-CEILINGS.md step 1): the heap the static array held until then, all of it mapped at spawn (the kernel maps up to 4 MiB of a growing image's heap, process.rs HEAP_START), so the arc is as it was. It grows past this once the kernel maps a start smaller than the ceiling (step 2), and the number goes with step 3.
const HEAP_CEILING: usize = 2 << 20;

unsafe extern "C" {
    /// The end of .bss (payload.ld): the heap starts there, in the zeroed tail the kernel maps after it.
    static __bss_end: u8;
}

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);

/// The published block (ferros_sys `sensor_block`): in this process's own memory, read in place by the kernel.
#[repr(C, align(64))]
pub struct Block(pub [u64; ferros_sys::sensor_block::WORDS]);
pub static mut BLOCK: Block = Block([0; ferros_sys::sensor_block::WORDS]);

/// One console line.
pub fn say(s: &str) {
    console(G.load(core::sync::atomic::Ordering::Relaxed), s.as_bytes());
}
static G: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The display's brightness as the AoC is told it (the kernel's DISPLAY_DBV), its refresh rate, and gravity's period.
const DISPLAY_DBV: u32 = 1353;
const DISPLAY_HZ: u32 = 120;
const GRAVITY_MS: u64 = 10;
/// The accelerometer's period for the motion mailbox (2026-11-19): 2^4 ms, about 62 a second, the owner's knob.
const ACCEL_MS: u64 = 1 << 4;
/// The IR thermometer's batching (2026-11-20): the AoC samples it at its own fixed rates whatever period is asked (64 a second in the normal range, 20 in the extended: their sensor info's least and most delay are one value) and holds the samples up to 2^9 ms, so each range wakes this process about twice a second (docs/SENSORS.md "The thermo mailbox").
const THERMO_MS: u64 = 1 << 9;

pub extern "C" fn sensors_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw const __bss_end) as usize, _input as usize, _input_len as usize) };
    G.store(grants, core::sync::atomic::Ordering::Relaxed);
    usf::init(grants);
    // The `sensors` switch: the kernel starts the AoC unless it is up (its service loop, some 1.4 s of firmware from the vault, no sooner than 4 s into the boot).
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say("sensors: the kernel refused the switch\n");
        ferros_sys::exit(1);
    }
    let block: &[u64] = unsafe { &(*(&raw const BLOCK)).0 };
    if !ferros_sys::sensors_publish(cap(grants, S_SENSORS), block) {
        say("sensors: the kernel refused the block\n");
        ferros_sys::exit(1);
    }
    let f = freq();
    // Wait for its control block; past 16 s the start failed: this process ends, and init's keeper starts another, whose switch asks again.
    let t0 = now();
    while !usf::aoc_online() {
        if now() - t0 > (1 << 4) * f {
            say("sensors: the AoC did not come online in 16 s; this process ends\n");
            ferros_sys::exit(2);
        }
        sleep_until(now() + f / 16);
    }
    say(&alloc::format!("sensors: AoC online after A#{} ms of waiting\n", (now() - t0) * 1000 / f));
    let mut out = alloc::string::String::new();
    let ok = usf::open(&mut out) && usf::registry(&mut out);
    say(&out);
    out.clear();
    if !ok {
        // One shot (owner, 2026-11-08): the switch it turned on goes off with it, the kernel's doing.
        say("sensors: USF did not open; this process ends, init's keeper starts another\n");
        ferros_sys::exit(2);
    }
    // The sensors appear once the AoC has probed its buses with the registry.
    let t1 = now();
    let mut n = 0;
    while now() - t1 < 4 * f {
        n = usf::sensor_count().unwrap_or(0);
        if n > 0 {
            break;
        }
        sleep_until(now() + f / 16);
    }
    say(&alloc::format!("sensors: A#{n} sensors after A#{} ms\n", (now() - t1) * 1000 / f));
    usf::subscribe_gravity(GRAVITY_MS, false, t1, &mut out);
    let mut screen_off = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
    usf::display_info(DISPLAY_DBV, !screen_off, DISPLAY_HZ, &mut out);
    if let Some(h) = usf::find("TMD3719 Proximity") {
        usf::subscribe(h, 25, 2, false, &mut out);
    }
    if let Some(h) = usf::find("VD6282 Spectral") {
        usf::subscribe(h, 200, 1, false, &mut out);
    }
    // The magnetometers for the compass puck (2026-11-17), every 2^6 ms, continuous: the field the puck turns its dial by while its circle is shown. Found by the part's name (MMC56X3X in husky's registry, SENSORS.md); whether the fused type-6 sensor carries that name is to be read on the device (`diag usf sensors`).
    if let Some(h) = usf::find("MMC56") {
        usf::subscribe(h, 1 << 6, 1, false, &mut out);
    }
    // The accelerometer for the motion mailbox (2026-11-19), every 2^4 ms, continuous: raw acceleration, gravity included, beside the fused gravity at 10 ms (the same part feeds both, so the AoC samples it anyway).
    // Found by its name in husky's registry (husky_common.reg "ICM45631 Accelerometer").
    // Another board names its own part, and without one the mailbox carries gravity and the field alone.
    match usf::find("ICM45631 Accelerometer") {
        Some(h) => {
            usf::subscribe(h, ACCEL_MS, 1, false, &mut out);
        }
        None => out.push_str("sensors: no ICM45631 Accelerometer listed: the motion mailbox carries no acceleration\n"),
    }
    // The IR thermometer for the thermo mailbox (2026-11-20), both ranges, continuous, batched up to 2^9 ms: no unsubscribe is known in USF, so it streams from here on (docs/SENSORS.md "The thermo mailbox").
    // Found by their names in husky's registry and the AoC's firmware ("MLX90632 FIR Temperature", "MLX90632 FIR Extended Temperature"); another board has no such part, and the mailbox stays empty.
    for name in ["MLX90632 FIR Temperature", "MLX90632 FIR Extended Temperature"] {
        match usf::find(name) {
            Some(h) => {
                usf::subscribe_batched(h, THERMO_MS, THERMO_MS, 1, false, &mut out);
            }
            None => {
                let _ = core::fmt::Write::write_fmt(&mut out, format_args!("sensors: no {name} listed: the thermo mailbox leaves that range empty\n"));
            }
        }
    }
    say(&out);
    out.clear();
    // The block is valid from here: the kernel answers from it once the magic is written.
    usf::publish();
    unsafe { core::ptr::write_volatile(&raw mut (*(&raw mut BLOCK)).0[0], ferros_sys::sensor_block::MAGIC) };
    say("sensors: serving\n");
    let mut scope = 0u8;
    // The motion mailbox: gravity and the field for any number of readers, published when either has a new sample.
    // None from a kernel before 2026-11-19 (no such slot): the facts alone, as before.
    let motion = ferros_sys::mailbox::Writer::take(cap(grants, S_MOTION));
    if motion.is_none() {
        say("sensors: no motion mailbox to publish in\n");
    }
    let mut motion_seen = (0u64, 0u64, 0u64);
    let mut motion_w = [0u64; ferros_sys::mailbox::motion::WORDS];
    // The thermo mailbox: the thermometer's two ranges, published when either has a new sample. None from a kernel before 2026-11-20 (no such slot).
    let thermo = ferros_sys::mailbox::Writer::take(cap(grants, S_THERMO));
    if thermo.is_none() {
        say("sensors: no thermo mailbox to publish in\n");
    }
    let mut thermo_seen = (0u32, 0u32);
    let mut thermo_w = [0u64; ferros_sys::mailbox::thermo::WORDS];
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        // The doorbell's count before the drain: a ring that lands while this pass runs ends the wait below at once.
        let seen = usf::RUNG.load(core::sync::atomic::Ordering::Acquire);
        usf::drain(None);
        usf::publish();
        if let Some(w) = &motion {
            let counts = usf::motion_words(&mut motion_w);
            if counts != motion_seen {
                motion_seen = counts;
                w.publish(ferros_sys::mailbox::motion::KIND, &motion_w);
            }
        }
        if let Some(w) = &thermo {
            let counts = usf::thermo_words(&mut thermo_w);
            if counts != thermo_seen {
                thermo_seen = counts;
                w.publish(ferros_sys::mailbox::thermo::KIND, &thermo_w);
            }
        }
        // The display's state follows the screen (the under-display sensor times its bursts to the panel), and the light scope follows its preference: both the kernel's to watch before the move.
        let off = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
        if off != screen_off {
            screen_off = off;
            usf::display_info(DISPLAY_DBV, !off, DISPLAY_HZ, &mut out);
        }
        let sc = ferros_sys::pref(ferros_sys::PREF_SCOPE);
        if sc != scope {
            scope = sc;
            usf::scope_follow(sc, &mut out);
        }
        if !out.is_empty() {
            say(&out);
            out.clear();
        }
        // Asleep until the AoC rings (a message on either usf ring) or a preference changes (the kernel wakes every word waiter on one, process.rs `set_pref`): no timer (owner, 2026-11-10: push or doorbell). Before 2026-11-12 the loop woke every 2 ms to look, some 500 wakes a second whether or not anything came.
        usf::wait_ring(seen, 0);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}
