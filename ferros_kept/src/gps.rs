//! The GPS driver's last fix (`gps.last`, identity class; payloads/gps `keep_last`): section `last_fix`, the place as vsf's position type `at` (`w`, a Dymaxion coordinate at 64 bits, 2 mm on average and 5 mm at worst), and `when` (`e`, Eagle oscillations) when the fix came, on the real clock; a fix kept while the kernel had no clock has no `when` (never an invented time).
//! The driver's fixes are in 2⁻²³ degrees (the chip's units, i32): a fix goes in as degrees and comes back rounded to the nearest unit (the tests measure the round trip).

use crate::{document, first, only};
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{EtType, VsfType, WorldCoord};

pub const SECTION: &str = "last_fix";

/// The fix's units in a degree: 2²³.
const UNITS: f64 = (1u64 << 23) as f64;

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION).field("at", only("w", |v| matches!(v, VsfType::w(_)))).field("when", TypeConstraint::AnyEagleTime)
}

/// The document for a fix at (`lat`, `lon`) in 2⁻²³ degrees, which came at Eagle time `when` when the clock was known; stamped at `eagle`.
pub fn build(lat: i32, lon: i32, when: Option<i64>, eagle: i64) -> Option<Vec<u8>> {
    let at = WorldCoord::from_lat_lon(lat as f64 / UNITS, lon as f64 / UNITS);
    let mut b = schema().build().set("at", VsfType::w(at)).ok()?;
    if let Some(t) = when {
        b = b.set("when", VsfType::e(EtType::e6(t))).ok()?;
    }
    document(SECTION, b.encode().ok()?, eagle)
}

/// A last-fix document read back: (latitude, longitude) in 2⁻²³ degrees, and when it came if it says. None for anything that is not one.
pub fn parse(doc: &[u8]) -> Option<(i32, i32, Option<i64>)> {
    let s = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let VsfType::w(at) = first(&s, "at")? else { return None };
    let (la, lo) = at.to_lat_lon();
    let when = match first(&s, "when") {
        Some(VsfType::e(EtType::e6(t))) => Some(t),
        _ => None,
    };
    Some((libm::round(la * UNITS) as i32, libm::round(lo * UNITS) as i32, when))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// Ground distance in metres between two fixes in 2⁻²³ degrees (a sphere of the mean Earth radius; small distances).
    fn metres(a: (i32, i32), b: (i32, i32)) -> f64 {
        let r = 6_371_008.8;
        let d = |u: i32| u as f64 / UNITS * core::f64::consts::PI / 180.;
        let (la, lb) = (d(a.0), d(b.0));
        let dlon = d(a.1) - d(b.1);
        let x = dlon * libm::cos((la + lb) / 2.);
        let y = la - lb;
        r * libm::sqrt(x * x + y * y)
    }

    /// A fix round trip thru `w`: across the globe (every 7° and 11° off the grid, poles excepted), the place comes back within 2 cm on the ground and within one unit in latitude; how many come back exact is printed.
    /// A unit is 2⁻²³ degrees, 13 mm of latitude; `w` is 2 mm on average, 5 mm at worst, so a fix lands on its own unit or the next.
    #[test]
    fn a_fix_round_trips_within_a_unit() {
        let (mut n, mut exact, mut worst_m, mut worst_lat, mut worst_lon) = (0u32, 0u32, 0f64, 0i64, 0i64);
        let mut lat = -(89 << 23) + 12345;
        while lat < 89 << 23 {
            let mut lon = -(180 << 23) + 777;
            while lon < 180 << 23 {
                let doc = build(lat, lon, None, 1 << 40).unwrap();
                let (la, lo, when) = parse(&doc).unwrap();
                assert_eq!(when, None);
                let m = metres((lat, lon), (la, lo));
                worst_m = worst_m.max(m);
                worst_lat = worst_lat.max((la as i64 - lat as i64).abs());
                worst_lon = worst_lon.max((lo as i64 - lon as i64).abs());
                exact += (la == lat && lo == lon) as u32;
                n += 1;
                lon += 11 << 23;
            }
            lat += 7 << 23;
        }
        std::println!("{n} fixes: {exact} exact; worst {worst_lat} unit(s) of latitude, {worst_lon} of longitude, {:.1} mm on the ground", worst_m * 1000.);
        assert!(worst_m < 0.02, "worst {worst_m} m");
        assert!(worst_lat <= 1);
    }

    #[test]
    fn a_fix_keeps_its_time_and_the_document_its_stamp() {
        let doc = build(37 << 23, -(122 << 23), Some(1 << 50), 1 << 51).unwrap();
        let (la, lo, when) = parse(&doc).unwrap();
        assert!(metres((37 << 23, -(122 << 23)), (la, lo)) < 0.02);
        assert_eq!(when, Some(1 << 50));
        assert_eq!(crate::created(&doc), Some(1 << 51));
        std::println!("a last fix: {} bytes", doc.len());
    }

    #[test]
    fn what_is_not_a_last_fix_is_refused() {
        assert_eq!(parse(&[0; 8]), None);
        assert_eq!(parse(&crate::map::view(&[1, 2, 3, 4], 5).unwrap()), None);
        let mut doc = build(1, 2, None, 3).unwrap();
        let k = doc.len() - 3;
        doc[k] ^= 1;
        assert_eq!(parse(&doc), None, "an altered document fails its provenance hash");
    }
}
