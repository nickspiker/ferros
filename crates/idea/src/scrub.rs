//! The scrub (docs/IDEAS.md "Scrubbing"; owner, 2026-11-10, "Yup"): NFC, whitespace runs collapsed to one space, trimmed, zero-width and control characters dropped, look-alike spaces and dashes unified; never case-folded, punctuation kept.
//!
//! Two forms of the same words:
//! - **Shown** ([`shown`]): what a phrasing record keeps and a person reads. The scrub, except that a run of whitespace holding a line break becomes one `\n` (ferros_lang's multi-line passages).
//! - **Scrubbed** ([`scrub`]): what the id hashes. The shown form with its line breaks as spaces.
//!
//! A phrasing's text must already be in its shown form ([`is_shown`]), so the words hashed and the words shown differ by line breaks alone: an invisible character can never make two phrasings that look alike hash apart, or one that hashes as the first phrasing look like something else.

use alloc::string::String;
use unicode_normalization::UnicodeNormalization;

/// What the scrub does with one character.
enum Class {
    /// Kept as it is.
    Keep(char),
    /// A space: runs of these collapse to one U+0020.
    Space,
    /// A line break: a run of whitespace holding one is one `\n` in the shown form.
    Line,
    /// Dropped: invisible, or a control.
    Drop,
    /// A look-alike dash: U+002D.
    Dash,
}

/// The scrub's class of `c`. The lists are the spec's (docs/IDEAS.md "Scrubbing"); changing one changes ids, so they are frozen once an idea is published.
fn class(c: char) -> Class {
    match c {
        // Line breaks: LF, VT, FF, CR, NEL, LINE SEPARATOR, PARAGRAPH SEPARATOR.
        '\n' | '\u{B}' | '\u{C}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => Class::Line,
        // Spaces: TAB, SPACE, NO-BREAK SPACE, OGHAM SPACE MARK, EN QUAD to HAIR SPACE, NARROW NO-BREAK SPACE, MEDIUM MATHEMATICAL SPACE, IDEOGRAPHIC SPACE.
        '\t' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => Class::Space,
        // Invisible: SOFT HYPHEN, COMBINING GRAPHEME JOINER, ARABIC LETTER MARK, MONGOLIAN VOWEL SEPARATOR, ZERO WIDTH SPACE, LEFT-TO-RIGHT and RIGHT-TO-LEFT MARK, the bidi embeddings and overrides, WORD JOINER and the invisible operators, the bidi isolates and the deprecated format characters, ZERO WIDTH NO-BREAK SPACE (BOM), the interlinear annotation marks.
        // Not ZERO WIDTH NON-JOINER (U+200C) or ZERO WIDTH JOINER (U+200D): they spell Persian and Hindi words and join emoji sequences (docs/IDEAS.md, open question 0).
        '\u{AD}' | '\u{34F}' | '\u{61C}' | '\u{180E}' | '\u{200B}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' => Class::Drop,
        // Dashes: HYPHEN, NON-BREAKING HYPHEN, FIGURE DASH, EN DASH, EM DASH, HORIZONTAL BAR, MINUS SIGN, TWO-EM and THREE-EM DASH, SMALL EM DASH, SMALL HYPHEN-MINUS, FULLWIDTH HYPHEN-MINUS.
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{2E3A}' | '\u{2E3B}' | '\u{FE58}' | '\u{FE63}' | '\u{FF0D}' => Class::Dash,
        // Every other control (C0, DEL, C1).
        c if c.is_control() => Class::Drop,
        c => Class::Keep(c),
    }
}

/// The shown form of `text`: dropped and unified as the scrub does, each whitespace run one space (or one `\n` if it held a line break), trimmed, then NFC.
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // The whitespace run pending before the next kept character: 0 none, 1 a space, 2 a line break.
    let mut gap = 0u8;
    for c in text.chars() {
        let keep = match class(c) {
            Class::Keep(k) => k,
            Class::Dash => '-',
            Class::Drop => continue,
            Class::Space => {
                if gap == 0 {
                    gap = 1;
                }
                continue;
            }
            Class::Line => {
                gap = 2;
                continue;
            }
        };
        // A run before the first kept character is leading whitespace: trimmed.
        if gap != 0 && !out.is_empty() {
            out.push(if gap == 2 { '\n' } else { ' ' });
        }
        gap = 0;
        out.push(keep);
    }
    // A run after the last kept character is never pushed: trailing whitespace trimmed.
    // NFC last: dropping a character between a letter and its combining mark leaves a pair NFC composes. No character NFC produces is one the classes above change (the two spaces with canonical decompositions, U+2000 and U+2001, are mapped before NFC ever sees them), so the result is its own shown form.
    out.nfc().collect()
}

/// The scrubbed form of `text`: its shown form with each line break a space. What the id hashes.
pub fn scrub(text: &str) -> String {
    shown(text).replace('\n', " ")
}

/// Whether `text` is already in its shown form: what a phrasing record must hold.
pub fn is_shown(text: &str) -> bool {
    shown(text) == text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_runs_collapse_and_trim() {
        assert_eq!(scrub("  remove \t from\u{A0}\u{3000} fleet  "), "remove from fleet");
    }

    #[test]
    fn invisibles_and_controls_drop() {
        assert_eq!(scrub("re\u{200B}move\u{FEFF} from \u{202E}fleet\u{7}"), "remove from fleet");
        // A dropped character between a letter and its mark: NFC still composes them.
        assert_eq!(scrub("e\u{200B}\u{301}"), "\u{E9}");
    }

    #[test]
    fn joiners_stay() {
        // Hindi's half form and an emoji sequence keep their joiners.
        assert_eq!(scrub("क्\u{200D}ष"), "क्\u{200D}ष");
        assert_eq!(scrub("\u{1F469}\u{200D}\u{1F4BB}"), "\u{1F469}\u{200D}\u{1F4BB}");
    }

    #[test]
    fn dashes_unify() {
        assert_eq!(scrub("on \u{2014} off \u{2013} 1\u{2212}2 \u{FF0D}"), "on - off - 1-2 -");
    }

    #[test]
    fn case_and_punctuation_stay() {
        assert_eq!(scrub("Remove, from: Fleet!"), "Remove, from: Fleet!");
    }

    #[test]
    fn line_breaks_show_and_scrub_as_spaces() {
        assert_eq!(shown("wipes from this device:  \r\n  the map"), "wipes from this device:\nthe map");
        assert_eq!(scrub("wipes from this device:  \r\n  the map"), "wipes from this device: the map");
        assert_eq!(shown("a\n\nb"), "a\nb");
    }

    #[test]
    fn the_shown_form_is_its_own() {
        for t in ["  a\u{200B} b\u{2014}c \n\n d\u{301}e ", "क्\u{200D}ष", "\u{2000}x\u{2001}", "", " \t "] {
            let s = shown(t);
            assert!(is_shown(&s), "{t:?} -> {s:?}");
            assert_eq!(scrub(&s), scrub(t));
        }
    }
}
