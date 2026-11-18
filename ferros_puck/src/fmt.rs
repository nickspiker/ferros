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
