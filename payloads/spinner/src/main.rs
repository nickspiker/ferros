//! A process that never blocks: it counts, reports through its console every 2^26 iterations, and exits after five reports. It exists to prove time slices — the kernel must get the CPU back (USB keeps answering) while it spins, and it must keep making progress in later slices.
//!
//! Spawn: ferros-bridge spawn spinner.bin --name spin   then   ferros-bridge diag console

#![no_std]
#![no_main]

use core::arch::asm;

fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    let base: u64;
    unsafe { asm!("mov {}, x28", out(reg) base) };
    let console = grants + 4;
    let mut line = *b"tick 0\n";
    let mut count: u64 = 0;
    let mut ticks = 0u8;
    loop {
        count = count.wrapping_add(1);
        core::hint::black_box(count);
        if count & ((1 << 26) - 1) == 0 {
            ticks += 1;
            line[5] = b'0' + ticks;
            svc(1, console, line.as_ptr() as u64 - base, line.len() as u64, 0xA00, 16);
            if ticks == 5 {
                unsafe { asm!("svc #0", in("x0") 0u64, in("x8") 0u64, options(noreturn, nostack)) };
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
