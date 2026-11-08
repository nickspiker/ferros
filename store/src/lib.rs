//! The vault's values of any size (docs/VAULT-OUT.md), over manifestus: what the vault process stores, now that the vault leaves the kernel.
//!
//! The kernel's vault sealed each value whole in its heap, and that capped a value at 3 MiB (owner, 2026-11-07: "a 3MB cap on files and having the kernel involved makes me nervous"). Here a value is a head and its pieces, each sealed on its own, so a value has no size limit beyond the disk and nothing of its size is ever in memory at once:
//! - The head: a VSF document (section `value`) holding the value's length and the id its pieces are addressed under, or, when the value fits one piece, the value itself (`data`).
//! - The pieces: PIECE bytes each (the last shorter), piece k under an address keyed from the domain's address key, the value's id and k.
//! - Every head and piece is sealed with XChaCha20-Poly1305 under the domain's sealing key, its own address as the associated data, so no sealed block can be moved to another name, value or position and still open.
//!
//! Keys come from outside: each call carries the domain's two keys, as the kernel issues them for the kist whose request it is (VAULT-OUT.md, decision 1). Nothing here derives a key from a root.
//!
//! A rewrite takes a fresh id, so its pieces never overwrite the live value's: the head moves to the new id in the same engine commit as the last piece, and only then are the old pieces removed. An interrupted write leaves the old value whole; a value large enough that its pieces reach the engine before its head is on the domain's pending list meanwhile, so `sweep` finds what such a write left behind.
//!
//! Each domain keeps its own books, as entries whose names begin with a zero byte: its list of names (`names`) and its pending list, each a VSF document with a repeated field.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use manifestus::{BlockDev, Vault};
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

/// A piece is 2^PIECE_LOG2 bytes (64 KiB): large enough that a 24 MB wallpaper is a few hundred pieces, small enough that opening one is a small allocation.
pub const PIECE_LOG2: u32 = 16;
pub const PIECE: usize = 1 << PIECE_LOG2;
const NONCE: usize = 24;
const TAG: usize = 16;
/// Pieces handed to the engine in one commit while a large value is written.
const BATCH: usize = 1 << 4;

/// One domain's keys, as the kernel issues them: the key its addresses are hashed under, and the key its values are sealed under.
pub struct Domain {
    pub addr: [u8; 32],
    pub seal: [u8; 32],
}

/// Fresh 24-byte nonces (the vault process draws them from its entropy grant).
pub trait Nonces {
    fn nonce(&mut self) -> [u8; NONCE];
}

#[derive(Debug)]
pub enum Error {
    /// The engine's own error.
    Engine(manifestus::Error),
    /// A sealed head or piece did not open under the domain's key: tampered, moved, or another domain's.
    Open,
    /// Sealing failed.
    Seal,
    /// A head opened but is not a value document, or a piece is missing.
    Format,
    /// An export would pass its bound (`Store::export`).
    TooLarge,
}

/// A person's export (`Store::export`): each value carried, under its full name, and each one left out for its size, with its length.
pub struct Export {
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
    pub left_out: Vec<(Vec<u8>, u64)>,
}

impl From<manifestus::Error> for Error {
    fn from(e: manifestus::Error) -> Error {
        Error::Engine(e)
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// A value's head as stored.
#[derive(Clone)]
struct Head {
    len: u64,
    id: [u8; 32],
    /// The value itself, when it fits one piece.
    inline: Option<Vec<u8>>,
}

const SECTION: &str = "value";

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION).field("length", TypeConstraint::AnyUnsigned).field("id", TypeConstraint::AnyHash).field("data", TypeConstraint::Wrapped(b'r'))
}

impl Head {
    fn encode(&self, now: i64) -> Result<Vec<u8>> {
        let mut b = schema().build();
        b = b.set("length", VsfType::u(self.len as usize, false)).map_err(|_| Error::Format)?;
        b = b.set("id", VsfType::hb(self.id.to_vec())).map_err(|_| Error::Format)?;
        if let Some(d) = &self.inline {
            b = b.set("data", VsfType::v(b'r', d.clone())).map_err(|_| Error::Format)?;
        }
        let section = b.encode().map_err(|_| Error::Format)?;
        vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed(SECTION, section).build().map_err(|_| Error::Format)
    }

    fn decode(doc: &[u8]) -> Result<Head> {
        let s = SectionBuilder::parse_document(schema(), doc, None).map_err(|_| Error::Format)?;
        let first = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let len = first("length").and_then(|v| v.as_u64()).ok_or(Error::Format)?;
        let id = match first("id") {
            Some(VsfType::hb(b)) if b.len() == 32 => {
                let mut id = [0u8; 32];
                id.copy_from_slice(&b);
                id
            }
            _ => return Err(Error::Format),
        };
        let inline = match first("data") {
            Some(VsfType::v(b'r', d)) => Some(d),
            _ => None,
        };
        Ok(Head { len, id, inline })
    }

    /// How many pieces the value is in (none when it is inline).
    fn pieces(&self) -> u64 {
        if self.inline.is_some() { 0 } else { self.len.div_ceil(PIECE as u64) }
    }
}

/// The vault's own entries in a domain: names beginning with a zero byte (a process's keys are printable ASCII, kete's address entries begin with a one byte, so none can be these). The domain's list of names, and the ids of values being written (`sweep`).
const NAMES: &[u8] = b"\0names";
const PENDING: &[u8] = b"\0pending";

fn names_schema() -> SectionSchema {
    SectionSchema::new("names").field("name", TypeConstraint::Wrapped(b'r'))
}

fn pending_schema() -> SectionSchema {
    SectionSchema::new("pending").field("id", TypeConstraint::AnyHash)
}

/// A value being written a piece at a time (`Store::begin`, `write`, `finish`).
pub struct Writer {
    /// Whether this write keeps the domain's books (its name in the list, its id in the pending list): every write a caller makes; not the books' own.
    books: bool,
    name: Vec<u8>,
    head_addr: [u8; 32],
    id: [u8; 32],
    len: u64,
    /// The piece being filled.
    piece: Vec<u8>,
    /// Sealed pieces not yet handed to the engine: (address, sealed bytes).
    sealed: Vec<([u8; 32], Vec<u8>)>,
    /// Pieces handed to the engine so far.
    written: u64,
}

/// The vault's values, over a manifestus vault.
pub struct Store<A: BlockDev, B: BlockDev, N: Nonces> {
    vault: Vault<A, B>,
    nonces: N,
    /// The time each commit is stamped with (Eagle time, oscillations).
    now: fn() -> i64,
    /// The last head and the last piece opened, by address and a hash of the sealing key that opened them (the address alone is not enough: a caller with the right address key and the wrong sealing key must not be served what it could not open, which the tests caught; a hash, so no key outlives the request it came with, VAULT-OUT.md): a large value is read a few KiB at a time (the kernel hands a process 4 KiB a call), and without these every read opened its head again and a 64 KiB piece sixteen times over. Dropped whenever a value is written or removed.
    last_head: Option<([u8; 32], [u8; 32], Head)>,
    last_piece: Option<([u8; 32], [u8; 32], Vec<u8>)>,
}

impl<A: BlockDev, B: BlockDev, N: Nonces> Store<A, B, N> {
    pub fn new(vault: Vault<A, B>, nonces: N, now: fn() -> i64) -> Self {
        Store { vault, nonces, now, last_head: None, last_piece: None }
    }

    /// The engine underneath (its stats, its commit).
    pub fn engine(&mut self) -> &mut Vault<A, B> {
        &mut self.vault
    }

    fn head_addr(d: &Domain, name: &[u8]) -> [u8; 32] {
        let mut h = blake3::Hasher::new_keyed(&d.addr);
        h.update(b"head\0");
        h.update(name);
        *h.finalize().as_bytes()
    }

    fn piece_addr(d: &Domain, id: &[u8; 32], k: u64) -> [u8; 32] {
        let mut h = blake3::Hasher::new_keyed(&d.addr);
        h.update(b"piece\0");
        h.update(id);
        h.update(&k.to_le_bytes());
        *h.finalize().as_bytes()
    }

    /// Sealed: the nonce, the bytes encrypted where they lie, the tag; the address bound in as associated data.
    fn seal(&mut self, d: &Domain, at: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
        let n = self.nonces.nonce();
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&d.seal));
        let mut out = Vec::with_capacity(NONCE + plain.len() + TAG);
        out.extend_from_slice(&n);
        out.extend_from_slice(plain);
        let tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&n), at, &mut out[NONCE..]).map_err(|_| Error::Seal)?;
        out.extend_from_slice(&tag);
        Ok(out)
    }

    fn open(d: &Domain, at: &[u8; 32], mut sealed: Vec<u8>) -> Result<Vec<u8>> {
        if sealed.len() < NONCE + TAG {
            return Err(Error::Open);
        }
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&d.seal));
        let mut n = [0u8; NONCE];
        n.copy_from_slice(&sealed[..NONCE]);
        sealed.drain(..NONCE);
        cipher.decrypt_in_place(XNonce::from_slice(&n), at, &mut sealed).map_err(|_| Error::Open)?;
        Ok(sealed)
    }

    fn head(&mut self, d: &Domain, name: &[u8]) -> Result<Option<([u8; 32], Head)>> {
        let at = Self::head_addr(d, name);
        if let Some((a, k, h)) = &self.last_head {
            if *a == at && *k == Self::key_id(d) {
                return Ok(Some((at, h.clone())));
            }
        }
        let Some(sealed) = self.vault.get(&at)? else { return Ok(None) };
        let h = Head::decode(&Self::open(d, &at, sealed)?)?;
        self.last_head = Some((at, Self::key_id(d), h.clone()));
        Ok(Some((at, h)))
    }

    /// Piece `k` of value `id`, opened (the last one opened is kept).
    fn piece(&mut self, d: &Domain, id: &[u8; 32], k: u64) -> Result<&[u8]> {
        let addr = Self::piece_addr(d, id, k);
        let id_k = Self::key_id(d);
        if !matches!(&self.last_piece, Some((a, k, _)) if *a == addr && *k == id_k) {
            let sealed = self.vault.get(&addr)?.ok_or(Error::Format)?;
            self.last_piece = Some((addr, id_k, Self::open(d, &addr, sealed)?));
        }
        Ok(&self.last_piece.as_ref().ok_or(Error::Format)?.2)
    }

    /// What the cache knows a sealing key by.
    fn key_id(d: &Domain) -> [u8; 32] {
        *blake3::hash(&d.seal).as_bytes()
    }

    /// Forget the kept head and piece: a value changed, or (the vault process, at a lock) the plaintext they hold must go.
    pub fn forget(&mut self) {
        self.last_head = None;
        self.last_piece = None;
    }

    /// A fresh value id: BLAKE3 of a fresh nonce, so it is a hash in fact as well as in type.
    fn fresh_id(&mut self) -> [u8; 32] {
        *blake3::hash(&self.nonces.nonce()).as_bytes()
    }

    /// The value's length, or None if it is absent.
    pub fn len(&mut self, d: &Domain, name: &[u8]) -> Result<Option<u64>> {
        Ok(self.head(d, name)?.map(|(_, h)| h.len))
    }

    /// Bytes of the value from `offset` into `out` (as many as fit and the value holds). Returns the value's whole length, or None if it is absent; only the pieces the range touches are read and opened.
    pub fn read(&mut self, d: &Domain, name: &[u8], offset: u64, out: &mut [u8]) -> Result<Option<u64>> {
        let Some((_, h)) = self.head(d, name)? else { return Ok(None) };
        if offset >= h.len || out.is_empty() {
            return Ok(Some(h.len));
        }
        let end = (offset + out.len() as u64).min(h.len);
        if let Some(data) = &h.inline {
            out[..(end - offset) as usize].copy_from_slice(&data[offset as usize..end as usize]);
            return Ok(Some(h.len));
        }
        let mut at = offset;
        while at < end {
            let k = at >> PIECE_LOG2;
            let piece = self.piece(d, &h.id, k)?;
            let p0 = k << PIECE_LOG2;
            let (from, to) = ((at - p0) as usize, ((end - p0) as usize).min(piece.len()));
            if from >= to {
                // The piece is shorter than the head says.
                return Err(Error::Format);
            }
            let dst = (at - offset) as usize;
            out[dst..dst + to - from].copy_from_slice(&piece[from..to]);
            at = p0 + to as u64;
        }
        Ok(Some(h.len))
    }

    /// The whole value (for small ones: a large value is read in ranges, `read`).
    pub fn get(&mut self, d: &Domain, name: &[u8]) -> Result<Option<Vec<u8>>> {
        let Some(len) = self.len(d, name)? else { return Ok(None) };
        let mut v = alloc::vec![0u8; len as usize];
        self.read(d, name, 0, &mut v)?;
        Ok(Some(v))
    }

    /// Start writing `name` afresh (its current value stays until `finish`).
    pub fn begin(&mut self, d: &Domain, name: &[u8]) -> Writer {
        self.writer(d, name, true)
    }

    fn writer(&mut self, d: &Domain, name: &[u8], books: bool) -> Writer {
        Writer { head_addr: Self::head_addr(d, name), id: self.fresh_id(), len: 0, piece: Vec::with_capacity(PIECE), sealed: Vec::new(), written: 0, books, name: name.to_vec() }
    }

    /// The next bytes of the value being written.
    pub fn write(&mut self, d: &Domain, w: &mut Writer, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            let take = (PIECE - w.piece.len()).min(bytes.len());
            w.piece.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            w.len += take as u64;
            if w.piece.len() == PIECE {
                self.seal_piece(d, w)?;
                if w.sealed.len() == BATCH {
                    self.flush_pieces(d, w)?;
                }
            }
        }
        Ok(())
    }

    fn seal_piece(&mut self, d: &Domain, w: &mut Writer) -> Result<()> {
        let k = w.written + w.sealed.len() as u64;
        let addr = Self::piece_addr(d, &w.id, k);
        let sealed = self.seal(d, &addr, &w.piece)?;
        w.sealed.push((addr, sealed));
        w.piece.clear();
        Ok(())
    }

    /// Hand the sealed pieces to the engine ahead of the head. The first time a value does, its id goes on the domain's pending list, so pieces stranded by a write that never finishes can be found again (`sweep`); a value whose pieces all go with its head never needs it.
    fn flush_pieces(&mut self, d: &Domain, w: &mut Writer) -> Result<()> {
        if w.written == 0 && w.books {
            self.edit_list(d, PENDING, pending_schema, "id", |l| l.push(VsfType::hb(w.id.to_vec())))?;
        }
        let items: Vec<(&[u8; 32], &[u8])> = w.sealed.iter().map(|(a, s)| (a, s.as_slice())).collect();
        self.vault.put_batch(&items, (self.now)())?;
        w.written += w.sealed.len() as u64;
        w.sealed.clear();
        Ok(())
    }

    /// Finish the value: its last piece and its new head in one engine commit (the switch from the old value to the new is that commit), then the old value's pieces removed, and the books: a new name joins the domain's list, the id leaves the pending list.
    pub fn finish(&mut self, d: &Domain, mut w: Writer) -> Result<()> {
        self.forget();
        let old = self.vault.get(&w.head_addr)?.map(|s| Self::open(d, &w.head_addr, s).and_then(|p| Head::decode(&p))).transpose()?;
        let pending = w.written > 0;
        // A value that fits one piece and was never split goes inline in its head: one engine entry.
        let head = if w.written == 0 && w.sealed.is_empty() {
            Head { len: w.len, id: w.id, inline: Some(core::mem::take(&mut w.piece)) }
        } else {
            if !w.piece.is_empty() {
                self.seal_piece(d, &mut w)?;
            }
            Head { len: w.len, id: w.id, inline: None }
        };
        let doc = head.encode((self.now)())?;
        let sealed_head = self.seal(d, &w.head_addr, &doc)?;
        let mut items: Vec<(&[u8; 32], &[u8])> = w.sealed.iter().map(|(a, s)| (a, s.as_slice())).collect();
        items.push((&w.head_addr, &sealed_head));
        self.vault.put_batch(&items, (self.now)())?;
        if let Some(old) = &old {
            self.drop_pieces(d, old)?;
        }
        if w.books {
            if old.is_none() {
                let name = w.name.clone();
                self.edit_list(d, NAMES, names_schema, "name", |l| l.push(VsfType::v(b'r', name)))?;
            }
            if pending {
                let id = w.id.to_vec();
                self.edit_list(d, PENDING, pending_schema, "id", |l| l.retain(|v| !matches!(v, VsfType::hb(b) if *b == id)))?;
            }
        }
        Ok(())
    }

    /// The whole value at once (`begin`, `write`, `finish`).
    pub fn put(&mut self, d: &Domain, name: &[u8], value: &[u8]) -> Result<()> {
        let mut w = self.begin(d, name);
        self.write(d, &mut w, value)?;
        self.finish(d, w)
    }

    fn drop_pieces(&mut self, d: &Domain, h: &Head) -> Result<()> {
        let now = (self.now)();
        for k in 0..h.pieces() {
            self.vault.delete(&Self::piece_addr(d, &h.id, k), now)?;
        }
        Ok(())
    }

    /// Remove the value: its head (so it is gone at once), then its pieces, then its name from the domain's list. Returns whether it was there.
    pub fn delete(&mut self, d: &Domain, name: &[u8]) -> Result<bool> {
        let Some((at, h)) = self.head(d, name)? else { return Ok(false) };
        self.vault.delete(&at, (self.now)())?;
        self.drop_pieces(d, &h)?;
        // After: looking the head up kept it.
        self.forget();
        let name = name.to_vec();
        self.edit_list(d, NAMES, names_schema, "name", |l| l.retain(|v| !matches!(v, VsfType::v(b'r', n) if *n == name)))?;
        Ok(true)
    }

    /// The domain's names, as callers wrote them (the vault's own entries are not among them).
    pub fn names(&mut self, d: &Domain) -> Result<Vec<Vec<u8>>> {
        Ok(self.list(d, NAMES, names_schema, "name")?.into_iter().filter_map(|v| if let VsfType::v(b'r', n) = v { Some(n) } else { None }).collect())
    }

    /// Every named value of each domain with its name made whole (the domain's scope prefix first), in the domains' order: what a person's export carries (`SYS_UNLOCK` op 5, VAULT-OUT.md). Address-named values (kete's raw addresses) are not names a person reads and are left out, as the kernel's export left them. The export is read whole into memory, so it is bounded: a value longer than `value_max` is left out and named in `Export::left_out` with its length, and when what remains passes `total_max` nothing is read and the export is `TooLarge`; the lengths are known from the heads before any value is read.
    pub fn export(&mut self, parts: &[(&[u8], &Domain)], value_max: u64, total_max: u64) -> Result<Export> {
        let mut take = Vec::new();
        let mut left_out = Vec::new();
        let mut total = 0u64;
        for (k, (prefix, d)) in parts.iter().enumerate() {
            for name in self.names(d)? {
                if name.first() == Some(&1) {
                    continue;
                }
                let Some(n) = self.len(d, &name)? else { continue };
                let mut full = prefix.to_vec();
                full.extend_from_slice(&name);
                if n > value_max {
                    left_out.push((full, n));
                } else {
                    total += n;
                    take.push((k, name, full));
                }
            }
        }
        if total > total_max {
            return Err(Error::TooLarge);
        }
        let mut entries = Vec::with_capacity(take.len());
        for (k, name, full) in take {
            if let Some(v) = self.get(parts[k].1, &name)? {
                entries.push((full, v));
            }
        }
        Ok(Export { entries, left_out })
    }

    /// Every value of the domain deleted and the pieces of its unfinished writes swept, its books going with them (an empty list is removed): nothing of the domain is left on the disk. Release, when a person leaves the device (VAULT-OUT.md). Returns how many values went.
    pub fn drop_domain(&mut self, d: &Domain) -> Result<u64> {
        let mut n = 0;
        for name in self.names(d)? {
            if self.delete(d, &name)? {
                n += 1;
            }
        }
        self.sweep(d)?;
        self.forget();
        Ok(n)
    }

    /// The pieces of every value whose write never finished in this domain, removed: each pending id's pieces from the first until one is not there (a value's pieces go to the engine in order). Run when the domain's keys are at hand (the vault process does it at a domain's first request after it starts). Returns how many pieces it removed.
    pub fn sweep(&mut self, d: &Domain) -> Result<u64> {
        let ids: Vec<[u8; 32]> = self.list(d, PENDING, pending_schema, "id")?.into_iter().filter_map(|v| if let VsfType::hb(b) = v { b.try_into().ok() } else { None }).collect();
        let now = (self.now)();
        let mut n = 0;
        for id in &ids {
            let mut k = 0;
            while self.vault.delete(&Self::piece_addr(d, id, k), now)? {
                k += 1;
            }
            n += k;
        }
        if !ids.is_empty() {
            self.edit_list(d, PENDING, pending_schema, "id", |l| l.clear())?;
        }
        Ok(n)
    }

    /// One of the domain's own lists: the values of its repeated field `field`, none when it is absent.
    fn list(&mut self, d: &Domain, which: &[u8], schema: fn() -> SectionSchema, field: &str) -> Result<Vec<VsfType>> {
        let Some(doc) = self.get(d, which)? else { return Ok(Vec::new()) };
        let s = SectionBuilder::parse_document(schema(), &doc, None).map_err(|_| Error::Format)?;
        Ok(s.get_fields(field).iter().flat_map(|f| f.values.iter().cloned()).collect())
    }

    /// Change one of the domain's own lists and write it again (an empty list is removed). Its own write keeps no books.
    fn edit_list(&mut self, d: &Domain, which: &[u8], schema: fn() -> SectionSchema, field: &str, f: impl FnOnce(&mut Vec<VsfType>)) -> Result<()> {
        let mut l = self.list(d, which, schema, field)?;
        f(&mut l);
        if l.is_empty() {
            if let Some((at, h)) = self.head(d, which)? {
                self.vault.delete(&at, (self.now)())?;
                self.drop_pieces(d, &h)?;
                self.forget();
            }
            return Ok(());
        }
        let mut b = schema().build();
        for v in l {
            b = b.append_multi(field, alloc::vec![v]).map_err(|_| Error::Format)?;
        }
        let section = b.encode().map_err(|_| Error::Format)?;
        let doc = vsf::VsfBuilder::new().creation_time_oscillations((self.now)()).provenance_only().add_unboxed(schema().name.clone(), section).build().map_err(|_| Error::Format)?;
        let mut w = self.writer(d, which, false);
        self.write(d, &mut w, &doc)?;
        self.finish(d, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifestus::{Block, DeletePolicy, Mirror};

    struct Mem(Vec<Block>);
    impl BlockDev for Mem {
        fn block_count(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, lba: u64, buf: &mut Block) -> manifestus::Result<()> {
            *buf = self.0[lba as usize];
            Ok(())
        }
        fn write(&mut self, lba: u64, buf: &Block) -> manifestus::Result<()> {
            self.0[lba as usize] = *buf;
            Ok(())
        }
        fn flush(&mut self) -> manifestus::Result<()> {
            Ok(())
        }
    }

    struct Counter(u64);
    impl Nonces for Counter {
        fn nonce(&mut self) -> [u8; NONCE] {
            self.0 += 1;
            let mut n = [0u8; NONCE];
            n[..8].copy_from_slice(&self.0.to_le_bytes());
            n
        }
    }

    fn now() -> i64 {
        1
    }

    /// A store over two RAM mirrors of `blocks` 4 KiB blocks.
    fn store(blocks: usize) -> Store<Mem, Mem, Counter> {
        let m = Mirror::new(Mem(vec![[0u8; 4096]; blocks]), Mem(vec![[0u8; 4096]; blocks]));
        let v = Vault::open(m, 8, 1, DeletePolicy::UnlinkOnly).expect("vault");
        Store::new(v, Counter(0), now)
    }

    fn domain(k: u8) -> Domain {
        Domain { addr: [k; 32], seal: [k ^ 0x5A; 32] }
    }

    fn bytes(n: usize, seed: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u32).wrapping_mul(2654435761).wrapping_add(seed as u32).to_le_bytes()[1]).collect()
    }

    /// Values of every awkward size round-trip whole, and in ranges across piece boundaries.
    #[test]
    fn sizes_and_ranges() {
        let mut s = store(1 << 14);
        let d = domain(1);
        for (i, n) in [0usize, 1, PIECE - 1, PIECE, PIECE + 1, 3 * PIECE + 77].into_iter().enumerate() {
            let name = [b'v', i as u8];
            let v = bytes(n, i as u8);
            s.put(&d, &name, &v).unwrap();
            assert_eq!(s.len(&d, &name).unwrap(), Some(n as u64));
            assert_eq!(s.get(&d, &name).unwrap().unwrap(), v, "size {n}");
            if n > 10 {
                for (off, len) in [(0, 10), (n - 10, 10), (n / 2, n / 3), (PIECE.min(n) - 3, 7)] {
                    let mut out = vec![0u8; len];
                    s.read(&d, &name, off as u64, &mut out).unwrap();
                    let end = (off + len).min(n);
                    assert_eq!(out[..end - off], v[off..end], "size {n} at {off}+{len}");
                }
            }
        }
    }

    /// A rewrite replaces the value and drops the old pieces; a delete removes it; another domain's keys find nothing and cannot open what they do find.
    #[test]
    fn rewrite_delete_domains() {
        let mut s = store(1 << 14);
        let (d, e) = (domain(1), domain(2));
        let big = bytes(5 * PIECE, 3);
        s.put(&d, b"x", &big).unwrap();
        let live = s.engine().live_keys().unwrap().len();
        let small = bytes(100, 4);
        s.put(&d, b"x", &small).unwrap();
        assert_eq!(s.get(&d, b"x").unwrap().unwrap(), small);
        // The five pieces gone, the head replaced: one entry left of six.
        assert_eq!(s.engine().live_keys().unwrap().len(), live - 5);
        assert!(s.get(&e, b"x").unwrap().is_none());
        // The same name under another domain's address key, but sealed with this one's: does not open.
        let mut wrong = domain(2);
        wrong.addr = d.addr;
        assert!(matches!(s.get(&wrong, b"x"), Err(Error::Open)));
        assert!(s.delete(&d, b"x").unwrap());
        assert!(s.get(&d, b"x").unwrap().is_none());
        assert!(!s.delete(&d, b"x").unwrap());
    }

    /// Read 4 KiB at a time, as a process reads a large value through the kernel, a value comes back whole; and with its head and a piece kept from those reads, the right address key with the wrong sealing key is still refused.
    #[test]
    fn cached_reads() {
        let mut s = store(1 << 14);
        let d = domain(5);
        let v = bytes(3 * PIECE + 999, 6);
        s.put(&d, b"big", &v).unwrap();
        let mut back = vec![0u8; v.len()];
        for off in (0..v.len()).step_by(4096) {
            let end = (off + 4096).min(v.len());
            assert_eq!(s.read(&d, b"big", off as u64, &mut back[off..end]).unwrap(), Some(v.len() as u64));
        }
        assert_eq!(back, v);
        let mut wrong = domain(9);
        wrong.addr = d.addr;
        assert!(matches!(s.read(&wrong, b"big", 0, &mut [0u8; 16]), Err(Error::Open)));
        assert!(matches!(s.read(&wrong, b"big", (3 * PIECE) as u64, &mut [0u8; 16]), Err(Error::Open)));
    }

    /// The domain's names follow its puts and deletes (a rewrite adds nothing), and never show the vault's own entries.
    #[test]
    fn names_follow() {
        let mut s = store(1 << 14);
        let d = domain(3);
        s.put(&d, b"a", b"1").unwrap();
        s.put(&d, b"b", &bytes(2 * PIECE, 1)).unwrap();
        s.put(&d, b"a", b"2").unwrap();
        let mut n = s.names(&d).unwrap();
        n.sort();
        assert_eq!(n, vec![b"a".to_vec(), b"b".to_vec()]);
        s.delete(&d, b"a").unwrap();
        assert_eq!(s.names(&d).unwrap(), vec![b"b".to_vec()]);
        assert!(s.names(&domain(4)).unwrap().is_empty());
    }

    /// A large write abandoned before its head leaves pieces only `sweep` finds; it removes exactly those, and the old value under the name stays whole. A finished large write leaves nothing to sweep.
    #[test]
    fn sweep_strays() {
        let mut s = store(1 << 15);
        let d = domain(8);
        let old = bytes(100, 2);
        s.put(&d, b"w", &old).unwrap();
        let before = s.engine().live_keys().unwrap().len();
        let mut w = s.begin(&d, b"w");
        s.write(&d, &mut w, &bytes(3 * BATCH * PIECE, 5)).unwrap();
        drop(w);
        assert_eq!(s.get(&d, b"w").unwrap().unwrap(), old);
        assert_eq!(s.sweep(&d).unwrap(), (3 * BATCH) as u64);
        assert_eq!(s.engine().live_keys().unwrap().len(), before);
        s.put(&d, b"x", &bytes(2 * BATCH * PIECE + 5, 6)).unwrap();
        assert_eq!(s.sweep(&d).unwrap(), 0);
    }

    /// Dropping a domain leaves nothing of it on the disk (its values, an abandoned large write's pieces, its books) and touches no other domain.
    #[test]
    fn drop_leaves_nothing() {
        let mut s = store(1 << 15);
        let (gone, kept) = (domain(5), domain(6));
        let keep = bytes(PIECE + 3, 4);
        s.put(&kept, b"keep", &keep).unwrap();
        let before = s.engine().live_keys().unwrap().len();
        s.put(&gone, b"small", b"x").unwrap();
        s.put(&gone, b"big", &bytes(3 * PIECE + 1, 7)).unwrap();
        let mut w = s.begin(&gone, b"cut");
        s.write(&gone, &mut w, &bytes(2 * BATCH * PIECE, 8)).unwrap();
        drop(w);
        assert!(s.engine().live_keys().unwrap().len() > before);
        assert_eq!(s.drop_domain(&gone).unwrap(), 2);
        assert!(s.names(&gone).unwrap().is_empty());
        assert_eq!(s.get(&gone, b"big").unwrap(), None);
        assert_eq!(s.engine().live_keys().unwrap().len(), before);
        assert_eq!(s.get(&kept, b"keep").unwrap().unwrap(), keep);
        assert_eq!(s.drop_domain(&gone).unwrap(), 0);
    }

    /// An export carries every named value of each domain given, whole and under its full name, and nothing of a domain not given or of another key.
    #[test]
    fn export_whole() {
        let mut s = store(1 << 15);
        let (map, wifi, other) = (domain(9), domain(10), domain(11));
        let big = bytes(2 * PIECE + 9, 3);
        s.put(&map, b"places", &big).unwrap();
        s.put(&map, b"theme", b"dark").unwrap();
        s.put(&wifi, b"cred", b"secret").unwrap();
        s.put(&other, b"x", b"y").unwrap();
        let mut addr = alloc::vec![1u8];
        addr.extend_from_slice(&[7u8; 32]);
        s.put(&map, &addr, b"kete").unwrap();
        let parts: [(&[u8], &Domain); 2] = [(b"map.", &map), (b"wifi.", &wifi)];
        let x = s.export(&parts, 1 << 20, 1 << 22).unwrap();
        let mut e = x.entries;
        e.sort();
        assert_eq!(e, vec![(b"map.places".to_vec(), big.clone()), (b"map.theme".to_vec(), b"dark".to_vec()), (b"wifi.cred".to_vec(), b"secret".to_vec())]);
        assert!(x.left_out.is_empty());
        assert!(s.export(&[], 1, 1).unwrap().entries.is_empty());
        // A value past the value bound is named, not carried; the rest still is.
        let x = s.export(&parts, PIECE as u64, 1 << 22).unwrap();
        assert_eq!(x.left_out, vec![(b"map.places".to_vec(), big.len() as u64)]);
        assert_eq!(x.entries.len(), 2);
        // Past the total bound, nothing is read.
        assert!(matches!(s.export(&parts, 1 << 20, 8), Err(Error::TooLarge)));
    }

    /// A value written in uneven slices is the value written whole.
    #[test]
    fn streaming() {
        let mut s = store(1 << 14);
        let d = domain(7);
        let v = bytes(2 * PIECE + 12345, 9);
        let mut w = s.begin(&d, b"stream");
        for chunk in v.chunks(4096 + 17) {
            s.write(&d, &mut w, chunk).unwrap();
        }
        s.finish(&d, w).unwrap();
        assert_eq!(s.get(&d, b"stream").unwrap().unwrap(), v);
    }
}
