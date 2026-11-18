//! The one-shot converters (docs/VSF-AUDIT.md wave 0, owner 2026-11-17): each old form read strictly, once, and written back as its document; nothing else reads an old form.
//! A converter runs where a process first reads its values after the change (the GPS driver at start, ahau and the identity service once the identity is open, wallgen at start, the map at start and ferros_space as it syncs); it costs one read when the value is already a document.
//! When a value is not its old form exactly, the vault refuses the write, or the value read back is not what was written (the old value is then written back), the old value stays and the caller says so on the console, one line naming the value and why.
//! This file goes once every device has run a build with it (the next commit removes it, its old-form parsers with it).

use crate::{gps, map, secret, sponsor};
use alloc::vec::Vec;

/// What converting one value did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Done {
    /// Nothing under the name.
    Absent,
    /// A document already.
    Already,
    /// The old form, now its document.
    Converted,
}

/// One value in the vault, as a converter reads and writes it.
pub trait Slot {
    /// The whole value: Ok(None) when absent, Err when the vault refuses.
    fn read(&mut self) -> Result<Option<Vec<u8>>, ()>;
    /// True when the vault took it.
    fn write(&mut self, bytes: &[u8]) -> bool;
}

/// A slot from a read and a write (a process's two vault calls on one name).
impl<R: FnMut() -> Result<Option<Vec<u8>>, ()>, W: FnMut(&[u8]) -> bool> Slot for (R, W) {
    fn read(&mut self) -> Result<Option<Vec<u8>>, ()> {
        (self.0)()
    }
    fn write(&mut self, bytes: &[u8]) -> bool {
        (self.1)(bytes)
    }
}

/// Convert one value: read it, and unless it is absent or `is_new` already, make its document with `from_old` (None when the bytes are not the old form exactly), write it, read it back and compare.
/// A read back that differs writes the old value back, so a failed conversion leaves what was there.
pub fn upgrade(slot: &mut impl Slot, is_new: impl Fn(&[u8]) -> bool, from_old: impl FnOnce(&[u8]) -> Option<Vec<u8>>) -> Result<Done, &'static str> {
    let old = match slot.read() {
        Err(()) => return Err("the vault refused the read"),
        Ok(None) => return Ok(Done::Absent),
        Ok(Some(v)) => v,
    };
    if is_new(&old) {
        return Ok(Done::Already);
    }
    let Some(doc) = from_old(&old) else { return Err("it is not the old form either") };
    if !slot.write(&doc) {
        return Err("the vault refused the document");
    }
    match slot.read() {
        Ok(Some(back)) if back == doc => Ok(Done::Converted),
        _ if slot.write(&old) => Err("the document read back differs; the old value is written back"),
        _ => Err("the document read back differs, and the old value could not be written back"),
    }
}

/// An old run of EWE integers (the map's place and view, ferros_ledger::ewe), read whole: None unless every byte is part of one of at most `most` integers.
fn ewe_run(bytes: &[u8], most: usize) -> Option<Vec<u64>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if out.len() == most {
            return None;
        }
        let (v, k) = ferros_ledger::ewe::decode_u64(&bytes[at..])?;
        out.push(v);
        at += k;
    }
    Some(out)
}

/// The old forms' zigzag (a signed value as an unsigned one, payloads/map `zig`) undone.
fn unzig(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// `gps.last` as the driver kept it: latitude then longitude, i32 little-endian, 8 bytes, no time (so the document has none).
pub fn gps_last(old: &[u8], eagle: i64) -> Option<Vec<u8>> {
    let b: [u8; 8] = old.try_into().ok()?;
    let lat = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let lon = i32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    gps::build(lat, lon, None, eagle)
}

/// `map.place.<label>` as the map kept it: zigzag x, y, the radius, zigzag cos, sin, as EWE integers; a place kept before circles turned has the first three and stands upright.
pub fn place(old: &[u8], eagle: i64) -> Option<Vec<u8>> {
    let v = ewe_run(old, 5)?;
    let (c, s) = match v.len() {
        5 => (unzig(v[3]), unzig(v[4])),
        3 => (map::ONE, 0),
        _ => return None,
    };
    map::place(&[unzig(v[0]), unzig(v[1]), i64::try_from(v[2]).ok()?, c, s], eagle)
}

/// `map.view` as the map kept it: zigzag x, y, cos, sin, as EWE integers.
pub fn view(old: &[u8], eagle: i64) -> Option<Vec<u8>> {
    let v = ewe_run(old, 4)?;
    if v.len() != 4 {
        return None;
    }
    map::view(&[unzig(v[0]), unzig(v[1]), unzig(v[2]), unzig(v[3])], eagle)
}

/// `map.dev` as the map kept it: the one byte 1 (it never wrote another).
pub fn developer(old: &[u8], eagle: i64) -> Option<Vec<u8>> {
    if old != [1] {
        return None;
    }
    map::developer(true, eagle)
}

/// A 32-byte secret as it was kept: the bytes alone.
pub fn secret(kind: secret::Kind, old: &[u8], eagle: i64) -> Option<Vec<u8>> {
    secret::build(kind, &<[u8; 32]>::try_from(old).ok()?, eagle)
}

/// `fgtw.sponsor` as it was placed: the family (4 or 6), 16 address bytes (an IPv4 address in the first 4), the port big-endian; 19 bytes.
pub fn sponsor(old: &[u8], eagle: i64) -> Option<Vec<u8>> {
    let b: [u8; 19] = old.try_into().ok()?;
    let mut ip = [0u8; 16];
    ip.copy_from_slice(&b[1..17]);
    sponsor::build(&(b[0], ip, u16::from_be_bytes([b[17], b[18]])), eagle)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::secret::Kind;
    use std::cell::RefCell;

    /// The old forms, built exactly as the old code wrote them.
    fn old_place(x: i64, y: i64, r: i64, c: Option<(i64, i64)>) -> Vec<u8> {
        // payloads/map `save_place` (and ferros_space `encode_place`): zig, EWE.
        let zig = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut vals = std::vec![zig(x), zig(y), r as u64];
        if let Some((c, s)) = c {
            vals.extend([zig(c), zig(s)]);
        }
        let mut b = [0u8; 64];
        let mut n = 0;
        for v in vals {
            n += ferros_ledger::ewe::encode_u64(&mut b[n..], v);
        }
        b[..n].to_vec()
    }

    fn old_view(v: [i64; 4]) -> Vec<u8> {
        let zig = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut b = [0u8; 64];
        let mut n = 0;
        for x in v {
            n += ferros_ledger::ewe::encode_u64(&mut b[n..], zig(x));
        }
        b[..n].to_vec()
    }

    fn old_gps(lat: i32, lon: i32) -> Vec<u8> {
        // payloads/gps `keep_last`.
        let mut v = [0u8; 8];
        v[..4].copy_from_slice(&lat.to_le_bytes());
        v[4..].copy_from_slice(&lon.to_le_bytes());
        v.to_vec()
    }

    fn old_sponsor(fam: u8, ip: [u8; 16], port: u16) -> Vec<u8> {
        let mut g = std::vec![fam];
        g.extend_from_slice(&ip);
        g.extend_from_slice(&port.to_be_bytes());
        g
    }

    /// One vault value as a converter sees it: `refuse_read` and `refuse_write` stand for a vault that says no, `garble` for a write that lands as something else.
    struct Cell {
        v: RefCell<Option<Vec<u8>>>,
        refuse_read: bool,
        refuse_write: bool,
        garble: bool,
        writes: RefCell<u32>,
    }

    impl Cell {
        fn new(v: Option<Vec<u8>>) -> Cell {
            Cell { v: RefCell::new(v), refuse_read: false, refuse_write: false, garble: false, writes: RefCell::new(0) }
        }
        fn run(&self, is_new: impl Fn(&[u8]) -> bool, from_old: impl FnOnce(&[u8]) -> Option<Vec<u8>>) -> Result<Done, &'static str> {
            let mut slot = (
                || if self.refuse_read { Err(()) } else { Ok(self.v.borrow().clone()) },
                |b: &[u8]| {
                    if self.refuse_write {
                        return false;
                    }
                    let first = *self.writes.borrow() == 0;
                    *self.writes.borrow_mut() += 1;
                    let mut b = b.to_vec();
                    if self.garble && first {
                        b[0] ^= 1;
                    }
                    *self.v.borrow_mut() = Some(b);
                    true
                },
            );
            upgrade(&mut slot, is_new, from_old)
        }
    }

    const NOW: i64 = 1 << 50;

    #[test]
    fn the_last_fix_converts_once() {
        let c = Cell::new(Some(old_gps(37 << 23, -(122 << 23))));
        assert_eq!(c.run(|b| gps::parse(b).is_some(), |o| gps_last(o, NOW)), Ok(Done::Converted));
        let doc = c.v.borrow().clone().unwrap();
        let (la, lo, when) = gps::parse(&doc).unwrap();
        assert!((la - (37 << 23)).abs() <= 1 && (lo + (122 << 23)).abs() <= 1);
        assert_eq!(when, None, "the old form had no time, and none is made up");
        assert_eq!(crate::created(&doc), Some(NOW));
        // Run again: a document now, nothing written.
        assert_eq!(c.run(|b| gps::parse(b).is_some(), |o| gps_last(o, NOW)), Ok(Done::Already));
        assert_eq!(*c.writes.borrow(), 1);
        assert_eq!(gps_last(&old_gps(1, 2)[..7], NOW), None);
        assert_eq!(gps_last(&[0; 9], NOW), None);
    }

    #[test]
    fn places_convert_in_both_old_shapes() {
        let five = old_place(-123456, 789, 4000, Some((-map::ONE, 3)));
        assert_eq!(map::read_place(&place(&five, NOW).unwrap()), Some([-123456, 789, 4000, -map::ONE, 3]));
        let three = old_place(5, -6, 70, None);
        assert_eq!(map::read_place(&place(&three, NOW).unwrap()), Some([5, -6, 70, map::ONE, 0]), "a place from before turns stands upright");
        let c = Cell::new(Some(five));
        assert_eq!(c.run(|b| map::read_place(b).is_some(), |o| place(o, NOW)), Ok(Done::Converted));
        assert_eq!(c.run(|b| map::read_place(b).is_some(), |o| place(o, NOW)), Ok(Done::Already));
    }

    #[test]
    fn what_is_not_an_old_place_is_refused() {
        // Four values, a trailing byte, a cut value, a zero radius, a radius past the map's, other bytes.
        let four = old_view([1, 2, 3, 4]);
        let mut trailing = old_place(1, 2, 3, None);
        trailing.push(0);
        let full = old_place(1, 2, 3, Some((4, 5)));
        let cut = &full[..full.len() - 1];
        for bad in [&four[..], &trailing, cut, &old_place(1, 2, 0, None), &old_place(1, 2, 1 << 16, None), b"place", &[]] {
            assert_eq!(place(bad, NOW), None, "{bad:?}");
        }
    }

    #[test]
    fn the_view_and_developer_convert() {
        let v = [12 << 8, -3 << 8, 11585, 11585];
        assert_eq!(map::read_view(&view(&old_view(v), NOW).unwrap()), Some(v));
        assert_eq!(view(&old_place(1, 2, 3, None), NOW), None, "three values are not a view");
        assert_eq!(view(&old_place(1, 2, 3, Some((4, 5))), NOW), None, "five are not either");
        assert_eq!(map::read_developer(&developer(&[1], NOW).unwrap()), Some(true));
        for bad in [&[0u8][..], &[1, 1], &[2], &[]] {
            assert_eq!(developer(bad, NOW), None);
        }
    }

    #[test]
    fn secrets_convert_each_as_its_kind() {
        for (k, kind) in [Kind::HandleProof, Kind::IdentitySeed, Kind::FleetKey, Kind::WallpaperSeed, Kind::WallpaperMade].into_iter().enumerate() {
            let raw = [0xA0 + k as u8; 32];
            let c = Cell::new(Some(raw.to_vec()));
            assert_eq!(c.run(|b| secret::parse(kind, b).is_some(), |o| secret(kind, o, NOW)), Ok(Done::Converted));
            assert_eq!(secret::parse(kind, c.v.borrow().as_ref().unwrap()), Some(raw));
            assert_eq!(secret(kind, &raw[..31], NOW), None);
            assert_eq!(secret(kind, &[0; 33], NOW), None);
        }
    }

    #[test]
    fn the_sponsor_converts_in_either_family() {
        let mut v4 = [0u8; 16];
        v4[..4].copy_from_slice(&[10, 0, 0, 7]);
        assert_eq!(sponsor::parse(&sponsor(&old_sponsor(4, v4, 4383), NOW).unwrap()), Some((4, v4, 4383)));
        let v6 = [0xFE; 16];
        assert_eq!(sponsor::parse(&sponsor(&old_sponsor(6, v6, 1 << 15), NOW).unwrap()), Some((6, v6, 1 << 15)));
        assert_eq!(sponsor(&old_sponsor(5, v6, 1), NOW), None, "a family that is neither");
        assert_eq!(sponsor(&old_sponsor(4, v4, 1)[..18], NOW), None);
    }

    /// The refusals: nothing there, the vault refusing the read or the write, a value that is not the old form, a read back that differs; in each the old value is what stays.
    #[test]
    fn a_failed_conversion_leaves_the_old_value() {
        let is_new = |b: &[u8]| gps::parse(b).is_some();
        let conv = |o: &[u8]| gps_last(o, NOW);
        let old = old_gps(5, 6);
        assert_eq!(Cell::new(None).run(is_new, conv), Ok(Done::Absent));
        let mut c = Cell::new(Some(old.clone()));
        c.refuse_read = true;
        assert_eq!(c.run(is_new, conv), Err("the vault refused the read"));
        let mut c = Cell::new(Some(old.clone()));
        c.refuse_write = true;
        assert_eq!(c.run(is_new, conv), Err("the vault refused the document"));
        assert_eq!(c.v.borrow().as_deref(), Some(&old[..]));
        let c = Cell::new(Some(b"not eight".to_vec()));
        assert_eq!(c.run(is_new, conv), Err("it is not the old form either"));
        assert_eq!(c.v.borrow().as_deref(), Some(&b"not eight"[..]));
        let mut c = Cell::new(Some(old.clone()));
        c.garble = true;
        assert_eq!(c.run(is_new, conv), Err("the document read back differs; the old value is written back"));
        assert_eq!(c.v.borrow().as_deref(), Some(&old[..]));
    }
}
