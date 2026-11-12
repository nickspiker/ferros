//! A note (docs/NOTIFY.md; owner, 2026-11-12: "I think we try it with the in view, it can notify me, out of view, silent"): something a kist has waiting on the person, as one complete VSF file with one section, `note`.
//!
//! - **What a kist says:** the kind it asks for (`kind`), the person it is about if a person wrote (`person`, their identity key), what it means (`idea`, a language-independent meaning, docs/IDEAS.md) and the kist's own words for it (`words`), how many things it stands for (`count`), where a tap on it lands in the kist (`target`, a page's or an item's 32-byte id of the kist's own), the note's own name across the person's devices (`key`), and at most one deadline (`deadline`, Eagle time), after which it goes as if withdrawn.
//! - **What the system adds** (never taken from a kist: the kernel builds every note it keeps and serves from the fields it parsed, `build`): the source (`source`, the kist's capsule address), the note's number among the kist's own (`number`), the kind the kist asked for (`asked`) beside the kind the system decided (`kind`), and when it was posted (`posted`).
//! - **The words** are ASCII text (`a`) in this first cut: the kernel and the map read VSF without the `x` codec (vsf's x-lang16 is 140 KB linked), so a note in another script carries its meaning as an `idea` and its words when the kernel reads `x` (NOTIFY.md, open questions).
//! - **Untrusted input:** a kist's note is parsed with the schema (`parse`); every hash and key is held to its length, the words to `WORDS_MAX` bytes, the whole to `MAX`. Anything else is refused whole, never cut.
use alloc::string::String;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::EtType;
use vsf::VsfType;

/// The section's name.
pub const SECTION: &str = "note";
/// The most bytes a note's document may have: its words, four hashes, a key and a few numbers fit with room.
pub const MAX: usize = 1 << 10;
/// The most bytes of words: a line, never a message (the message stays in the kist).
pub const WORDS_MAX: usize = 1 << 8;

/// The kinds, by what the thing is (owner, 2026-11-12: live, personal, status), lowest first: a kist can always lower its own note, never raise it past what the system sees (docs/NOTIFY.md "Who decides the kind").
/// Status: updates, downloads, anything not from a person: a glow, never a sound.
pub const STATUS: u8 = 0;
/// Personal: a person wrote: one haptic tap (or sound, by the sound mode) as it arrives, in view, and a glow.
pub const PERSONAL: u8 = 1;
/// Live: a beam, an alarm the person set: rings and takes the screen until answered, declined, or ended by the caller.
pub const LIVE: u8 = 2;

/// A note's fields.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Note {
    /// The kind: as the kist asked, in what it posts; as the system decided, in what the kernel serves.
    pub kind: u8,
    pub person: Option<[u8; 32]>,
    pub idea: Option<[u8; 32]>,
    pub words: Option<String>,
    /// How many things it stands for (five messages from one person: one note, count 5); 1 when the kist says nothing.
    pub count: u32,
    pub target: Option<[u8; 32]>,
    pub key: Option<[u8; 32]>,
    /// Eagle time, oscillations.
    pub deadline: Option<i64>,
    /// The system's: the kist's capsule address (all zeros for the system's own notes).
    pub source: Option<[u8; 32]>,
    /// The system's: the note's number among its kist's own (0 to 15).
    pub number: Option<u8>,
    /// The system's: the kind the kist asked for.
    pub asked: Option<u8>,
    /// The system's: when it was posted, Eagle time.
    pub posted: Option<i64>,
}

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION)
        .field("kind", TypeConstraint::AnyUnsigned)
        .field("person", TypeConstraint::Ed25519Key)
        .field("idea", TypeConstraint::Idea)
        .field("words", TypeConstraint::AsciiText)
        .field("count", TypeConstraint::AnyUnsigned)
        .field("target", TypeConstraint::AnyHash)
        .field("key", TypeConstraint::AnyHash)
        .field("deadline", TypeConstraint::AnyEagleTime)
        .field("source", TypeConstraint::AnyHash)
        .field("number", TypeConstraint::AnyUnsigned)
        .field("asked", TypeConstraint::AnyUnsigned)
        .field("posted", TypeConstraint::AnyEagleTime)
}

/// The note as a complete VSF file made at `made` (Eagle time). None when a field is out of its bounds (words past `WORDS_MAX` or not ASCII).
pub fn build(n: &Note, made: i64) -> Option<alloc::vec::Vec<u8>> {
    let mut b = schema().build().set("kind", VsfType::u(n.kind as usize, false)).ok()?;
    let hash = |v: &[u8; 32]| VsfType::hb(v.to_vec());
    if let Some(p) = &n.person {
        b = b.set("person", VsfType::ke(p.to_vec())).ok()?;
    }
    if let Some(i) = &n.idea {
        b = b.set("idea", VsfType::hi(i.to_vec())).ok()?;
    }
    if let Some(w) = &n.words {
        if w.len() > WORDS_MAX {
            return None;
        }
        b = b.set("words", VsfType::a(vsf::Ascii::new(w.clone()).ok()?)).ok()?;
    }
    if n.count != 1 {
        b = b.set("count", VsfType::u(n.count as usize, false)).ok()?;
    }
    for (name, v) in [("target", &n.target), ("key", &n.key), ("source", &n.source)] {
        if let Some(v) = v {
            b = b.set(name, hash(v)).ok()?;
        }
    }
    for (name, v) in [("deadline", n.deadline), ("posted", n.posted)] {
        if let Some(t) = v {
            b = b.set(name, VsfType::e(EtType::e6(t))).ok()?;
        }
    }
    for (name, v) in [("number", n.number), ("asked", n.asked)] {
        if let Some(v) = v {
            b = b.set(name, VsfType::u(v as usize, false)).ok()?;
        }
    }
    let section = b.encode().ok()?;
    let doc = vsf::VsfBuilder::new().creation_time_oscillations(made).provenance_only().add_unboxed(String::from(SECTION), section).build().ok()?;
    (doc.len() <= MAX).then_some(doc)
}

/// A note read back from a complete VSF file: None for anything that is not a note of this shape (a kist's note is untrusted input: refused whole, never cut). The system's fields are read too; the kernel overwrites them on what a kist posts.
pub fn parse(doc: &[u8]) -> Option<Note> {
    // WHY the bound: the document is a kist's; past MAX it is no note, and its parse would cost the kernel's heap.
    if doc.len() > MAX {
        return None;
    }
    let s = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    // A hash or a key of exactly 32 bytes; anything else is not this note's.
    let h32 = |v: Option<VsfType>| -> Result<Option<[u8; 32]>, ()> {
        match v {
            None => Ok(None),
            Some(VsfType::hb(b) | VsfType::hi(b) | VsfType::ke(b)) => b.as_slice().try_into().map(Some).map_err(|_| ()),
            Some(_) => Err(()),
        }
    };
    let time = |v: Option<VsfType>| -> Result<Option<i64>, ()> {
        match v {
            None => Ok(None),
            Some(VsfType::e(EtType::e6(t))) => Ok(Some(t)),
            Some(VsfType::e(EtType::e5(t))) => Ok(Some(t as i64)),
            Some(_) => Err(()),
        }
    };
    // A small unsigned: the kind, the number, the kind asked; past a byte it is not this note's.
    let small = |v: Option<VsfType>| -> Result<Option<u8>, ()> {
        match v {
            None => Ok(None),
            Some(v) => v.as_u64().filter(|&x| x <= 0xFF).map(|x| Some(x as u8)).ok_or(()),
        }
    };
    let kind = small(first("kind")).ok()??;
    if kind > LIVE {
        return None;
    }
    let words = match first("words") {
        None => None,
        Some(VsfType::a(a)) if a.len() <= WORDS_MAX => Some(String::from(&*a)),
        Some(_) => return None,
    };
    let count = match first("count") {
        None => 1,
        // WHY the bound: a count is the kist's word; past a u32 it is not a count of anything waiting.
        Some(v) => v.as_u64().filter(|&c| c <= u32::MAX as u64)? as u32,
    };
    Some(Note {
        kind,
        person: h32(first("person")).ok()?,
        idea: h32(first("idea")).ok()?,
        words,
        count,
        target: h32(first("target")).ok()?,
        key: h32(first("key")).ok()?,
        deadline: time(first("deadline")).ok()?,
        source: h32(first("source")).ok()?,
        number: small(first("number")).ok()?,
        asked: small(first("asked")).ok()?,
        posted: time(first("posted")).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let n = Note { kind: PERSONAL, person: Some([7; 32]), idea: Some([1; 32]), words: Some(String::from("Ana wrote")), count: 3, target: Some([2; 32]), key: Some([3; 32]), deadline: Some(1 << 40), source: Some([4; 32]), number: Some(5), asked: Some(LIVE), posted: Some(1 << 41) };
        let doc = build(&n, 1 << 42).unwrap();
        assert!(doc.len() <= MAX);
        assert_eq!(parse(&doc).unwrap(), n);
    }

    #[test]
    fn bare_note() {
        let n = Note { kind: STATUS, count: 1, ..Default::default() };
        let doc = build(&n, 0).unwrap();
        assert_eq!(parse(&doc).unwrap(), n);
    }

    #[test]
    fn refused() {
        // A kind past live, words past their bound, a short hash: each refused whole.
        let mut n = Note { kind: STATUS, count: 1, ..Default::default() };
        n.words = Some("x".repeat(WORDS_MAX + 1));
        assert!(build(&n, 0).is_none());
        assert!(parse(b"not a note").is_none());
        let bad = schema().build().set("kind", VsfType::u(3, false)).unwrap().encode().unwrap();
        let doc = vsf::VsfBuilder::new().creation_time_oscillations(0).provenance_only().add_unboxed(String::from(SECTION), bad).build().unwrap();
        assert!(parse(&doc).is_none());
        let short = schema().build().set("kind", VsfType::u(0, false)).unwrap().set("key", VsfType::hb(alloc::vec![1; 16])).unwrap().encode().unwrap();
        let doc = vsf::VsfBuilder::new().creation_time_oscillations(0).provenance_only().add_unboxed(String::from(SECTION), short).build().unwrap();
        assert!(parse(&doc).is_none());
    }
}
