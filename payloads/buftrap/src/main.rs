//! buftrap: the syscall buffer rule's test (owner, 2026-11-08: a buffer resolves against the arc, then the process's own DMA regions; anything else traps). It asks the console to print one byte from a buffer past its arc: a development kernel parks this thread with a report (process, syscall, address, nearest region, PC and LR); a production one kills the process. With input byte 1, the buffer is the first byte past the arc; with 0, a line in the arc first, to show the ordinary path works.
//!
//! ferros-bridge spawn buftrap.bin 01 --name buftrap
#![no_std]
#![no_main]

use ferros_sys::{arc_base, cap, console, exit};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let past = input_len >= 1 && unsafe { core::ptr::read_volatile((arc_base() + input) as *const u8) } == 1;
    console(grants, b"buftrap: a line from the arc\n");
    if past {
        // The arc's size from its mask (x27); one byte past its end, an offset the kernel used to wrap to the arc's first byte.
        let mask: u64;
        unsafe { core::arch::asm!("mov {}, x27", out(reg) mask) };
        let mut r = [0u8; 4];
        ferros_sys::call(cap(grants, 0), unsafe { core::slice::from_raw_parts((arc_base() + mask + 1) as *const u8, 1) }, &mut r);
        console(grants, b"buftrap: the call past the arc returned (the rule is not enforced)\n");
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
