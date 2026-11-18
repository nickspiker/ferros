//! A sponsor placed by hand (`fgtw.sponsor`; payloads/fgtw `given`): a fleet device to hand the binding request (and a departure request) to directly, for a network whose access point does not pass multicast between its wireless clients.
//! Section `sponsor`: `address`, the device's IPv4 (`ni`, 4 bytes) or IPv6 (`nj`, 16 bytes) address, and `port` (`np`).
//! The family is the type marker: there is no family byte.
//!
//! This file is shared by path with the host's bridge (`ferros-bridge sponsor`, which writes the document placed with `store put fgtw.sponsor`), so it uses only `vsf` and `alloc`.

use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

pub const SECTION: &str = "sponsor";

fn schema() -> SectionSchema {
    let address = TypeConstraint::Custom { name: "ni or nj", description: "an IPv4 or IPv6 address", validator: alloc::sync::Arc::new(|v: &VsfType| matches!(v, VsfType::ni(_) | VsfType::nj(_))) };
    let port = TypeConstraint::Custom { name: "np", description: "a network port", validator: alloc::sync::Arc::new(|v: &VsfType| matches!(v, VsfType::np(_))) };
    SectionSchema::new(SECTION).field("address", address).field("port", port)
}

/// An address as payloads/fgtw holds one (photon_core `Addr`): the family (4 or 6), 16 bytes with an IPv4 address in the first 4, and the port.
pub type Addr = (u8, [u8; 16], u16);

/// The document for a sponsor at `addr`, stamped at `eagle` (oscillations). None for a family other than 4 or 6.
pub fn build(addr: &Addr, eagle: i64) -> Option<Vec<u8>> {
    let (fam, ip, port) = *addr;
    let a = match fam {
        4 => VsfType::ni([ip[0], ip[1], ip[2], ip[3]]),
        6 => VsfType::nj(ip),
        _ => return None,
    };
    let section = schema().build().set("address", a).ok()?.set("port", VsfType::np(port)).ok()?.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only().add_unboxed(SECTION, section).build().ok()
}

/// A sponsor document read back. None for anything that is not one.
pub fn parse(doc: &[u8]) -> Option<Addr> {
    let s = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let first = |field: &str| s.get_fields(field).first().and_then(|f| f.values.first()).cloned();
    let mut ip = [0u8; 16];
    let fam = match first("address")? {
        VsfType::ni(a) => {
            ip[..4].copy_from_slice(&a);
            4
        }
        VsfType::nj(a) => {
            ip = a;
            6
        }
        _ => return None,
    };
    let VsfType::np(port) = first("port")? else { return None };
    Some((fam, ip, port))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn a_sponsor_round_trips_in_either_family() {
        let mut v4 = [0u8; 16];
        v4[..4].copy_from_slice(&[192, 168, 1, 23]);
        let mut v6 = [0u8; 16];
        for (k, b) in v6.iter_mut().enumerate() {
            *b = 0xF0 | k as u8;
        }
        for a in [(4, v4, 4383), (6, v6, 1 << 15)] {
            let doc = build(&a, 1 << 46).unwrap();
            assert_eq!(parse(&doc), Some(a));
        }
        assert_eq!(build(&(5, v4, 1), 0), None);
        assert_eq!(parse(&[4; 19]), None);
        std::println!("a sponsor: {} bytes", build(&(6, v6, 1), 0).unwrap().len());
    }
}
