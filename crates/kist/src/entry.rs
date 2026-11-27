//! A kist's entry (docs/KIST-DISTRIBUTION.md 1.0; owner, 2026-11-24: "One entry format, yes."): one kist version as its developer states it, signed by the developer with every egg, so it verifies alone whoever passed it.
//!
//! - **The file**: a complete VSF document, its header naming the developer's Ed25519 key (`ke`) with an egg slot per scheme (`fgtw::release::release_slots`, as a capsule's), stamped with Eagle time, its provenance `hp`. One section `kist_entry`:
//!   `option` (`hR`), `name` (`a`), `developer_name` (`a`, optional), `version` (a `z` per part, as written), `capsule` (`hb`, the capsule's BLAKE3), `size` (`u`), `kb` (the developer's public bundle: `ke`, `kvf`, `kvs`, read by type marker), `about` (`a`), `web` (`nu`, one or more: plain-http URLs where the developer hosts the capsule).
//! - **Where it lives**: beside its capsule on its developer's host and on fgtw.org's phonebook, both at `capsules/<blake3>.entry.vsf` ([`path`]), the capsule at `capsules/<blake3>.vsf`.
//! - **At stow** the capsule's own signed declaration must agree with the entry ([`agrees`]): developer, option, name, version and the bundle it carries. One that does not is refused and the entry dropped.
//! - **Text is ASCII** (`a`): the device's stower is no_std, as the capsule's declaration has it.

use super::*;
use crate::capsule::{bundle_from_values, bundle_values, BUNDLE};
use fgtw::pq::KeyBundle;

/// The entry's one section.
pub const SECTION: &str = "kist_entry";

/// One kist version as its developer states it.
#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    /// The developer's Ed25519 key: the header's signer, and the `kb` bundle's Ed25519 half.
    pub developer: [u8; 32],
    /// With the developer, the kist's identity across versions.
    pub option: [u8; 32],
    pub name: String,
    pub developer_name: Option<String>,
    pub version: Version,
    /// The capsule's BLAKE3: its name everywhere (the vault, the person's list, every host).
    pub capsule: [u8; 32],
    pub size: u64,
    /// The developer's whole public bundle, every scheme: what every egg of the entry is checked against.
    pub bundle: KeyBundle,
    /// One line of what the kist is.
    pub about: String,
    /// Plain-http URLs where the developer hosts the capsule (the fetch's web rung, per developer): at least one.
    pub web: Vec<String>,
}

/// A BLAKE3 as lowercase hex.
pub fn hex(b: &[u8; 32]) -> String {
    let mut p = String::with_capacity(64);
    for x in b {
        let d = |n: u8| (if n < 10 { b'0' + n } else { b'a' + n - 10 }) as char;
        p.push(d(x >> 4));
        p.push(d(x & 15));
    }
    p
}

/// An entry's path on a host by its capsule's BLAKE3, beside the capsule: `/capsules/<blake3>.entry.vsf`.
pub fn path(capsule: &[u8; 32]) -> String {
    alloc::format!("/capsules/{}.entry.vsf", hex(capsule))
}

fn text(what: &str, s: &str) -> Result<VsfType, String> {
    vsf::types::Ascii::new(s).map(VsfType::a).map_err(|e| alloc::format!("{what} {s:?}: {e} (an entry's text is ASCII, read by a no_std stower)"))
}

/// The entry unsigned, stamped `made` (Eagle oscillations), its header naming the bundle's Ed25519 key with an empty egg slot per scheme: sign it with `fgtw::release::sign_release` and the developer's `SigningBundle`, as a capsule is signed.
pub fn unsigned(e: &Entry, made: i64) -> Result<Vec<u8>, String> {
    if e.bundle.ed25519() != e.developer {
        return Err(String::from("the bundle is not the developer's key"));
    }
    if e.version.0.is_empty() {
        return Err(String::from("a version with no parts"));
    }
    if e.web.is_empty() {
        return Err(String::from("an entry names at least one web host for its capsule"));
    }
    let mut s = VsfSection::new(SECTION);
    s.add_field("option", VsfType::hR(e.option.to_vec()));
    s.add_field("name", text("the name", &e.name)?);
    if let Some(n) = &e.developer_name {
        s.add_field("developer_name", text("the developer's name", n)?);
    }
    s.add_field_multi(VERSION, e.version.values());
    s.add_field("capsule", VsfType::hb(e.capsule.to_vec()));
    s.add_field("size", VsfType::u(e.size as usize, false));
    s.add_field_multi(BUNDLE, bundle_values(&e.bundle).map_err(String::from)?);
    s.add_field("about", text("the about line", &e.about)?);
    for u in &e.web {
        if !u.starts_with("http://") {
            return Err(alloc::format!("{u}: a web host is plain HTTP (the device has no TLS)"));
        }
    }
    s.add_field_multi("web", e.web.iter().map(|u| VsfType::nu(u.clone())).collect());
    vsf::VsfBuilder::new().creation_time_oscillations(made).signed_only_eggs(VsfType::ke(e.developer.to_vec()), &fgtw::release::release_slots()).add_section_direct(s).build()
}

/// A field's first value of the wanted type (the type marker says what a value is).
fn value<'a, T>(s: &'a VsfSection, name: &str, f: impl Fn(&'a VsfType) -> Option<T>) -> Option<T> {
    s.get_field(name)?.values.iter().find_map(f)
}

fn ascii(s: &VsfSection, name: &str) -> Option<String> {
    value(s, name, |v| match v {
        VsfType::a(t) => Some(String::from(t.as_str())),
        _ => None,
    })
}

/// The entry's fields, read for shape only: who signed it is [`verify`]'s. Every field present and of its type; the header's signer the bundle's Ed25519 key.
pub fn read(doc: &[u8]) -> Result<Entry, &'static str> {
    let (h, _) = VsfHeader::decode(doc).map_err(|_| "not a VSF file")?;
    let developer: [u8; 32] = match &h.signer_pubkey {
        // WHY: the key's length is the document's; anything but 32 bytes is no Ed25519 key.
        Some(VsfType::ke(k)) => k[..].try_into().map_err(|_| "a signer key that is not Ed25519")?,
        _ => return Err("an entry not signed by a developer key"),
    };
    let s = section(doc, SECTION)?.ok_or("no kist_entry section")?;
    let bundle = bundle_from_values(&s.get_field(BUNDLE).ok_or("an entry without its developer's bundle")?.values)?;
    if bundle.ed25519() != developer {
        return Err("its bundle is not its signer's key");
    }
    let web: Vec<String> = s.get_field("web").map(|f| f.values.iter().filter_map(|v| if let VsfType::nu(u) = v { Some(u.clone()) } else { None }).collect()).unwrap_or_default();
    if web.is_empty() {
        return Err("an entry without a web host");
    }
    Ok(Entry {
        developer,
        // WHY: the length is the document's; `try_into` refuses a value of any other length rather than reading past it.
        option: value(&s, "option", |v| if let VsfType::hR(o) = v { o[..].try_into().ok() } else { None }).ok_or("an entry without its option")?,
        name: ascii(&s, "name").ok_or("an entry without its name")?,
        developer_name: ascii(&s, "developer_name"),
        version: Version::from_values(&s.get_field(VERSION).ok_or("an entry without its version")?.values)?,
        // WHY: as `option`.
        capsule: value(&s, "capsule", |v| if let VsfType::hb(h) = v { h[..].try_into().ok() } else { None }).ok_or("an entry without its capsule's blake3")?,
        size: value(&s, "size", |v| v.as_u64()).ok_or("an entry without its size")?,
        bundle,
        about: ascii(&s, "about").ok_or("an entry without its about line")?,
        web,
    })
}

/// The entry, if every egg verifies against the bundle it carries and that bundle's Ed25519 key is the header's signer: an entry verifies alone, with no capsule in hand. For Photon's release key the carried bundle must be the one fgtw pins. Whether the bundle is the one the person pins for this developer is the caller's (`Kists::judge`), as for a capsule.
pub fn verify(doc: &[u8]) -> Result<Entry, String> {
    let e = read(doc).map_err(String::from)?;
    if e.developer == fgtw::release::RELEASE_ED25519 && e.bundle != fgtw::release::release_bundle() {
        return Err(String::from("an entry under Photon's release key with another bundle"));
    }
    fgtw::release::verify_signed_by(doc, &e.developer, &e.bundle)?;
    Ok(e)
}

/// Whether a capsule's own signed declaration (its first bytes, `first`) agrees with the entry: its developer and option, its name, its version, and the bundle it carries (none for a capsule of Photon's release key from before bundles). Its BLAKE3 and size are the fetch's to check. Err names what differs.
pub fn agrees(e: &Entry, first: &[u8]) -> Result<(), &'static str> {
    let (developer, option) = identity(first)?;
    if developer != e.developer || option != e.option {
        return Err("the capsule is another kist than its entry says");
    }
    if declaration(first)?.name != e.name {
        return Err("the capsule's name is not its entry's");
    }
    if version_of(first)?.is_some_and(|v| v != e.version) {
        return Err("the capsule's version is not its entry's");
    }
    match crate::capsule::bundle_of(first)? {
        Some(b) if b != e.bundle => Err("the capsule carries another bundle than its entry"),
        None if developer != fgtw::release::RELEASE_ED25519 => Err("the capsule carries no developer bundle"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use crate::capsule::{unsigned as capsule_unsigned, Code};
    use fgtw::pq::SigningBundle;

    fn capsule(key: &SigningBundle, version: &[u64]) -> Vec<u8> {
        let d = Decl { name: String::from("Clock"), wants: alloc::vec![(PUCK, String::from("its circle on the deck"))] };
        let code = Code { image: &[7u8; 256], masked: true, text: Some(128), fixed: None, bti: false };
        fgtw::release::sign_release(capsule_unsigned(&d, [5; 32], Some(&Version(version.to_vec())), Some("Test Developer"), None, &key.public(), &code, 9).unwrap(), key).unwrap()
    }

    fn entry_of(key: &SigningBundle, c: &[u8]) -> Entry {
        Entry {
            developer: key.public().ed25519(),
            option: [5; 32],
            name: String::from("Clock"),
            developer_name: Some(String::from("Test Developer")),
            version: version_of(c).unwrap().unwrap(),
            capsule: *blake3::hash(c).as_bytes(),
            size: c.len() as u64,
            bundle: key.public(),
            about: String::from("the time, drawn as rings"),
            web: alloc::vec![alloc::format!("http://example.org{}", path(blake3::hash(c).as_bytes()).replace(".entry", ""))],
        }
    }

    fn signed(e: &Entry, key: &SigningBundle) -> Vec<u8> {
        fgtw::release::sign_release(unsigned(e, 11).unwrap(), key).unwrap()
    }

    #[test]
    fn an_entry_round_trips_and_verifies_alone() {
        let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
        let c = capsule(&key, &[0, 1, 0]);
        let e = entry_of(&key, &c);
        let doc = signed(&e, &key);
        assert_eq!(verify(&doc).unwrap(), e);
        assert_eq!(agrees(&e, &c), Ok(()));
        // One byte changed anywhere: refused.
        for at in [doc.len() >> 1, doc.len() - 1] {
            let mut t = doc.clone();
            t[at] ^= 1;
            assert!(verify(&t).is_err(), "a byte changed at {at}");
        }
        assert_eq!(path(&[0xAB; 32]), alloc::format!("/capsules/{}.entry.vsf", "ab".repeat(32)));
    }

    #[test]
    fn an_entry_signed_by_another_key_is_refused() {
        let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
        let other = SigningBundle::developer(&[4; 32], "TEST_DEVELOPER");
        let c = capsule(&key, &[1]);
        // The entry names the developer's bundle but another key signs it: the header names that key, not the bundle's.
        let e = entry_of(&key, &c);
        let mut u = unsigned(&e, 11).unwrap();
        let (ed_mine, ed_other) = (key.public().ed25519(), other.public().ed25519());
        let h = (0..u.len() - 32).find(|&i| u[i..i + 32] == ed_mine[..]).unwrap();
        u[h..h + 32].copy_from_slice(&ed_other);
        let forged = fgtw::release::sign_release(u, &other).unwrap();
        assert!(verify(&forged).is_err());
        // The same Ed25519 key with other post-quantum halves, carried as its own: its eggs verify, so the person's pin is what refuses it (`Kists::judge`).
        let twin = SigningBundle::developer(&[3; 32], "ANOTHER_LABEL");
        let mut e2 = entry_of(&twin, &capsule(&twin, &[1]));
        e2.bundle = twin.public();
        let d = verify(&signed(&e2, &twin)).unwrap();
        assert_ne!(d.bundle.commit(), key.public().commit());
    }

    #[test]
    fn a_capsule_that_disagrees_with_its_entry_is_named() {
        let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
        let c = capsule(&key, &[0, 1, 0]);
        let mut e = entry_of(&key, &c);
        // 0.1.0 and 1.0 are the same version by the stage rule.
        e.version = Version(alloc::vec![1, 0]);
        assert_eq!(agrees(&e, &c), Ok(()));
        e.version = Version(alloc::vec![1, 1]);
        assert_eq!(agrees(&e, &c), Err("the capsule's version is not its entry's"));
        let mut e = entry_of(&key, &c);
        e.name = String::from("Tide");
        assert_eq!(agrees(&e, &c), Err("the capsule's name is not its entry's"));
        let mut e = entry_of(&key, &c);
        e.option = [6; 32];
        assert_eq!(agrees(&e, &c), Err("the capsule is another kist than its entry says"));
        let twin = SigningBundle::developer(&[3; 32], "ANOTHER_LABEL");
        let mut e = entry_of(&key, &c);
        e.bundle = twin.public();
        assert_eq!(agrees(&e, &c), Err("the capsule carries another bundle than its entry"));
    }

    #[test]
    fn an_entry_needs_a_plain_http_host() {
        let key = SigningBundle::developer(&[3; 32], "TEST_DEVELOPER");
        let c = capsule(&key, &[1]);
        let mut e = entry_of(&key, &c);
        e.web = alloc::vec![String::from("https://example.org/x.vsf")];
        assert!(unsigned(&e, 1).is_err());
        e.web.clear();
        assert!(unsigned(&e, 1).is_err());
    }
}
