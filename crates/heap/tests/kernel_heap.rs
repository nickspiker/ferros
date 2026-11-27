//! The kernel's heap (`LockedTlsf`) as the kernel lays it out: two regions apart, 12 MiB and 10 MiB (main.rs `HEAP_SIZE`, cachemap `HEAP2`), and as the kernel uses it.
//! Nothing here asks for more than the heap can give: the refusals, whose record is one per binary, are in kernel_fail.rs.
use core::alloc::{GlobalAlloc, Layout};
use ferros_heap::kernel::LockedTlsf;

const MIB: usize = 1 << 20;
const FIRST: usize = 12 * MIB;
const SECOND: usize = 10 * MIB;
/// The gap between the two regions: they are never neighbours, as on the device.
const GAP: usize = 1 << 16;

/// The kernel's two regions in a leaked arena: (heap, first region's start, second's).
fn heap() -> (&'static LockedTlsf, usize, usize) {
    let arena = Box::leak(vec![0u8; FIRST + GAP + SECOND + 4096].into_boxed_slice());
    let a = (arena.as_ptr() as usize + 0xFFF) & !0xFFF;
    let b = a + FIRST + GAP;
    let h = Box::leak(Box::new(LockedTlsf::empty()));
    unsafe {
        h.add(a, FIRST);
        h.add(b, SECOND);
    }
    (h, a, b)
}

fn lay(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

/// SplitMix64, deterministic.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() as usize) % (hi - lo)
    }
}

#[test]
fn two_regions_reported_and_walked() {
    let (h, a, b) = heap();
    let r = h.report().unwrap();
    assert_eq!(r.regions, 2);
    assert_eq!(r.region[0], (a, a + FIRST));
    assert_eq!(r.region[1], (b, b + SECOND));
    assert_eq!(r.size, FIRST + SECOND);
    // Fresh: one free block a region, each short of its region by its header and its sentinel.
    assert_eq!(r.free_blocks, 2);
    assert!(r.free > FIRST + SECOND - 64 && r.free < FIRST + SECOND);
    assert!(r.largest > FIRST - 64 && r.largest < FIRST);
    assert_eq!(h.lock().verify(), (r.free_blocks, r.free));
    // The levels sum to the lists.
    let (mut blocks, mut bytes) = (0, 0);
    h.lock().levels(|_, n, s| {
        blocks += n;
        bytes += s;
    });
    assert_eq!((blocks, bytes), (r.free_blocks, r.free));
}

#[test]
fn a_block_too_big_for_the_first_region_lands_in_the_second() {
    let (h, _, b) = heap();
    // 11 MiB fills most of the first region; the next 8 MiB fits only the second.
    let x = unsafe { h.alloc(lay(11 * MIB, 16)) };
    let y = unsafe { h.alloc(lay(8 * MIB, 16)) };
    assert!(!x.is_null() && !y.is_null());
    assert!((y as usize) >= b && (y as usize) + 8 * MIB <= b + SECOND);
    unsafe {
        h.dealloc(x, lay(11 * MIB, 16));
        h.dealloc(y, lay(8 * MIB, 16));
    }
    let r = h.report().unwrap();
    assert_eq!(r.free_blocks, 2, "every free merges back to one block a region");
    h.lock().verify();
}

/// A Vec that only grows (the kernel's console buffer between two `diag console` reads, a vault get's reply): by doubling, from 4 KiB to 8 MiB, with a small block taken after every step as other callers would. In place every time the block behind it is free, so it never needs its old and new sizes at once: the free list's realloc (a new block, a copy, the old freed) needs 4 + 8 MiB in one region for the last step.
#[test]
fn a_doubling_vec_grows_in_place() {
    let (h, _, _) = heap();
    let mut size = 1 << 12;
    let p0 = unsafe { h.alloc(lay(size, 1)) };
    unsafe { core::ptr::write_bytes(p0, 0xA5, size) };
    let mut p = p0;
    let mut moved = 0;
    while size < 8 * MIB {
        let q = unsafe { h.realloc(p, lay(size, 1), size * 2) };
        assert!(!q.is_null(), "growing to {} failed", size * 2);
        if q != p {
            moved += 1;
        }
        // What was there came along.
        assert!(unsafe { core::slice::from_raw_parts(q, size) }.iter().all(|&x| x == 0xA5));
        unsafe { core::ptr::write_bytes(q.add(size), 0xA5, size) };
        p = q;
        size *= 2;
    }
    assert_eq!(moved, 0, "the first block had the whole region behind it");
    unsafe { h.dealloc(p, lay(size, 1)) };
    h.lock().verify();
}

/// The same doubling with a small long-lived block taken after each step, which may land right behind the Vec: then it moves (the old and new blocks both live during the copy), and its bytes come along. kernel_fail.rs shows where a moving doubling stops.
#[test]
fn a_doubling_vec_with_a_neighbour_moves_and_keeps_its_bytes() {
    let (h, _, _) = heap();
    let mut size = 1 << 12;
    let mut p = unsafe { h.alloc(lay(size, 1)) };
    unsafe { core::ptr::write_bytes(p, 0x3C, size) };
    let mut pins = Vec::new();
    while size < 2 * MIB {
        pins.push(unsafe { h.alloc(lay(64, 16)) });
        let q = unsafe { h.realloc(p, lay(size, 1), size * 2) };
        assert!(!q.is_null());
        assert!(unsafe { core::slice::from_raw_parts(q, size) }.iter().all(|&x| x == 0x3C));
        unsafe { core::ptr::write_bytes(q.add(size), 0x3C, size) };
        p = q;
        size *= 2;
    }
    unsafe { h.dealloc(p, lay(size, 1)) };
    for q in pins {
        unsafe { h.dealloc(q, lay(64, 16)) };
    }
    assert_eq!(h.report().unwrap().free_blocks, 2);
    h.lock().verify();
}

/// The kernel's load, shaped: small blocks of interleaved lifetimes (process tables, endpoint messages of up to 4 KiB, capability lists, flash log lines) churning beside the vault's pieces (`ferros_pieces::PIECE`, 512 KiB: a sealed piece, its staging and the piece it replaces) and a capsule read whole now and then. With pieces, nothing needs more than one piece in one block, so no interleaving of lifetimes can refuse it while the heap holds a few of them free.
#[test]
fn churn_beside_pieces_never_refuses() {
    let (h, _, _) = heap();
    let mut rng = Rng(0xF3_2026_1112);
    let mut live: Vec<(*mut u8, Layout)> = Vec::new();
    let piece = 1 << 19;
    for round in 0..(1 << 12) {
        // Small churn: about 4 MiB live on average.
        for _ in 0..(1 << 4) {
            if live.len() < 1 << 11 || rng.next() & 1 == 0 {
                let l = lay(rng.range(16, 4096), [1usize, 8, 16, 64][rng.range(0, 4)]);
                let p = unsafe { h.alloc(l) };
                assert!(!p.is_null(), "a small block refused at round {round}");
                live.push((p, l));
            } else {
                let (p, l) = live.swap_remove(rng.range(0, live.len()));
                unsafe { h.dealloc(p, l) };
            }
        }
        // A vault put's piece: the sealed piece, its staging (a little more than a piece), and the old piece read back, all live at once.
        let ls = [lay(piece + 40, 1), lay(piece + (piece >> 6), 8), lay(piece, 1)];
        let ps: Vec<*mut u8> = ls.iter().map(|&l| unsafe { h.alloc(l) }).collect();
        assert!(ps.iter().all(|p| !p.is_null()), "a piece refused at round {round}: {:?}", h.report());
        for (p, l) in ps.into_iter().zip(ls) {
            unsafe { h.dealloc(p, l) };
        }
        if round & 0xFF == 0 {
            h.lock().verify();
        }
    }
    for (p, l) in live {
        unsafe { h.dealloc(p, l) };
    }
    assert_eq!(h.report().unwrap().free_blocks, 2);
}

/// Alignments past TLSF's 16-byte grain (a page-aligned table, a cache-line one) are honoured, and their gaps come back.
#[test]
fn wide_alignments() {
    let (h, _, _) = heap();
    let mut held = Vec::new();
    for &align in &[16usize, 64, 1 << 12, 1 << 16] {
        for size in [1usize, 100, 5000, 70_000] {
            let l = lay(size, align);
            let p = unsafe { h.alloc(l) };
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0);
            held.push((p, l));
        }
    }
    for (p, l) in held {
        unsafe { h.dealloc(p, l) };
    }
    assert_eq!(h.report().unwrap().free_blocks, 2);
    h.lock().verify();
}
