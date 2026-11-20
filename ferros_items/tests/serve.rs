//! The whole call path on the host: a kist's `kist::Hold` calls, the kernel's stamp, the vault's `serve::Server`, a RAM vault and a RAM clips area with its extents' table (manifestus `Extents`), as the device runs them.
//! Lumis keeps a still and records a clip; Kūaha finds them, reads them by range and removes them; the rights are the stamps'.

use std::cell::RefCell;
use std::rc::Rc;

use ferros_items::kist::{Hold, Transport, ERR};
use ferros_items::serve::{clip_name, Clips, Rec, Server, MAKINGS_MOST};
use ferros_items::{kinds, wire, Caller, Error, Extent, Find, Maker, Proposed, IS_EQUAL};
use ferros_store::{Domain, Nonces, Store};
use manifestus::{Block, BlockDev, DeletePolicy, Extents, Mirror, Vault};
use vsf::VsfType;

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
    fn nonce(&mut self) -> [u8; 24] {
        self.0 += 1;
        let mut n = [0u8; 24];
        n[..8].copy_from_slice(&self.0.to_le_bytes());
        n
    }
}

type S = Store<Mem, Mem, Counter>;

/// The clips area's blocks in these tests (the device's is 12 << 20).
const AREA: u64 = 1 << 12;
const T0: i64 = 1 << 60;

/// The vault's clips area and table over RAM, as payloads/vault `ItemClips` uses the clips window: a given extent is written by its holder alone (`grants`), taken back at its seal.
struct MemClips {
    table: Option<Extents>,
    area: Vec<u8>,
    /// Whether `give` hands the caller its extent (a kist on the device) or not (the bench, a kernel from before).
    direct: bool,
    /// Extents given: (capability, name, extent).
    grants: Vec<([u8; 32], [u8; 32], Extent)>,
}

impl MemClips {
    fn table(&mut self, s: &mut S) -> &mut Extents {
        if self.table.is_none() {
            self.table = Some(Extents::load(s.engine(), AREA).unwrap());
        }
        self.table.as_mut().unwrap()
    }
    /// The vault's recoverer closing a clip whose writer is gone: sealed in the table, as `recover_seal` does.
    fn recover(&mut self, s: &mut S, name: &[u8; 32], len: u64, hash: [u8; 32]) {
        self.grants.retain(|g| g.1 != *name);
        let t = self.table(s);
        t.seal(name, len, hash, T0).unwrap();
        let t = t.clone();
        t.store(s.engine(), T0).unwrap();
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
        if !self.direct {
            return None;
        }
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
    fn space(&mut self, s: &mut S) -> Option<wire::ClipsUse> {
        let t = self.table(s);
        Some(wire::ClipsUse { blocks: t.area, taken: t.taken(), largest_free: t.largest_free() })
    }
}

/// The device in one place: the vault process's server, its store and clips, the person's items domain, the clock and the entropy.
struct Bench {
    server: Server,
    store: S,
    d: Domain,
    clips: MemClips,
    now: i64,
    random: u64,
}

impl Bench {
    fn new(direct: bool) -> Rc<RefCell<Bench>> {
        let m = Mirror::new(Mem(vec![[0u8; 4096]; 1 << 14]), Mem(vec![[0u8; 4096]; 1 << 14]));
        let store = Store::new(Vault::open(m, 8, 1, DeletePolicy::UnlinkOnly).unwrap(), Counter(0), || T0);
        let clips = MemClips { table: None, area: vec![0u8; (AREA as usize) << 12], direct, grants: Vec::new() };
        Rc::new(RefCell::new(Bench { server: Server::new([42; 32]), store, d: Domain { addr: [7; 32], seal: [9; 32] }, clips, now: T0, random: 0 }))
    }
}

/// A caller's side: the kernel stamping its every call, the vault answering it.
struct Kist {
    b: Rc<RefCell<Bench>>,
    stamp: Vec<u8>,
}

impl Transport for Kist {
    fn call(&mut self, call: &[u8], reply: &mut [u8]) -> u64 {
        let mut b = self.b.borrow_mut();
        let b = &mut *b;
        b.now += 1;
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
    /// SYS_BLOCK on a given extent: its holder writes whole blocks inside it, nothing else.
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

fn clock() -> i64 {
    T0
}

fn lumis() -> Caller {
    Caller { maker: Some(Maker { developer: [1; 32], option: [2; 32], name: "Lumis".into() }), makes: vec![kinds::PHOTO, kinds::RAW_VIDEO], ..Default::default() }
}

fn kuaha() -> Caller {
    Caller { maker: Some(Maker { developer: [3; 32], option: [4; 32], name: "Kūaha".into() }), opens: vec![kinds::PHOTO, kinds::RAW_VIDEO, kinds::LIST, kinds::SCAN, kinds::CONTACT], ..Default::default() }
}

fn hold(b: &Rc<RefCell<Bench>>, c: &Caller) -> Hold<Kist> {
    Hold::new(Kist { b: b.clone(), stamp: c.stamp(c.maker.is_some()) }, clock)
}

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 12) as u8)).collect()
}

#[test]
fn lumis_keeps_a_still_and_kuaha_finds_reads_and_removes_it() {
    let b = Bench::new(true);
    let (mut l, mut k) = (hold(&b, &lumis()), hold(&b, &kuaha()));
    let still = bytes(3 << 20, 5);
    let p = Proposed { kind: kinds::PHOTO, title: Some("Kids in the river".into()), attrs: vec![("lens".into(), VsfType::u3(24))], ..Default::default() };
    let (lh, card) = l.keep(&p, &still).unwrap();
    assert_eq!((card.size, card.body, card.kist.name.as_str()), (still.len() as u64, *blake3::hash(&still).as_bytes(), "Lumis"));
    // Kūaha finds it by kind and attribute, with its own handle.
    let f = Find { kinds: vec![kinds::PHOTO], wheres: vec![("lens".into(), IS_EQUAL, VsfType::u3(24))], ..Default::default() };
    let found = k.find(&f).unwrap();
    assert_eq!(found.total, 1);
    let (kh, kc) = found.items[0].clone();
    assert_ne!(kh, lh);
    assert_eq!(kc.title.as_deref(), Some("Kids in the river"));
    // Read by ranges: across a transfer's end, and the tail.
    let mut out = vec![0u8; (1 << 20) + 777];
    assert_eq!(k.read(&kh, (1 << 20) - 333, &mut out).unwrap(), (out.len(), still.len() as u64));
    assert_eq!(&out[..], &still[(1 << 20) - 333..(2 << 20) + 444]);
    let mut tail = vec![0u8; 1000];
    assert_eq!(k.read(&kh, still.len() as u64 - 10, &mut tail).unwrap(), (10, still.len() as u64));
    assert_eq!(&tail[..10], &still[still.len() - 10..]);
    // A handle is its holder's alone.
    assert!(matches!(k.card(&lh), Err(Error::Refused)));
    // Kūaha makes nothing.
    assert!(matches!(k.keep(&p, b"x"), Err(Error::Refused)));
    // Removed by Kūaha: gone at once, for Lumis too; nothing of it left listed.
    k.remove(&kh).unwrap();
    assert!(l.card(&lh).is_err());
    assert_eq!(l.find(&Find::default()).unwrap().total, 0);
    let mut bb = b.borrow_mut();
    let bb = &mut *bb;
    assert!(bb.store.names(&bb.d).unwrap().is_empty());
}

#[test]
fn a_clip_recorded_straight_to_its_extent() {
    let b = Bench::new(true);
    let (mut l, mut k) = (hold(&b, &lumis()), hold(&b, &kuaha()));
    let p = Proposed { kind: kinds::RAW_VIDEO, title: Some("Bench, 30 a second".into()), attrs: vec![("fps".into(), VsfType::u3(30))], ..Default::default() };
    let clip = l.make_clip(&p, 1 << 10).unwrap();
    assert!(clip.direct());
    assert_eq!(clip.blocks, 1 << 10);
    // Found while it records, no bytes yet; not removable while its writer holds it.
    let f = Find { kinds: vec![kinds::RAW_VIDEO], ..Default::default() };
    let r = k.find(&f).unwrap();
    assert_eq!((r.total, r.items[0].1.size), (1, 0));
    assert!(matches!(k.remove(&r.items[0].0), Err(Error::Refused)));
    // Written straight to the flash, a transfer at a time; a write past its extent refused.
    let body = bytes((3 << 20) - 100, 9);
    let mut padded = body.clone();
    padded.resize(3 << 20, 0);
    for (n, chunk) in padded.chunks(1 << 20).enumerate() {
        l.write_clip(&clip, (n as u64) << 8, chunk).unwrap();
    }
    assert!(l.write_clip(&clip, (1 << 10) - 1, &[0u8; 8192]).is_err());
    let hash = *blake3::hash(&body).as_bytes();
    let card = l.seal_clip(clip, body.len() as u64, &hash).unwrap();
    let x = card.extent.unwrap();
    assert_eq!((card.size, card.body, x.blocks), (body.len() as u64, hash, (body.len() as u64).div_ceil(4096)));
    // Kūaha reads it by range like any body, from an offset inside a block.
    let r = k.find(&f).unwrap();
    let (kh, kc) = r.items[0].clone();
    assert_eq!(kc.size, body.len() as u64);
    let mut out = vec![0u8; (1 << 20) + 5000];
    assert_eq!(k.read(&kh, 12_345, &mut out).unwrap(), (out.len(), body.len() as u64));
    assert_eq!(&out[..], &body[12_345..12_345 + out.len()]);
    let mut whole = vec![0u8; body.len() + 4096];
    assert_eq!(k.read(&kh, 0, &mut whole).unwrap().0, body.len());
    assert_eq!(*blake3::hash(&whole[..body.len()]).as_bytes(), hash);
    // Removed: the card and the extent go, its blocks free again.
    k.remove(&kh).unwrap();
    assert_eq!(k.find(&f).unwrap().total, 0);
    let mut bb = b.borrow_mut();
    let bb = &mut *bb;
    let t = bb.clips.table(&mut bb.store);
    assert!(t.list().is_empty());
    assert_eq!(t.largest_free(), AREA);
}

#[test]
fn a_clip_written_thru_the_vault() {
    let b = Bench::new(false);
    let mut l = hold(&b, &lumis());
    let p = Proposed { kind: kinds::RAW_VIDEO, ..Default::default() };
    let clip = l.make_clip(&p, 64).unwrap();
    assert!(!clip.direct());
    let body = bytes(64 << 12, 1);
    l.write_clip(&clip, 0, &body[..32 << 12]).unwrap();
    l.write_clip(&clip, 32, &body[32 << 12..]).unwrap();
    // Not whole blocks, past the extent: refused.
    assert!(l.write_clip(&clip, 0, &body[..100]).is_err());
    assert!(l.write_clip(&clip, 63, &body[..8192]).is_err());
    let card = l.seal_clip(clip, body.len() as u64, blake3::hash(&body).as_bytes()).unwrap();
    let mut out = vec![0u8; body.len()];
    let h = l.find(&Find::default()).unwrap().items[0].0;
    assert_eq!(l.read(&h, 0, &mut out).unwrap(), (body.len(), card.size));
    assert_eq!(out, body);
}

#[test]
fn a_clip_whose_writer_died_gets_its_card_from_the_recoverer() {
    let b = Bench::new(true);
    let (mut l, mut k) = (hold(&b, &lumis()), hold(&b, &kuaha()));
    let clip = l.make_clip(&Proposed { kind: kinds::RAW_VIDEO, title: Some("cut short".into()), ..Default::default() }, 256).unwrap();
    l.write_clip(&clip, 0, &bytes(16 << 12, 3)).unwrap();
    // Lumis is killed: the kernel takes its extent back, the vault's recoverer closes the clip at its last whole frame.
    let id = k.find(&Find::default()).unwrap().items[0].1.id;
    {
        let mut bb = b.borrow_mut();
        let bb = &mut *bb;
        let name = clip_name(&bb.d, &id);
        bb.clips.recover(&mut bb.store, &name, 50_000, [6; 32]);
    }
    // The next call on the hold (anyone's) corrects the card.
    let r = k.find(&Find::default()).unwrap();
    let c = &r.items[0].1;
    assert_eq!((c.size, c.body, c.extent.unwrap().blocks), (50_000, [6; 32], 13));
    let mut out = vec![0u8; 50_000];
    assert_eq!(k.read(&r.items[0].0, 0, &mut out).unwrap().0, 50_000);
    assert_eq!(&out[..], &bytes(16 << 12, 3)[..50_000]);
    // A clip whose extent was dropped by hand (`store extent-drop`) loses its card at the next call: it names blocks nobody holds.
    let clip = l.make_clip(&Proposed { kind: kinds::RAW_VIDEO, title: Some("dropped".into()), ..Default::default() }, 8).unwrap();
    let _ = clip;
    {
        let mut bb = b.borrow_mut();
        let bb = &mut *bb;
        bb.clips.grants.clear();
        let t = bb.clips.table(&mut bb.store);
        let e = *t.list().iter().find(|e| !e.sealed).unwrap();
        t.remove(&e.name);
    }
    let r = k.find(&Find::default()).unwrap();
    assert_eq!(r.total, 1);
    assert_eq!(r.items[0].1.title.as_deref(), Some("cut short"));
}

#[test]
fn rights_makings_and_the_person() {
    let b = Bench::new(true);
    let (mut l, mut k) = (hold(&b, &lumis()), hold(&b, &kuaha()));
    let mut person = hold(&b, &Caller::person());
    let lists = Caller { maker: Some(Maker { developer: [5; 32], option: [5; 32], name: "Lists".into() }), makes: vec![kinds::LIST], ..Default::default() };
    let mut li = hold(&b, &lists);
    li.keep(&Proposed { kind: kinds::LIST, title: Some("Groceries: milk, bread".into()), ..Default::default() }, b"milk\nbread\n").unwrap();
    l.keep(&Proposed { kind: kinds::PHOTO, ..Default::default() }, b"p").unwrap();
    // Lumis may not make a list; it sees only what it made; Kūaha opens both kinds; the person all.
    assert!(matches!(l.keep(&Proposed { kind: kinds::LIST, ..Default::default() }, b"x"), Err(Error::Refused)));
    assert_eq!(l.find(&Find::default()).unwrap().total, 1);
    assert_eq!(k.find(&Find::default()).unwrap().total, 2);
    assert_eq!(person.find(&Find { words: Some("milk".into()), ..Default::default() }).unwrap().total, 1);
    // A making is its maker's: Kūaha cannot write into Lumis's.
    let m = l.make(&Proposed { kind: kinds::PHOTO, ..Default::default() }).unwrap();
    let stolen = wire::call(wire::OP_WRITE, &[0; 32], 0, 0, b"evil");
    assert_eq!(k.t.call(&stolen, &mut []), ERR);
    l.write(&m, b"mine").unwrap();
    // At most MAKINGS_MOST unfinished at once a caller.
    let open: Vec<_> = (1..MAKINGS_MOST).map(|_| l.make(&Proposed { kind: kinds::PHOTO, ..Default::default() }).unwrap()).collect();
    assert!(matches!(l.make(&Proposed { kind: kinds::PHOTO, ..Default::default() }), Err(Error::Refused)));
    assert!(k.find(&Find::default()).is_ok(), "another caller is not refused");
    l.finish(m).unwrap();
    drop(open);
    // The person locks: handles from before mean nothing, a find hands new ones; what is kept is still there.
    let h = k.find(&Find::default()).unwrap().items[0].0;
    b.borrow_mut().server.forget();
    assert!(matches!(k.card(&h), Err(Error::Refused)));
    assert_eq!(k.find(&Find::default()).unwrap().total, 3);
    // A new vault process (a fresh server over the same disk): the same items.
    b.borrow_mut().server = Server::new([43; 32]);
    assert_eq!(person.find(&Find::default()).unwrap().total, 3);
}

/// What is on the device: the hold's totals by kind (whoever made them, whatever the asker may open), the clips area, and the kists the vault found with their domains' sizes.
#[test]
fn usage_shows_the_whole_device() {
    let b = Bench::new(true);
    let (mut l, mut k) = (hold(&b, &lumis()), hold(&b, &kuaha()));
    l.keep(&Proposed { kind: kinds::PHOTO, ..Default::default() }, &bytes(1000, 1)).unwrap();
    l.keep(&Proposed { kind: kinds::PHOTO, ..Default::default() }, &bytes(3000, 2)).unwrap();
    let clip = l.make_clip(&Proposed { kind: kinds::RAW_VIDEO, ..Default::default() }, 100).unwrap();
    l.write_clip(&clip, 0, &bytes(8192, 3)).unwrap();
    l.seal_clip(clip, 6000, &[1; 32]).unwrap();
    let u = k.usage().unwrap();
    let photo = u.kinds.iter().find(|x| x.kind == kinds::PHOTO).unwrap();
    let raw = u.kinds.iter().find(|x| x.kind == kinds::RAW_VIDEO).unwrap();
    assert_eq!((photo.items, photo.bytes, raw.items, raw.bytes), (2, 4000, 1, 6000));
    assert_eq!(u.clips, Some(wire::ClipsUse { blocks: AREA, taken: 2, largest_free: AREA - 2 }));
    assert!(u.kists.is_empty(), "the kists are the vault's to add");
    // The vault's part: the kists from the person's list, sized where named; the document carries them whole.
    let kist = wire::KistUse { developer: [1; 32], option: [2; 32], name: "Lumis".into(), values: Some(12), bytes: Some(1 << 20), clip_blocks: 2 };
    let plain = wire::KistUse { developer: [3; 32], option: [4; 32], name: "Kūaha".into(), ..Default::default() };
    let mut bb = b.borrow_mut();
    let bb = &mut *bb;
    let mut out = vec![0u8; 1 << 14];
    let a = bb.server.usage(&mut bb.store, &bb.d, vec![kist.clone(), plain.clone()], &mut out, 1 << 14, T0, &mut bb.clips);
    let u = wire::parse_usage(&out[..a.value as usize]).unwrap();
    assert_eq!(u.kists, vec![kist, plain]);
    assert_eq!(u.kinds.len(), 2);
}

#[test]
fn release_drops_the_whole_hold() {
    let b = Bench::new(true);
    let mut l = hold(&b, &lumis());
    let before = {
        let mut bb = b.borrow_mut();
        let bb = &mut *bb;
        // The extents' table is the vault's own value, there before and after.
        bb.clips.table(&mut bb.store);
        bb.store.engine().live_counts()
    };
    for k in 0..3u8 {
        l.keep(&Proposed { kind: kinds::PHOTO, title: Some(format!("p{k}")), ..Default::default() }, &bytes(200_000, k)).unwrap();
    }
    let mut bb = b.borrow_mut();
    let bb = &mut *bb;
    let kept = bb.store.engine().live_counts();
    // As the vault's OP_DROP does: the hold's own values, then ferros_store's domain (its books).
    assert_eq!(bb.server.drop_domain(&mut bb.store, &bb.d).unwrap(), 3);
    bb.store.drop_domain(&bb.d).unwrap();
    let after = bb.store.engine().live_counts();
    eprintln!("live blocks: {before:?} before, {kept:?} kept, {after:?} after the drop");
    // What stays is the engine's index skeleton, never a piece of a body (three bodies are a dozen pieces).
    assert!(after.0 + after.1 < 8 && kept.0 + kept.1 > 12, "every body, page and list of the hold is gone");
    assert_eq!(Server::new([1; 32]).items.find(&mut bb.store, &bb.d, &Caller::person(), &Find::default()).unwrap().total, 0);
}
