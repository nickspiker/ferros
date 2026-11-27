//! A WiFi network's credential as the vault keeps it (docs/ATTEST-UI.md "WiFi connect"): one VSF document, section `network`, stamped with its Eagle time and a provenance hash.
//! `name` is the network's SSID as `nw` (vsf's SSID type, 2026-11-09): its octets as the radio carries them, so a name in any language, or none, is kept bit for bit. `passphrase` is ASCII text (`a`): WPA2-PSK's passphrase is 8 to 63 printable ASCII characters by IEEE 802.11i.
//! The person's is `wifi.cred` (identity class: networks travel with the person, AUTH.md); one joined before anyone attests is the device's, `wifi.net`.
//! It replaced the two-line text file (SSID, passphrase) on 2026-11-09, and the name moved from `a` to `nw` the same day; neither older form is read: a credential that is not this document is dropped.
//!
//! This file is shared by path with the host's bridge (`ferros-bridge wifi-cred`), so it uses only `vsf` and `alloc`.

use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

const SECTION: &str = "network";

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION).field("name", TypeConstraint::Ssid).field("passphrase", TypeConstraint::AsciiText)
}

/// A name and passphrase WPA2-PSK can use: the name 1 to 32 octets, the passphrase 8 to 63 (IEEE 802.11i's passphrase range).
pub fn usable(name: &[u8], pass: &[u8]) -> bool {
    (1..=32).contains(&name.len()) && (8..=63).contains(&pass.len())
}

/// The document for a credential, made at `eagle` (oscillations): None if the name or passphrase is not usable, or the passphrase is not ASCII.
pub fn build(name: &[u8], pass: &[u8], eagle: i64) -> Option<Vec<u8>> {
    if !usable(name, pass) {
        return None;
    }
    let name = vsf::Ssid::new(name)?;
    let pass = vsf::Ascii::from_bytes(pass)?;
    let section = schema().build().set("name", VsfType::nw(name)).ok()?.set("passphrase", VsfType::a(pass)).ok()?.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only().add_unboxed(SECTION, section).build().ok()
}

/// The name and passphrase from a credential document, or None if it is not one (or what it holds is not usable).
pub fn parse(doc: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let section = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let first = |field: &str| section.get_fields(field).first().and_then(|f| f.values.first()).cloned();
    let name = match first("name")? {
        VsfType::nw(s) => s.as_bytes().to_vec(),
        _ => return None,
    };
    let pass = match first("passphrase")? {
        VsfType::a(s) => s.as_bytes().to_vec(),
        _ => return None,
    };
    usable(&name, &pass).then_some((name, pass))
}

/// The networks a vault entry keeps (docs/WIFI.md "Networks remembered"; owner, 2026-11-18: more than one network remembered, each forgettable): one VSF document, section `networks`, a `network` field each.
/// A field's values are told apart by their type markers, never their places: the name `nw`, the passphrase `a`, and when this device last joined it `e` (Eagle time, `e6`; absent for one handed in and not joined yet).
const LIST: &str = "networks";

/// One network remembered: its SSID's octets, its passphrase (never printed), and when this device last joined it in Eagle oscillations (None: handed in, not joined yet).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Kept {
    pub name: Vec<u8>,
    pub pass: Vec<u8>,
    pub joined: Option<i64>,
}

fn list_schema() -> SectionSchema {
    SectionSchema::new(LIST).field("network", TypeConstraint::OneOf(alloc::vec![TypeConstraint::Ssid, TypeConstraint::AsciiText, TypeConstraint::AnyEagleTime]))
}

/// The document for these networks, made at `eagle`, as many as the person keeps (docs/NO-CEILINGS.md step 7; sixteen until then, the oldest let go silently): None if one of them is not usable (`usable`) or its passphrase is not ASCII.
pub fn build_list(nets: &[Kept], eagle: i64) -> Option<Vec<u8>> {
    let mut b = list_schema().build();
    for k in nets {
        if !usable(&k.name, &k.pass) {
            return None;
        }
        let mut vals = alloc::vec![VsfType::nw(vsf::Ssid::new(&k.name)?), VsfType::a(vsf::Ascii::from_bytes(&k.pass)?)];
        if let Some(t) = k.joined {
            vals.push(VsfType::e(vsf::EtType::e6(t)));
        }
        b = b.append_multi("network", vals).ok()?;
    }
    let section = b.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only().add_unboxed(LIST, section).build().ok()
}

/// The networks an entry keeps, and whether it was the one-network document of before 2026-11-18 (`build`), which the caller writes again as a list; that network is taken as joined (at the epoch), so a failed join never forgets it.
/// None for anything that is neither, or that holds a network that is not usable: the whole value is then not a list, never half of one.
pub fn parse_list(doc: &[u8]) -> Option<(Vec<Kept>, bool)> {
    if let Ok(section) = SectionBuilder::parse_document(list_schema(), doc, None) {
        let mut out = Vec::new();
        for f in section.get_fields("network") {
            let (mut name, mut pass, mut joined) = (None, None, None);
            for v in &f.values {
                match v {
                    VsfType::nw(s) => name = Some(s.as_bytes().to_vec()),
                    VsfType::a(s) => pass = Some(s.as_bytes().to_vec()),
                    VsfType::e(vsf::EtType::e6(t)) => joined = Some(*t),
                    _ => return None,
                }
            }
            let (name, pass) = (name?, pass?);
            if !usable(&name, &pass) {
                return None;
            }
            out.push(Kept { name, pass, joined });
        }
        return Some((out, false));
    }
    let (name, pass) = parse(doc)?;
    Some((alloc::vec![Kept { name, pass, joined: Some(0) }], true))
}

/// `nets` with `name` kept with `pass` (handed in: not joined yet), replacing what that name kept; returns what it kept before under the name (None: nothing). Nothing else is let go: the list is as long as the person keeps it (docs/NO-CEILINGS.md step 7).
pub fn keep(nets: &mut Vec<Kept>, name: &[u8], pass: &[u8]) -> Option<Kept> {
    let before = nets.iter().position(|k| k.name == name).map(|i| nets.remove(i));
    nets.push(Kept { name: name.to_vec(), pass: pass.to_vec(), joined: None });
    before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(name: &str, pass: &str, joined: Option<i64>) -> Kept {
        Kept { name: name.as_bytes().to_vec(), pass: pass.as_bytes().to_vec(), joined }
    }

    #[test]
    fn list_round_trip() {
        let nets = alloc::vec![net("pangolin", "a passphrase", Some(1 << 40)), net("café", "another one", None), net("x", "12345678", Some(-5))];
        let doc = build_list(&nets, 1 << 50).unwrap();
        assert_eq!(parse_list(&doc), Some((nets, false)));
        assert_eq!(parse_list(&build_list(&[], 0).unwrap()), Some((alloc::vec![], false)));
    }

    #[test]
    fn a_name_is_its_octets() {
        // An SSID is octets, not text: one that is not UTF-8 is kept bit for bit.
        let nets = alloc::vec![Kept { name: alloc::vec![0xFF, 0x00, 0x80], pass: b"passphrase".to_vec(), joined: Some(7) }];
        assert_eq!(parse_list(&build_list(&nets, 0).unwrap()).unwrap().0, nets);
    }

    #[test]
    fn the_old_document_reads_as_one_network() {
        let doc = build(b"pangolin", b"a passphrase", 9).unwrap();
        assert_eq!(parse_list(&doc), Some((alloc::vec![net("pangolin", "a passphrase", Some(0))], true)));
    }

    #[test]
    fn refused() {
        assert!(build_list(&[net("", "12345678", None)], 0).is_none());
        assert!(build_list(&[net("n", "short", None)], 0).is_none());
        assert!(build_list(&[net("n", "not ascii é passphrase", None)], 0).is_none());
        assert!(parse_list(b"not a document").is_none());
        let mut doc = build_list(&[net("pangolin", "a passphrase", None)], 0).unwrap();
        let k = doc.len() - 3;
        doc[k] ^= 1;
        assert!(parse_list(&doc).is_none(), "a document whose hash does not verify is no list");
    }

    #[test]
    fn keeping() {
        let mut nets = alloc::vec![net("a", "12345678", Some(3)), net("b", "12345678", Some(1))];
        assert_eq!(keep(&mut nets, b"a", b"new pass phrase"), Some(net("a", "12345678", Some(3))));
        assert_eq!(nets, alloc::vec![net("b", "12345678", Some(1)), net("a", "new pass phrase", None)]);
    }

    #[test]
    fn as_many_as_the_person_keeps() {
        // Past the sixteen that were the most until docs/NO-CEILINGS.md step 7: none let go, and the document round-trips whole.
        let mut nets: Vec<Kept> = Vec::new();
        for k in 0..300u32 {
            let name = alloc::format!("net {k}");
            assert_eq!(keep(&mut nets, name.as_bytes(), b"12345678"), None);
        }
        assert_eq!(nets.len(), 300);
        let doc = build_list(&nets, 1 << 50).unwrap();
        assert_eq!(parse_list(&doc), Some((nets, false)));
    }
}
