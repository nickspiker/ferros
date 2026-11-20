//! The map's own copy of the deck (payloads/map; synced across the person's fleet by ferros_space, whose facts stay VSF `i` values in the shared space): one document per value.
//! - `map.place.<label>`: section `place`, `x`, `y` (`i`, world units ×256), `radius` (`u`, world units), `cos`, `sin` (`i`, the circle's turn in Q14).
//! - `map.view`: section `view`, `x`, `y`, `cos`, `sin` (`i`): where the camera is and how the plane is turned.
//! - `map.dev`: section `developer`, `unlocked` (`u0`): the twelve taps made Developer a page on a release build.
//!
//! This is the one definition: the map writes and reads these, and ferros_space reads and writes the same device copies while it syncs.

use crate::{document, first, only};
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// One in Q14, the turns' scale (payloads/map `ONE`).
pub const ONE: i64 = 1 << 14;

pub const PLACE: &str = "place";
pub const VIEW: &str = "view";
pub const DEVELOPER: &str = "developer";

/// A circle's old vault name in the map's scope (until 2026-11-20, read only to migrate to the deck document, ferros_kept::deck): `place.` and its label, a space as an underscore, cut at 26 bytes (a name in the map's scope held 32).
pub fn place_key(label: &str) -> Vec<u8> {
    let mut k = b"place.".to_vec();
    for &c in label.as_bytes().iter().take(26) {
        k.push(if c == b' ' { b'_' } else { c });
    }
    k
}

/// The radii a place may have: 1 to 2¹⁶ − 1 world units, what the map has always taken (a radius of 0 draws nothing, and the map's loader refused 2¹⁶ and past before this document existed).
fn radius_ok(r: i64) -> bool {
    (1..1 << 16).contains(&r)
}

fn place_schema() -> SectionSchema {
    SectionSchema::new(PLACE).field("x", TypeConstraint::AnySigned).field("y", TypeConstraint::AnySigned).field("radius", TypeConstraint::AnyUnsigned).field("cos", TypeConstraint::AnySigned).field("sin", TypeConstraint::AnySigned)
}

fn view_schema() -> SectionSchema {
    SectionSchema::new(VIEW).field("x", TypeConstraint::AnySigned).field("y", TypeConstraint::AnySigned).field("cos", TypeConstraint::AnySigned).field("sin", TypeConstraint::AnySigned)
}

fn developer_schema() -> SectionSchema {
    SectionSchema::new(DEVELOPER).field("unlocked", only("u0", |v| matches!(v, VsfType::u0(_))))
}

/// The document for a place: x, y, radius, cos, sin (the space's PLACE fact's order); stamped at `eagle`. None for a radius the map does not take (`radius_ok`).
pub fn place(p: &[i64; 5], eagle: i64) -> Option<Vec<u8>> {
    if !radius_ok(p[2]) {
        return None;
    }
    let mut b = place_schema().build();
    for (name, v) in [("x", p[0]), ("y", p[1]), ("cos", p[3]), ("sin", p[4])] {
        b = b.set(name, VsfType::i(v as isize)).ok()?;
    }
    b = b.set("radius", VsfType::u(p[2] as usize, false)).ok()?;
    document(PLACE, b.encode().ok()?, eagle)
}

/// A place document read back: x, y, radius, cos, sin. None for anything that is not one, a field missing, or a radius the map does not take.
pub fn read_place(doc: &[u8]) -> Option<[i64; 5]> {
    let s = SectionBuilder::parse_document(place_schema(), doc, None).ok()?;
    let int = |name: &str| first(&s, name)?.as_i64();
    let r = i64::try_from(first(&s, "radius")?.as_u64()?).ok()?;
    let p = [int("x")?, int("y")?, r, int("cos")?, int("sin")?];
    radius_ok(p[2]).then_some(p)
}

/// The document for the view: x, y, cos, sin; stamped at `eagle`.
pub fn view(v: &[i64; 4], eagle: i64) -> Option<Vec<u8>> {
    let mut b = view_schema().build();
    for (name, x) in [("x", v[0]), ("y", v[1]), ("cos", v[2]), ("sin", v[3])] {
        b = b.set(name, VsfType::i(x as isize)).ok()?;
    }
    document(VIEW, b.encode().ok()?, eagle)
}

/// A view document read back: x, y, cos, sin. None for anything that is not one.
pub fn read_view(doc: &[u8]) -> Option<[i64; 4]> {
    let s = SectionBuilder::parse_document(view_schema(), doc, None).ok()?;
    let int = |name: &str| first(&s, name)?.as_i64();
    Some([int("x")?, int("y")?, int("cos")?, int("sin")?])
}

/// The document saying whether Developer is a page; stamped at `eagle`.
pub fn developer(unlocked: bool, eagle: i64) -> Option<Vec<u8>> {
    let b = developer_schema().build().set("unlocked", VsfType::u0(unlocked)).ok()?;
    document(DEVELOPER, b.encode().ok()?, eagle)
}

/// A Developer document read back. None for anything that is not one.
pub fn read_developer(doc: &[u8]) -> Option<bool> {
    let s = SectionBuilder::parse_document(developer_schema(), doc, None).ok()?;
    match first(&s, "unlocked")? {
        VsfType::u0(b) => Some(b),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn a_place_round_trips() {
        for p in [[-123456, 789, 4000, -ONE, 3], [0, 0, 1, ONE, 0], [i32::MIN as i64 * 256, i32::MAX as i64 * 256, (1 << 16) - 1, 11585, -11585]] {
            let doc = place(&p, 1 << 45).unwrap();
            assert_eq!(read_place(&doc), Some(p));
            assert_eq!(crate::created(&doc), Some(1 << 45));
        }
        std::println!("a place: {} bytes", place(&[-123456, 789, 4000, -ONE, 3], 1 << 45).unwrap().len());
    }

    #[test]
    fn a_radius_the_map_does_not_take_is_never_written() {
        assert_eq!(place(&[1, 2, 0, ONE, 0], 3), None);
        assert_eq!(place(&[1, 2, 1 << 16, ONE, 0], 3), None);
        assert_eq!(place(&[1, 2, -5, ONE, 0], 3), None);
    }

    #[test]
    fn a_view_round_trips() {
        let v = [12 << 8, -3 << 8, 11585, 11585];
        let doc = view(&v, 7).unwrap();
        assert_eq!(read_view(&doc), Some(v));
        assert_eq!(read_place(&doc), None, "a view is not a place");
        assert_eq!(read_view(&place(&[1, 2, 3, 4, 5], 7).unwrap()), None, "nor a place a view");
        std::println!("the view: {} bytes", doc.len());
    }

    #[test]
    fn developer_round_trips() {
        assert_eq!(read_developer(&developer(true, 9).unwrap()), Some(true));
        assert_eq!(read_developer(&developer(false, 9).unwrap()), Some(false));
        assert_eq!(read_developer(&[1]), None);
    }
}
