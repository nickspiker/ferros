//! What `diag log` answers with, and how the bridge shows it (LEDGER.md "Reading it").
//!
//! The answer is a run of complete VSF documents, back to back, each found by its header's length:
//! - every entry as the flash holds it (sealed, section `ledger`), followed by
//! - `ledger_open`: that entry opened, when this device holds its key now (its provenance hash, category, writer, words);
//! - `ledger_finding`: something the read found wrong, named by block and byte (a torn last write, a damaged entry, a break in the chain, a seal that does not open);
//! - `ledger_state`, first: the ring's head and its counters.
//!
//! The bridge's one-shot converter writes `ledger_legacy` documents, one per line of the 2026-10-20 text log, chained like entries; they are never sealed (the host has no ira) and never written to the device.
//!
//! `render` turns any such run into lines a person reads; the wall time comes from the caller (the bridge shows the host's local time), so this crate keeps no clock and no zone.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::{VsfBuilder, VsfType};

use crate::entry::{self, lean, uint, Error, Hash, Payload, Stored};
use crate::ring::{Finding, Why, MIRROR, PRIMARY};

pub const OPEN: &str = "ledger_open";
pub const FINDING: &str = "ledger_finding";
pub const STATE: &str = "ledger_state";
pub const LEGACY: &str = "ledger_legacy";

const ENTRY: &str = "entry";
const BLOCK: &str = "block";
const AT: &str = "at";
const KIND: &str = "kind";
const WHY: &str = "why";
const COPY: &str = "copy";
const SEQ: &str = "seq";
const NEXT: &str = "next";
const HALF: &str = "blocks";
const HELD: &str = "preboot_held";
const LOST: &str = "preboot_lost";
const DROPPED: &str = "personal_dropped";
const FAILS: &str = "write_fails";
const PERSON: &str = "person";
const SINCE: &str = "since_boot_ms";

fn open_schema() -> SectionSchema {
    SectionSchema::new(OPEN)
        .field(ENTRY, TypeConstraint::Blake3Provenance)
        .field(entry::CATEGORY, TypeConstraint::DictKey)
        .field(entry::WRITER, entry::text_constraint())
        .field(entry::WORDS, entry::text_constraint())
        .field(entry::REFUSED, TypeConstraint::AnyUnsigned)
}

fn finding_schema() -> SectionSchema {
    SectionSchema::new(FINDING)
        .field(BLOCK, TypeConstraint::AnyUnsigned)
        .field(AT, TypeConstraint::AnyUnsigned)
        .field(KIND, TypeConstraint::AnyUnsigned)
        .field(COPY, TypeConstraint::AnyUnsigned)
        .field(SEQ, TypeConstraint::AnyUnsigned)
        .field(WHY, entry::text_constraint())
}

fn state_schema() -> SectionSchema {
    SectionSchema::new(STATE)
        .field(entry::BOOT, TypeConstraint::AnyUnsigned)
        .field(NEXT, TypeConstraint::AnyUnsigned)
        .field(BLOCK, TypeConstraint::AnyUnsigned)
        .field(HALF, TypeConstraint::AnyUnsigned)
        .field(HELD, TypeConstraint::AnyUnsigned)
        .field(LOST, TypeConstraint::AnyUnsigned)
        .field(DROPPED, TypeConstraint::AnyUnsigned)
        .field(FAILS, TypeConstraint::AnyUnsigned)
        .field(PERSON, TypeConstraint::AnyUnsigned)
}

/// A line of the old text log, as the converter keeps it: its boot, its counter time (milliseconds since that boot's counter started: the only time the old log had, kept as the number it was, never as a time of day), its words, chained.
pub fn legacy_schema() -> SectionSchema {
    SectionSchema::new(LEGACY)
        .field(entry::SEQ, TypeConstraint::AnyUnsigned)
        .field(entry::PREV, TypeConstraint::Blake3Provenance)
        .field(entry::BOOT, TypeConstraint::AnyUnsigned)
        .field(SINCE, TypeConstraint::AnyUnsigned)
        .field(entry::WORDS, entry::text_constraint())
        .field(entry::REFUSED, TypeConstraint::AnyUnsigned)
}

fn doc(time: Option<i64>, name: &str, section: Result<Vec<u8>, vsf::schema::ValidationError>) -> Result<Vec<u8>, Error> {
    let section = section.map_err(|e| Error::Schema(e.to_string()))?;
    let mut b = VsfBuilder::new();
    if let Some(t) = time {
        b = b.creation_time_oscillations(t);
    }
    b.provenance_only().add_unboxed(name, section).build().map_err(Error::Build)
}

/// An entry opened: its hash names the entry it opens (the stored document just before it in the answer).
pub fn opened(s: &Stored, p: &Payload) -> Result<Vec<u8>, Error> {
    let category = vsf::types::Ascii::new(p.category.as_str()).map_err(|_| Error::Build("a category name that is not ASCII".into()))?;
    // What was read back is in the table already (or ASCII), so `lean` replaces nothing here; the entry's own count goes on.
    let mut b = open_schema()
        .build()
        .set(ENTRY, VsfType::hp(s.hash.to_vec()))
        .and_then(|b| b.set(entry::CATEGORY, VsfType::d(category)))
        .and_then(|b| b.set(entry::WRITER, lean(&p.writer).0))
        .and_then(|b| b.set(entry::WORDS, lean(&p.words).0));
    if p.refused > 0 {
        b = b.and_then(|b| b.set(entry::REFUSED, uint(p.refused)));
    }
    let section = b.and_then(|b| b.encode());
    doc(s.place.time, OPEN, section)
}

impl Why {
    /// The finding's kind as a number (the `kind` field): what it is, for a reader that sorts or counts them.
    pub fn kind(&self) -> u64 {
        match self {
            Why::Torn => 0,
            Why::Corrupt(_) => 1,
            Why::OneCopy { .. } => 2,
            Why::MirrorBehind => 3,
            Why::ChainBreak { .. } => 4,
            Why::SeqGap { .. } => 5,
            Why::Unreadable { .. } => 6,
            Why::Shut(_) => 7,
            Why::NotARing => 8,
        }
    }
}

fn copy_name(c: u8) -> &'static str {
    if c == PRIMARY {
        "the primary"
    } else if c == MIRROR {
        "the mirror"
    } else {
        "a copy"
    }
}

impl core::fmt::Display for Why {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Why::Torn => write!(f, "the last write was cut off: skipped, the next entry goes in its place"),
            Why::Corrupt(e) => write!(f, "damaged in both copies, {e}"),
            Why::OneCopy { bad, error } => write!(f, "{} copy {error}; read from the other", copy_name(*bad)),
            Why::MirrorBehind => write!(f, "the mirror holds older entries here (a write cut off before the mirror): the primary read"),
            Why::ChainBreak { seq } => write!(f, "entry A#{seq} does not follow the entry before it: an entry was changed or removed"),
            Why::SeqGap { expected, found } => write!(f, "entries A#{expected} to A#{} are missing", found - 1),
            Why::Unreadable { copy } => write!(f, "{} copy of this block could not be read", copy_name(*copy)),
            Why::Shut(e) => write!(f, "an entry this device should open does not open: {e}"),
            Why::NotARing => write!(f, "block 0 holds no entry in either copy: not a ring; the ring starts afresh here and writes over it"),
        }
    }
}

/// A finding, as a document. `seq` names the entry when there is one (a seal that does not open).
pub fn finding(f: &Finding, seq: Option<u64>, time: Option<i64>) -> Result<Vec<u8>, Error> {
    let mut b = finding_schema().build().set(BLOCK, uint(f.block as u64)).and_then(|b| b.set(AT, uint(f.at as u64))).and_then(|b| b.set(KIND, uint(f.why.kind())));
    if let Why::OneCopy { bad: c, .. } | Why::Unreadable { copy: c } = &f.why {
        b = b.and_then(|b| b.set(COPY, uint(*c as u64)));
    }
    if let Some(s) = seq {
        b = b.and_then(|b| b.set(SEQ, uint(s)));
    }
    let section = b.and_then(|b| b.set(WHY, lean(&f.why.to_string()).0)).and_then(|b| b.encode());
    doc(time, FINDING, section)
}

/// The ring's state, first in every answer.
pub struct State {
    pub boot: u64,
    pub next_seq: u64,
    pub block: u32,
    pub half: u32,
    pub preboot_held: u64,
    pub preboot_lost: u64,
    pub personal_dropped: u64,
    pub write_fails: u64,
    pub person: bool,
}

pub fn state(s: &State, time: Option<i64>) -> Result<Vec<u8>, Error> {
    let section = state_schema()
        .build()
        .set(entry::BOOT, uint(s.boot))
        .and_then(|b| b.set(NEXT, uint(s.next_seq)))
        .and_then(|b| b.set(BLOCK, uint(s.block as u64)))
        .and_then(|b| b.set(HALF, uint(s.half as u64)))
        .and_then(|b| b.set(HELD, uint(s.preboot_held)))
        .and_then(|b| b.set(LOST, uint(s.preboot_lost)))
        .and_then(|b| b.set(DROPPED, uint(s.personal_dropped)))
        .and_then(|b| b.set(FAILS, uint(s.write_fails)))
        .and_then(|b| b.set(PERSON, uint(s.person as u64)))
        .and_then(|b| b.encode());
    doc(time, STATE, section)
}

/// One line of the old log as a document, chained to the one before (`prev`; the genesis hash for the first). `time` is when it was converted.
pub fn legacy(seq: u64, prev: &Hash, boot: u64, since_boot_ms: u64, words: &str, time: Option<i64>) -> Result<Vec<u8>, Error> {
    let (words, refused) = lean(words);
    let mut b = legacy_schema()
        .build()
        .set(entry::SEQ, uint(seq))
        .and_then(|b| b.set(entry::PREV, VsfType::hp(prev.to_vec())))
        .and_then(|b| b.set(entry::BOOT, uint(boot)))
        .and_then(|b| b.set(SINCE, uint(since_boot_ms)))
        .and_then(|b| b.set(entry::WORDS, words));
    if refused > 0 {
        b = b.and_then(|b| b.set(entry::REFUSED, uint(refused)));
    }
    doc(time, LEGACY, b.and_then(|b| b.encode()))
}

/// The complete documents in a run of them, in order.
/// A run that ends in a partial document gives the bytes from it on as the error.
pub fn split(run: &[u8]) -> Result<Vec<&[u8]>, (usize, Error)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < run.len() {
        let len = entry::doc_len(&run[at..]).map_err(|e| (at, e))?;
        out.push(&run[at..at + len]);
        at += len;
    }
    Ok(out)
}

/// A verified document's one section: its name, its creation time, its bytes, its provenance hash.
fn section_of(d: &[u8]) -> Result<(String, Option<i64>, &[u8], Hash), Error> {
    let (h, _) = vsf::verification::read_verified(d, None).map_err(Error::Provenance)?;
    let f = h.fields.first().ok_or_else(|| Error::Schema("no section".into()))?;
    // WHY checked: the range is the document's own, from the host's input; a sum that wraps would pass the bound check.
    let end = f.offset_bytes.checked_add(f.size_bytes).filter(|&e| e <= d.len()).ok_or_else(|| Error::Schema("a section outside the document".into()))?;
    let time = match &h.creation_time {
        Some(VsfType::e(vsf::EtType::e6(t))) => Some(*t),
        _ => None,
    };
    let hash = match &h.provenance_hash {
        VsfType::hp(b) if b.len() == 32 => b.as_slice().try_into().unwrap(),
        _ => return Err(Error::Provenance("no 32-byte provenance hash".into())),
    };
    Ok((f.name.clone(), time, &d[f.offset_bytes..end], hash))
}

fn text(s: &SectionBuilder, name: &str) -> String {
    s.get_value::<String>(name).unwrap_or_default()
}

/// Words with the count of characters the text table did not hold, when there were any.
fn with_refused(words: String, refused: Option<u64>) -> String {
    match refused {
        Some(n) if n > 0 => format!("{words} (A#{n} characters outside the text table, shown as {})", entry::MARKER),
        _ => words,
    }
}

fn num(s: &SectionBuilder, name: &str) -> Option<u64> {
    s.get_value::<u64>(name).ok()
}

/// A run of documents as lines a person reads. `wall` shows an Eagle time (oscillations) as a wall time; entries made before the clock was read show `(no clock yet)`.
/// Every entry is shown: opened when its `ledger_open` follows it, else as sealed (its class named); every finding is shown where it was found.
pub fn render(run: &[u8], wall: &dyn Fn(i64) -> String) -> Vec<String> {
    let mut lines = Vec::new();
    let docs = match split(run) {
        Ok(d) => d,
        Err((at, e)) => {
            // Show what is whole, then where it stops.
            let whole = split(&run[..at]).unwrap_or_default();
            let mut l = render_docs(&whole, wall);
            l.push(format!("!! the answer stops being whole at byte A#{at}: {e}"));
            return l;
        }
    };
    lines.extend(render_docs(&docs, wall));
    lines
}

fn when(t: Option<i64>, wall: &dyn Fn(i64) -> String) -> String {
    match t {
        Some(t) => wall(t),
        None => "(no clock yet)".to_string(),
    }
}

fn entry_line(s: &Stored, p: Option<&(String, String, String)>, wall: &dyn Fn(i64) -> String) -> String {
    let head = format!("b{} {}", s.place.boot, when(s.place.time, wall));
    match p {
        Some((category, writer, words)) if category == crate::category::TEXT.name => format!("{head} {writer}: {words}"),
        Some((category, writer, words)) => format!("{head} {writer} [{category}]: {words}"),
        None => {
            let class = crate::category::Class::from_number(s.class).map(|c| c.name()).unwrap_or("unknown class");
            format!("{head} (a {class} entry, sealed: not opened on this answer)")
        }
    }
}

fn render_docs(docs: &[&[u8]], wall: &dyn Fn(i64) -> String) -> Vec<String> {
    let mut lines = Vec::new();
    let mut k = 0usize;
    while k < docs.len() {
        let d = docs[k];
        k += 1;
        let (name, time, body, _) = match section_of(d) {
            Ok(x) => x,
            Err(e) => {
                lines.push(format!("!! a document of A#{} bytes that does not verify: {e}", d.len()));
                continue;
            }
        };
        match name.as_str() {
            entry::SECTION => {
                let s = match entry::parse(d) {
                    Ok(s) => s,
                    Err(e) => {
                        lines.push(format!("!! an entry that does not read: {e}"));
                        continue;
                    }
                };
                // Its opening, if the next document is one and names it.
                let mut p = None;
                if let Some(next) = docs.get(k) {
                    if let Ok((n, _, b, _)) = section_of(next) {
                        if n == OPEN {
                            if let Ok(o) = SectionBuilder::parse(open_schema(), b) {
                                if o.get_value::<[u8; 32]>(ENTRY).ok() == Some(s.hash) {
                                    p = Some((text(&o, entry::CATEGORY), text(&o, entry::WRITER), with_refused(text(&o, entry::WORDS), num(&o, entry::REFUSED))));
                                    k += 1;
                                }
                            }
                        }
                    }
                }
                lines.push(entry_line(&s, p.as_ref(), wall));
            }
            OPEN => lines.push("!! an opening with no entry before it".to_string()),
            FINDING => match SectionBuilder::parse(finding_schema(), body) {
                Ok(f) => {
                    let seq = num(&f, SEQ).map(|s| format!(", entry A#{s}")).unwrap_or_default();
                    lines.push(format!("!! block A#{} byte A#{}{seq}: {}", num(&f, BLOCK).unwrap_or(0), num(&f, AT).unwrap_or(0), text(&f, WHY)));
                }
                Err(e) => lines.push(format!("!! a finding that does not read: {e}")),
            },
            STATE => match SectionBuilder::parse(state_schema(), body) {
                Ok(s) => {
                    let n = |f: &str| num(&s, f).unwrap_or(0);
                    lines.push(format!(
                        "ledger at {}: this is b{}, next entry A#{}, head block A#{} of A#{}; pre-boot buffer A#{} held, A#{} overwritten; personal entries dropped (nobody present) A#{}; writes failed A#{}; a person {}",
                        when(time, wall),
                        n(entry::BOOT),
                        n(NEXT),
                        n(BLOCK),
                        n(HALF),
                        n(HELD),
                        n(LOST),
                        n(DROPPED),
                        n(FAILS),
                        if n(PERSON) == 1 { "present" } else { "not present" }
                    ));
                }
                Err(e) => lines.push(format!("!! a state that does not read: {e}")),
            },
            LEGACY => match SectionBuilder::parse(legacy_schema(), body) {
                Ok(l) => lines.push(format!("b{} (old log, A#{} ms after the counter started) {}", num(&l, entry::BOOT).unwrap_or(0), num(&l, SINCE).unwrap_or(0), with_refused(text(&l, entry::WORDS), num(&l, entry::REFUSED)))),
                Err(e) => lines.push(format!("!! an old-log line that does not read: {e}")),
            },
            other => lines.push(format!("!! a VSF document of A#{} bytes with a section `{other}`: not the ledger's", d.len())),
        }
    }
    lines
}

/// Check a run of `ledger_legacy` documents: each verifies and names the one before.
/// Returns how many, or the first that does not.
pub fn check_legacy(run: &[u8]) -> Result<u64, String> {
    let docs = split(run).map_err(|(at, e)| format!("byte A#{at}: {e}"))?;
    let mut prev = entry::genesis();
    for (n, d) in docs.iter().enumerate() {
        let (name, _, body, hash) = section_of(d).map_err(|e| format!("line A#{n}: {e}"))?;
        if name != LEGACY {
            return Err(format!("line A#{n}: a `{name}` document"));
        }
        let l = SectionBuilder::parse(legacy_schema(), body).map_err(|e| format!("line A#{n}: {e}"))?;
        if l.get_value::<[u8; 32]>(entry::PREV).ok() != Some(prev) || num(&l, entry::SEQ) != Some(n as u64) {
            return Err(format!("line A#{n}: does not follow the line before"));
        }
        prev = hash;
    }
    Ok(docs.len() as u64)
}

/// A verified document's provenance hash (what a chained document after it names as `prev`).
pub fn hash_of(d: &[u8]) -> Result<Hash, Error> {
    section_of(d).map(|s| s.3)
}
