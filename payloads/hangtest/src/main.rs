//! Watchdog recovery test — deliberately hangs forever.
//!
//! Running this via `bridge run` makes the kernel call into a payload that never returns, so the main loop stops petting the recoverable cluster watchdog. If the watchdog works, the phone auto-resets in ~87s and re-enumerates. If it does not, the phone freezes (needs a physical power-cycle) — i.e. this is exactly as risky as any real hang, no more.
//!
//! Expected: no output (never returns); phone drops off USB, then re-enumerates ~87s later as a fresh boot.

#![no_std]
#![no_main]

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, _out: *mut u8, _out_cap: usize) -> u64 {
    loop { core::hint::spin_loop(); }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
