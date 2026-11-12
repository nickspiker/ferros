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

/// What the system sees of a note's poster, for `decide`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Poster {
    /// The system's own note (an alarm the person set, the development command).
    System,
    /// A kist: whether it holds the network (`udp`: a person's words reach a kist only over a link).
    Kist { network: bool },
}

/// The kind a note gets (docs/NOTIFY.md "Who decides the kind"): the kind asked for, lowered to what the system can see, never raised. The system's own: as asked. A kist's: never live yet (a beam is live once the system's beam path says one is open, an alarm once the person set it thru the system; neither is built), personal when it names a person and the kist holds the network, else status.
pub fn decide(asked: u8, names_person: bool, poster: Poster) -> u8 {
    let ceiling = match poster {
        Poster::System => LIVE,
        Poster::Kist { network: true } if names_person => PERSONAL,
        Poster::Kist { .. } => STATUS,
    };
    asked.min(ceiling)
}

/// The lowest note number free among a kist's (bit k of `used` set: number k is taken), or None when all `WAITING` are.
pub fn free_number(used: u16) -> Option<u8> {
    let free = (!used).trailing_zeros();
    // WHY the bound: `!used` of a full mask is zero, whose trailing zeros are 16, past every number.
    (free < WAITING as u32).then_some(free as u8)
}

/// Whether a circle at (`cx`, `cy`), radius `r` (panel pixels, as the resting view showed it) is in view (docs/NOTIFY.md "The resting view"): its disc meets the screen's rectangle, `w` by `h`. The screen's nearest point to the centre is the centre held to the rectangle (the nearest point's definition, not a guard).
pub fn in_view(cx: i64, cy: i64, r: i64, w: i64, h: i64) -> bool {
    let (nx, ny) = (cx.clamp(0, w), cy.clamp(0, h));
    (cx - nx) * (cx - nx) + (cy - ny) * (cy - ny) <= r * r
}

/// A kist's notes waiting at once: a note's number is a nibble.
pub const WAITING: usize = 1 << 4;
/// A kist's posts between two looks from the person (a revision of a waiting note counts too).
pub const POSTS: u16 = 1 << 6;

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
    fn a_kist_lowers_never_raises() {
        // The system: as asked.
        for asked in [STATUS, PERSONAL, LIVE] {
            assert_eq!(decide(asked, false, Poster::System), asked);
        }
        // A kist asking live: personal at most, and only naming a person with the network.
        assert_eq!(decide(LIVE, true, Poster::Kist { network: true }), PERSONAL);
        assert_eq!(decide(LIVE, true, Poster::Kist { network: false }), STATUS);
        assert_eq!(decide(LIVE, false, Poster::Kist { network: true }), STATUS);
        assert_eq!(decide(PERSONAL, false, Poster::Kist { network: true }), STATUS);
        // Lowering is always the kist's.
        assert_eq!(decide(STATUS, true, Poster::Kist { network: true }), STATUS);
    }

    #[test]
    fn in_view_is_the_disc_meeting_the_screen() {
        let (w, h) = (1080, 2400);
        // Inside, and straddling an edge.
        assert!(in_view(540, 1200, 100, w, h));
        assert!(in_view(-50, 1200, 100, w, h));
        // Wholly off one side.
        assert!(!in_view(-101, 1200, 100, w, h));
        assert!(!in_view(540, 2501, 100, w, h));
        // Near a corner: the box would meet it, the disc does not (70.7 from the corner's diagonal at 71 each way, radius 100).
        assert!(!in_view(-71, -71, 100, w, h));
        assert!(in_view(-70, -70, 100, w, h));
        // A huge circle around the whole screen.
        assert!(in_view(540, 1200, 1 << 20, w, h));
    }

    #[test]
    fn numbers_by_count() {
        assert_eq!(free_number(0), Some(0));
        assert_eq!(free_number(0b1011), Some(2));
        assert_eq!(free_number(0x7FFF), Some(15));
        assert_eq!(free_number(0xFFFF), None);
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
