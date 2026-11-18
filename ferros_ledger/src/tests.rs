//! The ledger on a RAM flash: the chain, the wrap, a torn write, a full ring, the seal, the render, and damage named rather than passed over.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::category::{self, Class};
use crate::entry::{self, Error, Payload, SealKey};
use crate::preboot::{Pending, Preboot};
use crate::ring::{Blocks, Item, Ring, Why, BLOCK};
use crate::view;

/// A flash in RAM: `2 × half` blocks, every read and write counted. `cut` cuts the n-th write off after `keep` bytes (the rest of the block keeps what it held) and fails every write after it, the way power going away mid-write does.
struct Mem {
    blocks: Vec<[u8; BLOCK]>,
    reads: u64,
    writes: u64,
    cut: Option<(u64, usize)>,
    dead: bool,
}

impl Mem {
    fn new(half: u32) -> Mem {
        Mem { blocks: vec![[0u8; BLOCK]; 2 * half as usize], reads: 0, writes: 0, cut: None, dead: false }
    }
}

impl Blocks for Mem {
    fn read(&mut self, block: u32, out: &mut [u8; BLOCK]) -> bool {
        self.reads += 1;
        out.copy_from_slice(&self.blocks[block as usize]);
        true
    }
    fn write(&mut self, block: u32, data: &[u8; BLOCK]) -> bool {
        if self.dead {
            return false;
        }
        self.writes += 1;
        if let Some((n, keep)) = self.cut {
            if self.writes == n {
                self.blocks[block as usize][..keep].copy_from_slice(&data[..keep]);
                self.dead = true;
                return false;
            }
        }
        self.blocks[block as usize] = *data;
        true
    }
}

fn sys() -> SealKey {
    SealKey::new(&[7u8; 32])
}

fn person() -> SealKey {
    SealKey::new(&[9u8; 32])
}

fn words(n: u64) -> String {
    alloc::format!("line {n}: camera on in A#{} ms, module A#2", n * 3)
}

fn put(r: &mut Ring, m: &mut Mem, n: u64) {
    let p = Payload::new(category::TEXT, "kernel", &words(n));
    r.append(m, 1 + n / 64, Some(1_000_000 + n as i64), Class::System, &sys(), &p).unwrap();
}

fn entries(items: &[Item]) -> Vec<&entry::Stored> {
    items.iter().filter_map(|i| if let Item::Entry { stored, .. } = i { Some(stored) } else { None }).collect()
}

fn findings(items: &[Item]) -> Vec<&Why> {
    items.iter().filter_map(|i| if let Item::Finding(f) = i { Some(&f.why) } else { None }).collect()
}

#[test]
fn chain() {
    let mut m = Mem::new(16);
    let (mut r, f) = Ring::open(&mut m, 16);
    assert!(r.is_empty() && f.is_empty());
    for n in 0..40 {
        put(&mut r, &mut m, n);
    }
    let (r2, f) = Ring::open(&mut m, 16);
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(r2.next_seq(), 40);
    assert_eq!(r2.head_hash(), r.head_hash());
    let items = r2.newest(&mut m, usize::MAX);
    assert!(findings(&items).is_empty());
    let e = entries(&items);
    assert_eq!(e.len(), 40);
    assert_eq!(e[0].place.prev, entry::genesis());
    for (k, s) in e.iter().enumerate() {
        assert_eq!(s.place.seq, k as u64);
        if k > 0 {
            assert_eq!(s.place.prev, e[k - 1].hash);
        }
        let p = entry::open(s, &sys()).unwrap();
        assert_eq!(p.words, words(k as u64));
        assert_eq!(p.writer, "kernel");
        assert_eq!(p.category, "text");
    }
}

#[test]
fn wrap() {
    // Four blocks a copy: a few dozen entries fill it many times over.
    let half = 4;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    for n in 0..500 {
        put(&mut r, &mut m, n);
    }
    let (r2, f) = Ring::open(&mut m, half);
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(r2.next_seq(), 500);
    assert_eq!(r2.head_hash(), r.head_hash());
    let items = r2.newest(&mut m, usize::MAX);
    assert!(findings(&items).is_empty(), "{:?}", findings(&items));
    let e = entries(&items);
    // The newest are all there, in order, chained; the oldest are gone.
    assert_eq!(e.last().unwrap().place.seq, 499);
    assert!(e.len() > 3 * 4096 / 512 && e[0].place.seq > 0);
    for k in 1..e.len() {
        assert_eq!(e[k].place.seq, e[k - 1].place.seq + 1);
        assert_eq!(e[k].place.prev, e[k - 1].hash);
    }
    // The newest few, as `diag log 5` asks.
    let five = entries(&r2.newest(&mut m, 5)).iter().map(|s| s.place.seq).collect::<Vec<_>>();
    assert_eq!(five, vec![495, 496, 497, 498, 499]);
}

#[test]
fn full_ring_every_head_position() {
    // Open after every append across several laps: the head is found wherever it is, by search and chain.
    let half = 8;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    for n in 0..400u64 {
        put(&mut r, &mut m, n);
        let (o, f) = Ring::open(&mut m, half);
        assert!(f.is_empty());
        assert_eq!(o.next_seq(), n + 1);
        assert_eq!(o.head_block(), r.head_block());
    }
}

#[test]
fn torn_write_mid_block() {
    torn_after(3, false);
}

#[test]
fn torn_write_into_a_fresh_block() {
    // Ten entries fill block 0 so the eleventh starts block 1, never written before.
    torn_after(10, true);
}

fn torn_after(count: u64, fresh: bool) {
    let half = 8;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    for n in 0..count {
        put(&mut r, &mut m, n);
    }
    let before = r.head_hash();
    let block_before = r.head_block();
    // The next append writes A, reads it back, writes B: cut its A write a hundred bytes into the new entry.
    let used = {
        let items = r.newest(&mut m, 1);
        match &items[0] {
            Item::Entry { at, doc, .. } => at + doc.len(),
            _ => unreachable!(),
        }
    };
    // The new entry starts at `used`, or at the start of the next block when it does not fit.
    m.cut = Some((m.writes + 1, if fresh { 0 } else { used } + 100));
    let p = Payload::new(category::TEXT, "kernel", "the entry the power cut");
    assert!(r.append(&mut m, 1, None, Class::System, &sys(), &p).is_err());
    // Power back: open finds the head before the cut, names the torn write, and loses nothing earlier.
    m.cut = None;
    m.dead = false;
    let (mut r2, f) = Ring::open(&mut m, half);
    assert_eq!(f.iter().map(|f| &f.why).collect::<Vec<_>>(), vec![&Why::Torn]);
    assert_eq!(r2.head_hash(), before);
    assert_eq!(r2.head_block(), block_before);
    assert_eq!(r2.next_seq(), count);
    let items = r2.newest(&mut m, usize::MAX);
    assert_eq!(entries(&items).len(), count as usize);
    assert_eq!(findings(&items), vec![&Why::Torn]);
    // The next entry goes where the torn one was; the torn bytes are gone.
    put(&mut r2, &mut m, count);
    let (r3, f) = Ring::open(&mut m, half);
    assert!(f.is_empty(), "{f:?}");
    let items = r3.newest(&mut m, usize::MAX);
    assert!(findings(&items).is_empty());
    assert_eq!(entries(&items).len(), count as usize + 1);
    assert_eq!(r3.head_block() != block_before, fresh);
}

#[test]
fn cut_between_primary_and_mirror() {
    // The primary written and verified, the power gone before the mirror: the entry is in, the mirror said to be behind, and the next append writes both.
    let half = 8;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    for n in 0..3 {
        put(&mut r, &mut m, n);
    }
    m.cut = Some((m.writes + 2, 0));
    let p = Payload::new(category::TEXT, "kernel", "written to the primary only");
    assert!(r.append(&mut m, 1, None, Class::System, &sys(), &p).is_err());
    m.cut = None;
    m.dead = false;
    let (mut r2, f) = Ring::open(&mut m, half);
    assert_eq!(r2.next_seq(), 4);
    assert_eq!(f.iter().map(|f| &f.why).collect::<Vec<_>>(), vec![&Why::MirrorBehind]);
    put(&mut r2, &mut m, 4);
    let (r3, f) = Ring::open(&mut m, half);
    assert!(f.is_empty(), "{f:?}");
    assert_eq!(entries(&r3.newest(&mut m, usize::MAX)).len(), 5);
}

#[test]
fn torn_write_starting_a_block() {
    // Fill the ring once round, then cut the write that starts a new block over the older lap: the mirror still holds the older lap there.
    let half = 4;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    let mut n = 0u64;
    while r.head_block() != half - 1 || n < 200 {
        put(&mut r, &mut m, n);
        n += 1;
    }
    // Fill the head block until the next entry must start the next one.
    let start = r.head_block();
    while r.head_block() == start {
        let head = r.head_hash();
        let mut probe = Mem { blocks: m.blocks.clone(), reads: 0, writes: 0, cut: None, dead: false };
        let (mut pr, _) = Ring::open(&mut probe, half);
        put(&mut pr, &mut probe, n);
        if pr.head_block() != start {
            assert_eq!(head, r.head_hash());
            break;
        }
        put(&mut r, &mut m, n);
        n += 1;
    }
    let before = r.head_hash();
    m.cut = Some((m.writes + 1, 200));
    let p = Payload::new(category::TEXT, "kernel", &words(n));
    assert!(r.append(&mut m, 9, None, Class::System, &sys(), &p).is_err());
    m.cut = None;
    m.dead = false;
    let (r2, _) = Ring::open(&mut m, half);
    assert_eq!(r2.head_hash(), before);
    assert_eq!(r2.next_seq(), n);
    let items = r2.newest(&mut m, usize::MAX);
    let e = entries(&items);
    assert_eq!(e.last().unwrap().place.seq, n - 1);
    // The damaged copy is named.
    assert!(findings(&items).iter().any(|w| matches!(w, Why::OneCopy { bad: 0, .. })), "{:?}", findings(&items));
}

#[test]
fn damage_is_named_not_skipped() {
    let half = 8;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    for n in 0..12 {
        put(&mut r, &mut m, n);
    }
    let items = r.newest(&mut m, usize::MAX);
    let (blk, at, len) = items
        .iter()
        .find_map(|i| match i {
            Item::Entry { block, at, doc, stored } if stored.place.seq == 4 => Some((*block, *at, doc.len())),
            _ => None,
        })
        .unwrap();
    // One copy damaged: read from the other, and said so.
    m.blocks[blk as usize][at + len / 2] ^= 1;
    let items = r.newest(&mut m, usize::MAX);
    assert_eq!(entries(&items).len(), 12);
    assert!(matches!(findings(&items)[..], [Why::OneCopy { bad: 0, error: Error::Provenance(_) }]), "{:?}", findings(&items));
    // Both copies damaged: the entry is gone, and named where it was, with the gap it leaves.
    m.blocks[(half + blk) as usize][at + len / 2] ^= 1;
    let items = r.newest(&mut m, usize::MAX);
    let e = entries(&items);
    assert_eq!(e.len(), 11);
    assert!(!e.iter().any(|s| s.place.seq == 4));
    let named = items.iter().find_map(|i| match i {
        Item::Finding(f) if matches!(f.why, Why::Corrupt(_)) => Some((f.block, f.at)),
        _ => None,
    });
    assert_eq!(named, Some((blk, at)));
    assert!(findings(&items).iter().any(|w| **w == Why::SeqGap { expected: 4, found: 5 }));
    // The render says so too.
    let mut run = Vec::new();
    for i in &items {
        match i {
            Item::Entry { doc, .. } => run.extend_from_slice(doc),
            Item::Finding(f) => run.extend_from_slice(&view::finding(f, None, None).unwrap()),
        }
    }
    let lines = view::render(&run, &|_| "T".to_string());
    assert!(lines.iter().any(|l| l.starts_with(&alloc::format!("!! block A#{blk} byte A#{at}: damaged in both copies"))), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("entries A#4 to A#4 are missing")), "{lines:?}");
}

#[test]
fn a_damaged_mirror_is_named() {
    let half = 8;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    for n in 0..6 {
        put(&mut r, &mut m, n);
    }
    let (blk, at, len) = r
        .newest(&mut m, usize::MAX)
        .iter()
        .find_map(|i| match i {
            Item::Entry { block, at, doc, stored } if stored.place.seq == 2 => Some((*block, *at, doc.len())),
            _ => None,
        })
        .unwrap();
    m.blocks[(half + blk) as usize][at + len / 3] ^= 1;
    let items = r.newest(&mut m, usize::MAX);
    assert_eq!(entries(&items).len(), 6);
    assert!(matches!(findings(&items)[..], [Why::OneCopy { bad: 1, .. }]), "{:?}", findings(&items));
}

#[test]
fn seal() {
    let place = entry::Place { seq: 3, boot: 931, prev: entry::genesis(), time: Some(42) };
    let p = Payload::new(category::PERSON_TEXT, "kernel", "alias given to the kist lumis");
    let doc = entry::build(&place, Class::Personal, &person(), &p).unwrap();
    // The words are not in the clear anywhere in the document.
    assert!(!doc.windows(5).any(|w| w == b"lumis"));
    let s = entry::parse(&doc).unwrap();
    assert_eq!(s.class, Class::Personal.number());
    assert_eq!(entry::open(&s, &person()).unwrap(), p);
    // Another key: refused by id before anything is tried.
    assert_eq!(entry::open(&s, &sys()), Err(Error::WrongKey));
    // Every open field is bound: a boot, a time, a place in the chain moved onto it does not open.
    for change in 0..4 {
        let mut t = s.clone();
        match change {
            0 => t.place.boot += 1,
            1 => t.place.time = Some(43),
            2 => t.place.prev[0] ^= 1,
            _ => t.sealed[0] ^= 1,
        }
        assert_eq!(entry::open(&t, &person()), Err(Error::Seal), "change {change}");
    }
    // A flipped byte anywhere fails the provenance hash before the seal is reached.
    let mut d = doc.clone();
    let mid = d.len() / 2;
    d[mid] ^= 1;
    assert!(matches!(entry::parse(&d), Err(Error::Provenance(_)) | Err(Error::Schema(_))));
    // The same entry built twice is the same bytes (the nonce is synthetic): nothing random to leak, nothing to repeat for other words.
    assert_eq!(doc, entry::build(&place, Class::Personal, &person(), &p).unwrap());
    let other = entry::build(&place, Class::Personal, &person(), &Payload::new(category::PERSON_TEXT, "kernel", "alias given to the kist mahere")).unwrap();
    assert_ne!(entry::parse(&other).unwrap().nonce, s.nonce);
    // No clock yet: the header carries no time at all.
    let early = entry::build(&entry::Place { time: None, ..place.clone() }, Class::System, &sys(), &p).unwrap();
    assert_eq!(entry::parse(&early).unwrap().place.time, None);
}

#[test]
fn integers_are_auto_sized() {
    // seq and boot small and large: the open section grows with them, two bytes of type each.
    let at = |seq: u64, boot: u64| entry::build(&entry::Place { seq, boot, prev: entry::genesis(), time: None }, Class::System, &sys(), &Payload::new(category::TEXT, "kernel", "x")).unwrap().len();
    assert!(at(1, 1) < at(1 << 20, 1));
    assert!(at(1 << 20, 1) < at(1 << 40, 1));
    assert_eq!(entry::parse(&entry::build(&entry::Place { seq: u64::MAX >> 1, boot: 7, prev: entry::genesis(), time: None }, Class::System, &sys(), &Payload::new(category::TEXT, "k", "x")).unwrap()).unwrap().place.seq, u64::MAX >> 1);
}

#[test]
fn long_words_split_in_order() {
    let half = 8;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    let long: String = (0..3000).map(|k| alloc::format!("{} é ", k % 10)).collect();
    r.append(&mut m, 1, None, Class::System, &sys(), &Payload::new(category::TEXT, "kernel", &long)).unwrap();
    let items = r.newest(&mut m, usize::MAX);
    let back: String = entries(&items).iter().map(|s| entry::open(s, &sys()).unwrap().words).collect();
    assert!(entries(&items).len() > 1);
    assert_eq!(back, long);
}

#[test]
fn render() {
    let half = 4;
    let mut m = Mem::new(half);
    let (mut r, _) = Ring::open(&mut m, half);
    r.append(&mut m, 931, None, Class::System, &sys(), &Payload::new(category::TEXT, "kernel", "boot cold, flash log init true")).unwrap();
    r.append(&mut m, 931, Some(5), Class::System, &sys(), &Payload::new(category::TEXT, "camera", "on, the main module")).unwrap();
    r.append(&mut m, 931, Some(6), Class::Personal, &person(), &Payload::new(category::PERSON_TEXT, "kernel", "alias given to the kist lumis")).unwrap();
    let mut run = view::state(&view::State { boot: 931, next_seq: 3, block: 0, half, preboot_held: 0, preboot_lost: 2, personal_dropped: 1, write_fails: 0, person: false }, Some(7)).unwrap();
    for i in r.newest(&mut m, usize::MAX) {
        if let Item::Entry { doc, stored, .. } = i {
            run.extend_from_slice(&doc);
            // Nobody present: only the system entries open.
            if let Ok(p) = entry::open(&stored, &sys()) {
                run.extend_from_slice(&view::opened(&stored, &p).unwrap());
            }
        }
    }
    let lines = view::render(&run, &|t| alloc::format!("T{t}"));
    assert_eq!(
        lines,
        vec![
            "ledger at T7: this is b931, next entry A#3, head block A#0 of A#4; pre-boot buffer A#0 held, A#2 overwritten; personal entries dropped (nobody present) A#1; writes failed A#0; a person not present".to_string(),
            "b931 (no clock yet) kernel: boot cold, flash log init true".to_string(),
            "b931 T5 camera: on, the main module".to_string(),
            "b931 T6 (a personal entry, sealed: not opened on this answer)".to_string(),
        ]
    );
    // With the person present the personal entry opens, its category shown.
    let s = entries(&r.newest(&mut m, 1))[0].clone();
    let mut one = Vec::new();
    for i in r.newest(&mut m, 1) {
        if let Item::Entry { doc, .. } = i {
            one.extend_from_slice(&doc);
        }
    }
    one.extend_from_slice(&view::opened(&s, &entry::open(&s, &person()).unwrap()).unwrap());
    assert_eq!(view::render(&one, &|t| alloc::format!("T{t}")), vec!["b931 T6 kernel [person.text]: alias given to the kist lumis".to_string()]);
    // A cut-off answer shows what is whole and where it stops.
    let cut = &run[..run.len() - 10];
    assert!(view::render(cut, &|t| alloc::format!("T{t}")).last().unwrap().starts_with("!! the answer stops being whole"));
}

#[test]
fn preboot_drains_in_order_and_counts_overwrites() {
    let mut b: Preboot<4> = Preboot::new();
    for n in 0..6u64 {
        b.push(Pending { tick: n, category: category::TEXT, writer: "kernel".into(), words: words(n) });
    }
    assert_eq!(b.len(), 4);
    let mut got = Vec::new();
    let lost = b.drain(|p| got.push(p.tick));
    assert_eq!(got, vec![2, 3, 4, 5]);
    assert_eq!(lost, 2);
    assert!(b.is_empty());
    b.push(Pending { tick: 9, category: category::TEXT, writer: "kernel".into(), words: "x".into() });
    let mut got = Vec::new();
    assert_eq!(b.drain(|p| got.push(p.tick)), 0);
    assert_eq!(got, vec![9]);
}

#[test]
fn category_names_are_vsf_names() {
    for c in category::ALL {
        vsf::validate_name(c.name).unwrap();
        assert_eq!(category::find(c.name), Some(c));
    }
    assert_eq!(Class::from_number(Class::Personal.number()), Some(Class::Personal));
}

#[test]
fn legacy_lines_chain() {
    let mut run = Vec::new();
    let mut prev = entry::genesis();
    for n in 0..3u64 {
        let d = view::legacy(n, &prev, 812, 1234 + n, &words(n), Some(1)).unwrap();
        prev = view::hash_of(&d).unwrap();
        run.extend_from_slice(&d);
    }
    assert_eq!(view::check_legacy(&run), Ok(3));
    assert_eq!(view::render(&run, &|_| String::new())[0], alloc::format!("b812 (old log, A#1234 ms after the counter started) {}", words(0)));
    let mut bad = run.clone();
    let k = bad.len() - 20;
    bad[k] ^= 1;
    assert!(view::check_legacy(&bad).is_err());
}

#[test]
fn lean_text_replaces_and_counts() {
    let words = |w: &str| {
        let s = entry::parse(&entry::build(&entry::Place { seq: 0, boot: 1, prev: entry::genesis(), time: None }, Class::System, &sys(), &Payload::new(category::TEXT, "kernel", w)).unwrap()).unwrap();
        let p = entry::open(&s, &sys()).unwrap();
        (p.words, p.refused)
    };
    // In the compact table: as it was.
    assert_eq!(words("camera: on, café, привет"), ("camera: on, café, привет".to_string(), 0));
    // ASCII the table lacks (`^`): stored as ASCII, nothing lost.
    assert_eq!(words("lock held long: holds of 2^-7 s"), ("lock held long: holds of 2^-7 s".to_string(), 0));
    // Outside the table and not ASCII: each such character becomes the marker, and is counted.
    assert_eq!(words("2^10 us → done µs"), ("2?10 us ? done ?s".to_string(), 3));
    // The render says so.
    let place = entry::Place { seq: 0, boot: 1, prev: entry::genesis(), time: None };
    let doc = entry::build(&place, Class::System, &sys(), &Payload::new(category::TEXT, "kernel", "a → b")).unwrap();
    let s = entry::parse(&doc).unwrap();
    let mut run = doc.clone();
    run.extend_from_slice(&view::opened(&s, &entry::open(&s, &sys()).unwrap()).unwrap());
    assert_eq!(view::render(&run, &|_| String::new()), vec!["b1 (no clock yet) kernel: a ? b (A#1 characters outside the text table, shown as ?)".to_string()]);
}

/// Old data in every block (what a region held before the ledger): not bytes the ring can parse.
fn old_data(m: &mut Mem) {
    for (k, b) in m.blocks.iter_mut().enumerate() {
        for (i, x) in b.iter_mut().enumerate() {
            *x = (k as u32).wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503) as u8 | 1;
        }
    }
}

#[test]
fn a_region_of_old_data_is_never_read_whole() {
    // 2026-11-18: the open fell back to reading every block when block 0 held no entry; over a region that held old data that was the whole region at every boot.
    let half = 1u32 << 10;
    let mut m = Mem::new(half);
    old_data(&mut m);
    let (mut r, f) = Ring::open(&mut m, half);
    assert!(r.is_empty());
    assert!(f.iter().any(|f| f.block == 0 && f.why == Why::NotARing), "{f:?}");
    assert!(m.reads <= 2, "the open read A#{} blocks", m.reads);
    // The ring starts afresh at block 0 and writes over the old data as it goes; the next open finds its head without reading the region whole.
    let n = 200u64;
    for i in 0..n {
        put(&mut r, &mut m, i);
    }
    let head = r.head_block();
    assert!(head > 1, "the entries span blocks");
    m.reads = 0;
    let (r2, f2) = Ring::open(&mut m, half);
    assert_eq!(r2.head_block(), head);
    assert_eq!(r2.next_seq(), n);
    assert!(!f2.iter().any(|f| matches!(f.why, Why::Corrupt(_) | Why::NotARing)), "{f2:?}");
    // A binary search (2 copies a probe of up to 2^4 blocks a step) and the chain's few blocks: far below the region's 2^11.
    assert!(m.reads < 1 << 9, "the open read A#{} blocks of A#{}", m.reads, 2 * half);
    let items = r2.newest(&mut m, n as usize);
    assert_eq!(entries(&items).len(), n as usize);
}
