//! Ideas (docs/IDEAS.md; owner, 2026-11-10: "We need a generic idea bubble that we can expand and add languages as the translation/need fits. Like think 'Remove from fleet' is an idea.").
//!
//! - **An idea** is a meaning named by 32 bytes, VSF's `hi` ([`id`]): keyed BLAKE3 (context [`CONTEXT`]) over its first phrasing's locale (ISO 639-3, as ASCII `a`) and its scrubbed words as `x` ("pinned by their locale hashed with their first literation in whatever that locale is, VSF proper x style"). Same words in the same language are the same idea, whoever mints them.
//! - **A phrasing** ([`Phrasing`]) is the idea in one language, in one register; the first phrasing proves itself (it hashes to the id), every other one is trusted for its signer.
//! - **A link** ([`Link`]) joins two ids minted for one meaning (same as) or marks them close (related, for search only); neither id is rewritten.
//! - **A sheet** ([`sheet`]) is a complete VSF file of phrasing and link sections, signed by its author with every egg; a kist's capsule carries its own ideas the same way.
//! - **Which words a person reads** is [`show::show`]: their languages in order, then keys they trust in order, then the first phrasing marked as such; never blank.
//!
//! no_std + alloc: the scrub's NFC is unicode-normalization's, the `x` codec vsf's own (x-full, every script).
#![no_std]

extern crate alloc;

pub mod scrub;
pub mod sheet;
pub mod show;

pub use scrub::{is_shown, scrub, shown};
pub use sheet::{Link, LinkKind, Phrasing, Sheet};
pub use show::{show, Shown};
pub use vsf;

use vsf::types::Ascii;
use vsf::VsfType;

/// The BLAKE3 derive-key context of an idea's id: what keeps an idea's 32 bytes from ever equalling another hash over the same bytes.
pub const CONTEXT: &str = "ferros idea";

/// An idea's id: VSF's `hi`.
pub type Id = [u8; 32];

/// A phrasing's language: its ISO 639-3 code, three lowercase ASCII letters, as fgtw's `Lang::iso` gives ferros_lang's sixteen and vsf's `wl` keeps any language.
pub type Locale = [u8; 3];

/// Whether `l` is three lowercase ASCII letters.
pub fn is_locale(l: &Locale) -> bool {
    l.iter().all(|c| c.is_ascii_lowercase())
}

/// The id of the idea whose first phrasing is `text` in `locale`: BLAKE3 derive-key [`CONTEXT`] over the VSF `a` of the locale's three letters followed by the VSF `x` of `scrub(text)`.
/// The `x` bytes are vsf's codebook (pinned by its BLAKE3, 51b6e8eb…): ids are only stable while it never changes, as handles' are.
/// Errors: a locale that is not three lowercase letters, words that scrub to nothing.
pub fn id(locale: Locale, text: &str) -> Result<Id, &'static str> {
    id_of_scrubbed(locale, &scrub(text))
}

/// The id over words already scrubbed.
pub(crate) fn id_of_scrubbed(locale: Locale, scrubbed: &str) -> Result<Id, &'static str> {
    if !is_locale(&locale) {
        return Err("a locale is three lowercase ISO 639-3 letters");
    }
    if scrubbed.is_empty() {
        return Err("an idea needs words");
    }
    // WHY expect: three lowercase letters are ASCII, checked above.
    let l = Ascii::new(alloc::string::String::from(core::str::from_utf8(&locale).expect("ASCII letters are UTF-8"))).expect("ASCII letters are ASCII");
    let mut h = blake3::Hasher::new_derive_key(CONTEXT);
    h.update(&VsfType::a(l).flatten());
    h.update(&VsfType::x(alloc::string::String::from(scrubbed)).try_flatten().map_err(|_| "words the x codebook cannot hold")?);
    Ok(*h.finalize().as_bytes())
}

/// A phrasing's register beside its locale (owner, 2026-11-10: "A phrasing may later carry a register (terse/plain/verbose)"). On the wire an unsigned `register`, written only when not plain: 0 terse, 1 plain, 2 verbose.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Register {
    /// Short enough for a key or a pill.
    Terse,
    #[default]
    Plain,
    /// Says more than plain, for a person who wants it explained.
    Verbose,
}

impl Register {
    pub const fn code(self) -> u8 {
        match self {
            Register::Terse => 0,
            Register::Plain => 1,
            Register::Verbose => 2,
        }
    }

    /// The register a wire code names; None for a code this reader does not know.
    pub const fn from_code(c: u64) -> Option<Register> {
        match c {
            0 => Some(Register::Terse),
            1 => Some(Register::Plain),
            2 => Some(Register::Verbose),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_words_same_id() {
        assert_eq!(id(*b"eng", "Remove from fleet"), id(*b"eng", "Remove from fleet"));
        assert_ne!(id(*b"eng", "Remove from fleet").unwrap(), id(*b"spa", "Remove from fleet").unwrap());
    }

    #[test]
    fn scrubbed_variants_agree() {
        let base = id(*b"eng", "Remove from fleet").unwrap();
        for v in ["  Remove   from fleet ", "Remove\tfrom\u{A0}fleet", "Re\u{200B}move from fleet\u{FEFF}", "Remove\nfrom fleet", "\u{202E}Remove from fleet"] {
            assert_eq!(id(*b"eng", v).unwrap(), base, "{v:?}");
        }
        // NFC: a precomposed letter and the letter with its combining mark.
        assert_eq!(id(*b"fra", "retirer de la flotte, d\u{E9}j\u{E0}").unwrap(), id(*b"fra", "retirer de la flotte, de\u{301}ja\u{300}").unwrap());
        // Dashes.
        assert_eq!(id(*b"eng", "on - off").unwrap(), id(*b"eng", "on \u{2014} off").unwrap());
    }

    #[test]
    fn case_and_punctuation_make_another_idea() {
        let base = id(*b"eng", "Remove from fleet").unwrap();
        assert_ne!(id(*b"eng", "remove from fleet").unwrap(), base);
        assert_ne!(id(*b"eng", "Remove from fleet.").unwrap(), base);
    }

    #[test]
    fn every_script_hashes() {
        for (l, t) in [(*b"kor", "함대에서 제거"), (*b"zho", "从舰队中移除"), (*b"hin", "बेड़े से हटाएँ"), (*b"rus", "Удалить из флота")] {
            assert!(id(l, t).is_ok(), "{t}");
        }
    }

    #[test]
    fn nothing_and_bad_locales_refuse() {
        assert!(id(*b"eng", " \u{200B} ").is_err());
        assert!(id(*b"EN ", "x").is_err());
    }

    /// The id's exact bytes, pinned: a change here is a change to every idea ever minted (the scrub, the context, the `a` and `x` encodings, the codebook).
    #[test]
    fn the_id_is_pinned() {
        // The message: VSF `a` of "eng" (a, size marker 3, length 3, the letters), then VSF `x` of the words (x, size marker 3, 17 characters, the codebook's 12 bytes).
        let a = VsfType::a(Ascii::new("eng").unwrap()).flatten();
        let x = VsfType::x(alloc::string::String::from("Remove from fleet")).try_flatten().unwrap();
        assert_eq!(a, [b'a', b'3', 3, b'e', b'n', b'g']);
        assert_eq!(x, [b'x', b'3', 17, 3, 48, 246, 221, 98, 133, 214, 28, 161, 209, 154, 128]);
        let mut h = blake3::Hasher::new_derive_key("ferros idea");
        h.update(&a);
        h.update(&x);
        let i = id(*b"eng", "Remove from fleet").unwrap();
        assert_eq!(i, *h.finalize().as_bytes());
        // G#4a7321a0 4a0fd95d ee18a342 07710b4d 0a987548 ebdbacc8 cd592129 6f624af2
        let pinned = [74, 115, 33, 160, 74, 15, 217, 93, 238, 24, 163, 66, 7, 113, 11, 77, 10, 152, 117, 72, 235, 219, 172, 200, 205, 89, 33, 41, 111, 98, 74, 242];
        assert_eq!(i, pinned);
    }
}
