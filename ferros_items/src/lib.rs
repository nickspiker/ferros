//! The hold's items (docs/STORE.md): what a person keeps, found by what is true of it, never by a place.
//!
//! An item is a body and a card.
//! The card (section `card`, a few hundred bytes, at most `CARD_MAX`) says what is true of it: its id, its kind (an idea, IDEAS.md), the kist that made it, when, its title, its size and its body's BLAKE3, its relations and its kind's own attributes.
//! The body is a ferros_store value, or, for a body too large to be written twice (a RAW clip: owner, 2026-11-19, "because of the 1:1 write speed, with no dupes since it's massive"), a single-copy extent of the clips area, written once at the flash's rate and named on the card; the card is mirrored and indexed like any other.
//!
//! The cards live in time pages: a page holds the cards of items made in a span of 2^k Eagle oscillations from a multiple of 2^k, one VSF document of `card` sections, split in two halves when it passes one ferros_store piece; the root lists the pages with the kinds and makers each holds, so a find reads only the pages that can answer it.
//! Pages, root and bodies are written without entering ferros_store's list of names (`begin_unlisted`): this index is their list.
//!
//! The owner's rules (STORE.md §7): keeping an item again replaces it in place, no versions (5); removed is gone at once, no bin (7).
//!
//! Who may do what is the caller's stamp (`Caller`, from the kernel, never the kist's word): a kist makes the kinds its `items` right declares, opens the kinds it was let open and everything it made itself (§7 question 2, proposed yes); the person (the kernel's own channel, the find page) opens all.
//! A kist names an item by a handle, a keyed token for its own identity, never the item's id.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod kinds;
pub mod kist;
pub mod serve;
pub mod wire;

use alloc::string::String;
use alloc::vec::Vec;
use ferros_store::{Domain, Nonces, Store, Writer};
use manifestus::BlockDev;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::{Ascii, EtType};
use vsf::VsfType;

/// A card is bounded to one 4 KiB block (STORE.md "The card"): a kind that wants more puts it in its body.
pub const CARD_MAX: usize = 1 << 12;
/// The widest page: 2^56 oscillations of the hydrogen line, about 1.6 years.
pub const TOP_SPAN: u8 = 56;
/// The narrowest a page splits to: 2^42 oscillations, about 52 minutes; a page still too full there grows past one piece (ferros_store values have no limit).
pub const MIN_SPAN: u8 = 42;
/// The most cards one find answers (`Find::count`).
pub const FIND_MAX: usize = 1 << 8;
/// Kept for the person (0) or by the kist for itself (1).
pub const KEEPER_PERSON: u64 = 0;
pub const KEEPER_KIST: u64 = 1;
/// A relation's kind (`Card::of`): made from (a crop of), belongs with (a save of a ROM).
pub const OF_MADE_FROM: u64 = 0;
pub const OF_BELONGS: u64 = 1;

/// The hold's own values in the items domain, never entered in ferros_store's list: names beginning with 2 (a process's names are printable ASCII, the store's books begin with 0, kete's addresses with 1).
const ROOT: &[u8] = b"\x02root";
const PENDING: &[u8] = b"\x02pending";
/// The single-copy bodies not yet sealed: each item's id and when it was made (its page), so a clip whose writer died is found again and its card corrected once the vault has closed it (`open_clips`, `amend_extent`), without reading every page.
const CLIPS: &[u8] = b"\x02clips";
const PAGE_TAG: u8 = b'p';
const BODY_TAG: u8 = b'b';

#[derive(Debug)]
pub enum Error {
    Store(ferros_store::Error),
    /// The caller may not: its stamp holds no right to that kind, or the handle is not one it was given.
    Refused,
    /// No such item (removed, or a handle from before).
    NotFound,
    /// A card, page or root that does not read as one.
    Format,
    /// A card past CARD_MAX, a find past FIND_MAX.
    TooLarge,
    /// The body is an extent: the caller reads its blocks (`Read::Extent`), or the call wants a stored body.
    Extent,
}

impl From<ferros_store::Error> for Error {
    fn from(e: ferros_store::Error) -> Error {
        Error::Store(e)
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// The kist that made an item (its developer key and option, its identity, KISTS.md) and its name then, so the card still reads after it is removed.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Maker {
    pub developer: [u8; 32],
    pub option: [u8; 32],
    pub name: String,
}

impl Maker {
    /// What the root and a find know a maker by: BLAKE3 of its identity.
    pub fn tag(&self) -> [u8; 32] {
        maker_tag(&self.developer, &self.option)
    }
}

pub fn maker_tag(developer: &[u8; 32], option: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("ferros items 2026-11-19 maker");
    h.update(developer);
    h.update(option);
    *h.finalize().as_bytes()
}

/// A single-copy body: blocks of the clips area (manifestus `extent`), named in the extents' table by the item's id in the items domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub first: u64,
    pub blocks: u64,
}

impl Extent {
    /// Whether `n` blocks from block `at` of the extent lie inside it (manifestus `Extent::holds`).
    pub fn holds(&self, at: u64, n: u64) -> bool {
        // WHY checked: `at` and `n` are a caller's words; a sum that wraps is outside, not inside.
        at.checked_add(n).is_some_and(|end| end <= self.blocks)
    }
}

/// An item's card (STORE.md "The card, as VSF").
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: [u8; 32],
    pub kind: [u8; 32],
    pub kist: Maker,
    /// The device it was made on, when the stamp names one.
    pub device: Option<[u8; 32]>,
    /// Made by someone else and received (a photo Ana sent); none for the person's own.
    pub person: Option<[u8; 32]>,
    /// Eagle time, oscillations.
    pub made: i64,
    pub changed: i64,
    pub title: Option<String>,
    /// The body's bytes and BLAKE3 (an extent's: the clip's seal, as its recorder gave it).
    pub size: u64,
    pub body: [u8; 32],
    pub extent: Option<Extent>,
    /// Relations: another item's id and how (`OF_*`).
    pub of: Vec<([u8; 32], u64)>,
    /// The kind's own facts, by name and type.
    pub attrs: Vec<(String, VsfType)>,
    pub keeper: u64,
}

/// What a kist proposes when it makes an item; the store adds who, when, where and what.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Proposed {
    pub kind: [u8; 32],
    pub title: Option<String>,
    pub of: Vec<([u8; 32], u64)>,
    pub attrs: Vec<(String, VsfType)>,
    pub keeper: u64,
}

fn card_schema() -> SectionSchema {
    SectionSchema::new("card")
        .field("id", TypeConstraint::Blake3Provenance)
        .field("kind", TypeConstraint::Idea)
        .field("kist", TypeConstraint::Any)
        .field("device", TypeConstraint::Ed25519Key)
        .field("person", TypeConstraint::Ed25519Key)
        .field("made", TypeConstraint::AnyEagleTime)
        .field("changed", TypeConstraint::AnyEagleTime)
        .field("title", TypeConstraint::Utf8Text)
        .field("size", TypeConstraint::AnyUnsigned)
        .field("body", TypeConstraint::Blake3Rolling)
        .field("extent_at", TypeConstraint::AnyUnsigned)
        .field("extent_blocks", TypeConstraint::AnyUnsigned)
        .field("of", TypeConstraint::Any)
        .field("attr", TypeConstraint::Any)
        .field("keeper", TypeConstraint::AnyUnsigned)
        .field("handle", TypeConstraint::Blake3Rolling)
}

fn e6(t: i64) -> VsfType {
    VsfType::e(EtType::e6(t))
}

fn eagle_of(v: &VsfType) -> Option<i64> {
    match v {
        VsfType::e(EtType::e6(o)) => Some(*o),
        VsfType::e(EtType::e5(o)) => Some(*o as i64),
        _ => None,
    }
}

fn h32(v: &VsfType) -> Option<[u8; 32]> {
    match v {
        VsfType::hp(b) | VsfType::hb(b) | VsfType::hi(b) | VsfType::ke(b) | VsfType::hR(b) => b.as_slice().try_into().ok(),
        _ => None,
    }
}

fn fmt<T>(_: T) -> Error {
    Error::Format
}

impl Card {
    /// The card's section bytes (with `handle` when it goes to a caller; never at rest).
    pub fn section(&self, handle: Option<&[u8; 32]>) -> Result<Vec<u8>> {
        let mut b = card_schema().build();
        b = b.set("id", VsfType::hp(self.id.to_vec())).map_err(fmt)?;
        b = b.set("kind", VsfType::hi(self.kind.to_vec())).map_err(fmt)?;
        b = b.set_multi("kist", alloc::vec![VsfType::ke(self.kist.developer.to_vec()), VsfType::hR(self.kist.option.to_vec()), VsfType::x(self.kist.name.clone())]).map_err(fmt)?;
        if let Some(k) = &self.device {
            b = b.set("device", VsfType::ke(k.to_vec())).map_err(fmt)?;
        }
        if let Some(k) = &self.person {
            b = b.set("person", VsfType::ke(k.to_vec())).map_err(fmt)?;
        }
        b = b.set("made", e6(self.made)).map_err(fmt)?;
        b = b.set("changed", e6(self.changed)).map_err(fmt)?;
        if let Some(t) = &self.title {
            b = b.set("title", VsfType::x(t.clone())).map_err(fmt)?;
        }
        b = b.set("size", VsfType::u(self.size as usize, false)).map_err(fmt)?;
        b = b.set("body", VsfType::hb(self.body.to_vec())).map_err(fmt)?;
        if let Some(x) = &self.extent {
            b = b.set("extent_at", VsfType::u(x.first as usize, false)).map_err(fmt)?;
            b = b.set("extent_blocks", VsfType::u(x.blocks as usize, false)).map_err(fmt)?;
        }
        for (id, how) in &self.of {
            b = b.append_multi("of", alloc::vec![VsfType::hp(id.to_vec()), VsfType::u(*how as usize, false)]).map_err(fmt)?;
        }
        for (name, v) in &self.attrs {
            let n = Ascii::new(name.clone()).map_err(fmt)?;
            b = b.append_multi("attr", alloc::vec![VsfType::d(n), v.clone()]).map_err(fmt)?;
        }
        b = b.set("keeper", VsfType::u(self.keeper as usize, false)).map_err(fmt)?;
        if let Some(h) = handle {
            b = b.set("handle", VsfType::hb(h.to_vec())).map_err(fmt)?;
        }
        b.encode().map_err(fmt)
    }

    /// A card from its section's bytes, and its handle when it carries one. Every value matched by its type marker, never its place.
    pub fn parse(bytes: &[u8]) -> Result<(Card, Option<[u8; 32]>)> {
        let s = SectionBuilder::parse(card_schema(), bytes).map_err(fmt)?;
        let one = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let mut kist = Maker::default();
        for f in s.get_fields("kist") {
            for v in &f.values {
                match v {
                    VsfType::ke(b) if b.len() == 32 => kist.developer.copy_from_slice(b),
                    VsfType::hR(b) if b.len() == 32 => kist.option.copy_from_slice(b),
                    VsfType::x(t) => kist.name = t.clone(),
                    _ => {}
                }
            }
        }
        let mut of = Vec::new();
        for f in s.get_fields("of") {
            let id = f.values.iter().find_map(|v| if let VsfType::hp(_) = v { h32(v) } else { None });
            let how = f.values.iter().find_map(|v| v.as_u64());
            if let (Some(id), Some(how)) = (id, how) {
                of.push((id, how));
            }
        }
        let mut attrs = Vec::new();
        for f in s.get_fields("attr") {
            // The attribute's name is the field's first `d`; its value the first value after it.
            let mut it = f.values.iter();
            if let (Some(VsfType::d(n)), Some(v)) = (it.next(), it.next()) {
                attrs.push((String::from(n.as_str()), v.clone()));
            }
        }
        let extent = match (one("extent_at").and_then(|v| v.as_u64()), one("extent_blocks").and_then(|v| v.as_u64())) {
            (Some(first), Some(blocks)) => Some(Extent { first, blocks }),
            _ => None,
        };
        let card = Card {
            id: one("id").as_ref().and_then(h32).ok_or(Error::Format)?,
            kind: one("kind").as_ref().and_then(h32).ok_or(Error::Format)?,
            kist,
            device: one("device").as_ref().and_then(h32),
            person: one("person").as_ref().and_then(h32),
            made: one("made").as_ref().and_then(eagle_of).ok_or(Error::Format)?,
            changed: one("changed").as_ref().and_then(eagle_of).ok_or(Error::Format)?,
            title: match one("title") {
                Some(VsfType::x(t)) => Some(t),
                _ => None,
            },
            size: one("size").and_then(|v| v.as_u64()).ok_or(Error::Format)?,
            body: one("body").as_ref().and_then(h32).ok_or(Error::Format)?,
            extent,
            of,
            attrs,
            keeper: one("keeper").and_then(|v| v.as_u64()).unwrap_or(KEEPER_PERSON),
        };
        Ok((card, one("handle").as_ref().and_then(h32)))
    }

    pub fn attr(&self, name: &str) -> Option<&VsfType> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// A document of sections all named `name` (a page's cards, the root's pages, a find's answer).
fn document(now: i64, name: &str, sections: Vec<Vec<u8>>, head: Option<(&str, Vec<u8>)>) -> Result<Vec<u8>> {
    let mut b = vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only();
    if let Some((n, s)) = head {
        b = b.add_unboxed(n, s);
    }
    for s in sections {
        b = b.add_unboxed(name, s);
    }
    b.build().map_err(fmt)
}

/// The sections of a verified document, by name, in order: each section's bytes.
pub fn sections<'a>(doc: &'a [u8], name: &str) -> Result<Vec<&'a [u8]>> {
    let (header, _) = vsf::verification::read_verified(doc, None).map_err(fmt)?;
    let mut out = Vec::new();
    for f in header.fields.iter().filter(|f| f.name == name) {
        // WHY: the offsets are the document's own; a section past its end is a bad document, not a panic.
        let end = f.offset_bytes.checked_add(f.size_bytes).ok_or(Error::Format)?;
        out.push(doc.get(f.offset_bytes..end).ok_or(Error::Format)?);
    }
    Ok(out)
}

/// A page as the root knows it: its span, and the kinds and makers it holds (sorted, for a binary search).
#[derive(Clone, Debug, PartialEq)]
struct PageRef {
    start: i64,
    span: u8,
    kinds: Vec<[u8; 32]>,
    makers: Vec<[u8; 32]>,
}

impl PageRef {
    fn end(&self) -> i64 {
        self.start + (1i64 << self.span)
    }
    fn holds(&self, made: i64) -> bool {
        made >= self.start && made < self.end()
    }
    fn name(&self) -> Vec<u8> {
        page_name(self.start, self.span)
    }
}

fn page_name(start: i64, span: u8) -> Vec<u8> {
    let mut n = alloc::vec![2u8, PAGE_TAG];
    n.extend_from_slice(&start.to_le_bytes());
    n.push(span);
    n
}

fn body_name(id: &[u8; 32]) -> Vec<u8> {
    let mut n = alloc::vec![2u8, BODY_TAG];
    n.extend_from_slice(id);
    n
}

fn page_schema() -> SectionSchema {
    SectionSchema::new("page").field("start", TypeConstraint::AnyEagleTime).field("span", TypeConstraint::AnyUnsigned).field("kind", TypeConstraint::Idea).field("maker", TypeConstraint::Blake3Rolling)
}

fn pending_schema() -> SectionSchema {
    SectionSchema::new("pending").field("id", TypeConstraint::Blake3Provenance)
}

/// The open clips' list: a field `clip` an item, its id (`hp`) and when it was made (`e`), told apart by their types.
fn clips_schema() -> SectionSchema {
    SectionSchema::new("clips").field("clip", TypeConstraint::Any)
}

fn insert_sorted(v: &mut Vec<[u8; 32]>, x: [u8; 32]) -> bool {
    match v.binary_search(&x) {
        Ok(_) => false,
        Err(i) => {
            v.insert(i, x);
            true
        }
    }
}

/// A page's cards, in the order they were made.
struct Page {
    start: i64,
    span: u8,
    cards: Vec<Card>,
}

/// Who calls, as the kernel stamped it (STORE.md part 1): the kist (none: the person, from the kernel's own channel or the find page), the kinds its `items` right lets it make, and the kinds it was let open (asked once, ever).
#[derive(Clone, Debug, Default)]
pub struct Caller {
    pub maker: Option<Maker>,
    pub makes: Vec<[u8; 32]>,
    pub opens: Vec<[u8; 32]>,
    pub device: Option<[u8; 32]>,
    /// Any kind, made or opened (a development spawn's `--items`, the bench's): never a kist's.
    pub any: bool,
}

impl Caller {
    pub fn person() -> Caller {
        Caller::default()
    }
    /// Who the caller is to the hold: its maker's tag, zeros for the person. Handles and makings are the tag's own.
    pub fn tag(&self) -> [u8; 32] {
        self.maker.as_ref().map(|m| m.tag()).unwrap_or([0; 32])
    }
    fn may_make(&self, kind: &[u8; 32]) -> bool {
        self.maker.is_none() || self.any || self.makes.contains(kind)
    }
    /// It may open (and change, and remove) what it made itself and the kinds it was let open; the person all.
    fn may_open(&self, c: &Card) -> bool {
        match &self.maker {
            None => true,
            Some(_) if self.any => true,
            Some(m) => (c.kist.developer == m.developer && c.kist.option == m.option) || self.opens.contains(&c.kind),
        }
    }

    /// The caller from the kernel's stamp (process.rs `items_stamp`): [flags u8: bit 0 a kist, bit 1 any kind, bit 2 the person][developer 32][option 32][name length u8][name][makes u16 LE][kinds][opens u16 LE][kinds]; and the bytes after it, the call. None when it does not read as one.
    pub fn from_stamp(s: &[u8]) -> Option<(Caller, &[u8])> {
        let flags = *s.first()?;
        let developer: [u8; 32] = s.get(1..33)?.try_into().ok()?;
        let option: [u8; 32] = s.get(33..65)?.try_into().ok()?;
        let nl = *s.get(65)? as usize;
        let name = core::str::from_utf8(s.get(66..66 + nl)?).ok()?;
        let mut at = 66 + nl;
        let mut lists = [Vec::new(), Vec::new()];
        for l in lists.iter_mut() {
            let n = u16::from_le_bytes(s.get(at..at + 2)?.try_into().ok()?) as usize;
            at += 2;
            for k in 0..n {
                l.push(s.get(at + 32 * k..at + 32 * (k + 1))?.try_into().ok()?);
            }
            at += 32 * n;
        }
        let [makes, opens] = lists;
        let caller = if flags & 4 != 0 { Caller::person() } else { Caller { maker: Some(Maker { developer, option, name: String::from(name) }), makes, opens, device: None, any: flags & 2 != 0 } };
        Some((caller, &s[at..]))
    }

    /// The stamp as the kernel writes it (process.rs `items_stamp`, `items_person_stamp`), for a host's test of the whole call path; `kist` sets bit 0. `from_stamp` reads it back.
    pub fn stamp(&self, kist: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let Some(m) = &self.maker else {
            out.push(1 << 2);
            out.extend_from_slice(&[0; 69]);
            return out;
        };
        out.push(kist as u8 | (self.any as u8) << 1);
        out.extend_from_slice(&m.developer);
        out.extend_from_slice(&m.option);
        out.push(m.name.len() as u8);
        out.extend_from_slice(m.name.as_bytes());
        for list in [&self.makes, &self.opens] {
            out.extend_from_slice(&(list.len() as u16).to_le_bytes());
            for k in list {
                out.extend_from_slice(k);
            }
        }
        out
    }
}

/// An attribute condition of a find (`Find::wheres`): equal, at least, at most, or (text) holds the words.
pub const IS_EQUAL: u64 = 0;
pub const IS_AT_LEAST: u64 = 1;
pub const IS_AT_MOST: u64 = 2;
pub const IS_WORDS: u64 = 3;
/// A find's order: newest first, oldest first, by title, largest first.
pub const ORDER_NEWEST: u64 = 0;
pub const ORDER_OLDEST: u64 = 1;
pub const ORDER_TITLE: u64 = 2;
pub const ORDER_LARGEST: u64 = 3;

/// A find (STORE.md "The find document"): every condition optional, all of them together.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Find {
    /// Any of these kinds (none: any the caller may open).
    pub kinds: Vec<[u8; 32]>,
    /// Made by this maker (`maker_tag`).
    pub maker: Option<[u8; 32]>,
    /// Made in [from, to).
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// Changed at or after.
    pub changed: Option<i64>,
    /// In the title or an `x` attribute, without regard to ASCII case.
    pub words: Option<String>,
    /// Related to this item (a handle the caller holds).
    pub of: Option<[u8; 32]>,
    pub wheres: Vec<(String, u64, VsfType)>,
    pub order: u64,
    pub skip: u64,
    pub count: u64,
}

/// A find's answer: how many matched, the index's change count, and a page of them with the caller's handles.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub total: u64,
    pub generation: u64,
    pub items: Vec<([u8; 32], Card)>,
}

/// A body being kept: a new item's, or one kept again in place.
pub struct Making {
    card: Card,
    writer: Option<Writer>,
    hasher: blake3::Hasher,
    len: u64,
    again: bool,
}

impl Making {
    pub fn id(&self) -> &[u8; 32] {
        &self.card.id
    }
    pub fn card(&self) -> &Card {
        &self.card
    }
}

/// What a read gives: bytes of a stored body (how many, and the body's size), or the extent a single-copy body lies in, for the caller to read its blocks.
#[derive(Debug, PartialEq)]
pub enum Read {
    Bytes(usize, u64),
    Extent(Extent, u64),
}

/// A handle given to a caller: the token, for whom, and which item (with when it was made: the page it is on).
#[derive(Clone)]
struct Handle {
    token: [u8; 32],
    caller: [u8; 32],
    id: [u8; 32],
    made: i64,
}

/// The hold of one person's items domain, as the vault process serves it: the root and the pages read so far, and the handles given out, all dropped when the person locks (`forget`).
pub struct Items {
    secret: [u8; 32],
    root: Option<Vec<PageRef>>,
    pages: Vec<Page>,
    handles: Vec<Handle>,
    generation: u64,
    /// The open clips' list (`CLIPS`), read at its first use.
    clips: Option<Vec<([u8; 32], i64)>>,
}

impl Items {
    /// `secret` keys the handles: drawn fresh when the vault process starts, so a handle means nothing to another process and nothing after it.
    pub fn new(secret: [u8; 32]) -> Items {
        Items { secret, root: None, pages: Vec::new(), handles: Vec::new(), generation: 0, clips: None }
    }

    /// Drop every card held in memory and every handle (the person locked: what is theirs goes, as the vault process drops its plaintext).
    pub fn forget(&mut self) {
        self.root = None;
        self.pages.clear();
        self.handles.clear();
        self.clips = None;
    }

    /// The root's pages: (start, span) each, oldest first (a bench's view of the index).
    pub fn pages<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain) -> Result<Vec<(i64, u8)>> {
        self.load_root(store, d)?;
        Ok(self.root.as_ref().ok_or(Error::Format)?.iter().map(|p| (p.start, p.span)).collect())
    }

    /// How many changes this index has taken since the vault process started (a find's `generation`, for a watch).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn handle_for(&mut self, caller: &Caller, id: &[u8; 32], made: i64) -> [u8; 32] {
        let tag = caller.tag();
        let mut h = blake3::Hasher::new_keyed(&self.secret);
        h.update(&tag);
        h.update(id);
        let token = *h.finalize().as_bytes();
        if let Err(i) = self.handles.binary_search_by(|x| x.token.cmp(&token)) {
            self.handles.insert(i, Handle { token, caller: tag, id: *id, made });
        }
        token
    }

    /// The item a caller's handle names: only a handle given to that caller.
    fn resolve(&self, caller: &Caller, token: &[u8; 32]) -> Result<([u8; 32], i64)> {
        match self.handles.binary_search_by(|x| x.token.cmp(token)) {
            Ok(i) if self.handles[i].caller == caller.tag() => Ok((self.handles[i].id, self.handles[i].made)),
            _ => Err(Error::Refused),
        }
    }

    fn drop_handles(&mut self, id: &[u8; 32]) {
        self.handles.retain(|h| h.id != *id);
    }

    fn load_root<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain) -> Result<()> {
        if self.root.is_some() {
            return Ok(());
        }
        let mut pages = Vec::new();
        if let Some(doc) = store.get(d, ROOT)? {
            for s in sections(&doc, "page")? {
                let p = SectionBuilder::parse(page_schema(), s).map_err(fmt)?;
                let one = |n: &str| p.get_fields(n).first().and_then(|f| f.values.first()).cloned();
                let all = |n: &str| p.get_fields(n).iter().flat_map(|f| f.values.iter().filter_map(h32)).collect::<Vec<_>>();
                let mut kinds = all("kind");
                let mut makers = all("maker");
                kinds.sort_unstable();
                makers.sort_unstable();
                pages.push(PageRef {
                    start: one("start").as_ref().and_then(eagle_of).ok_or(Error::Format)?,
                    span: one("span").and_then(|v| v.as_u64()).ok_or(Error::Format)? as u8,
                    kinds,
                    makers,
                });
            }
        }
        pages.sort_by_key(|p| p.start);
        self.root = Some(pages);
        Ok(())
    }

    fn save_root<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, now: i64) -> Result<()> {
        let root = self.root.as_ref().ok_or(Error::Format)?;
        let mut secs = Vec::with_capacity(root.len());
        for p in root {
            let mut b = page_schema().build().set("start", e6(p.start)).map_err(fmt)?.set("span", VsfType::u(p.span as usize, false)).map_err(fmt)?;
            if !p.kinds.is_empty() {
                b = b.set_multi("kind", p.kinds.iter().map(|k| VsfType::hi(k.to_vec())).collect()).map_err(fmt)?;
            }
            if !p.makers.is_empty() {
                b = b.set_multi("maker", p.makers.iter().map(|k| VsfType::hb(k.to_vec())).collect()).map_err(fmt)?;
            }
            secs.push(b.encode().map_err(fmt)?);
        }
        let head = SectionSchema::new("root").field("pages", TypeConstraint::AnyUnsigned).build().set("pages", VsfType::u(root.len(), false)).map_err(fmt)?.encode().map_err(fmt)?;
        let doc = document(now, "page", secs, Some(("root", head)))?;
        store.put_unlisted(d, ROOT, &doc)?;
        Ok(())
    }

    /// The page at `i` of the root, read if it is not held.
    fn page<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, i: usize) -> Result<usize> {
        let r = self.root.as_ref().ok_or(Error::Format)?[i].clone();
        if let Some(k) = self.pages.iter().position(|p| p.start == r.start && p.span == r.span) {
            return Ok(k);
        }
        let mut cards = Vec::new();
        if let Some(doc) = store.get(d, &r.name())? {
            for s in sections(&doc, "card")? {
                cards.push(Card::parse(s)?.0);
            }
        }
        self.pages.push(Page { start: r.start, span: r.span, cards });
        Ok(self.pages.len() - 1)
    }

    /// The root's page that holds `made`, made at the widest span when none does.
    fn page_for<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, made: i64) -> Result<(usize, usize, bool)> {
        self.load_root(store, d)?;
        let root = self.root.as_mut().ok_or(Error::Format)?;
        let (ri, new) = match root.iter().position(|p| p.holds(made)) {
            Some(i) => (i, false),
            None => {
                let start = (made >> TOP_SPAN) << TOP_SPAN;
                let at = root.partition_point(|p| p.start < start);
                root.insert(at, PageRef { start, span: TOP_SPAN, kinds: Vec::new(), makers: Vec::new() });
                (at, true)
            }
        };
        let pi = self.page(store, d, ri)?;
        Ok((ri, pi, new))
    }

    /// Write page `pi` (root entry `ri`), split in halves while it is past one piece and wider than MIN_SPAN; the root rewritten when it changed.
    fn save_page<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, ri: usize, pi: usize, mut root_changed: bool, now: i64) -> Result<()> {
        let mut work = alloc::vec![(ri, pi)];
        while let Some((ri, pi)) = work.pop() {
            let (start, span) = (self.pages[pi].start, self.pages[pi].span);
            let name = page_name(start, span);
            if self.pages[pi].cards.is_empty() {
                store.delete_unlisted(d, &name)?;
                self.pages.swap_remove(pi);
                self.root.as_mut().ok_or(Error::Format)?.remove(ri);
                root_changed = true;
                continue;
            }
            let secs: Vec<Vec<u8>> = self.pages[pi].cards.iter().map(|c| c.section(None)).collect::<Result<_>>()?;
            let doc = document(now, "card", secs, None)?;
            if doc.len() > ferros_store::PIECE && span > MIN_SPAN {
                // Split: the two halves (2^(span-1) each) take the cards made in them; the old page goes.
                let half = span - 1;
                let mid = start + (1i64 << half);
                let cards = core::mem::take(&mut self.pages[pi].cards);
                let (lo, hi): (Vec<Card>, Vec<Card>) = cards.into_iter().partition(|c| c.made < mid);
                store.delete_unlisted(d, &name)?;
                self.pages.swap_remove(pi);
                let root = self.root.as_mut().ok_or(Error::Format)?;
                root.remove(ri);
                for (s, cards) in [(start, lo), (mid, hi)] {
                    let mut r = PageRef { start: s, span: half, kinds: Vec::new(), makers: Vec::new() };
                    for c in &cards {
                        insert_sorted(&mut r.kinds, c.kind);
                        insert_sorted(&mut r.makers, c.kist.tag());
                    }
                    let at = root.partition_point(|p| p.start < s);
                    root.insert(at, r);
                    self.pages.push(Page { start: s, span: half, cards });
                }
                root_changed = true;
                // Both halves written in turn (each may split again); their root places found afresh.
                for s in [start, mid] {
                    let root = self.root.as_ref().ok_or(Error::Format)?;
                    let ri = root.iter().position(|p| p.start == s && p.span == half).ok_or(Error::Format)?;
                    let pi = self.pages.iter().position(|p| p.start == s && p.span == half).ok_or(Error::Format)?;
                    work.push((ri, pi));
                }
                continue;
            }
            store.put_unlisted(d, &name, &doc)?;
        }
        if root_changed {
            self.save_root(store, d, now)?;
        }
        self.generation += 1;
        Ok(())
    }

    /// The items domain's pending list: ids whose body is being written, so a body whose card never landed is found again (`sweep`).
    fn pending_edit<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, id: &[u8; 32], add: bool, now: i64) -> Result<()> {
        let mut ids = self.pending(store, d)?;
        if add {
            if ids.contains(id) {
                return Ok(());
            }
            ids.push(*id);
        } else {
            let n = ids.len();
            ids.retain(|x| x != id);
            if ids.len() == n {
                return Ok(());
            }
        }
        if ids.is_empty() {
            store.delete_unlisted(d, PENDING)?;
            return Ok(());
        }
        let b = pending_schema().build().set_multi("id", ids.iter().map(|x| VsfType::hp(x.to_vec())).collect()).map_err(fmt)?;
        let doc = document(now, "pending", alloc::vec![b.encode().map_err(fmt)?], None)?;
        store.put_unlisted(d, PENDING, &doc)?;
        Ok(())
    }

    fn pending<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain) -> Result<Vec<[u8; 32]>> {
        let Some(doc) = store.get(d, PENDING)? else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for s in sections(&doc, "pending")? {
            let p = SectionBuilder::parse(pending_schema(), s).map_err(fmt)?;
            out.extend(p.get_fields("id").iter().flat_map(|f| f.values.iter().filter_map(h32)));
        }
        Ok(out)
    }

    /// Every item of the domain gone (release: the person's items leave the device with their domains, AUTH.md): each page's stored bodies, the pages, the root and the hold's lists. The hold's values are off ferros_store's list, so `drop_domain` alone would leave them; the vault calls this first. Single-copy bodies' extents are the vault's to drop (by their owner tag). How many items went.
    pub fn drop_all<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain) -> Result<u64> {
        self.forget();
        self.load_root(store, d)?;
        let root = self.root.take().ok_or(Error::Format)?;
        let mut n = 0;
        for r in &root {
            if let Some(doc) = store.get(d, &r.name())? {
                for s in sections(&doc, "card")? {
                    let c = Card::parse(s)?.0;
                    if c.extent.is_none() {
                        store.delete_unlisted(d, &body_name(&c.id))?;
                    }
                    n += 1;
                }
            }
            store.delete_unlisted(d, &r.name())?;
        }
        // A body whose card never landed is on the pending list: it goes too.
        for id in self.pending(store, d)? {
            store.delete_unlisted(d, &body_name(&id))?;
        }
        for name in [ROOT, PENDING, CLIPS] {
            store.delete_unlisted(d, name)?;
        }
        self.forget();
        Ok(n)
    }

    /// The single-copy bodies not yet sealed, each item's id and when it was made: what the vault looks for in its extents' table once a clip's writer is gone (payloads/vault: a clip closed by its recoverer, its card then corrected by `amend_extent`).
    pub fn open_clips<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain) -> Result<Vec<([u8; 32], i64)>> {
        if let Some(c) = &self.clips {
            return Ok(c.clone());
        }
        let mut out = Vec::new();
        if let Some(doc) = store.get(d, CLIPS)? {
            for s in sections(&doc, "clips")? {
                let p = SectionBuilder::parse(clips_schema(), s).map_err(fmt)?;
                for f in p.get_fields("clip") {
                    let id = f.values.iter().find_map(|v| if let VsfType::hp(_) = v { h32(v) } else { None });
                    let made = f.values.iter().find_map(eagle_of);
                    if let (Some(id), Some(made)) = (id, made) {
                        out.push((id, made));
                    }
                }
            }
        }
        self.clips = Some(out.clone());
        Ok(out)
    }

    /// An item's id put on or taken off the open clips' list, the list written when it changed.
    fn clips_edit<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, id: &[u8; 32], made: i64, add: bool, now: i64) -> Result<()> {
        let mut list = self.open_clips(store, d)?;
        let had = list.iter().any(|(x, _)| x == id);
        if add == had {
            return Ok(());
        }
        if add {
            list.push((*id, made));
        } else {
            list.retain(|(x, _)| x != id);
        }
        if list.is_empty() {
            store.delete_unlisted(d, CLIPS)?;
        } else {
            let mut b = clips_schema().build();
            for (x, m) in &list {
                b = b.append_multi("clip", alloc::vec![VsfType::hp(x.to_vec()), e6(*m)]).map_err(fmt)?;
            }
            let doc = document(now, "clips", alloc::vec![b.encode().map_err(fmt)?], None)?;
            store.put_unlisted(d, CLIPS, &doc)?;
        }
        self.clips = Some(list);
        Ok(())
    }

    /// Bodies whose card never landed (a kill between the body's commit and its page's) removed; returns how many. Run at the domain's first request after the vault process starts.
    pub fn sweep<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, now: i64) -> Result<u64> {
        let ids = self.pending(store, d)?;
        let mut n = 0;
        for id in &ids {
            if self.locate(store, d, id, None)?.is_none() && store.delete_unlisted(d, &body_name(id))? {
                n += 1;
            }
        }
        if !ids.is_empty() {
            store.delete_unlisted(d, PENDING)?;
            let _ = now;
        }
        Ok(n)
    }

    /// Where an item's card is: (root entry, held page, card index); `made` narrows the pages to read.
    fn locate<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, id: &[u8; 32], made: Option<i64>) -> Result<Option<(usize, usize, usize)>> {
        self.load_root(store, d)?;
        let n = self.root.as_ref().ok_or(Error::Format)?.len();
        for ri in 0..n {
            let r = &self.root.as_ref().ok_or(Error::Format)?[ri];
            if made.is_some_and(|m| !r.holds(m)) {
                continue;
            }
            let pi = self.page(store, d, ri)?;
            if let Some(ci) = self.pages[pi].cards.iter().position(|c| c.id == *id) {
                return Ok(Some((ri, pi, ci)));
            }
        }
        Ok(None)
    }

    /// A new item: its card drafted from the proposal and the caller's stamp; its body follows (`write`, then `finish`), or it is a single-copy extent (`make_extent`). `random` is 32 bytes drawn for it (its id is their hash with what is first true of it, so the same bytes kept twice are two items and nobody can confirm a guess).
    pub fn make(&mut self, caller: &Caller, p: Proposed, now: i64, random: [u8; 32]) -> Result<Making> {
        if !caller.may_make(&p.kind) {
            return Err(Error::Refused);
        }
        let kist = caller.maker.clone().unwrap_or_default();
        let mut h = blake3::Hasher::new_derive_key("ferros items 2026-11-19 id");
        h.update(&random);
        h.update(&p.kind);
        h.update(&kist.tag());
        h.update(&now.to_le_bytes());
        let id = *h.finalize().as_bytes();
        let card = Card { id, kind: p.kind, kist, device: caller.device, person: None, made: now, changed: now, title: p.title, size: 0, body: [0; 32], extent: None, of: p.of, attrs: p.attrs, keeper: p.keeper };
        if card.section(None)?.len() > CARD_MAX {
            return Err(Error::TooLarge);
        }
        Ok(Making { card, writer: None, hasher: blake3::Hasher::new(), len: 0, again: false })
    }

    /// Keep an item again in place (no versions, STORE.md §7.5): a new body under the same id, its card changed at `finish`.
    pub fn keep_again<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, handle: &[u8; 32], now: i64) -> Result<Making> {
        let (id, made) = self.resolve(caller, handle)?;
        let (_, pi, ci) = self.locate(store, d, &id, Some(made))?.ok_or(Error::NotFound)?;
        let mut card = self.pages[pi].cards[ci].clone();
        if !caller.may_open(&card) {
            return Err(Error::Refused);
        }
        card.changed = now;
        Ok(Making { card, writer: None, hasher: blake3::Hasher::new(), len: 0, again: true })
    }

    /// The next bytes of the body being kept.
    pub fn write<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, m: &mut Making, bytes: &[u8], now: i64) -> Result<()> {
        if m.card.extent.is_some() {
            return Err(Error::Extent);
        }
        if m.writer.is_none() {
            // On the pending list before any of its pieces reach the engine.
            self.pending_edit(store, d, &m.card.id, true, now)?;
            m.writer = Some(store.begin_unlisted(d, &body_name(&m.card.id)));
        }
        let w = m.writer.as_mut().ok_or(Error::Format)?;
        store.write(d, w, bytes)?;
        m.hasher.update(bytes);
        m.len += bytes.len() as u64;
        Ok(())
    }

    /// Commit the body (its value whole, atomically), then its card into its page: the item is kept. Returns the caller's handle and the card.
    pub fn finish<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, mut m: Making, now: i64) -> Result<([u8; 32], Card)> {
        if m.card.extent.is_some() {
            return Err(Error::Extent);
        }
        if m.writer.is_none() {
            // An empty body: a value of no bytes, so a read finds it.
            self.write(store, d, &mut m, &[], now)?;
        }
        let w = m.writer.take().ok_or(Error::Format)?;
        store.finish(d, w)?;
        m.card.size = m.len;
        m.card.body = *m.hasher.finalize().as_bytes();
        let id = m.card.id;
        let out = self.place(store, d, caller, m.card, m.again, now)?;
        self.pending_edit(store, d, &id, false, now)?;
        Ok(out)
    }

    /// A new item whose body is a single-copy extent the caller has written (a RAW clip): its length and BLAKE3 as the recorder sealed it. The vault reserves the extent (manifestus `Extents`, named by the item's id) before the recording and calls this once it is sealed; removing the item frees it (`remove` gives the card back with its extent).
    pub fn finish_extent<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, mut m: Making, extent: Extent, len: u64, hash: [u8; 32], now: i64) -> Result<([u8; 32], Card)> {
        if m.writer.is_some() {
            return Err(Error::Format);
        }
        let old = if m.again { self.locate(store, d, &m.card.id, Some(m.card.made))?.map(|(_, pi, ci)| self.pages[pi].cards[ci].clone()) } else { None };
        m.card.extent = Some(extent);
        m.card.size = len;
        m.card.body = hash;
        let again = m.again;
        let out = self.place(store, d, caller, m.card, again, now)?;
        // Kept again over a stored body: that body goes.
        if old.as_ref().is_some_and(|c| c.extent.is_none()) {
            store.delete_unlisted(d, &body_name(&out.1.id))?;
        }
        Ok(out)
    }

    /// A new item whose single-copy body is about to be recorded into `extent`: its card placed at once, with no bytes yet, so the item is found (and a clip whose writer dies is still the person's, its card corrected by `seal_extent` or `amend_extent` once the vault has closed it). Returns the caller's handle and the card.
    pub fn make_extent<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, p: Proposed, extent: Extent, now: i64, random: [u8; 32]) -> Result<([u8; 32], Card)> {
        let m = self.make(caller, p, now, random)?;
        self.begin_extent(store, d, caller, m, extent, now)
    }

    /// `make_extent` in two steps, for a vault that names the extent by the item's id before reserving it: `make` drafts the card (its id), the extent is reserved under that id, and this places the card with it and puts the item on the open clips' list.
    pub fn begin_extent<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, mut m: Making, extent: Extent, now: i64) -> Result<([u8; 32], Card)> {
        if m.again || m.writer.is_some() {
            return Err(Error::Format);
        }
        m.card.extent = Some(extent);
        let (id, made) = (m.card.id, m.card.made);
        // On the list before the card is placed: a kill between the two leaves an id with no card, which `amend_extent` finds absent and the list lets go.
        self.clips_edit(store, d, &id, made, true, now)?;
        self.place(store, d, caller, m.card, false, now)
    }

    /// The single-copy body sealed: its extent as it now stands (its tail given back), its length and BLAKE3; the card changed in place.
    pub fn seal_extent<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, handle: &[u8; 32], extent: Extent, len: u64, hash: [u8; 32], now: i64) -> Result<Card> {
        let (id, made) = self.resolve(caller, handle)?;
        let c = self.card(store, d, caller, handle)?;
        if c.extent.is_none() {
            return Err(Error::Extent);
        }
        self.amend_extent(store, d, &id, made, extent, len, hash, now)
    }

    /// The vault's own correction of a single-copy body's card (no caller: the vault process found its clip closed by its recoverer): its extent, length and BLAKE3.
    pub fn amend_extent<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, id: &[u8; 32], made: i64, extent: Extent, len: u64, hash: [u8; 32], now: i64) -> Result<Card> {
        let Some((ri, pi, ci)) = self.locate(store, d, id, Some(made))? else {
            // A clip whose card never landed: nothing to correct, and nothing left to look for.
            self.clips_edit(store, d, id, made, false, now)?;
            return Err(Error::NotFound);
        };
        let c = &mut self.pages[pi].cards[ci];
        c.extent = Some(extent);
        c.size = len;
        c.body = hash;
        c.changed = now;
        let out = c.clone();
        self.save_page(store, d, ri, pi, false, now)?;
        // Sealed: off the open clips' list once its card says so.
        self.clips_edit(store, d, id, made, false, now)?;
        Ok(out)
    }

    /// The vault's own removal of an item by its id (no caller: a single-copy body whose extent is gone from the table, so the card would name blocks nobody holds). The card comes back.
    pub fn remove_id<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, id: &[u8; 32], made: i64, now: i64) -> Result<Card> {
        let Some((ri, pi, ci)) = self.locate(store, d, id, Some(made))? else {
            self.clips_edit(store, d, id, made, false, now)?;
            return Err(Error::NotFound);
        };
        let card = self.pages[pi].cards.remove(ci);
        if card.extent.is_none() {
            store.delete_unlisted(d, &body_name(&card.id))?;
        }
        self.save_page(store, d, ri, pi, false, now)?;
        self.drop_handles(id);
        self.clips_edit(store, d, id, made, false, now)?;
        Ok(card)
    }

    /// The card into its page (replacing the item's own when kept again), the page and, when it changed, the root written.
    fn place<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, card: Card, again: bool, now: i64) -> Result<([u8; 32], Card)> {
        let (ri, pi, mut root_changed) = self.page_for(store, d, card.made)?;
        let cards = &mut self.pages[pi].cards;
        match cards.iter().position(|c| c.id == card.id) {
            Some(ci) if again => cards[ci] = card.clone(),
            Some(_) => return Err(Error::Format),
            None => {
                let at = cards.partition_point(|c| (c.made, c.id) < (card.made, card.id));
                cards.insert(at, card.clone());
            }
        }
        let r = &mut self.root.as_mut().ok_or(Error::Format)?[ri];
        root_changed |= insert_sorted(&mut r.kinds, card.kind);
        root_changed |= insert_sorted(&mut r.makers, card.kist.tag());
        self.save_page(store, d, ri, pi, root_changed, now)?;
        let handle = self.handle_for(caller, &card.id, card.made);
        Ok((handle, card))
    }

    /// An item's card, as the caller may see it.
    pub fn card<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, handle: &[u8; 32]) -> Result<Card> {
        let (id, made) = self.resolve(caller, handle)?;
        let (_, pi, ci) = self.locate(store, d, &id, Some(made))?.ok_or(Error::NotFound)?;
        let c = self.pages[pi].cards[ci].clone();
        if !caller.may_open(&c) {
            return Err(Error::Refused);
        }
        Ok(c)
    }

    /// Body bytes from `offset` (a transfer at a time; a long body is read as a stream of ranges, never assembled): a stored body's bytes, or the extent a single-copy body lies in.
    pub fn read<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, handle: &[u8; 32], offset: u64, out: &mut [u8]) -> Result<Read> {
        let c = self.card(store, d, caller, handle)?;
        if let Some(x) = c.extent {
            return Ok(Read::Extent(x, c.size));
        }
        if offset >= c.size {
            return Ok(Read::Bytes(0, c.size));
        }
        // WHY: the offset and length are the caller's; the read ends at the body's end.
        let n = out.len().min((c.size - offset) as usize);
        store.read(d, &body_name(&c.id), offset, &mut out[..n])?.ok_or(Error::NotFound)?;
        Ok(Read::Bytes(n, c.size))
    }

    /// Removed, gone at once (STORE.md §7.7): its body's value, its card from its page. The card comes back, so the vault frees a single-copy body's extent.
    pub fn remove<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, handle: &[u8; 32], now: i64) -> Result<Card> {
        let (id, made) = self.resolve(caller, handle)?;
        let (ri, pi, ci) = self.locate(store, d, &id, Some(made))?.ok_or(Error::NotFound)?;
        if !caller.may_open(&self.pages[pi].cards[ci]) {
            return Err(Error::Refused);
        }
        let card = self.pages[pi].cards.remove(ci);
        // The card leaves its page first: a kill after it leaves the body on no page and on no pending list, so the body goes before the page is written.
        if card.extent.is_none() {
            store.delete_unlisted(d, &body_name(&card.id))?;
        }
        // The root keeps the page's kinds and makers (a find may read a page with none left of one: never a wrong answer, only a page read), rewritten when the page goes or splits.
        self.save_page(store, d, ri, pi, false, now)?;
        self.drop_handles(&id);
        if card.extent.is_some() {
            self.clips_edit(store, d, &id, made, false, now)?;
        }
        Ok(card)
    }

    /// The items a find matches that the caller may open, in its order, a page of at most FIND_MAX, each with the caller's handle.
    pub fn find<A: BlockDev, B: BlockDev, N: Nonces>(&mut self, store: &mut Store<A, B, N>, d: &Domain, caller: &Caller, f: &Find) -> Result<Found> {
        if f.count as usize > FIND_MAX {
            return Err(Error::TooLarge);
        }
        self.load_root(store, d)?;
        let of = match &f.of {
            Some(h) => Some(self.resolve(caller, h)?.0),
            None => None,
        };
        let words = f.words.as_ref().map(|w| w.to_ascii_lowercase());
        let n = self.root.as_ref().ok_or(Error::Format)?.len();
        let mut hits: Vec<Card> = Vec::new();
        for ri in 0..n {
            let r = &self.root.as_ref().ok_or(Error::Format)?[ri];
            if f.from.is_some_and(|t| r.end() <= t) || f.to.is_some_and(|t| r.start >= t) {
                continue;
            }
            if !f.kinds.is_empty() && !f.kinds.iter().any(|k| r.kinds.binary_search(k).is_ok()) {
                continue;
            }
            if f.maker.as_ref().is_some_and(|m| r.makers.binary_search(m).is_err()) {
                continue;
            }
            let pi = self.page(store, d, ri)?;
            for c in &self.pages[pi].cards {
                if caller.may_open(c) && matches(c, f, of.as_ref(), words.as_deref()) {
                    hits.push(c.clone());
                }
            }
        }
        match f.order {
            ORDER_OLDEST => hits.sort_by(|a, b| (a.made, a.id).cmp(&(b.made, b.id))),
            ORDER_TITLE => hits.sort_by(|a, b| a.title.cmp(&b.title).then((b.made, b.id).cmp(&(a.made, a.id)))),
            ORDER_LARGEST => hits.sort_by(|a, b| b.size.cmp(&a.size).then((b.made, b.id).cmp(&(a.made, a.id)))),
            _ => hits.sort_by(|a, b| (b.made, b.id).cmp(&(a.made, a.id))),
        }
        let total = hits.len() as u64;
        let count = if f.count == 0 { FIND_MAX } else { f.count as usize };
        let mut items = Vec::new();
        for c in hits.into_iter().skip(f.skip as usize).take(count) {
            let h = self.handle_for(caller, &c.id, c.made);
            items.push((h, c));
        }
        Ok(Found { total, generation: self.generation, items })
    }
}

/// Whether a card meets a find's conditions past its pages' (kind, maker, time, changed, relation, words, attributes).
fn matches(c: &Card, f: &Find, of: Option<&[u8; 32]>, words: Option<&str>) -> bool {
    if !f.kinds.is_empty() && !f.kinds.contains(&c.kind) {
        return false;
    }
    if f.maker.is_some_and(|m| m != c.kist.tag()) {
        return false;
    }
    if f.from.is_some_and(|t| c.made < t) || f.to.is_some_and(|t| c.made >= t) || f.changed.is_some_and(|t| c.changed < t) {
        return false;
    }
    if of.is_some_and(|id| !c.of.iter().any(|(x, _)| x == id)) {
        return false;
    }
    if let Some(w) = words {
        let hit = |t: &str| t.to_ascii_lowercase().contains(w);
        if !c.title.as_deref().is_some_and(hit) && !c.attrs.iter().any(|(_, v)| matches!(v, VsfType::x(t) if hit(t))) {
            return false;
        }
    }
    f.wheres.iter().all(|(name, is, want)| match c.attr(name) {
        None => false,
        Some(have) => compare(have, *is, want),
    })
}

/// An attribute against a condition: equal by its flattened bytes (type and value alike); at least and at most by number (unsigned, signed, Eagle time); words in text.
fn compare(have: &VsfType, is: u64, want: &VsfType) -> bool {
    let num = |v: &VsfType| -> Option<i128> { v.as_u64().map(|x| x as i128).or_else(|| v.as_i64().map(|x| x as i128)).or_else(|| eagle_of(v).map(|x| x as i128)) };
    match is {
        IS_EQUAL => have.flatten() == want.flatten(),
        IS_AT_LEAST => matches!((num(have), num(want)), (Some(a), Some(b)) if a >= b),
        IS_AT_MOST => matches!((num(have), num(want)), (Some(a), Some(b)) if a <= b),
        IS_WORDS => match (have, want) {
            (VsfType::x(h), VsfType::x(w)) => h.to_ascii_lowercase().contains(&w.to_ascii_lowercase()),
            _ => false,
        },
        _ => false,
    }
}
