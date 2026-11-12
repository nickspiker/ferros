//! A large value in a sealed key-value store as pieces, for the kernel's vault (ferros_kernel kvault.rs): a value over `PIECE` bytes is stored as pieces of at most `PIECE`, each sealed on its own, under a manifest that names how long the value is, how it was cut, which generation of pieces holds it and the BLAKE3 of the whole.
//!
//! Why: manifestus stores a value whole, and the kernel sealed it whole, so storing a 2.8 MB capsule held the sealed value and its staged furrows (each a little over the value) in the kernel heap at once, each in one block, and reading it back held the value and a 1 MiB bounce; spawning it then copied its code twice more. Under a busy kernel (2026-11-12: Mahere running, the display capsule stored and restarted) the 22 MiB heap had the bytes but not a 3 MB hole, and the kernel panicked.
//! In pieces, nothing holds more than one piece in one block: a put seals, stages and replaces one piece at a time, and a read hands each piece to its caller as it opens (`read`), which may copy it straight to where it is going (a process's arc) and never hold the whole.
//!
//! Crash safety: a put writes its pieces under a new generation, never over the pieces the manifest names, and switches the manifest only when every piece is down (each store write is durable on return). Before the pieces, the manifest records the new generation as pending, so pieces a crash leaves behind are found and dropped by the next put or remove of the name. A crash leaves the old value or the new one, never a mix.
//!
//! Addresses: the manifest and the pieces live under a key derived from the store's address key (`Place`), so no name's own address (a keyed hash under the store's key) can be one of them. The store seals each value with its own address bound in (associated data), so a piece moved to another address does not open; the whole value's hash in the manifest catches the rest (a piece of another generation, a short one).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// The largest piece: 512 KiB. A put holds a sealed piece, its staged furrows and, for a while, the piece it replaces, each a block of about this size; a read holds a piece and manifestus's bounce for it.
pub const PIECE: usize = 1 << 19;

/// The context deriving a store's pieces key from its address key.
const KEY_CONTEXT: &str = "ferros 2026-11-12 kernel vault pieces";
/// The manifest: a VSF document with one section.
const SECTION: &str = "pieces";
const LENGTH: &str = "length";
const CUT: &str = "piece";
const GENERATION: &str = "generation";
const HASH: &str = "hash";
const PENDING: &str = "pending";

/// What the pieces are kept in: values by 32-byte address, each sealed by the store with its address as associated data.
pub trait Store {
    type Error;
    /// The value at `addr`, opened; None when there is none.
    fn get(&mut self, addr: &[u8; 32]) -> Result<Option<Vec<u8>>, Self::Error>;
    /// `value` at `addr`, sealed, durable when this returns.
    fn put(&mut self, addr: &[u8; 32], value: &[u8]) -> Result<(), Self::Error>;
    /// The value at `addr` gone; whether there was one.
    fn delete(&mut self, addr: &[u8; 32]) -> Result<bool, Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Store(E),
    /// The manifest or a piece is not what it should be: unreadable, missing, the wrong length, or the whole not its hash.
    Corrupt(&'static str),
}

/// Where one name's manifest and pieces live.
pub struct Place<'a> {
    key: [u8; 32],
    name: &'a [u8],
}

impl<'a> Place<'a> {
    /// The place of `name` in a store whose names hash under `addr_key`.
    pub fn new(addr_key: &[u8; 32], name: &'a [u8]) -> Self {
        Place { key: blake3::derive_key(KEY_CONTEXT, addr_key), name }
    }

    /// The manifest's address: 0, then the name.
    pub fn manifest(&self) -> [u8; 32] {
        *blake3::Hasher::new_keyed(&self.key).update(&[0]).update(self.name).finalize().as_bytes()
    }

    /// Piece `index` of `generation`: 1, the two numbers (8 bytes each, little-endian: hash input, never stored), then the name.
    pub fn piece(&self, generation: u64, index: u64) -> [u8; 32] {
        *blake3::Hasher::new_keyed(&self.key).update(&[1]).update(&generation.to_le_bytes()).update(&index.to_le_bytes()).update(self.name).finalize().as_bytes()
    }
}

/// A value the manifest names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    pub length: u64,
    /// The cut: every piece this long but the last.
    pub piece: u64,
    pub generation: u64,
    /// BLAKE3 of the whole value.
    pub hash: [u8; 32],
}

impl Value {
    fn count(&self) -> u64 {
        self.length.div_ceil(self.piece)
    }

    /// Piece `k`'s length.
    fn len_of(&self, k: u64) -> u64 {
        let rest = self.length - k * self.piece;
        if rest < self.piece { rest } else { self.piece }
    }
}

/// The manifest: the value it names, if any, and a generation whose pieces may lie about without a manifest naming them (a put under way or cut short, or a value replaced).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub value: Option<Value>,
    pub pending: Option<u64>,
}

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION).field(LENGTH, TypeConstraint::AnyUnsigned).field(CUT, TypeConstraint::AnyUnsigned).field(GENERATION, TypeConstraint::AnyUnsigned).field(HASH, TypeConstraint::Blake3Rolling).field(PENDING, TypeConstraint::AnyUnsigned)
}

impl Manifest {
    /// The VSF document, stamped `now` (Eagle time, oscillations).
    pub fn encode(&self, now: i64) -> Result<Vec<u8>, &'static str> {
        let bad = |_| "the manifest does not encode";
        let mut b = schema().build();
        if let Some(v) = &self.value {
            b = b.set(LENGTH, VsfType::u(v.length as usize, false)).map_err(bad)?;
            b = b.set(CUT, VsfType::u(v.piece as usize, false)).map_err(bad)?;
            b = b.set(GENERATION, VsfType::u(v.generation as usize, false)).map_err(bad)?;
            b = b.set(HASH, VsfType::hb(v.hash.to_vec())).map_err(bad)?;
        }
        if let Some(p) = self.pending {
            b = b.set(PENDING, VsfType::u(p as usize, false)).map_err(bad)?;
        }
        let section = b.encode().map_err(bad)?;
        vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed(SECTION, section).build().map_err(|_| "the manifest does not build")
    }

    /// The manifest from its document: refused unless it verifies and every field it carries is whole.
    pub fn decode(doc: &[u8]) -> Result<Self, &'static str> {
        let s = SectionBuilder::parse_document(schema(), doc, None).map_err(|_| "the manifest does not read")?;
        let num = |name: &str| -> Result<Option<u64>, &'static str> {
            match s.get_fields(name).first().and_then(|f| f.values.first()) {
                None => Ok(None),
                Some(v) => <u64 as vsf::schema::FromVsfType>::from_vsf_type(v).map(Some).map_err(|_| "a manifest number is not one"),
            }
        };
        let pending = num(PENDING)?;
        let value = match (num(LENGTH)?, num(CUT)?, num(GENERATION)?) {
            (None, None, None) => None,
            (Some(length), Some(piece), Some(generation)) => {
                // WHY: the cut divides the length (`count`); it comes off the disk, and a zero would divide by zero.
                if piece == 0 {
                    return Err("the manifest's cut is zero");
                }
                let hash = match s.get_fields(HASH).first().and_then(|f| f.values.first()) {
                    Some(VsfType::hb(h)) if h.len() == 32 => <[u8; 32]>::try_from(&h[..]).unwrap(),
                    _ => return Err("the manifest's hash is missing"),
                };
                Some(Value { length, piece, generation, hash })
            }
            _ => return Err("the manifest names part of a value"),
        };
        Ok(Manifest { value, pending })
    }

    /// A generation no piece of this name has had: past both the value's and the pending one.
    fn next_generation(&self) -> u64 {
        let v = self.value.map_or(0, |v| v.generation + 1);
        let p = self.pending.map_or(0, |p| p + 1);
        if v > p { v } else { p }
    }
}

fn store<E>(e: E) -> Error<E> {
    Error::Store(e)
}

/// The manifest of `place`, if it has one.
pub fn manifest<S: Store>(s: &mut S, place: &Place) -> Result<Option<Manifest>, Error<S::Error>> {
    match s.get(&place.manifest()).map_err(store)? {
        None => Ok(None),
        Some(doc) => Manifest::decode(&doc).map(Some).map_err(Error::Corrupt),
    }
}

fn write<S: Store>(s: &mut S, place: &Place, m: &Manifest, now: i64) -> Result<(), Error<S::Error>> {
    let doc = m.encode(now).map_err(Error::Corrupt)?;
    s.put(&place.manifest(), &doc).map_err(store)
}

/// Every piece of `generation` gone, from the first while there is one (a put writes them in order, each durable before the next, so a cut-short put leaves a run from the first).
fn drop_generation<S: Store>(s: &mut S, place: &Place, generation: u64) -> Result<(), Error<S::Error>> {
    let mut k = 0;
    while s.delete(&place.piece(generation, k)).map_err(store)? {
        k += 1;
    }
    Ok(())
}

/// Store `value` under `place` in pieces of `PIECE`: the new generation's pieces first, then the manifest naming them, then the old generation's pieces dropped. `now` stamps the manifest.
pub fn put<S: Store>(s: &mut S, place: &Place, value: &[u8], now: i64) -> Result<(), Error<S::Error>> {
    let old = manifest(s, place)?.unwrap_or_default();
    if let Some(p) = old.pending {
        drop_generation(s, place, p)?;
    }
    let generation = old.next_generation();
    write(s, place, &Manifest { value: old.value, pending: Some(generation) }, now)?;
    for (k, piece) in value.chunks(PIECE).enumerate() {
        s.put(&place.piece(generation, k as u64), piece).map_err(store)?;
    }
    let v = Value { length: value.len() as u64, piece: PIECE as u64, generation, hash: *blake3::hash(value).as_bytes() };
    // The switch. The old generation stays pending until it is dropped: a crash before the drop leaves it for the next put or remove.
    write(s, place, &Manifest { value: Some(v), pending: old.value.map(|o| o.generation) }, now)?;
    if let Some(o) = old.value {
        drop_generation(s, place, o.generation)?;
    }
    Ok(())
}

/// The value's length, without reading it; None when `place` holds no value in pieces.
pub fn len<S: Store>(s: &mut S, place: &Place) -> Result<Option<u64>, Error<S::Error>> {
    Ok(manifest(s, place)?.and_then(|m| m.value).map(|v| v.length))
}

/// The value read a piece at a time: `sink(offset, bytes)` for each piece in order as it opens, its buffer wiped after. The whole is held to the manifest's hash at the end: a sink that has already used the bytes must undo what it did when this is Corrupt. Returns the length, None when `place` holds no value in pieces.
pub fn read<S: Store>(s: &mut S, place: &Place, mut sink: impl FnMut(usize, &[u8])) -> Result<Option<u64>, Error<S::Error>> {
    let Some(v) = manifest(s, place)?.and_then(|m| m.value) else { return Ok(None) };
    let mut h = blake3::Hasher::new();
    let mut at = 0usize;
    for k in 0..v.count() {
        let Some(mut p) = s.get(&place.piece(v.generation, k)).map_err(store)? else {
            return Err(Error::Corrupt("a piece is missing"));
        };
        if p.len() as u64 != v.len_of(k) {
            p.fill(0);
            return Err(Error::Corrupt("a piece is not its length"));
        }
        h.update(&p);
        sink(at, &p);
        at += p.len();
        p.fill(0);
    }
    if h.finalize().as_bytes() != &v.hash {
        return Err(Error::Corrupt("the pieces are not the value the manifest names"));
    }
    Ok(Some(v.length))
}

/// The value read whole into one buffer of its length (one block of the heap, never two); None when `place` holds no value in pieces.
pub fn read_whole<S: Store>(s: &mut S, place: &Place) -> Result<Option<Vec<u8>>, Error<S::Error>> {
    let Some(n) = len(s, place)? else { return Ok(None) };
    let mut out = Vec::with_capacity(n as usize);
    match read(s, place, |_, b| out.extend_from_slice(b)) {
        Ok(_) => Ok(Some(out)),
        Err(e) => {
            out.fill(0);
            Err(e)
        }
    }
}

/// The value under `place` gone, its pieces and its manifest: first the manifest says it holds nothing and its generation is pending, then the pieces go, then the manifest. Whether there was a value.
pub fn remove<S: Store>(s: &mut S, place: &Place, now: i64) -> Result<bool, Error<S::Error>> {
    let Some(m) = manifest(s, place)? else { return Ok(false) };
    if let Some(v) = m.value {
        write(s, place, &Manifest { value: None, pending: Some(v.generation) }, now)?;
    }
    if let Some(p) = m.pending {
        drop_generation(s, place, p)?;
    }
    if let Some(v) = m.value {
        drop_generation(s, place, v.generation)?;
    }
    s.delete(&place.manifest()).map_err(store)?;
    Ok(m.value.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// The store in memory, by address; `fail_after` puts succeed and then every put fails (a crash, as the vault sees it: what landed stays).
    #[derive(Default)]
    struct Mem {
        map: BTreeMap<[u8; 32], Vec<u8>>,
        puts: usize,
        fail_after: Option<usize>,
    }

    impl Store for Mem {
        type Error = &'static str;
        fn get(&mut self, a: &[u8; 32]) -> Result<Option<Vec<u8>>, Self::Error> {
            Ok(self.map.get(a).cloned())
        }
        fn put(&mut self, a: &[u8; 32], v: &[u8]) -> Result<(), Self::Error> {
            if self.fail_after.is_some_and(|n| self.puts >= n) {
                return Err("crash");
            }
            self.puts += 1;
            self.map.insert(*a, v.to_vec());
            Ok(())
        }
        fn delete(&mut self, a: &[u8; 32]) -> Result<bool, Self::Error> {
            Ok(self.map.remove(a).is_some())
        }
    }

    const KEY: [u8; 32] = [7; 32];

    /// `n` bytes no two pieces of which are alike (BLAKE3's output stream from `seed`).
    fn value(n: usize, seed: u8) -> Vec<u8> {
        let mut v = vec![0u8; n];
        blake3::Hasher::new().update(&[seed]).finalize_xof().fill(&mut v);
        v
    }

    #[test]
    fn round_trip_at_every_cut() {
        let place = Place::new(&KEY, b"display");
        for n in [1, PIECE - 1, PIECE, PIECE + 1, 2_800_000, 6 * PIECE] {
            let mut s = Mem::default();
            let v = value(n, 3);
            put(&mut s, &place, &v, 0).unwrap();
            assert_eq!(len(&mut s, &place).unwrap(), Some(n as u64));
            assert_eq!(read_whole(&mut s, &place).unwrap().unwrap(), v);
            // The manifest and the pieces, nothing else.
            assert_eq!(s.map.len(), 1 + n.div_ceil(PIECE));
            // Read in order, each piece at most PIECE.
            let mut at = 0;
            read(&mut s, &place, |o, b| {
                assert_eq!(o, at);
                assert!(b.len() <= PIECE);
                at += b.len();
            })
            .unwrap();
            assert_eq!(at, n);
        }
    }

    #[test]
    fn replace_drops_the_old_generation() {
        let place = Place::new(&KEY, b"display");
        let mut s = Mem::default();
        put(&mut s, &place, &value(3 * PIECE, 1), 0).unwrap();
        put(&mut s, &place, &value(PIECE + 5, 2), 0).unwrap();
        assert_eq!(read_whole(&mut s, &place).unwrap().unwrap(), value(PIECE + 5, 2));
        assert_eq!(s.map.len(), 1 + 2, "the first value's three pieces are gone");
        let m = manifest(&mut s, &place).unwrap().unwrap();
        assert_eq!(m.value.unwrap().generation, 1);
        // Gone, every address.
        assert!(remove(&mut s, &place, 0).unwrap());
        assert!(s.map.is_empty());
        assert_eq!(len(&mut s, &place).unwrap(), None);
        assert!(!remove(&mut s, &place, 0).unwrap());
    }

    #[test]
    fn names_and_keys_keep_apart() {
        let mut s = Mem::default();
        let a = Place::new(&KEY, b"display");
        let b = Place::new(&KEY, b"display2");
        let c = Place::new(&[8; 32], b"display");
        put(&mut s, &a, &value(PIECE + 1, 1), 0).unwrap();
        put(&mut s, &b, &value(PIECE + 1, 2), 0).unwrap();
        put(&mut s, &c, &value(PIECE + 1, 3), 0).unwrap();
        assert_eq!(s.map.len(), 9);
        assert_eq!(read_whole(&mut s, &a).unwrap().unwrap(), value(PIECE + 1, 1));
        assert_eq!(read_whole(&mut s, &b).unwrap().unwrap(), value(PIECE + 1, 2));
        assert_eq!(read_whole(&mut s, &c).unwrap().unwrap(), value(PIECE + 1, 3));
        // No name's own address (a keyed hash under the store's key) is a manifest's.
        assert_ne!(a.manifest(), *blake3::keyed_hash(&KEY, b"display").as_bytes());
    }

    /// A put cut short at every one of its writes: the old value reads back whole, and the next put leaves nothing behind but its own manifest and pieces.
    #[test]
    fn a_crash_leaves_the_old_value_or_the_new() {
        let place = Place::new(&KEY, b"display");
        let (old, new) = (value(2 * PIECE + 9, 1), value(3 * PIECE, 2));
        // The writes of a put over a value: the pending manifest, three pieces, the switch.
        for crash_at in 0..5 {
            let mut s = Mem::default();
            put(&mut s, &place, &old, 0).unwrap();
            s.fail_after = Some(s.puts + crash_at);
            assert!(put(&mut s, &place, &new, 0).is_err());
            s.fail_after = None;
            assert_eq!(read_whole(&mut s, &place).unwrap().unwrap(), old, "crash at write {crash_at}");
            put(&mut s, &place, &value(PIECE, 3), 0).unwrap();
            assert_eq!(read_whole(&mut s, &place).unwrap().unwrap(), value(PIECE, 3));
            assert_eq!(s.map.len(), 2, "crash at write {crash_at}: orphans left");
        }
        // Cut short after the switch, before the old pieces went: the new value reads, and the old pieces go with the next write.
        let mut s = Mem::default();
        put(&mut s, &place, &old, 0).unwrap();
        put(&mut s, &place, &new, 0).unwrap();
        // Put the old generation's pieces back as a crash before the drop would have left them.
        for (k, p) in old.chunks(PIECE).enumerate() {
            s.map.insert(place.piece(0, k as u64), p.to_vec());
        }
        assert_eq!(read_whole(&mut s, &place).unwrap().unwrap(), new);
        assert!(remove(&mut s, &place, 0).unwrap());
        assert!(s.map.is_empty(), "the pending generation dropped with the value");
    }

    #[test]
    fn tampering_is_corrupt() {
        let place = Place::new(&KEY, b"display");
        let v = value(3 * PIECE, 1);
        // Two pieces swapped: each opens (the store's seal binds its address, which this store does not), the whole is not the value.
        let mut s = Mem::default();
        put(&mut s, &place, &v, 0).unwrap();
        let (p0, p1) = (place.piece(0, 0), place.piece(0, 1));
        let (a, b) = (s.map[&p0].clone(), s.map[&p1].clone());
        s.map.insert(p0, b);
        s.map.insert(p1, a);
        assert_eq!(read_whole(&mut s, &place), Err(Error::Corrupt("the pieces are not the value the manifest names")));
        // A piece gone.
        let mut s = Mem::default();
        put(&mut s, &place, &v, 0).unwrap();
        s.map.remove(&place.piece(0, 2));
        assert_eq!(read_whole(&mut s, &place), Err(Error::Corrupt("a piece is missing")));
        // A piece short.
        let mut s = Mem::default();
        put(&mut s, &place, &v, 0).unwrap();
        s.map.get_mut(&place.piece(0, 1)).unwrap().pop();
        assert_eq!(read_whole(&mut s, &place), Err(Error::Corrupt("a piece is not its length")));
        // The manifest not a manifest.
        let mut s = Mem::default();
        put(&mut s, &place, &v, 0).unwrap();
        s.map.insert(place.manifest(), b"not a manifest".to_vec());
        assert_eq!(len(&mut s, &place), Err(Error::Corrupt("the manifest does not read")));
    }

    #[test]
    fn manifest_round_trip() {
        for m in [
            Manifest::default(),
            Manifest { value: None, pending: Some(4) },
            Manifest { value: Some(Value { length: 2_800_000, piece: PIECE as u64, generation: 9, hash: [5; 32] }), pending: None },
            Manifest { value: Some(Value { length: 1, piece: 1, generation: 0, hash: [0; 32] }), pending: Some(0) },
        ] {
            assert_eq!(Manifest::decode(&m.encode(1 << 40).unwrap()).unwrap(), m);
        }
        let zero_cut = Manifest { value: Some(Value { length: 1, piece: 0, generation: 0, hash: [0; 32] }), pending: None };
        assert_eq!(Manifest::decode(&zero_cut.encode(0).unwrap()), Err("the manifest's cut is zero"));
    }
}
