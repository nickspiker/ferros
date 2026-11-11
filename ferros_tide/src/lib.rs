//! The tide engine (docs/PUCKS.md, the tide puck; owner, 2026-11-11: "tides on a puck").
//! A station's tide is the sum of its harmonic constituents, each a cosine at a speed the sky sets, with an amplitude and a phase the station's own record sets:
//!
//! h(t) = Z0 + Σ f_i H_i cos(V_i(t) + u_i(t) − G_i)
//!
//! - H_i and G_i are the station's (its published harmonic constants: amplitude, and the phase lag G referred to Greenwich, NOAA's `phase_GMT`).
//! - V_i(t) is the constituent's equilibrium argument at Greenwich, from the mean longitudes of the Moon (s), the Sun (h), the lunar perigee (p), the solar perigee (p1) and the mean sun's hour angle (T), by its Doodson numbers.
//! - f_i and u_i are its nodal factor and nodal angle: the slow (18.61 year) wobble of the Moon's orbit, from the longitude of the Moon's ascending node (N), by Schureman's formulas (Manual of Harmonic Analysis and Prediction of Tides, US Coast and Geodetic Survey Special Publication 98, 1958; the equations are cited by number where used).
//! - Z0 is mean sea level above the datum the heights are wanted in (MLLW for NOAA's published predictions).
//!
//! The 37 constituents are NOAA CO-OPS's, in NOAA's order ([`CONSTITUENTS`]), so a station's published table maps one to one.
//! The astronomical arguments are evaluated at the moment asked (no year-start V0 and mid-year f and u as printed tables use), so the curve is smooth (C∞) across any boundary: the puck draws it as a continuous function.
//!
//! Time is Eagle time thruout (seconds since 1969-07-20 20:17:40 UTC, counted as vsf's `eagle_time_now` counts them today: POSIX seconds from that label, [`EAGLE_EPOCH_UNIX_SECS`]); the engine converts to days from J2000 inside ([`days_j2000`]).
//! The arguments are taken in that UT scale; the half minute or so between it and the dynamical time the polynomials are strictly in moves M2's phase by under a third of a degree, well under the minute the published predictions are rounded to.
//!
//! [`Tide::extremes`] finds the highs and lows by sampling h'(t) and bisecting each change of sign; [`Tide::next`] the next high and the next low after a moment.
//!
//! Host-tested against NOAA CO-OPS's published highs and lows for 2026-11-01 to 07 at four stations (San Francisco, The Battery, Seattle, Honolulu: semidiurnal to strongly mixed), src/tests.rs: as the printed tables take the constituents ([`Frame::printed`]) every time within 2.1 minutes and every height within 1.3 cm (means 0.3 to 0.7 min, 0.2 to 0.8 cm); as the puck takes them, live ([`Frame::at`]), within 3.3 minutes and 3.0 cm (Seattle's large diurnal tide, where the year-middle f and u differ most from the moment's).
#![no_std]

use libm::{acos, atan, atan2, cos, floor, sin, sqrt, tan};

/// Where the Eagle epoch (1969-07-20 20:17:40 UTC) falls in POSIX seconds: vsf's `EAGLE_EPOCH_UNIX_SECS`, repeated here so the engine needs no vsf.
pub const EAGLE_EPOCH_UNIX_SECS: i64 = -14_182_940;
/// Oscillations of the hydrogen line per Eagle second (vsf's `OSCILLATIONS_PER_SECOND`): an `e6` Eagle stamp counts these.
pub const OSCILLATIONS_PER_SECOND: i64 = 1_420_407_826;
/// J2000.0 (2000-01-01 12:00 UTC) in Eagle seconds: POSIX 946 728 000 less the epoch.
pub const J2000_EAGLE: f64 = 946_728_000. - EAGLE_EPOCH_UNIX_SECS as f64;
/// Seconds in a day (a published constant, not a knob).
const DAY: f64 = 86_400.;
/// Days in a Julian century.
const CENTURY: f64 = 36_525.;

/// Eagle seconds from an `e6` Eagle stamp (oscillations).
pub fn eagle_secs(oscillations: i64) -> f64 {
    let whole = oscillations.div_euclid(OSCILLATIONS_PER_SECOND);
    let rest = oscillations.rem_euclid(OSCILLATIONS_PER_SECOND);
    whole as f64 + rest as f64 / OSCILLATIONS_PER_SECOND as f64
}

/// An `e6` Eagle stamp (oscillations) from Eagle seconds.
pub fn eagle_oscillations(secs: f64) -> i64 {
    let whole = floor(secs);
    whole as i64 * OSCILLATIONS_PER_SECOND + ((secs - whole) * OSCILLATIONS_PER_SECOND as f64) as i64
}

/// Eagle seconds from POSIX UTC seconds (the device's `utc_time`).
pub fn eagle_from_unix(unix_secs: i64) -> f64 {
    (unix_secs - EAGLE_EPOCH_UNIX_SECS) as f64
}

/// Days from J2000.0 at Eagle second `t`.
pub fn days_j2000(t: f64) -> f64 {
    (t - J2000_EAGLE) / DAY
}

const D2R: f64 = core::f64::consts::PI / 180.;

/// The fundamental arguments (degrees) and their rates (degrees an hour), in the order the Doodson numbers take them: T (the mean sun's hour angle), s, h, p, N, p1.
#[derive(Clone, Copy, Debug)]
pub struct Astro {
    pub arg: [f64; 6],
    pub rate: [f64; 6],
}

/// A polynomial in Julian centuries from J2000 (degrees), and its rate (degrees a century).
fn poly(c: &[f64], tc: f64) -> (f64, f64) {
    let (mut v, mut d) = (0., 0.);
    let mut x = 1.;
    for (k, a) in c.iter().enumerate() {
        v += a * x;
        if k + 1 < c.len() {
            d += (k + 1) as f64 * c[k + 1] * x;
        }
        x *= tc;
    }
    (v, d)
}

// Mean longitudes, from Meeus (Astronomical Algorithms, 2nd ed., chapters 22 and 47) as pytides and XTide's congen take them.
/// The Moon's mean longitude s.
const S: [f64; 5] = [218.3164591, 481267.88134236, -0.0013268, 1. / 538841., -1. / 65194000.];
/// The Sun's mean longitude h.
const H: [f64; 3] = [280.46645, 36000.7697489, 0.0003032];
/// The lunar perigee p.
const P: [f64; 5] = [83.3532430, 4069.0137111, -0.0103238, -1. / 80053., 1. / 18999000.];
/// The longitude of the Moon's ascending node N.
const NODE: [f64; 5] = [125.0445550, -1934.1361849, 0.0020762, 1. / 467410., -1. / 60616000.];
/// The solar perigee p1.
const P1: [f64; 3] = [282.93734, 1.71945766667, 0.00045688889];
/// The obliquity of the ecliptic ω (Meeus 22.2).
const OMEGA: [f64; 4] = [23.4392911, -0.0130041667, -1.638e-7, 5.036e-7];
/// The inclination of the Moon's orbit to the ecliptic, i (Schureman's 5.145°).
const INCL: f64 = 5.145;

/// Reduce degrees to [0, 360).
fn turn(d: f64) -> f64 {
    d - 360. * floor(d / 360.)
}

impl Astro {
    /// The arguments at Eagle second `t`.
    pub fn at(t: f64) -> Astro {
        let d = days_j2000(t);
        let tc = d / CENTURY;
        // T: at noon UT the mean sun is on the Greenwich meridian (0°), at midnight 180°, 15° an hour.
        let tt = 360. * (d - floor(d));
        let per_hour = 1. / (CENTURY * 24.);
        let mut arg = [tt, 0., 0., 0., 0., 0.];
        let mut rate = [15., 0., 0., 0., 0., 0.];
        for (k, c) in [&S[..], &H[..], &P[..], &NODE[..], &P1[..]].iter().enumerate() {
            let (v, r) = poly(c, tc);
            arg[k + 1] = turn(v);
            rate[k + 1] = r * per_hour;
        }
        Astro { arg, rate }
    }

    /// The obliquity ω (degrees) at this moment's century.
    fn omega(t: f64) -> f64 {
        poly(&OMEGA, days_j2000(t) / CENTURY).0
    }
}

/// The node's angles (radians) that the nodal factors and angles are built from (Schureman §§ 46-50, 224, 232).
#[derive(Clone, Copy, Debug)]
pub struct Node {
    /// I, the inclination of the Moon's orbit to the equator.
    pub i: f64,
    /// ν, the right ascension of the lunar intersection.
    pub nu: f64,
    /// ξ, the longitude in the Moon's orbit of the lunar intersection.
    pub xi: f64,
    /// ν′, for K1 (Schureman 224).
    pub nup: f64,
    /// 2ν″, for K2 (Schureman 232), held as ν″.
    pub nupp: f64,
    /// P = p − ξ, for L2 and M1.
    pub pp: f64,
    /// ω and i, for the mean values the factors are divided by.
    pub omega: f64,
    pub incl: f64,
}

impl Node {
    pub fn at(t: f64, a: &Astro) -> Node {
        let n = a.arg[4] * D2R;
        let (omega, incl) = (Astro::omega(t) * D2R, INCL * D2R);
        // Schureman 191-193: I from the node; ν and ξ by Napier's analogies over the spherical triangle the equator, the ecliptic and the Moon's orbit make.
        let cos_i = cos(incl) * cos(omega) - sin(incl) * sin(omega) * cos(n);
        let i = acos(cos_i);
        let e1 = atan(cos(0.5 * (omega - incl)) / cos(0.5 * (omega + incl)) * tan(0.5 * n)) - 0.5 * n;
        let e2 = atan(sin(0.5 * (omega - incl)) / sin(0.5 * (omega + incl)) * tan(0.5 * n)) - 0.5 * n;
        let xi = -(e1 + e2);
        let nu = e1 - e2;
        // 224 and 232: the solar terms' constants 0.3347 and 0.0727 are Schureman's.
        let nup = atan2(sin(2. * i) * sin(nu), sin(2. * i) * cos(nu) + 0.3347);
        let nupp = 0.5 * atan2(sin(i) * sin(i) * sin(2. * nu), sin(i) * sin(i) * cos(2. * nu) + 0.0727);
        let pp = a.arg[3] * D2R - xi;
        Node { i, nu, xi, nup, nupp, pp, omega, incl }
    }

    /// The nodal angle u (radians) and factor f of a base constituent's kind.
    fn uf(&self, k: Kind) -> (f64, f64) {
        let Node { i, nu, xi, nup, nupp, pp, omega, incl } = *self;
        let (s, c) = (sin, cos);
        let c4 = |x: f64| {
            let v = c(0.5 * x);
            v * v * v * v
        };
        let f_m2 = c4(i) / (c4(omega) * c4(incl));
        let f_o1 = s(i) * c(0.5 * i) * c(0.5 * i) / (s(omega) * c(0.5 * omega) * c(0.5 * omega) * c4(incl));
        match k {
            Kind::One => (0., 1.),
            // 73, 65.
            Kind::Mm => (0., (2. / 3. - s(i) * s(i)) / ((2. / 3. - s(omega) * s(omega)) * (1. - 1.5 * s(incl) * s(incl)))),
            // 74, 66.
            Kind::Mf => (-2. * xi, s(i) * s(i) / (s(omega) * s(omega) * c4(incl))),
            // 75, 67.
            Kind::O1 => (2. * xi - nu, f_o1),
            // 76, 68.
            Kind::J1 => (-nu, s(2. * i) / (s(2. * omega) * (1. - 1.5 * s(incl) * s(incl)))),
            // 77, 69.
            Kind::Oo1 => (-2. * xi - nu, s(i) * s(0.5 * i) * s(0.5 * i) / (s(omega) * s(0.5 * omega) * s(0.5 * omega) * c4(incl))),
            // 78, 70.
            Kind::M2 => (2. * xi - 2. * nu, f_m2),
            // 227 (f), 226 (u = −ν′).
            Kind::K1 => (-nup, sqrt(0.8965 * s(2. * i) * s(2. * i) + 0.6001 * s(2. * i) * c(nu) + 0.1006)),
            // 213-215: R from P and I.
            Kind::L2 => {
                let t2 = tan(0.5 * i) * tan(0.5 * i);
                let r = atan(s(2. * pp) / (1. / (6. * t2) - c(2. * pp)));
                (2. * xi - 2. * nu - r, f_m2 * sqrt(1. - 12. * t2 * c(2. * pp) + 36. * t2 * t2))
            }
            // 235 (f), 232 (u = −2ν″).
            Kind::K2 => (-2. * nupp, sqrt(19.0444 * s(i) * s(i) * s(i) * s(i) + 2.7702 * s(i) * s(i) * c(2. * nu) + 0.0981)),
            // 195, 206, 207: Q from P and I.
            Kind::M1 => {
                // Q in P's own quadrant (Schureman 195 gives tan Q; a bare arctangent would jump half a turn each time P passes a right angle).
                let q = atan2((5. * c(i) - 1.) * s(pp), (7. * c(i) + 1.) * c(pp));
                let qa = sqrt(0.25 + 1.5 * c(i) * c(2. * pp) / sqrt(c(0.5 * i)) + 2.25 * c(i) * c(i) / c4(i));
                (xi - nu + q, f_o1 * qa)
            }
            // M3: the lunar terdiurnal, u = 3/2 u(M2), f = f(M2)^(3/2) (Schureman 149).
            Kind::M3 => (3. * xi - 3. * nu, f_m2 * sqrt(f_m2)),
        }
    }
}

/// How a base constituent's nodal correction is formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    One,
    Mm,
    Mf,
    O1,
    J1,
    Oo1,
    M2,
    K1,
    L2,
    K2,
    M1,
    M3,
}

/// A constituent: a base one by its Doodson numbers (on T, s, h, p, N, p1, and a multiple of 90°) and its nodal kind, or a compound of base ones (index into [`CONSTITUENTS`], multiple).
#[derive(Clone, Copy, Debug)]
enum Form {
    Base([i8; 7], Kind),
    Compound(&'static [(u8, i8)]),
}

/// One constituent: NOAA's name and how its argument is made.
#[derive(Clone, Copy, Debug)]
pub struct Constituent {
    pub name: &'static str,
    form: Form,
}

const fn base(name: &'static str, xdo: [i8; 7], k: Kind) -> Constituent {
    Constituent { name, form: Form::Base(xdo, k) }
}
const fn compound(name: &'static str, of: &'static [(u8, i8)]) -> Constituent {
    Constituent { name, form: Form::Compound(of) }
}

/// How many constituents.
pub const N: usize = 37;

// Indices of the base constituents compounds are made of.
const M2: u8 = 0;
const S2: u8 = 1;
const N2: u8 = 2;
const K1: u8 = 3;

/// NOAA CO-OPS's 37 constituents, in NOAA's order (the `number` in a station's harcon table, counted from 0 here). Doodson numbers and nodal kinds after Schureman's Table 2. The multiple of 90° each carries is where the sources part (pytides writes the diurnal ones with the opposite sign of Schureman's, and M1 is conventional rather than physical: Schureman §§ 126-127): the ones here are those NOAA's published predictions use, checked by `conventions_are_the_published_ones` (no quarter or half turn on any one constituent fits the four stations better). M1 carries a half turn with its Q kept in P's quadrant.
pub static CONSTITUENTS: [Constituent; N] = [
    base("M2", [2, -2, 2, 0, 0, 0, 0], Kind::M2),
    base("S2", [2, 0, 0, 0, 0, 0, 0], Kind::One),
    base("N2", [2, -3, 2, 1, 0, 0, 0], Kind::M2),
    base("K1", [1, 0, 1, 0, 0, 0, -1], Kind::K1),
    compound("M4", &[(M2, 2)]),
    base("O1", [1, -2, 1, 0, 0, 0, 1], Kind::O1),
    compound("M6", &[(M2, 3)]),
    compound("MK3", &[(M2, 1), (K1, 1)]),
    compound("S4", &[(S2, 2)]),
    compound("MN4", &[(M2, 1), (N2, 1)]),
    base("NU2", [2, -3, 4, -1, 0, 0, 0], Kind::M2),
    compound("S6", &[(S2, 3)]),
    base("MU2", [2, -4, 4, 0, 0, 0, 0], Kind::M2),
    base("2N2", [2, -4, 2, 2, 0, 0, 0], Kind::M2),
    base("OO1", [1, 2, 1, 0, 0, 0, -1], Kind::Oo1),
    base("LAM2", [2, -1, 0, 1, 0, 0, 2], Kind::M2),
    base("S1", [1, 0, 0, 0, 0, 0, 0], Kind::One),
    base("M1", [1, -1, 1, 1, 0, 0, 2], Kind::M1),
    base("J1", [1, 1, 1, -1, 0, 0, -1], Kind::J1),
    base("MM", [0, 1, 0, -1, 0, 0, 0], Kind::Mm),
    base("SSA", [0, 0, 2, 0, 0, 0, 0], Kind::One),
    base("SA", [0, 0, 1, 0, 0, 0, 0], Kind::One),
    compound("MSF", &[(S2, 1), (M2, -1)]),
    base("MF", [0, 2, 0, 0, 0, 0, 0], Kind::Mf),
    base("RHO", [1, -3, 3, -1, 0, 0, 1], Kind::O1),
    base("Q1", [1, -3, 1, 1, 0, 0, 1], Kind::O1),
    base("T2", [2, 0, -1, 0, 0, 1, 0], Kind::One),
    base("R2", [2, 0, 1, 0, 0, -1, 2], Kind::One),
    base("2Q1", [1, -4, 1, 2, 0, 0, 1], Kind::O1),
    base("P1", [1, 0, -1, 0, 0, 0, 1], Kind::One),
    compound("2SM2", &[(S2, 2), (M2, -1)]),
    base("M3", [3, -3, 3, 0, 0, 0, 0], Kind::M3),
    base("L2", [2, -1, 2, -1, 0, 0, 2], Kind::L2),
    compound("2MK3", &[(M2, 2), (K1, -1)]),
    base("K2", [2, 0, 2, 0, 0, 0, 0], Kind::K2),
    compound("M8", &[(M2, 4)]),
    compound("MS4", &[(M2, 1), (S2, 1)]),
];

/// A constituent's index by its name (NOAA's spelling, any case).
pub fn index_of(name: &str) -> Option<usize> {
    CONSTITUENTS.iter().position(|c| c.name.eq_ignore_ascii_case(name))
}

/// Every constituent's state at one moment: its argument V + u (radians), its speed (radians a second) and its nodal factor f.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// The moment (Eagle seconds).
    pub t: f64,
    pub arg: [f64; N],
    pub speed: [f64; N],
    pub f: [f64; N],
    /// The nodal angle u inside `arg` (radians), kept apart for [`Frame::printed`].
    pub u: [f64; N],
}

/// Days from 1970-01-01 to a civil (proleptic Gregorian) date (Hinnant's days_from_civil).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil UTC year a day from 1970-01-01 falls in (Hinnant's civil_from_days, the year alone).
fn year_of(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + (m <= 2) as i64
}

/// The UTC year Eagle second `t` falls in: its first and its last moment (Eagle seconds).
pub fn year_bounds(t: f64) -> (f64, f64) {
    let unix = floor(t) as i64 + EAGLE_EPOCH_UNIX_SECS;
    let y = year_of(unix.div_euclid(DAY as i64));
    let at = |y: i64| eagle_from_unix(days_from_civil(y, 1, 1) * DAY as i64);
    (at(y), at(y + 1))
}

impl Frame {
    /// The constituents at Eagle second `t`.
    pub fn at(t: f64) -> Frame {
        let a = Astro::at(t);
        let node = Node::at(t, &a);
        let mut fr = Frame { t, arg: [0.; N], speed: [0.; N], f: [1.; N], u: [0.; N] };
        // Base ones first (compounds refer only to base ones).
        for (k, c) in CONSTITUENTS.iter().enumerate() {
            if let Form::Base(x, kind) = c.form {
                let mut v = 90. * x[6] as f64;
                let mut w = 0.;
                for j in 0..6 {
                    v += x[j] as f64 * a.arg[j];
                    w += x[j] as f64 * a.rate[j];
                }
                let (u, f) = node.uf(kind);
                fr.arg[k] = turn(v) * D2R + u;
                fr.speed[k] = w * D2R / 3600.;
                fr.f[k] = f;
                fr.u[k] = u;
            }
        }
        for (k, c) in CONSTITUENTS.iter().enumerate() {
            if let Form::Compound(of) = c.form {
                let (mut v, mut w, mut f, mut u) = (0., 0., 1., 0.);
                for &(b, m) in of {
                    v += m as f64 * fr.arg[b as usize];
                    w += m as f64 * fr.speed[b as usize];
                    u += m as f64 * fr.u[b as usize];
                    for _ in 0..m.unsigned_abs() {
                        f *= fr.f[b as usize];
                    }
                }
                fr.arg[k] = v;
                fr.speed[k] = w;
                fr.f[k] = f;
                fr.u[k] = u;
            }
        }
        fr
    }

    /// The constituents as printed tide tables take them (NOAA's, XTide's): the equilibrium argument V0 at the start of the UTC year `t` falls in, run on at its speed, and the nodal f and u held at the year's middle.
    /// The curve this makes steps at each new year (by a centimetre or so where the diurnal tide is large), so the puck draws from [`Frame::at`]; this one is here to check the engine against the published predictions to the millimetre.
    pub fn printed(t: f64) -> Frame {
        let (y0, y1) = year_bounds(t);
        let start = Frame::at(y0);
        let mid = Frame::at(0.5 * (y0 + y1));
        let mut fr = start;
        for k in 0..N {
            fr.arg[k] = start.arg[k] - start.u[k] + mid.u[k];
            fr.u[k] = mid.u[k];
            fr.f[k] = mid.f[k];
        }
        fr
    }

    /// Each constituent's speed in degrees an hour (to check against the published speeds).
    pub fn speed_deg_per_hour(&self, k: usize) -> f64 {
        self.speed[k] * 3600. / D2R
    }
}

/// A station's harmonic constants, by constituent index: amplitude (metres) and the Greenwich phase lag G (radians); a constituent the station does not list has amplitude 0. And Z0, mean sea level above the datum the heights are given in (metres).
#[derive(Clone, Copy, Debug)]
pub struct Tide {
    pub z0: f64,
    pub amp: [f64; N],
    pub phase: [f64; N],
}

/// A high or a low water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extreme {
    /// Eagle seconds.
    pub t: f64,
    /// Metres above the datum.
    pub h: f64,
    pub high: bool,
}

/// The step h′ is sampled at when looking for extremes: 2^8 seconds (about 4 minutes). Two extremes closer together than this (a stand with a ripple on it) can be missed as a pair, which no printed table counts as two tides either; the four test stations' every published event is found.
pub const STEP: f64 = (1 << 8) as f64;
/// Bisections of a sampled change of sign: 2^8 s / 2^16, under a hundredth of a second, finer than anything shown.
const BISECT: usize = 1 << 4;

impl Tide {
    /// No constituents: a flat sea at `z0`.
    pub const fn flat(z0: f64) -> Tide {
        Tide { z0, amp: [0.; N], phase: [0.; N] }
    }

    /// Set constituent `k`'s amplitude (metres) and Greenwich phase (degrees).
    pub fn set(&mut self, k: usize, amplitude: f64, phase_deg: f64) {
        self.amp[k] = amplitude;
        self.phase[k] = phase_deg * D2R;
    }

    /// The height at Eagle second `t`, from frame `fr` (made near `t`: its arguments run on linearly at their speeds, exact for the hours a search spans).
    pub fn height_in(&self, fr: &Frame, t: f64) -> f64 {
        let dt = t - fr.t;
        let mut h = self.z0;
        for k in 0..N {
            h += fr.f[k] * self.amp[k] * cos(fr.arg[k] + fr.speed[k] * dt - self.phase[k]);
        }
        h
    }

    /// The rate of rise at Eagle second `t` from frame `fr`, metres a second.
    pub fn rate_in(&self, fr: &Frame, t: f64) -> f64 {
        let dt = t - fr.t;
        let mut r = 0.;
        for k in 0..N {
            r -= fr.f[k] * self.amp[k] * fr.speed[k] * sin(fr.arg[k] + fr.speed[k] * dt - self.phase[k]);
        }
        r
    }

    /// The height at Eagle second `t` (metres above the datum).
    pub fn height(&self, t: f64) -> f64 {
        self.height_in(&Frame::at(t), t)
    }

    /// The rate of rise at Eagle second `t`, metres a second.
    pub fn rate(&self, t: f64) -> f64 {
        self.rate_in(&Frame::at(t), t)
    }

    /// The highs and lows in [from, to), in time order, into `out`: how many were written (the rest are not looked for once `out` is full).
    /// One frame serves the span, made at its middle; keep spans to a few days.
    pub fn extremes(&self, from: f64, to: f64, out: &mut [Extreme]) -> usize {
        self.extremes_in(&Frame::at(0.5 * (from + to)), from, to, out)
    }

    /// [`Tide::extremes`] from a frame of the caller's ([`Frame::printed`] to match a printed table).
    pub fn extremes_in(&self, fr: &Frame, from: f64, to: f64, out: &mut [Extreme]) -> usize {
        let fr = *fr;
        let mut n = 0;
        let mut a = from;
        let mut ra = self.rate_in(&fr, a);
        while a < to && n < out.len() {
            let b = a + STEP;
            let rb = self.rate_in(&fr, b);
            // A change of sign (a zero exactly at a sample counts once, at the sample after it).
            if (ra > 0.) != (rb > 0.) && ra != 0. {
                let high = ra > 0.;
                let (mut lo, mut hi) = (a, b);
                for _ in 0..BISECT {
                    let m = 0.5 * (lo + hi);
                    if (self.rate_in(&fr, m) > 0.) == high {
                        lo = m;
                    } else {
                        hi = m;
                    }
                }
                let t = 0.5 * (lo + hi);
                if t >= from && t < to {
                    out[n] = Extreme { t, h: self.height_in(&fr, t), high };
                    n += 1;
                }
            }
            a = b;
            ra = rb;
        }
        n
    }

    /// The next high and the next low after Eagle second `t`: looked for over the following day and a half, which always holds both (no station's tide stands for longer).
    pub fn next(&self, t: f64) -> (Option<Extreme>, Option<Extreme>) {
        let mut e = [Extreme { t: 0., h: 0., high: false }; 1 << 4];
        let n = self.extremes(t, t + 1.5 * DAY, &mut e);
        let high = e[..n].iter().find(|x| x.high).copied();
        let low = e[..n].iter().find(|x| !x.high).copied();
        (high, low)
    }
}

#[cfg(test)]
mod tests;
