//! An IPC server process: holds the receive right on one endpoint (grant slot 1) and answers every call with "echo: " + the message upper-cased. It never exits; between calls it is parked in recv.
//!
//! Spawn: ferros-bridge spawn echo_server.bin --name echo --serve echo
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

const CALL: u64 = 1;
const RECV: u64 = 2;
const REPLY: u64 = 3;
const MSG: u64 = 0x8000;
const OUT: u64 = 0x9000;
const REPLY_TOKEN: u64 = 0x900;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    let base = arc_base();
    let console = grants + 4;
    let endpoint = grants + 4 + 32;
    let up = b"serving, parked in recv\n";
    svc(CALL, console, up.as_ptr() as u64 - base, up.len() as u64, 0xA00, 16);
    loop {
        let n = svc(RECV, endpoint, MSG, 256, REPLY_TOKEN, 0);
        if n == u64::MAX {
            continue;
        }
        let out = (base + OUT) as *mut u8;
        let prefix = b"echo: ";
        let mut len = 0usize;
        for &c in prefix {
            unsafe { out.add(len).write_volatile(c) };
            len += 1;
        }
        for i in 0..n as usize {
            let c = unsafe { ((base + MSG) as *const u8).add(i).read_volatile() };
            unsafe { out.add(len).write_volatile(c.to_ascii_uppercase()) };
            len += 1;
        }
        svc(REPLY, REPLY_TOKEN, OUT, len as u64, 0, 0);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
