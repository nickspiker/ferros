//! An IPC server process: holds the receive right on one endpoint (grant slot 1) and answers every call with "echo: " + the message upper-cased, a message of up to 1 MiB (docs/NO-CEILINGS.md step 5's check: a call carries its whole message). It never exits; between calls it is parked in recv.
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
/// A receive's flag: tell this server a message longer than its buffer (ferros_sys `RECV_LONG`); the answer is then `TOO_LONG` and the length (docs/NO-CEILINGS.md step 5).
const RECV_LONG: u64 = 1 << 1;
/// The largest message it echoes: 1 MiB, the step's check (a 1 MiB call answered whole). A longer one is refused by the kernel (its caller answered ERR), never cut.
const MOST: usize = 1 << 20;
const PREFIX: &[u8] = b"echo: ";

static mut MSG: [u8; MOST] = [0; MOST];
static mut OUT: [u8; 6 + MOST] = [0; 6 + MOST];
static mut TOKEN: [u8; 32] = [0; 32];

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let base = arc_base();
    let off = |p: *const u8| p as u64 - base;
    let console = grants + 0x204;
    let endpoint = grants + 0x204 + 32;
    let up = b"serving, parked in recv\n";
    let mut r = [0u8; 16];
    svc(CALL, console, off(up.as_ptr()), up.len() as u64, off(r.as_mut_ptr()), 16);
    let (msg, out, token) = unsafe { (&mut *(&raw mut MSG), &mut *(&raw mut OUT), &mut *(&raw mut TOKEN)) };
    out[..PREFIX.len()].copy_from_slice(PREFIX);
    // Told "too long" (top bits 01): the next receive goes without the flag, so the kernel refuses what does not fit (its caller answered ERR) and hands the next message that does.
    let mut flags = RECV_LONG;
    loop {
        let n = svc(RECV, endpoint, off(msg.as_ptr()), msg.len() as u64, off(token.as_ptr()), flags);
        if n >> 62 == 1 {
            flags = 0;
            continue;
        }
        flags = RECV_LONG;
        if n == u64::MAX {
            continue;
        }
        let n = n as usize;
        for (o, &c) in out[PREFIX.len()..PREFIX.len() + n].iter_mut().zip(&msg[..n]) {
            *o = c.to_ascii_uppercase();
        }
        svc(REPLY, off(token.as_ptr()), off(out.as_ptr()), (PREFIX.len() + n) as u64, 0, 0);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

// The entry and its image header (as ferros_sys::entry!, which this payload does not link): "ferrosRT" at offset 8, then .bss + 16, so the kernel sizes the arc from the image and its .bss.
core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym payload_main
);
