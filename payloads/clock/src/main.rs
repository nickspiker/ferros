//! The clock kist's capsule (owner, 2026-11-10: "can I get my clock widget kist on the homescreen again?"). The clock is a widget: the display server draws its face itself, live, in the kist's circle (display WIDGET_CLOCK, scene icon G#80; the map gives a kist named Clock that icon). So this capsule is what makes the clock installable like any app (KISTS.md: the person installs it, it is theirs and travels with them, nothing is baked in) and has nothing to run: the map does not launch a widget's kist, and if anything does, it says so and ends.
//!
//! Install: tools/clock-kist.sh
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    ferros_sys::console(grants, b"clock: a widget, drawn by the display; nothing to run\n");
    ferros_sys::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
