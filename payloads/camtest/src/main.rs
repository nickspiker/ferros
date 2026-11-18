//! camtest: the camera driver's endpoint driven headless, for the measures docs/CAMERA.md asks of a run (development): LIST, OPEN, frames at one bin, CLOSE, with the time to the first frame, the frames' spacing, a frame's mean sample and its header on the console. No screen, no touch: nothing the person sees moves.
//!
//! ferros-bridge spawn camtest.bin <hex input> --name camtest --use camera   (e.g. `4008`: 64 frames at bin 8)
//! Its input, bytes given in hex: how many frames (1 to 255; none, 16), the bin (2, 4 or 8; 8 by default, a reply under 400 KiB), optionally a multiplier of the frames (`FF0205`: 1275 frames, past the descriptor ring's 1024), and optionally 1 to drive the controls mid-stream (`60080001`: 96 frames at bin 8, exposure, gain and focus changed).
//! Bin 1 is the view (docs/CAMERA.md "The view"): whole frames read in place, held one at a time (`4001`: 64 frames); a fourth byte of 2 also stores into the view at the end, which must fault (`40010002`).
//! A fifth byte names the camera (ferros_cam cams: 0 the main, the default; 1 the ultrawide; 2 the front; 3 the tele): `4001000001` the ultrawide's view, `4008000001` its frames at bin 8. A fourth byte of 7 PROBEs that camera instead and ends (`0000000702`: the front's identity and which addresses answer on its bus; docs/CAMERA.md "Device test plan").
//! A fourth byte of 4 measures where the analogue gain ends (2026-11-17): raw frames thru the view at each step 1x to 160x, the temporal noise and its input-referred value, the codes' histogram near the mean (`0400000400` the main at the init's exposure; `040000040007` at 128 us, dark).
//! A fourth byte of 3 measures the gain's curve (2026-11-17): the first byte frames per step (16 if 0), the sixth the exposure as 2^n us (the init's if 0), each step 1x to 256x a half stop apart with the mean sample and the header's gain (`1008000300` the main at bin 8; `100800030010` at 2^16 us, about 66 ms; `10080003010C` the ultrawide at 4 ms).
//! A fourth byte of 5 walks the exposure up past the init's frame (2026-11-17, long exposures, docs/CAMERA.md "Long exposures"): 2^m ms (the seventh byte; 6 if 0), doubling to 2^n ms (the sixth byte; 11, about 2 s, if 0), then back to 2^6 ms, the first byte frames at each step (4 if 0), each step's SET answer, the frames' spacing and mean sample, the header's exposure and the sensor's own frame length, line length and integration read back (CAPS) (`0408000500` the main; `04080005010B` the ultrawide).
//! A fourth byte of 6 keeps images with their phase-detect rows as pairs (2026-11-17, docs/CAMERA.md "Phase detect as data"): the init's mode, the first byte frames to settle (16 if 0), the sixth the lens's target's high byte (G#38 if 0; FF a focus rack, G#1000 to G#7000, seven pairs), the seventh the hold of each as about 2^n s (7 if 0) while the host reads it by address (`1000000600` the main: one pair at about a metre; `100000060000FF06` the rack, each held about 64 s).
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
    // The ladder (fourth byte 5) opens first: a driver spawned by hand ends after a LIST that leaves nothing open, and the OPEN behind it would then start the vault's driver, not the one under test (2026-11-17, b932: the first ladder ran on the stored driver).
    let n = if arg(3) == 5 { 0 } else { call(ep, &[0], &mut buf[..1 << 16]) };
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
    // A fourth byte of 4: where the analogue gain really ends (owner, 2026-11-17: "Was that a typo?" of 160x), from raw samples thru the view.
    if arg(3) == 4 {
        gain_noise(g, ep, id, arg(0), arg(5), t1);
    }
    if bin == 1 {
        view(g, ep, id, frames, arg(3) == 2, t1);
    }
    // A fourth byte of 3: the gain's curve (owner, 2026-11-17: "the ISO doesn't do anything in lumis").
    if arg(3) == 3 {
        gain_curve(g, ep, id, bin, arg(0), arg(5), t1);
    }
    // A fourth byte of 5: the exposure walked past the init's frame (owner, 2026-11-17: Lumis to expose as long as on Android).
    if arg(3) == 5 {
        long_ladder(g, ep, id, bin, arg(0), arg(5), arg(6), t1);
    }
    // A fourth byte of 6: an image and its phase-detect rows kept as a pair (owner, 2026-11-17: phase detect as data).
    if arg(3) == 6 {
        snap_pair(g, ep, id, arg(0), arg(5), arg(6), t1);
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

/// The exposure past the init's frame, step by step: OPEN, then 2^6 ms doubling to 2^`top_log2` ms (11 if 0) and back to 2^6 ms, each a SET and `per` frames (4 if 0) after two let pass (the hold lands at a frame's start, and the frame begun before it may still come), with:
/// - the SET's answer (0 taken, 1 refused: past the list's longest);
/// - the frames' spacing (us), which should follow the frame: the init's 68.9 ms until the exposure passes it, then the exposure and the sensor's margin;
/// - the mean sample (every 16th binned sample, in 16ths), which should double with each step until it saturates;
/// - the header's exposure, and the sensor's frame length, line length and integration read back (CAPS: G#0340, G#0342, G#0202), which should be what the driver's console line said it wrote.
/// A step whose frames stop ends the run with the driver's own "no frame" line on the console; the close powers the module down, so the next open is a cold sensor (docs/CAMERA.md "Long exposures").
fn long_ladder(g: u64, ep: u64, id: u8, bin: u8, per: u8, top_log2: u8, bottom_log2: u8, t1: u64) -> ! {
    let buf = unsafe { &mut *(&raw mut BUF) };
    let mut r = [0u8; 4];
    let n = call(ep, &[1, id], &mut r);
    say!(g, "camtest: long exposures: OPEN answered G#{:X} in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 {
        ferros_sys::exit(1);
    }
    let set = |value: u64| -> u8 {
        let mut m = [3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        m[2..10].copy_from_slice(&value.to_le_bytes());
        let mut r = [0xEEu8; 1];
        if call(ep, &m, &mut r) == ERR { 0xEE } else { r[0] }
    };
    let per = if per == 0 { 4 } else { per as u32 };
    let top = if top_log2 == 0 { 11 } else { top_log2 };
    // A frame's mean sample (every 16th binned sample, in 16ths) and the header's exposure (ns); None when no frame came.
    let mut frame = || -> Option<(u64, u64)> {
        let n = call(ep, &[2, bin], buf);
        if n == ERR || n < 64 {
            return None;
        }
        let samples = &buf[64..n as usize];
        let (mut sum, mut cnt) = (0u64, 0u64);
        for s in samples.chunks_exact(2).step_by(1 << 4) {
            sum += u16::from_le_bytes([s[0], s[1]]) as u64;
            cnt += 1;
        }
        // WHY the max: a reply of the header alone has no samples, and the mean would divide by zero and panic the process.
        Some((sum * 16 / cnt.max(1), u64::from_le_bytes(buf[16..24].try_into().unwrap())))
    };
    let mut steps = [0u8; 1 << 5];
    let mut k = 0;
    // The first step, 2^`bottom_log2` ms (6 if 0): a ladder of long steps need not walk up from 64 ms.
    let bottom = if bottom_log2 == 0 { 6 } else { bottom_log2 };
    for e in bottom..=top {
        steps[k] = e;
        k += 1;
    }
    steps[k] = 6;
    k += 1;
    for &e in steps[..k].iter() {
        let ms = 1u64 << e;
        let a = set(ms * 1_000_000);
        if a != 0 {
            say!(g, "camtest: exposure A#{ms} ms: refused (A#{a})\n");
            continue;
        }
        for _ in 0..2 {
            if frame().is_none() {
                say!(g, "camtest: exposure A#{ms} ms: no frame after the SET\n");
                ferros_sys::exit(1);
            }
        }
        let (mut sum, mut hdr, mut gaps) = (0u64, 0u64, 0u64);
        let mut last = ferros_sys::now();
        for _ in 0..per {
            let Some((m, x)) = frame() else {
                say!(g, "camtest: exposure A#{ms} ms: the frames stopped\n");
                ferros_sys::exit(1);
            };
            let now = ferros_sys::now();
            gaps += (now - last) * 1_000_000 / ferros_sys::freq();
            last = now;
            sum += m;
            hdr = x;
        }
        let mut c = [0u8; 103];
        let cn = call(ep, &[11], &mut c);
        let be = |k: usize| u16::from_be_bytes([c[k], c[k + 1]]);
        say!(g, "camtest: exposure A#{ms} ms: header A#{} us; frames A#{} us apart; mean sample A#{} sixteenths; the sensor reads frame A#{} lines of A#{} clocks, integration A#{}{}\n", hdr / 1000, gaps / per as u64, sum / per as u64, be(38), be(40), be(67), if cn == ERR { " (CAPS failed)" } else { "" });
    }
    let n = call(ep, &[4], &mut r);
    say!(g, "camtest: long exposures done; CLOSE G#{:X}\n", if n == ERR { 0xEEE } else { r[0] as u64 });
    ferros_sys::exit(0)
}

/// Rows of the frame's middle the noise test reads (one green-red row pair at a time is not split: an even count), and the most samples a row holds.
const NOISE_ROWS: usize = 1 << 6;
const ROW_MOST: usize = 1 << 13;
static mut NOISE_A: [u16; NOISE_ROWS * ROW_MOST] = [0; NOISE_ROWS * ROW_MOST];
static mut NOISE_B: [u16; NOISE_ROWS * ROW_MOST] = [0; NOISE_ROWS * ROW_MOST];
static mut HIST: [u32; 1 << 10] = [0; 1 << 10];
static mut NOISE_LINE: [u8; (1 << 14) + 64] = [0; (1 << 14) + 64];

/// `NOISE_ROWS` rows from the frame's middle out of the view (64-byte loads, then the plain RAW10 unpack) into `dst`, `w` samples a row.
fn noise_rows(at: usize, h: usize, w: usize, stride: usize, dst: &mut [u16]) {
    let line = unsafe { &mut *(&raw mut NOISE_LINE) };
    for r in 0..NOISE_ROWS {
        let src = at + (h / 2 - NOISE_ROWS / 2 + r) * stride;
        for i in 0..stride / 64 {
            unsafe {
                let v = core::arch::aarch64::vld1q_u8_x4((src + i * 64) as *const u8);
                core::arch::aarch64::vst1q_u8_x4(line.as_mut_ptr().add(i * 64), v);
            }
        }
        for q in 0..w / 4 {
            let b = &line[q * 5..q * 5 + 5];
            for j in 0..4 {
                dst[r * w + q * 4 + j] = (b[j] as u16) << 2 | (b[4] as u16 >> (2 * j)) & 3;
            }
        }
    }
}

/// The integer square root (Newton's, from above).
fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// Where the analogue gain really ends: OPEN with the view, the exposure set to 2^`exp_log2` us (the init's when 0; a short one gives dark frames), then each gain step 1x to 160x a half stop apart, `pairs` pairs of consecutive frames (4 when 0) of the frame's middle `NOISE_ROWS` rows, raw:
/// - the temporal noise, from each pair's difference (its variance halved), in 16ths of a code, and the same referred to the input (times 1024 over the gain): falling while the gain is analogue (the read noise after the gain shrinks against it), flat once the gain is applied after the ADC;
/// - the codes' histogram near the mean (within 4 sigma): how many of those codes hold samples, and the samples' residues mod 2 and mod 4 (in 256ths): a gain after the ADC leaves only every 2nd or 4th code filled.
fn gain_noise(g: u64, ep: u64, id: u8, pairs: u8, exp_log2: u8, t1: u64) -> ! {
    let mut r = [0u8; 64];
    let n = call(ep, &[1, id, 1], &mut r);
    say!(g, "camtest: gain noise: OPEN with the view answered G#{:X} in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 || n < 48 {
        ferros_sys::exit(1);
    }
    let tok = unsafe { &mut *(&raw mut VIEW_TOKEN) };
    tok.copy_from_slice(&r[1..33]);
    let base = ferros_sys::dma(ferros_sys::off(tok.as_ptr()));
    if base == ERR {
        ferros_sys::exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[36], r[37]]) as usize, u16::from_le_bytes([r[38], r[39]]) as usize);
    let stride = u32::from_le_bytes(r[40..44].try_into().unwrap()) as usize;
    let set = |what: u8, value: u64| -> u8 {
        let mut m = [3u8, what, 0, 0, 0, 0, 0, 0, 0, 0];
        m[2..10].copy_from_slice(&value.to_le_bytes());
        let mut r = [0xEEu8; 1];
        if call(ep, &m, &mut r) == ERR { 0xEE } else { r[0] }
    };
    if exp_log2 != 0 {
        say!(g, "camtest: exposure A#{} us: answered A#{}\n", 1u64 << exp_log2, set(0, (1u64 << exp_log2) * 1000));
    }
    let pairs = if pairs == 0 { 4 } else { pairs as u32 };
    let mut held = 0xFFu8;
    let mut h64 = [0u8; 64];
    // The next frame thru the view: its samples' place and the gain its header carries.
    let mut map = |held: &mut u8| -> Option<(usize, u32)> {
        let n = call(ep, &[5, *held], &mut h64);
        if n == ERR || n < 64 {
            return None;
        }
        *held = h64[40];
        Some((base as usize + u64::from_le_bytes(h64[48..56].try_into().unwrap()) as usize, u32::from_le_bytes([h64[24], h64[25], h64[26], h64[27]])))
    };
    let steps: [u64; 16] = [1024, 1448, 2048, 2896, 4096, 5793, 8192, 11585, 16384, 23170, 32768, 46341, 65536, 92682, 131072, 163840];
    let (a, b, hist) = unsafe { (&mut *(&raw mut NOISE_A), &mut *(&raw mut NOISE_B), &mut *(&raw mut HIST)) };
    let len = NOISE_ROWS * w;
    for &want in steps.iter() {
        let ans = set(1, want);
        if ans != 0 {
            say!(g, "camtest: gain A#{want}: refused (A#{ans})\n");
            continue;
        }
        // Two frames for the change to land.
        for _ in 0..2 {
            if map(&mut held).is_none() {
                say!(g, "camtest: no frame\n");
                ferros_sys::exit(1);
            }
        }
        hist.fill(0);
        let (mut sum, mut dsum, mut dsq, mut cnt, mut gain) = (0u64, 0i64, 0u64, 0u64, 0u32);
        for _ in 0..pairs {
            let (Some((pa, ga)), _) = (map(&mut held), ()) else {
                ferros_sys::exit(1);
            };
            noise_rows(pa, h, w, stride, &mut a[..len]);
            let Some((pb, _)) = map(&mut held) else {
                ferros_sys::exit(1);
            };
            noise_rows(pb, h, w, stride, &mut b[..len]);
            gain = ga;
            for (&x, &y) in a[..len].iter().zip(b[..len].iter()) {
                sum += x as u64;
                let d = x as i64 - y as i64;
                dsum += d;
                dsq += (d * d) as u64;
                hist[x as usize] += 1;
                cnt += 1;
            }
        }
        // The pair difference's variance is twice a frame's: the noise in 16ths of a code is sqrt(var / 2) x 16.
        let var256 = (dsq * 256 - (dsum * dsum) as u64 * 256 / cnt) / cnt;
        let sigma16 = isqrt(var256 / 2);
        let mean16 = sum * 16 / cnt;
        // The codes within 4 sigma of the mean: how many hold samples, and the residues mod 2 and 4 of the samples there.
        // WHY the saturating subtraction and the min: at high gain 4 sigma passes the black level's 64 codes below the mean or the white's above, and the window would index the 1024-entry histogram outside itself; it stops at the codes there are.
        let (lo, hi) = ((mean16 / 16).saturating_sub(sigma16 * 4 / 16 + 2), ((mean16 / 16) + sigma16 * 4 / 16 + 2).min(1023));
        let (mut used, mut inside) = (0u64, 0u64);
        let mut res = [0u64; 4];
        for c in lo..=hi {
            let k = hist[c as usize] as u64;
            used += (k != 0) as u64;
            inside += k;
            res[(c & 3) as usize] += k;
        }
        // WHY the maxes: a window with no samples (a frame of one code everywhere) and a header without a gain (a driver with no model) would divide by zero and end the measure without its line.
        let r256 = |k: u64| k * 256 / inside.max(1);
        say!(g, "camtest: gain A#{want} (header A#{gain}): mean A#{mean16}/16, noise A#{sigma16}/16 of a code, input-referred A#{}/16; codes A#{lo} to A#{hi}: A#{used} of A#{} hold samples; residues mod 4 A#{} A#{} A#{} A#{} of 256\n", sigma16 * 1024 / gain.max(1) as u64, hi - lo + 1, r256(res[0]), r256(res[1]), r256(res[2]), r256(res[3]));
    }
    let mut c = [0u8; 4];
    let n = call(ep, &[4], &mut c);
    say!(g, "camtest: gain noise done; CLOSE G#{:X}\n", if n == ERR { 0xEEE } else { c[0] as u64 });
    ferros_sys::exit(0)
}

/// The gain's curve: OPEN, the exposure set to 2^`exp_log2` us (none when 0: the init's), then each gain step in turn (1024 one: 1x to 64x in halves of a stop), the frames' mean sample over `per` frames (16 when 0) after two to let the change land, said with the gain each frame's header carries; a step the driver refuses is said and passed. The mean above black against the gain says whether it is linear and where the analogue gain ends (the driver's console line names the sensor's model and its analogue most).
fn gain_curve(g: u64, ep: u64, id: u8, bin: u8, per: u8, exp_log2: u8, t1: u64) -> ! {
    let buf = unsafe { &mut *(&raw mut BUF) };
    let mut r = [0u8; 4];
    let n = call(ep, &[1, id], &mut r);
    say!(g, "camtest: gain curve: OPEN answered G#{:X} in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 {
        ferros_sys::exit(1);
    }
    let set = |what: u8, value: u64| -> u8 {
        let mut m = [3u8, what, 0, 0, 0, 0, 0, 0, 0, 0];
        m[2..10].copy_from_slice(&value.to_le_bytes());
        let mut r = [0xEEu8; 1];
        if call(ep, &m, &mut r) == ERR { 0xEE } else { r[0] }
    };
    if exp_log2 != 0 {
        say!(g, "camtest: exposure A#{} us: answered A#{}\n", 1u64 << exp_log2, set(0, (1u64 << exp_log2) * 1000));
    }
    let per = if per == 0 { 16 } else { per as u32 };
    // A frame's mean sample (every 16th of its binned samples) and the gain its header carries; None when no frame came.
    let mut frame = || -> Option<(u64, u32)> {
        let n = call(ep, &[2, bin], buf);
        if n == ERR || n < 64 {
            return None;
        }
        let samples = &buf[64..n as usize];
        let mut sum = 0u64;
        let mut cnt = 0u64;
        for s in samples.chunks_exact(2).step_by(1 << 4) {
            sum += u16::from_le_bytes([s[0], s[1]]) as u64;
            cnt += 1;
        }
        Some((sum * 16 / cnt, u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]])))
    };
    // 1x to 256x, a half stop apart (the halves rounded to 1024ths): past the main's analogue most (160x) into its digital gain, whose steps are whole (320x, 640x: the main's digital step is G#100, one).
    let steps: [u64; 19] = [1024, 1448, 2048, 2896, 4096, 5793, 8192, 11585, 16384, 23170, 32768, 46341, 65536, 92682, 131072, 185364, 262144, 327680, 655360];
    for &want in steps.iter() {
        let a = set(1, want);
        if a != 0 {
            say!(g, "camtest: gain A#{want}: refused (A#{a})\n");
            continue;
        }
        for _ in 0..2 {
            if frame().is_none() {
                say!(g, "camtest: no frame\n");
                ferros_sys::exit(1);
            }
        }
        let (mut sum, mut got_gain) = (0u64, 0u32);
        for _ in 0..per {
            let Some((m, gn)) = frame() else {
                say!(g, "camtest: no frame\n");
                ferros_sys::exit(1);
            };
            sum += m;
            got_gain = gn;
        }
        // The mean in 16ths of a code, so the curve near black keeps its fractions.
        say!(g, "camtest: gain A#{want} asked, A#{got_gain} in the header: mean sample A#{} sixteenths over A#{per} frames\n", sum / per as u64);
    }
    let n = call(ep, &[4], &mut r);
    say!(g, "camtest: gain curve done; CLOSE G#{:X}\n", if n == ERR { 0xEEE } else { r[0] as u64 });
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

/// The pairs (docs/CAMERA.md "Phase detect as data"): OPEN the camera in its init's mode (its PD rows in every ring buffer), then per focus target: SET the lens, `settle` frames by copy at bin 8 to let it land (16 if 0), SNAP (the driver holds the next frame's buffer, image and PD rows, and says both addresses), and hold it 2^`hold_log2` s (about; 2^7 if 0) for the host to read (tools/camera-pd-pair.sh); the next SNAP gives the last back.
/// `focus_hi` the lens's target's high byte (G#38, about a metre, if 0); G#FF a rack: G#1000 to G#7000 in steps of G#1000, seven pairs.
fn snap_pair(g: u64, ep: u64, id: u8, settle: u8, focus_hi: u8, hold_log2: u8, t1: u64) -> ! {
    let mut r = [0u8; 4];
    let n = call(ep, &[1, id], &mut r);
    say!(g, "camtest: OPEN answered G#{:X} in A#{} ms\n", if n == ERR { 0xEEE } else { r[0] as u64 }, ferros_sys::us_since(t1) / 1000);
    if n == ERR || r[0] != 0 {
        ferros_sys::exit(1);
    }
    let one = [(if focus_hi == 0 { 0x38 } else { focus_hi } as u64) << 8];
    let rack = [0x1000u64, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000, 0x7000];
    let targets: &[u64] = if focus_hi == 0xFF { &rack } else { &one };
    let buf = unsafe { &mut *(&raw mut BUF) };
    // WHY the min: the byte is the person's, and a shift past 43 would leave the u64; 2^10 (about 18 min) is more than any read of a pair takes.
    let hold_us = 1u64 << (if hold_log2 == 0 { 7 } else { hold_log2.min(10) } as u64 + 20);
    for (k, &focus) in targets.iter().enumerate() {
        let mut m = [3u8, 2, 0, 0, 0, 0, 0, 0, 0, 0];
        m[2..10].copy_from_slice(&focus.to_le_bytes());
        let mut a = [0xEEu8; 1];
        let n = call(ep, &m, &mut a);
        say!(g, "camtest: pair A#{k}: focus G#{focus:X}: answered A#{}\n", if n == ERR { 0xEE } else { a[0] });
        for _ in 0..if settle == 0 { 16 } else { settle } {
            if call(ep, &[2, 8], buf) == ERR {
                say!(g, "camtest: no frame while settling\n");
                ferros_sys::exit(1);
            }
        }
        let mut s = [0u8; 96];
        let n = call(ep, &[12], &mut s);
        if n == ERR || n < 96 {
            say!(g, "camtest: SNAP gave nothing (G#{:X}): the driver's console line says why\n", n);
            let _ = call(ep, &[4], &mut r);
            ferros_sys::exit(1);
        }
        let w = |k: usize| u64::from_le_bytes(s[k..k + 8].try_into().unwrap());
        // The line the host's pull reads (tools/camera-pd-pair.sh): every field it packs, in order.
        say!(g, "camtest: PAIR A#{k} image G#{:X} pd G#{:X} count A#{} exposure_ns A#{} gain A#{} focus A#{} eagle_ns A#{} status A#{}\n", w(64), w(72), u32::from_le_bytes(s[0..4].try_into().unwrap()), w(16), u32::from_le_bytes(s[24..28].try_into().unwrap()), u32::from_le_bytes(s[28..32].try_into().unwrap()), w(32), w(92) as u32);
        // STATS every 2^21 us meanwhile: the holder's quiet past 2^22 us ends the open (payloads/camera `UNASKED_US`), and a held pair must outlive the host's reads.
        let start = ferros_sys::now();
        let mut st = [0u8; 208];
        while ferros_sys::us_since(start) < hold_us {
            ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() * (1 << 21) / 1_000_000);
            let _ = call(ep, &[8], &mut st);
        }
    }
    let n = call(ep, &[4], &mut r);
    say!(g, "camtest: CLOSE G#{:X}; the pairs are gone with the region\n", if n == ERR { 0xEEE } else { r[0] as u64 });
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(payload_main);
