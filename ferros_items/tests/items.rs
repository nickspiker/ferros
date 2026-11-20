//! The hold on a RAM vault: make, write, finish, read, card, find, keep again, remove, single-copy extents, pages that split, rights, the wire documents.

use ferros_items::{kinds, wire, Caller, Error, Extent, Find, Items, Maker, Proposed, Read, CARD_MAX, IS_AT_LEAST, IS_EQUAL, IS_WORDS, ORDER_LARGEST, ORDER_OLDEST, ORDER_TITLE};
use ferros_store::{Domain, Nonces, Store};
use manifestus::{Block, BlockDev, DeletePolicy, Mirror, Vault};
use vsf::types::EtType;
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

fn now() -> i64 {
    1
}

type S = Store<Mem, Mem, Counter>;

fn store(blocks: usize) -> S {
    let m = Mirror::new(Mem(vec![[0u8; 4096]; blocks]), Mem(vec![[0u8; 4096]; blocks]));
    Store::new(Vault::open(m, 8, 1, DeletePolicy::UnlinkOnly).expect("vault"), Counter(0), now)
}

fn domain() -> Domain {
    Domain { addr: [7; 32], seal: [9; 32] }
}

fn lumis() -> Caller {
    Caller { maker: Some(Maker { developer: [1; 32], option: [2; 32], name: "Lumis".into() }), makes: vec![kinds::PHOTO, kinds::RAW_VIDEO], opens: vec![], device: Some([5; 32]), any: false }
}

fn kuaha() -> Caller {
    Caller { maker: Some(Maker { developer: [3; 32], option: [4; 32], name: "Kūaha".into() }), makes: vec![], opens: vec![kinds::PHOTO, kinds::RAW_VIDEO, kinds::LIST], device: Some([5; 32]), any: false }
}

/// An hour of oscillations of the hydrogen line, about 2^42.4: a page's narrowest span is 2^42.
const HOUR: i64 = 5_113_460_706_365;
const T0: i64 = 1 << 60;

fn photo(title: &str, lens: u64) -> Proposed {
    Proposed { kind: kinds::PHOTO, title: Some(title.into()), attrs: vec![("lens".into(), VsfType::u3(lens as u8)), ("place".into(), VsfType::x("Raglan".into()))], ..Default::default() }
}

fn keep(items: &mut Items, s: &mut S, d: &Domain, c: &Caller, p: Proposed, t: i64, body: &[u8]) -> [u8; 32] {
    let mut m = items.make(c, p, t, [t as u8; 32]).unwrap();
    for chunk in body.chunks(1 << 18) {
        items.write(s, d, &mut m, chunk, t).unwrap();
    }
    items.finish(s, d, c, m, t).unwrap().0
}

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

#[test]
fn make_read_find_keep_again_remove() {
    let (mut s, d) = (store(1 << 14), domain());
    let mut items = Items::new([42; 32]);
    let l = lumis();
    let body = bytes(200_000, 3);
    let h = keep(&mut items, &mut s, &d, &l, photo("Sunset at Raglan", 24), T0, &body);
    let c = items.card(&mut s, &d, &l, &h).unwrap();
    assert_eq!(c.size, body.len() as u64);
    assert_eq!(c.body, *blake3::hash(&body).as_bytes());
    assert_eq!(c.kist.name, "Lumis");
    assert_eq!(c.device, Some([5; 32]));
    // Read by ranges.
    let mut out = vec![0u8; 70_000];
    assert_eq!(items.read(&mut s, &d, &l, &h, 100_000, &mut out).unwrap(), Read::Bytes(70_000, 200_000));
    assert_eq!(&out[..], &body[100_000..170_000]);
    assert_eq!(items.read(&mut s, &d, &l, &h, 190_000, &mut out).unwrap(), Read::Bytes(10_000, 200_000));
    // A second item, a find by kind and attributes, with every condition the cards answer.
    keep(&mut items, &mut s, &d, &l, photo("Kids in the river", 50), T0 + HOUR, &bytes(10, 1));
    let f = Find { kinds: vec![kinds::PHOTO], ..Default::default() };
    let found = items.find(&mut s, &d, &l, &f).unwrap();
    assert_eq!(found.total, 2);
    assert_eq!(found.items[0].1.title.as_deref(), Some("Kids in the river"), "newest first");
    let f = Find { kinds: vec![kinds::PHOTO], wheres: vec![("lens".into(), IS_AT_LEAST, VsfType::u(30, false))], ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &l, &f).unwrap().total, 1);
    let f = Find { wheres: vec![("lens".into(), IS_EQUAL, VsfType::u(24, false))], ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &l, &f).unwrap().items[0].1.title.as_deref(), Some("Sunset at Raglan"));
    let f = Find { words: Some("RIVER".into()), ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &l, &f).unwrap().total, 1);
    let f = Find { wheres: vec![("place".into(), IS_WORDS, VsfType::x("ragl".into()))], order: ORDER_OLDEST, ..Default::default() };
    let r = items.find(&mut s, &d, &l, &f).unwrap();
    assert_eq!((r.total, r.items[0].1.title.as_deref()), (2, Some("Sunset at Raglan")));
    let f = Find { from: Some(T0 + 1), ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &l, &f).unwrap().total, 1);
    // Kept again in place: the same item, a new body, no versions.
    let mut m = items.keep_again(&mut s, &d, &l, &h, T0 + 2 * HOUR).unwrap();
    items.write(&mut s, &d, &mut m, b"cropped", T0).unwrap();
    let (h2, c2) = items.finish(&mut s, &d, &l, m, T0 + 2 * HOUR).unwrap();
    assert_eq!((h2, c2.id, c2.size, c2.made, c2.changed), (h, c.id, 7, c.made, T0 + 2 * HOUR));
    assert_eq!(items.find(&mut s, &d, &l, &Find::default()).unwrap().total, 2);
    // Removed: gone at once, its handle with it; nothing else of the domain is listed.
    let gone = items.remove(&mut s, &d, &l, &h, T0).unwrap();
    assert_eq!(gone.id, c.id);
    assert!(matches!(items.card(&mut s, &d, &l, &h), Err(Error::Refused)));
    assert_eq!(items.find(&mut s, &d, &l, &Find::default()).unwrap().total, 1);
    assert!(s.names(&d).unwrap().is_empty(), "the hold's values stay off ferros_store's list");
    // Read again from the disk alone (a new vault process): the same answer.
    let mut fresh = Items::new([43; 32]);
    let r = fresh.find(&mut s, &d, &Caller::person(), &Find::default()).unwrap();
    assert_eq!((r.total, r.items[0].1.title.as_deref()), (1, Some("Kids in the river")));
}

#[test]
fn rights_are_the_stamps() {
    let (mut s, d) = (store(1 << 13), domain());
    let mut items = Items::new([1; 32]);
    let (l, h) = (lumis(), kuaha());
    // Kūaha makes nothing; Lumis makes photos and RAW video, not lists.
    assert!(matches!(items.make(&h, photo("x", 1), T0, [0; 32]), Err(Error::Refused)));
    assert!(matches!(items.make(&l, Proposed { kind: kinds::LIST, ..Default::default() }, T0, [0; 32]), Err(Error::Refused)));
    let lh = keep(&mut items, &mut s, &d, &l, photo("mine", 1), T0, b"abc");
    // A handle is the caller's own: Kūaha cannot use Lumis's.
    assert!(matches!(items.card(&mut s, &d, &h, &lh), Err(Error::Refused)));
    // Kūaha finds it by kind (it may open photos) and gets its own handle.
    let found = items.find(&mut s, &d, &h, &Find::default()).unwrap();
    assert_eq!(found.total, 1);
    let hh = found.items[0].0;
    assert_ne!(hh, lh);
    let mut out = [0u8; 3];
    assert_eq!(items.read(&mut s, &d, &h, &hh, 0, &mut out).unwrap(), Read::Bytes(3, 3));
    // Another kist with no opens sees nothing but what it made.
    let other = Caller { maker: Some(Maker { developer: [8; 32], option: [8; 32], name: "Other".into() }), makes: vec![kinds::PHOTO], ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &other, &Find::default()).unwrap().total, 0);
    keep(&mut items, &mut s, &d, &other, photo("theirs", 2), T0 + 1, b"z");
    assert_eq!(items.find(&mut s, &d, &other, &Find::default()).unwrap().total, 1);
    assert_eq!(items.find(&mut s, &d, &l, &Find::default()).unwrap().total, 1, "Lumis sees its own only");
    // Found by maker.
    let f = Find { maker: Some(l.maker.as_ref().unwrap().tag()), ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &Caller::person(), &f).unwrap().total, 1);
    // A card past one block is refused.
    let big = Proposed { kind: kinds::PHOTO, attrs: (0..CARD_MAX / 32).map(|k| (format!("a{k}"), VsfType::hb(vec![k as u8; 32]))).collect(), ..Default::default() };
    assert!(matches!(items.make(&l, big, T0, [0; 32]), Err(Error::TooLarge)));
}

#[test]
fn single_copy_bodies() {
    let (mut s, d) = (store(1 << 13), domain());
    let mut items = Items::new([1; 32]);
    let l = lumis();
    let clip = Proposed { kind: kinds::RAW_VIDEO, title: Some("Bench, 30 a second".into()), attrs: vec![("fps".into(), VsfType::u(30, false))], ..Default::default() };
    let m = items.make(&l, clip, T0, [9; 32]).unwrap();
    let id = *m.id();
    let x = Extent { first: 618_729, blocks: 266_261 };
    let (h, c) = items.finish_extent(&mut s, &d, &l, m, x, 1_090_604_642, [0xAB; 32], T0).unwrap();
    assert_eq!((c.id, c.extent, c.size, c.body), (id, Some(x), 1_090_604_642, [0xAB; 32]));
    // Its body is the extent: the caller reads its blocks.
    let mut out = [0u8; 16];
    assert_eq!(items.read(&mut s, &d, &l, &h, 0, &mut out).unwrap(), Read::Extent(x, 1_090_604_642));
    // Found like any item, by kind and attribute.
    let f = Find { kinds: vec![kinds::RAW_VIDEO], wheres: vec![("fps".into(), IS_EQUAL, VsfType::u(30, false))], ..Default::default() };
    assert_eq!(items.find(&mut s, &d, &kuaha(), &f).unwrap().total, 1);
    // Removing it gives the card back with its extent, for the vault to free.
    assert_eq!(items.remove(&mut s, &d, &l, &h, T0).unwrap().extent, Some(x));
    assert_eq!(items.find(&mut s, &d, &l, &Find::default()).unwrap().total, 0);
}

#[test]
fn pages_split_and_finds_read_only_their_pages() {
    let (mut s, d) = (store(1 << 15), domain());
    let mut items = Items::new([1; 32]);
    let l = lumis();
    // 600 items over 600 hours: the cards pass one 64 KiB page and split down by time.
    for k in 0..600i64 {
        let p = Proposed { kind: if k % 3 == 0 { kinds::RAW_VIDEO } else { kinds::PHOTO }, title: Some(format!("item {k}")), attrs: vec![("n".into(), VsfType::u(k as usize, false))], ..Default::default() };
        let m = items.make(&l, p, T0 + k * HOUR, [k as u8; 32]).unwrap();
        if k % 3 == 0 {
            items.finish_extent(&mut s, &d, &l, m, Extent { first: k as u64, blocks: 1 }, 1, [0; 32], T0 + k * HOUR).unwrap();
        } else {
            let mut m = m;
            items.write(&mut s, &d, &mut m, &[k as u8], T0).unwrap();
            items.finish(&mut s, &d, &l, m, T0 + k * HOUR).unwrap();
        }
    }
    let p = Caller::person();
    let all = Find { count: 256, ..Default::default() };
    let r = items.find(&mut s, &d, &p, &all).unwrap();
    assert_eq!((r.total, r.items.len()), (600, 256));
    assert_eq!(r.items[0].1.title.as_deref(), Some("item 599"));
    let r = items.find(&mut s, &d, &p, &Find { skip: 512, ..all.clone() }).unwrap();
    assert_eq!(r.items.len(), 600 - 512);
    // Read again from the disk: more than one page, and the same answers.
    let mut fresh = Items::new([2; 32]);
    let pages = fresh.pages(&mut s, &d).unwrap();
    assert!(pages.len() > 1 && pages.iter().all(|p| p.1 < ferros_items::TOP_SPAN), "split: {pages:?}");
    eprintln!("600 cards in {} pages", pages.len());
    let f = Find { kinds: vec![kinds::RAW_VIDEO], from: Some(T0 + 100 * HOUR), to: Some(T0 + 200 * HOUR), order: ORDER_OLDEST, ..Default::default() };
    let r = fresh.find(&mut s, &d, &p, &f).unwrap();
    assert_eq!(r.total, 33);
    assert_eq!(r.items[0].1.title.as_deref(), Some("item 102"));
    let r = fresh.find(&mut s, &d, &p, &Find { order: ORDER_TITLE, count: 1, ..Default::default() }).unwrap();
    assert_eq!(r.items[0].1.title.as_deref(), Some("item 0"));
    let r = fresh.find(&mut s, &d, &p, &Find { order: ORDER_LARGEST, count: 1, ..Default::default() }).unwrap();
    assert_eq!(r.items[0].1.size, 1);
    // Removing every RAW video of one span empties nothing wrongly: the rest still found.
    let r = fresh.find(&mut s, &d, &p, &f).unwrap();
    for (h, _) in &r.items {
        fresh.remove(&mut s, &d, &p, h, T0).unwrap();
    }
    assert_eq!(fresh.find(&mut s, &d, &p, &Find::default()).unwrap().total, 600 - 33);
    assert_eq!(Items::new([3; 32]).find(&mut s, &d, &p, &f).unwrap().total, 0);
}

#[test]
fn a_body_whose_card_never_landed_is_swept() {
    let (mut s, d) = (store(1 << 14), domain());
    let mut items = Items::new([1; 32]);
    let l = lumis();
    let mut m = items.make(&l, photo("lost", 1), T0, [1; 32]).unwrap();
    items.write(&mut s, &d, &mut m, &bytes(40 << 16, 2), T0).unwrap();
    drop(m);
    let kept = keep(&mut items, &mut s, &d, &l, photo("kept", 1), T0 + 1, b"ok");
    let mut fresh = Items::new([2; 32]);
    assert_eq!(fresh.sweep(&mut s, &d, T0).unwrap(), 0, "an unfinished body was never committed: ferros_store's own sweep takes its pieces");
    assert!(s.sweep(&d).unwrap() > 0);
    // The kept item is whole.
    let r = fresh.find(&mut s, &d, &l, &Find::default()).unwrap();
    assert_eq!(r.total, 1);
    let _ = kept;
}

#[test]
fn wire_documents_round_trip() {
    let p = Proposed { kind: kinds::LIST, title: Some("Groceries: milk, bread".into()), of: vec![(ferros_items::Ht::from_bytes([6; 32]), 1)], attrs: vec![("items".into(), VsfType::u3(2)), ("at".into(), VsfType::e(EtType::e6(T0)))], keeper: 0 };
    assert_eq!(wire::parse_propose(&wire::propose(&p, T0).unwrap()).unwrap(), p);
    let f = Find { kinds: vec![kinds::PHOTO, kinds::SCAN], maker: Some([2; 32]), from: Some(1), to: Some(T0), changed: Some(5), words: Some("tax 2026".into()), of: Some([4; 32]), wheres: vec![("lens".into(), IS_AT_LEAST, VsfType::u3(30)), ("place".into(), IS_WORDS, VsfType::x("Ra".into()))], order: 2, skip: 3, count: 4 };
    assert_eq!(wire::parse_find(&wire::find(&f, T0).unwrap()).unwrap(), f);
    let (mut s, d) = (store(1 << 13), domain());
    let mut items = Items::new([1; 32]);
    keep(&mut items, &mut s, &d, &lumis(), photo("a", 1), T0, b"x");
    let found = items.find(&mut s, &d, &lumis(), &Find::default()).unwrap();
    assert_eq!(wire::parse_found(&wire::found(&found, T0).unwrap()).unwrap(), found);
    let (h, c) = &found.items[0];
    assert_eq!(wire::parse_card_doc(&wire::card_doc(c, h, T0).unwrap()).unwrap(), (*h, c.clone()));
    let call = wire::call(wire::OP_READ, h, 7, 1 << 20, b"pay");
    assert_eq!(wire::parse_call(&call), Some((wire::OP_READ, *h, 7, 1 << 20, &b"pay"[..])));
}

#[test]
fn a_clip_kept_as_it_records() {
    let (mut s, d) = (store(1 << 13), domain());
    let mut items = Items::new([1; 32]);
    let l = lumis();
    let clip = Proposed { kind: kinds::RAW_VIDEO, title: Some("recording".into()), ..Default::default() };
    let reserved = Extent { first: 100, blocks: 1 << 20 };
    let (h, c) = items.make_extent(&mut s, &d, &l, clip, reserved, T0, [3; 32]).unwrap();
    assert_eq!((c.size, c.extent), (0, Some(reserved)));
    // Found while it records.
    assert_eq!(items.find(&mut s, &d, &l, &Find::default()).unwrap().total, 1);
    let sealed = Extent { first: 100, blocks: 1000 };
    let c = items.seal_extent(&mut s, &d, &l, &h, sealed, 4_000_000, [7; 32], T0 + 1).unwrap();
    assert_eq!((c.size, c.extent, c.body), (4_000_000, Some(sealed), [7; 32]));
    // Read from the disk alone: the sealed card.
    let r = Items::new([2; 32]).find(&mut s, &d, &Caller::person(), &Find::default()).unwrap();
    assert_eq!(r.items[0].1.extent, Some(sealed));
    // The vault's own correction of a clip its recoverer closed.
    let c = items.amend_extent(&mut s, &d, &c.id, c.made, Extent { first: 100, blocks: 900 }, 3_600_000, [8; 32], T0 + 2).unwrap();
    assert_eq!(c.size, 3_600_000);
}

#[test]
fn the_callers_stamp_reads() {
    let mut s = vec![1u8];
    s.extend_from_slice(&[3; 32]);
    s.extend_from_slice(&[4; 32]);
    s.push(5);
    s.extend_from_slice(b"lumis");
    s.extend_from_slice(&1u16.to_le_bytes());
    s.extend_from_slice(&kinds::RAW_VIDEO);
    s.extend_from_slice(&2u16.to_le_bytes());
    s.extend_from_slice(&kinds::PHOTO);
    s.extend_from_slice(&kinds::SCAN);
    s.extend_from_slice(b"call");
    let (c, call) = Caller::from_stamp(&s).unwrap();
    let m = c.maker.unwrap();
    assert_eq!((m.developer, m.option, m.name.as_str(), c.makes, c.opens, c.any, call), ([3; 32], [4; 32], "lumis", vec![kinds::RAW_VIDEO], vec![kinds::PHOTO, kinds::SCAN], false, &b"call"[..]));
    let mut p = vec![4u8];
    p.extend_from_slice(&[0; 69]);
    let (c, call) = Caller::from_stamp(&p).unwrap();
    assert!(c.maker.is_none() && call.is_empty());
    assert!(Caller::from_stamp(&s[..70]).is_none());
}

/// An item's id is a thing (`ht`) on its card, and a card the hold wrote before (its id an `hp`, 2026-11-19) reads with the same id.
#[test]
fn an_item_id_is_a_thing_and_an_older_card_keeps_it() {
    let mut items = Items::new([1; 32]);
    let m = items.make(&lumis(), photo("a photo", 30), T0, [7; 32]).unwrap();
    let card = m.card().clone();
    let bytes = card.section(None).unwrap();
    let id = *card.id.as_bytes();
    let at = (0..bytes.len() - 36).find(|&i| bytes[i..i + 2] == *b"ht" && bytes[i + 4..i + 36] == id).expect("the id written as ht");
    assert_eq!(ferros_items::Card::parse(&bytes).unwrap().0, card);
    let mut older = bytes.clone();
    older[at + 1] = b'p';
    assert_eq!(ferros_items::Card::parse(&older).unwrap().0.id, card.id, "an hp id reads as the same thing");
    // Any other marker for the id is refused.
    older[at + 1] = b'b';
    assert!(ferros_items::Card::parse(&older).is_err());
}
