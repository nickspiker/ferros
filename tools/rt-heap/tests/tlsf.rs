//! tlsf.rs alone over memory from the host: random allocs, frees and reallocs from 1 byte to 32 MiB at alignments up to 64 KiB, a shadow map that no byte is handed out twice, patterns that no block is written by another, and the heap's own structure walked as it goes.

use rt_heap::Rng;
use rt_heap::tlsf::{HEADER, MIN_BLOCK, Tlsf};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;

/// Memory from the host for a heap: `bytes` at a page.
fn arena(bytes: usize) -> usize {
    let p = unsafe { System.alloc(Layout::from_size_align(bytes, 1 << 12).unwrap()) };
    assert!(!p.is_null());
    p as usize
}

/// A live block: its layout and the tag its pattern was written with.
struct Live {
    size: usize,
    align: usize,
    tag: u64,
}

/// The bytes a block's pattern touches: its first and last 64, and one each 4 KiB between (a full write of 32 MiB blocks would be the test's whole time).
fn marks(size: usize) -> impl Iterator<Item = usize> {
    let head = 0..size.min(64);
    let tail = size.saturating_sub(64).max(64)..size;
    let mid = (4096..size).step_by(4096);
    head.chain(tail).chain(mid)
}

fn stamp(p: usize, l: &Live) {
    for i in marks(l.size) {
        unsafe { *((p + i) as *mut u8) = (l.tag as u8) ^ (i as u8) ^ ((i >> 12) as u8) };
    }
}

/// The pattern's marks under `limit` are intact (a block cut in place keeps only its head).
fn check(p: usize, l: &Live, limit: usize) {
    for i in marks(l.size).filter(|&i| i < limit) {
        let got = unsafe { *((p + i) as *const u8) };
        assert_eq!(got, (l.tag as u8) ^ (i as u8) ^ ((i >> 12) as u8), "block {p:#x} (tag {}) overwritten at {i}", l.tag);
    }
}

/// The shadow map: no live block may overlap [p, p + size).
fn claim(map: &mut BTreeMap<usize, Live>, p: usize, l: Live) {
    if let Some((&q, other)) = map.range(..p + l.size).next_back() {
        assert!(q + other.size <= p, "{p:#x}+{} overlaps the live {q:#x}+{}", l.size, other.size);
    }
    assert!(map.insert(p, l).is_none());
}

/// A size and an alignment as programs ask: mostly small, some large, a few huge; mostly the grain or less, a few page and beyond.
fn layout(rng: &mut Rng) -> (usize, usize) {
    let r = rng.range(0, 1 << 10);
    let size = if r < 600 {
        rng.log_size(1, 512)
    } else if r < 900 {
        rng.log_size(512, 64 << 10)
    } else if r < 1016 {
        rng.log_size(64 << 10, 4 << 20)
    } else {
        rng.log_size(4 << 20, 32 << 20)
    };
    let a = rng.range(0, 1 << 6);
    let align = if a < 56 { 1 << rng.range(0, 5) } else if a < 62 { 1 << rng.range(5, 13) } else { 1 << 16 };
    (size, align)
}

fn verify(h: &Tlsf, regions: &[usize], ends: &[usize]) {
    let (blocks, bytes) = h.verify_lists();
    let (mut b, mut n) = (0, 0);
    for (&s, &e) in regions.iter().zip(ends) {
        let (end, fb, fbytes) = unsafe { h.verify_region(s) };
        assert_eq!(end, e, "the region from {s:#x} ends at {end:#x}, not {e:#x}");
        b += fb;
        n += fbytes;
    }
    assert_eq!((b, n), (blocks, bytes), "the regions' free blocks are not the lists'");
}

#[test]
fn random_ops_with_a_shadow_map() {
    let mut h = Tlsf::empty();
    // Two regions apart, the second extended later by a third that starts where it ends (as an arc's grows do).
    let (a, b) = (arena(64 << 20), arena(512 << 20));
    assert!(unsafe { h.add(a, a + (64 << 20)) });
    assert!(unsafe { h.add(b, b + (256 << 20)) });
    let mut regions = vec![a, b];
    let mut ends = vec![a + (64 << 20), b + (256 << 20)];
    let all = (64 << 20) + (256 << 20) - 2 * HEADER;
    assert_eq!(h.free_bytes(), all);
    let mut rng = Rng(0xF3_1105_CAFE_D00D);
    let mut map: BTreeMap<usize, Live> = BTreeMap::new();
    let (mut tag, mut nulls, mut moved, mut in_place) = (0u64, 0, 0, 0);
    let mut extended = false;
    for op in 0..(1 << 18) {
        if op == 1 << 17 {
            // The grow: [b + 256 MiB, b + 512 MiB) joins the second region.
            assert!(unsafe { h.add(b + (256 << 20), b + (512 << 20)) });
            ends[1] = b + (512 << 20);
            extended = true;
        }
        let r = rng.range(0, 100);
        if map.is_empty() || r < 50 {
            let (size, align) = layout(&mut rng);
            let p = unsafe { h.alloc(Layout::from_size_align(size, align).unwrap()) } as usize;
            if p == 0 {
                nulls += 1;
                continue;
            }
            assert_eq!(p % align, 0, "{size} bytes at {align} came back at {p:#x}");
            assert_eq!(p % 16, 0);
            tag += 1;
            let l = Live { size, align, tag };
            stamp(p, &l);
            claim(&mut map, p, l);
        } else {
            // A live block at random: the first at or past a random address.
            let (lo, hi) = (*map.first_key_value().unwrap().0, *map.last_key_value().unwrap().0);
            let at = rng.range(lo, hi + 1);
            let p = *map.range(at..).next().unwrap().0;
            let l = map.remove(&p).unwrap();
            check(p, &l, l.size);
            if r < 85 {
                unsafe { h.free(p as *mut u8) };
            } else {
                // realloc as alloc.rs does it: in place, else a new block, the copy (here the pattern again), the old one freed.
                let (new, _) = layout(&mut rng);
                let (done, _) = unsafe { h.resize(p as *mut u8, new) };
                let q = if done {
                    in_place += 1;
                    p
                } else {
                    let q = unsafe { h.alloc(Layout::from_size_align(new, l.align).unwrap()) } as usize;
                    if q == 0 {
                        nulls += 1;
                        claim(&mut map, p, l);
                        continue;
                    }
                    unsafe { h.free(p as *mut u8) };
                    moved += 1;
                    q
                };
                assert_eq!(q % l.align, 0);
                // In place, what was kept is still the old pattern.
                if done {
                    check(q, &l, new.min(l.size));
                }
                tag += 1;
                let l = Live { size: new, align: l.align, tag };
                stamp(q, &l);
                claim(&mut map, q, l);
            }
        }
        if op % (1 << 12) == 0 {
            verify(&h, &regions, &ends);
            let live: usize = map.values().map(|l| l.size).sum();
            let total = (64 << 20) + if extended { 512 << 20 } else { 256 << 20 } - 2 * HEADER;
            assert!(h.free_bytes() + live <= total);
        }
    }
    eprintln!("tlsf: {tag} blocks handed out, {nulls} refused (full), {in_place} resized in place, {moved} moved; {} live at the end, {} free blocks", map.len(), h.free_blocks());
    for (p, l) in std::mem::take(&mut map) {
        check(p, &l, l.size);
        unsafe { h.free(p as *mut u8) };
    }
    verify(&h, &regions, &ends);
    // Everything back: one free block a region (the extension merged into the second), every byte but the sentinels'.
    assert_eq!(h.free_blocks(), 2);
    assert_eq!(h.free_bytes(), (64 << 20) + (512 << 20) - 2 * HEADER);
    regions.clear();
}

#[test]
fn every_alignment_and_the_top() {
    let mut h = Tlsf::empty();
    let a = arena(16 << 20);
    assert!(unsafe { h.add(a, a + (16 << 20)) });
    let mut ps = Vec::new();
    for shift in 0..17 {
        for size in [1usize, 15, 16, 17, 100, 4096, 70000] {
            let l = Layout::from_size_align(size, 1 << shift).unwrap();
            let p = unsafe { h.alloc(l) } as usize;
            assert!(p != 0 && p % (1 << shift) == 0);
            ps.push(p);
        }
    }
    h.verify_lists();
    unsafe { h.verify_region(a) };
    for p in ps {
        unsafe { h.free(p as *mut u8) };
    }
    assert_eq!(h.free_blocks(), 1);
    // The top: the whole region is one free block that reaches it; cut to a megabyte, then back.
    let b = h.take_top().unwrap();
    assert_eq!(b, a);
    assert_eq!(h.free_blocks(), 0);
    unsafe { h.cut(b, a + (1 << 20)) };
    assert_eq!(h.free_bytes(), (1 << 20) - HEADER);
    assert_eq!(h.top(), a + (1 << 20));
    unsafe { h.verify_region(a) };
    // A region added at the new top extends it again.
    assert!(unsafe { h.add(a + (1 << 20), a + (16 << 20)) });
    assert_eq!(h.free_blocks(), 1);
    assert_eq!(h.free_bytes(), (16 << 20) - HEADER);
    // Full: null, not a panic; past the largest class: null.
    assert!(unsafe { h.alloc(Layout::from_size_align(32 << 20, 16).unwrap()) }.is_null());
    assert!(unsafe { h.alloc(Layout::from_size_align(1 << 40, 16).unwrap()) }.is_null());
    // Slivers are no region.
    let mut s = Tlsf::empty();
    assert!(!unsafe { s.add(a, a + MIN_BLOCK) });
}

#[test]
fn realloc_in_place_grows_into_free_space_and_shrinks() {
    let mut h = Tlsf::empty();
    let a = arena(1 << 20);
    assert!(unsafe { h.add(a, a + (1 << 20)) });
    let p = unsafe { h.alloc(Layout::from_size_align(100, 8).unwrap()) };
    // The rest of the region lies behind it, free: a Vec doubling there never moves.
    let mut size = 100;
    while size < 512 << 10 {
        size *= 2;
        // Done, and the tail it cut off is the free block at the top.
        assert_eq!(unsafe { h.resize(p, size) }, (true, true));
    }
    // A cut that frees a tail reaching the top says so.
    assert_eq!(unsafe { h.resize(p, 64) }, (true, true));
    let q = unsafe { h.alloc(Layout::from_size_align(64, 8).unwrap()) };
    // With a used block behind it, a grow cannot be done in place.
    assert_eq!(unsafe { h.resize(p, 4096) }, (false, false));
    unsafe {
        h.free(q);
        h.free(p);
    }
    assert_eq!(h.free_blocks(), 1);
}
