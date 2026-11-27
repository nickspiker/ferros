//! The compass puck's engine.
//!
//! Three parts, each pure math with no allocation:
//! 0. The World Magnetic Model WMM2025, so the puck knows how far magnetic north sits from true north where the device is (declination) and how strong the field there should be.
//! 1. A tilt-compensated heading from the magnetometer and gravity, so the device need not be held flat.
//! 2. A hard-iron calibration, so the device's own magnetised parts stop dragging the needle, with an honest measure of whether the person has turned the device enough yet.
//!
//! The device frame is Android's: x to the right of the screen, y up the screen, z out of the screen.

#![no_std]

use libm::{atan2, cos, floor, sin, sqrt};

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// Where the Eagle epoch (1969-07-20 20:17:40 UTC) falls in POSIX seconds, as in ferros_tide and vsf, repeated so the engine needs neither.
pub const EAGLE_EPOCH_UNIX_SECS: i64 = -14_182_940;
/// Seconds in a POSIX day (a published constant, not a knob).
const DAY: f64 = 86_400.;

/// Days from 1970-01-01 to a civil date (proleptic Gregorian), Howard Hinnant's `days_from_civil`.
/// Used to find where a year starts so the model gets its decimal year.
pub const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil year holding a day count from 1970-01-01 (the year half of Hinnant's `civil_from_days`).
const fn year_from_days(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400;
    if month <= 2 { y + 1 } else { y }
}

/// Eagle seconds at a civil UTC date and time of day (seconds), counted as POSIX counts them.
pub fn eagle_from_civil(year: i64, month: u32, day: u32, secs_of_day: f64) -> f64 {
    (days_from_civil(year, month, day) * 86_400 - EAGLE_EPOCH_UNIX_SECS) as f64 + secs_of_day
}

/// The decimal year the model is indexed by (2025.5 is halfway thru 2025), from Eagle seconds.
/// A leap year's fraction divides by 366 days, as the WMM software does.
pub fn decimal_year(eagle_secs: f64) -> f64 {
    let unix = eagle_secs + EAGLE_EPOCH_UNIX_SECS as f64;
    let year = year_from_days(floor(unix / DAY) as i64);
    let start = days_from_civil(year, 1, 1);
    let length = days_from_civil(year + 1, 1, 1) - start;
    year as f64 + (unix - start as f64 * DAY) / (length as f64 * DAY)
}

// ---------------------------------------------------------------------------
// The World Magnetic Model
// ---------------------------------------------------------------------------

/// One Gauss coefficient pair of degree n and order m, in nT, with its secular variation in nT a year.
#[derive(Clone, Copy, Debug)]
pub struct Coef {
    pub n: u8,
    pub m: u8,
    pub g: f64,
    pub h: f64,
    pub g_dot: f64,
    pub h_dot: f64,
}

/// The model's epoch (decimal year): the coefficients are the field at the start of 2025.
pub const WMM_EPOCH: f64 = 2025.;
/// The model is published as valid from its epoch until 2030.0; outside that the secular variation is an extrapolation NOAA does not stand behind.
pub const WMM_VALID_UNTIL: f64 = 2030.;
/// The model's degree.
const DEGREE: usize = 12;
/// Entries in a triangular (n, m) table of degree 12, including n = 0.
const TRIANGLE: usize = (DEGREE + 1) * (DEGREE + 2) / 2;
/// The geomagnetic reference radius, 6371.2 km (WMM2025 report, published).
const A_REF: f64 = 6_371_200.;
/// WGS84 semi-major axis in metres (published).
const WGS84_A: f64 = 6_378_137.;
/// WGS84 inverse flattening (published).
const WGS84_INV_F: f64 = 298.257_223_563;

/// WMM2025 Gauss coefficients, degree and order 12, transcribed from the official `WMM.COF` (header "2025.0 WMM-2025 11/13/2024", BLAKE3 9d0971ab5f303f4076dfc20b5e24339eac33eeaba97c8063c47482f2f710f267) in `WMM2025COF.zip` from NOAA NCEI, <https://www.ncei.noaa.gov/products/world-magnetic-model/wmm-coefficients>.
/// The model is by NOAA NCEI and the British Geological Survey, a work of the US government in the public domain.
/// Columns as published: n, m, g, h (nT), g dot, h dot (nT a year).
pub const WMM2025: [Coef; 90] = [
    Coef { n: 1, m: 0, g: -29351.8, h: 0., g_dot: 12., h_dot: 0. },
    Coef { n: 1, m: 1, g: -1410.8, h: 4545.4, g_dot: 9.7, h_dot: -21.5 },
    Coef { n: 2, m: 0, g: -2556.6, h: 0., g_dot: -11.6, h_dot: 0. },
    Coef { n: 2, m: 1, g: 2951.1, h: -3133.6, g_dot: -5.2, h_dot: -27.7 },
    Coef { n: 2, m: 2, g: 1649.3, h: -815.1, g_dot: -8., h_dot: -12.1 },
    Coef { n: 3, m: 0, g: 1361., h: 0., g_dot: -1.3, h_dot: 0. },
    Coef { n: 3, m: 1, g: -2404.1, h: -56.6, g_dot: -4.2, h_dot: 4. },
    Coef { n: 3, m: 2, g: 1243.8, h: 237.5, g_dot: 0.4, h_dot: -0.3 },
    Coef { n: 3, m: 3, g: 453.6, h: -549.5, g_dot: -15.6, h_dot: -4.1 },
    Coef { n: 4, m: 0, g: 895., h: 0., g_dot: -1.6, h_dot: 0. },
    Coef { n: 4, m: 1, g: 799.5, h: 278.6, g_dot: -2.4, h_dot: -1.1 },
    Coef { n: 4, m: 2, g: 55.7, h: -133.9, g_dot: -6., h_dot: 4.1 },
    Coef { n: 4, m: 3, g: -281.1, h: 212., g_dot: 5.6, h_dot: 1.6 },
    Coef { n: 4, m: 4, g: 12.1, h: -375.6, g_dot: -7., h_dot: -4.4 },
    Coef { n: 5, m: 0, g: -233.2, h: 0., g_dot: 0.6, h_dot: 0. },
    Coef { n: 5, m: 1, g: 368.9, h: 45.4, g_dot: 1.4, h_dot: -0.5 },
    Coef { n: 5, m: 2, g: 187.2, h: 220.2, g_dot: 0., h_dot: 2.2 },
    Coef { n: 5, m: 3, g: -138.7, h: -122.9, g_dot: 0.6, h_dot: 0.4 },
    Coef { n: 5, m: 4, g: -142., h: 43., g_dot: 2.2, h_dot: 1.7 },
    Coef { n: 5, m: 5, g: 20.9, h: 106.1, g_dot: 0.9, h_dot: 1.9 },
    Coef { n: 6, m: 0, g: 64.4, h: 0., g_dot: -0.2, h_dot: 0. },
    Coef { n: 6, m: 1, g: 63.8, h: -18.4, g_dot: -0.4, h_dot: 0.3 },
    Coef { n: 6, m: 2, g: 76.9, h: 16.8, g_dot: 0.9, h_dot: -1.6 },
    Coef { n: 6, m: 3, g: -115.7, h: 48.8, g_dot: 1.2, h_dot: -0.4 },
    Coef { n: 6, m: 4, g: -40.9, h: -59.8, g_dot: -0.9, h_dot: 0.9 },
    Coef { n: 6, m: 5, g: 14.9, h: 10.9, g_dot: 0.3, h_dot: 0.7 },
    Coef { n: 6, m: 6, g: -60.7, h: 72.7, g_dot: 0.9, h_dot: 0.9 },
    Coef { n: 7, m: 0, g: 79.5, h: 0., g_dot: -0., h_dot: 0. },
    Coef { n: 7, m: 1, g: -77., h: -48.9, g_dot: -0.1, h_dot: 0.6 },
    Coef { n: 7, m: 2, g: -8.8, h: -14.4, g_dot: -0.1, h_dot: 0.5 },
    Coef { n: 7, m: 3, g: 59.3, h: -1., g_dot: 0.5, h_dot: -0.8 },
    Coef { n: 7, m: 4, g: 15.8, h: 23.4, g_dot: -0.1, h_dot: 0. },
    Coef { n: 7, m: 5, g: 2.5, h: -7.4, g_dot: -0.8, h_dot: -1. },
    Coef { n: 7, m: 6, g: -11.1, h: -25.1, g_dot: -0.8, h_dot: 0.6 },
    Coef { n: 7, m: 7, g: 14.2, h: -2.3, g_dot: 0.8, h_dot: -0.2 },
    Coef { n: 8, m: 0, g: 23.2, h: 0., g_dot: -0.1, h_dot: 0. },
    Coef { n: 8, m: 1, g: 10.8, h: 7.1, g_dot: 0.2, h_dot: -0.2 },
    Coef { n: 8, m: 2, g: -17.5, h: -12.6, g_dot: 0., h_dot: 0.5 },
    Coef { n: 8, m: 3, g: 2., h: 11.4, g_dot: 0.5, h_dot: -0.4 },
    Coef { n: 8, m: 4, g: -21.7, h: -9.7, g_dot: -0.1, h_dot: 0.4 },
    Coef { n: 8, m: 5, g: 16.9, h: 12.7, g_dot: 0.3, h_dot: -0.5 },
    Coef { n: 8, m: 6, g: 15., h: 0.7, g_dot: 0.2, h_dot: -0.6 },
    Coef { n: 8, m: 7, g: -16.8, h: -5.2, g_dot: -0., h_dot: 0.3 },
    Coef { n: 8, m: 8, g: 0.9, h: 3.9, g_dot: 0.2, h_dot: 0.2 },
    Coef { n: 9, m: 0, g: 4.6, h: 0., g_dot: -0., h_dot: 0. },
    Coef { n: 9, m: 1, g: 7.8, h: -24.8, g_dot: -0.1, h_dot: -0.3 },
    Coef { n: 9, m: 2, g: 3., h: 12.2, g_dot: 0.1, h_dot: 0.3 },
    Coef { n: 9, m: 3, g: -0.2, h: 8.3, g_dot: 0.3, h_dot: -0.3 },
    Coef { n: 9, m: 4, g: -2.5, h: -3.3, g_dot: -0.3, h_dot: 0.3 },
    Coef { n: 9, m: 5, g: -13.1, h: -5.2, g_dot: 0., h_dot: 0.2 },
    Coef { n: 9, m: 6, g: 2.4, h: 7.2, g_dot: 0.3, h_dot: -0.1 },
    Coef { n: 9, m: 7, g: 8.6, h: -0.6, g_dot: -0.1, h_dot: -0.2 },
    Coef { n: 9, m: 8, g: -8.7, h: 0.8, g_dot: 0.1, h_dot: 0.4 },
    Coef { n: 9, m: 9, g: -12.9, h: 10., g_dot: -0.1, h_dot: 0.1 },
    Coef { n: 10, m: 0, g: -1.3, h: 0., g_dot: 0.1, h_dot: 0. },
    Coef { n: 10, m: 1, g: -6.4, h: 3.3, g_dot: 0., h_dot: 0. },
    Coef { n: 10, m: 2, g: 0.2, h: 0., g_dot: 0.1, h_dot: -0. },
    Coef { n: 10, m: 3, g: 2., h: 2.4, g_dot: 0.1, h_dot: -0.2 },
    Coef { n: 10, m: 4, g: -1., h: 5.3, g_dot: -0., h_dot: 0.1 },
    Coef { n: 10, m: 5, g: -0.6, h: -9.1, g_dot: -0.3, h_dot: -0.1 },
    Coef { n: 10, m: 6, g: -0.9, h: 0.4, g_dot: 0., h_dot: 0.1 },
    Coef { n: 10, m: 7, g: 1.5, h: -4.2, g_dot: -0.1, h_dot: 0. },
    Coef { n: 10, m: 8, g: 0.9, h: -3.8, g_dot: -0.1, h_dot: -0.1 },
    Coef { n: 10, m: 9, g: -2.7, h: 0.9, g_dot: -0., h_dot: 0.2 },
    Coef { n: 10, m: 10, g: -3.9, h: -9.1, g_dot: -0., h_dot: -0. },
    Coef { n: 11, m: 0, g: 2.9, h: 0., g_dot: 0., h_dot: 0. },
    Coef { n: 11, m: 1, g: -1.5, h: 0., g_dot: -0., h_dot: -0. },
    Coef { n: 11, m: 2, g: -2.5, h: 2.9, g_dot: 0., h_dot: 0.1 },
    Coef { n: 11, m: 3, g: 2.4, h: -0.6, g_dot: 0., h_dot: -0. },
    Coef { n: 11, m: 4, g: -0.6, h: 0.2, g_dot: 0., h_dot: 0.1 },
    Coef { n: 11, m: 5, g: -0.1, h: 0.5, g_dot: -0.1, h_dot: -0. },
    Coef { n: 11, m: 6, g: -0.6, h: -0.3, g_dot: 0., h_dot: -0. },
    Coef { n: 11, m: 7, g: -0.1, h: -1.2, g_dot: -0., h_dot: 0.1 },
    Coef { n: 11, m: 8, g: 1.1, h: -1.7, g_dot: -0.1, h_dot: -0. },
    Coef { n: 11, m: 9, g: -1., h: -2.9, g_dot: -0.1, h_dot: 0. },
    Coef { n: 11, m: 10, g: -0.2, h: -1.8, g_dot: -0.1, h_dot: 0. },
    Coef { n: 11, m: 11, g: 2.6, h: -2.3, g_dot: -0.1, h_dot: 0. },
    Coef { n: 12, m: 0, g: -2., h: 0., g_dot: 0., h_dot: 0. },
    Coef { n: 12, m: 1, g: -0.2, h: -1.3, g_dot: 0., h_dot: -0. },
    Coef { n: 12, m: 2, g: 0.3, h: 0.7, g_dot: -0., h_dot: 0. },
    Coef { n: 12, m: 3, g: 1.2, h: 1., g_dot: -0., h_dot: -0.1 },
    Coef { n: 12, m: 4, g: -1.3, h: -1.4, g_dot: -0., h_dot: 0.1 },
    Coef { n: 12, m: 5, g: 0.6, h: -0., g_dot: -0., h_dot: -0. },
    Coef { n: 12, m: 6, g: 0.6, h: 0.6, g_dot: 0.1, h_dot: -0. },
    Coef { n: 12, m: 7, g: 0.5, h: -0.1, g_dot: -0., h_dot: -0. },
    Coef { n: 12, m: 8, g: -0.1, h: 0.8, g_dot: 0., h_dot: 0. },
    Coef { n: 12, m: 9, g: -0.4, h: 0.1, g_dot: 0., h_dot: -0. },
    Coef { n: 12, m: 10, g: -0.2, h: -1., g_dot: -0.1, h_dot: -0. },
    Coef { n: 12, m: 11, g: -1.3, h: 0.1, g_dot: -0., h_dot: 0. },
    Coef { n: 12, m: 12, g: -0.7, h: 0.2, g_dot: -0.1, h_dot: -0.1 },
];

/// The geomagnetic field at a place and time, in the geodetic frame (north, east, down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    /// North component, nT.
    pub x: f64,
    /// East component, nT.
    pub y: f64,
    /// Down component, nT.
    pub z: f64,
    /// Horizontal intensity, nT.
    pub h: f64,
    /// Total intensity, nT.
    pub f: f64,
    /// Declination: the angle from true north to magnetic north, radians, east positive.
    pub d: f64,
    /// Inclination: the angle of the field below the horizontal, radians, down positive.
    pub i: f64,
}

/// The field at a geodetic latitude and longitude (radians, east positive), height above the WGS84 ellipsoid (metres), at an Eagle second.
pub fn field(lat_rad: f64, lon_rad: f64, height_m: f64, eagle_secs: f64) -> Field {
    field_at_year(lat_rad, lon_rad, height_m, decimal_year(eagle_secs))
}

/// The magnetic declination in radians, east positive: add it to a magnetic heading to get a true one.
pub fn declination(lat_rad: f64, lon_rad: f64, height_m: f64, eagle_secs: f64) -> f64 {
    field(lat_rad, lon_rad, height_m, eagle_secs).d
}

/// Index of (n, m) in a triangular table.
const fn tri(n: usize, m: usize) -> usize {
    n * (n + 1) / 2 + m
}

/// The field at a decimal year, following the WMM2025 technical report section 1.2 step by step.
/// The year is not limited to the validity window: the caller can test it against `WMM_EPOCH` and `WMM_VALID_UNTIL` and say so.
pub fn field_at_year(lat_rad: f64, lon_rad: f64, height_m: f64, year: f64) -> Field {
    // Step 0: geodetic to geocentric spherical coordinates on WGS84.
    let f = 1. / WGS84_INV_F;
    let e2 = f * (2. - f);
    let (sin_lat, cos_lat) = (sin(lat_rad), cos(lat_rad));
    // The prime vertical radius of curvature.
    let rc = WGS84_A / sqrt(1. - e2 * sin_lat * sin_lat);
    let p = (rc + height_m) * cos_lat;
    let zc = (rc * (1. - e2) + height_m) * sin_lat;
    let r = sqrt(p * p + zc * zc);
    let lat_c = atan2(zc, p);
    // x and z of the Legendre recursion: sin and cos of the geocentric latitude.
    let (x, z) = (sin(lat_c), cos(lat_c));

    // Step 1: Gauss-normalised associated Legendre functions of sin(lat_c) and their derivatives in colatitude.
    // The recursion and the Schmidt factors are those of the WMM software's MAG_PcupLow.
    let mut pn = [0f64; TRIANGLE];
    let mut dp = [0f64; TRIANGLE];
    pn[0] = 1.;
    for n in 1..=DEGREE {
        for m in 0..=n {
            let k = tri(n, m);
            if n == m {
                let j = tri(n - 1, m - 1);
                pn[k] = z * pn[j];
                dp[k] = z * dp[j] + x * pn[j];
            } else if m + 1 == n {
                // The (n - 2, m) term does not exist here and its weight is zero anyway: (n - 1)^2 - m^2 = 0.
                let j = tri(n - 1, m);
                pn[k] = x * pn[j];
                dp[k] = x * dp[j] - z * pn[j];
            } else {
                let j1 = tri(n - 1, m);
                let j2 = tri(n - 2, m);
                let w = ((n - 1) * (n - 1) - m * m) as f64 / ((2 * n - 1) * (2 * n - 3)) as f64;
                pn[k] = x * pn[j1] - w * pn[j2];
                dp[k] = x * dp[j1] - z * pn[j1] - w * dp[j2];
            }
        }
    }
    // Schmidt semi-normalisation, and the derivative turned from colatitude to latitude (the sign flip).
    let mut s = 1.;
    for n in 1..=DEGREE {
        // s(n, 0) = s(n - 1, 0) (2n - 1) / n.
        s *= (2 * n - 1) as f64 / n as f64;
        let mut sm = s;
        for m in 0..=n {
            if m > 0 {
                let twice = if m == 1 { 2 } else { 1 };
                sm *= sqrt(((n - m + 1) * twice) as f64 / (n + m) as f64);
            }
            let k = tri(n, m);
            pn[k] *= sm;
            dp[k] *= -sm;
        }
    }

    // Step 2: the coefficients at the requested time.
    let dt = year - WMM_EPOCH;
    // (a / r)^(n + 2) for every degree, and cos, sin of m times the longitude.
    let mut ar = [0f64; DEGREE + 1];
    let ratio = A_REF / r;
    let mut pow = ratio * ratio;
    for slot in ar.iter_mut() {
        *slot = pow;
        pow *= ratio;
    }
    let mut cm = [0f64; DEGREE + 1];
    let mut sm = [0f64; DEGREE + 1];
    for m in 0..=DEGREE {
        cm[m] = cos(m as f64 * lon_rad);
        sm[m] = sin(m as f64 * lon_rad);
    }

    // Step 3: the field in geocentric spherical components (report equations 10 to 12).
    let (mut xp, mut yp, mut zp) = (0., 0., 0.);
    for c in WMM2025.iter() {
        let (n, m) = (c.n as usize, c.m as usize);
        let g = c.g + dt * c.g_dot;
        let h = c.h + dt * c.h_dot;
        let k = tri(n, m);
        let t = g * cm[m] + h * sm[m];
        xp -= ar[n] * t * dp[k];
        yp += ar[n] * m as f64 * (g * sm[m] - h * cm[m]) * pn[k];
        zp -= ar[n] * (n + 1) as f64 * t * pn[k];
    }
    // Every term with m > 0 carries a factor cos(lat_c)^m, so this division is finite; cos of an f64 latitude is never exactly zero because pi/2 is not an f64.
    yp /= z;

    // Step 4: rotate into the geodetic frame.
    let psi = lat_c - lat_rad;
    let (sp, cp) = (sin(psi), cos(psi));
    let bx = xp * cp - zp * sp;
    let bz = xp * sp + zp * cp;
    let by = yp;
    let h = sqrt(bx * bx + by * by);
    Field { x: bx, y: by, z: bz, h, f: sqrt(h * h + bz * bz), d: atan2(by, bx), i: atan2(bz, h) }
}

// ---------------------------------------------------------------------------
// Tilt-compensated heading
// ---------------------------------------------------------------------------

/// The device's y axis must lean at least this far from vertical (as the sine of its angle off vertical) for its heading to mean anything: about 3.6 degrees.
/// Nearer vertical the horizontal shadow of the top edge is so short that sensor noise swings it around the dial.
pub const TILT_LIMIT: f64 = 1. / (1 << 4) as f64;
/// The field must lean at least this far from gravity (as the sine of the angle between them) for east to be defined: about 0.9 degrees.
/// Only within about a degree of a magnetic dip pole (or with a dead sensor reading zero) does the field lose its horizontal part.
pub const FIELD_LIMIT: f64 = 1. / (1 << 6) as f64;

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn widen(v: [f32; 3]) -> [f64; 3] {
    [v[0] as f64, v[1] as f64, v[2] as f64]
}

/// Magnetic heading, radians clockwise from magnetic north in [0, 2 pi), of any direction fixed to the device.
/// `m` is the calibrated magnetometer and `g` the accelerometer (or gravity sensor), both in the device frame, any units.
/// `axis` is the device direction to report (it need not be unit length).
///
/// Derivation: the accelerometer at rest reads the reaction to gravity, so `g` points up.
/// The field points north and, away from the equator, also up or down; crossing it with up kills the vertical part and leaves a horizontal vector at right angles to north, which by the right-hand rule is east: east = m x g.
/// Then north = g x east, horizontal and at right angles to both.
/// The axis's heading is atan2 of its east and north parts, which needs no tilt angles at all.
///
/// Returns None when the axis is within `TILT_LIMIT` of vertical (its heading is undefined, not merely noisy) or the field is within `FIELD_LIMIT` of vertical (east is undefined).
pub fn heading_of(m: [f32; 3], g: [f32; 3], axis: [f32; 3]) -> Option<f64> {
    let (m, g, axis) = (widen(m), widen(g), widen(axis));
    let east = cross(m, g);
    let east_len = sqrt(dot(east, east));
    let mg = sqrt(dot(m, m) * dot(g, g));
    // |m x g| = |m| |g| sin(angle between them).
    if !(east_len > FIELD_LIMIT * mg) {
        return None;
    }
    let north = cross(g, east);
    // |g x east| = |g| |east| because they are at right angles.
    let g_len = sqrt(dot(g, g));
    let e = dot(east, axis) / east_len;
    let n = dot(north, axis) / (g_len * east_len);
    let axis_len = sqrt(dot(axis, axis));
    // sqrt(e^2 + n^2) is the length of the axis's horizontal shadow.
    if !(sqrt(e * e + n * n) > TILT_LIMIT * axis_len) {
        return None;
    }
    Some(wrap(atan2(e, n)))
}

/// Magnetic heading of the device's top edge (y axis), the needle a person expects when the device lies flat or leans back.
/// None when the device is held upright (y near vertical); use `heading_back` then.
pub fn heading(m: [f32; 3], g: [f32; 3]) -> Option<f64> {
    heading_of(m, g, [0., 1., 0.])
}

/// Magnetic heading of the way the back of the device faces (minus z), the direction the back camera looks.
/// This is the useful one when the device is held upright like a viewfinder; None when the device lies flat.
pub fn heading_back(m: [f32; 3], g: [f32; 3]) -> Option<f64> {
    heading_of(m, g, [0., 0., -1.])
}

/// An angle taken modulo a full turn into [0, 2 pi), core's stand-in for std's `rem_euclid` (same arithmetic: remainder, then add a turn when negative).
/// As with `rem_euclid`, an input a hair below zero can round to 2 pi itself, which is the same direction as 0.
fn wrap(a: f64) -> f64 {
    let r = libm::fmod(a, core::f64::consts::TAU);
    if r < 0. { r + core::f64::consts::TAU } else { r }
}

/// True heading from a magnetic heading and the declination, wrapped to [0, 2 pi).
/// The wrap is modular arithmetic on an angle, not a clamp: 361 degrees and 1 degree are the same direction.
pub fn true_heading(magnetic: f64, declination: f64) -> f64 {
    wrap(magnetic + declination)
}

// ---------------------------------------------------------------------------
// Hard-iron calibration
// ---------------------------------------------------------------------------

/// A hard-iron calibration: the constant field the device's own parts add to every reading, in microtesla, plain data the puck stores as VSF.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration {
    /// The offset to subtract, per axis of the device frame, microtesla.
    pub offset: [f32; 3],
    /// How many samples the offset was found from.
    pub samples: u32,
}

impl Calibration {
    /// No correction.
    pub const NONE: Calibration = Calibration { offset: [0.; 3], samples: 0 };

    /// A raw magnetometer reading (microtesla) with the device's own field taken out.
    pub fn apply(&self, raw: [f32; 3]) -> [f32; 3] {
        [raw[0] - self.offset[0], raw[1] - self.offset[1], raw[2] - self.offset[2]]
    }
}

/// Each axis should have swept at least this share of the full 2F a complete turn gives before the puck calls the calibration good: three quarters.
pub const COVERAGE_GOOD: f32 = 1. - 1. / (1 << 2) as f32;

/// A running hard-iron fit: per axis, the smallest and largest raw reading seen.
/// Turning the device thru every orientation sweeps each axis from offset - F to offset + F, so the middle of the range is the offset.
#[derive(Clone, Copy, Debug)]
pub struct HardIron {
    lo: [f32; 3],
    hi: [f32; 3],
    samples: u32,
}

impl HardIron {
    pub const fn new() -> HardIron {
        HardIron { lo: [0.; 3], hi: [0.; 3], samples: 0 }
    }

    /// Fold in one raw magnetometer reading, microtesla.
    /// The running smallest and largest are the fit itself, not a limit on the reading.
    /// One wild sample widens the range for good; `new` starts over.
    pub fn add(&mut self, raw: [f32; 3]) {
        if self.samples == 0 {
            self.lo = raw;
            self.hi = raw;
        } else {
            for a in 0..3 {
                if raw[a] < self.lo[a] {
                    self.lo[a] = raw[a];
                }
                if raw[a] > self.hi[a] {
                    self.hi[a] = raw[a];
                }
            }
        }
        self.samples += 1;
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// The calibration so far: the centre of each axis's range.
    pub fn calibration(&self) -> Calibration {
        let mut offset = [0f32; 3];
        for a in 0..3 {
            offset[a] = (self.hi[a] + self.lo[a]) * 0.5;
        }
        Calibration { offset, samples: self.samples }
    }

    /// How much of a full turn each axis has seen: its range against the 2F a complete sweep gives, where F is the field strength (microtesla) the WMM predicts here (`Field::f` / 1000).
    /// A value can pass 1 when soft iron stretches an axis; it is reported as measured.
    pub fn coverage(&self, field_ut: f32) -> [f32; 3] {
        let full = 2. * field_ut;
        [(self.hi[0] - self.lo[0]) / full, (self.hi[1] - self.lo[1]) / full, (self.hi[2] - self.lo[2]) / full]
    }

    /// Whether every axis has swept `COVERAGE_GOOD` of a full turn; while not, the puck says "turn the device to calibrate".
    pub fn is_good(&self, field_ut: f32) -> bool {
        let c = self.coverage(field_ut);
        c[0] >= COVERAGE_GOOD && c[1] >= COVERAGE_GOOD && c[2] >= COVERAGE_GOOD
    }
}

impl Default for HardIron {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
