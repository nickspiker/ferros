//! The values a list carries typed, as the display shows them: a moment as wall time and a number in the person's base (docs/PUCKS.md "typed values the display formats").
//!
//! The base and the zone are the person's, never the puck's: a change of either redraws every puck with no puck waking.
//! Dozenal digits are the bytes U+0010 to U+001B, as the display's clock and its font write them; hex digits are 0-9 and A-F; decimal 0-9.
//! Zones as TIME.md decided (memory time_zones_decided): the caller passes ferros_sys::zone(), the legal offset for decimal and the standard offset for dozenal and hex.

use crate::{EAGLE_EPOCH_UNIX_SECS, OSC};

/// The most bytes one formatted value takes (a moment, or a number with its unit).
pub const OUT: usize = 1 << 5;
/// The most places after the first a moment or a number may ask for.
pub const PLACES_MAX: u8 = 4;
/// Units a `NUMBER` names by index, as symbols written after it (units are symbols, not words, so they need no language).
pub const UNITS: [&str; 10] = ["", "m", "\u{B0}", "\u{B0}C", "m/s", "km", "mm", "hPa", "\u{B5}T", "%"];
/// Unit indices, by name.
pub mod unit {
    pub const NONE: u8 = 0;
    pub const METRE: u8 = 1;
    pub const DEGREE: u8 = 2;
    pub const CELSIUS: u8 = 3;
    pub const METRE_PER_S: u8 = 4;
    pub const KM: u8 = 5;
    pub const MM: u8 = 6;
    pub const HPA: u8 = 7;
    pub const MICROTESLA: u8 = 8;
    pub const PERCENT: u8 = 9;
}

struct Out<'a> {
    b: &'a mut [u8; OUT],
    n: usize,
}

impl Out<'_> {
    fn ch(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        for &x in c.encode_utf8(&mut tmp).as_bytes() {
            // WHY the bound: OUT holds every value this module writes (a sign, 2^5 digits at most for an i64 in base 10 is 19, places, a unit of at most 4 bytes); a longer one is cut, never written past the buffer.
            if self.n < OUT {
                self.b[self.n] = x;
                self.n += 1;
            }
        }
    }
    fn digit(&mut self, d: u32, base: u8) {
        self.ch(digit(d, base));
    }
}

/// Digit d (below the base) in base `base`'s characters.
pub fn digit(d: u32, base: u8) -> char {
    match base {
        12 => char::from_u32(0x10 + d).unwrap_or('?'),
        _ => char::from_digit(d, base as u32).unwrap_or('?').to_ascii_uppercase(),
    }
}

/// The local seconds of the day a moment falls on: its Eagle oscillations to POSIX seconds (floored), plus the zone.
pub fn second_of_day(t: i64, zone: i64) -> i64 {
    (t.div_euclid(OSC) + EAGLE_EPOCH_UNIX_SECS + zone).rem_euclid(86400)
}

/// The moment `t` (Eagle oscillations) as wall time in `base` (10, 12 or 16), `places` digits after the first:
/// - decimal: twelve-hour h:mm, as the clock writes it (no leading zero, 12 at noon and midnight), then "am" or "pm"; `places` past 1 adds :ss;
/// - dozenal: the day's share, 1 + `places` dozenal places from the day's twelfth down (each place a twelfth of the one before: 2 h, 10 min, 50 s, ...), trailing zils dropped as the clock drops them (noon is Lun, not Lun:Zil);
/// - hex: the seconds since local midnight in hex, as the clock's centre shows them, leading zeros dropped.
pub fn moment(t: i64, zone: i64, base: u8, places: u8, b: &mut [u8; OUT]) -> &str {
    let sod = second_of_day(t, zone);
    let mut o = Out { b, n: 0 };
    match base {
        12 => {
            let p = places as u32 + 1;
            let u = sod * 12i64.pow(p) / 86400;
            let d: [i64; 5] = core::array::from_fn(|k| if (k as u32) < p { (u / 12i64.pow(p - 1 - k as u32)) % 12 } else { 0 });
            let last = (1..p as usize).rev().find(|&k| d[k] != 0).unwrap_or(0);
            for (k, v) in d.iter().enumerate().take(last + 1) {
                if k > 0 {
                    o.ch(':');
                }
                o.digit(*v as u32, 12);
            }
        }
        16 => {
            let mut started = false;
            for k in (0..5).rev() {
                let v = (sod >> (4 * k)) & 0xF;
                if v != 0 || started || k == 0 {
                    started = true;
                    o.digit(v as u32, 16);
                }
            }
        }
        _ => {
            let (h, m, s) = (sod / 3600, (sod / 60) % 60, sod % 60);
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            if h12 >= 10 {
                o.digit((h12 / 10) as u32, 10);
            }
            o.digit((h12 % 10) as u32, 10);
            o.ch(':');
            o.digit((m / 10) as u32, 10);
            o.digit((m % 10) as u32, 10);
            if places > 1 {
                o.ch(':');
                o.digit((s / 10) as u32, 10);
                o.digit((s % 10) as u32, 10);
            }
            o.ch(if h >= 12 { 'p' } else { 'a' });
            o.ch('m');
        }
    }
    let n = o.n;
    core::str::from_utf8(&b[..n]).unwrap_or("")
}

/// A value in Q16 of its unit, in `base` with `places` places after the point (rounded to the nearest last place), its unit's symbol after it.
pub fn number(value_q16: i64, places: u8, unit: u8, base: u8, b: &mut [u8; OUT]) -> &str {
    let base_i = base as i128;
    let scale = base_i.pow(places as u32);
    // The value in units of its last place, rounded half away from zero: exact in 128 bits for any i64 and up to PLACES_MAX places.
    let mag = (value_q16 as i128).abs();
    let scaled = (mag * scale + (1 << 15)) >> 16;
    let mut o = Out { b, n: 0 };
    if value_q16 < 0 && scaled != 0 {
        o.ch('-');
    }
    let int = scaled / scale;
    let mut digits = [0u8; 40];
    let mut k = 0;
    let mut v = int;
    loop {
        digits[k] = (v % base_i) as u8;
        k += 1;
        v /= base_i;
        if v == 0 {
            break;
        }
    }
    for i in (0..k).rev() {
        o.digit(digits[i] as u32, base);
    }
    if places > 0 {
        o.ch('.');
        let mut f = scaled % scale;
        let mut div = scale / base_i;
        for _ in 0..places {
            o.digit((f / div) as u32, base);
            f %= div;
            div /= base_i;
            if div == 0 {
                break;
            }
        }
    }
    for c in UNITS[unit as usize].chars() {
        o.ch(c);
    }
    let n = o.n;
    core::str::from_utf8(&b[..n]).unwrap_or("")
}

/// A day's seconds: the dozenal places of a duration are the clock's places of the day.
const DAY: f64 = 86400.;

/// A duration's last place in `base`: the place it counts in under its threshold (the fine), the place at and past it (the coarse), and the threshold as a count of fine places.
/// - decimal: a tenth of a second under a minute (s.t), else a second (m:ss, h:mm:ss);
/// - dozenal: the clock's places of the day (2 h, 10 min, 50 s, 50/12 s, 50/144 s), the fifth under the third's 50 s, else the fourth;
/// - hex: a sixteenth of a second under G#40 s, else a second.
fn duration_places(base: u8) -> (f64, f64, u64) {
    match base {
        12 => (DAY / libm::pow(12., 5.), DAY / libm::pow(12., 4.), 12 * 12),
        16 => (1. / 16., 1., 64 * 16),
        _ => (1. / 10., 1., 60 * 10),
    }
}

/// The count of last places a duration of `secs` shows, rounded up (`up`: a countdown, which reads zero only once it is over) or down (a count up): in the fine place while that count is under the threshold, else in the coarse. With the place's seconds and whether it is the fine one.
fn duration_count(secs: f64, base: u8, up: bool) -> (u64, f64, bool) {
    let (fine, coarse, threshold) = duration_places(base);
    let round = |x: f64| if up { libm::ceil(x) } else { libm::floor(x) };
    // f64 to u64 saturates in Rust: a duration past 2^64 tenths (58 billion years) reads as that many, never wraps.
    let n = round(secs / fine) as u64;
    if n < threshold { (n, fine, true) } else { (round(secs / coarse) as u64, coarse, false) }
}

impl Out<'_> {
    /// `v` in `base`, at least `min` digits (leading zeros; every caller asks one or more, so zero is written "0").
    fn uint(&mut self, v: u64, base: u8, min: usize) {
        // A u64 has at most 64 digits in any base from 2.
        let mut digits = [0u8; 64];
        let (mut k, mut v) = (0, v);
        while v > 0 || k < min {
            digits[k] = (v % base as u64) as u8;
            v /= base as u64;
            k += 1;
        }
        for i in (0..k).rev() {
            self.digit(digits[i] as u32, base);
        }
    }
}

/// A duration of `secs` seconds (not negative) as a timer shows it in `base` (10, 12 or 16), rounded up to its last place when `up` (a countdown: the last place stays until it has passed, so zero shows only at the end) or down (a stopwatch):
/// - decimal: h:mm:ss, m:ss under an hour, s.t under a minute, the hours counting on past a day;
/// - dozenal: the clock's places of the day joined by ':' as the clock joins them, from the first that is not zil (at least the last two) to the fourth (50/12 s), or the fifth (50/144 s) under 50 s; the first place counts on past a dozen (a day is 10 of them);
/// - hex: the seconds, with a sixteenth after the point under G#40 s.
pub fn duration(secs: f64, base: u8, up: bool, b: &mut [u8; OUT]) -> &str {
    let (n, _, fine) = duration_count(secs, base, up);
    let mut o = Out { b, n: 0 };
    match base {
        12 => {
            // Place `last` of the day (4 or 5) is the count's unit; place k above it is n / 12^(last − k), the first place unbounded.
            let last = if fine { 5 } else { 4 };
            let place = |k: u32| if k == 1 { n / 12u64.pow(last - 1) } else { (n / 12u64.pow(last - k)) % 12 };
            let first = (1..last - 1).find(|&k| place(k) != 0).unwrap_or(last - 1);
            for k in first..=last {
                if k > first {
                    o.ch(':');
                }
                o.uint(place(k), 12, 1);
            }
        }
        16 => {
            if fine {
                o.uint(n / 16, 16, 1);
                o.ch('.');
                o.digit((n % 16) as u32, 16);
            } else {
                o.uint(n, 16, 1);
            }
        }
        _ => {
            if fine {
                o.uint(n / 10, 10, 1);
                o.ch('.');
                o.digit((n % 10) as u32, 10);
            } else {
                let (h, m, s) = (n / 3600, (n / 60) % 60, n % 60);
                if h > 0 {
                    o.uint(h, 10, 1);
                    o.ch(':');
                    o.uint(m, 10, 2);
                } else {
                    o.uint(m, 10, 1);
                }
                o.ch(':');
                o.uint(s, 10, 2);
            }
        }
    }
    let n = o.n;
    core::str::from_utf8(&b[..n]).unwrap_or("")
}

/// How far a duration of `secs` moves (down when `up`, a countdown; else up) before what `duration` shows of it changes, in seconds; infinite for a countdown at zero, which shows zero from then on.
pub fn duration_next(secs: f64, base: u8, up: bool) -> f64 {
    let (n, step, fine) = duration_count(secs, base, up);
    if !up {
        return (n + 1) as f64 * step - secs;
    }
    if n == 0 {
        return f64::INFINITY;
    }
    let mut at = (n - 1) as f64 * step;
    // Counting down from the coarse place into the fine one: the coarse reading holds until the fine count falls under the threshold, at its last fine place below it, which is later than the coarse place's own boundary there.
    let (f, _, threshold) = duration_places(base);
    let edge = (threshold - 1) as f64 * f;
    if !fine && at < edge {
        at = edge;
    }
    // A reading that lies on a boundary to within f64's rounding of secs / step can count one place high, its boundary then secs itself: the change after it is a place further down, never a step of zero (which would wake the puck at once, again and again).
    if at >= secs {
        at -= step;
    }
    secs - at
}
