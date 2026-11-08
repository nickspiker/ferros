//! A test client for the `gps` endpoint (payloads/gps): input 00 asks for the fix once and prints whether there is one, how many, its age and error estimate (the position itself stays off the console); input 01 stops the driver; input 02 asks for the satellites (op 2 SATS) and prints each signal's system, number, band, elevation, azimuth, C/N0 and rates.
//!
//! Spawn: ferros-bridge spawn gpstest.bin 00 --name gpstest --use gps
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
    // Input byte 0: the op (0 FIX, 1 STOP, 2 SATS).
    let op = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    if op == 2 {
        sats(grants);
    }
    let n = call(cap(grants, 1), &[op], &mut r);
    if op == 1 {
        console(grants, if r[0] == 1 { b"gpstest: STOP answered, powered down\n" } else { b"gpstest: STOP answered, power-down failed\n" });
        ferros_sys::exit(0);
    }
    let u = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    let mut l = Line { b: [0; 128], n: 0 };
    let _ = write!(l, "gpstest: FIX answered {} bytes: valid {}, {} fixes, the latest {} ms old, ±{} m\n", n, r[0], u(1), u(5), u(21));
    console(grants, &l.b[..l.n]);
    ferros_sys::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

/// Op 2 SATS (payloads/gps main.rs's header): a line for the list, then one per signal; elevation and azimuth only, never a place.
fn sats(grants: u64) -> ! {
    const REC: usize = 25;
    let mut r = [0u8; 9 + 64 * REC];
    let n = call(cap(grants, 1), &[2], &mut r) as usize;
    let u = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    let mut l = Line { b: [0; 128], n: 0 };
    let _ = write!(l, "gpstest: SATS answered {} bytes: {} signals at fix message {}, {} ms old\n", n, r[0], u(1), u(5));
    console(grants, &l.b[..l.n]);
    if n == ferros_sys::ERR as usize || n < 9 {
        console(grants, b"gpstest: SATS: no answer\n");
        ferros_sys::exit(1);
    }
    // WHY the min: r[0] is the server's count and n what it sent (at most r.len()); a count past the records sent would print our zeroed buffer as satellites, and one past 64 would index past r (a panic).
    for k in 0..(r[0] as usize).min((n - 9) / REC) {
        let x = &r[9 + REC * k..9 + REC * (k + 1)];
        let i16_at = |o: usize| i16::from_le_bytes([x[o], x[o + 1]]);
        let i32_at = |o: usize| i32::from_le_bytes([x[o], x[o + 1], x[o + 2], x[o + 3]]);
        let sys = match x[0] { 0 => 'G', 8 => 'R', _ => '?' };
        let band = match x[1] { 0 => "L1", 2 => "L5", _ => "?" };
        let rate = |v: i32, l: &mut Line| if v == i32::MIN { let _ = write!(l, "-"); } else { let _ = write!(l, "{}", v); };
        let mut l = Line { b: [0; 128], n: 0 };
        let _ = write!(l, "gpstest:   {}{:02} {} {} el {} az {} C/N0 {}, moved el {} az {} in {} ms, mdeg/s el ", sys, x[2], band, if x[3] & 1 == 1 { "used" } else { "    " }, x[4] as i8, u16::from_le_bytes([x[5], x[6]]), u16::from_le_bytes([x[11], x[12]]) >> 2, i16_at(7), i16_at(9), u32::from_le_bytes([x[13], x[14], x[15], x[16]]));
        rate(i32_at(17), &mut l);
        let _ = write!(l, " az ");
        rate(i32_at(21), &mut l);
        let _ = write!(l, "\n");
        console(grants, &l.b[..l.n]);
    }
    ferros_sys::exit(0);
}
