//! camtest: the camera driver's endpoint driven headless, for the measures docs/CAMERA.md asks of a run (development): LIST, OPEN, frames at one bin, CLOSE, with the time to the first frame, the frames' spacing, a frame's mean sample and its header on the console. No screen, no touch: nothing the person sees moves.
//!
//! ferros-bridge spawn camtest.bin <hex input> --name camtest --use camera   (e.g. `4008`: 64 frames at bin 8)
//! Its input, bytes given in hex: how many frames (1 to 255; none, 16), the bin (2, 4 or 8; 8 by default, a reply under 400 KiB), optionally a multiplier of the frames (`FF0205`: 1275 frames, past the descriptor ring's 1024), and optionally 1 to drive the controls mid-stream (`60080001`: 96 frames at bin 8, exposure, gain and focus changed).
//! Bin 1 is the view (docs/CAMERA.md "The view"): whole frames read in place, held one at a time (`4001`: 64 frames); a fourth byte of 2 also stores into the view at the end, which must fault (`40010002`).
//! A fifth byte names the camera (ferros_cam cams: 0 the main, the default; 1 the ultrawide; 2 the front; 3 the tele): `4001000001` the ultrawide's view, `4008000001` its frames at bin 8. A fourth byte of 7 PROBEs that camera instead and ends (`0000000702`: the front's identity and which addresses answer on its bus; docs/CAMERA.md "Device test plan").
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
        n => n as u32 * if arg(2) == 0 { 1 } else { arg(2) as u32 },
    };
    let bin = match arg(1) {
        1 => 1u8,
        2 => 2,
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
    // The fifth input byte: which camera (0 the main, 1 the ultrawide, 2 the front, 3 the tele; ferros_cam cams).
    let id = arg(4);
    // A fourth byte of 7: PROBE that camera instead (power it, read its identity, map which addresses answer on its bus, power it down), and end.
    if arg(3) == 7 {
        let mut p = [0u8; 20];
        let n = call(ep, &[7, id], &mut p);
        say!(g, "camtest: PROBE of camera A#{id} answered A#{} in A#{} ms: identity G#{:02X}{:02X}, error G#{:X}\n", if n == ERR { 0xEEE } else { p[0] as u64 }, ferros_sys::us_since(t1) / 1000, p[1], p[2], p[3]);
        for a in 0x08u8..0x78 {
            if p[4 + (a >> 3) as usize] >> (a & 7) & 1 == 1 {
                say!(g, "camtest: G#{a:02X} acknowledged on its bus\n");
            }
        }
        ferros_sys::exit(if n == ERR || p[0] != 0 { 1 } else { 0 });
    }
    if bin == 1 {
        view(g, ep, id, frames, arg(3) == 2, t1);
    }
    let mut r = [0u8; 4];
    let n = call(ep, &[1, id], &mut r);
    say!(g, "camtest: OPEN answered G#{:X} in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 {
        ferros_sys::exit(1);
    }
    // With a fourth input byte of 1, the controls mid-stream (SET, docs/CAMERA.md "The endpoint"), each at its frame, the frames around each change said with their means: the exposure to a quarter of its longest at frame 16, the gain to twice unity at 32 (outside the captured code's range: refused) and back to unity, the focus to G#2000 at 48, the exposure back to its longest at 64.
    let controls = arg(3) == 1;
    let set = |what: u8, value: u64| -> u8 {
        let mut m = [3u8, what, 0, 0, 0, 0, 0, 0, 0, 0];
        m[2..10].copy_from_slice(&value.to_le_bytes());
        let mut r = [0xEEu8; 1];
        if call(ep, &m, &mut r) == ERR { 0xEE } else { r[0] }
    };
    let changes: [(u32, u8, u64); 5] = [(16, 0, 17_200_000), (32, 1, 2048), (32, 1, 1024), (48, 2, 0x2000), (64, 0, 68_800_000)];
    let near = |k: u32| controls && changes.iter().any(|c| k + 1 >= c.0 && k <= c.0 + 4);
    let (mut first_us, mut last, mut gaps, mut got) = (0u64, ferros_sys::now(), 0u64, 0u32);
    for k in 0..frames {
        for &(_, what, value) in changes.iter().filter(|c| controls && c.0 == k) {
            say!(g, "camtest: before frame A#{k}: SET A#{what} to A#{value}: answered A#{}\n", set(what, value));
        }
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
        if k == 0 || k + 1 == frames || near(k) {
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

/// The view's token, where the kernel looks for it (an arc offset).
static mut VIEW_TOKEN: [u8; 32] = [0; 32];

/// Bin 1 (docs/CAMERA.md "The view"): OPEN with the view, `frames` MAPs, each frame read in place thru the view, CLOSE, then the token tried once more. Said on the console: the view's base and shape, each MAP's wait, the first and last frame's header and mean (a sample's high eight bits every 52 samples of every 16th line), a whole frame copied out with 64-byte loads (what a kist pays to take one), and whether the token still answers after the close (it must not). With `write` set, a store into the view after the last frame: the view is read-only, so camtest ends on that fault, which is the test.
fn view(g: u64, ep: u64, id: u8, frames: u32, write: bool, t1: u64) -> ! {
    let mut r = [0u8; 64];
    let n = call(ep, &[1, id, 1], &mut r);
    say!(g, "camtest: OPEN with the view answered G#{:X}, A#{} bytes, in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, if n == ERR { 0 } else { n }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 || n < 48 {
        ferros_sys::exit(1);
    }
    let tok = unsafe { &mut *(&raw mut VIEW_TOKEN) };
    tok.copy_from_slice(&r[1..33]);
    let toff = ferros_sys::off(tok.as_ptr());
    let base = ferros_sys::dma(toff);
    let size = ferros_sys::svc(ferros_sys::nr::DMA, toff, 1, 0, 0, 0);
    let (ring, w, h) = (r[33], u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    say!(g, "camtest: the view at G#{base:X}, A#{} MiB; a ring of A#{ring}, A#{w} x A#{h}, A#{stride} bytes a line, format A#{}\n", if size == ERR { 0 } else { size >> 20 }, r[34]);
    if base == ERR {
        ferros_sys::exit(1);
    }
    let (mut held, mut waits, mut most, mut got, mut last_count) = (0xFFu8, 0u64, 0u64, 0u32, 0u32);
    let mut h64 = [0u8; 64];
    for k in 0..frames {
        let t = ferros_sys::now();
        let n = call(ep, &[5, held], &mut h64);
        let us = ferros_sys::us_since(t);
        if n == ERR || n < 64 {
            say!(g, "camtest: MAP A#{k}: no frame (G#{:X})\n", n);
            break;
        }
        got += 1;
        if k != 0 {
            waits += us;
            most = most.max(us);
        }
        held = h64[40];
        let count = u32::from_le_bytes([h64[0], h64[1], h64[2], h64[3]]);
        let skipped = count.wrapping_sub(last_count).wrapping_sub(1);
        last_count = count;
        if k == 0 || k + 1 == frames {
            let at = base as usize + u64::from_le_bytes(h64[48..56].try_into().unwrap()) as usize;
            let (mut sum, mut cnt) = (0u64, 0u64);
            for y in (0..h).step_by(1 << 4) {
                // Every 65th byte of the line's samples: a multiple of five, so always a sample's high eight bits (MIPI RAW10: four of those, then their low bits).
                for x in (0..w * 5 / 4).step_by(65) {
                    sum += unsafe { core::ptr::read_volatile((at + y * stride + x) as *const u8) } as u64;
                    cnt += 1;
                }
            }
            // A whole frame out of the view, 64 bytes a load, as a kist takes one (into a scratch line, so nothing but the reads is timed).
            let t = ferros_sys::now();
            let mut acc = 0u64;
            for y in 0..h {
                let line = (at + y * stride) as *const u8;
                for i in 0..stride / 64 {
                    let v = unsafe { core::arch::aarch64::vld1q_u8_x4(line.add(i * 64)) };
                    acc ^= unsafe { core::arch::aarch64::vgetq_lane_u64::<0>(core::arch::aarch64::vreinterpretq_u64_u8(v.0)) };
                }
            }
            let copy_us = ferros_sys::us_since(t);
            say!(g, "camtest: MAP A#{k}: buffer A#{} count A#{count} (A#{skipped} skipped) exposure A#{} us at Eagle A#{} s, answered in A#{us} us; mean sample about A#{} of A#1023; a frame read in A#{copy_us} us (G#{:X})\n", h64[40], u64::from_le_bytes(h64[16..24].try_into().unwrap()) / 1000, u64::from_le_bytes(h64[32..40].try_into().unwrap()) / 1_000_000_000, 4 * sum / cnt.max(1), acc & 0xFF);
        }
    }
    say!(g, "camtest: A#{got} frames mapped, A#{} us apart on average, A#{most} us the longest\n", if got > 1 { waits / (got - 1) as u64 } else { 0 });
    if write {
        say!(g, "camtest: a store into the view (read-only: this process should end on the fault)\n");
        unsafe { core::ptr::write_volatile(base as *mut u64, 0) };
        say!(g, "camtest: the store into the view went thru: the view is NOT read-only\n");
    }
    let mut c = [0u8; 4];
    let n = call(ep, &[4], &mut c);
    let after = ferros_sys::dma(toff);
    say!(g, "camtest: CLOSE G#{:X}; the view's token after it: {}\n", if n == ERR { 0xEEE } else { c[0] as u64 }, if after == ERR { "refused (taken back)" } else { "STILL ANSWERS" });
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(payload_main);
