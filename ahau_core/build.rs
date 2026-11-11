//! The attest screen's encoder table is frozen in src/frozen_codes.rs: ihi's identity seed hashes vsf's `x` encoding under ihi's own lane (vsf 0.6.42's codebook), and vsf main reissued its codebook on 2026-11-10, moving seven keys' codes (£ ¥ § ° • … €). Reading the sibling vsf's file would now name other identities for any handle holding those, so the table is committed source, generated once from the frozen file, and this script only checks it.
//! It refuses: an edit to the frozen entries (their pinned hash), a key whose character is not its own NFC form or is a combining mark (vsf normalizes before it encodes and this encoder does not, so the seed would name someone else), and a key with no frozen code. A key added to the keyboard needs its code from the frozen codebook added to src/frozen_codes.rs by hand, and the pin below updated with it.
use unicode_normalization::char::canonical_combining_class;
use unicode_normalization::UnicodeNormalization;

include!("src/frozen_codes.rs");

/// BLAKE3 of the frozen entries (each `    (` line of src/frozen_codes.rs with its newline), as generated from the frozen codebook (BLAKE3 523769a9…6dad0e).
const ENTRIES_BLAKE3: &str = "2dd7565c6fa18c32a5cf17441f9b1c8d1fbe4f1141446a5d7f8acb304173ff90";

fn main() {
    println!("cargo:rerun-if-changed=src/frozen_codes.rs");
    println!("cargo:rerun-if-changed=../honeycomb/src/lib.rs");
    let text = std::fs::read_to_string("src/frozen_codes.rs").expect("src/frozen_codes.rs");
    let entries: String = text.lines().filter(|l| l.starts_with("    (")).map(|l| format!("{l}\n")).collect();
    assert_eq!(blake3::hash(entries.as_bytes()).to_hex().as_str(), ENTRIES_BLAKE3, "src/frozen_codes.rs: the frozen codes changed; they must stay ihi's (vsf 0.6.42's codebook), or identities move");
    for c in honeycomb::typeable() {
        let s: String = [c].iter().collect();
        assert_eq!(s.nfc().collect::<String>(), s, "the keyboard types {c:?}, which is not its own NFC form");
        assert_eq!(canonical_combining_class(c), 0, "the keyboard types {c:?}, a combining mark");
        assert!(CODES.binary_search_by(|e| e.0.cmp(&c)).is_ok(), "the keyboard types {c:?}, which has no frozen code: add its code from the frozen codebook to src/frozen_codes.rs");
    }
}
