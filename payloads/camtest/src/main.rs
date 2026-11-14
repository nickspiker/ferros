//! camtest: the camera driver's endpoint driven headless, for the measures docs/CAMERA.md asks of a run (development): LIST, OPEN, frames at one bin, CLOSE, with the time to the first frame, the frames' spacing, a frame's mean sample and its header on the console. No screen, no touch: nothing the person sees moves.
//!
//! ferros-bridge spawn camtest.bin <hex input> --name camtest --use camera   (e.g. `4008`: 64 frames at bin 8)
//! Its input, two bytes given in hex: how many frames (1 to 255; none, 16) and the bin (2, 4 or 8; 8 by default, a reply under 400 KiB).
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{call, cap, console, ERR};

struct Line {
    b: [u8; 256],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // WHY the min: a formatted line longer than the 256-byte buffer would index past it and panic the process; the console gets the line cut instead.
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

/// The reply buffer: a frame at bin 2 is 6 MiB and its header.
static mut BUF: [u8; (1 << 23) + 64] = [0; (1 << 23) + 64];

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let arg = |k: u64| if k < input_len { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k) as *const u8) } } else { 0 };
    let frames = match arg(0) {
        0 => 16,
        n => n as u32,
    };
    let bin = match arg(1) {
        2 => 2u8,
        4 => 4,
        _ => 8,
    };
    // Off the spawn first: the bridge's spawn returns once this blocks, before any call that may wait seconds on the driver (an open replays the sensor's init).
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / (1 << 4));
    let ep = cap(g, 1);
    let buf = unsafe { &mut *(&raw mut BUF) };
    let t0 = ferros_sys::now();
    let n = call(ep, &[0], &mut buf[..1 << 16]);
    say!(g, "camtest: LIST A#{} bytes in A#{} ms\n", if n == ERR { 0 } else { n }, ferros_sys::us_since(t0) / 1000);
    if n == ERR {
        ferros_sys::exit(1);
    }
    let t1 = ferros_sys::now();
    let mut r = [0u8; 4];
    let n = call(ep, &[1, 0], &mut r);
    say!(g, "camtest: OPEN answered G#{:X} in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 {
        ferros_sys::exit(1);
    }
    let (mut first_us, mut last, mut gaps, mut got) = (0u64, ferros_sys::now(), 0u64, 0u32);
    for k in 0..frames {
        let n = call(ep, &[2, bin], buf);
        let now = ferros_sys::now();
        if n == ERR || n < 64 {
            say!(g, "camtest: frame A#{k}: no frame (G#{:X})\n", n);
            break;
        }
        got += 1;
        if k == 0 {
            first_us = ferros_sys::us_since(t1);
        } else {
            gaps += (now - last) * 1_000_000 / ferros_sys::freq();
        }
        last = now;
        let h = &buf[..64];
        let (w, hh) = (u16::from_le_bytes([h[4], h[5]]) as u64, u16::from_le_bytes([h[6], h[7]]) as u64);
        if k == 0 || k + 1 == frames {
            let samples = &buf[64..n as usize];
            let mut sum = 0u64;
            for s in samples.chunks_exact(2).step_by(1 << 4) {
                sum += u16::from_le_bytes([s[0], s[1]]) as u64;
            }
            // WHY the max: a reply of the header alone has no samples, and the mean would divide by zero and panic the process.
            let mean = sum * (1 << 4) * 2 / samples.len().max(2) as u64;
            say!(g, "camtest: frame A#{k}: count A#{} A#{w} x A#{hh} bin A#{} exposure A#{} us gain A#{} focus G#{:X} at Eagle A#{} s; A#{n} bytes; mean sample A#{mean}\n", u32::from_le_bytes([h[0], h[1], h[2], h[3]]), h[8], u64::from_le_bytes(h[16..24].try_into().unwrap()) / 1000, u32::from_le_bytes(h[24..28].try_into().unwrap()), u32::from_le_bytes(h[28..32].try_into().unwrap()), u64::from_le_bytes(h[32..40].try_into().unwrap()) / 1_000_000_000);
        }
    }
    let n = call(ep, &[4], &mut r);
    say!(g, "camtest: A#{got} frames at bin A#{bin}; the first A#{} ms after the open began; then A#{} us apart; CLOSE G#{:X}\n", first_us / 1000, if got > 1 { gaps / (got - 1) as u64 } else { 0 }, if n == ERR { 0xEEE } else { r[0] as u64 });
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(payload_main);
