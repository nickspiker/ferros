//! The retype gate (docs/AUTH.md "Unlock at boot"; docs/PRESENCE.md): turning attest at boot on asks for the handle again, and the kernel compares what it gives with the seed it holds for the person present.
//!
//! The owner, 2026-11-17: "from a security perspective we can change the UI to require a handle to be typed in to enable attest at boot (since my toddler could do it without me)".
//! Shared by the kernel (it compares and answers, SYS_UNLOCK op 12) and the attest screen's keyboard (it asks and reads the answer), so the compare and the reply's fields are written once.
//!
//! The compare is over one-way tags, never the seeds: the kernel derives its side from the seed it holds sealed under the wairua, in registers (`keys::held_derive`, the same `derive_key` as `tag`), so the held seed is never opened into memory for it.
//! Equal tags mean equal seeds (BLAKE3 is collision resistant), and the compare runs in time that does not depend on where they differ (`crate::same`).
//!
//! The reply is a complete VSF document (docs/VSF-AUDIT.md "The reply form"): section `attest_at_boot`, a typed boolean `error`, a short `reason` in ASCII, and the typed booleans `present` (a person is present: the kernel held a seed to compare with), `matched` (the handle typed is the person present's) and `on` (attest at boot is on after the call).

use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// The context both sides derive their tag under.
pub const CONTEXT: &str = "ferros 2026-11-17 retype gate v0";

/// The tag of a seed: `derive_key(CONTEXT, seed)`, which the kernel makes in registers from the seed it holds.
pub fn tag(seed: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key(CONTEXT, seed)
}

/// Whether the seed typed now is the one `held_tag` was made from, in time that does not depend on where they differ.
pub fn matches(held_tag: &[u8; 32], typed_seed: &[u8; 32]) -> bool {
    let mut t = tag(typed_seed);
    let same = crate::same(held_tag, &t);
    for b in t.iter_mut() {
        // SAFETY: a valid, aligned byte of a local array.
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    same
}

/// The reply's section and fields.
pub const SECTION: &str = "attest_at_boot";
const ERROR: &str = "error";
const REASON: &str = "reason";
const PRESENT: &str = "present";
const MATCHED: &str = "matched";
const ON: &str = "on";

fn schema() -> SectionSchema {
    // The booleans are vsf `u0`; AnyUnsigned is the constraint that takes them, and the reader below takes nothing but `u0`.
    SectionSchema::new(SECTION)
        .field(ERROR, TypeConstraint::AnyUnsigned)
        .field(REASON, TypeConstraint::AsciiText)
        .field(PRESENT, TypeConstraint::AnyUnsigned)
        .field(MATCHED, TypeConstraint::AnyUnsigned)
        .field(ON, TypeConstraint::AnyUnsigned)
}

/// What the kernel answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    /// Nothing changed (a mismatch, nobody present, the setting not kept).
    pub error: bool,
    /// A person is present: the kernel held a seed to compare with.
    pub present: bool,
    /// The handle typed is the person present's.
    pub matched: bool,
    /// Attest at boot is on after the call.
    pub on: bool,
}

/// The most a reply takes (the test `reply_round_trip` holds every reply the kernel makes to it): the buffer the keyboard reads it into.
pub const REPLY_MAX: usize = 1 << 8;

/// The reply as a VSF document, stamped `eagle` (Eagle time, oscillations). `reason` is ASCII, a few words for the console; the screen speaks from the typed fields.
pub fn reply(r: Reply, reason: &'static str, eagle: i64) -> Option<Vec<u8>> {
    let section = schema()
        .build()
        .set(ERROR, VsfType::u0(r.error))
        .and_then(|b| b.set(REASON, VsfType::a(vsf::Ascii::lit(reason))))
        .and_then(|b| b.set(PRESENT, VsfType::u0(r.present)))
        .and_then(|b| b.set(MATCHED, VsfType::u0(r.matched)))
        .and_then(|b| b.set(ON, VsfType::u0(r.on)))
        .and_then(|b| b.encode())
        .ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only().add_unboxed(SECTION, section).build().ok()
}

/// A reply read back, with its reason copied into `reason` (how many bytes of it fit). None for anything that is not this reply: its provenance does not verify, the section is missing, or a boolean is not a `u0`.
pub fn read(doc: &[u8], reason: &mut [u8]) -> Option<(Reply, usize)> {
    let section = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let first = |name: &str| section.get_fields(name).first().and_then(|f| f.values.first()).cloned();
    let flag = |name: &str| match first(name) {
        Some(VsfType::u0(b)) => Some(b),
        _ => None,
    };
    let r = Reply { error: flag(ERROR)?, present: flag(PRESENT)?, matched: flag(MATCHED)?, on: flag(ON)? };
    let n = match first(REASON) {
        Some(VsfType::a(s)) => {
            // WHY the min: the reason's length is the document's; only what the caller's buffer holds is copied.
            let n = s.as_bytes().len().min(reason.len());
            reason[..n].copy_from_slice(&s.as_bytes()[..n]);
            n
        }
        _ => 0,
    };
    Some((r, n))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// The kernel's side is `derive_key(CONTEXT, held seed ‖ nothing)` (keys::held_derive with an empty message); the blake3 crate's derive_key over the seed alone is the same bytes.
    #[test]
    fn tag_is_derive_key() {
        let seed = [7u8; 32];
        let mut h = blake3::Hasher::new_derive_key(CONTEXT);
        h.update(&seed);
        h.update(&[]);
        assert_eq!(tag(&seed), *h.finalize().as_bytes());
    }

    /// The same handle matches; another handle, and a seed one bit off anywhere, does not.
    #[test]
    fn compare() {
        let mine = crate::identity_seed("Correct Horse").unwrap();
        let held = tag(&mine);
        assert!(matches(&held, &crate::identity_seed("Correct Horse").unwrap()));
        assert!(!matches(&held, &crate::identity_seed("correct horse").unwrap()));
        assert!(!matches(&held, &crate::identity_seed("Correct Horse ").unwrap()));
        for byte in 0..32 {
            for bit in 0..8 {
                let mut s = mine;
                s[byte] ^= 1 << bit;
                assert!(!matches(&held, &s), "byte {byte} bit {bit}");
            }
        }
    }

    /// `same` looks at every byte: a difference at the first or the last is found alike.
    #[test]
    fn same_every_byte() {
        let a = [0x5Au8; 32];
        for at in 0..32 {
            let mut b = a;
            b[at] ^= 0x80;
            assert!(!crate::same(&a, &b));
        }
        assert!(crate::same(&a, &a));
    }

    /// Every reply the kernel makes reads back as it was made.
    #[test]
    fn reply_round_trip() {
        for (r, why) in [
            (Reply { error: false, present: true, matched: true, on: true }, "on"),
            (Reply { error: false, present: true, matched: true, on: true }, "on already"),
            (Reply { error: true, present: true, matched: false, on: false }, "they do not match"),
            (Reply { error: true, present: true, matched: false, on: true }, "they do not match"),
            (Reply { error: true, present: false, matched: false, on: false }, "no seed held: nobody present"),
            (Reply { error: true, present: true, matched: true, on: false }, "the setting was not kept"),
        ] {
            // The clock's whole range: the stamp's width is the document's longest field.
            let doc = reply(r, why, i64::MAX).unwrap();
            assert!(doc.len() <= REPLY_MAX, "{} bytes", doc.len());
            let doc = reply(r, why, 1 << 40).unwrap();
            let mut reason = [0u8; 64];
            let (back, n) = read(&doc, &mut reason).unwrap();
            assert_eq!(back, r);
            assert_eq!(&reason[..n], why.as_bytes());
        }
    }

    /// A document changed after it was made is refused, never read.
    #[test]
    fn reply_tampered() {
        let doc = reply(Reply { error: true, present: true, matched: false, on: false }, "they do not match", 1 << 40).unwrap();
        for at in 0..doc.len() {
            let mut d = doc.clone();
            d[at] ^= 1;
            let mut reason = [0u8; 64];
            assert!(read(&d, &mut reason).is_none(), "a changed byte {at} was read");
        }
    }
}
