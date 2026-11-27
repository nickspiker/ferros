//! The deck (payloads/map), keyed by what each circle is, never by its name (owner, 2026-11-20).
//!
//! - **A circle** ([`Circle`]) is a thing on the deck ([`CircleId`]: the settings circle, or a kist, a puck or a contact by its thing identity, `ht`), where it is, how large, how it is turned, the icon it shows by its content identity (`hp`), and whether it was moved by hand since arranging began.
//! - **The deck** ([`Deck`]) is the circles in the map's order: settings first, then each kist of the person's list.
//! - **The deck document** (`map.deck`, [`KEY`]): one VSF document, a section `circle` per circle placed by hand, written whole at every change: `kind` (`hi`, the circle's kind as an idea), `thing` (`ht`, absent for settings), `x`, `y` (`i`, world units ×256), `radius` (`u`, world units), `cos`, `sin` (`i`, its turn in Q14). A circle with no section stands where the seeded fit puts it.
//!
//! Until 2026-11-20 each circle's place was its own value, `map.place.<label>` (ferros_kept::map `place`), named by the kist's name: a rename lost its place and two names alike in their first 26 bytes shared one. [`migrate`] reads those once, by each circle's label as it was, and says which old names to drop.
//!
//! The kind is written for whoever reads the document; a circle is found by its thing ([`CircleId::same`]), so a kist that becomes a puck in an update (the clock did) keeps its place.

use crate::map;
use alloc::vec::Vec;
use ferros_thing::{kinds, Hi, Hp, Ht};
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// The deck document's name in the map's scope. Its one writer is the map (owner, 2026-11-20).
pub const KEY: &[u8] = b"deck";
/// The places the person's fleet sent that the map has not taken yet, a deck document of its own in the map's scope, whose one writer is the identity service's sync (ferros_space): the map reads it and takes each place into the deck ([`take`]); the sync drops what the map has taken ([`taken`]). So `map.deck` has one writer, and a place from the fleet never races the map's own write.
pub const SYNCED: &[u8] = b"deck.synced";

/// The fleet's places taken into the map's own: each place in `synced` set in `kept`. Whether anything changed (the map then writes its deck document, stamped after the synced copy, which tells the sync it was taken).
pub fn take(kept: &mut Kept, synced: &Kept) -> bool {
    let mut changed = false;
    for (id, p) in &synced.places {
        changed |= kept.set(*id, *p);
    }
    changed
}

/// What of the synced copy the map has taken: a place its deck holds the same, or every one once its deck document was written after the synced copy (the map takes the whole copy every time it reads its deck, `take`). The places still waiting stay in `synced`. Whether any was dropped.
pub fn taken(synced: &mut Kept, deck: &Kept, deck_made: Option<i64>, synced_made: Option<i64>) -> bool {
    let n = synced.places.len();
    if matches!((deck_made, synced_made), (Some(d), Some(s)) if d > s) {
        synced.places.clear();
    } else {
        synced.places.retain(|(id, p)| deck.get(id) != Some(p));
    }
    synced.places.len() != n
}
/// Its section, one per circle placed by hand.
pub const SECTION: &str = "circle";
/// The most circles one document holds: far past any deck (the map draws 16), so a document past it is no deck this build reads.
pub const CIRCLES_MAX: usize = 1 << 10;

/// What a circle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircleId {
    /// The system's settings circle, the one built in.
    Settings,
    /// A kist the person stowed, by its thing (ferros_thing `kist`).
    Kist(Ht),
    /// A puck, a kist that is its whole UI, by its thing (the same derivation as a kist's).
    Puck(Ht),
    /// A person, by their thing (ferros_thing `person`).
    Contact(Ht),
}

impl CircleId {
    /// Its kind, as an idea.
    pub fn kind(&self) -> Hi {
        match self {
            CircleId::Settings => kinds::SETTINGS,
            CircleId::Kist(_) => kinds::KIST,
            CircleId::Puck(_) => kinds::PUCK,
            CircleId::Contact(_) => kinds::CONTACT,
        }
    }

    /// The thing it is, none for settings.
    pub fn thing(&self) -> Option<Ht> {
        match self {
            CircleId::Settings => None,
            CircleId::Kist(t) | CircleId::Puck(t) | CircleId::Contact(t) => Some(*t),
        }
    }

    /// Whether two ids name one circle: settings is settings, anything else is its thing, whatever its kind now.
    pub fn same(&self, other: &CircleId) -> bool {
        self.thing() == other.thing()
    }

    /// An id from its kind and thing as the document carries them; None for a kind this build does not know, or a thing missing where the kind has one.
    pub fn from_parts(kind: &Hi, thing: Option<Ht>) -> Option<CircleId> {
        match (*kind, thing) {
            (k, None) if k == kinds::SETTINGS => Some(CircleId::Settings),
            (k, Some(t)) if k == kinds::KIST => Some(CircleId::Kist(t)),
            (k, Some(t)) if k == kinds::PUCK => Some(CircleId::Puck(t)),
            (k, Some(t)) if k == kinds::CONTACT => Some(CircleId::Contact(t)),
            _ => None,
        }
    }
}

/// A place on the plane, world units ×256.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct At {
    pub x: i64,
    pub y: i64,
}

/// A circle's own turn (its icon), (cos, sin) in Q14.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
    pub c: i64,
    pub s: i64,
}

impl Turn {
    /// Upright.
    pub const UP: Turn = Turn { c: map::ONE, s: 0 };

    /// Whether it is a turn the map takes: each part within one, and near the unit circle (a turn read back from elsewhere that is not one is left upright).
    pub fn ok(&self) -> bool {
        self.c.abs() <= map::ONE && self.s.abs() <= map::ONE && self.c * self.c + self.s * self.s > map::ONE * map::ONE / 2
    }
}

impl Default for Turn {
    fn default() -> Turn {
        Turn::UP
    }
}

/// Where a circle stands, as the document keeps it: its place, its radius in world units, its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub at: At,
    pub r: i64,
    pub turn: Turn,
}

impl Place {
    /// As the space's PLACE fact and ferros_kept::map's place document order it: x, y, radius, cos, sin.
    pub fn ints(&self) -> [i64; 5] {
        [self.at.x, self.at.y, self.r, self.turn.c, self.turn.s]
    }

    pub fn from_ints(p: &[i64; 5]) -> Place {
        Place { at: At { x: p[0], y: p[1] }, r: p[2], turn: Turn { c: p[3], s: p[4] } }
    }
}

/// One circle on the deck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Circle {
    pub id: CircleId,
    pub at: At,
    /// World units; 0 draws nothing.
    pub r: i64,
    pub turn: Turn,
    /// The icon it shows, by its content identity (a kist's: its capsule's icon section, ferros_kist `icon_hp`); none draws the theme's fill.
    pub icon: Option<Hp>,
    /// Moved or sized by hand since arranging began, to keep when it ends.
    pub arranged: bool,
}

impl Circle {
    pub fn new(id: CircleId, r: i64) -> Circle {
        Circle { id, at: At::default(), r, turn: Turn::UP, icon: None, arranged: false }
    }

    pub fn place(&self) -> Place {
        Place { at: self.at, r: self.r, turn: self.turn }
    }

    /// Stands where `p` says; a turn that is not one stays as it was.
    pub fn set_place(&mut self, p: &Place) {
        self.at = p.at;
        self.r = p.r;
        if p.turn.ok() {
            self.turn = p.turn;
        }
    }
}

/// The circles in the map's order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Deck {
    pub circles: Vec<Circle>,
}

impl Deck {
    pub fn n(&self) -> usize {
        self.circles.len()
    }

    /// The index of the circle that is `id`'s thing.
    pub fn find(&self, id: &CircleId) -> Option<usize> {
        self.circles.iter().position(|c| c.id.same(id))
    }
}

/// The places kept by hand, as the document holds them: each circle's id and place, in the order written.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Kept {
    pub places: Vec<(CircleId, Place)>,
}

impl Kept {
    /// The place kept for `id`'s thing.
    pub fn get(&self, id: &CircleId) -> Option<&Place> {
        self.places.iter().find(|(k, _)| k.same(id)).map(|(_, p)| p)
    }

    /// `id`'s place set (its kind as `id` says now), added after the rest when it had none. Whether anything changed.
    pub fn set(&mut self, id: CircleId, p: Place) -> bool {
        match self.places.iter_mut().find(|(k, _)| k.same(&id)) {
            Some(e) if *e == (id, p) => false,
            Some(e) => {
                *e = (id, p);
                true
            }
            None => {
                self.places.push((id, p));
                true
            }
        }
    }

    /// `id`'s place dropped. Whether it had one.
    pub fn remove(&mut self, id: &CircleId) -> bool {
        let n = self.places.len();
        self.places.retain(|(k, _)| !k.same(id));
        self.places.len() != n
    }
}

/// The radii a place may have (ferros_kept::map's: 1 to 2¹⁶ − 1 world units).
pub fn radius_ok(r: i64) -> bool {
    (1..1 << 16).contains(&r)
}

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION)
        .field("kind", TypeConstraint::Idea)
        .field("thing", TypeConstraint::Thing)
        .field("x", TypeConstraint::AnySigned)
        .field("y", TypeConstraint::AnySigned)
        .field("radius", TypeConstraint::AnyUnsigned)
        .field("cos", TypeConstraint::AnySigned)
        .field("sin", TypeConstraint::AnySigned)
}

/// The deck document: every place kept, stamped at `eagle`. None when a radius is one the map does not take (nothing is written then: the caller's deck stays as the vault has it) or the builder refuses it.
pub fn document(kept: &Kept, eagle: i64) -> Option<Vec<u8>> {
    let mut b = vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only();
    for (id, p) in &kept.places {
        if !radius_ok(p.r) {
            return None;
        }
        let mut s = schema().build().set("kind", VsfType::from(id.kind())).ok()?;
        if let Some(t) = id.thing() {
            s = s.set("thing", VsfType::from(t)).ok()?;
        }
        for (name, v) in [("x", p.at.x), ("y", p.at.y), ("cos", p.turn.c), ("sin", p.turn.s)] {
            s = s.set(name, VsfType::i(v as isize)).ok()?;
        }
        s = s.set("radius", VsfType::u(p.r as usize, false)).ok()?;
        b = b.add_unboxed(SECTION, s.encode().ok()?);
    }
    b.build().ok()
}

/// The deck document read back, verified whole first. None for anything that is not one: a section of another name, a field missing or of another type, a kind this build does not know, a radius the map does not take, or more than [`CIRCLES_MAX`] circles. A deck the reader cannot read whole is none, never part of one (a caller that took part for the whole would drop the rest at its next write).
pub fn read(doc: &[u8]) -> Option<Kept> {
    let (header, _) = vsf::verification::read_verified(doc, None).ok()?;
    if header.fields.len() > CIRCLES_MAX {
        return None;
    }
    let mut kept = Kept::default();
    for f in &header.fields {
        if f.name != SECTION {
            return None;
        }
        // WHY checked and `get`: the offsets are the document's own; a section past its end is no deck, not a panic.
        let bytes = doc.get(f.offset_bytes..f.offset_bytes.checked_add(f.size_bytes)?)?;
        // Each field against the schema: a name the circle does not have, or a value of another type, refuses it.
        let s = SectionBuilder::parse(schema(), bytes).ok()?;
        let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let kind = Hi::from_vsf(&first("kind")?).ok()?;
        let thing = match first("thing") {
            Some(v) => Some(Ht::from_vsf(&v).ok()?),
            None => None,
        };
        let id = CircleId::from_parts(&kind, thing)?;
        let int = |name: &str| first(name)?.as_i64();
        let r = i64::try_from(first("radius")?.as_u64()?).ok()?;
        if !radius_ok(r) {
            return None;
        }
        let p = Place { at: At { x: int("x")?, y: int("y")? }, r, turn: Turn { c: int("cos")?, s: int("sin")? } };
        // A thing twice is no deck: which place would it stand at?
        if kept.get(&id).is_some() {
            return None;
        }
        kept.places.push((id, p));
    }
    Some(kept)
}

/// The places kept the old way, one value a circle by its label (`map.place.<label>`, before 2026-11-20), as one deck: each circle's old value read by `get` under its old name ([`map::place_key`]), and the names to drop once the deck document is written. A circle with no old value has no place.
/// None when an old value is present and is no place document (an older form left by a conversion that failed): nothing migrates then, so no place is lost and none reads as the person removing it; the caller keeps them all out of the sync, as it kept out such a value before.
pub fn migrate(circles: &[(CircleId, &str)], mut get: impl FnMut(&[u8]) -> Option<Vec<u8>>) -> Option<(Kept, Vec<Vec<u8>>)> {
    let mut kept = Kept::default();
    let mut drop: Vec<Vec<u8>> = Vec::new();
    for (id, label) in circles {
        let key = map::place_key(label);
        if drop.contains(&key) {
            // Two labels alike in their first 26 bytes shared one value: the first circle took it.
            continue;
        }
        let Some(doc) = get(&key) else { continue };
        kept.set(*id, Place::from_ints(&map::read_place(&doc)?));
        drop.push(key);
    }
    Some((kept, drop))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    fn kist(n: u8) -> CircleId {
        CircleId::Kist(ferros_thing::kist(&[n; 32], &[n ^ 1; 32]))
    }

    fn place(x: i64, y: i64, r: i64, c: i64, s: i64) -> Place {
        Place::from_ints(&[x, y, r, c, s])
    }

    /// Saved and loaded: every place, the settings circle's with no thing, each kind as written.
    #[test]
    fn a_deck_round_trips() {
        let mut kept = Kept::default();
        kept.set(CircleId::Settings, place(-5 << 8, 7 << 8, 300, 0, map::ONE));
        kept.set(kist(1), place(900 << 8, -40 << 8, 180, -map::ONE, 0));
        kept.set(CircleId::Puck(ferros_thing::kist(&[2; 32], &[3; 32])), place(-700 << 8, 650 << 8, 220, 11585, -11585));
        kept.set(CircleId::Contact(ferros_thing::person(&[4; 32])), place(1, 2, 1, map::ONE, 0));
        let doc = document(&kept, 1 << 45).unwrap();
        assert_eq!(read(&doc), Some(kept.clone()));
        assert_eq!(crate::created(&doc), Some(1 << 45));
        std::println!("a deck of four: {} bytes", doc.len());
        // None kept is an empty deck, not no deck.
        assert_eq!(read(&document(&Kept::default(), 3).unwrap()), Some(Kept::default()));
    }

    /// Written at every change: a move replaces the circle's place, a new circle goes after the rest, a removal drops it; an unchanged place is no change.
    #[test]
    fn a_change_is_one_place() {
        let mut kept = Kept::default();
        assert!(kept.set(kist(1), place(1, 1, 10, map::ONE, 0)));
        assert!(kept.set(CircleId::Settings, place(0, 0, 20, map::ONE, 0)));
        assert!(!kept.set(kist(1), place(1, 1, 10, map::ONE, 0)));
        assert!(kept.set(kist(1), place(5, 5, 10, map::ONE, 0)));
        assert_eq!(kept.places.len(), 2);
        assert_eq!(kept.get(&kist(1)), Some(&place(5, 5, 10, map::ONE, 0)));
        assert!(kept.remove(&kist(1)));
        assert!(!kept.remove(&kist(1)));
        assert_eq!(kept.places, vec![(CircleId::Settings, place(0, 0, 20, map::ONE, 0))]);
    }

    /// A kist that becomes a puck in an update keeps its place: the circle is its thing, whatever its kind.
    #[test]
    fn a_kist_become_a_puck_keeps_its_place() {
        let t = ferros_thing::kist(&[8; 32], &[9; 32]);
        let mut kept = Kept::default();
        kept.set(CircleId::Kist(t), place(3, 4, 50, map::ONE, 0));
        let doc = read(&document(&kept, 1).unwrap()).unwrap();
        assert_eq!(doc.get(&CircleId::Puck(t)), Some(&place(3, 4, 50, map::ONE, 0)));
        let mut deck = Deck { circles: vec![Circle::new(CircleId::Settings, 10), Circle::new(CircleId::Puck(t), 10)] };
        assert_eq!(deck.find(&CircleId::Kist(t)), Some(1));
        deck.circles[1].set_place(doc.get(&CircleId::Puck(t)).unwrap());
        assert_eq!(deck.circles[1].place(), place(3, 4, 50, map::ONE, 0));
    }

    /// Anything not a whole deck reads as none: another section, a thing on settings, none on a kist, an unknown kind, a radius the map does not take, a thing twice, bytes that are not VSF, a document whose bytes changed.
    #[test]
    fn what_is_not_a_deck_is_refused() {
        assert_eq!(read(&[1, 2, 3]), None);
        assert_eq!(read(&map::place(&[1, 2, 3, 4, 5], 7).unwrap()), None, "a place is not a deck");
        let section = |fields: &[(&str, VsfType)]| {
            let mut s = vsf::VsfSection::new(SECTION);
            for (n, v) in fields {
                s.add_field(*n, v.clone());
            }
            vsf::VsfBuilder::new().creation_time_oscillations(1).provenance_only().add_section_direct(s).build().unwrap()
        };
        let good = |kind: Hi, thing: Option<Ht>, r: usize| {
            let mut f = vec![("kind", VsfType::from(kind)), ("x", VsfType::i(1)), ("y", VsfType::i(2)), ("radius", VsfType::u(r, false)), ("cos", VsfType::i(map::ONE as isize)), ("sin", VsfType::i(0))];
            if let Some(t) = thing {
                f.push(("thing", VsfType::from(t)));
            }
            f
        };
        let t = ferros_thing::kist(&[1; 32], &[2; 32]);
        assert!(read(&section(&good(kinds::KIST, Some(t), 9))).is_some(), "the same, well formed");
        assert_eq!(read(&section(&good(kinds::SETTINGS, Some(t), 9))), None, "settings is no thing");
        assert_eq!(read(&section(&good(kinds::KIST, None, 9))), None, "a kist is one");
        assert_eq!(read(&section(&good(Hi::from_bytes([5; 32]), Some(t), 9))), None, "a kind this build does not know");
        assert_eq!(read(&section(&good(kinds::KIST, Some(t), 0))), None);
        assert_eq!(read(&section(&good(kinds::KIST, Some(t), 1 << 16))), None);
        let mut hp_thing = good(kinds::KIST, None, 9);
        hp_thing.push(("thing", VsfType::hp(t.as_bytes().to_vec())));
        assert_eq!(read(&section(&hp_thing)), None, "a thing is an ht, never an hp");
        let mut twice = Kept::default();
        twice.places = vec![(kist(1), place(1, 1, 1, map::ONE, 0)), (CircleId::Puck(kist(1).thing().unwrap()), place(2, 2, 2, map::ONE, 0))];
        assert_eq!(read(&document(&twice, 1).unwrap()), None, "one thing twice");
        let mut kept = Kept::default();
        kept.set(kist(1), place(1, 1, 1, map::ONE, 0));
        let mut doc = document(&kept, 1).unwrap();
        let last = doc.len() - 2;
        doc[last] ^= 1;
        assert_eq!(read(&doc), None, "a byte changed after it was written");
        // A radius the map does not take is never written.
        let mut bad = Kept::default();
        bad.set(kist(1), place(1, 1, 0, map::ONE, 0));
        assert_eq!(document(&bad, 1), None);
    }

    /// The old per-label values become one deck, once: each circle's by its label as it was (spaces as underscores, cut at 26 bytes), a turn and all; the old names listed to drop, the one two long labels shared read once.
    #[test]
    fn the_old_places_migrate_once() {
        let mut vault: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        vault.insert(map::place_key("settings"), map::place(&[-5 << 8, 7 << 8, 300, 0, map::ONE], 1).unwrap());
        vault.insert(map::place_key("Mahere"), map::place(&[900 << 8, -40 << 8, 180, -map::ONE, 0], 1).unwrap());
        vault.insert(map::place_key("a very long kist name that goes on"), map::place(&[1 << 8, 2 << 8, 99, map::ONE, 0], 1).unwrap());
        let long = CircleId::Kist(ferros_thing::kist(&[6; 32], &[6; 32]));
        let longer = CircleId::Kist(ferros_thing::kist(&[7; 32], &[7; 32]));
        let circles = [(CircleId::Settings, "settings"), (kist(1), "Mahere"), (kist(2), "Photon"), (long, "a very long kist name that goes on"), (longer, "a very long kist name that goes further")];
        // An old value that is no place document: nothing migrates, nothing is lost.
        vault.insert(map::place_key("Photon"), b"not a place".to_vec());
        assert_eq!(migrate(&circles, |k| vault.get(k).cloned()), None);
        vault.remove(&map::place_key("Photon"));
        let (kept, drop) = migrate(&circles, |k| vault.get(k).cloned()).unwrap();
        assert_eq!(kept.places, vec![(CircleId::Settings, place(-5 << 8, 7 << 8, 300, 0, map::ONE)), (kist(1), place(900 << 8, -40 << 8, 180, -map::ONE, 0)), (long, place(1 << 8, 2 << 8, 99, map::ONE, 0))]);
        assert_eq!(drop, vec![map::place_key("settings"), map::place_key("Mahere"), map::place_key("a very long kist name that goes on")]);
        assert_eq!(map::place_key("a kist"), b"place.a_kist".to_vec());
        // The deck written, the old names dropped: nothing left to migrate.
        let doc = document(&kept, 2).unwrap();
        for k in &drop {
            vault.remove(k);
        }
        assert_eq!(read(&doc), Some(kept));
        assert_eq!(migrate(&circles, |k| vault.get(k).cloned()), Some((Kept::default(), vec![])));
    }

    /// The fleet's places wait in the synced copy until the map takes them: taken when its deck holds them, or all at once when its deck was written after the copy.
    #[test]
    fn the_map_takes_the_fleets_places() {
        let mut deck = Kept::default();
        deck.set(kist(1), place(1, 1, 10, map::ONE, 0));
        let mut synced = Kept::default();
        synced.set(kist(1), place(5, 5, 10, map::ONE, 0));
        synced.set(kist(2), place(7, 7, 20, map::ONE, 0));
        // Not taken yet: both wait.
        assert!(!taken(&mut synced.clone(), &deck, Some(10), Some(20)));
        assert!(take(&mut deck, &synced));
        assert!(!take(&mut deck, &synced), "taken twice is no change");
        assert_eq!(deck.get(&kist(1)), Some(&place(5, 5, 10, map::ONE, 0)));
        // The deck holds them: dropped from the copy, whatever the stamps.
        let mut s = synced.clone();
        assert!(taken(&mut s, &deck, Some(10), Some(20)));
        assert!(s.places.is_empty());
        // The person moved one after the map took the copy: the deck was written after it, so the copy goes whole and the move stands.
        deck.set(kist(1), place(9, 9, 10, map::ONE, 0));
        let mut s = synced.clone();
        assert!(taken(&mut s, &deck, Some(30), Some(20)));
        assert!(s.places.is_empty());
        // Same stamps (no clock yet): only what the deck holds the same goes.
        let mut s = synced.clone();
        assert!(taken(&mut s, &deck, Some(20), Some(20)));
        assert_eq!(s.places, vec![(kist(1), place(5, 5, 10, map::ONE, 0))]);
    }

    #[test]
    fn an_id_is_its_kind_and_thing() {
        for id in [CircleId::Settings, kist(1), CircleId::Puck(kist(2).thing().unwrap()), CircleId::Contact(kist(3).thing().unwrap())] {
            assert_eq!(CircleId::from_parts(&id.kind(), id.thing()), Some(id));
        }
    }
}
