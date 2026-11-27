//! The sun and the moon for the sun puck and the moon puck (docs/PUCKS.md).
//! Where each body stands in the sky, when it crosses any altitude (rise, set, the three twilights, golden and blue hour), the moon's phase and its four quarters, and a face per body a puck can draw and wake on.
//!
//! - The sun: Meeus, Astronomical Algorithms, 2nd ed. (1998), chapter 25, the method "of low accuracy" (0.01 degree), with the apparent longitude and obliquity of 25.8; NOAA's solar calculator is this same method.
//! - The moon: Meeus chapter 47, the ELP-2000/82 truncation of tables 47.A and 47.B, all 60 + 60 terms and the additive terms (A1, A2, A3), about 10 arcseconds in longitude and 4 in latitude.
//! - Nutation and the obliquity: Meeus chapter 22, the short series (0.5 arcsecond in Δψ, 0.1 in Δε) and 22.2.
//! - Sidereal time: Meeus 12.4, apparent by Δψ cos ε.
//! - The moon seen from the place: parallax by Meeus 40.2 and 40.3 with ρ sin φ′ and ρ cos φ′ from 11.1 at sea level.
//! - Rise and set: the altitudes of Meeus chapter 15 (h0 = −0.8333 degree for the sun's centre, h0 = 0.7275 π − 0.5667 degree for the moon's geocentric centre), found by sampling then bisecting the altitude as a function of time, so any altitude takes the one function ([`crossings`], [`next_crossing`]).
//! - Illumination: Meeus chapter 48 (48.1 to 48.3 and the bright limb's angle 48.5); the quarters: Meeus chapter 49 with all its periodic and planetary corrections.
//!
//! Time is Eagle time thruout: seconds since 1969-07-20 20:17:40 UTC, counted as POSIX seconds from that label ([`EAGLE_EPOCH_UNIX_SECS`], as ferros_tide counts them); the engine converts to days from J2000 inside ([`days_j2000`]), in UT for the earth's turning and in TT (UT + [`delta_t`]) for the bodies' motion.
//! A place is a latitude and a longitude in radians, north and east positive ([`Place`]); every angle the API returns is in radians.
//!
//! Host-tested against Meeus's worked examples (12.a, 12.b, 15.a, 25.a, 28.a, 47.a, 48.a, 49.a, 49.b), the US Naval Observatory's published rise, set, twilight and transit times, its celestial navigation data, and its 2026 list of the moon's phases (src/tests.rs).
#![no_std]

use libm::{atan2, cos, floor, sin, sqrt};

/// Where the Eagle epoch (1969-07-20 20:17:40 UTC) falls in POSIX seconds: vsf's `EAGLE_EPOCH_UNIX_SECS`, repeated here so the engine needs no vsf (and no ferros_tide).
pub const EAGLE_EPOCH_UNIX_SECS: i64 = -14_182_940;
/// J2000.0 (2000-01-01 12:00 UTC) in Eagle seconds: POSIX 946 728 000 less the epoch.
pub const J2000_EAGLE: f64 = 946_728_000. - EAGLE_EPOCH_UNIX_SECS as f64;
/// Seconds in a day (a published constant, not a knob).
pub const DAY: f64 = 86_400.;
/// Days in a Julian century.
const CENTURY: f64 = 36_525.;
const D2R: f64 = core::f64::consts::PI / 180.;
const TAU: f64 = core::f64::consts::TAU;
const PI: f64 = core::f64::consts::PI;

/// Days from J2000.0 at Eagle second `t` (UT).
pub fn days_j2000(t: f64) -> f64 {
    (t - J2000_EAGLE) / DAY
}

/// Reduce degrees to [0, 360).
fn rev(d: f64) -> f64 {
    d - 360. * floor(d / 360.)
}

/// Reduce radians to [0, 2π).
fn rev_rad(a: f64) -> f64 {
    a - TAU * floor(a / TAU)
}

/// Reduce radians to [−π, π).
fn wrap_pi(a: f64) -> f64 {
    a - TAU * floor((a + PI) / TAU)
}

// ΔT = TT − UT.

/// ΔT at the first of January of every fifth year from 1975 to 2025 (seconds): IERS/USNO, maia.usno.navy.mil/ser7/deltat.data, fetched 2026-11-17.
const DELTA_T: [f64; 11] = [45.4761, 50.5387, 54.3427, 56.8553, 60.7853, 63.8285, 64.6876, 66.0699, 67.6439, 69.3612, 69.1377];
const DELTA_T_FIRST: f64 = 1975.;
const DELTA_T_STEP: f64 = 5.;

/// ΔT = TT − UT in seconds at Eagle second `t`: the moon moves half a degree an hour against the stars, so its 69 seconds (2026) move the moon by a hundredth of a degree and its quarters by a minute.
/// Measured values ([`DELTA_T`]) joined by cubic Hermite pieces with Catmull-Rom tangents (C¹, so no curve built from it kinks); outside 1975 to 2025 the end piece's tangent runs on as a line, which holds within a second or two for the decades either side (the earth's turning cannot be predicted further than that anyway).
pub fn delta_t(t: f64) -> f64 {
    let year = 2000. + days_j2000(t) / 365.25;
    let u = (year - DELTA_T_FIRST) / DELTA_T_STEP;
    let last = DELTA_T.len() - 1;
    let tangent = |k: usize| -> f64 {
        if k == 0 {
            DELTA_T[1] - DELTA_T[0]
        } else if k == last {
            DELTA_T[last] - DELTA_T[last - 1]
        } else {
            0.5 * (DELTA_T[k + 1] - DELTA_T[k - 1])
        }
    };
    // WHY: the table holds eleven knots; outside them the knot index would leave it, so the ends run on as lines (the same value and slope at the join, C¹).
    if u < 0. {
        return DELTA_T[0] + tangent(0) * u;
    }
    if u >= last as f64 {
        return DELTA_T[last] + tangent(last) * (u - last as f64);
    }
    let k = u as usize;
    let s = u - k as f64;
    let (s2, s3) = (s * s, s * s * s);
    (2. * s3 - 3. * s2 + 1.) * DELTA_T[k] + (s3 - 2. * s2 + s) * tangent(k) + (-2. * s3 + 3. * s2) * DELTA_T[k + 1] + (s3 - s2) * tangent(k + 1)
}

/// Days from J2000.0 in TT at Eagle second `t`.
fn tt_days(t: f64) -> f64 {
    days_j2000(t) + delta_t(t) / DAY
}

/// A place on the earth: geodetic latitude and longitude in radians, north and east positive (at sea level).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
}

impl Place {
    /// A place from degrees, north and east positive.
    pub fn deg(lat: f64, lon: f64) -> Place {
        Place { lat: lat * D2R, lon: lon * D2R }
    }

    /// ρ sin φ′ and ρ cos φ′ (Meeus 11.1, sea level): where the place sits relative to the earth's centre, in equatorial radii, for the moon's parallax.
    fn geocentric(&self) -> (f64, f64) {
        // b/a of the earth's figure (Meeus chapter 11, the IAU 1976 ellipsoid).
        const BA: f64 = 0.99664719;
        let u = atan2(BA * sin(self.lat), cos(self.lat));
        (BA * sin(u), cos(u))
    }
}

/// An altitude above the horizon and an azimuth from north thru east, radians; the altitude is geometric (no refraction: the event altitudes carry it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Horizontal {
    pub alt: f64,
    pub az: f64,
}

/// Altitude and azimuth from a local hour angle, a declination and a latitude (Meeus 13.5 and 13.6, the azimuth taken from north).
/// Both come from atan2 of the direction's horizon components, so nothing leaves an inverse sine's domain at the zenith.
fn horizontal(h: f64, dec: f64, lat: f64) -> Horizontal {
    let up = sin(lat) * sin(dec) + cos(lat) * cos(dec) * cos(h);
    let north = sin(dec) * cos(lat) - cos(dec) * cos(h) * sin(lat);
    let east = -cos(dec) * sin(h);
    Horizontal { alt: atan2(up, sqrt(north * north + east * east)), az: rev_rad(atan2(east, north)) }
}

/// Nutation and the obliquity at `d` days from J2000 (TT).
#[derive(Clone, Copy, Debug)]
struct Nutation {
    /// Nutation in longitude, radians.
    dpsi: f64,
    /// The true obliquity ε0 + Δε, radians.
    eps: f64,
}

/// The mean obliquity ε0, degrees (Meeus 22.2).
fn mean_obliquity(tc: f64) -> f64 {
    23. + 26. / 60. + (21.448 - 46.8150 * tc - 0.00059 * tc * tc + 0.001813 * tc * tc * tc) / 3600.
}

/// Meeus chapter 22, the short series: within 0.5 arcsecond in Δψ and 0.1 in Δε.
fn nutation(d: f64) -> Nutation {
    let tc = d / CENTURY;
    let om = (125.04452 - 1934.136261 * tc + 0.0020708 * tc * tc + tc * tc * tc / 450_000.) * D2R;
    let l = (280.4665 + 36000.7698 * tc) * D2R;
    let lp = (218.3165 + 481267.8813 * tc) * D2R;
    let dpsi = -17.20 * sin(om) - 1.32 * sin(2. * l) - 0.23 * sin(2. * lp) + 0.21 * sin(2. * om);
    let deps = 9.20 * cos(om) + 0.57 * cos(2. * l) + 0.10 * cos(2. * lp) - 0.09 * cos(2. * om);
    Nutation { dpsi: dpsi / 3600. * D2R, eps: (mean_obliquity(tc) + deps / 3600.) * D2R }
}

/// Greenwich apparent sidereal time, radians, at `d_ut` days from J2000 in UT with the nutation `n` (Meeus 12.4, plus Δψ cos ε).
/// The day's whole turns are taken off before the large product so the angle keeps its precision.
fn sidereal(d_ut: f64, n: &Nutation) -> f64 {
    let tc = d_ut / CENTURY;
    let theta = 360. * (d_ut - floor(d_ut)) + 0.98564736629 * d_ut + 280.46061837 + 0.000387933 * tc * tc - tc * tc * tc / 38_710_000.;
    rev(theta) * D2R + n.dpsi * cos(n.eps)
}

// The sun.

/// The sun's apparent place at a moment.
#[derive(Clone, Copy, Debug)]
pub struct Sun {
    /// Apparent right ascension and declination, radians.
    pub ra: f64,
    pub dec: f64,
    /// Apparent ecliptic longitude, radians.
    pub lon: f64,
    /// Distance, astronomical units.
    pub au: f64,
    /// The equation of time, apparent less mean solar time, seconds (Meeus 28.1).
    pub eot: f64,
    /// Greenwich apparent sidereal time then, radians.
    pub gast: f64,
}

/// The sun at `d` days from J2000 (TT), by Meeus chapter 25's low-accuracy method; returns it with the mean longitude L0 (degrees) and the nutation.
fn sun_tt(d: f64) -> (Sun, f64, Nutation) {
    let tc = d / CENTURY;
    let l0 = 280.46646 + 36000.76983 * tc + 0.0003032 * tc * tc;
    let m = (357.52911 + 35999.05029 * tc - 0.0001537 * tc * tc) * D2R;
    let e = 0.016708634 - 0.000042037 * tc - 0.0000001267 * tc * tc;
    let c = (1.914602 - 0.004817 * tc - 0.000014 * tc * tc) * sin(m) + (0.019993 - 0.000101 * tc) * sin(2. * m) + 0.000289 * sin(3. * m);
    let true_lon = l0 + c;
    let nu = m + c * D2R;
    let au = 1.000001018 * (1. - e * e) / (1. + e * cos(nu));
    let om = (125.04 - 1934.136 * tc) * D2R;
    // 25.8: the apparent longitude and the obliquity corrected for it.
    let lambda = (true_lon - 0.00569 - 0.00478 * sin(om)) * D2R;
    let eps = (mean_obliquity(tc) + 0.00256 * cos(om)) * D2R;
    let ra = rev_rad(atan2(cos(eps) * sin(lambda), cos(lambda)));
    let dec = libm::asin(sin(eps) * sin(lambda));
    let n = nutation(d);
    // 28.1: E = L0 − 0.0057183° − α + Δψ cos ε, reduced to (−180°, 180°), four seconds of time a degree.
    let eot_deg = l0 - 0.0057183 - ra / D2R + n.dpsi / D2R * cos(n.eps);
    let eot = (rev(eot_deg + 180.) - 180.) * 240.;
    (Sun { ra, dec, lon: rev_rad(lambda), au, eot, gast: 0. }, l0, n)
}

impl Sun {
    /// The sun at Eagle second `t`.
    pub fn at(t: f64) -> Sun {
        let (mut s, _, n) = sun_tt(tt_days(t));
        s.gast = sidereal(days_j2000(t), &n);
        s
    }

    /// The local hour angle at `place`, radians in [−π, π): negative before the meridian (morning), positive after.
    pub fn hour_angle(&self, place: Place) -> f64 {
        wrap_pi(self.gast + place.lon - self.ra)
    }

    /// Altitude and azimuth at `place`.
    pub fn horizontal(&self, place: Place) -> Horizontal {
        horizontal(self.hour_angle(place), self.dec, place.lat)
    }
}

/// The sun's geometric altitude at Eagle second `t` and `place`, radians.
pub fn sun_altitude(t: f64, place: Place) -> f64 {
    Sun::at(t).horizontal(place).alt
}

// The moon.

/// Table 47.A: the multiples of D, M, M′, F, and the coefficients of Σl (sine, 10^-6 degree) and Σr (cosine, 10^-3 km).
const MOON_LR: [(i8, i8, i8, i8, i32, i32); 60] = [
    (0, 0, 1, 0, 6288774, -20905355),
    (2, 0, -1, 0, 1274027, -3699111),
    (2, 0, 0, 0, 658314, -2955968),
    (0, 0, 2, 0, 213618, -569925),
    (0, 1, 0, 0, -185116, 48888),
    (0, 0, 0, 2, -114332, -3149),
    (2, 0, -2, 0, 58793, 246158),
    (2, -1, -1, 0, 57066, -152138),
    (2, 0, 1, 0, 53322, -170733),
    (2, -1, 0, 0, 45758, -204586),
    (0, 1, -1, 0, -40923, -129620),
    (1, 0, 0, 0, -34720, 108743),
    (0, 1, 1, 0, -30383, 104755),
    (2, 0, 0, -2, 15327, 10321),
    (0, 0, 1, 2, -12528, 0),
    (0, 0, 1, -2, 10980, 79661),
    (4, 0, -1, 0, 10675, -34782),
    (0, 0, 3, 0, 10034, -23210),
    (4, 0, -2, 0, 8548, -21636),
    (2, 1, -1, 0, -7888, 24208),
    (2, 1, 0, 0, -6766, 30824),
    (1, 0, -1, 0, -5163, -8379),
    (1, 1, 0, 0, 4987, -16675),
    (2, -1, 1, 0, 4036, -12831),
    (2, 0, 2, 0, 3994, -10445),
    (4, 0, 0, 0, 3861, -11650),
    (2, 0, -3, 0, 3665, 14403),
    (0, 1, -2, 0, -2689, -7003),
    (2, 0, -1, 2, -2602, 0),
    (2, -1, -2, 0, 2390, 10056),
    (1, 0, 1, 0, -2348, 6322),
    (2, -2, 0, 0, 2236, -9884),
    (0, 1, 2, 0, -2120, 5751),
    (0, 2, 0, 0, -2069, 0),
    (2, -2, -1, 0, 2048, -4950),
    (2, 0, 1, -2, -1773, 4130),
    (2, 0, 0, 2, -1595, 0),
    (4, -1, -1, 0, 1215, -3958),
    (0, 0, 2, 2, -1110, 0),
    (3, 0, -1, 0, -892, 3258),
    (2, 1, 1, 0, -810, 2616),
    (4, -1, -2, 0, 759, -1897),
    (0, 2, -1, 0, -713, -2117),
    (2, 2, -1, 0, -700, 2354),
    (2, 1, -2, 0, 691, 0),
    (2, -1, 0, -2, 596, 0),
    (4, 0, 1, 0, 549, -1423),
    (0, 0, 4, 0, 537, -1117),
    (4, -1, 0, 0, 520, -1571),
    (1, 0, -2, 0, -487, -1739),
    (2, 1, 0, -2, -399, 0),
    (0, 0, 2, -2, -381, -4421),
    (1, 1, 1, 0, 351, 0),
    (3, 0, -2, 0, -340, 0),
    (4, 0, -3, 0, 330, 0),
    (2, -1, 2, 0, 327, 0),
    (0, 2, 1, 0, -323, 1165),
    (1, 1, -1, 0, 299, 0),
    (2, 0, 3, 0, 294, 0),
    (2, 0, -1, -2, 0, 8752),
];

/// Table 47.B: the multiples of D, M, M′, F, and the coefficient of Σb (sine, 10^-6 degree).
const MOON_B: [(i8, i8, i8, i8, i32); 60] = [
    (0, 0, 0, 1, 5128122),
    (0, 0, 1, 1, 280602),
    (0, 0, 1, -1, 277693),
    (2, 0, 0, -1, 173237),
    (2, 0, -1, 1, 55413),
    (2, 0, -1, -1, 46271),
    (2, 0, 0, 1, 32573),
    (0, 0, 2, 1, 17198),
    (2, 0, 1, -1, 9266),
    (0, 0, 2, -1, 8822),
    (2, -1, 0, -1, 8216),
    (2, 0, -2, -1, 4324),
    (2, 0, 1, 1, 4200),
    (2, 1, 0, -1, -3359),
    (2, -1, -1, 1, 2463),
    (2, -1, 0, 1, 2211),
    (2, -1, -1, -1, 2065),
    (0, 1, -1, -1, -1870),
    (4, 0, -1, -1, 1828),
    (0, 1, 0, 1, -1794),
    (0, 0, 0, 3, -1749),
    (0, 1, -1, 1, -1565),
    (1, 0, 0, 1, -1491),
    (0, 1, 1, 1, -1475),
    (0, 1, 1, -1, -1410),
    (0, 1, 0, -1, -1344),
    (1, 0, 0, -1, -1335),
    (0, 0, 3, 1, 1107),
    (4, 0, 0, -1, 1021),
    (4, 0, -1, 1, 833),
    (0, 0, 1, -3, 777),
    (4, 0, -2, 1, 671),
    (2, 0, 0, -3, 607),
    (2, 0, 2, -1, 596),
    (2, -1, 1, -1, 491),
    (2, 0, -2, 1, -451),
    (0, 0, 3, -1, 439),
    (2, 0, 2, 1, 422),
    (2, 0, -3, -1, 421),
    (2, 1, -1, 1, -366),
    (2, 1, 0, 1, -351),
    (4, 0, 0, 1, 331),
    (2, -1, 1, 1, 315),
    (2, -2, 0, -1, 302),
    (0, 0, 1, 3, -283),
    (2, 1, 1, -1, -229),
    (1, 1, 0, -1, 223),
    (1, 1, 0, 1, 223),
    (0, 1, -2, -1, -220),
    (2, 1, -1, -1, -220),
    (1, 0, 1, 1, -185),
    (2, -1, -2, -1, 181),
    (0, 1, 2, 1, -177),
    (4, 0, -2, -1, 176),
    (4, -1, -1, -1, 166),
    (1, 0, 1, -1, -164),
    (4, 0, 1, -1, 132),
    (1, 0, -1, -1, -119),
    (4, -1, 0, -1, 115),
    (2, -2, 0, 1, 107),
];

/// The moon's geocentric ecliptic longitude and latitude (degrees, of the date, without nutation) and its distance (km) at `d` days from J2000 (TT): Meeus chapter 47.
fn moon_geo(d: f64) -> (f64, f64, f64) {
    let tc = d / CENTURY;
    let (t2, t3, t4) = (tc * tc, tc * tc * tc, tc * tc * tc * tc);
    let lp = 218.3164477 + 481267.88123421 * tc - 0.0015786 * t2 + t3 / 538_841. - t4 / 65_194_000.;
    let dd = (297.8501921 + 445267.1114034 * tc - 0.0018819 * t2 + t3 / 545_868. - t4 / 113_065_000.) * D2R;
    let m = (357.5291092 + 35999.0502909 * tc - 0.0001536 * t2 + t3 / 24_490_000.) * D2R;
    let mp = (134.9633964 + 477198.8675055 * tc + 0.0087414 * t2 + t3 / 69_699. - t4 / 14_712_000.) * D2R;
    let f = (93.2720950 + 483202.0175233 * tc - 0.0036539 * t2 - t3 / 3_526_000. + t4 / 863_310_000.) * D2R;
    let a1 = (119.75 + 131.849 * tc) * D2R;
    let a2 = (53.09 + 479264.290 * tc) * D2R;
    let a3 = (313.45 + 481266.484 * tc) * D2R;
    // The eccentricity of the earth's orbit shrinks: terms in M carry E, in 2M carry E².
    let e = 1. - 0.002516 * tc - 0.0000074 * t2;
    let ecc = |k: i8| -> f64 {
        match k {
            0 => 1.,
            1 | -1 => e,
            _ => e * e,
        }
    };
    let (mut sl, mut sr, mut sb) = (0., 0., 0.);
    for &(kd, km, kmp, kf, l, r) in MOON_LR.iter() {
        let arg = kd as f64 * dd + km as f64 * m + kmp as f64 * mp + kf as f64 * f;
        let w = ecc(km);
        sl += l as f64 * w * sin(arg);
        sr += r as f64 * w * cos(arg);
    }
    for &(kd, km, kmp, kf, b) in MOON_B.iter() {
        let arg = kd as f64 * dd + km as f64 * m + kmp as f64 * mp + kf as f64 * f;
        sb += b as f64 * ecc(km) * sin(arg);
    }
    let lpr = lp * D2R;
    // The additive terms: Venus (A1), Jupiter (A2), and the flattening of the earth (L′ − F).
    sl += 3958. * sin(a1) + 1962. * sin(lpr - f) + 318. * sin(a2);
    sb += -2235. * sin(lpr) + 382. * sin(a3) + 175. * sin(a1 - f) + 175. * sin(a1 + f) + 127. * sin(lpr - mp) - 115. * sin(lpr + mp);
    (rev(lp + sl / 1e6), sb / 1e6, 385_000.56 + sr / 1000.)
}

/// The moon's apparent geocentric place at a moment.
#[derive(Clone, Copy, Debug)]
pub struct Moon {
    /// Apparent right ascension and declination, radians.
    pub ra: f64,
    pub dec: f64,
    /// Apparent ecliptic longitude and latitude, radians.
    pub lon: f64,
    pub lat: f64,
    /// Distance between the centres, km.
    pub km: f64,
    /// Equatorial horizontal parallax, radians.
    pub parallax: f64,
    /// Greenwich apparent sidereal time then, radians.
    pub gast: f64,
}

/// The earth's equatorial radius, km (Meeus 47, the value its parallax takes).
const EARTH_KM: f64 = 6378.14;

/// The moon at `d` days from J2000 (TT): its place without sidereal time.
fn moon_tt(d: f64) -> (Moon, Nutation) {
    let (l, b, km) = moon_geo(d);
    let n = nutation(d);
    let lon = l * D2R + n.dpsi;
    let lat = b * D2R;
    // Ecliptic to equatorial (Meeus 13.3 and 13.4) with the true obliquity; the declination's sine is a unit vector's component, inside asin's domain.
    let ra = rev_rad(atan2(sin(lon) * cos(n.eps) - libm::tan(lat) * sin(n.eps), cos(lon)));
    let dec = libm::asin(sin(lat) * cos(n.eps) + cos(lat) * sin(n.eps) * sin(lon));
    (Moon { ra, dec, lon: rev_rad(lon), lat, km, parallax: libm::asin(EARTH_KM / km), gast: 0. }, n)
}

impl Moon {
    /// The moon at Eagle second `t`.
    pub fn at(t: f64) -> Moon {
        let (mut m, n) = moon_tt(tt_days(t));
        m.gast = sidereal(days_j2000(t), &n);
        m
    }

    /// The geocentric local hour angle at `place`, radians in [−π, π).
    pub fn hour_angle(&self, place: Place) -> f64 {
        wrap_pi(self.gast + place.lon - self.ra)
    }

    /// Altitude and azimuth as seen from the earth's centre (what the rise and set altitude of Meeus chapter 15 is measured in).
    pub fn geocentric(&self, place: Place) -> Horizontal {
        horizontal(self.hour_angle(place), self.dec, place.lat)
    }

    /// The topocentric hour angle and declination at `place` (Meeus 40.2 and 40.3): the moon is near enough that the place's offset from the earth's centre moves it by up to a degree.
    pub fn topocentric_equatorial(&self, place: Place) -> (f64, f64) {
        let (rs, rc) = place.geocentric();
        let h = self.hour_angle(place);
        let sp = sin(self.parallax);
        let den = cos(self.dec) - rc * sp * cos(h);
        let da = atan2(-rc * sp * sin(h), den);
        let dec = atan2((sin(self.dec) - rs * sp) * cos(da), den);
        (h - da, dec)
    }

    /// Altitude and azimuth as seen from `place`.
    pub fn topocentric(&self, place: Place) -> Horizontal {
        let (h, dec) = self.topocentric_equatorial(place);
        horizontal(h, dec, place.lat)
    }

    /// The moon's horizon function: geocentric altitude less h0 = 0.7275 π − 0.5667° (Meeus chapter 15), zero when its upper limb meets the refracted horizon.
    fn horizon(&self, place: Place) -> f64 {
        self.geocentric(place).alt - (0.7275 * self.parallax - MOON_REFRACTION)
    }
}

/// The moon's topocentric altitude at Eagle second `t` and `place`, radians (geometric, as seen from the place).
pub fn moon_altitude(t: f64, place: Place) -> f64 {
    Moon::at(t).topocentric(place).alt
}

// Altitudes the events are defined by (Meeus chapter 15; the twilights and the photographers' hours by their published definitions).

/// Sunrise and sunset: the centre 50 arcminutes down (34′ of refraction at the horizon and a 16′ semidiameter), Meeus chapter 15's −0.8333°.
pub const SUNRISE: f64 = -0.8333 * D2R;
/// Civil twilight's edge, −6°.
pub const CIVIL: f64 = -6. * D2R;
/// Nautical twilight's edge, −12°.
pub const NAUTICAL: f64 = -12. * D2R;
/// Astronomical twilight's edge, −18°.
pub const ASTRONOMICAL: f64 = -18. * D2R;
/// Golden hour: the sun between −4° and +6°; blue hour: between −6° and −4°.
pub const GOLDEN_LOW: f64 = -4. * D2R;
pub const GOLDEN_HIGH: f64 = 6. * D2R;
/// The refraction at the horizon Meeus's moon h0 takes, 34′.
const MOON_REFRACTION: f64 = 0.5667 * D2R;

/// The sun's levels a face shows, in the order the deadline scan takes them.
const SUN_LEVELS: [f64; 6] = [ASTRONOMICAL, NAUTICAL, CIVIL, GOLDEN_LOW, SUNRISE, GOLDEN_HIGH];

// Root finding.

/// The step an altitude is sampled at when looking for crossings: 2^9 seconds (about 8.5 minutes).
/// Two crossings of one altitude closer together than this (the sun grazing it near a polar day's edge) can be missed as a pair; then the day reads as never crossing, which is how it looks.
pub const STEP: f64 = (1 << 9) as f64;
/// Bisections of a sampled change of sign: 2^9 s / 2^16, under a hundredth of a second.
const BISECT: usize = 1 << 4;
/// Newton steps for a transit from a guess within the equation of time's drift (seconds), each cutting the error by the ratio of the day's drift to the day.
const NEWTON: usize = 1 << 2;
/// The step of the central difference an altitude's rate is taken by: 2^5 s, where the third derivative's error (rate × (ω step)² / 6, about 10^-6 of the rate) is far below anything drawn.
const RATE_STEP: f64 = (1 << 5) as f64;

/// A crossing of an altitude: when, and whether the body was rising thru it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    pub t: f64,
    pub rising: bool,
}

/// The zeros of `f` (an altitude less a threshold) in [from, to), in time order, into `out`: how many were written.
/// Samples every [`STEP`] and bisects each change of sign (ferros_tide's extremes, on the altitude instead of the rate).
pub fn crossings_of<F: Fn(f64) -> f64>(f: F, from: f64, to: f64, out: &mut [Crossing]) -> usize {
    let mut n = 0;
    let mut a = from;
    let mut fa = f(a);
    while a < to && n < out.len() {
        let b = a + STEP;
        let fb = f(b);
        if (fa > 0.) != (fb > 0.) {
            let rising = fb > 0.;
            let t = bisect(&f, a, b, rising);
            if t >= from && t < to {
                out[n] = Crossing { t, rising };
                n += 1;
            }
        }
        a = b;
        fa = fb;
    }
    n
}

/// The zero of `f` in [a, b], where f goes up thru zero when `rising` (down otherwise).
fn bisect<F: Fn(f64) -> f64>(f: &F, a: f64, b: f64, rising: bool) -> f64 {
    let (mut lo, mut hi) = (a, b);
    for _ in 0..BISECT {
        let m = 0.5 * (lo + hi);
        if (f(m) > 0.) == rising {
            hi = m;
        } else {
            lo = m;
        }
    }
    0.5 * (lo + hi)
}

/// The sun or the moon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    Sun,
    Moon,
}

/// A body's altitude at Eagle second `t` and `place`: the sun's geometric, the moon's topocentric.
pub fn altitude(body: Body, t: f64, place: Place) -> f64 {
    match body {
        Body::Sun => sun_altitude(t, place),
        Body::Moon => moon_altitude(t, place),
    }
}

/// Every crossing of `alt` (radians) by `body` at `place` in [from, to), into `out`: how many were written.
pub fn crossings(body: Body, from: f64, to: f64, place: Place, alt: f64, out: &mut [Crossing]) -> usize {
    crossings_of(|t| altitude(body, t, place) - alt, from, to, out)
}

/// The first time in [from, to) that `body` crosses altitude `alt` going up (`rising`) or down; None when it does not.
pub fn next_crossing(body: Body, from: f64, to: f64, place: Place, alt: f64, rising: bool) -> Option<f64> {
    let f = |t: f64| altitude(body, t, place) - alt;
    let mut a = from;
    let mut fa = f(a);
    while a < to {
        let b = a + STEP;
        let fb = f(b);
        if (fa > 0.) != (fb > 0.) && (fb > 0.) == rising {
            let t = bisect(&f, a, b, rising);
            if t >= from && t < to {
                return Some(t);
            }
        }
        a = b;
        fa = fb;
    }
    None
}

/// How a body meets an altitude over a span: its first rise thru it and its first set, or, when it crosses neither way, which side it stays on the whole span (the polar day and the polar night, said outright).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pass {
    /// It crosses at least once; either may be absent (the moon some days rises and does not set).
    Crosses { rise: Option<f64>, set: Option<f64> },
    /// Above the altitude the whole span.
    Up,
    /// Below it the whole span.
    Down,
}

impl Pass {
    /// From a function that is above zero when the body is above the altitude.
    fn of<F: Fn(f64) -> f64>(f: F, from: f64, to: f64) -> Pass {
        let mut c = [Crossing { t: 0., rising: false }; 1 << 3];
        let n = crossings_of(&f, from, to, &mut c);
        if n == 0 {
            return if f(from) > 0. { Pass::Up } else { Pass::Down };
        }
        let rise = c[..n].iter().find(|x| x.rising).map(|x| x.t);
        let set = c[..n].iter().find(|x| !x.rising).map(|x| x.t);
        Pass::Crosses { rise, set }
    }

    /// The rise (or dawn), when there is one.
    pub fn rise(&self) -> Option<f64> {
        match self {
            Pass::Crosses { rise, .. } => *rise,
            _ => None,
        }
    }

    /// The set (or dusk), when there is one.
    pub fn set(&self) -> Option<f64> {
        match self {
            Pass::Crosses { set, .. } => *set,
            _ => None,
        }
    }
}

/// How the sun meets altitude `alt` over [from, to) at `place`.
pub fn sun_pass(from: f64, to: f64, place: Place, alt: f64) -> Pass {
    Pass::of(|t| sun_altitude(t, place) - alt, from, to)
}

/// Moonrise and moonset over [from, to) at `place`, by Meeus chapter 15's h0 for the moon.
pub fn moon_pass(from: f64, to: f64, place: Place) -> Pass {
    Pass::of(|t| Moon::at(t).horizon(place), from, to)
}

/// A span of time, Eagle seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub from: f64,
    pub to: f64,
}

/// Where a span lies against the day's solar noon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Morning,
    Evening,
    /// It holds noon (the sun stays in the band all thru the middle of the day, as golden hour does in a high winter).
    Thru,
}

impl Span {
    /// Morning when it ends by `noon`, evening when it starts after, else thru.
    pub fn side(&self, noon: f64) -> Side {
        if self.to <= noon {
            Side::Morning
        } else if self.from >= noon {
            Side::Evening
        } else {
            Side::Thru
        }
    }
}

/// The spans of a day the sun spends between two altitudes, cut at the day's ends: at most four (a morning, an evening, and a band around midnight cut in two by the day's edges).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bands {
    pub n: usize,
    pub spans: [Span; 1 << 2],
}

impl Bands {
    /// The spans in time order.
    pub fn spans(&self) -> &[Span] {
        &self.spans[..self.n]
    }

    /// The times the sun at `place` spends between altitudes `lo` and `hi` in [from, to).
    /// The sun's altitude is continuous, so every crossing of either edge toggles in and out.
    pub fn of(from: f64, to: f64, place: Place, lo: f64, hi: f64) -> Bands {
        let mut c = [Crossing { t: 0., rising: false }; 1 << 4];
        let n_lo = crossings(Body::Sun, from, to, place, lo, &mut c);
        let n = n_lo + crossings(Body::Sun, from, to, place, hi, &mut c[n_lo..]);
        // Insertion sort: a handful of crossings.
        for k in 1..n {
            let mut j = k;
            while j > 0 && c[j - 1].t > c[j].t {
                c.swap(j - 1, j);
                j -= 1;
            }
        }
        let h0 = sun_altitude(from, place);
        let mut inside = h0 > lo && h0 < hi;
        let mut b = Bands { n: 0, spans: [Span { from: 0., to: 0. }; 1 << 2] };
        let mut open = from;
        for x in &c[..n] {
            if inside {
                b.push(Span { from: open, to: x.t });
            } else {
                open = x.t;
            }
            inside = !inside;
        }
        if inside {
            b.push(Span { from: open, to });
        }
        b
    }

    fn push(&mut self, s: Span) {
        // WHY: the spans live in a fixed array of four; a fifth (a sun grazing an edge several times in a day, which the sampling step already all but rules out) is dropped rather than written past the array's end.
        if self.n < self.spans.len() {
            self.spans[self.n] = s;
            self.n += 1;
        }
    }
}

// Transits.

/// The sun's hour angle advances a turn a solar day.
const SUN_RATE: f64 = TAU / DAY;
/// The moon's a turn a mean lunar day (24 h 50.47 min, the synodic month's beat against the solar day).
const MOON_RATE: f64 = TAU / (DAY * (1. + 1. / 28.530588861));

/// The sun's upper transit (solar noon) nearest Eagle second `t` at `place`.
pub fn sun_transit_near(t: f64, place: Place) -> f64 {
    let mut x = t;
    for _ in 0..NEWTON {
        x -= Sun::at(x).hour_angle(place) / SUN_RATE;
    }
    x
}

/// The sun's next upper transit after Eagle second `t` at `place`.
pub fn sun_transit_after(t: f64, place: Place) -> f64 {
    let h = rev_rad(Sun::at(t).hour_angle(place));
    let x = sun_transit_near(t + (TAU - h) / SUN_RATE, place);
    // A Newton step landing on the noon just passed (t a hair after it) moves on to the next.
    if x > t { x } else { sun_transit_near(x + DAY, place) }
}

/// The moon's upper transit nearest Eagle second `t` at `place` (topocentric hour angle zero).
pub fn moon_transit_near(t: f64, place: Place) -> f64 {
    let mut x = t;
    for _ in 0..NEWTON {
        x -= Moon::at(x).topocentric_equatorial(place).0 / MOON_RATE;
    }
    x
}

// Illumination and the phases.

/// The four principal phases, counted from new moon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quarter {
    New = 0,
    First = 1,
    Full = 2,
    Last = 3,
}

impl Quarter {
    fn of(k: u8) -> Quarter {
        match k & 3 {
            0 => Quarter::New,
            1 => Quarter::First,
            2 => Quarter::Full,
            _ => Quarter::Last,
        }
    }
}

/// The mean synodic month, days (Meeus 49.1).
const SYNODIC: f64 = 29.530588861;

/// The moment of the phase `q` of lunation `k` (k = 0 the new moon of 2000 January 6), days from J2000 in TT: Meeus chapter 49, 49.1 with every correction of its tables and the fourteen planetary arguments.
fn phase_tt(k: i64, q: Quarter) -> f64 {
    let k = k as f64 + q as u8 as f64 / 4.;
    let tc = k / 1236.85;
    let (t2, t3, t4) = (tc * tc, tc * tc * tc, tc * tc * tc * tc);
    let mean = 5.09766 + SYNODIC * k + 0.00015437 * t2 - 0.000000150 * t3 + 0.00000000073 * t4;
    let e = 1. - 0.002516 * tc - 0.0000074 * t2;
    let m = (2.5534 + 29.10535670 * k - 0.0000014 * t2 - 0.00000011 * t3) * D2R;
    let mp = (201.5643 + 385.81693528 * k + 0.0107582 * t2 + 0.00001238 * t3 - 0.000000058 * t4) * D2R;
    let f = (160.7108 + 390.67050284 * k - 0.0016118 * t2 - 0.00000227 * t3 + 0.000000011 * t4) * D2R;
    let om = (124.7746 - 1.56375588 * k + 0.0020672 * t2 + 0.00000215 * t3) * D2R;
    let s = |x: f64| sin(x);
    let c = match q {
        Quarter::New => {
            -0.40720 * s(mp) + 0.17241 * e * s(m) + 0.01608 * s(2. * mp) + 0.01039 * s(2. * f) + 0.00739 * e * s(mp - m) - 0.00514 * e * s(mp + m) + 0.00208 * e * e * s(2. * m) - 0.00111 * s(mp - 2. * f) - 0.00057 * s(mp + 2. * f) + 0.00056 * e * s(2. * mp + m) - 0.00042 * s(3. * mp) + 0.00042 * e * s(m + 2. * f) + 0.00038 * e * s(m - 2. * f) - 0.00024 * e * s(2. * mp - m) - 0.00017 * s(om) - 0.00007 * s(mp + 2. * m) + 0.00004 * s(2. * mp - 2. * f) + 0.00004 * s(3. * m) + 0.00003 * s(mp + m - 2. * f) + 0.00003 * s(2. * mp + 2. * f) - 0.00003 * s(mp + m + 2. * f) + 0.00003 * s(mp - m + 2. * f) - 0.00002 * s(mp - m - 2. * f) - 0.00002 * s(3. * mp + m) + 0.00002 * s(4. * mp)
        }
        Quarter::Full => {
            -0.40614 * s(mp) + 0.17302 * e * s(m) + 0.01614 * s(2. * mp) + 0.01043 * s(2. * f) + 0.00734 * e * s(mp - m) - 0.00515 * e * s(mp + m) + 0.00209 * e * e * s(2. * m) - 0.00111 * s(mp - 2. * f) - 0.00057 * s(mp + 2. * f) + 0.00056 * e * s(2. * mp + m) - 0.00042 * s(3. * mp) + 0.00042 * e * s(m + 2. * f) + 0.00038 * e * s(m - 2. * f) - 0.00024 * e * s(2. * mp - m) - 0.00017 * s(om) - 0.00007 * s(mp + 2. * m) + 0.00004 * s(2. * mp - 2. * f) + 0.00004 * s(3. * m) + 0.00003 * s(mp + m - 2. * f) + 0.00003 * s(2. * mp + 2. * f) - 0.00003 * s(mp + m + 2. * f) + 0.00003 * s(mp - m + 2. * f) - 0.00002 * s(mp - m - 2. * f) - 0.00002 * s(3. * mp + m) + 0.00002 * s(4. * mp)
        }
        Quarter::First | Quarter::Last => {
            let quarter = -0.62801 * s(mp) + 0.17172 * e * s(m) - 0.01183 * e * s(mp + m) + 0.00862 * s(2. * mp) + 0.00804 * s(2. * f) + 0.00454 * e * s(mp - m) + 0.00204 * e * e * s(2. * m) - 0.00180 * s(mp - 2. * f) - 0.00070 * s(mp + 2. * f) - 0.00040 * s(3. * mp) - 0.00034 * e * s(2. * mp - m) + 0.00032 * e * s(m + 2. * f) + 0.00032 * e * s(m - 2. * f) - 0.00028 * e * e * s(mp + 2. * m) + 0.00027 * e * s(2. * mp + m) - 0.00017 * s(om) - 0.00005 * s(mp - m - 2. * f) + 0.00004 * s(2. * mp + 2. * f) - 0.00004 * s(mp + m + 2. * f) + 0.00004 * s(mp - 2. * m) + 0.00003 * s(mp + m - 2. * f) + 0.00003 * s(3. * m) + 0.00002 * s(2. * mp - 2. * f) + 0.00002 * s(mp - m + 2. * f) - 0.00002 * s(3. * mp + m);
            let w = 0.00306 - 0.00038 * e * cos(m) + 0.00026 * cos(mp) - 0.00002 * cos(mp - m) + 0.00002 * cos(mp + m) + 0.00002 * cos(2. * f);
            if q == Quarter::First { quarter + w } else { quarter - w }
        }
    };
    // The planetary arguments, degrees, and their coefficients (days).
    const PLANETS: [(f64, f64, f64); 14] = [
        (299.77, 0.107408, 0.000325),
        (251.88, 0.016321, 0.000165),
        (251.83, 26.651886, 0.000164),
        (349.42, 36.412478, 0.000126),
        (84.66, 18.206239, 0.000110),
        (141.74, 53.303771, 0.000062),
        (207.14, 2.453732, 0.000060),
        (154.84, 7.306860, 0.000056),
        (34.52, 27.261239, 0.000047),
        (207.19, 0.121824, 0.000042),
        (291.34, 1.844379, 0.000040),
        (161.72, 24.198154, 0.000037),
        (239.56, 25.513099, 0.000035),
        (331.55, 3.592518, 0.000023),
    ];
    let mut p = 0.;
    for (j, &(a0, a1, coeff)) in PLANETS.iter().enumerate() {
        // A1 alone carries a term in T².
        let a = a0 + a1 * k - if j == 0 { 0.009173 * t2 } else { 0. };
        p += coeff * sin(a * D2R);
    }
    mean + c + p
}

/// Eagle seconds (UT) of the phase `q` of lunation `k`: TT less ΔT, ΔT taken at the moment itself.
fn phase_time(k: i64, q: Quarter) -> f64 {
    let tt = J2000_EAGLE + phase_tt(k, q) * DAY;
    tt - delta_t(tt)
}

/// The lunation whose new moon is the last at or before Eagle second `t`.
fn lunation(t: f64) -> i64 {
    // The mean new moon's guess can be off by up to 14 hours either way; one step back or forward settles it.
    let mut k = floor((days_j2000(t) - 5.09766) / SYNODIC) as i64;
    while phase_time(k, Quarter::New) > t {
        k -= 1;
    }
    while phase_time(k + 1, Quarter::New) <= t {
        k += 1;
    }
    k
}

/// The first of the four principal phases strictly after Eagle second `t`, and its moment.
pub fn next_phase(t: f64) -> (Quarter, f64) {
    let k = lunation(t);
    for j in 1..=4u8 {
        let q = Quarter::of(j);
        let x = phase_time(k + (j >> 2) as i64, q);
        if x > t {
            return (q, x);
        }
    }
    // The new moon of lunation k + 1 is after t by the definition of k, so the loop always returns by j = 4.
    unreachable!()
}

/// The last new moon at or before Eagle second `t`.
pub fn last_new_moon(t: f64) -> f64 {
    phase_time(lunation(t), Quarter::New)
}

/// How the moon is lit at a moment, and how it looks from a place.
#[derive(Clone, Copy, Debug)]
pub struct Illumination {
    /// The phase angle i, sun to moon to earth, radians (Meeus 48.3): 0 full, π new.
    pub phase_angle: f64,
    /// The illuminated fraction of the disc, (1 + cos i) / 2 (Meeus 48.1).
    pub fraction: f64,
    /// The position angle of the bright limb's midpoint, from north thru east, radians (Meeus 48.5).
    pub bright_limb: f64,
    /// The same angle measured from the place's zenith (χ − q, q the parallactic angle of Meeus 14.1): how far to turn the drawn moon's lit side from straight up.
    pub zenith_limb: f64,
    /// Waxing: the moon's longitude runs ahead of the sun's by under 180°.
    pub waxing: bool,
    /// Seconds since the last new moon.
    pub age: f64,
}

impl Illumination {
    /// The moon's illumination at Eagle second `t` seen from `place`.
    pub fn at(t: f64, place: Place) -> Illumination {
        let sun = Sun::at(t);
        let moon = Moon::at(t);
        illumination(&sun, &moon, place, t)
    }
}

/// Astronomical units in km (IAU 2012), to put the sun's distance beside the moon's.
const AU_KM: f64 = 149_597_870.7;

fn illumination(sun: &Sun, moon: &Moon, place: Place, t: f64) -> Illumination {
    // The geocentric elongation ψ from the two directions' dot and cross products (48.2), so no inverse cosine meets a rounded 1.
    let unit = |ra: f64, dec: f64| [cos(dec) * cos(ra), cos(dec) * sin(ra), sin(dec)];
    let (s, m) = (unit(sun.ra, sun.dec), unit(moon.ra, moon.dec));
    let dot = s[0] * m[0] + s[1] * m[1] + s[2] * m[2];
    let cross = [s[1] * m[2] - s[2] * m[1], s[2] * m[0] - s[0] * m[2], s[0] * m[1] - s[1] * m[0]];
    let sin_psi = sqrt(cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]);
    let r = sun.au * AU_KM;
    let i = atan2(r * sin_psi, moon.km - r * dot);
    let da = sun.ra - moon.ra;
    let chi = rev_rad(atan2(cos(sun.dec) * sin(da), sin(sun.dec) * cos(moon.dec) - cos(sun.dec) * sin(moon.dec) * cos(da)));
    let h = moon.hour_angle(place);
    // 14.1 multiplied thru by cos φ, so the poles need no tangent.
    let q = atan2(cos(place.lat) * sin(h), sin(place.lat) * cos(moon.dec) - cos(place.lat) * sin(moon.dec) * cos(h));
    Illumination {
        phase_angle: i,
        fraction: 0.5 * (1. + cos(i)),
        bright_limb: chi,
        zenith_limb: rev_rad(chi - q),
        waxing: rev_rad(moon.lon - sun.lon) < PI,
        age: t - last_new_moon(t),
    }
}

// The faces.

/// Samples of the altitude curve a face carries.
pub const CURVE: usize = 1 << 6;

/// The local day holding Eagle second `t`, for a zone `utc_offset` seconds east of UTC: its start and end (Eagle seconds).
/// A day that a clock change makes 23 or 25 hours long is the caller's to bound (ferros_tz knows it); every face takes its bounds as given.
pub fn local_day(t: f64, utc_offset: f64) -> (f64, f64) {
    // 2000-01-01 00:00 UTC, a UTC midnight.
    let midnight = J2000_EAGLE - DAY / 2.;
    let k = floor((t + utc_offset - midnight) / DAY);
    let start = midnight + k * DAY - utc_offset;
    (start, start + DAY)
}

/// The offset of local mean solar time at `place`, seconds east of UTC: a day for a place with no zone (its midnight is the mean sun's).
pub fn mean_solar_offset(place: Place) -> f64 {
    place.lon / TAU * DAY
}

/// An altitude curve over a span: `CURVE` samples evenly from its start to its end (both ends sampled), the altitude and its rate at each, so a drawer can join them with cubic Hermite pieces that are C¹ and pass thru every sample at its true slope.
#[derive(Clone, Copy, Debug)]
pub struct Curve {
    pub start: f64,
    /// Seconds between samples.
    pub step: f64,
    /// Altitudes, radians.
    pub alt: [f32; CURVE],
    /// Rates in radians per sample step, as a Hermite piece over one step wants its tangents.
    pub rate: [f32; CURVE],
    /// The lowest and highest sample, for the scale.
    pub lo: f32,
    pub hi: f32,
}

impl Curve {
    fn of<F: Fn(f64) -> f64>(f: F, start: f64, end: f64) -> Curve {
        let step = (end - start) / (CURVE - 1) as f64;
        let mut c = Curve { start, step, alt: [0.; CURVE], rate: [0.; CURVE], lo: f32::MAX, hi: f32::MIN };
        for k in 0..CURVE {
            let t = start + k as f64 * step;
            let a = f(t) as f32;
            c.alt[k] = a;
            c.rate[k] = ((f(t + RATE_STEP) - f(t - RATE_STEP)) / (2. * RATE_STEP) * step) as f32;
            // The range the samples span, for the scale (a reduction, not a clamp).
            c.lo = c.lo.min(a);
            c.hi = c.hi.max(a);
        }
        c
    }
}

/// What the sun puck shows for a day at a place.
#[derive(Clone, Copy, Debug)]
pub struct SunDay {
    pub start: f64,
    pub end: f64,
    /// Solar noon: the upper transit nearest the day's middle, and the sun's altitude then.
    pub noon: f64,
    pub noon_altitude: f64,
    /// Sunrise and sunset ([`SUNRISE`]).
    pub sun: Pass,
    /// Dawn (rise) and dusk (set) of each twilight.
    pub civil: Pass,
    pub nautical: Pass,
    pub astronomical: Pass,
    /// The sun between −4° and +6°, and between −6° and −4°; [`Span::side`] against `noon` tells morning from evening.
    pub golden: Bands,
    pub blue: Bands,
    /// The sun's geometric altitude over the day.
    pub curve: Curve,
}

impl SunDay {
    /// The sun over [start, end) at `place`.
    pub fn new(start: f64, end: f64, place: Place) -> SunDay {
        let noon = sun_transit_near(0.5 * (start + end), place);
        SunDay {
            start,
            end,
            noon,
            noon_altitude: sun_altitude(noon, place),
            sun: sun_pass(start, end, place, SUNRISE),
            civil: sun_pass(start, end, place, CIVIL),
            nautical: sun_pass(start, end, place, NAUTICAL),
            astronomical: sun_pass(start, end, place, ASTRONOMICAL),
            golden: Bands::of(start, end, place, GOLDEN_LOW, GOLDEN_HIGH),
            blue: Bands::of(start, end, place, CIVIL, GOLDEN_LOW),
            curve: Curve::of(|t| sun_altitude(t, place), start, end),
        }
    }

    /// The local day around Eagle second `t`, for a zone `utc_offset` seconds east of UTC.
    pub fn around(t: f64, place: Place, utc_offset: f64) -> SunDay {
        let (start, end) = local_day(t, utc_offset);
        SunDay::new(start, end, place)
    }
}

/// What the moon puck shows for a day at a place and a moment in it.
#[derive(Clone, Copy, Debug)]
pub struct MoonDay {
    pub start: f64,
    pub end: f64,
    /// Moonrise and moonset (Meeus chapter 15's h0).
    pub moon: Pass,
    /// The upper transit nearest the day's middle when it falls in the day, and the topocentric altitude then.
    pub transit: Option<f64>,
    pub transit_altitude: f64,
    /// The topocentric altitude at which the rise and set happen, about the day's middle (−0.5667° less the semidiameter), to draw the horizon line on the curve.
    pub horizon: f64,
    /// The moment the face was made, and the moon's light then.
    pub made: f64,
    pub light: Illumination,
    /// The next principal phase after `made`.
    pub next_phase: Quarter,
    pub next_phase_at: f64,
    /// The moon's topocentric altitude over the day.
    pub curve: Curve,
}

impl MoonDay {
    /// The moon over [start, end) at `place`, its light at `now`.
    pub fn new(start: f64, end: f64, place: Place, now: f64) -> MoonDay {
        let mid = 0.5 * (start + end);
        let tr = moon_transit_near(mid, place);
        let transit = if tr >= start && tr < end { Some(tr) } else { None };
        let m = Moon::at(mid);
        // The geocentric h0 less the parallax of an object at that altitude (π cos h0, cos h0 = 1 to 10^-4) is the topocentric one.
        let horizon = 0.7275 * m.parallax - MOON_REFRACTION - m.parallax;
        let (next_phase, next_phase_at) = next_phase(now);
        MoonDay {
            start,
            end,
            moon: moon_pass(start, end, place),
            transit,
            transit_altitude: moon_altitude(tr, place),
            horizon,
            made: now,
            light: Illumination::at(now, place),
            next_phase,
            next_phase_at,
            curve: Curve::of(|t| moon_altitude(t, place), start, end),
        }
    }

    /// The local day around Eagle second `t`, for a zone `utc_offset` seconds east of UTC.
    pub fn around(t: f64, place: Place, utc_offset: f64) -> MoonDay {
        let (start, end) = local_day(t, utc_offset);
        MoonDay::new(start, end, place, t)
    }
}

/// A crossing within this long after a wake is the one woken for: one second, a hundred times the bisection's resolution.
const MARGIN: f64 = 1.;

/// The earliest moment after Eagle second `t` at which anything the sun and moon faces show changes: a crossing of any of the sun's levels (astronomical, nautical and civil twilight, golden and blue hour's edges, rise and set), solar noon (the sun turns from rising to setting), moonrise or moonset, the next principal phase, or `day_end` (the day's events give way to the next day's), whichever comes first.
/// The pucks wake only at these; between them the drawer moves "now" along the curves on its own clock.
pub fn next_deadline(t: f64, place: Place, day_end: f64) -> f64 {
    let from = t + MARGIN;
    let mut best = day_end;
    let noon = sun_transit_after(from, place);
    if noon < best {
        best = noon;
    }
    let (_, phase) = next_phase(from);
    if phase < best {
        best = phase;
    }
    // One pass over the span, the sun computed once per sample for all six levels.
    let levels = |x: f64| -> ([f64; 6], f64) {
        let h = sun_altitude(x, place);
        let mut v = [0.; 6];
        for j in 0..6 {
            v[j] = h - SUN_LEVELS[j];
        }
        (v, Moon::at(x).horizon(place))
    };
    let mut a = from;
    let (mut sa, mut ma) = levels(a);
    while a < best {
        let b = a + STEP;
        let (sb, mb) = levels(b);
        for j in 0..6 {
            if (sa[j] > 0.) != (sb[j] > 0.) {
                let c = bisect(&|x: f64| sun_altitude(x, place) - SUN_LEVELS[j], a, b, sb[j] > 0.);
                if c >= from && c < best {
                    best = c;
                }
            }
        }
        if (ma > 0.) != (mb > 0.) {
            let c = bisect(&|x: f64| Moon::at(x).horizon(place), a, b, mb > 0.);
            if c >= from && c < best {
                best = c;
            }
        }
        a = b;
        sa = sb;
        ma = mb;
    }
    best
}

#[cfg(test)]
mod tests;
