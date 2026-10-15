//! ferros's first process: runs at EL0 in its own arc and can say something only by calling the one capability it was granted.
//!
//! Build:
//!   cargo build -p ferros-payload-hello-el0 --target aarch64-unknown-none --release
//!   aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/hello_el0 hello_el0.bin
//! Spawn:
//!   ferros-bridge spawn hello_el0.bin [hex-input]
//! Input "kernel" (6b65726e656c) makes it try to read kernel memory and "mmio" (6d6d696f) a device register: the process must end with a fault, the kernel must not notice.
//!
//! Process ABI (ferros_kernel process.rs): entered at offset 0 with x0 = grants offset (u32 count, then 32-byte tokens; slot 0 = console), x1 = input offset, x2 = input length, x28 = arc base, x27 = arc mask. Syscalls: svc #0, x8 = 0 exit(x0) / 1 call(x0 cap, x1 msg, x2 len, x3 reply, x4 reply cap) — all offsets are arc offsets.

#![no_std]
#![no_main]

use core::arch::asm;

fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b) };
    b
}

fn call(cap: u64, msg: u64, len: u64, reply: u64, reply_cap: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") cap => r, in("x1") msg, in("x2") len, in("x3") reply, in("x4") reply_cap, in("x8") 1u64, options(nostack)) };
    r
}

fn exit(code: u64) -> ! {
    unsafe { asm!("svc #0", in("x0") code, in("x8") 0u64, options(noreturn, nostack)) }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    let base = arc_base();
    let console = grants + 4;
    let reply = 0x800; // tail of the grants page
    let say = |m: &[u8]| call(console, m.as_ptr() as u64 - base, m.len() as u64, reply, 16);

    let r = say(b"hello from ferros's first process: EL0, one arc, one capability\n");
    if input_len > 0 {
        say(b"input: ");
        call(console, input, input_len, reply, 16);
        say(b"\n");
        let first = unsafe { core::ptr::read_volatile((base + input) as *const u8) };
        if first == b'k' {
            // Kernel memory: EL2-only in the static map, so this must fault and end the process.
            unsafe { core::ptr::read_volatile(0x8008_0000 as *const u64) };
        } else if first == b'm' {
            // A device register (the GIC distributor): also EL2-only.
            unsafe { core::ptr::read_volatile(0x1040_0000 as *const u32) };
        }
    }
    exit(r)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    exit(0xDEAD)
}
