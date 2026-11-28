//! capture: asks the display server for a screencap (op 30) and prints its answer, the finished frame's address (`G#…`) or `canvas`, for tools/screencap. Spawned from the host: `ferros-bridge spawn capture.bin --use display`. Grant slots: 0 console, 1 display (use).
//! With an input, the input is the message instead (its first byte the op) and the reply is printed in hex, 256 bytes a line and then `end`: the display's development ops from the host, as `ferros-bridge spawn capture.bin 1F --use display` for op 31 (`panel off`, the bench for the panel's power-up).
//! Except the input `m` (6D): the glass's mask alone (op 30 with 1), its answer printed as text like a capture's, for `screencap --mask`.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit};

const S_DISPLAY: u64 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    // Room for a text reply (the nfc driver's STATE runs to a few hundred bytes), printed in hex.
    let mut r = [0u8; 512];
    let msg = unsafe { core::slice::from_raw_parts((ferros_sys::arc_base() + input) as *const u8, input_len as usize) };
    if msg == b"m" {
        let n = call(cap(grants, S_DISPLAY), &[30, 1], &mut r);
        if n == ferros_sys::ERR || n == 0 {
            console(grants, b"capture: the display server did not answer\n");
            exit(1);
        }
        console(grants, &r[..(n as usize).min(r.len())]);
        exit(0);
    }
    if input_len > 0 {
        // The reply as long as the kernel's longest (2^18 bytes, process.rs RECV_MAX): a list of what is on the glass (op 89) runs to kilobytes. In .bss: the stack is far smaller.
        static mut BIG: [u8; 1 << 18] = [0; 1 << 18];
        let big = unsafe { &mut *(&raw mut BIG) };
        let n = call(cap(grants, S_DISPLAY), msg, big);
        if n == ferros_sys::ERR {
            console(grants, b"capture: the display server did not answer\n");
            exit(1);
        }
        // Printed in hex, 256 bytes a line (the bridge joins the lines), so no console line runs past a few hundred characters.
        // WHY the min: the length is the kernel's, at most what the buffer took.
        let got = &big[..(n as usize).min(big.len())];
        for chunk in got.chunks(1 << 8) {
            let mut line = [0u8; (1 << 9) + 1];
            let mut k = 0;
            for &b in chunk {
                for d in [b >> 4, b & 15] {
                    line[k] = if d < 10 { b'0' + d } else { b'A' + d - 10 };
                    k += 1;
                }
            }
            line[k] = b'\n';
            console(grants, &line[..k + 1]);
        }
        // The reply's end, so a reader joining the lines knows it has them all (not a hex digit, so no hex line reads as it).
        console(grants, b"end\n");
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
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
