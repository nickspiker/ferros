//! capture: asks the display server for a screencap (op 30) and prints its answer, the finished frame's address (`G#…`) or `canvas`, for tools/screencap. Spawned from the host: `ferros-bridge spawn capture.bin --use display`. Grant slots: 0 console, 1 display (use).
//! With an input, the input is the message instead (its first byte the op) and the reply is printed in hex: the display's development ops from the host, as `ferros-bridge spawn capture.bin 1F --use display` for op 31 (`panel off`, the bench for the panel's power-up).
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit};

const S_DISPLAY: u64 = 1;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut r = [0u8; 16];
    if input_len > 0 {
        let msg = unsafe { core::slice::from_raw_parts((ferros_sys::arc_base() + input) as *const u8, input_len as usize) };
        let n = call(cap(grants, S_DISPLAY), msg, &mut r);
        if n == ferros_sys::ERR {
            console(grants, b"capture: the display server did not answer\n");
            exit(1);
        }
        let mut line = [0u8; 48];
        let mut k = 0;
        for &b in r.iter().take((n as usize).min(r.len())) {
            for d in [b >> 4, b & 15] {
                line[k] = if d < 10 { b'0' + d } else { b'A' + d - 10 };
                k += 1;
            }
        }
        line[k] = b'\n';
        console(grants, &line[..k + 1]);
        exit(0);
    }
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
