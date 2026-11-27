//! ferros's own words in every language, done as Photon does its text (photon docs/languages.md, src/ui/lang.rs): every word a person reads on the settings pages, the attest screen and the keyboard is a `Msg` (words alone) or a `Say` (words with holes), every language is one exhaustive match over them, and the compiler is the completeness checker: adding a message fails every language's build until its arm exists, so no English leaks silently into a translated screen.
//!
//! - **One language list, fgtw's** (`fgtw::text::Lang`, re-exported here): Photon's sixteen, its storage order, autonyms and picker order. The fleet's and the identity's words are fgtw's too (`fgtw::text::Msg`, `fleet` here), written once for Photon and ferros (owner, 2026-11-10).
//! - **Whole messages with holes**, never fragments glued together; multi-line passages are one message joined with '\n', the call site drawing each line.
//! - **Never translated:** handles, pairing words, names (a kist's, a device's, a network's, a zone's), console lines, brand and technology names (ferros, Photon, WiFi, Bluetooth, NFC, GPS, USB, LTE, NR, DNS, APN, MCS, MCC, VSWR, PMIC, UTC, DLS, DMS, IR, WPA2, WPA3, WEP, Dvorak, QWERTY), unit symbols (dBm, dB, dB-Hz, MHz, GHz, m, °C), the dozenal digit names (Zil, Zila, …).
//! - **Numbers are in the person's base:** a `Say` carries a count and its language writes it with `num` (tatau), or carries a value already written in that base as a `&str` when the caller formats it (an offset, a signal, a share).
//! - **Script coverage** (`renders`): the display draws every text with one face, Oxanium +glyphs (payloads/display), by its character map with no shaping. A language the face cannot draw is carried in full but not offered, and reads as English while it is the setting, rather than drawing missing-glyph boxes.
#![no_std]

mod catalog;
pub use catalog::*;
pub use fgtw::text::{Lang, COUNT};
/// The fleet's and the identity's words (fgtw::text): one-line labels and wrapping sentences, each said in its doc.
pub use fgtw::text::Msg as Fleet;

/// Whether the shell's face can draw this language (payloads/display: Oxanium +glyphs, by character map, no shaping). Measured against the face's character map (fc-query, 2026-11-10): Latin-1 and most of Latin Extended-A are there, so English, Spanish, Māori (ā ē ī ō ū), Portuguese, French (œ), Indonesian, German (ß ẞ), Swahili, Turkish (ı İ ğ ş), Italian, Polish (ą ć ę ł ń ś ź ż) and Filipino draw whole. Not there: Cyrillic (U+0400 up: Russian, Ukrainian), Devanagari (U+0900 up, and it needs shaping the display does not do: Hindi), and Vietnamese's ơ ư (U+01A0, U+01AF) with its precomposed tone letters (U+1EA0 up; the combining hook above U+0309, dot below U+0323 and horn U+031B are missing too, so no decomposed form draws either). Those four are carried in full and switched on here once the display has a face for them.
pub const fn renders(l: Lang) -> bool {
    !matches!(l, Lang::Vi | Lang::Ru | Lang::Hi | Lang::Uk)
}

/// The language the shell draws in now: the language in effect (the kernel works it out: the identity's while the device follows it, else the device's own; ferros_sys::lang), read at draw time as every preference is. English while it is none, a language this list does not have, or one the face cannot draw (`renders`).
#[cfg(feature = "process")]
pub fn current() -> Lang {
    match ferros_sys::lang::effective().and_then(Lang::from_iso) {
        Some(l) if renders(l) => l,
        _ => Lang::En,
    }
}

/// A message in the current language.
#[cfg(feature = "process")]
pub fn t(m: Msg) -> &'static str {
    text(current(), m)
}

/// A message with holes, in the current language, written into `o` with its counts in the device's base.
#[cfg(feature = "process")]
pub fn s(m: Say, o: &mut dyn core::fmt::Write) -> core::fmt::Result {
    say(current(), m, ferros_sys::numbers(tatau::Style::Glyphs), o)
}

/// A fleet or identity message (fgtw's), in the current language, written into `o`.
#[cfg(feature = "process")]
pub fn fleet(m: Fleet, o: &mut dyn core::fmt::Write) -> core::fmt::Result {
    fgtw::text::tr(current(), m, o)
}
