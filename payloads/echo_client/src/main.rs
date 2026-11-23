//! An IPC client process: holds the send right on one endpoint (grant slot 1), calls it once with its input (or a default), and reports through its console capability whether the echo came back whole (docs/NO-CEILINGS.md step 5's check: a 1 MiB input is one call, answered whole).
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
/// A call's reply flag: tell this caller a reply longer than its buffer (ferros_sys `CALL_LONG`, docs/NO-CEILINGS.md step 5).
const CALL_LONG: u64 = 1 << 61;
/// Room for the echo of a 1 MiB message and its prefix: the step's check (a 1 MiB call answered whole).
static mut REPLY: [u8; (1 << 20) + 64] = [0; (1 << 20) + 64];

/// `v` in decimal after "A#" into `out`; the bytes written.
fn dec(out: &mut [u8], v: u64) -> usize {
    out[..2].copy_from_slice(b"A#");
    let mut d = [0u8; 20];
    let (mut n, mut v) = (0, v);
    loop {
        d[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        out[2 + i] = d[n - 1 - i];
    }
    2 + n
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let base = arc_base();
    let off = |p: *const u8| p as u64 - base;
    let console = grants + 0x204;
    let endpoint = grants + 0x204 + 32;
    let mut r = [0u8; 16];
    let say = |t: &[u8], r: &mut [u8; 16]| svc(CALL, console, off(t.as_ptr()), t.len() as u64, off(r.as_mut_ptr()), 16);
    let default = b"hello across an endpoint";
    let (msg, len) = if input_len > 0 { (input, input_len) } else { (off(default.as_ptr()), default.len() as u64) };
    let reply = unsafe { &mut *(&raw mut REPLY) };
    let n = svc(CALL, endpoint, msg, len, off(reply.as_ptr()), reply.len() as u64 | CALL_LONG);
    let mut line = [0u8; 128];
    let mut at = 0;
    let mut put = |b: &[u8], line: &mut [u8; 128], at: &mut usize| {
        line[*at..*at + b.len()].copy_from_slice(b);
        *at += b.len();
    };
    if n == u64::MAX {
        put(b"call failed (refused: past the server's buffer, or no room in the kernel)\n", &mut line, &mut at);
    } else if n >> 62 == 1 {
        put(b"reply too long for the buffer: ", &mut line, &mut at);
        at += dec(&mut line[at..], n & !(1 << 62));
        put(b" bytes\n", &mut line, &mut at);
    } else {
        // The echo checked whole: "echo: " and the message upper-cased, every byte.
        let sent = unsafe { core::slice::from_raw_parts((base + msg) as *const u8, len as usize) };
        let got = &reply[..n as usize];
        let whole = got.len() == 6 + sent.len() && &got[..6] == b"echo: " && got[6..].iter().zip(sent).all(|(&g, &s)| g == s.to_ascii_uppercase());
        put(if whole { b"echo whole: sent " } else { b"echo WRONG: sent " }, &mut line, &mut at);
        at += dec(&mut line[at..], len);
        put(b" bytes, got ", &mut line, &mut at);
        at += dec(&mut line[at..], n);
        put(b"\n", &mut line, &mut at);
    }
    say(&line[..at], &mut r);
    unsafe { asm!("svc #0", in("x0") n, in("x8") EXIT, options(noreturn, nostack)) }
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
