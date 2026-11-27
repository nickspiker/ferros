//! The broadcast ephemerides the GPS driver gives the chip (`gps.eph`, device class: orbits are public; payloads/gps, docs/GPS.md "Broadcast ephemerides for the chip"): section `gnss_eph`, `fetched` (`e`, when they were fetched, on the real clock), then one field per quantity holding every satellite's value in turn, the satellites in the same order in each: `system` and `number` (`u`: 0 GPS by PRN, 1 GLONASS by slot), `week` (`u`, GPS's continuous week of the set's toe), then IS-GPS-200's set in SI units (`f6`: seconds, metres, radians and radians a second): `toe`, `toc`, `af0`, `af1`, `af2`, `tgd`, `sqrt_a`, `e`, `i0`, `omega0`, `omega`, `m0`, `dn`, `omega_dot`, `idot`, `cuc`, `cus`, `crc`, `crs`, `cic`, `cis`.
//! A GLONASS set is the IS-GPS-200 form fitted to that satellite's broadcast vectors (ferros_gnss `glo`), the form the chip takes for both systems.
//! Made from the IGS's broadcast navigation files (BKG's BRDC) by whoever can fetch them (the host's gps-trace `eph` today; a device of the person's fleet, or ferros itself over HTTPS, later; never fgtw.org, owner 2026-11-19); the numbers only, never the file's text.

use crate::{document, first};
use alloc::vec::Vec;
use ferros_gnss::eph::GpsEph;
use ferros_gnss::windows::Set;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{EtType, VsfType};

pub const SECTION: &str = "gnss_eph";

const FLOATS: [&str; 21] = ["toe", "toc", "af0", "af1", "af2", "tgd", "sqrt_a", "e", "i0", "omega0", "omega", "m0", "dn", "omega_dot", "idot", "cuc", "cus", "crc", "crs", "cic", "cis"];

fn schema() -> SectionSchema {
    let mut s = SectionSchema::new(SECTION).field("fetched", TypeConstraint::AnyEagleTime).field("system", TypeConstraint::AnyUnsigned).field("number", TypeConstraint::AnyUnsigned).field("week", TypeConstraint::AnyUnsigned);
    for f in FLOATS {
        s = s.field(f, TypeConstraint::AnyFloat);
    }
    s
}

fn floats(e: &GpsEph) -> [f64; 21] {
    [e.toe, e.toc, e.af0, e.af1, e.af2, e.tgd, e.sqrt_a, e.e, e.i0, e.omega0, e.omega, e.m0, e.dn, e.omega_dot, e.idot, e.cuc, e.cus, e.crc, e.crs, e.cic, e.cis]
}

/// The document for `sets`, fetched at Eagle time `fetched` (oscillations), stamped at `eagle`.
pub fn build(sets: &[Set], fetched: i64, eagle: i64) -> Option<Vec<u8>> {
    let mut b = schema().build().set("fetched", VsfType::e(EtType::e6(fetched))).ok()?;
    b = b.set_multi("system", sets.iter().map(|s| s.system as u64).collect()).ok()?;
    b = b.set_multi("number", sets.iter().map(|s| s.number as u64).collect()).ok()?;
    b = b.set_multi("week", sets.iter().map(|s| s.eph.week as u64).collect()).ok()?;
    for (k, f) in FLOATS.iter().enumerate() {
        b = b.set_multi(*f, sets.iter().map(|s| floats(&s.eph)[k]).collect()).ok()?;
    }
    document(SECTION, b.encode().ok()?, eagle)
}

fn column(s: &SectionBuilder, name: &str) -> Vec<VsfType> {
    s.get_fields(name).first().map(|f| f.values.clone()).unwrap_or_default()
}

/// An ephemerides document read back: when they were fetched (oscillations) and the sets. None for anything that is not one, or columns of different lengths.
pub fn parse(doc: &[u8]) -> Option<(i64, Vec<Set>)> {
    let s = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let fetched = match first(&s, "fetched")? {
        VsfType::e(EtType::e6(t)) => t,
        _ => return None,
    };
    let uint = |v: &VsfType| -> Option<u64> {
        match *v {
            VsfType::u3(x) => Some(x as u64),
            VsfType::u4(x) => Some(x as u64),
            VsfType::u5(x) => Some(x as u64),
            VsfType::u6(x) => Some(x),
            _ => None,
        }
    };
    let float = |v: &VsfType| match *v {
        VsfType::f6(x) => Some(x),
        VsfType::f5(x) => Some(x as f64),
        _ => None,
    };
    let (sys, num, week) = (column(&s, "system"), column(&s, "number"), column(&s, "week"));
    let fl: Vec<Vec<VsfType>> = FLOATS.iter().map(|f| column(&s, f)).collect();
    let n = sys.len();
    if num.len() != n || week.len() != n || fl.iter().any(|c| c.len() != n) {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let f = |k: usize| float(&fl[k][i]);
        let number = u8::try_from(uint(&num[i])?).ok()?;
        let eph = GpsEph { prn: number, week: u16::try_from(uint(&week[i])?).ok()?, toe: f(0)?, toc: f(1)?, af0: f(2)?, af1: f(3)?, af2: f(4)?, tgd: f(5)?, sqrt_a: f(6)?, e: f(7)?, i0: f(8)?, omega0: f(9)?, omega: f(10)?, m0: f(11)?, dn: f(12)?, omega_dot: f(13)?, idot: f(14)?, cuc: f(15)?, cus: f(16)?, crc: f(17)?, crs: f(18)?, cic: f(19)?, cis: f(20)?, ..Default::default() };
        out.push(Set { system: u8::try_from(uint(&sys[i])?).ok()?, number, eph });
    }
    Some((fetched, out))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn some() -> Vec<Set> {
        (0..54u32)
            .map(|k| Set { system: (k >= 30) as u8, number: (k % 30 + 1) as u8, eph: GpsEph { prn: (k % 30 + 1) as u8, week: 2445, toe: 410_400. + k as f64 * 16., toc: 410_400., af0: 1.2398e-4, af1: -7.958e-12, tgd: -9.31e-9, sqrt_a: 5153.7298, e: 0.0022449, i0: 0.9556, omega0: -1.5037, omega: 0.1426, m0: -1.9874, dn: 4.5059e-9, omega_dot: -8.1225e-9, idot: -2.4e-10, cuc: -2.8e-6, cus: 7.36e-6, crc: 236.7, crs: -55.6, cic: -9.7e-8, cis: 3.5e-8, ..Default::default() } })
            .collect()
    }

    #[test]
    fn the_ephemerides_round_trip() {
        let v = some();
        let doc = build(&v, 1 << 50, 1 << 51).unwrap();
        std::println!("{} sets: {} bytes", v.len(), doc.len());
        let (fetched, back) = parse(&doc).unwrap();
        assert_eq!(fetched, 1 << 50);
        assert_eq!(back, v);
        assert_eq!(crate::created(&doc), Some(1 << 51));
    }

    #[test]
    fn what_is_not_ephemerides_is_refused() {
        assert_eq!(parse(&crate::gps::build(1, 2, None, 3).unwrap()), None);
        let mut doc = build(&some(), 1, 2).unwrap();
        let k = doc.len() - 9;
        doc[k] ^= 1;
        assert_eq!(parse(&doc), None);
    }
}
