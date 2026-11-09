//! switchtest: prints SYS_FACTS 82, then asks the one power switch it is granted for on (input byte 1, or no input) or off (0) and prints the kernel's answer, for testing a switch's own rules from the host: `ferros-bridge spawn switchtest.bin 01 --power android`. Grant slots: 0 console, 1 the switch.
#![no_std]
#![no_main]

use ferros_sys::{cap, console, exit};

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let on = input_len == 0 || unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 1;
    // SYS_FACTS 82 first: whether the device allows Android (2 yes, 1 no), the android switch's other rule.
    let f = ferros_sys::facts().0[82];
    console(grants, match f { 2 => b"switchtest: fact 82 = 2 (Android allowed)\n", 1 => b"switchtest: fact 82 = 1 (Android not allowed)\n", _ => b"switchtest: fact 82 unknown\n" });
    let ok = ferros_sys::power(cap(grants, 1), on);
    console(grants, match (on, ok) {
        (true, true) => b"switchtest: on: accepted\n",
        (true, false) => b"switchtest: on: refused\n",
        (false, true) => b"switchtest: off: accepted\n",
        (false, false) => b"switchtest: off: refused\n",
    });
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
