//! A test client for the `gnss` endpoint (payloads/gnss): input 00 asks for the fix once and prints whether there is one, how many, its age and error estimate (the position itself stays off the console); input 01 stops the driver.
//!
//! Spawn: ferros-bridge spawn gnsstest.bin 00 --name gnsstest --use gnss
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{call, cap, console};

struct Line {
    b: [u8; 128],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let s = s.as_bytes();
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s[..k]);
        self.n += k;
        Ok(())
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut r = [0u8; 33];
    // Input byte 0: the op (0 FIX, 1 STOP).
    let op = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    let n = call(cap(grants, 1), &[op], &mut r);
    if op == 1 {
        console(grants, if r[0] == 1 { b"gnsstest: STOP answered, powered down\n" } else { b"gnsstest: STOP answered, power-down failed\n" });
        ferros_sys::exit(0);
    }
    let u = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    let mut l = Line { b: [0; 128], n: 0 };
    let _ = write!(l, "gnsstest: FIX answered {} bytes: valid {}, {} fixes, the latest {} ms old, ±{} m\n", n, r[0], u(1), u(5), u(21));
    console(grants, &l.b[..l.n]);
    ferros_sys::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
