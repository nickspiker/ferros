//! An IPC client process: holds the send right on one endpoint (grant slot 1), calls it once with its input (or a default), and reports the reply through its console capability.
//!
//! Spawn (after the server): ferros-bridge spawn echo_client.bin [hex-message] --name client --use echo
//! Process ABI and syscalls: payloads/hello_el0 and ferros_kernel process.rs.

#![no_std]
#![no_main]

use core::arch::asm;

fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b) };
    b
}

fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

const EXIT: u64 = 0;
const CALL: u64 = 1;
const REPLY_BUF: u64 = 0x8000;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    let base = arc_base();
    let console = grants + 4;
    let endpoint = grants + 4 + 32;
    let default = b"hello across an endpoint";
    let (msg, len) = if input_len > 0 { (input, input_len) } else { (default.as_ptr() as u64 - base, default.len() as u64) };
    let n = svc(CALL, endpoint, msg, len, REPLY_BUF, 256);
    if n == u64::MAX {
        let e = b"call failed\n";
        svc(CALL, console, e.as_ptr() as u64 - base, e.len() as u64, 0xA00, 16);
    } else {
        let g = b"got: ";
        svc(CALL, console, g.as_ptr() as u64 - base, g.len() as u64, 0xA00, 16);
        svc(CALL, console, REPLY_BUF, n, 0xA00, 16);
        svc(CALL, console, b"\n".as_ptr() as u64 - base, 1, 0xA00, 16);
    }
    unsafe { asm!("svc #0", in("x0") n, in("x8") EXIT, options(noreturn, nostack)) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
