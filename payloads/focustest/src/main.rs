//! A test client that sets the display's focus directly (display op 25), as the map does when a widget is arrived at: input byte 0 is the widget's icon (G#80 the clock, G#82 the scope, 0 none). For exercising a widget's focused drawing without a finger.
//!
//! Spawn: ferros-bridge spawn focustest.bin 80 --name focustest --use display
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let icon = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    // [25][icon][span µs u32 = 1 s][mode 0]
    let mut m = [25u8, icon, 0, 0, 0, 0, 0];
    m[2..6].copy_from_slice(&1_000_000u32.to_le_bytes());
    let mut r = [0u8; 4];
    call(cap(grants, 1), &m, &mut r);
    console(grants, b"focustest: focus sent\n");
    ferros_sys::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
