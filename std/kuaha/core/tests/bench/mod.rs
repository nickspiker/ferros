//! The device's hold on the host, as ferros_items's own tests (tests/serve.rs) run it: the vault process's `Server` over a RAM vault and a RAM clips area with its extents' table, each caller's calls stamped as the kernel stamps them.
//! Lumis keeps things into it; Kūaha reaches it thru `kuaha_core::Over`, the same adapter the kist uses on the device over its syscall.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use ferros_items::kist::{Hold, Transport, ERR};
use ferros_items::serve::{Clips, Rec, Server};
use ferros_items::{kinds, Caller, Extent, Maker};
use ferros_store::{Domain, Nonces, Store};
use manifestus::{Block, BlockDev, DeletePolicy, Extents, Mirror, Vault};

pub struct Mem(Vec<Block>);
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

pub struct Counter(u64);
impl Nonces for Counter {
    fn nonce(&mut self) -> [u8; 24] {
        self.0 += 1;
        let mut n = [0u8; 24];
        n[..8].copy_from_slice(&self.0.to_le_bytes());
        n
    }
}

pub type S = Store<Mem, Mem, Counter>;

/// The clips area's blocks here (the device's is 12 << 20).
pub const AREA: u64 = 1 << 12;
/// The bench's clock: 2026-11-20, 09:30 UTC, in Eagle time (oscillations): seconds since the Eagle landed, times the hydrogen line's periods a second.
pub const T0: i64 = 1_809_349_940 * 1_420_407_826;
/// An Eagle second.
pub const SECOND: i64 = 1_420_407_826;

pub struct MemClips {
    table: Option<Extents>,
    area: Vec<u8>,
    pub grants: Vec<([u8; 32], [u8; 32], Extent)>,
}

impl MemClips {
    fn table(&mut self, s: &mut S) -> &mut Extents {
        if self.table.is_none() {
            self.table = Some(Extents::load(s.engine(), AREA).unwrap());
        }
        self.table.as_mut().unwrap()
    }
}

impl Clips<S> for MemClips {
    fn open(&mut self, s: &mut S, name: [u8; 32], owner: [u8; 32], blocks: u64, now: i64) -> Option<Extent> {
        let mut t = self.table(s).clone();
        let e = t.reserve(name, owner, blocks, now).ok()?;
        t.store(s.engine(), now).ok()?;
        self.table = Some(t);
        Some(Extent { first: e.first, blocks: e.blocks })
    }
    fn give(&mut self, name: &[u8; 32], x: Extent) -> Option<[u8; 32]> {
        let cap = *blake3::hash(name).as_bytes();
        self.grants.push((cap, *name, x));
        Some(cap)
    }
    fn take_back(&mut self) {
        self.grants.clear();
    }
    fn write(&mut self, at: u64, bytes: &[u8]) -> bool {
        let a = (at as usize) << 12;
        self.area[a..a + bytes.len()].copy_from_slice(bytes);
        true
    }
    fn read(&mut self, at: u64, buf: &mut [u8]) -> bool {
        let a = (at as usize) << 12;
        buf.copy_from_slice(&self.area[a..a + buf.len()]);
        true
    }
    fn seal(&mut self, s: &mut S, name: &[u8; 32], len: u64, hash: [u8; 32], now: i64) -> Option<Extent> {
        let mut t = self.table(s).clone();
        let e = t.seal(name, len, hash, now).ok()?;
        t.store(s.engine(), now).ok()?;
        self.table = Some(t);
        self.grants.retain(|g| g.1 != *name);
        Some(Extent { first: e.first, blocks: e.blocks })
    }
    fn remove(&mut self, s: &mut S, name: &[u8; 32], now: i64) -> Result<(), ()> {
        if self.grants.iter().any(|g| g.1 == *name) {
            return Err(());
        }
        let mut t = self.table(s).clone();
        if t.remove(name).is_some() {
            t.store(s.engine(), now).map_err(|_| ())?;
            self.table = Some(t);
        }
        Ok(())
    }
    fn record(&mut self, s: &mut S, name: &[u8; 32]) -> Option<Rec> {
        let e = *self.table(s).find(name)?;
        Some(Rec { extent: Extent { first: e.first, blocks: e.blocks }, bytes: e.bytes, hash: e.hash, sealed: e.sealed })
    }
    fn space(&mut self, s: &mut S) -> Option<ferros_items::wire::ClipsUse> {
        let t = self.table(s);
        Some(ferros_items::wire::ClipsUse { blocks: t.area, taken: t.taken(), largest_free: t.largest_free() })
    }
}

pub struct Bench {
    server: Server,
    store: S,
    d: Domain,
    pub clips: MemClips,
    /// Eagle time now; each call moves it a second, so items made one after another are a second apart.
    pub now: i64,
    random: u64,
}

impl Bench {
    pub fn new() -> Rc<RefCell<Bench>> {
        let m = Mirror::new(Mem(vec![[0u8; 4096]; 1 << 14]), Mem(vec![[0u8; 4096]; 1 << 14]));
        let store = Store::new(Vault::open(m, 8, 1, DeletePolicy::UnlinkOnly).unwrap(), Counter(0), || T0);
        let clips = MemClips { table: None, area: vec![0u8; (AREA as usize) << 12], grants: Vec::new() };
        Rc::new(RefCell::new(Bench { server: Server::new([42; 32]), store, d: Domain { addr: [7; 32], seal: [9; 32] }, clips, now: T0, random: 0 }))
    }
}

/// A caller: the kernel stamping its every call, the vault answering it.
pub struct Kist {
    pub b: Rc<RefCell<Bench>>,
    stamp: Vec<u8>,
}

impl Transport for Kist {
    fn call(&mut self, call: &[u8], reply: &mut [u8]) -> u64 {
        let mut b = self.b.borrow_mut();
        let b = &mut *b;
        b.now += SECOND;
        let mut rest = self.stamp.clone();
        rest.extend_from_slice(call);
        let mut n = b.random;
        let mut random = || {
            n += 1;
            *blake3::hash(&n.to_le_bytes()).as_bytes()
        };
        let a = b.server.serve(&mut b.store, &b.d, &rest, reply, b.now, &mut random, &mut b.clips);
        b.random = n;
        a.value
    }
    fn block_write(&mut self, cap: &[u8; 32], block: u64, bytes: &[u8]) -> u64 {
        let mut b = self.b.borrow_mut();
        let Some(&(_, _, x)) = b.clips.grants.iter().find(|g| g.0 == *cap) else { return ERR };
        if bytes.len() & 4095 != 0 || !x.holds(block, (bytes.len() >> 12) as u64) {
            return ERR;
        }
        b.clips.write(x.first + block, bytes);
        0
    }
}

/// A transport whose every call is refused: an empty slot, or a kernel from before the hold.
pub struct Refusing;
impl Transport for Refusing {
    fn call(&mut self, _call: &[u8], _reply: &mut [u8]) -> u64 {
        ERR
    }
}

pub fn clock() -> i64 {
    T0
}

pub fn lumis() -> Caller {
    Caller { maker: Some(Maker { developer: [1; 32], option: [2; 32], name: "Lumis".into() }), makes: vec![kinds::PHOTO, kinds::RAW_VIDEO, kinds::SCAN], ..Default::default() }
}

/// A list-keeping kist, for a second maker.
pub fn lists() -> Caller {
    Caller { maker: Some(Maker { developer: [5; 32], option: [6; 32], name: "Rārangi".into() }), makes: vec![kinds::LIST, kinds::CONTACT], ..Default::default() }
}

/// Kūaha as its capsule declares it: no kind it makes, every kind it shows opened.
pub fn kuaha() -> Caller {
    Caller { maker: Some(Maker { developer: [3; 32], option: [4; 32], name: "Kūaha".into() }), opens: kuaha_core::kinds::SHOWN.to_vec(), ..Default::default() }
}

pub fn hold(b: &Rc<RefCell<Bench>>, c: &Caller) -> Hold<Kist> {
    Hold::new(Kist { b: b.clone(), stamp: c.stamp(c.maker.is_some()) }, clock)
}

/// Kūaha's source over the bench, its right held.
pub fn source(b: &Rc<RefCell<Bench>>) -> kuaha_core::Over<Kist> {
    kuaha_core::Over::new(Kist { b: b.clone(), stamp: kuaha().stamp(true) }, clock, true)
}

pub fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}
