//! A link's detail (docs/RADIO-CONTROLS.md): what a driver knows that is text or an identifier — a network's name, an access point's address, the line's IP addresses — never smuggled into a measure's number. One complete VSF document (header, Eagle time, provenance hash) with one section named for the link and a typed field each, kept by the kernel (`SYS_DETAIL`) and read only with the Settings capability.
//!
//! The schemas are here, once, for the driver that writes and the page that reads: the reader verifies the document and refuses a field its schema does not name.
extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// A field: its name, and what its value must be.
#[derive(Clone, Copy)]
pub enum Kind {
    /// ASCII text (`a`): a name.
    Text,
    /// An IPv4 address (`ni`).
    Ipv4,
    /// An IPv6 address (`nj`).
    Ipv6,
    /// A hardware address (`nm`).
    Mac,
    /// Seconds (unsigned).
    Seconds,
}

/// The cellular link's fields: the carrier pack the driver loads (fixed today, Verizon), the data APN and the attach APN, and while a call is up its addresses and DNS servers.
pub const CELLULAR: &[(&str, Kind)] = &[("carrier", Kind::Text), ("apn", Kind::Text), ("attach_apn", Kind::Text), ("ipv4", Kind::Ipv4), ("ipv6", Kind::Ipv6), ("dns4", Kind::Ipv4), ("dns6", Kind::Ipv6)];
/// The WiFi link's fields: the network's name (the person's credential), the access point's address, this device's address on that network, the address DHCP gave, the router, the DNS server, the lease's length.
pub const WIFI: &[(&str, Kind)] = &[("ssid", Kind::Text), ("bssid", Kind::Mac), ("mac", Kind::Mac), ("ipv4", Kind::Ipv4), ("router", Kind::Ipv4), ("dns", Kind::Ipv4), ("lease", Kind::Seconds)];

/// A link's section name and fields (`control`'s link numbers), None for a link with no detail yet.
pub fn fields(link: u64) -> Option<(&'static str, &'static [(&'static str, Kind)])> {
    match link {
        crate::control::CELLULAR => Some(("cellular", CELLULAR)),
        crate::control::WIFI => Some(("wifi", WIFI)),
        _ => None,
    }
}

fn constraint(k: Kind) -> TypeConstraint {
    let custom = |name: &'static str, f: fn(&VsfType) -> bool| TypeConstraint::Custom { name, description: name, validator: Arc::new(f) };
    match k {
        Kind::Text => TypeConstraint::AsciiText,
        Kind::Seconds => TypeConstraint::AnyUnsigned,
        Kind::Ipv4 => custom("IPv4 address (ni)", |v| matches!(v, VsfType::ni(_))),
        Kind::Ipv6 => custom("IPv6 address (nj)", |v| matches!(v, VsfType::nj(_))),
        Kind::Mac => custom("hardware address (nm)", |v| matches!(v, VsfType::nm(_))),
    }
}

fn schema(link: u64) -> Option<SectionSchema> {
    let (name, fs) = fields(link)?;
    let mut s = SectionSchema::new(name);
    for &(f, k) in fs {
        s = s.field(f, constraint(k));
    }
    Some(s)
}

/// Eagle time now in oscillations, from the wall clock the kernel hands every process (0 before it has read the PMIC's clock: the document then says it was made at the epoch).
pub fn eagle_now() -> i64 {
    // Unix seconds at the Eagle epoch (1969-07-20 20:17:40 UTC).
    const EPOCH_UNIX: i128 = -14_182_940;
    let Some((secs, ticks, hz)) = crate::utc_time() else { return 0 };
    let ops = vsf::OSCILLATIONS_PER_SECOND as i128;
    ((secs as i128 - EPOCH_UNIX) * ops + ticks as i128 * ops / hz as i128) as i64
}

/// The document for link `link` with these fields (a name its schema does not have, or a value of the wrong type, refuses the whole document: None).
pub fn build(link: u64, values: Vec<(&str, VsfType)>) -> Option<Vec<u8>> {
    let (name, _) = fields(link)?;
    let mut b = schema(link)?.build();
    for (f, v) in values {
        b = b.set(f, v).ok()?;
    }
    let section = b.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle_now()).provenance_only().add_unboxed(String::from(name), section).build().ok()
}

/// Build and hand to the kernel as this driver's link's detail: true when it was taken.
pub fn publish(link: u64, values: Vec<(&str, VsfType)>) -> bool {
    match build(link, values) {
        Some(doc) => crate::detail_set(&doc),
        None => false,
    }
}

/// A document read back (`crate::detail`), verified and parsed: each field present, in the schema's order, with its kind and value. None for a document that does not verify, or does not match the link's schema.
pub fn parse(link: u64, doc: &[u8]) -> Option<Vec<(&'static str, Kind, VsfType)>> {
    let (_, fs) = fields(link)?;
    let section = SectionBuilder::parse_document(schema(link)?, doc, None).ok()?;
    let mut out = Vec::new();
    for &(f, k) in fs {
        if let Some(v) = section.get_fields(f).first().and_then(|x| x.values.first()) {
            out.push((f, k, v.clone()));
        }
    }
    Some(out)
}
