//! The GNSS orbits the GPS driver predicts the sky from (`gps.orbits`, device class: orbits are public; payloads/gps, docs/GPS.md "Predicted birds"): section `gnss_orbits`, `fetched` (`e`, when they were fetched, on the real clock), then one field per quantity holding every satellite's value in turn, the satellites in the same order in each (a column each, its name saying what every value in it is): `system` and `number` (`u`), `epoch` (`e`), `inclination`, `node`, `eccentricity`, `perigee`, `anomaly` (`f6`, radians, the eccentricity plain) and `motion` (`f6`, revolutions a day).
//! What is kept is the elements as numbers, never the text they were read from (CelesTrak's two-line form is read once, by ferros_gnss's fixed-column parser with its checksums, at the fetch).

use crate::{document, first};
use alloc::vec::Vec;
use ferros_gnss::Elements;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{EtType, VsfType};

pub const SECTION: &str = "gnss_orbits";

/// Eagle oscillations in a second (vsf's), for the epochs.
const OPS: f64 = vsf::OSCILLATIONS_PER_SECOND as f64;

const FLOATS: [&str; 6] = ["inclination", "node", "eccentricity", "perigee", "anomaly", "motion"];

fn schema() -> SectionSchema {
    let mut s = SectionSchema::new(SECTION).field("fetched", TypeConstraint::AnyEagleTime).field("system", TypeConstraint::AnyUnsigned).field("number", TypeConstraint::AnyUnsigned).field("epoch", TypeConstraint::AnyEagleTime);
    for f in FLOATS {
        s = s.field(f, TypeConstraint::AnyFloat);
    }
    s
}

fn floats(e: &Elements) -> [f64; 6] {
    [e.inclination, e.node, e.eccentricity, e.perigee, e.anomaly, e.motion]
}

/// The document for `orbits`, fetched at Eagle time `fetched` (oscillations), stamped at `eagle`.
pub fn build(orbits: &[Elements], fetched: i64, eagle: i64) -> Option<Vec<u8>> {
    let mut b = schema().build().set("fetched", VsfType::e(EtType::e6(fetched))).ok()?;
    b = b.set_multi("system", orbits.iter().map(|e| e.system as u64).collect()).ok()?;
    b = b.set_multi("number", orbits.iter().map(|e| e.number as u64).collect()).ok()?;
    b = b.set_multi("epoch", orbits.iter().map(|e| VsfType::e(EtType::e6((e.epoch * OPS) as i64))).collect()).ok()?;
    for (k, f) in FLOATS.iter().enumerate() {
        b = b.set_multi(*f, orbits.iter().map(|e| floats(e)[k]).collect()).ok()?;
    }
    document(SECTION, b.encode().ok()?, eagle)
}

/// A field's values, every one of them.
fn column(s: &SectionBuilder, name: &str) -> Vec<VsfType> {
    s.get_fields(name).first().map(|f| f.values.clone()).unwrap_or_default()
}

/// An orbits document read back: when they were fetched (oscillations) and the elements. None for anything that is not one, or columns of different lengths.
pub fn parse(doc: &[u8]) -> Option<(i64, Vec<Elements>)> {
    let s = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let fetched = match first(&s, "fetched")? {
        VsfType::e(EtType::e6(t)) => t,
        _ => return None,
    };
    let uint = |v: &VsfType| -> Option<u8> {
        match *v {
            VsfType::u3(x) => Some(x),
            VsfType::u4(x) => u8::try_from(x).ok(),
            VsfType::u5(x) => u8::try_from(x).ok(),
            VsfType::u6(x) => u8::try_from(x).ok(),
            _ => None,
        }
    };
    let (sys, num, ep) = (column(&s, "system"), column(&s, "number"), column(&s, "epoch"));
    let fl: Vec<Vec<VsfType>> = FLOATS.iter().map(|f| column(&s, f)).collect();
    let n = sys.len();
    if num.len() != n || ep.len() != n || fl.iter().any(|c| c.len() != n) {
        return None;
    }
    let float = |v: &VsfType| match *v {
        VsfType::f6(x) => Some(x),
        VsfType::f5(x) => Some(x as f64),
        _ => None,
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let epoch = match ep[i] {
            VsfType::e(EtType::e6(t)) => t as f64 / OPS,
            _ => return None,
        };
        let f = |k: usize| float(&fl[k][i]);
        out.push(Elements { system: uint(&sys[i])?, number: uint(&num[i])?, epoch, inclination: f(0)?, node: f(1)?, eccentricity: f(2)?, perigee: f(3)?, anomaly: f(4)?, motion: f(5)? });
    }
    Some((fetched, out))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn some() -> Vec<Elements> {
        (0..130u32)
            .map(|k| Elements { system: (k % 4) as u8, number: (k % 64) as u8, epoch: 1.8e9 + k as f64 * 1000.25, inclination: 0.96 + k as f64 * 1e-6, node: k as f64 * 0.04, eccentricity: 0.0114629, perigee: 5.3, anomaly: 3.9, motion: 2.00559061 })
            .collect()
    }

    #[test]
    fn the_orbits_round_trip() {
        let v = some();
        let doc = build(&v, 1 << 50, 1 << 51).unwrap();
        std::println!("{} satellites: {} bytes", v.len(), doc.len());
        let (fetched, back) = parse(&doc).unwrap();
        assert_eq!(fetched, 1 << 50);
        assert_eq!(back.len(), v.len());
        for (a, b) in v.iter().zip(&back) {
            assert_eq!((a.system, a.number), (b.system, b.number));
            assert!((a.epoch - b.epoch).abs() < 1e-6);
            assert_eq!(floats(a), floats(b));
        }
        assert_eq!(crate::created(&doc), Some(1 << 51));
    }

    #[test]
    fn what_is_not_orbits_is_refused() {
        assert_eq!(parse(&crate::gps::build(1, 2, None, 3).unwrap()), None);
        let mut doc = build(&some(), 1, 2).unwrap();
        let k = doc.len() - 9;
        doc[k] ^= 1;
        assert_eq!(parse(&doc), None);
    }
}
