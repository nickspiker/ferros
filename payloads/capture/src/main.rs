//! capture: asks the display server for a screencap (op 30) and prints its answer, the finished frame's address (`G#…`) or `canvas`, for tools/screencap. Spawned from the host: `ferros-bridge spawn capture.bin --use display`. Grant slots: 0 console, 1 display (use).
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit};

const S_DISPLAY: u64 = 1;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut r = [0u8; 16];
    let n = call(cap(grants, S_DISPLAY), &[30], &mut r);
    if n == ferros_sys::ERR || n == 0 {
        console(grants, b"capture: the display server did not answer\n");
        exit(1);
    }
    console(grants, &r[..(n as usize).min(r.len())]);
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
