//! A test client for the `gps` endpoint (payloads/gps): input 00 asks for the fix once and prints whether there is one, how many, its age and error estimate (the position itself stays off the console); input 01 stops the driver; input 02 asks for the satellites (op 2 SATS) and prints each signal's system, number, band, elevation, azimuth, C/N0 and rates; input 04 prints the kinds of frame the chip has sent (op 4 FRAMES), and 04 with a kind's index (0402) that kind's latest frames in hex.
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
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut r = [0u8; 33];
    // Input byte 0: the op (0 FIX, 1 STOP, 2 SATS).
    let op = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    if op == 2 {
        sats(grants);
    }
    if op == 4 {
        // Input byte 1, when given: the kind whose latest frames to print.
        let k = if input_len > 1 { Some(unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 1) as *const u8) }) } else { None };
        frames(grants, k);
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

/// Op 4 FRAMES (payloads/gps main.rs's header, GPS.md "What the chip sends before a fix"): with no kind, the table of kinds (a line each: its index, key, count, bytes, age); with kind `k`, its latest frames in hex, 32 bytes a line.
/// The bytes are the chip's: a kind other than the fix message could still echo the aiding's place, so a capture of them stays on Harbor, never in the repo.
fn frames(grants: u64, k: Option<u8>) -> ! {
    let mut r = [0u8; 1 << 10];
    let n = match k {
        None => call(cap(grants, 1), &[4], &mut r),
        Some(k) => call(cap(grants, 1), &[4, k], &mut r),
    };
    if n == ferros_sys::ERR {
        console(grants, b"gpstest: FRAMES: no answer\n");
        ferros_sys::exit(1);
    }
    let n = n as usize;
    let u = |o: usize| u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
    let key = |l: &mut Line, x: &[u8]| {
        let _ = write!(l, "{:02x}", x[0]);
        if x[1] != 0 {
            let _ = write!(l, ".{:02x}", x[1]);
        }
        let _ = write!(l, "/{:02x}{:02x}/{:02x}{:02x}", x[2], x[3], x[4], x[5]);
    };
    let Some(k) = k else {
        if n < 9 {
            console(grants, b"gpstest: FRAMES: too short\n");
            ferros_sys::exit(1);
        }
        let mut l = Line { b: [0; 128], n: 0 };
        let _ = write!(l, "gpstest: FRAMES: {} kinds, {} frames, {} CRC failures\n", r[0], u(1), u(5));
        console(grants, &l.b[..l.n]);
        // WHY the min: r[0] is the driver's count and n what it sent; a count past the records sent would print the zeroed buffer.
        for i in 0..(r[0] as usize).min((n - 9) / 18) {
            let o = 9 + 18 * i;
            let mut l = Line { b: [0; 128], n: 0 };
            let _ = write!(l, "gpstest:   {:02} ", i);
            key(&mut l, &r[o..o + 6]);
            let _ = write!(l, " x{} {} B, the latest {} ms ago\n", u(o + 6), u(o + 10), u(o + 14));
            console(grants, &l.b[..l.n]);
        }
        ferros_sys::exit(0);
    };
    if n < 7 {
        console(grants, b"gpstest: FRAMES: no such kind\n");
        ferros_sys::exit(1);
    }
    let mut l = Line { b: [0; 128], n: 0 };
    let _ = write!(l, "gpstest: FRAMES kind {:02} ", k);
    key(&mut l, &r[..6]);
    let _ = write!(l, ": {} kept\n", r[6]);
    console(grants, &l.b[..l.n]);
    let mut o = 7;
    for _ in 0..r[6] {
        // WHY the checks: the lengths are the driver's; a record past what it sent ends the print rather than reading the zeroed buffer.
        if o + 4 > n {
            break;
        }
        let (len, kept) = (u16::from_le_bytes([r[o], r[o + 1]]), u16::from_le_bytes([r[o + 2], r[o + 3]]) as usize);
        o += 4;
        if o + kept > n {
            break;
        }
        let mut l = Line { b: [0; 128], n: 0 };
        let _ = write!(l, "gpstest:   frame of {} B, {} kept:\n", len, kept);
        console(grants, &l.b[..l.n]);
        for c in r[o..o + kept].chunks(1 << 5) {
            let mut l = Line { b: [0; 128], n: 0 };
            let _ = write!(l, "gpstest:     ");
            for b in c {
                let _ = write!(l, "{:02x}", b);
            }
            let _ = write!(l, "\n");
            console(grants, &l.b[..l.n]);
        }
        o += kept;
    }
    ferros_sys::exit(0);
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
