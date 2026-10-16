//! A masked payload that tries to break the sandbox (SANDBOX.md), to prove W^X stops it rather than the process escaping. The checker accepts every instruction here (an in-arc store and an indirect call are both allowed forms); W^X is what must make them fault at run time.
//!
//! Input byte selects the attempt: 0 = write to its own code page (must fault, code is read-only), 1 = call into its stack (must fault, data is non-executable). Either way the process should end with a FAULT exit (G#FA… high byte) and the kernel carries on — a successful return would mean the sandbox failed.
//!
//! Spawn masked+sealed: it never reaches "survived" on a correct kernel.

#![no_std]
#![no_main]

use ferros_sys::{arc_base, console, input};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _in: u64, _inlen: u64) -> ! {
    let base = arc_base();
    let mut sel = [0u8; 1];
    input(0, &mut sel);
    console(grants, b"wxtest: attempting a sandbox escape\n");
    match sel[0] {
        1 => {
            // Call into the stack: a buffer of RET bytes, jumped to as code. Data pages are non-executable, so this must fault.
            let mut buf = [0xC0u8, 0x03, 0x5F, 0xD6]; // ret
            let f: extern "C" fn() = unsafe { core::mem::transmute(buf.as_mut_ptr()) };
            f();
        }
        _ => {
            // Store over the first instruction of this code (arc offset 0x1000). Code pages are read-only, so this must fault.
            unsafe { core::ptr::write_volatile((base + 0x1000) as *mut u32, 0) };
        }
    }
    // Only reached if the sandbox FAILED to stop the attempt.
    console(grants, b"wxtest: SURVIVED - sandbox did not stop it\n");
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
