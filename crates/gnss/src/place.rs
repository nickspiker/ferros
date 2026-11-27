//! The rough place, from several sources at once (owner's single egg rule: no one source decides).
//! Each source is a circle: a centre and a radius in metres within which it says the device is.
//! - the person's last fix (`gps.last`), its radius growing with its age ([`last_fix`]);
//! - fgtw.org's view of the device's address (not built: GPS.md "Proposed", the signed reply);
//! - the zone in effect's principal place (some 2000 km).
//!
//! [`combine`] starts from the tightest circle and walks out thru the looser ones: one that overlaps it agrees and changes nothing; one that does not contradicts it, and the circle grows to the smallest one holding both.
//! So a tight source agreed with stands, and a tight source contradicted is widened to cover the disagreement instead of silently winning or losing.

use libm::{acos, asin, atan2, cos, sin, sqrt};

/// The earth's mean radius, m (IUGG): distances on the ground at the accuracy a rough place needs.
pub const EARTH_M: f64 = 6_371_008.8;

/// A place's circle: centre latitude and longitude in radians (north and east positive), radius in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub lat: f64,
    pub lon: f64,
    pub radius: f64,
}

/// The ground distance between two centres, m (the spherical law of cosines' arc, by the haversine's form for short ones).
pub fn distance(a: &Circle, b: &Circle) -> f64 {
    let (dlat, dlon) = (b.lat - a.lat, b.lon - a.lon);
    let h = sin(dlat / 2.) * sin(dlat / 2.) + cos(a.lat) * cos(b.lat) * sin(dlon / 2.) * sin(dlon / 2.);
    2. * EARTH_M * asin(sqrt(h))
}

/// The point a fraction `f` of the way along the great circle from `a`'s centre to `b`'s.
fn between(a: &Circle, b: &Circle, f: f64) -> (f64, f64) {
    let v = |c: &Circle| [cos(c.lat) * cos(c.lon), cos(c.lat) * sin(c.lon), sin(c.lat)];
    let (p, q) = (v(a), v(b));
    let d = p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
    // WHY the bounds: rounding can carry the dot product of two unit vectors a hair past ±1, where acos is NaN; it is the cosine's own range, not a limit on the data.
    let w = acos(d.clamp(-1., 1.));
    if w < 1e-12 {
        return (a.lat, a.lon);
    }
    let (ka, kb) = (sin((1. - f) * w) / sin(w), sin(f * w) / sin(w));
    let r = [ka * p[0] + kb * q[0], ka * p[1] + kb * q[1], ka * p[2] + kb * q[2]];
    (atan2(r[2], sqrt(r[0] * r[0] + r[1] * r[1])), atan2(r[1], r[0]))
}

/// What the combination made of its sources.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// The circle used.
    pub circle: Circle,
    /// The source (its index in the slice given) the walk started from: the tightest.
    pub tightest: usize,
    /// The looser sources that overlapped the circle as it stood when they were reached, and those that did not (each of these widened it).
    pub agreed: u32,
    pub contradicted: u32,
}

/// The sources combined (above), or None with none. At most 1 << 3 sources are read; more are left out (there are three today).
pub fn combine(sources: &[Circle]) -> Option<Verdict> {
    const MOST: usize = 1 << 3;
    let n = sources.len().min(MOST);
    if n == 0 {
        return None;
    }
    // The sources by radius, tightest first: an insertion sort of their indices.
    let mut order = [0usize; MOST];
    for i in 0..n {
        let mut k = i;
        while k > 0 && sources[order[k - 1]].radius > sources[i].radius {
            order[k] = order[k - 1];
            k -= 1;
        }
        order[k] = i;
    }
    let mut c = sources[order[0]];
    let (mut agreed, mut contradicted) = (0u32, 0u32);
    for &i in &order[1..n] {
        let l = sources[i];
        let d = distance(&c, &l);
        if d <= c.radius + l.radius {
            agreed += 1;
            continue;
        }
        contradicted += 1;
        // The smallest circle holding both: its diameter runs from c's far side to l's, so its radius is half of d + both radii, its centre that far from c's far edge toward l.
        let r = (d + c.radius + l.radius) / 2.;
        let (lat, lon) = between(&c, &l, (r - c.radius) / d);
        c = Circle { lat, lon, radius: r };
    }
    Some(Verdict { circle: c, tightest: order[0], agreed, contradicted })
}

/// The least a fix's circle is, m: the chip's own error estimate is seldom under a few metres, and a circle of 0 would refuse every other source.
pub const FIX_FLOOR_M: f64 = (1 << 6) as f64;
/// How fast a last fix's circle grows with its age, m/s: a car on a highway (some 115 km/h); a flight since outruns it, and the next source out widens it then.
pub const DRIFT_M_S: f64 = (1 << 5) as f64;
/// A last fix with no time kept: as wide as a zone, since it could be from any day.
pub const UNDATED_M: f64 = (1 << 21) as f64;

/// The last fix's circle: its centre (radians), its error estimate in metres when kept, and its age in seconds when its time was kept.
pub fn last_fix(lat: f64, lon: f64, err_m: Option<f64>, age_s: Option<f64>) -> Circle {
    let radius = match age_s {
        Some(a) => FIX_FLOOR_M + err_m.unwrap_or(0.) + DRIFT_M_S * a.abs(),
        None => UNDATED_M,
    };
    Circle { lat, lon, radius }
}
