//! Ramps: the present stage's test screen as an app (DISPLAY-CALIB.md). Its circle on the map launches this capsule; it asks the display server for the ramps (op 21), drawn straight from 16-bit working values through the one table, the dither and the 10-bit layer, and ends. The map comes back on the next tap.
//!
//! The bands, top to bottom: grey to white dithered and undithered; near-black (light to 1/64 of white) dithered, undithered, and as an 8-bit canvas would hand it over; the headroom to 16× white through the roll-off; red, green, blue.
//!
//! Launched by the map (SYS_SPAWN), which passes its own display capability. Grant slots: 0 console, 1 display (use).
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut r = [0u8; 4];
    call(cap(grants, 1), &[21], &mut r);
    console(grants, if r[0] == 0 { b"ramps: on the panel\n" } else { b"ramps: the display is not in 10 bits\n" });
    exit(r[0] as u64)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
