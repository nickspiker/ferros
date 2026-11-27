//! The GNSS sky before a lock (docs/GPS.md "Predicted birds"; owner, 2026-11-11: "can't we figure that out from the orbital data anyway? Like there's zero satellites showing on this page").
//! Which satellites stand above a place at a time follows from three things, none of them the chip: their orbits, the time, and a rough place.
//!
//! - **Orbits:** GP elements in the two-line form CelesTrak serves for every GNSS satellite (`http://celestrak.org/NORAD/elements/gp.php?GROUP=gnss&FORMAT=tle`, plain HTTP, some 28 KB), read by fixed columns with each line's checksum checked ([`tle`], [`each_tle`]); only GPS, GLONASS, Galileo and BeiDou are kept ([`identify`]), numbered as `ferros_sys::Sat` numbers its systems.
//! - **Motion:** the elements are SGP4's mean elements, so they are read as SGP4 reads them (the Kozai mean motion undone, WGS-72's constants) and carried from their epoch by the secular rates of J2, J2² and J4 that SGP4 applies ([`Orbit`]); the short-period terms, and SDP4's lunar and solar terms and its 12-hour resonance (the elements were fitted with them), are left out. Measured against the GPS almanac (src/tests.rs): every GPS satellite within 40 km (20 to 38 km, the same at the elements' epoch as a day on, so the terms left out, not drift), under 0.1 degree of sky from the ground; the chip reports whole degrees.
//! - **Where it is seen:** the orbit's frame (TEME) turned into the earth's by the mean sidereal time (IAU 1982), the place on the WGS-84 ellipsoid, the azimuth from north thru east and the geometric elevation ([`look`]).
//! - **The place:** circles from independent sources combined so that no single one decides (the owner's single egg rule; [`place`]).
//!
//! Time is Eagle time thruout (seconds since 1969-07-20 20:17:40 UTC, as ferros_sky counts them); angles are radians unless a name says degrees.
//! Host-tested against the same day's GPS almanac (CelesTrak's Yuma file, IS-GPS-200's own Kepler equations: an independent orbit source, an independent method) in src/tests.rs.
#![no_std]

pub mod place;
pub mod eph;
pub mod glo;
pub mod windows;

extern crate alloc;

use ferros_sky::{days_j2000, Place, DAY, EAGLE_EPOCH_UNIX_SECS};
use libm::{atan2, cos, floor, pow, sin, sqrt};

const TAU: f64 = core::f64::consts::TAU;
const D2R: f64 = core::f64::consts::PI / 180.;

/// The systems, as `ferros_sys::Sat` numbers them.
pub const GPS: u8 = 0;
pub const GLONASS: u8 = 1;
pub const GALILEO: u8 = 2;
pub const BEIDOU: u8 = 3;

/// One satellite's mean elements at their epoch: which satellite (its system, and its number in that system where the name says it, else 0), the epoch in Eagle seconds, the angles in radians, the mean motion in revolutions a day (as the elements give it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elements {
    pub system: u8,
    /// GPS: the PRN; BeiDou: the C number; GLONASS and Galileo: 0 (CelesTrak names a GLONASS satellite by its GLONASS number and a Galileo one by its build, neither the number the chip reports), so a heard one is matched by where it is.
    pub number: u8,
    pub epoch: f64,
    pub inclination: f64,
    pub node: f64,
    pub eccentricity: f64,
    pub perigee: f64,
    pub anomaly: f64,
    pub motion: f64,
}

/// The system and number a GNSS satellite's name gives, for the four systems kept; None for every other (QZSS, NavIC, the augmentation satellites in geostationary orbit, anything else).
/// The names as CelesTrak gives them (2026-11-18): `GPS BIIR-5  (PRN 22)`, `COSMOS 2433 (720)`, `GSAT0201 (GALILEO 5)`, `BEIDOU-3 M1 (C19)`; a three-line file's `0 ` before the name is allowed.
pub fn identify(name: &str) -> Option<(u8, u8)> {
    let name = name.trim();
    let name = name.strip_prefix("0 ").unwrap_or(name).trim();
    // The number in parentheses after `key`: its digits up to the first other character.
    let after = |key: &str| -> Option<u8> {
        let at = name.find(key)? + key.len();
        let digits = name[at..].split(|c: char| !c.is_ascii_digit()).next()?;
        digits.parse::<u8>().ok()
    };
    if name.starts_with("GPS ") {
        return Some((GPS, after("(PRN ")?));
    }
    if name.starts_with("COSMOS ") {
        return Some((GLONASS, 0));
    }
    if name.starts_with("GSAT0") || name.contains("(GALILEO") {
        return Some((GALILEO, 0));
    }
    if name.starts_with("BEIDOU") {
        return Some((BEIDOU, after("(C")?));
    }
    None
}

/// A two-line element line's checksum: the sum of its first 68 columns' digits, a minus sign counting one, modulo ten, against column 69.
fn checksum_ok(l: &[u8]) -> bool {
    if l.len() < 69 || !l[68].is_ascii_digit() {
        return false;
    }
    let s: u32 = l[..68].iter().map(|&c| if c.is_ascii_digit() { (c - b'0') as u32 } else if c == b'-' { 1 } else { 0 }).sum();
    s % 10 == (l[68] - b'0') as u32
}

/// Columns `a..b` (zero-based, end excluded) as a number; the columns trimmed of spaces.
fn field(l: &[u8], a: usize, b: usize) -> Option<f64> {
    core::str::from_utf8(&l[a..b]).ok()?.trim().parse::<f64>().ok()
}

/// Eagle seconds at the start of January 1 of year `y` (Hinnant's days from civil).
fn year_start(y: i64) -> f64 {
    let y = y - 1;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    // March-based day of the year of January 1 is 306.
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + 306;
    let days = era * 146_097 + doe - 719_468;
    (days * 86_400 - EAGLE_EPOCH_UNIX_SECS) as f64
}

/// One satellite's elements from its name and two lines, or None: not one of the four systems, a line of the wrong number or length, a checksum that fails, the two lines of different satellites, or a field that is not a number.
/// Columns by the format's definition (one-based in its documents, zero-based here): line 1 the catalogue number 2-7 and the epoch 18-32 (two-digit year, then the day of the year with its fraction); line 2 the catalogue number 2-7, inclination 8-16, ascending node 17-25, eccentricity 26-33 (its decimal point implied), argument of perigee 34-42, mean anomaly 43-51 (degrees), mean motion 52-63 (revolutions a day).
pub fn tle(name: &str, l1: &str, l2: &str) -> Option<Elements> {
    let (system, number) = identify(name)?;
    let (a, b) = (l1.trim_end().as_bytes(), l2.trim_end().as_bytes());
    if a.len() < 69 || b.len() < 69 || a[0] != b'1' || b[0] != b'2' || !checksum_ok(a) || !checksum_ok(b) || a[2..7] != b[2..7] {
        return None;
    }
    let yy = field(a, 18, 20)? as i64;
    // The format's own rule: years 57-99 are the 1900s.
    let year = if yy < 57 { 2000 + yy } else { 1900 + yy };
    let day = field(a, 20, 32)?;
    let ecc = core::str::from_utf8(&b[26..33]).ok()?.trim();
    let mut e = 0.;
    let mut scale = 0.1;
    for c in ecc.bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        e += (c - b'0') as f64 * scale;
        scale *= 0.1;
    }
    Some(Elements {
        system,
        number,
        epoch: year_start(year) + (day - 1.) * DAY,
        inclination: field(b, 8, 16)? * D2R,
        node: field(b, 17, 25)? * D2R,
        eccentricity: e,
        perigee: field(b, 34, 42)? * D2R,
        anomaly: field(b, 43, 51)? * D2R,
        motion: field(b, 52, 63)?,
    })
}

/// Every satellite of the four systems in a file of names and two-line elements (CelesTrak's `FORMAT=tle`: a name line, then lines 1 and 2), each handed to `take`; returns (taken, refused), refused counting the sets that failed a check (a satellite of another system is neither).
/// Lines end in LF or CR LF; anything that is not a name followed by two element lines is stepped over a line at a time.
pub fn each_tle(text: &[u8], mut take: impl FnMut(Elements)) -> (u32, u32) {
    let Ok(text) = core::str::from_utf8(text) else { return (0, 0) };
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r')).peekable();
    let (mut taken, mut refused) = (0u32, 0u32);
    while let Some(name) = lines.next() {
        if name.starts_with("1 ") || name.starts_with("2 ") || name.trim().is_empty() {
            continue;
        }
        let Some(l1) = lines.peek().copied().filter(|l| l.starts_with("1 ")) else { continue };
        lines.next();
        let Some(l2) = lines.peek().copied().filter(|l| l.starts_with("2 ")) else { continue };
        lines.next();
        if identify(name).is_none() {
            continue;
        }
        match tle(name, l1, l2) {
            Some(e) => {
                taken += 1;
                take(e);
            }
            None => refused += 1,
        }
    }
    (taken, refused)
}

// SGP4's earth (WGS-72, as the elements were fitted with it): Vallado, Crawford, Hujsak and Kelso, "Revisiting Spacetrack Report #3" (AIAA 2006-6753), getgravconst("wgs72").
/// Equatorial radius, km.
const RE_72: f64 = 6378.135;
/// μ, km³/s².
const MU_72: f64 = 398_600.8;
const J2: f64 = 0.001_082_616;
const J4: f64 = -0.000_001_655_97;

/// A satellite's elements made ready to carry forward: SGP4's initialisation of the mean motion and semi-major axis, and the secular rates of the node, the perigee and the mean anomaly.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub e: Elements,
    /// Semi-major axis, km.
    a: f64,
    /// Rates, radians a second.
    mdot: f64,
    argpdot: f64,
    nodedot: f64,
}

impl Orbit {
    /// SGP4's `initl` and the secular part of `sgp4init` (the same report): the Kozai mean motion turned into Brouwer's, then the rates of J2, J2² and J4.
    pub fn new(e: &Elements) -> Orbit {
        // xke: sqrt(μ) in earth radii and minutes.
        let xke = 60. / sqrt(RE_72 * RE_72 * RE_72 / MU_72);
        let no_kozai = e.motion * TAU / 1440.;
        let (cosio, ecc) = (cos(e.inclination), e.eccentricity);
        let cosio2 = cosio * cosio;
        let omeosq = 1. - ecc * ecc;
        let rteosq = sqrt(omeosq);
        let ak = pow(xke / no_kozai, 2. / 3.);
        let d1 = 0.75 * J2 * (3. * cosio2 - 1.) / (rteosq * omeosq);
        let del = d1 / (ak * ak);
        let adel = ak * (1. - del * del - del * (1. / 3. + 134. * del * del / 81.));
        let del = d1 / (adel * adel);
        let no = no_kozai / (1. + del);
        let ao = pow(xke / no, 2. / 3.);
        let pinvsq = 1. / (ao * ao * omeosq * omeosq);
        let cosio4 = cosio2 * cosio2;
        let temp1 = 1.5 * J2 * pinvsq * no;
        let temp2 = 0.5 * temp1 * J2 * pinvsq;
        let temp3 = -0.46875 * J4 * pinvsq * pinvsq * no;
        let con41 = 3. * cosio2 - 1.;
        let con42 = 1. - 5. * cosio2;
        let mdot = no + 0.5 * temp1 * rteosq * con41 + 0.0625 * temp2 * rteosq * (13. - 78. * cosio2 + 137. * cosio4);
        let argpdot = -0.5 * temp1 * con42 + 0.0625 * temp2 * (7. - 114. * cosio2 + 395. * cosio4) + temp3 * (3. - 36. * cosio2 + 49. * cosio4);
        let nodedot = -temp1 * cosio + (0.5 * temp2 * (4. - 19. * cosio2) + 2. * temp3 * (3. - 7. * cosio2)) * cosio;
        // Per minute to per second.
        let m = 1. / 60.;
        Orbit { e: *e, a: ao * RE_72, mdot: mdot * m, argpdot: argpdot * m, nodedot: nodedot * m }
    }

    /// Where the satellite is at Eagle second `t` in the orbit's frame (TEME, the true equator and mean equinox of the date), km.
    pub fn teme(&self, t: f64) -> [f64; 3] {
        let dt = t - self.e.epoch;
        let ecc = self.e.eccentricity;
        let m = self.e.anomaly + self.mdot * dt;
        let w = self.e.perigee + self.argpdot * dt;
        let node = self.e.node + self.nodedot * dt;
        let m = m - TAU * floor(m / TAU);
        // Kepler's equation by Newton's method from E = M: a GNSS orbit's eccentricity is under 0.03 (some BeiDou and Galileo ones up to 0.2), so eight steps are many more than the doubles hold.
        let mut ea = m;
        for _ in 0..1 << 3 {
            ea -= (ea - ecc * sin(ea) - m) / (1. - ecc * cos(ea));
        }
        let (xp, yp) = (self.a * (cos(ea) - ecc), self.a * sqrt(1. - ecc * ecc) * sin(ea));
        let (cw, sw, cn, sn, ci, si) = (cos(w), sin(w), cos(node), sin(node), cos(self.e.inclination), sin(self.e.inclination));
        // R3(−Ω) R1(−i) R3(−ω) on the in-plane position.
        let (x1, y1) = (cw * xp - sw * yp, sw * xp + cw * yp);
        [cn * x1 - sn * ci * y1, sn * x1 + cn * ci * y1, si * y1]
    }

    /// Where the satellite is at Eagle second `t` in the earth's frame (polar motion left out, a few metres), km.
    pub fn ecef(&self, t: f64) -> [f64; 3] {
        let p = self.teme(t);
        let g = gmst(t);
        let (c, s) = (cos(g), sin(g));
        [c * p[0] + s * p[1], -s * p[0] + c * p[1], p[2]]
    }
}

/// Greenwich mean sidereal time at Eagle second `t` (UT1 taken as UTC: under a second, 0.004 degree), radians: IAU 1982 as SGP4 uses it (Vallado's `gstime`).
pub fn gmst(t: f64) -> f64 {
    let tu = days_j2000(t) / 36_525.;
    let s = -6.2e-6 * tu * tu * tu + 0.093_104 * tu * tu + (876_600. * 3600. + 8_640_184.812_866) * tu + 67_310.548_41;
    let g = s * D2R / 240.;
    g - TAU * floor(g / TAU)
}

// The place's ellipsoid, WGS-84.
const RE_84: f64 = 6378.137;
const F_84: f64 = 1. / 298.257_223_563;

/// A place on the ellipsoid (at its surface) in the earth's frame, km.
pub fn place_ecef(p: Place) -> [f64; 3] {
    let e2 = F_84 * (2. - F_84);
    let (sl, cl) = (sin(p.lat), cos(p.lat));
    let n = RE_84 / sqrt(1. - e2 * sl * sl);
    [n * cl * cos(p.lon), n * cl * sin(p.lon), n * (1. - e2) * sl]
}

/// Elevation and azimuth (radians; the azimuth from north thru east in [0, 2π)) of a point `sat` (km, the earth's frame) seen from place `p`.
pub fn look(sat: [f64; 3], p: Place) -> (f64, f64) {
    let o = place_ecef(p);
    let d = [sat[0] - o[0], sat[1] - o[1], sat[2] - o[2]];
    let (sl, cl, so, co) = (sin(p.lat), cos(p.lat), sin(p.lon), cos(p.lon));
    let east = -so * d[0] + co * d[1];
    let north = -sl * co * d[0] - sl * so * d[1] + cl * d[2];
    let up = cl * co * d[0] + cl * so * d[1] + sl * d[2];
    let az = atan2(east, north);
    (atan2(up, sqrt(east * east + north * north)), if az < 0. { az + TAU } else { az })
}

/// One satellite in the sky: which, and where, in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bird {
    pub system: u8,
    pub number: u8,
    pub el: f32,
    pub az: f32,
}

/// The satellites above `min_el` (radians) from place `p` at Eagle second `t`, into `out` (as many as fit, in the elements' order); how many were written.
pub fn sky(orbits: &[Orbit], t: f64, p: Place, min_el: f64, out: &mut [Bird]) -> usize {
    let mut n = 0;
    for o in orbits {
        if n == out.len() {
            break;
        }
        let (el, az) = look(o.ecef(t), p);
        if el >= min_el {
            out[n] = Bird { system: o.e.system, number: o.e.number, el: (el / D2R) as f32, az: (az / D2R) as f32 };
            n += 1;
        }
    }
    n
}

/// The angle between two directions in the sky, each (elevation, azimuth) in degrees, in degrees.
pub fn apart(a: (f32, f32), b: (f32, f32)) -> f32 {
    let r = D2R as f32;
    let (ea, aa, eb, ab) = (a.0 * r, a.1 * r, b.0 * r, b.1 * r);
    let c = libm::sinf(ea) * libm::sinf(eb) + libm::cosf(ea) * libm::cosf(eb) * libm::cosf(aa - ab);
    // WHY the min: rounding can carry the cosine of two equal directions past 1, where acos is NaN; this is the cosine's own range, not a limit on the data.
    libm::acosf(c.min(1.)) / r
}

#[cfg(test)]
mod tests;
