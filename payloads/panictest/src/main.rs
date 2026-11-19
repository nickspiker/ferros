//! panictest: a process that dies on purpose, for the death reports (KERNEL.md "A process that dies"): what the kernel keeps of a panic or a fault, and what `ferros-bridge deaths` names from it.
//!
//! Its input's first byte says how it dies, three calls deep so the frames have something to walk:
//! - 0: a panic with a formatted message, a slice range past its end (as the display's on the 8a, 2026-11-18), its grants not given to `panic_grants`, so the kernel alone says it;
//! - 1: the same panic with its grants given, so the payload's own console line comes first;
//! - 2: a fault: a load from an address outside its arc (a data abort);
//! - 3: a panic whose message's Display panics again (the second panic goes straight to the exit).
//!
//! ferros-bridge spawn panictest.bin 00 --name panictest
#![no_std]
#![no_main]

use ferros_sys::{arc_base, console, exit};

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let how = if input_len >= 1 { unsafe { core::ptr::read_volatile((arc_base() + input) as *const u8) } } else { 0 };
    if how == 1 {
        ferros_sys::panic_grants(grants);
    }
    console(grants, b"panictest: dying on purpose\n");
    let v = core::hint::black_box([7u8; 1 << 6]);
    let r = outer(how, &v);
    console(grants, b"panictest: lived (no such way to die)\n");
    exit(r as u64)
}

#[inline(never)]
fn outer(how: u8, v: &[u8]) -> u32 {
    core::hint::black_box(middle(how, v)) + 1
}

#[inline(never)]
fn middle(how: u8, v: &[u8]) -> u32 {
    core::hint::black_box(inner(how, v)) + 1
}

/// A Display that panics: the second panic, inside the first's formatting.
struct Twice;
impl core::fmt::Display for Twice {
    fn fmt(&self, _: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        panic!("the second panic, from the first's message")
    }
}

#[inline(never)]
fn inner(how: u8, v: &[u8]) -> u32 {
    match how {
        0 | 1 => {
            // Past the slice's end by a number only known at run time.
            let end = core::hint::black_box(v.len() * 6409);
            v[..end].iter().map(|&x| x as u32).sum()
        }
        2 => {
            // An address far outside the arc: a translation fault at EL0.
            let at = core::hint::black_box(16usize);
            unsafe { core::ptr::read_volatile(at as *const u32) }
        }
        3 => panic!("the first panic: {}", Twice),
        _ => 0,
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
