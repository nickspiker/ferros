// Host tests: every message in every language is written, keeps its holes, its line count and the house rules for drawn text.
extern crate std;
use super::*;
use crate::COUNT;
use std::string::String;
use std::vec;
use std::vec::Vec;

/// A hole's mark: a name or a written value the test can find again.
const H: &str = "\u{27e6}H\u{27e7}";

/// Every `Say`, its `&str` holes marked; with how many marks each must carry.
fn every_say() -> Vec<(Say<'static>, usize)> {
    vec![
        (Say::SatPlace { up: H, round: H, cn0: H, used: false }, 3),
        (Say::SatPlace { up: H, round: H, cn0: H, used: true }, 3),
        (Say::ChipHeard { messages: 1, kinds: 1 }, 0),
        (Say::ChipHeard { messages: 2900, kinds: 5 }, 0),
        (Say::DaylightTime(H), 1),
        (Say::HeardMcc(H), 1),
        (Say::HeardMccIn { mcc: H, country: H }, 2),
        (Say::TapsToDeveloper(1), 0),
        (Say::TapsToDeveloper(2), 0),
        (Say::TapsToDeveloper(5), 0),
        (Say::Charging(H), 1),
        (Say::OnBattery(H), 1),
        (Say::Throttled(H), 1),
        (Say::NotAskedYet(H), 1),
        (Say::UsingNow(H), 1),
        (Say::CouldNotStart(H), 1),
        (Say::Stopped(H), 1),
        (Say::ByDeveloper(H), 1),
        (Say::Fetching { name: H, got: H }, 2),
        (Say::FetchingOf { name: H, got: H, total: H }, 3),
        (Say::Signal(H), 1),
        (Say::JoiningNet(H), 1),
        (Say::PassphraseFor(H), 1),
        (Say::OutOfReach(H), 1),
        (Say::SecurityUnsupported(H), 1),
        (Say::DidNotAnswer(H), 1),
        (Say::RefusedPassphrase(H), 1),
        (Say::PassphraseLength { min: 8, max: 63 }, 0),
    ]
}

/// Drawn text never carries an em dash (the owner's rule: a line break where text wraps, plain punctuation elsewhere).
fn house(l: Lang, what: &str, s: &str) {
    assert!(!s.trim().is_empty(), "{l:?} {what} is empty");
    assert!(!s.contains('\u{2014}'), "{l:?} {what} has an em dash: {s}");
}

#[test]
fn every_message_in_every_language() {
    for &m in Msg::ALL {
        let english = text(Lang::En, m);
        for i in 0..COUNT {
            let l = Lang::from_index(i);
            let s = text(l, m);
            house(l, &std::format!("{m:?}"), s);
            // The call sites lay a message out by its lines: a translation keeps the English's count.
            assert_eq!(s.matches('\n').count(), english.matches('\n').count(), "{l:?} {m:?}: {s}");
        }
    }
}

#[test]
fn every_hole_in_every_language() {
    let f = tatau::Fmt::new(tatau::Base::Arabic, tatau::Style::Glyphs);
    for (m, marks) in every_say() {
        let mut english = String::new();
        say(Lang::En, m, f, &mut english).unwrap();
        for i in 0..COUNT {
            let l = Lang::from_index(i);
            let mut s = String::new();
            say(l, m, f, &mut s).unwrap();
            house(l, &std::format!("{m:?}"), &s);
            assert_eq!(s.matches(H).count(), marks, "{l:?} {m:?}: {s}");
            assert_eq!(s.matches('\n').count(), english.matches('\n').count(), "{l:?} {m:?}: {s}");
        }
    }
}

/// A count is written by the language in the base given, never as raw digits of its own.
#[test]
fn counts_are_in_the_base_given() {
    let hex = tatau::Fmt::new(tatau::Base::Hex, tatau::Style::Glyphs);
    for i in 0..COUNT {
        let l = Lang::from_index(i);
        let mut s = String::new();
        say(l, Say::PassphraseLength { min: 8, max: 63 }, hex, &mut s).unwrap();
        assert!(!s.contains("63"), "{l:?} wrote 63 in arabic under hex: {s}");
        let mut e = String::new();
        say(Lang::En, Say::PassphraseLength { min: 8, max: 63 }, hex, &mut e).unwrap();
        let written = e.trim_start_matches("A passphrase is ");
        let max = written.split(" to ").nth(1).unwrap().trim_end_matches(" plain characters.");
        assert!(s.contains(max), "{l:?} lost the count {max}: {s}");
    }
}

#[test]
fn the_shell_draws_twelve_of_sixteen() {
    assert_eq!((0..COUNT).map(Lang::from_index).filter(|&l| crate::renders(l)).count(), 12);
    assert!(crate::renders(Lang::En));
}
