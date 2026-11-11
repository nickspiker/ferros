//! Sheets (docs/IDEAS.md "Records"): a complete VSF file whose sections are phrasings and links, signed by its author with every egg (Ed25519, Falcon-512, SPHINCS+), as a kist's capsule is.
//!
//! - **A section `phrasing`**: `idea` (hi), `locale` (wl, its ISO 639-3 code what counts), `text` (x, in its shown form), `register` (unsigned, only when not plain).
//! - **A section `link`**: `kind` (unsigned: 0 same as, 1 related) and `ideas` (two hi).
//! - **The author and the time** are the file's own: the header's signer (`ke`) and creation time (`e`), signed with everything else. A section never repeats them, so they can never disagree.
//! - **Many records, one signature**: a sheet holds as many sections as its author writes at once (ferros's catalog in sixteen languages is one sheet), since every signature costs about 8.6 KB of eggs.
//!
//! [`Sheet::read`] reads any VSF file (a sheet, a capsule) and checks its integrity and Ed25519 anchor, naming no author; [`Sheet::verify`] also checks every egg against the signer's bundle and names the author on every record.

use crate::{id_of_scrubbed, is_locale, is_shown, Id, Locale, Register};
use alloc::string::String;
use alloc::vec::Vec;
use fgtw::pq::KeyBundle;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{VsfHeader, VsfType};

/// A phrasing section's name.
pub const PHRASING: &str = "phrasing";
/// A link section's name.
pub const LINK: &str = "link";

fn phrasing_schema() -> SectionSchema {
    SectionSchema::new(PHRASING)
        .field("idea", TypeConstraint::Idea)
        .field("locale", TypeConstraint::Language)
        .field("text", TypeConstraint::Utf8Text)
        .field("register", TypeConstraint::AnyUnsigned)
}

fn link_schema() -> SectionSchema {
    SectionSchema::new(LINK).field("kind", TypeConstraint::AnyUnsigned).field("ideas", TypeConstraint::Idea)
}

/// An idea in one language and register.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Phrasing {
    pub idea: Id,
    pub locale: Locale,
    /// The words, in their shown form ([`crate::shown`]).
    pub text: String,
    pub register: Register,
    /// The signer of the sheet it came in, once every egg verified against that signer's bundle ([`Sheet::verify`]); None when only read.
    pub author: Option<[u8; 32]>,
    /// The sheet's creation time (Eagle oscillations), when it has one.
    pub created: Option<i64>,
    /// Whether this is the idea's first phrasing: its locale and scrubbed words hash to its idea.
    first: bool,
}

impl Phrasing {
    /// A phrasing of `idea`: the words in their shown form; whether it is the first phrasing is worked out here. Errors: a bad locale, words that are nothing.
    pub fn new(idea: Id, locale: Locale, text: &str, register: Register) -> Result<Phrasing, &'static str> {
        let text = crate::shown(text);
        let first = id_of_scrubbed(locale, &text.replace('\n', " "))? == idea;
        Ok(Phrasing { idea, locale, text, register, author: None, created: None, first })
    }

    /// A new idea: its first phrasing, the id worked out from it.
    pub fn mint(locale: Locale, text: &str, register: Register) -> Result<Phrasing, &'static str> {
        let idea = crate::id(locale, text)?;
        Phrasing::new(idea, locale, text, register)
    }

    /// Whether this is the idea's first phrasing: it proves itself, whoever signed it.
    pub fn is_first(&self) -> bool {
        self.first
    }

    fn section(&self) -> Result<Vec<u8>, String> {
        let e = |e: vsf::schema::ValidationError| alloc::format!("phrasing: {e}");
        let mut b = phrasing_schema()
            .build()
            .set("idea", VsfType::hi(self.idea.to_vec()))
            .map_err(e)?
            .set("locale", VsfType::wl(vsf::types::Language::new(self.locale)))
            .map_err(e)?
            .set("text", VsfType::x(self.text.clone()))
            .map_err(e)?;
        if self.register != Register::Plain {
            b = b.set("register", VsfType::u(self.register.code() as usize, false)).map_err(e)?;
        }
        b.encode().map_err(e)
    }
}

/// What a link says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkKind {
    /// One meaning minted twice: a device that trusts the signer shows either id's phrasings for both (owner, 2026-11-10).
    SameAs,
    /// Close, not the same: helps search, never changes what is shown.
    Related,
}

impl LinkKind {
    const fn code(self) -> u8 {
        match self {
            LinkKind::SameAs => 0,
            LinkKind::Related => 1,
        }
    }

    const fn from_code(c: u64) -> Option<LinkKind> {
        match c {
            0 => Some(LinkKind::SameAs),
            1 => Some(LinkKind::Related),
            _ => None,
        }
    }
}

/// Two ideas joined, as the signer says.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Link {
    pub kind: LinkKind,
    pub ideas: [Id; 2],
    /// As [`Phrasing::author`].
    pub author: Option<[u8; 32]>,
    pub created: Option<i64>,
}

impl Link {
    pub fn new(kind: LinkKind, a: Id, b: Id) -> Link {
        Link { kind, ideas: [a, b], author: None, created: None }
    }

    fn section(&self) -> Result<Vec<u8>, String> {
        let e = |e: vsf::schema::ValidationError| alloc::format!("link: {e}");
        link_schema()
            .build()
            .set("kind", VsfType::u(self.kind.code() as usize, false))
            .map_err(e)?
            .set_multi("ideas", alloc::vec![VsfType::hi(self.ideas[0].to_vec()), VsfType::hi(self.ideas[1].to_vec())])
            .map_err(e)?
            .encode()
            .map_err(e)
    }
}

/// Why [`Sheet::verify`] refused a sheet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refused {
    /// Not a VSF file, or one whose integrity or Ed25519 anchor fails, or a record that does not read.
    Unreadable(&'static str),
    /// No signer, or a signer whose bundle the caller does not hold: nothing to check its eggs against.
    Unknown,
    /// An egg is missing or does not verify against the signer's bundle.
    Eggs,
}

/// The records a sheet carries, and who signed it when.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Sheet {
    /// The header's Ed25519 key, when signed.
    pub signer: Option<[u8; 32]>,
    pub created: Option<i64>,
    pub phrasings: Vec<Phrasing>,
    pub links: Vec<Link>,
}

/// Each `name` section's bytes, in order.
fn bodies<'a>(doc: &'a [u8], header: &'a VsfHeader, name: &'a str) -> impl Iterator<Item = Result<&'a [u8], &'static str>> + 'a {
    header.fields.iter().filter(move |f| f.name == name).map(move |f| {
        // WHY checked_add and the length check: the offset and size are the document's own (from outside); their sum can wrap past usize, or end past the file, and slicing would panic on a hostile sheet instead of refusing it.
        let end = f.offset_bytes.checked_add(f.size_bytes).ok_or("a section's range wraps")?;
        if end > doc.len() {
            return Err("a section past the file's end");
        }
        Ok(&doc[f.offset_bytes..end])
    })
}

fn first_value(b: &SectionBuilder, name: &str) -> Option<VsfType> {
    b.get_fields(name).first().and_then(|f| f.values.first()).cloned()
}

fn idea_of(v: &VsfType) -> Option<Id> {
    match v {
        VsfType::hi(h) => h.as_slice().try_into().ok(),
        _ => None,
    }
}

impl Sheet {
    /// The unsigned sheet `signer` signs: stamped `made` (Eagle oscillations), its header naming `signer` with an egg slot per scheme (`fgtw::release::release_slots`), a section per phrasing then per link. Sign it with `fgtw::release::sign_release` and the author's `SigningBundle`.
    pub fn unsigned(signer: &[u8; 32], made: i64, phrasings: &[Phrasing], links: &[Link]) -> Result<Vec<u8>, String> {
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(made).signed_only_eggs(VsfType::ke(signer.to_vec()), &fgtw::release::release_slots());
        for p in phrasings {
            if !is_shown(&p.text) {
                return Err(String::from("a phrasing's text not in its shown form"));
            }
            b = b.add_unboxed(PHRASING, p.section()?);
        }
        for l in links {
            b = b.add_unboxed(LINK, l.section()?);
        }
        b.build()
    }

    /// The records of any VSF file (a sheet, a capsule), its integrity and Ed25519 anchor checked; no record names an author (only [`Sheet::verify`] does).
    /// A phrasing whose text is not in its shown form, a locale that is not one, or a link without its two ideas refuses the whole file. A phrasing whose register, or a link whose kind, this reader does not know is skipped: a later writer's record, not an error.
    pub fn read(doc: &[u8]) -> Result<Sheet, &'static str> {
        let (header, _) = vsf::verification::read_verified(doc, None).map_err(|_| "not a whole, unbroken VSF file")?;
        let signer = match &header.signer_pubkey {
            Some(VsfType::ke(k)) => Some(k.as_slice().try_into().map_err(|_| "a signer that is not an Ed25519 key")?),
            _ => None,
        };
        let created = header.creation_time.as_ref().and_then(|t| t.as_eagle());
        let mut s = Sheet { signer, created, ..Sheet::default() };
        for body in bodies(doc, &header, PHRASING) {
            let b = SectionBuilder::parse(phrasing_schema(), body?).map_err(|_| "a phrasing that does not read")?;
            let idea = first_value(&b, "idea").as_ref().and_then(idea_of).ok_or("a phrasing without its idea")?;
            let locale = match first_value(&b, "locale") {
                Some(VsfType::wl(l)) => l.code(),
                _ => return Err("a phrasing without its locale"),
            };
            if !is_locale(&locale) {
                return Err("a phrasing whose locale is not ISO 639-3");
            }
            let text = match first_value(&b, "text") {
                Some(VsfType::x(t)) => t,
                _ => return Err("a phrasing without its words"),
            };
            if !is_shown(&text) {
                return Err("a phrasing's text not in its shown form");
            }
            let register = match first_value(&b, "register") {
                None => Register::Plain,
                Some(v) => match v.as_u64().and_then(Register::from_code) {
                    Some(r) => r,
                    None => continue,
                },
            };
            let mut p = Phrasing::new(idea, locale, &text, register)?;
            p.created = created;
            s.phrasings.push(p);
        }
        for body in bodies(doc, &header, LINK) {
            let b = SectionBuilder::parse(link_schema(), body?).map_err(|_| "a link that does not read")?;
            let Some(kind) = first_value(&b, "kind").and_then(|v| v.as_u64()).map(LinkKind::from_code).ok_or("a link without its kind")? else { continue };
            let ideas: Vec<Id> = b.get_fields("ideas").first().map(|f| f.values.iter().filter_map(idea_of).collect()).unwrap_or_default();
            let [a, z] = ideas[..] else { return Err("a link without its two ideas") };
            s.links.push(Link { kind, ideas: [a, z], author: None, created });
        }
        Ok(s)
    }

    /// The records of a signed sheet whose signer's bundle is among `bundles`, every egg verified against it (`fgtw::release::verify_signed_by`: every scheme, never Ed25519 alone); every record names the signer as its author.
    pub fn verify(doc: &[u8], bundles: &[KeyBundle]) -> Result<Sheet, Refused> {
        let mut s = Sheet::read(doc).map_err(Refused::Unreadable)?;
        let signer = s.signer.ok_or(Refused::Unknown)?;
        let bundle = bundles.iter().find(|b| b.ed25519() == signer).ok_or(Refused::Unknown)?;
        fgtw::release::verify_signed_by(doc, &signer, bundle).map_err(|_| Refused::Eggs)?;
        for p in &mut s.phrasings {
            p.author = Some(signer);
        }
        for l in &mut s.links {
            l.author = Some(signer);
        }
        Ok(s)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use fgtw::pq::{FleetSigner, SigningBundle};

    /// A test developer's key: the derivation tools/kistcat gives a developer.
    pub fn key(seed: u8) -> SigningBundle {
        SigningBundle::developer(&[seed; 32], "TEST_IDEAS")
    }

    pub fn signed(k: &SigningBundle, made: i64, phrasings: &[Phrasing], links: &[Link]) -> Vec<u8> {
        fgtw::release::sign_release(Sheet::unsigned(&k.public().ed25519(), made, phrasings, links).unwrap(), k).unwrap()
    }

    #[test]
    fn a_sheet_round_trips_and_verifies() {
        let k = key(3);
        let remove = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let es = Phrasing::new(remove.idea, *b"spa", "Quitar de la flota", Register::Plain).unwrap();
        let terse = Phrasing::new(remove.idea, *b"eng", "Remove", Register::Terse).unwrap();
        let ko = Phrasing::mint(*b"kor", "함대에서 제거", Register::Plain).unwrap();
        let same = Link::new(LinkKind::SameAs, remove.idea, ko.idea);
        let doc = signed(&k, 7, &[remove.clone(), es.clone(), terse.clone(), ko.clone()], &[same.clone()]);

        let read = Sheet::read(&doc).unwrap();
        assert_eq!(read.signer, Some(k.public().ed25519()));
        assert_eq!(read.created, Some(7));
        assert_eq!(read.phrasings.len(), 4);
        assert!(read.phrasings[0].is_first() && !read.phrasings[1].is_first() && !read.phrasings[2].is_first() && read.phrasings[3].is_first());
        assert_eq!(read.phrasings[2].register, Register::Terse);
        assert!(read.phrasings.iter().all(|p| p.author.is_none() && p.created == Some(7)));
        assert_eq!(read.links[0].ideas, same.ideas);
        assert_eq!(read.links[0].kind, LinkKind::SameAs);

        let v = Sheet::verify(&doc, &[key(9).public(), k.public()]).unwrap();
        assert!(v.phrasings.iter().all(|p| p.author == Some(k.public().ed25519())));
        assert_eq!(v.links[0].author, Some(k.public().ed25519()));
        assert_eq!(v.phrasings[1].text, "Quitar de la flota");
    }

    #[test]
    fn a_line_break_is_kept_and_hashed_as_a_space() {
        let p = Phrasing::mint(*b"eng", "wipes from this device:\nthe map", Register::Plain).unwrap();
        assert_eq!(p.text, "wipes from this device:\nthe map");
        assert_eq!(p.idea, crate::id(*b"eng", "wipes from this device: the map").unwrap());
        let doc = signed(&key(3), 1, &[p.clone()], &[]);
        assert!(Sheet::read(&doc).unwrap().phrasings[0].is_first());
    }

    #[test]
    fn a_forged_signature_is_refused() {
        let k = key(3);
        let mallory = key(4);
        let p = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let fake = Phrasing::new(p.idea, *b"spa", "Borrar todo", Register::Plain).unwrap();

        // A sheet naming k as its signer, its eggs made by mallory.
        let u = Sheet::unsigned(&k.public().ed25519(), 1, &[fake.clone()], &[]).unwrap();
        let forged = vsf::verification::sign_file_with(u, |h| mallory.eggs(h, fgtw::release::RELEASE_MASK)).unwrap();
        assert!(Sheet::verify(&forged, &[k.public()]).is_err());
        assert!(Sheet::verify(&forged, &[k.public(), mallory.public()]).is_err());

        // k's own sheet with a byte of its words changed after signing (the last section's bytes end just before the file's closing ']').
        let good = signed(&k, 1, &[Phrasing::new(p.idea, *b"spa", "Quitar de la flota", Register::Plain).unwrap()], &[]);
        assert!(Sheet::verify(&good, &[k.public()]).is_ok());
        let mut bad = good.clone();
        let i = bad.len() - 2;
        bad[i] ^= 1;
        assert!(Sheet::verify(&bad, &[k.public()]).is_err());

        // mallory's own good sheet, verified against a list that does not hold mallory: nothing to check it against.
        let m = signed(&mallory, 1, &[fake], &[]);
        assert_eq!(Sheet::verify(&m, &[k.public()]), Err(Refused::Unknown));
        // A bundle whose Ed25519 half is k's but whose post-quantum halves are another's: the eggs do not all verify.
        let other_pq = SigningBundle::developer(&[3; 32], "ANOTHER_LABEL");
        assert_eq!(other_pq.public().ed25519(), k.public().ed25519());
        assert_eq!(Sheet::verify(&good, &[other_pq.public()]), Err(Refused::Eggs));
    }

    #[test]
    fn unknown_registers_and_kinds_are_skipped_not_fatal() {
        let k = key(3);
        let p = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let later = phrasing_schema()
            .build()
            .set("idea", VsfType::hi(p.idea.to_vec()))
            .unwrap()
            .set("locale", VsfType::wl(vsf::types::Language::new(*b"eng")))
            .unwrap()
            .set("text", VsfType::x(String::from("Take it out")))
            .unwrap()
            .set("register", VsfType::u(7, false))
            .unwrap()
            .encode()
            .unwrap();
        let link = link_schema().build().set("kind", VsfType::u(5, false)).unwrap().set_multi("ideas", alloc::vec![VsfType::hi(p.idea.to_vec()), VsfType::hi(p.idea.to_vec())]).unwrap().encode().unwrap();
        let u = vsf::VsfBuilder::new()
            .creation_time_oscillations(1)
            .signed_only_eggs(VsfType::ke(k.public().ed25519().to_vec()), &fgtw::release::release_slots())
            .add_unboxed(PHRASING, p.section().unwrap())
            .add_unboxed(PHRASING, later)
            .add_unboxed(LINK, link)
            .build()
            .unwrap();
        let s = Sheet::read(&fgtw::release::sign_release(u, &k).unwrap()).unwrap();
        assert_eq!(s.phrasings, alloc::vec![Phrasing { created: Some(1), ..p }]);
        assert!(s.links.is_empty());
    }

    #[test]
    fn text_not_in_its_shown_form_is_refused() {
        let k = key(3);
        let p = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let mut bad = p.clone();
        bad.text = String::from("Remove\u{200B} from fleet");
        assert!(Sheet::unsigned(&k.public().ed25519(), 1, &[bad.clone()], &[]).is_err());
        // Written around the check: the reader refuses it.
        let section = phrasing_schema()
            .build()
            .set("idea", VsfType::hi(p.idea.to_vec()))
            .unwrap()
            .set("locale", VsfType::wl(vsf::types::Language::new(*b"eng")))
            .unwrap()
            .set("text", VsfType::x(bad.text))
            .unwrap()
            .encode()
            .unwrap();
        let u = vsf::VsfBuilder::new().creation_time_oscillations(1).signed_only_eggs(VsfType::ke(k.public().ed25519().to_vec()), &fgtw::release::release_slots()).add_unboxed(PHRASING, section).build().unwrap();
        assert!(Sheet::read(&fgtw::release::sign_release(u, &k).unwrap()).is_err());
    }
}
