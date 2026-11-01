//! The sensors process (KERNEL-OUT.md move 4, phase 1): the USF conversation with the AoC — Google's sensor framework, spoken directly over the AoC's IPC rings — out of the kernel. It opens ferros's channel, pushes the board's sensor registry, subscribes to gravity, proximity and the rear spectral sensor, tells the AoC the display's state, follows the light-scope preference, and publishes every sample in one block the kernel reads in place: SYS_FACTS and SYS_LIGHT answer from it, and the power key's wake rule reads proximity from it (sensorlink.rs).
//!
//! The kernel keeps the AoC's start — its rails, the firmware through GSA and Trusty — for now: those are secure-world calls (phase 2). This process waits for the AoC to come online, then takes the conversation; should it end, the kernel takes the conversation back.
//!
//! Spawn: ferros-bridge spawn sensors.bin --name sensors --dma aoc-ipc --hw aoc-mbox --store aoc --sensors --power sensors
//! Grant slots: 0 console, 1 the AoC's IPC block (`aoc-ipc`), 2 its mailboxes (`aoc-mbox`), 3 the vault scope `aoc.` (the registry scripts), 4 the Sensors right (publish), 5 the `sensors` power switch.
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

global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym sensors_main
);

/// The heap: the protocol's messages (a kilobyte or two each), the registry scripts while they are pushed.
const HEAP_SIZE: usize = 2 << 20;
#[repr(C, align(64))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

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

pub extern "C" fn sensors_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw mut HEAP) as usize, HEAP_SIZE) };
    G.store(grants, core::sync::atomic::Ordering::Relaxed);
    usf::init(grants);
    // The conversation from the kernel first: from here it neither polls USF nor opens it after the AoC comes online.
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say("sensors: the kernel kept the conversation\n");
        ferros_sys::exit(1);
    }
    let block: &[u64] = unsafe { &(*(&raw const BLOCK)).0 };
    if !ferros_sys::sensors_publish(cap(grants, S_SENSORS), block) {
        say("sensors: the kernel refused the block\n");
        ferros_sys::power(cap(grants, S_POWER), false);
        ferros_sys::exit(1);
    }
    let f = freq();
    // The AoC's firmware is the kernel's to start (a few seconds into the boot): wait for its control block.
    let t0 = now();
    while !usf::aoc_online() {
        sleep_until(now() + f / 16);
    }
    say(&alloc::format!("sensors: AoC online after A#{} ms of waiting\n", (now() - t0) * 1000 / f));
    let mut out = alloc::string::String::new();
    let ok = usf::open(&mut out) && usf::registry(&mut out);
    say(&out);
    out.clear();
    if !ok {
        say("sensors: USF did not open; the conversation goes back to the kernel\n");
        ferros_sys::power(cap(grants, S_POWER), false);
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
    usf::subscribe_gravity(GRAVITY_MS, false, &mut out);
    let mut screen_off = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
    usf::display_info(DISPLAY_DBV, !screen_off, DISPLAY_HZ, &mut out);
    if let Some(h) = usf::find("TMD3719 Proximity") {
        usf::subscribe(h, 25, 2, false, &mut out);
    }
    if let Some(h) = usf::find("VD6282 Spectral") {
        usf::subscribe(h, 200, 1, false, &mut out);
    }
    say(&out);
    out.clear();
    // The block is valid from here: the kernel answers from it once the magic is written.
    usf::publish();
    unsafe { core::ptr::write_volatile(&raw mut (*(&raw mut BLOCK)).0[0], ferros_sys::sensor_block::MAGIC) };
    say("sensors: serving\n");
    let mut scope = 0u8;
    loop {
        usf::drain(None);
        usf::publish();
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
        // The flicker stream sends about a hundred small messages a second into a 256-slot ring: every 2 ms is far inside it.
        sleep_until(now() + f / 512);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
