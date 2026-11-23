//! The growing heap (src/arc.rs) over an arena that plays the arc: the kernel's grow and shrink as SYS_ARC answers them, natively (whole frames) and in ring mode (the mask doubles), refused past the ceiling.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};
use ferros_heap::{Arc, ArcHeap};

const MIB: usize = 1 << 20;
const FRAME: usize = 2 * MIB;
/// The kernel's entry stack and the guard page under it (process.rs STACK, STACK_GUARD).
const STACK: usize = 64 << 10;
const GUARD: usize = 4 << 10;

/// An arc in a leaked arena: its top moves as the kernel would move it.
struct Mock {
    base: usize,
    top: AtomicUsize,
    ceiling: usize,
    /// Ring mode: the arc is a power of two, a grow doubles it, a shrink keeps a power of two.
    ring: bool,
    /// The least top the arc may shrink to: what it was spawned with.
    floor: usize,
}

impl Arc for Mock {
    fn base(&self) -> usize {
        self.base
    }
    fn size(&self) -> (usize, usize) {
        (self.top.load(Ordering::SeqCst), self.ceiling)
    }
    fn grow(&self, bytes: usize) -> Option<(usize, usize)> {
        let old = self.top.load(Ordering::SeqCst);
        let new = if self.ring { (old + bytes).next_power_of_two() } else { (old + bytes).next_multiple_of(FRAME) };
        if new > self.ceiling {
            return None;
        }
        // The kernel hands new frames zeroed.
        unsafe { core::ptr::write_bytes((self.base + old) as *mut u8, 0, new - old) };
        self.top.store(new, Ordering::SeqCst);
        Some((new, old))
    }
    fn shrink(&self, keep: usize) -> Option<usize> {
        let old = self.top.load(Ordering::SeqCst);
        let new = if self.ring { keep.next_power_of_two() } else { keep.next_multiple_of(FRAME) };
        if new < self.floor || new >= old {
            return Some(old);
        }
        // What is given back is poisoned: a heap that still used it would read garbage.
        unsafe { core::ptr::write_bytes((self.base + new) as *mut u8, 0xA5, old - new) };
        self.top.store(new, Ordering::SeqCst);
        Some(new)
    }
}

/// The arc as the kernel lays it out at spawn: .bss ends at 1 MiB, then 4 MiB of heap, the input (`input_len` bytes), the slack, the guard page and the entry stack under `top`.
struct Lay {
    heap: &'static ArcHeap<Mock>,
    base: usize,
    bss_end: usize,
    input: usize,
    input_len: usize,
}

fn spawn(top: usize, ceiling: usize, ring: bool, input_len: usize) -> Lay {
    // The window: the ceiling, aligned to itself as the kernel's buddy gives it.
    let arena = Box::leak(vec![0u8; ceiling * 2].into_boxed_slice());
    let base = (arena.as_ptr() as usize + ceiling - 1) & !(ceiling - 1);
    let mock = Mock { base, top: AtomicUsize::new(top), ceiling, ring, floor: top };
    let heap = Box::leak(Box::new(ArcHeap::new(mock)));
    let bss_end = base + MIB;
    let input = 5 * MIB;
    unsafe { heap.init(bss_end, input, input_len) };
    Lay { heap, base, bss_end, input, input_len }
}

fn lay(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

#[test]
fn starts_with_the_tail_and_the_slack_around_the_input() {
    let top = 8 * MIB;
    let l = spawn(top, 64 * MIB, false, 100);
    // The tail up to the input, the slack from its end up to the guard page.
    assert_eq!(l.heap.held(), (l.base + l.input - l.bss_end) + (top - STACK - GUARD - (l.input + l.input_len)));
    let (input_lo, input_hi) = (l.base + l.input, l.base + l.input + l.input_len);
    let reserved = l.base + top - STACK - GUARD;
    let mut got = Vec::new();
    loop {
        let p = unsafe { l.heap.alloc(lay(4096, 16)) };
        let a = p as usize;
        if a >= l.base + top {
            unsafe { l.heap.dealloc(p, lay(4096, 16)) };
            break;
        }
        assert!(a >= l.bss_end && a + 4096 <= reserved, "an allocation at G#{a:X} lies outside the heap's start");
        assert!(a + 4096 <= input_lo || a >= input_hi, "an allocation overlaps the input");
        got.push(p);
    }
    assert!(got.len() > 1500, "the start held only {} pages", got.len());
    assert_eq!(l.heap.grow_counts().0, 1, "the first allocation past the start grows the arc once");
    for p in got {
        unsafe { l.heap.dealloc(p, lay(4096, 16)) };
    }
}

#[test]
fn grows_past_the_start_and_is_refused_only_at_the_ceiling() {
    let top = 8 * MIB;
    let ceiling = 64 * MIB;
    let l = spawn(top, ceiling, false, 0);
    let mut held = Vec::new();
    let big = lay(3 * MIB, 16);
    loop {
        let p = unsafe { l.heap.alloc(big) };
        if p.is_null() {
            break;
        }
        // The memory is the process's: written whole, read back.
        unsafe {
            core::ptr::write_bytes(p, held.len() as u8, big.size());
            assert_eq!(*p.add(big.size() - 1), held.len() as u8);
        }
        assert!((p as usize) + big.size() <= l.base + ceiling);
        held.push(p);
    }
    // 64 MiB less the spawn's 8 holds 18 blocks of 3 MiB above the start, besides the one or two the start holds.
    assert!(held.len() >= 18, "{} blocks of 3 MiB in a 64 MiB arc", held.len());
    let (grows, refused) = l.heap.grow_counts();
    assert!(grows >= 2 && refused >= 1);
    assert_eq!(l.heap.arc_size().0 <= ceiling, true);
    for (k, &p) in held.iter().enumerate() {
        assert_eq!(unsafe { *p }, k as u8, "block {k} was overwritten");
    }
    for p in held {
        unsafe { l.heap.dealloc(p, big) };
    }
}

#[test]
fn gives_the_top_back_when_it_is_free() {
    let top = 8 * MIB;
    let l = spawn(top, 256 * MIB, false, 0);
    // A heap that grew by many 4 MiB buffers (ferros_rt's policy keeps a cushion of the largest grow, so one big buffer freed is kept for the next).
    let piece = lay(4 * MIB, 16);
    let held: Vec<_> = (0..32).map(|_| unsafe { l.heap.alloc(piece) }).collect();
    assert!(held.iter().all(|p| !p.is_null()));
    let grown = l.heap.arc_size().0;
    assert!(grown >= top + 120 * MIB);
    let before = l.heap.held();
    for p in held.into_iter().rev() {
        unsafe { l.heap.dealloc(p, piece) };
    }
    let after = l.heap.arc_size().0;
    assert!(after < grown / 2, "the arc kept G#{after:X} of G#{grown:X} after everything was freed");
    assert!(l.heap.held() < before);
    // The cushion keeps the largest grow: the same allocation again needs no grow.
    let grows = l.heap.grow_counts().0;
    let q = unsafe { l.heap.alloc(piece) };
    assert!(!q.is_null());
    assert_eq!(l.heap.grow_counts().0, grows, "the cushion did not hold the largest grow");
    unsafe { l.heap.dealloc(q, piece) };
}

#[test]
fn ring_mode_doubles() {
    let top = 8 * MIB;
    let ceiling = 128 * MIB;
    let l = spawn(top, ceiling, true, 0);
    let big = lay(20 * MIB, 16);
    let p = unsafe { l.heap.alloc(big) };
    assert!(!p.is_null());
    let t = l.heap.arc_size().0;
    assert!(t.is_power_of_two() && t >= top + 20 * MIB);
    unsafe { core::ptr::write_bytes(p, 7, big.size()) };
    unsafe { l.heap.dealloc(p, big) };
    assert!(l.heap.arc_size().0.is_power_of_two());
    // Past the ceiling: refused, null, nothing else changed.
    let huge = lay(200 * MIB, 16);
    assert!(unsafe { l.heap.alloc(huge) }.is_null());
    let q = unsafe { l.heap.alloc(lay(MIB, 16)) };
    assert!(!q.is_null());
    unsafe { l.heap.dealloc(q, lay(MIB, 16)) };
}

#[test]
fn realloc_keeps_the_bytes_across_grows() {
    let l = spawn(8 * MIB, 256 * MIB, false, 0);
    let mut n = 64;
    let mut p = unsafe { l.heap.alloc(lay(n, 8)) };
    for k in 0..n {
        unsafe { *p.add(k) = k as u8 };
    }
    // A Vec doubling to 64 MiB: thru the start and several grows.
    while n < 64 * MIB {
        let m = n * 2;
        p = unsafe { l.heap.realloc(p, lay(n, 8), m) };
        assert!(!p.is_null(), "realloc to {m} failed");
        for k in n..m {
            unsafe { *p.add(k) = k as u8 };
        }
        n = m;
    }
    for k in (0..n).step_by(4093) {
        assert_eq!(unsafe { *p.add(k) }, k as u8);
    }
    unsafe { l.heap.dealloc(p, lay(n, 8)) };
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
}

#[test]
fn threads_grow_and_shrink_it_together() {
    let l = spawn(8 * MIB, 512 * MIB, false, 0);
    let heap = l.heap;
    let threads: Vec<_> = (0..4u64)
        .map(|t| {
            std::thread::spawn(move || {
                let mut r = Rng(t + 1);
                let mut live: Vec<(usize, usize, u8)> = Vec::new();
                for i in 0..20_000 {
                    if live.len() < 64 && (r.next() & 1 == 0 || live.is_empty()) {
                        // Mostly small, now and then a few MiB, so the arc grows and gives back.
                        let size = if r.next() % 64 == 0 { (1 + r.next() as usize % 4) * MIB } else { 16 + r.next() as usize % 4096 };
                        let p = unsafe { heap.alloc(lay(size, 16)) };
                        assert!(!p.is_null(), "thread {t} op {i}: {size} bytes refused");
                        let tag = r.next() as u8;
                        unsafe {
                            *p = tag;
                            *p.add(size - 1) = tag;
                        }
                        live.push((p as usize, size, tag));
                    } else {
                        let k = r.next() as usize % live.len();
                        let (p, size, tag) = live.swap_remove(k);
                        let p = p as *mut u8;
                        unsafe {
                            assert_eq!((*p, *p.add(size - 1)), (tag, tag), "thread {t}: a block was overwritten");
                            heap.dealloc(p, lay(size, 16));
                        }
                    }
                }
                for (p, size, _) in live {
                    unsafe { heap.dealloc(p as *mut u8, lay(size, 16)) };
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert!(heap.grow_counts().0 >= 1);
    // Everything freed: the whole start and every grow is one free run per region again.
    let (free, blocks, _) = heap.try_stats().unwrap();
    assert!(blocks <= 3, "{blocks} free blocks after everything was freed");
    assert!(free + (1 << 10) >= heap.held() - 64, "free {free} of held {}", heap.held());
}
