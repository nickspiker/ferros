//! epcall: one call to an endpoint from the bridge, for poking a server's ops by hand (development). Its input is the message, sent as is to grant slot 1; the reply's length and first bytes go to the console.
//!
//! ferros-bridge spawn epcall.bin <hex message> --name epcall --use <endpoint>
#![no_std]
#![no_main]

use ferros_sys::{call_live, cap, console, exit};

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut msg = [0u8; 256];
    let n = (input_len as usize).min(msg.len());
    for (k, b) in msg[..n].iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k as u64) as *const u8) };
    }
    let mut r = [0u8; 64];
    // CALL_LIVE: a server that is not receiving (down, or busy bringing its device up) is an ERR at once. A waiting call here holds the bridge's spawn, and so the kernel's USB service, until the watchdog resets the device (twice on 2026-10-31).
    let got = call_live(cap(grants, 1), &msg[..n], &mut r);
    let mut line = [0u8; 128];
    let mut k = 0;
    for &c in b"epcall: reply " {
        line[k] = c;
        k += 1;
    }
    let hex = b"0123456789ABCDEF";
    if got == ferros_sys::ERR {
        for &c in b"ERR" {
            line[k] = c;
            k += 1;
        }
    } else {
        for &b in &r[..(got as usize).min(8)] {
            line[k] = hex[(b >> 4) as usize];
            line[k + 1] = hex[(b & 15) as usize];
            line[k + 2] = b' ';
            k += 3;
        }
    }
    line[k] = b'\n';
    console(grants, &line[..k + 1]);
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
