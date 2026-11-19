//! Broadcast ephemerides (docs/GPS.md "Fresh aiding without Google's servers"): a GPS satellite's LNAV ephemeris and clock as IS-GPS-200 defines them, read from a RINEX 3 navigation file (the IGS's BRDC, BKG or ESA), and the satellite placed by IS-GPS-200's own equations.
//! The file is read once, at the edge, by fixed columns (RINEX 3.05 §A: an 4-column label, then 19-column numbers, `D` or `E` exponents); what is kept is the numbers.
//! Only GPS records are read here; Galileo's and BeiDou's use the same Kepler set and are added when the chip's message for them is known.

use libm::{atan2, cos, sin, sqrt};

/// One GPS LNAV ephemeris (IS-GPS-200 Table 20-III, 20-I), SI units: seconds, metres, radians (RINEX gives the angles in radians and the rates in radians a second).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpsEph {
    pub prn: u8,
    /// The GPS week of the ephemeris (RINEX's continuous week, not mod 1024) and its time of ephemeris in that week.
    pub week: u16,
    pub toe: f64,
    /// The clock: its reference time in the same week (seconds), the bias, drift and drift rate.
    pub toc: f64,
    pub af0: f64,
    pub af1: f64,
    pub af2: f64,
    pub iode: u16,
    pub iodc: u16,
    pub crs: f64,
    pub dn: f64,
    pub m0: f64,
    pub cuc: f64,
    pub e: f64,
    pub cus: f64,
    pub sqrt_a: f64,
    pub cic: f64,
    pub omega0: f64,
    pub cis: f64,
    pub i0: f64,
    pub crc: f64,
    pub omega: f64,
    pub omega_dot: f64,
    pub idot: f64,
    pub tgd: f64,
    /// The user range accuracy in metres, the health word (0 healthy), the fit interval in hours (0: 4 hours).
    pub ura: f64,
    pub health: u16,
    pub fit: f64,
}

/// A RINEX number: `-1.032240688801e-04`, `D` exponents too, blanks as 0. None for anything else.
fn num(s: &[u8]) -> Option<f64> {
    let s = core::str::from_utf8(s).ok()?.trim();
    if s.is_empty() {
        return Some(0.);
    }
    let mut b = [0u8; 32];
    if s.len() > b.len() {
        return None;
    }
    for (i, c) in s.bytes().enumerate() {
        b[i] = if c == b'D' || c == b'd' { b'E' } else { c };
    }
    core::str::from_utf8(&b[..s.len()]).ok()?.parse().ok()
}

/// The four numbers of a broadcast orbit line (`    ` then 4 × 19 columns); a short line's missing numbers are 0.
fn four(l: &[u8]) -> Option<[f64; 4]> {
    let mut v = [0.; 4];
    for (k, x) in v.iter_mut().enumerate() {
        let a = 4 + 19 * k;
        if a >= l.len() {
            break;
        }
        *x = num(&l[a..(a + 19).min(l.len())])?;
    }
    Some(v)
}

/// GPS seconds of week and week of a calendar epoch read as GPS time (RINEX gives GPS records' epochs in GPS time).
fn gps_week_sow(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: f64) -> (u16, f64) {
    // Days from 1980-01-06 (the GPS epoch) by the civil-from-days count (Howard Hinnant's, proleptic Gregorian).
    let days = |y: i64, m: i64, d: i64| {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    };
    let dd = days(y, mo, d) - days(1980, 1, 6);
    let week = dd.div_euclid(7);
    let sow = (dd.rem_euclid(7) * 86400 + h * 3600 + mi * 60) as f64 + s;
    (week as u16, sow)
}

/// Every GPS ephemeris in a RINEX 3 navigation file, in file order; `take` gets each. Returns (taken, refused): a record whose numbers do not read is refused whole.
pub fn each_gps(text: &[u8], mut take: impl FnMut(GpsEph)) -> (u32, u32) {
    let lines: alloc_free::Lines = alloc_free::Lines::new(text);
    let mut it = lines;
    let (mut ok, mut bad) = (0u32, 0u32);
    let mut header = true;
    while let Some(l) = it.next() {
        if header {
            if l.windows(13).any(|w| w == b"END OF HEADER") {
                header = false;
            }
            continue;
        }
        if l.first() != Some(&b'G') || l.len() < 23 {
            continue;
        }
        let mut rec: [&[u8]; 8] = [l; 8];
        let mut whole = true;
        for r in rec.iter_mut().skip(1) {
            match it.next() {
                Some(x) => *r = x,
                None => whole = false,
            }
        }
        if !whole {
            bad += 1;
            break;
        }
        match gps_record(&rec) {
            Some(e) => {
                ok += 1;
                take(e)
            }
            None => bad += 1,
        }
    }
    (ok, bad)
}

fn gps_record(r: &[&[u8]; 8]) -> Option<GpsEph> {
    let l = r[0];
    let int = |a: usize, b: usize| -> Option<i64> { core::str::from_utf8(l.get(a..b)?).ok()?.trim().parse().ok() };
    let prn = int(1, 3)? as u8;
    let (y, mo, d, h, mi) = (int(4, 8)?, int(9, 11)?, int(12, 14)?, int(15, 17)?, int(18, 20)?);
    let s = int(21, 23)? as f64;
    let (_, toc) = gps_week_sow(y, mo, d, h, mi, s);
    let c = |k: usize| num(&l[(23 + 19 * k).min(l.len())..(23 + 19 * (k + 1)).min(l.len())]);
    let (af0, af1, af2) = (c(0)?, c(1)?, c(2)?);
    let o: [[f64; 4]; 7] = [four(r[1])?, four(r[2])?, four(r[3])?, four(r[4])?, four(r[5])?, four(r[6])?, four(r[7])?];
    Some(GpsEph {
        prn,
        week: o[4][2] as u16,
        toe: o[2][0],
        toc,
        af0,
        af1,
        af2,
        iode: o[0][0] as u16,
        crs: o[0][1],
        dn: o[0][2],
        m0: o[0][3],
        cuc: o[1][0],
        e: o[1][1],
        cus: o[1][2],
        sqrt_a: o[1][3],
        cic: o[2][1],
        omega0: o[2][2],
        cis: o[2][3],
        i0: o[3][0],
        crc: o[3][1],
        omega: o[3][2],
        omega_dot: o[3][3],
        idot: o[4][0],
        ura: o[5][0],
        health: o[5][1] as u16,
        tgd: o[5][2],
        iodc: o[5][3] as u16,
        fit: o[6][1],
    })
}

/// WGS-84's GM and the earth's rotation as IS-GPS-200 fixes them.
const GM: f64 = 3.986005e14;
const OMEGA_E: f64 = 7.2921151467e-5;

/// The satellite's earth-fixed position (WGS-84, metres) at GPS time `t` (seconds of the ephemeris's week; a time in another week is given as seconds from this week's start), by IS-GPS-200 Table 20-IV.
pub fn gps_ecef(e: &GpsEph, t: f64) -> [f64; 3] {
    let a = e.sqrt_a * e.sqrt_a;
    let n = sqrt(GM / (a * a * a)) + e.dn;
    let tk = t - e.toe;
    let m = e.m0 + n * tk;
    let mut ea = m;
    for _ in 0..1 << 4 {
        ea = m + e.e * sin(ea);
    }
    let v = atan2(sqrt(1. - e.e * e.e) * sin(ea), cos(ea) - e.e);
    let phi = v + e.omega;
    let (s2, c2) = (sin(2. * phi), cos(2. * phi));
    let u = phi + e.cus * s2 + e.cuc * c2;
    let r = a * (1. - e.e * cos(ea)) + e.crs * s2 + e.crc * c2;
    let i = e.i0 + e.idot * tk + e.cis * s2 + e.cic * c2;
    let (x, y) = (r * cos(u), r * sin(u));
    let om = e.omega0 + (e.omega_dot - OMEGA_E) * tk - OMEGA_E * e.toe;
    [x * cos(om) - y * cos(i) * sin(om), x * sin(om) + y * cos(i) * cos(om), y * sin(i)]
}

/// Lines of a text without allocation (`\n` ends one, a `\r` before it dropped).
mod alloc_free {
    pub struct Lines<'a> {
        t: &'a [u8],
        at: usize,
    }
    impl<'a> Lines<'a> {
        pub fn new(t: &'a [u8]) -> Self {
            Lines { t, at: 0 }
        }
        pub fn next(&mut self) -> Option<&'a [u8]> {
            if self.at >= self.t.len() {
                return None;
            }
            let rest = &self.t[self.at..];
            let n = rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
            self.at += n + 1;
            let l = &rest[..n];
            Some(if l.last() == Some(&b'\r') { &l[..n - 1] } else { l })
        }
    }
}
