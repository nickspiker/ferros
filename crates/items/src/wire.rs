//! The hold's calls over IPC (STORE.md "The calls"): what a kist sends, what the vault process answers, what the bridge sends for the bench.
//!
//! A call is the kernel's stamp (the caller's identity, its `items` kinds, the person's items domain keys), then `rest`: [op u8][handle 32][offset u64 LE][length u32 LE][payload].
//! The payload is a complete VSF document where the call carries one (a proposed card, a find; VSF for control) and the body's bytes for a write (no framing of its own: the transfer is the frame).
//! The answer is the call's value and bytes: a token or handle (32), a card or a found document, body bytes.
//! The vault side is `serve`, the kist side `kist`.
//!
//! The ops keep STORE.md's numbers where it gave them; no versions (owner, §7.5), so `versions` (11) is not one.

use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::{Ascii, EtType};
use vsf::VsfType;

use crate::{document, sections, Card, Error, Find, Found, Proposed, Result};

/// Every answer's value is ferros_sys `ERR` for a refusal (no right to that kind, a handle not the caller's, a malformed call) and `NOTHING` for an item that is gone; otherwise as each op says.
/// A card or a find answers a complete VSF document; its value is the document's whole length, so a caller whose room was short sees it and asks again with more (`CARD_DOC_MAX` always holds a card).
/// A new item: payload a `propose` document → the making's token (32), value 0. Needs the `items` right for that kind.
pub const OP_MAKE: u8 = 0;
/// Body bytes appended to the making (handle: its token), a transfer at a time → value 0.
pub const OP_WRITE: u8 = 1;
/// The body committed and the card placed (handle: the making's token) → a card document carrying the item's handle.
pub const OP_FINISH: u8 = 2;
/// Body bytes from `offset`, at most `length` (handle: the item's) → the bytes; the value is the body's size. A single-copy body reads the same way, from its blocks.
pub const OP_READ: u8 = 3;
/// The card (handle) → a card document.
pub const OP_CARD: u8 = 4;
/// A find: payload a `find` document → a `found` document (its cards each with the caller's handle).
pub const OP_FIND: u8 = 6;
/// Removed, gone at once (handle): its card, its body, a single-copy body's extent → value 0. A clip a live writer still records into is refused.
pub const OP_REMOVE: u8 = 8;
/// Keep again in place (handle: the item's) → the making's token (32); then OP_WRITE and OP_FINISH as for a new item.
pub const OP_KEEP_AGAIN: u8 = 12;
/// A new item whose body is a single-copy extent (a RAW clip): payload a `propose` document, `offset` the blocks wanted → the item's handle (32) then the extent's SYS_BLOCK capability (32; zeros when the kernel gave none, and the body is then written with OP_WRITE_EXTENT); the value the blocks reserved. The card is placed at once, so the clip is found while it records.
pub const OP_MAKE_EXTENT: u8 = 13;
/// The clip sealed (handle: the item's): payload its length u64 LE and BLAKE3 (32) → its card document; the extent's tail given back and the caller's own writes ended.
pub const OP_FINISH_EXTENT: u8 = 14;
/// Whole blocks of the item's extent at block `offset`, at most one UFS command (1 MiB), written once (a caller the kernel gave no capability: the bench) → value 0.
pub const OP_WRITE_EXTENT: u8 = 15;

/// What is on the device, read only (Kūaha's whole view): payload the kists to size, each [developer 32][option 32] (at most `USAGE_KISTS` a call; none sizes none) → a `usage` document: the hold's totals by kind, the clips area, and each stowed kist (from the person's list) with its own domain's size where it was named. Needs the `items` right and the person present; the kernel stamps each named kist's domain (process.rs `items_domains`).
pub const OP_USAGE: u8 = 16;
/// The most kists one usage call sizes (the kernel derives each one's domain keys in the call).
pub const USAGE_KISTS: usize = 1 << 6;

/// The room that always holds one card's document (a card is at most `CARD_MAX`, its document's head and the handle less than a KiB more).
pub const CARD_DOC_MAX: usize = crate::CARD_MAX + (1 << 10);
/// The most one transfer carries: a body's write, a clip's blocks, a read's room (the kernel's ITEMS_CALL_MAX less the head; ferros_sys STORE_GET_MAX).
pub const TRANSFER: usize = 1 << 20;

/// The head of a call's rest.
pub const HEAD: usize = 1 + 32 + 8 + 4;

/// A call's rest: its head and payload.
pub fn call(op: u8, handle: &[u8; 32], offset: u64, len: u32, payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(HEAD + payload.len());
    m.push(op);
    m.extend_from_slice(handle);
    m.extend_from_slice(&offset.to_le_bytes());
    m.extend_from_slice(&len.to_le_bytes());
    m.extend_from_slice(payload);
    m
}

/// A call's rest read back: (op, handle, offset, length, payload). None when shorter than its head.
pub fn parse_call(rest: &[u8]) -> Option<(u8, [u8; 32], u64, u32, &[u8])> {
    if rest.len() < HEAD {
        return None;
    }
    Some((rest[0], rest[1..33].try_into().ok()?, u64::from_le_bytes(rest[33..41].try_into().ok()?), u32::from_le_bytes(rest[41..45].try_into().ok()?), &rest[HEAD..]))
}

fn fmt<T>(_: T) -> Error {
    Error::Format
}

fn propose_schema() -> SectionSchema {
    SectionSchema::new("propose").field("kind", TypeConstraint::Idea).field("title", TypeConstraint::Utf8Text).field("of", TypeConstraint::Any).field("attr", TypeConstraint::Any).field("keeper", TypeConstraint::AnyUnsigned)
}

fn h32(v: &VsfType) -> Option<[u8; 32]> {
    match v {
        VsfType::hp(b) | VsfType::hb(b) | VsfType::hi(b) => b.as_slice().try_into().ok(),
        _ => None,
    }
}

fn eagle_of(v: &VsfType) -> Option<i64> {
    match v {
        VsfType::e(EtType::e6(o)) => Some(*o),
        VsfType::e(EtType::e5(o)) => Some(*o as i64),
        _ => None,
    }
}

fn attrs_of(fields: &[&vsf::schema::section::FieldValue]) -> Vec<(String, VsfType)> {
    fields
        .iter()
        .filter_map(|f| {
            let mut it = f.values.iter();
            match (it.next(), it.next()) {
                (Some(VsfType::d(n)), Some(v)) => Some((String::from(n.as_str()), v.clone())),
                _ => None,
            }
        })
        .collect()
}

/// A proposed card as a `propose` document.
pub fn propose(p: &Proposed, now: i64) -> Result<Vec<u8>> {
    let mut b = propose_schema().build().set("kind", VsfType::hi(p.kind.to_vec())).map_err(fmt)?;
    if let Some(t) = &p.title {
        b = b.set("title", VsfType::x(t.clone())).map_err(fmt)?;
    }
    for (id, how) in &p.of {
        b = b.append_multi("of", alloc::vec![VsfType::from(*id), VsfType::u(*how as usize, false)]).map_err(fmt)?;
    }
    for (name, v) in &p.attrs {
        b = b.append_multi("attr", alloc::vec![VsfType::d(Ascii::new(name.clone()).map_err(fmt)?), v.clone()]).map_err(fmt)?;
    }
    b = b.set("keeper", VsfType::u(p.keeper as usize, false)).map_err(fmt)?;
    document(now, "propose", alloc::vec![b.encode().map_err(fmt)?], None)
}

pub fn parse_propose(doc: &[u8]) -> Result<Proposed> {
    let s = sections(doc, "propose")?;
    let s = SectionBuilder::parse(propose_schema(), s.first().ok_or(Error::Format)?).map_err(fmt)?;
    let one = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    let mut of = Vec::new();
    for f in s.get_fields("of") {
        let id = f.values.iter().find_map(crate::ht_of);
        if let (Some(id), Some(how)) = (id, f.values.iter().find_map(|v| v.as_u64())) {
            of.push((id, how));
        }
    }
    Ok(Proposed {
        kind: one("kind").as_ref().and_then(h32).ok_or(Error::Format)?,
        title: match one("title") {
            Some(VsfType::x(t)) => Some(t),
            _ => None,
        },
        of,
        attrs: attrs_of(&s.get_fields("attr")),
        keeper: one("keeper").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

/// The attribute conditions' field names: the condition is the field (`IS_*` in that order), never a number beside the value.
const WHERE: [&str; 4] = ["where_equal", "where_at_least", "where_at_most", "where_words"];

fn find_schema() -> SectionSchema {
    let mut s = SectionSchema::new("find")
        .field("kind", TypeConstraint::Idea)
        .field("maker", TypeConstraint::Blake3Rolling)
        .field("from", TypeConstraint::AnyEagleTime)
        .field("to", TypeConstraint::AnyEagleTime)
        .field("changed", TypeConstraint::AnyEagleTime)
        .field("words", TypeConstraint::Utf8Text)
        .field("of", TypeConstraint::Blake3Rolling)
        .field("order", TypeConstraint::AnyUnsigned)
        .field("skip", TypeConstraint::AnyUnsigned)
        .field("count", TypeConstraint::AnyUnsigned);
    for w in WHERE {
        s = s.field(w, TypeConstraint::Any);
    }
    s
}

/// A find as a `find` document.
pub fn find(f: &Find, now: i64) -> Result<Vec<u8>> {
    let mut b = find_schema().build();
    if !f.kinds.is_empty() {
        b = b.set_multi("kind", f.kinds.iter().map(|k| VsfType::hi(k.to_vec())).collect()).map_err(fmt)?;
    }
    if let Some(m) = &f.maker {
        b = b.set("maker", VsfType::hb(m.to_vec())).map_err(fmt)?;
    }
    for (name, t) in [("from", f.from), ("to", f.to), ("changed", f.changed)] {
        if let Some(t) = t {
            b = b.set(name, VsfType::e(EtType::e6(t))).map_err(fmt)?;
        }
    }
    if let Some(w) = &f.words {
        b = b.set("words", VsfType::x(w.clone())).map_err(fmt)?;
    }
    if let Some(h) = &f.of {
        b = b.set("of", VsfType::hb(h.to_vec())).map_err(fmt)?;
    }
    for (name, is, v) in &f.wheres {
        let field = WHERE.get(*is as usize).ok_or(Error::Format)?;
        b = b.append_multi(*field, alloc::vec![VsfType::d(Ascii::new(name.clone()).map_err(fmt)?), v.clone()]).map_err(fmt)?;
    }
    for (name, v) in [("order", f.order), ("skip", f.skip), ("count", f.count)] {
        b = b.set(name, VsfType::u(v as usize, false)).map_err(fmt)?;
    }
    document(now, "find", alloc::vec![b.encode().map_err(fmt)?], None)
}

pub fn parse_find(doc: &[u8]) -> Result<Find> {
    let s = sections(doc, "find")?;
    let s = SectionBuilder::parse(find_schema(), s.first().ok_or(Error::Format)?).map_err(fmt)?;
    let one = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    let mut wheres = Vec::new();
    for (is, field) in WHERE.iter().enumerate() {
        for (name, v) in attrs_of(&s.get_fields(field)) {
            wheres.push((name, is as u64, v));
        }
    }
    Ok(Find {
        kinds: s.get_fields("kind").iter().flat_map(|f| f.values.iter().filter_map(h32)).collect(),
        maker: one("maker").as_ref().and_then(h32),
        from: one("from").as_ref().and_then(eagle_of),
        to: one("to").as_ref().and_then(eagle_of),
        changed: one("changed").as_ref().and_then(eagle_of),
        words: match one("words") {
            Some(VsfType::x(t)) => Some(t),
            _ => None,
        },
        of: one("of").as_ref().and_then(h32),
        wheres,
        order: one("order").and_then(|v| v.as_u64()).unwrap_or(0),
        skip: one("skip").and_then(|v| v.as_u64()).unwrap_or(0),
        count: one("count").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

fn found_schema() -> SectionSchema {
    SectionSchema::new("found").field("total", TypeConstraint::AnyUnsigned).field("generation", TypeConstraint::AnyUnsigned)
}

/// A find's answer as a `found` document: its head, then a `card` section an item, each with the caller's handle.
pub fn found(f: &Found, now: i64) -> Result<Vec<u8>> {
    let head = found_schema().build().set("total", VsfType::u(f.total as usize, false)).map_err(fmt)?.set("generation", VsfType::u(f.generation as usize, false)).map_err(fmt)?.encode().map_err(fmt)?;
    let cards = f.items.iter().map(|(h, c)| c.section(Some(h))).collect::<Result<Vec<_>>>()?;
    document(now, "card", cards, Some(("found", head)))
}

pub fn parse_found(doc: &[u8]) -> Result<Found> {
    let head = sections(doc, "found")?;
    let h = SectionBuilder::parse(found_schema(), head.first().ok_or(Error::Format)?).map_err(fmt)?;
    let one = |name: &str| h.get_fields(name).first().and_then(|f| f.values.first()).and_then(|v| v.as_u64());
    let mut items = Vec::new();
    for s in sections(doc, "card")? {
        let (c, handle) = Card::parse(s)?;
        items.push((handle.ok_or(Error::Format)?, c));
    }
    Ok(Found { total: one("total").ok_or(Error::Format)?, generation: one("generation").unwrap_or(0), items })
}

/// A single card answered with its handle (OP_FINISH, OP_CARD), as a document of one `card`.
pub fn card_doc(c: &Card, handle: &[u8; 32], now: i64) -> Result<Vec<u8>> {
    document(now, "card", alloc::vec![c.section(Some(handle))?], None)
}

/// A body kept (OP_FINISH): the item's handle and card, and how many copies of it the vault wrote and read back equal before it answered.
/// The vault keeps two (manifestus's mirror, write-verify-then-mirror): `copies` 2 is the body, its card and the commit naming them each written and read back equal on both, before the answer was sent; 1 is the vault running on one copy (the other failed, at this write or earlier, and was dropped until the vault opens again), everything verified on that one. None: a vault from before 2026-11-22, which does not say.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub handle: [u8; 32],
    pub card: Card,
    pub copies: Option<u8>,
}

fn kept_schema() -> SectionSchema {
    SectionSchema::new("kept").field("copies", TypeConstraint::AnyUnsigned)
}

/// OP_FINISH's answer: the card document with a `kept` head saying the copies.
pub fn kept_doc(c: &Card, handle: &[u8; 32], now: i64, copies: u8) -> Result<Vec<u8>> {
    let head = kept_schema().build().set("copies", VsfType::u(copies as usize, false)).map_err(fmt)?.encode().map_err(fmt)?;
    document(now, "card", alloc::vec![c.section(Some(handle))?], Some(("kept", head)))
}

pub fn parse_kept_doc(doc: &[u8]) -> Result<Kept> {
    let (handle, card) = parse_card_doc(doc)?;
    let copies = match sections(doc, "kept")?.first() {
        Some(s) => {
            let h = SectionBuilder::parse(kept_schema(), s).map_err(fmt)?;
            let n = h.get_fields("copies").first().and_then(|f| f.values.first()).and_then(|v| v.as_u64()).ok_or(Error::Format)?;
            // WHY: the count is the vault's; a value past a byte is a bad answer, not a count to truncate.
            Some(u8::try_from(n).map_err(|_| Error::Format)?)
        }
        None => None,
    };
    Ok(Kept { handle, card, copies })
}

pub fn parse_card_doc(doc: &[u8]) -> Result<([u8; 32], Card)> {
    let s = sections(doc, "card")?;
    let (c, h) = Card::parse(s.first().ok_or(Error::Format)?)?;
    Ok((h.ok_or(Error::Format)?, c))
}

/// One kind's share of the hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindUse {
    pub kind: [u8; 32],
    pub items: u64,
    pub bytes: u64,
}

/// One stowed kist and what it keeps for itself: its own domain's values and their bytes (None where the call did not name it), and the clips area's blocks its own extents take (Lumis's clips recorded into its domain, not the hold).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KistUse {
    pub developer: [u8; 32],
    pub option: [u8; 32],
    pub name: String,
    pub values: Option<u64>,
    pub bytes: Option<u64>,
    pub clip_blocks: u64,
}

/// The clips area (single-copy extents, 4 KiB blocks): its blocks, the blocks every extent takes, the longest free stretch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClipsUse {
    pub blocks: u64,
    pub taken: u64,
    pub largest_free: u64,
}

/// What is on the device (OP_USAGE).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Usage {
    pub kinds: Vec<KindUse>,
    pub clips: Option<ClipsUse>,
    pub kists: Vec<KistUse>,
}

fn usage_schema() -> SectionSchema {
    SectionSchema::new("usage").field("clips_blocks", TypeConstraint::AnyUnsigned).field("clips_taken", TypeConstraint::AnyUnsigned).field("clips_largest_free", TypeConstraint::AnyUnsigned)
}

fn kind_use_schema() -> SectionSchema {
    SectionSchema::new("kind").field("kind", TypeConstraint::Idea).field("items", TypeConstraint::AnyUnsigned).field("bytes", TypeConstraint::AnyUnsigned)
}

fn kist_use_schema() -> SectionSchema {
    SectionSchema::new("kist")
        .field("developer", TypeConstraint::Ed25519Key)
        .field("option", TypeConstraint::Any)
        .field("name", TypeConstraint::Utf8Text)
        .field("values", TypeConstraint::AnyUnsigned)
        .field("bytes", TypeConstraint::AnyUnsigned)
        .field("clip_blocks", TypeConstraint::AnyUnsigned)
}

fn u(v: u64) -> VsfType {
    VsfType::u(v as usize, false)
}

/// A usage answer as a `usage` document: its head (the clips area), a `kind` section a kind, a `kist` section a kist.
pub fn usage(x: &Usage, now: i64) -> Result<Vec<u8>> {
    let mut head = usage_schema().build();
    if let Some(c) = &x.clips {
        head = head.set("clips_blocks", u(c.blocks)).map_err(fmt)?.set("clips_taken", u(c.taken)).map_err(fmt)?.set("clips_largest_free", u(c.largest_free)).map_err(fmt)?;
    }
    let mut b = vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed("usage", head.encode().map_err(fmt)?);
    for k in &x.kinds {
        let s = kind_use_schema().build().set("kind", VsfType::hi(k.kind.to_vec())).map_err(fmt)?.set("items", u(k.items)).map_err(fmt)?.set("bytes", u(k.bytes)).map_err(fmt)?;
        b = b.add_unboxed("kind", s.encode().map_err(fmt)?);
    }
    for k in &x.kists {
        let mut s = kist_use_schema().build().set("developer", VsfType::ke(k.developer.to_vec())).map_err(fmt)?.set("option", VsfType::hR(k.option.to_vec())).map_err(fmt)?.set("name", VsfType::x(k.name.clone())).map_err(fmt)?.set("clip_blocks", u(k.clip_blocks)).map_err(fmt)?;
        if let (Some(v), Some(n)) = (k.values, k.bytes) {
            s = s.set("values", u(v)).map_err(fmt)?.set("bytes", u(n)).map_err(fmt)?;
        }
        b = b.add_unboxed("kist", s.encode().map_err(fmt)?);
    }
    b.build().map_err(fmt)
}

pub fn parse_usage(doc: &[u8]) -> Result<Usage> {
    let head = sections(doc, "usage")?;
    let h = SectionBuilder::parse(usage_schema(), head.first().ok_or(Error::Format)?).map_err(fmt)?;
    let hu = |n: &str| h.get_fields(n).first().and_then(|f| f.values.first()).and_then(|v| v.as_u64());
    let clips = match (hu("clips_blocks"), hu("clips_taken"), hu("clips_largest_free")) {
        (Some(blocks), Some(taken), Some(largest_free)) => Some(ClipsUse { blocks, taken, largest_free }),
        _ => None,
    };
    let mut x = Usage { kinds: Vec::new(), clips, kists: Vec::new() };
    for s in sections(doc, "kind")? {
        let p = SectionBuilder::parse(kind_use_schema(), s).map_err(fmt)?;
        let one = |n: &str| p.get_fields(n).first().and_then(|f| f.values.first()).cloned();
        x.kinds.push(KindUse { kind: one("kind").as_ref().and_then(h32).ok_or(Error::Format)?, items: one("items").and_then(|v| v.as_u64()).ok_or(Error::Format)?, bytes: one("bytes").and_then(|v| v.as_u64()).ok_or(Error::Format)? });
    }
    for s in sections(doc, "kist")? {
        let p = SectionBuilder::parse(kist_use_schema(), s).map_err(fmt)?;
        let one = |n: &str| p.get_fields(n).first().and_then(|f| f.values.first()).cloned();
        let key = |n: &str| match one(n) {
            Some(VsfType::ke(b)) | Some(VsfType::hR(b)) => <[u8; 32]>::try_from(&b[..]).ok(),
            _ => None,
        };
        x.kists.push(KistUse {
            developer: key("developer").ok_or(Error::Format)?,
            option: key("option").ok_or(Error::Format)?,
            name: match one("name") {
                Some(VsfType::x(t)) => t,
                _ => String::new(),
            },
            values: one("values").and_then(|v| v.as_u64()),
            bytes: one("bytes").and_then(|v| v.as_u64()),
            clip_blocks: one("clip_blocks").and_then(|v| v.as_u64()).unwrap_or(0),
        });
    }
    Ok(x)
}
