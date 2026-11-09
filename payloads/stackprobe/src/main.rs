//! The stack's guard page, proven (owner, 2026-11-09: a stack run past its 64 KiB faults, named, not into memory it does not own): recurses with a 4 KiB frame, saying each kilobyte of depth on the console, until the kernel's guard page under the stack stops it. Expect about 64 lines, then the process's FAULT report.
#![no_std]
#![no_main]

use ferros_sys::console;

#[inline(never)]
fn deeper(grants: u64, depth: u64) -> u64 {
    // A frame the compiler cannot fold away: 4 KiB written and read back.
    let mut frame = [0u8; 1 << 12];
    frame[0] = depth as u8;
    let frame = core::hint::black_box(&mut frame);
    if depth % 4 == 0 {
        let mut b = *b"stackprobe: 0000 KiB deep\n";
        let mut k = depth * 4;
        for i in (12..16).rev() {
            b[i] = b'0' + (k % 10) as u8;
            k /= 10;
        }
        console(grants, &b);
    }
    frame[1] = deeper(grants, depth + 1) as u8;
    frame[0] as u64 + frame[1] as u64
}

pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let _ = deeper(grants, 0);
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::exit(1)
}

// The entry and its image header (ferros_sys::entry!).
ferros_sys::entry!(payload_main);
