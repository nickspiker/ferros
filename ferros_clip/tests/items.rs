//! The recorder writing a clip as an item of the hold (ferros_clip `items::ItemSink`): a kist's calls thru the kernel's stamp into the vault's `serve::Server` over a RAM vault and clips area, as ferros_items' tests/serve.rs runs them.
//! The clip's card is found while it records, its blocks go straight to its extent (or thru the vault without the capability), and the sealed card carries the clip's length and seal; the body read back by range decodes every frame.

use std::sync::{Arc, Mutex};

use ferros_clip::items::{raw_video_card, ItemSink, ItemSource};
use ferros_clip::{cfa_of_byte, Camera, FrameIn, Mode, Pacing, Recorder, Taken};
use ferros_items::kist::{Hold, Transport, ERR};
use ferros_items::serve::{Clips, Rec, Server};
use ferros_items::{kinds, wire, Caller, Extent, Find, Maker};
use ferros_store::{Domain, Nonces, Store};
use manifestus::{Block, BlockDev, DeletePolicy, Extents, Mirror, Vault};
use vsf::visual::clip::Reader;
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

/// The clips area's blocks here: 2^14, 64 MiB.
const AREA: u64 = 1 << 14;
const T0: i64 = 1 << 60;

/// The vault's clips area and table over RAM (ferros_items tests/serve.rs `MemClips`).
struct MemClips {
    table: Option<Extents>,
    area: Vec<u8>,
    direct: bool,
    grants: Vec<([u8; 32], [u8; 32], Extent)>,
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

struct Bench {
    server: Server,
    store: S,
    d: Domain,
    clips: MemClips,
    now: i64,
    random: u64,
    /// Blocks written straight to the flash (the capability), and calls thru the vault.
    direct_blocks: u64,
}

/// A caller's side, shareable with the recorder's writer thread: the kernel's stamp on every call.
struct Kist {
    b: Arc<Mutex<Bench>>,
    stamp: Vec<u8>,
}

impl Transport for Kist {
    fn call(&mut self, call: &[u8], reply: &mut [u8]) -> u64 {
        let mut b = self.b.lock().unwrap();
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
    fn block_write(&mut self, cap: &[u8; 32], block: u64, bytes: &[u8]) -> u64 {
        let mut b = self.b.lock().unwrap();
        let Some(&(_, _, x)) = b.clips.grants.iter().find(|g| g.0 == *cap) else { return ERR };
        if bytes.len() & 4095 != 0 || !x.holds(block, (bytes.len() >> 12) as u64) {
            return ERR;
        }
        b.clips.write(x.first + block, bytes);
        b.direct_blocks += (bytes.len() >> 12) as u64;
        0
    }
}

fn bench(direct: bool) -> Arc<Mutex<Bench>> {
    let m = Mirror::new(Mem(vec![[0u8; 4096]; 1 << 14]), Mem(vec![[0u8; 4096]; 1 << 14]));
    let store = Store::new(Vault::open(m, 8, 1, DeletePolicy::UnlinkOnly).unwrap(), Counter(0), || T0);
    let clips = MemClips { table: None, area: vec![0u8; (AREA as usize) << 12], direct, grants: Vec::new() };
    Arc::new(Mutex::new(Bench { server: Server::new([42; 32]), store, d: Domain { addr: [7; 32], seal: [9; 32] }, clips, now: T0, random: 0, direct_blocks: 0 }))
}

fn clock() -> i64 {
    T0
}

fn hold(b: &Arc<Mutex<Bench>>, makes: bool) -> Hold<Kist> {
    let c = if makes {
        Caller { maker: Some(Maker { developer: [1; 32], option: [2; 32], name: "Lumis".into() }), makes: vec![kinds::RAW_VIDEO], ..Default::default() }
    } else {
        Caller { maker: Some(Maker { developer: [3; 32], option: [4; 32], name: "Kūaha".into() }), opens: vec![kinds::RAW_VIDEO], ..Default::default() }
    };
    Hold::new(Kist { b: b.clone(), stamp: c.stamp(true) }, clock)
}

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); // the algorithm: an LCG wraps by definition
    *s >> 33
}

/// A packed RAW10 frame of `w` x `h` and its samples.
fn frame(w: usize, h: usize, stride: usize, seed: u64) -> (Vec<u8>, Vec<u16>) {
    let mut s = seed;
    let px: Vec<u16> = (0..w * h).map(|i| ((64 + ((i % w) & 1) * 64 + (lcg(&mut s) & 63) as usize) & 1023) as u16).collect();
    let mut packed = vec![0u8; stride * h];
    for y in 0..h {
        for g in 0..w / 4 {
            let v = &px[y * w + g * 4..y * w + g * 4 + 4];
            let o = y * stride + g * 5;
            for i in 0..4 {
                packed[o + i] = (v[i] >> 2) as u8;
            }
            packed[o + 4] = (0..4).map(|i| ((v[i] & 3) << (2 * i)) as u8).fold(0, |a, b| a | b);
        }
    }
    (packed, px)
}

fn record_as_item(direct: bool) {
    let b = bench(direct);
    let (w, h, stride) = (1024usize, 320usize, 1280usize);
    let cam = Camera { width: w, height: h, stride, cfa: cfa_of_byte(0x49), black: 64, white: 1023, make: "ferros".into(), model: "test".into(), sensor_module: "main".into(), lens: String::new(), f_number: None, focal_mm: None, characterization: None, observer: String::new(), orientation: 1 };
    let at = 1i64 << 40;
    let (sink, receipt) = ItemSink::new(hold(&b, true), raw_video_card(Some("Bench at 30 a second".into()), at, vec![("fps".into(), VsfType::u3(30))]));
    let mode = Mode { pacing: Pacing::of(30), title: "host test".into(), workers: vec![0; 4], writer: 0, slots: 32, blocks: 1 << 13, at, source_digests: false };
    let mut rec = Recorder::start(&cam, &mode, Box::new(sink)).unwrap();
    // Found while it records, by Kūaha, at no bytes yet.
    let mut k = hold(&b, false);
    let found = k.find(&Find { kinds: vec![kinds::RAW_VIDEO], ..Default::default() }).unwrap();
    assert_eq!((found.total, found.items[0].1.size), (1, 0));
    assert_eq!(found.items[0].1.title.as_deref(), Some("Bench at 30 a second"));
    let mut frames = Vec::new();
    for k in 0..24u64 {
        let (packed, px) = frame(w, h, stride, k);
        let f = FrameIn { rows: &packed, count: 100 + k, eagle_ns: 1_000_000_000_000 + k * 33_334_420, exposure_ns: 1 << 20, gain: 2048, focus: 7 };
        assert_eq!(rec.frame(&f), Taken::Recorded, "frame {k}");
        frames.push(px);
    }
    let info = rec.stop();
    assert!(info.error.is_none(), "{:?}", info.error);
    let r = receipt.lock().unwrap().clone();
    assert_eq!(r.direct, direct);
    let card = r.card.expect("the sealed card");
    assert_eq!((card.size, card.body, card.kind), (info.len, info.hash, kinds::RAW_VIDEO));
    assert_eq!(b.lock().unwrap().direct_blocks > 0, direct, "the writes' path");
    // Read back by Kūaha a range at a time and decoded: every frame its samples.
    let found = k.find(&Find { kinds: vec![kinds::RAW_VIDEO], ..Default::default() }).unwrap();
    let (kh, kc) = found.items[0].clone();
    assert_eq!(kc.size, info.len);
    let rd = Reader::open(ItemSource::new(k, kh, kc.size)).unwrap();
    assert_eq!(rd.frames().len(), frames.len());
    for (i, px) in frames.iter().enumerate() {
        assert_eq!(&rd.decode(i).unwrap(), px, "frame {i}");
    }
}

#[test]
fn a_clip_kept_as_an_item_straight_to_its_extent() {
    record_as_item(true);
}

#[test]
fn a_clip_kept_as_an_item_thru_the_vault() {
    record_as_item(false);
}
