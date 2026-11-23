//! The clock, a puck (docs/PUCKS.md; owner, 2026-11-18: "ideally we just build this fresh and make another clock proper, then delete the weird wired in one"): the face the display drew itself until then (display `clock_state` and draw.rs `clock`), drawn here into the puck's own square and handed to the display, which trims it to the circle and places it.
//!
//! - **The face**: rings that sweep from 12:00 and click back to empty when they wrap, ticks across each ring black once the sweep has passed them, a stroke between the rings, the time in the centre, upright with the face.
//!   Decimal: seconds, minutes and the twelve-hour ring (the afternoon in the lighter fill), the digits h:mm:ss and am or pm under them.
//!   Dozenal: the day in twelfths, four places, the last places that are zil dropped; on a large circle six places, two rows of three.
//!   Hex: the seconds since midnight, four rings of sixteen.
//! - **When it is drawn**: at every frame the deck shows while the circle is on the glass (the watch's `FRAMES`: the rings move continuously), at the size and turn the display asks for; off the glass, never.
//! - **Its turn**: gravity's, or the camera's and the circle's own, as the display gives every puck.
//!
//! Stow: tools/clock-kist.sh (the owner's developer key, the clock's option since 2026-11-10, so the puck is the next version of the clock kist).
#![no_std]
#![no_main]

extern crate alloc;

#[path = "../../pucks/src/common.rs"]
mod common;

use ferros_puck::square::{cover, solid, under, words};
use libm::{atan2f, floorf, sqrtf};

/// One ring of the face: how far its sweep has gone (0 to 1 of a turn), whether in the lighter fill (the afternoon), how many ticks.
#[derive(Clone, Copy)]
struct Ring {
    frac: f32,
    lighter: bool,
    ticks: u32,
}
const NO_RING: Ring = Ring { frac: 0., lighter: false, ticks: 0 };

/// The bands of an n-ring face, in 256ths of the radius from the rim inward: each ring's outer and inner edge, the stroke between it and the next, then the centre's edge (the display's draw.rs BANDS3, BANDS4, BANDS6).
const BANDS3: [f32; 7] = [256., 214., 208., 172., 166., 130., 124.];
const BANDS4: [f32; 9] = [256., 222., 216., 188., 182., 156., 150., 130., 124.];
const BANDS6: [f32; 13] = [256., 240., 234., 218., 212., 196., 190., 174., 168., 152., 146., 130., 124.];

fn bands(n: usize) -> &'static [f32] {
    match n {
        6 => &BANDS6,
        4 => &BANDS4,
        _ => &BANDS3,
    }
}

/// The time as the face shows it: the rings, how many, the words in the centre (rows split by '\n'), am or pm when twelve-hour, and the widest the words can be (so the digits keep one size as the time's width changes).
struct Time {
    rings: [Ring; 6],
    n: usize,
    text: [u8; 32],
    len: usize,
    pm: Option<bool>,
    widest: &'static str,
}

/// The time now, from local wall time to the counter's tick (ferros_sys::local_time), in the person's base; `large` the circle has room for six dozenal places.
fn time(large: bool) -> Time {
    let (secs, sub) = ferros_sys::local_time().unwrap_or((0, 0));
    let hz = ferros_sys::freq() as i64;
    let t = secs.rem_euclid(86400) * hz + sub as i64;
    let frac = |p: i64| (t % p) as f32 / p as f32;
    let base = ferros_sys::device_base() & 0x7F;
    let mut out = Time { rings: [NO_RING; 6], n: 3, text: [0; 32], len: 0, pm: None, widest: "00:00:00" };
    let mut put = |c: char, o: &mut Time| {
        let mut b = [0u8; 4];
        for &x in c.encode_utf8(&mut b).as_bytes() {
            if o.len < o.text.len() {
                o.text[o.len] = x;
                o.len += 1;
            }
        }
    };
    let ring = |frac: f32, lighter: bool, ticks: u32| Ring { frac, lighter, ticks };
    if base == 12 && large {
        // Six places: the day's share in dozenal, exact in 128-bit integers (a place's unit is not a whole number of counter ticks): place k counts day / 12^(k+1).
        let day = 86400 * hz as i128;
        let tt = t as i128;
        for j in 0..6 {
            let m = 5 - j as u32;
            out.rings[j] = ring(((tt * 12i128.pow(m) % day) as f64 / day as f64) as f32, false, 12);
        }
        let u = (tt * 12i128.pow(6) / day) as i64;
        for k in 0..6u32 {
            if k == 3 {
                put('\n', &mut out);
            } else if k > 0 {
                put(':', &mut out);
            }
            put(char::from_u32(0x10 + ((u / 12i64.pow(5 - k)) % 12) as u32).unwrap_or('?'), &mut out);
        }
        out.n = 6;
        out.widest = "\u{10}:\u{10}:\u{10}";
    } else if base == 12 {
        // A dozenal place's unit: 50 s / 12 in counter ticks (exact at 24.576 MHz).
        let q4 = 50 * hz / 12;
        out.rings[..4].copy_from_slice(&[ring(frac(q4 * 12), false, 12), ring(frac(q4 * 144), false, 12), ring(frac(q4 * 1728), false, 12), ring(frac(q4 * 20736), false, 12)]);
        let u = t / q4;
        let d = [u / 1728, (u / 144) % 12, (u / 12) % 12, u % 12];
        // The last place that is not zil; the hour always shows.
        let last = (1..4).rev().find(|&k| d[k] != 0).unwrap_or(0);
        for (k, v) in d.iter().enumerate().take(last + 1) {
            if k > 0 {
                put(':', &mut out);
            }
            put(char::from_u32(0x10 + *v as u32).unwrap_or('?'), &mut out);
        }
        out.n = 4;
        out.widest = "\u{10}:\u{10}:\u{10}:\u{10}";
    } else if base == 16 {
        // The seconds since midnight in hex, four rings of sixteen ticks: a turn every G#10, G#100, G#1000 and G#1_0000 seconds.
        out.rings[..4].copy_from_slice(&[ring(frac(16 * hz), false, 16), ring(frac(256 * hz), false, 16), ring(frac(4096 * hz), false, 16), ring(frac(65536 * hz), false, 16)]);
        let sod = t / hz;
        let mut started = false;
        for k in (0..5).rev() {
            let v = (sod >> (4 * k)) & 0xF;
            if v != 0 || started || k == 0 {
                started = true;
                put(char::from_digit(v as u32, 16).unwrap_or('?').to_ascii_uppercase(), &mut out);
            }
        }
        out.n = 4;
        out.widest = "00000";
    } else {
        let sod = t / hz;
        let (h, m, s) = (sod / 3600, (sod / 60) % 60, sod % 60);
        out.rings[..3].copy_from_slice(&[ring(frac(60 * hz), false, 60), ring(frac(3600 * hz), false, 60), ring(frac(43200 * hz), h >= 12, 12)]);
        // Twelve-hour digits to match the twelve-hour ring, the hour without a leading zero, 12 at noon and midnight.
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        if h12 >= 10 {
            put((b'0' + (h12 / 10) as u8) as char, &mut out);
        }
        put((b'0' + (h12 % 10) as u8) as char, &mut out);
        for v in [m, s] {
            put(':', &mut out);
            put((b'0' + (v / 10) as u8) as char, &mut out);
            put((b'0' + (v % 10) as u8) as char, &mut out);
        }
        out.pm = Some(h >= 12);
    }
    out
}

/// Coverage of the inside of a disc of radius `r` at distance `dist` (pixels): the C¹ edge every puck shape takes (ferros_puck::draw::edge).
fn inside(dist: f32, r: f32) -> f32 {
    ferros_puck::draw::edge(dist - r)
}

/// The face into the square `px` (`d` a side, cleared), turned by (tc, ts): the words first, in front, then each pixel's face under them.
fn face(px: &mut [u64], d: usize, (tc, ts): (f32, f32), tm: &Time, th: &ferros_sys::theme::Theme, glyphs: &mut ferros_glyphs::Glyphs) {
    let r = d as f32 * 0.5;
    let b = bands(tm.n);
    let rad: [f32; 13] = core::array::from_fn(|i| if i < b.len() { r * b[i] / 256. } else { 0. });
    let n = tm.n;
    let cb = 2 * n;
    // The words, in front: as big as the centre makes room for (its width less a margin) at the widest the time can be.
    let room = rad[cb] * 2. * 0.82;
    // WHY the floor: a typeface with no advance for these digits would give a width of 0 to divide by.
    let unit = ferros_puck::square::width(glyphs, tm.widest, 100.).max(1.);
    let size = room / unit * 100.;
    let text = core::str::from_utf8(&tm.text[..tm.len]).unwrap_or("");
    let rows = text.split('\n').count() as f32;
    for (k, line) in text.split('\n').enumerate() {
        let o = (k as f32 - (rows - 1.) * 0.5) * size * 1.15;
        words(px, d, glyphs, r - ts * o, r + tc * o, size, (tc, ts), th.text, 0xFFFF, line);
    }
    if let Some(pm) = tm.pm {
        let o = size * 0.8;
        words(px, d, glyphs, r - ts * o, r + tc * o, size * 0.45, (tc, ts), th.text2, 0xFFFF, if pm { "pm" } else { "am" });
    }
    let (face_px, fill, fill_pm, stroke, tick, passed) = (solid(th.plane), solid(th.clock_fill), solid(th.clock_fill_pm), solid(th.clock_stroke), solid(th.clock_tick), solid(th.clock_tick_passed));
    // Each ring's sweep boundary as a direction in the face's frame, (sin θ, −cos θ) from 12:00 clockwise, and whether it is past half a turn.
    let bound: [(f32, f32, bool); 6] = core::array::from_fn(|i| {
        let a = tm.rings[i].frac * core::f32::consts::TAU;
        (libm::sinf(a), -libm::cosf(a), tm.rings[i].frac > 0.5)
    });
    let tau = core::f32::consts::TAU;
    for y in 0..d {
        for x in 0..d {
            // Into the face's own frame (the turn undone), where 12:00 is up.
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            let (fx, fy) = (dx * tc + dy * ts, dy * tc - dx * ts);
            let dist = sqrtf(fx * fx + fy * fy);
            if dist > r + 1. {
                continue;
            }
            // Clockwise from 12:00, 0 to 1 of a turn.
            let theta = {
                let a = atan2f(fx, -fy) / tau;
                if a < 0. { a + 1. } else { a }
            };
            // The bands as nested discs: each band's weight is the coverage inside its outer edge less inside the next, so the face's pixel is a partition of its bands (premultiplied, so the sum is a pixel).
            let mut acc = [0f32; 4];
            let mut add = |p: u64, w: f32| {
                if w > 0. {
                    for (k, a) in acc.iter_mut().enumerate() {
                        *a += (p >> (16 * k) & 0xFFFF) as f32 * w;
                    }
                }
            };
            for bi in 0..=cb {
                let w = inside(dist, rad[bi]) - if bi < cb { inside(dist, rad[bi + 1]) } else { 0. };
                if w <= 0. {
                    continue;
                }
                let p = if bi == cb {
                    // The centre: the face, and the zero mark from 70 % of its radius to a pixel inside its edge at 12:00.
                    let along = -fy;
                    let mark = if along > rad[cb] * 0.7 && along < rad[cb] - 1. { 1. - libm::fabsf(fx) } else { 0. };
                    if mark > 0. { under(cover(tick, (mark * 65535.) as u32), face_px) } else { face_px }
                } else if bi % 2 == 1 {
                    stroke
                } else {
                    let i = bi / 2;
                    let rg = tm.rings[i];
                    let (bx, by, past) = bound[i];
                    // A half-plane's coverage of a pixel is its signed distance from the pixel's centre plus half a pixel, whole from half a pixel on: the definition of the coverage, not a guard.
                    let cov_x = (fx + 0.5).clamp(0., 1.);
                    let cov_line = (-(bx * fy - by * fx) + 0.5).clamp(0., 1.);
                    let swept = if rg.frac == 0. { 0. } else if past { cov_x.max(cov_line) } else { cov_x.min(cov_line) };
                    let f = if rg.lighter { fill_pm } else { fill };
                    let ring_px = if swept <= 0. { face_px } else if swept >= 1. { f } else { under(cover(f, (swept * 65535.) as u32), face_px) };
                    // The ring's nearest tick, across the ring inset a pixel at both ends, black once the sweep has reached it: in front of the ring.
                    let t = rg.ticks as f32;
                    let k = floorf(theta * t + 0.5);
                    let a = k / t * tau;
                    let (ux, uy) = (libm::sinf(a), -libm::cosf(a));
                    let along = fx * ux + fy * uy;
                    let perp = libm::fabsf(ux * fy - uy * fx);
                    let tc_ = if along > rad[bi + 1] + 1. && along < rad[bi] - 1. { 1. - perp } else { 0. };
                    if tc_ > 0. {
                        let reached = rg.frac > 0. && rg.frac * t >= k - 1e-3;
                        under(cover(if reached { passed } else { tick }, (tc_ * 65535.) as u32), ring_px)
                    } else {
                        ring_px
                    }
                };
                add(p, w);
            }
            // WHY the min: the weights sum to one at most and each lane is at most G#FFFF, so the sum is too, but for the float's rounding, which could carry a lane into the next one's bits.
            let p = (0..4).fold(0u64, |v, k| v | ((acc[k] + 0.5) as u64).min(0xFFFF) << (16 * k));
            let i = y * d + x;
            px[i] = under(px[i], p);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    let link = common::start(grants, input, input_len, true);
    let mut shown = 0u64;
    loop {
        let seen = common::rt::bell();
        let v = common::rt::view();
        if v.shown {
            let tm = time(v.large);
            let th = ferros_sys::theme::current();
            if let Some(glyphs) = common::glyphs() {
                let t0 = ferros_sys::now();
                let r = common::rt::show_with(link, &v, |px, d, turn| face(px, d, turn, &tm, th, glyphs));
                // The first squares and every 1024th after (about every 100 s at the flips): its side, how long the face and the hand-over took, and the display's answer (a measure for the 8a, 2026-11-18).
                shown += 1;
                if shown <= 4 || shown % 1024 == 0 {
                    let mut l = [0u8; 96];
                    let n = line(&mut l, &[b"clock: square ", &dec(shown), b": ", &dec(v.d as u64), b" px in ", &dec(ferros_sys::us_since(t0)), b" us, answer ", &dec(r.map_or(9, |s| s as u64)), b"\n"]);
                    common::say(grants, &l[..n]);
                }
            }
        }
        // The next frame the deck shows (the watch's FRAMES), a view change, or nothing while off the glass.
        common::wait(seen, None);
    }
}

// The arc grows on demand to 2^26 (64 MiB of addresses): the scratch its square is drawn in before it goes thru the handle, as large as its circle asks (ferros_puck::rt `scratch`; at most two squares of `SIDE_MAX` fill the slot, one fills half of this).
ferros_sys::entry!(payload_main, grows 1 << 26);

/// `v` in decimal digits, for a console line.
fn dec(v: u64) -> alloc::vec::Vec<u8> {
    let mut d = alloc::vec::Vec::new();
    let mut v = v;
    loop {
        d.push(b'0' + (v % 10) as u8);
        v /= 10;
        if v == 0 {
            break;
        }
    }
    d.reverse();
    d
}

/// The parts one after another into `out`; how many bytes.
fn line(out: &mut [u8], parts: &[&[u8]]) -> usize {
    let mut n = 0;
    for p in parts {
        for &b in *p {
            // WHY the bound: the line is cut at its buffer rather than written past it.
            if n < out.len() {
                out[n] = b;
                n += 1;
            }
        }
    }
    n
}
