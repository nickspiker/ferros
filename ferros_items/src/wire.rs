//! The hold's calls over IPC (STORE.md "The calls"): what a kist sends, what the vault process answers, what the bridge sends for the bench.
//!
//! A call is the kernel's stamp (the caller's identity, its `items` kinds, the person's items domain keys), then `rest`: [op u8][handle 32][offset u64 LE][length u32 LE][payload].
//! The payload is a complete VSF document where the call carries one (a proposed card, a find; VSF for control) and the body's bytes for a write (no framing of its own: the transfer is the frame).
//! The answer is the call's value and bytes: a handle (32), a card or a found document, body bytes.
//!
//! The ops keep STORE.md's numbers where it gave them; no versions (owner, §7.5), so `versions` (11) is not one.

use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::{Ascii, EtType};
use vsf::VsfType;

use crate::{document, sections, Card, Error, Find, Found, Proposed, Result};

/// A new item: payload a `propose` document → the making's handle (32). Needs the `items` right for that kind.
pub const OP_MAKE: u8 = 0;
/// Body bytes at the end of the making (handle) → 0.
pub const OP_WRITE: u8 = 1;
/// The body committed and the card placed (handle: the making's) → the item's handle (32) then its card section.
pub const OP_FINISH: u8 = 2;
/// Body bytes at offset, length at most the transfer (handle) → the bytes; the value is the body's size.
pub const OP_READ: u8 = 3;
/// The card (handle) → its section.
pub const OP_CARD: u8 = 4;
/// A find: payload a `find` document → a `found` document.
pub const OP_FIND: u8 = 6;
/// Removed, gone at once (handle) → 0.
pub const OP_REMOVE: u8 = 8;
/// Keep again in place (handle: the item's) → the making's handle (32).
pub const OP_KEEP_AGAIN: u8 = 12;
/// A new item whose body is a single-copy extent: payload a `propose` document, offset the blocks wanted → the making's handle (32) then the extent's SYS_BLOCK capability (32, zeros when the kernel gave none: then the body is written with OP_WRITE_EXTENT).
pub const OP_MAKE_EXTENT: u8 = 13;
/// The extent's clip sealed (handle: the making's): payload its length u64 LE and BLAKE3 (32) → the item's handle then its card section; the extent's tail given back.
pub const OP_FINISH_EXTENT: u8 = 14;
/// Whole blocks of the making's extent at block `offset`, written once (a kernel that gave no capability) → 0.
pub const OP_WRITE_EXTENT: u8 = 15;

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
        b = b.append_multi("of", alloc::vec![VsfType::hp(id.to_vec()), VsfType::u(*how as usize, false)]).map_err(fmt)?;
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
        let id = f.values.iter().find_map(|v| if let VsfType::hp(_) = v { h32(v) } else { None });
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

pub fn parse_card_doc(doc: &[u8]) -> Result<([u8; 32], Card)> {
    let s = sections(doc, "card")?;
    let (c, h) = Card::parse(s.first().ok_or(Error::Format)?)?;
    Ok((h.ok_or(Error::Format)?, c))
}
