//! A thing's identity (owner, 2026-11-20: the thing as VSF `ht`, its content as `hp`, its kind as `hi`).
//!
//! - **`ht`, the thing:** what stays the same while its content changes. A kist across its updates, a person across their avatars, a hold item across its edits.
//! - **`hp`, one version's content:** these exact bytes. An icon, one build of a kist. For caches and dedup.
//! - **`hi`, the kind:** an idea (IDEAS.md). A reference to a thing is (kind, thing), optionally with its current content.
//!
//! Each kind's `ht` is derived under a BLAKE3 derive-key context of its own, so two kinds never share one even over the same bytes:
//! - **A kist or a puck** ([`kist`]): its developer's Ed25519 key and the option they minted for it (the capsule's `kist` section's `option`), the kist's identity across its versions as the gate already knows it (ferros_kist `identity`). A puck is a kist that declares the puck right, and a kist may become one in an update (the clock did), so both take one context: the thing is the same, its kind on the deck says which it is now.
//! - **A person** ([`person`]): the identity key pinned when they were first met (fgtw::people `Person::identity`, Ed25519 of their identity seed), Photon's identifier for a person (PHOTON.md M2), never their handle.
//! - **A hold item** ([`hold_item`]): minted when first kept, from 32 random bytes and what is first true of it, under the context ferros_items has minted ids with since 2026-11-19, so every item kept since keeps its id.
//!
//! A kist's icon's `hp` ([`icon`]) is the BLAKE3 of its capsule's signed `icon` section as the capsule holds it: the same icon in two capsules is one icon, a changed icon is another.
//!
//! no_std and no heap, so ferros's processes and Photon both derive the same 32 bytes.
#![no_std]

pub use vsf::types::{Hi, Hp, Ht};

/// The derive-key context of a kist's or a puck's thing identity.
pub const KIST: &str = "ferros thing 2026-11-20 kist";
/// The derive-key context of a person's thing identity.
pub const PERSON: &str = "ferros thing 2026-11-20 person";
/// The derive-key context a hold item's identity is minted under (ferros_items `make`, since 2026-11-19): kept, so an item kept before this crate keeps its id.
pub const HOLD_ITEM: &str = "ferros items 2026-11-19 id";

/// A kist's or a puck's thing: its developer's Ed25519 key, then its option.
pub fn kist(developer: &[u8; 32], option: &[u8; 32]) -> Ht {
    let mut h = blake3::Hasher::new_derive_key(KIST);
    h.update(developer);
    h.update(option);
    Ht::from_bytes(*h.finalize().as_bytes())
}

/// A person's thing: their pinned identity key.
pub fn person(identity: &[u8; 32]) -> Ht {
    let mut h = blake3::Hasher::new_derive_key(PERSON);
    h.update(identity);
    Ht::from_bytes(*h.finalize().as_bytes())
}

/// A hold item's thing, minted when it is first kept: 32 bytes drawn for it, its kind, its maker's tag (ferros_items `maker_tag`) and when it was made (Eagle oscillations), so the same bytes kept twice are two items and nobody can confirm a guess.
pub fn hold_item(random: &[u8; 32], kind: &Hi, maker: &[u8; 32], made: i64) -> Ht {
    let mut h = blake3::Hasher::new_derive_key(HOLD_ITEM);
    h.update(random);
    h.update(kind.as_bytes());
    h.update(maker);
    h.update(&made.to_le_bytes());
    Ht::from_bytes(*h.finalize().as_bytes())
}

/// A kist's icon's content identity: the BLAKE3 of the capsule's `icon` section, its bytes as the capsule holds them (ferros_kist `icon_hp`).
pub fn icon(section: &[u8]) -> Hp {
    Hp::from_bytes(*blake3::hash(section).as_bytes())
}

/// The deck's kinds, each an idea pinned by its first English phrasing (`ferros_idea::id(eng, words)`; the test holds each to it).
pub mod kinds {
    use super::Hi;

    /// "settings": the system's own circle, the one built in.
    pub const SETTINGS: Hi = Hi::from_bytes([109, 190, 57, 36, 30, 23, 88, 13, 219, 169, 132, 202, 158, 245, 168, 34, 82, 176, 229, 244, 252, 254, 6, 106, 19, 114, 131, 144, 227, 197, 82, 143]);
    /// "kist": an app the person stowed.
    pub const KIST: Hi = Hi::from_bytes([110, 49, 150, 130, 14, 244, 249, 7, 245, 223, 77, 156, 140, 87, 95, 40, 91, 128, 75, 24, 24, 255, 108, 27, 72, 137, 123, 219, 242, 1, 196, 172]);
    /// "puck": a kist that is its whole UI, a live circle never opened.
    pub const PUCK: Hi = Hi::from_bytes([113, 174, 213, 4, 40, 103, 9, 202, 91, 246, 31, 249, 91, 241, 189, 12, 18, 64, 180, 132, 9, 233, 166, 142, 155, 108, 118, 53, 132, 83, 27, 141]);
    /// "contact": a person (the hold's kind of the same words, ferros_items `kinds::CONTACT`).
    pub const CONTACT: Hi = Hi::from_bytes([75, 70, 179, 140, 250, 88, 36, 7, 82, 170, 48, 162, 74, 120, 24, 5, 233, 201, 201, 153, 243, 38, 245, 177, 241, 227, 250, 113, 243, 23, 48, 158]);

    /// (the first phrasing, the idea).
    pub const ALL: [(&str, Hi); 4] = [("settings", SETTINGS), ("kist", KIST), ("puck", PUCK), ("contact", CONTACT)];
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::format;
    use std::string::String;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn kinds_are_their_first_phrasings() {
        for (words, id) in kinds::ALL {
            let want = ferros_idea::id(*b"eng", words).unwrap();
            assert_eq!(*id.as_bytes(), want, "{words}: [{}]", want.iter().map(|b| format!("{b}, ")).collect::<String>());
        }
    }

    /// Each derivation pinned to its bytes: a changed context or input order is a changed identity for every thing of that kind, so it fails here first.
    #[test]
    fn derivations_are_pinned() {
        let (d, o) = ([1u8; 32], [2u8; 32]);
        let got = [hex(kist(&d, &o).as_bytes()), hex(person(&d).as_bytes()), hex(hold_item(&d, &kinds::CONTACT, &o, 1 << 45).as_bytes()), hex(icon(b"an icon section").as_bytes())];
        assert_eq!(got, ["a285210e8c5b70567fa332625bae7d6dbd2ec8d92b05119ddce89b37d101ffb9", "50b359aa20a91c00c54519747d15fb1b2e824a9b5fa336263c29d8d3fea8c392", "bc1a84b9d9a560f336494d6f6b9c18b1e799696987f83a9823c6a29c16f1c20d", "489ffa60a8b2bf9091c75cdcb907e6eb7359e1ff8f8cfda2e6268ddf684a6716"]);
    }

    /// Kinds never collide: the same 32 bytes as a kist's developer, a person's identity and an item's random give three things.
    #[test]
    fn kinds_never_collide() {
        let k = [7u8; 32];
        let (a, b) = (kist(&k, &[0; 32]), person(&k));
        assert_ne!(a, b);
        assert_ne!(a, hold_item(&k, &kinds::KIST, &[0; 32], 0));
        assert_ne!(*b.as_bytes(), *icon(&k).as_bytes());
        // The developer and the option are not interchangeable.
        assert_ne!(kist(&[1; 32], &[2; 32]), kist(&[2; 32], &[1; 32]));
    }

    /// A hold item's id is what ferros_items minted before this crate (its `make` from 2026-11-19), so an item kept then keeps it.
    #[test]
    fn a_hold_item_keeps_its_id() {
        let (random, maker, made) = ([9u8; 32], [3u8; 32], -(1i64 << 50));
        let mut h = blake3::Hasher::new_derive_key("ferros items 2026-11-19 id");
        h.update(&random);
        h.update(kinds::CONTACT.as_bytes());
        h.update(&maker);
        h.update(&made.to_le_bytes());
        assert_eq!(*hold_item(&random, &kinds::CONTACT, &maker, made).as_bytes(), *h.finalize().as_bytes());
    }

    /// A thing is the same across what changes: a kist's identity alone names it, whatever its build.
    #[test]
    fn a_kist_is_the_same_across_versions() {
        let (d, o) = ([4u8; 32], [5u8; 32]);
        assert_eq!(kist(&d, &o), kist(&d, &o));
        assert_ne!(kist(&d, &o), kist(&d, &[6; 32]), "another option is another kist");
        assert_ne!(kist(&d, &o), kist(&[8; 32], &o), "another developer is another kist");
    }
}
