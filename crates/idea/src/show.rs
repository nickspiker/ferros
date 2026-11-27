//! Which words a person is shown for an idea (docs/IDEAS.md "The display rule"): one pure function of the idea, the phrasings and links the device holds, the person's languages and the keys they trust.
//!
//! 0. **The idea's family**: the idea and every idea joined to it by a same-as link whose author the person trusts, followed link to link. Related links are never followed. The family's ideas are taken oldest first (owner, 2026-11-10: "the older id is shown first"): an idea's age is the earliest creation time of its first phrasing in a sheet signed by a key the person trusts; an idea with none comes after those with one, and ties go by the ids' bytes, so every device orders a family alike.
//! 1. **The person's languages, in their order**: for each, the family's ideas oldest first; for each idea, its phrasings in that language whose author the person trusts, best-ranked key first ("theirs, contacts', the developer's, the project's", the order of `trusted`), then the idea's first phrasing if it is in that language, whoever signed it. Among one key's phrasings: the register asked for, then plain, then any; then the newest.
//! 2. **Else the first phrasing**, in its own language, marked as such ([`Shown::First`]): the oldest idea's that the device holds.
//! 3. **Never blank**: a device holding none of the family's first phrasings has [`Shown::Missing`], for the caller to draw the id itself; every carrier of an idea carries its first phrasing, so this is a broken carrier, never a normal state.

use crate::{Id, Link, LinkKind, Locale, Phrasing, Register};
use alloc::vec::Vec;

/// What to draw for an idea.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shown<'a> {
    /// A phrasing in one of the person's languages.
    Phrased(&'a Phrasing),
    /// None in their languages: the idea's first phrasing in its own language, to be marked as such (its locale is on it).
    First(&'a Phrasing),
    /// No phrasing of the idea at all: draw the id.
    Missing,
}

impl<'a> Shown<'a> {
    /// The phrasing to draw, if any.
    pub fn phrasing(self) -> Option<&'a Phrasing> {
        match self {
            Shown::Phrased(p) | Shown::First(p) => Some(p),
            Shown::Missing => None,
        }
    }
}

/// Where `author` stands among the keys the person trusts (0 their own), or None.
fn rank(trusted: &[[u8; 32]], author: Option<[u8; 32]>) -> Option<usize> {
    let a = author?;
    trusted.iter().position(|k| *k == a)
}

/// The idea and every idea same-as links by trusted authors join to it, oldest first.
pub fn family(idea: &Id, phrasings: &[Phrasing], links: &[Link], trusted: &[[u8; 32]]) -> Vec<Id> {
    let mut fam: Vec<Id> = alloc::vec![*idea];
    // Grown until a pass adds nothing: each pass adds at least one id or ends, and there are at most two ids per link, so this ends.
    loop {
        let mut grew = false;
        for l in links.iter().filter(|l| l.kind == LinkKind::SameAs && rank(trusted, l.author).is_some()) {
            let [a, b] = l.ideas;
            let (has_a, has_b) = (fam.contains(&a), fam.contains(&b));
            if has_a && !has_b {
                fam.push(b);
                grew = true;
            } else if has_b && !has_a {
                fam.push(a);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    // Age: the earliest trusted sheet carrying the idea's first phrasing.
    let age = |i: &Id| phrasings.iter().filter(|p| p.idea == *i && p.is_first() && rank(trusted, p.author).is_some()).filter_map(|p| p.created).min();
    fam.sort_by(|x, y| {
        let (ax, ay) = (age(x), age(y));
        // A known age before an unknown one, then the older, then the ids' bytes.
        (ax.is_none(), ax, *x).cmp(&(ay.is_none(), ay, *y))
    });
    fam
}

/// What the person is shown for `idea` (the module docs give the rule): `languages` theirs in order, `trusted` the keys they trust in rank order, `register` the one the place wants (a key wants terse).
pub fn show<'a>(idea: &Id, phrasings: &'a [Phrasing], links: &[Link], languages: &[Locale], trusted: &[[u8; 32]], register: Register) -> Shown<'a> {
    let fam = family(idea, phrasings, links, trusted);
    // How well a phrasing's register fits: the one asked for, then plain, then any.
    let fit = |p: &Phrasing| -> u8 {
        if p.register == register {
            0
        } else if p.register == Register::Plain {
            1
        } else {
            2
        }
    };
    for lang in languages {
        for i in &fam {
            // A trusted author's phrasing ranks by its key; the first phrasing by an author the person does not trust comes after every trusted one.
            let best = phrasings
                .iter()
                .filter(|p| p.idea == *i && p.locale == *lang)
                .filter_map(|p| match rank(trusted, p.author) {
                    Some(r) => Some((r, p)),
                    None if p.is_first() => Some((trusted.len(), p)),
                    None => None,
                })
                // The newest wins among equals: its creation time negated would overflow at i64::MIN, so the comparison is reversed instead.
                .min_by(|(ra, a), (rb, b)| (ra, fit(a)).cmp(&(rb, fit(b))).then(b.created.cmp(&a.created)));
            if let Some((_, p)) = best {
                return Shown::Phrased(p);
            }
        }
    }
    for i in &fam {
        let first = phrasings.iter().filter(|p| p.idea == *i && p.is_first()).min_by_key(|p| rank(trusted, p.author).unwrap_or(trusted.len()));
        if let Some(p) = first {
            return Shown::First(p);
        }
    }
    Shown::Missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::tests::{key, signed};
    use crate::Sheet;
    use alloc::vec::Vec;
    use fgtw::pq::KeyBundle;

    /// Everything a set of sheets carries, each verified against `bundles` (or only read, its records unauthored, when its signer is not among them).
    fn hold(docs: &[Vec<u8>], bundles: &[KeyBundle]) -> (Vec<Phrasing>, Vec<Link>) {
        let (mut ps, mut ls) = (Vec::new(), Vec::new());
        for d in docs {
            let s = Sheet::verify(d, bundles).or_else(|_| Sheet::read(d)).unwrap();
            ps.extend(s.phrasings);
            ls.extend(s.links);
        }
        (ps, ls)
    }

    fn words<'a>(s: Shown<'a>) -> (&'a str, bool) {
        match s {
            Shown::Phrased(p) => (p.text.as_str(), false),
            Shown::First(p) => (p.text.as_str(), true),
            Shown::Missing => ("", false),
        }
    }

    #[test]
    fn the_fallback_is_the_first_phrasing_marked() {
        let dev = key(3);
        let haere = Phrasing::mint(*b"mri", "Haere mai", Register::Plain).unwrap();
        let (ps, ls) = hold(&[signed(&dev, 1, &[haere.clone()], &[])], &[dev.public()]);
        let t = [dev.public().ed25519()];
        // In English: none; the Māori first phrasing, marked.
        assert_eq!(words(show(&haere.idea, &ps, &ls, &[*b"eng"], &t, Register::Plain)), ("Haere mai", true));
        // In Māori: shown as theirs, not marked.
        assert_eq!(words(show(&haere.idea, &ps, &ls, &[*b"eng", *b"mri"], &t, Register::Plain)), ("Haere mai", false));
        // Trusting nobody: the first phrasing proves itself, still shown.
        assert_eq!(words(show(&haere.idea, &ps, &ls, &[*b"eng"], &[], Register::Plain)), ("Haere mai", true));
        // Nothing held: Missing, never blank words.
        assert_eq!(show(&haere.idea, &[], &[], &[*b"eng"], &t, Register::Plain), Shown::Missing);
    }

    #[test]
    fn languages_in_order_then_keys_in_order() {
        let (me, dev, stranger) = (key(1), key(3), key(5));
        let remove = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let es_dev = Phrasing::new(remove.idea, *b"spa", "Quitar de la flota", Register::Plain).unwrap();
        let es_me = Phrasing::new(remove.idea, *b"spa", "Sacar de la flota", Register::Plain).unwrap();
        let fr_stranger = Phrasing::new(remove.idea, *b"fra", "Retirer de la flotte", Register::Plain).unwrap();
        let docs = [signed(&dev, 1, &[remove.clone(), es_dev], &[]), signed(&me, 2, &[es_me], &[]), signed(&stranger, 3, &[fr_stranger], &[])];
        let (ps, ls) = hold(&docs, &[me.public(), dev.public(), stranger.public()]);
        let t = [me.public().ed25519(), dev.public().ed25519()];
        // Spanish first: the person's own phrasing beats the developer's.
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"spa", *b"eng"], &t, Register::Plain)), ("Sacar de la flota", false));
        // Their own key not trusted: the developer's.
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"spa"], &t[1..], Register::Plain)), ("Quitar de la flota", false));
        // French only by a key they do not trust: passed over for English, their next language.
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"fra", *b"eng"], &t, Register::Plain)), ("Remove from fleet", false));
        // French alone: the first phrasing, marked.
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"fra"], &t, Register::Plain)), ("Remove from fleet", true));
    }

    #[test]
    fn a_link_is_followed_only_when_its_signer_is_trusted() {
        let (dev, contact, stranger) = (key(3), key(4), key(5));
        // One meaning minted twice: in Korean (with a Chinese phrasing), later in English.
        let ko = Phrasing::mint(*b"kor", "함대에서 제거", Register::Plain).unwrap();
        let zh = Phrasing::new(ko.idea, *b"zho", "从舰队中移除", Register::Plain).unwrap();
        let en = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let same = Link::new(LinkKind::SameAs, ko.idea, en.idea);
        let docs = [signed(&contact, 1, &[ko.clone(), zh], &[]), signed(&dev, 2, &[en.clone()], &[]), signed(&stranger, 3, &[], &[same.clone()])];
        let bundles = [dev.public(), contact.public(), stranger.public()];
        let (ps, ls) = hold(&docs, &bundles);
        let t = [contact.public().ed25519(), dev.public().ed25519()];

        // The stranger's link is not followed: an English reader of the Korean idea sees the Korean first phrasing, marked.
        assert_eq!(words(show(&ko.idea, &ps, &ls, &[*b"eng"], &t, Register::Plain)), ("함대에서 제거", true));
        // Trusting the stranger too: the link joins them, English shown, and the Chinese reader of the English idea sees the Chinese.
        let t2 = [contact.public().ed25519(), dev.public().ed25519(), stranger.public().ed25519()];
        assert_eq!(words(show(&ko.idea, &ps, &ls, &[*b"eng"], &t2, Register::Plain)), ("Remove from fleet", false));
        assert_eq!(words(show(&en.idea, &ps, &ls, &[*b"zho"], &t2, Register::Plain)), ("从舰队中移除", false));
        // The same link signed by the developer the person trusts.
        let (ps2, ls2) = hold(&[docs[0].clone(), docs[1].clone(), signed(&dev, 4, &[], &[same])], &bundles);
        assert_eq!(words(show(&ko.idea, &ps2, &ls2, &[*b"eng"], &t, Register::Plain)), ("Remove from fleet", false));
        // The older id first: both first phrasings trusted, the Korean (made at 1) older than the English (at 2), so a reader of neither language sees the Korean, marked.
        assert_eq!(family(&en.idea, &ps2, &ls2, &t), alloc::vec![ko.idea, en.idea]);
        assert_eq!(words(show(&en.idea, &ps2, &ls2, &[*b"fra"], &t, Register::Plain)), ("함대에서 제거", true));
    }

    #[test]
    fn a_related_link_never_changes_what_is_shown() {
        let dev = key(3);
        let close = Phrasing::mint(*b"eng", "Close", Register::Plain).unwrap();
        let shut = Phrasing::mint(*b"spa", "Cerrar", Register::Plain).unwrap();
        let related = Link::new(LinkKind::Related, close.idea, shut.idea);
        let (ps, ls) = hold(&[signed(&dev, 1, &[close.clone(), shut.clone()], &[related])], &[dev.public()]);
        let t = [dev.public().ed25519()];
        assert_eq!(words(show(&close.idea, &ps, &ls, &[*b"spa"], &t, Register::Plain)), ("Close", true));
        assert_eq!(family(&close.idea, &ps, &ls, &t), alloc::vec![close.idea]);
    }

    #[test]
    fn a_phrasing_by_an_untrusted_key_is_never_shown() {
        let (dev, stranger) = (key(3), key(5));
        let remove = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let rude = Phrasing::new(remove.idea, *b"spa", "Borrar todo", Register::Plain).unwrap();
        let (ps, ls) = hold(&[signed(&dev, 1, &[remove.clone()], &[]), signed(&stranger, 2, &[rude], &[])], &[dev.public(), stranger.public()]);
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"spa"], &[dev.public().ed25519()], Register::Plain)), ("Remove from fleet", true));
        // Only read (no bundle held for it): unauthored, never shown either.
        let (ps, ls) = hold(&[signed(&dev, 1, &[remove.clone()], &[]), signed(&stranger, 2, &[Phrasing::new(remove.idea, *b"spa", "Borrar todo", Register::Plain).unwrap()], &[])], &[dev.public()]);
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"spa"], &[dev.public().ed25519()], Register::Plain)), ("Remove from fleet", true));
    }

    #[test]
    fn the_register_asked_for_then_plain_then_the_newest() {
        let dev = key(3);
        let remove = Phrasing::mint(*b"eng", "Remove from fleet", Register::Plain).unwrap();
        let terse = Phrasing::new(remove.idea, *b"eng", "Remove", Register::Terse).unwrap();
        let fixed = Phrasing::new(remove.idea, *b"spa", "Quitar de la flota", Register::Plain).unwrap();
        let old = Phrasing::new(remove.idea, *b"spa", "Remover de la flota", Register::Plain).unwrap();
        let docs = [signed(&dev, 1, &[remove.clone(), terse, old], &[]), signed(&dev, 2, &[fixed], &[])];
        let (ps, ls) = hold(&docs, &[dev.public()]);
        let t = [dev.public().ed25519()];
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"eng"], &t, Register::Terse)), ("Remove", false));
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"eng"], &t, Register::Plain)), ("Remove from fleet", false));
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"eng"], &t, Register::Verbose)), ("Remove from fleet", false));
        // Spanish has no terse: plain; and the newer sheet's correction wins.
        assert_eq!(words(show(&remove.idea, &ps, &ls, &[*b"spa"], &t, Register::Terse)), ("Quitar de la flota", false));
    }
}
