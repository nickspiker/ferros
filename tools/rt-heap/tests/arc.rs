//! alloc.rs as std calls it, over the fake arc, with threads shaped like Mahere's (2026-11-11): two drawing threads (hundreds of short allocations a frame, Vecs that double), three loaders (cell buffers of 64 KiB to 2 MiB, and the small long-lived allocations a cell's labels leave, which fragment), a vault thread freeing what the loaders hand it (frees from another thread), and one that takes 8-32 MiB now and then (the arc grows, and shrinks when it is freed).
//! A shadow map says no byte is handed out twice; patterns say no block is written by another; frames given back are unreadable, so a heap that touched one would fault the test.

use rt_heap::{Rng, alloc, layout, sys};
use std::alloc::Layout;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

static SHADOW: Mutex<BTreeMap<usize, usize>> = Mutex::new(BTreeMap::new());
/// Blocks a loader hands the vault thread: (address, size, tag).
static QUEUE: Mutex<Vec<(usize, usize, u64)>> = Mutex::new(Vec::new());
static PEAK_TOP: AtomicUsize = AtomicUsize::new(0);
static OPS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct Block {
    p: usize,
    size: usize,
    align: usize,
    tag: u64,
}

fn marks(size: usize) -> impl Iterator<Item = usize> {
    let head = 0..size.min(32);
    let tail = size.saturating_sub(32).max(32)..size;
    let mid = (4096..size).step_by(4096);
    head.chain(tail).chain(mid)
}

fn take(rng: &mut Rng, size: usize, align: usize) -> Block {
    let p = unsafe { alloc::alloc(Layout::from_size_align(size, align).unwrap()) } as usize;
    assert!(p != 0, "the heap refused {size} bytes with a gigabyte's ceiling");
    assert_eq!(p % align, 0);
    // Claimed after the heap answers, released before the heap is told: two live claims on one byte is a block handed out twice.
    {
        let mut m = SHADOW.lock().unwrap();
        if let Some((&q, &n)) = m.range(..p + size).next_back() {
            assert!(q + n <= p, "{p:#x}+{size} handed out over the live {q:#x}+{n}");
        }
        m.insert(p, size);
    }
    let b = Block { p, size, align, tag: rng.next() };
    for i in marks(size) {
        unsafe { *((p + i) as *mut u8) = (b.tag as u8) ^ (i as u8) };
    }
    let top = sys::top();
    PEAK_TOP.fetch_max(top, Ordering::Relaxed);
    OPS.fetch_add(1, Ordering::Relaxed);
    b
}

fn check(b: &Block, limit: usize) {
    for i in marks(b.size).filter(|&i| i < limit) {
        let got = unsafe { *((b.p + i) as *const u8) };
        assert_eq!(got, (b.tag as u8) ^ (i as u8), "{:#x}+{} overwritten at {i}", b.p, b.size);
    }
}

fn give(b: Block) {
    check(&b, b.size);
    assert_eq!(SHADOW.lock().unwrap().remove(&b.p), Some(b.size));
    unsafe { alloc::dealloc(b.p as *mut u8, Layout::from_size_align(b.size, b.align).unwrap()) };
    OPS.fetch_add(1, Ordering::Relaxed);
}

/// realloc as std's Vec does it: the old pattern kept up to the smaller size, the new one stamped.
fn grow_vec(rng: &mut Rng, b: Block, new: usize) -> Block {
    check(&b, b.size);
    assert_eq!(SHADOW.lock().unwrap().remove(&b.p), Some(b.size));
    let q = unsafe { alloc::realloc(b.p as *mut u8, Layout::from_size_align(b.size, b.align).unwrap(), new) } as usize;
    assert!(q != 0);
    let kept = Block { p: q, ..b };
    check(&kept, new.min(b.size));
    {
        let mut m = SHADOW.lock().unwrap();
        if let Some((&s, &n)) = m.range(..q + new).next_back() {
            assert!(s + n <= q, "realloc to {q:#x}+{new} over the live {s:#x}+{n}");
        }
        m.insert(q, new);
    }
    let c = Block { p: q, size: new, align: b.align, tag: rng.next() };
    for i in marks(new) {
        unsafe { *((q + i) as *mut u8) = (c.tag as u8) ^ (i as u8) };
    }
    OPS.fetch_add(1, Ordering::Relaxed);
    c
}

fn draw(seed: u64, frames: usize) {
    let mut rng = Rng(seed);
    for _ in 0..frames {
        let mut live = Vec::with_capacity(1 << 10);
        for _ in 0..570 / 2 {
            let size = rng.log_size(16, 4096);
            let align = 1 << rng.range(0, 5);
            live.push(take(&mut rng, size, align));
            if rng.range(0, 4) == 0 {
                let i = rng.range(0, live.len());
                give(live.swap_remove(i));
            }
        }
        // A Vec that doubles.
        let mut v = take(&mut rng, 16, 8);
        while v.size < 1 << 14 {
            let n = v.size * 2;
            v = grow_vec(&mut rng, v, n);
        }
        live.push(v);
        for b in live {
            give(b);
        }
    }
}

fn loader(seed: u64, cells: usize) {
    let mut rng = Rng(seed);
    let mut buffers: Vec<Block> = Vec::new();
    let mut labels: Vec<Block> = Vec::new();
    for _ in 0..cells {
        let size = rng.log_size(64 << 10, 2 << 20);
        let b = take(&mut rng, size, 16);
        if rng.range(0, 2) == 0 {
            // Still claimed in the shadow map until the vault thread frees it.
            QUEUE.lock().unwrap().push((b.p, b.size, b.tag));
        } else {
            buffers.push(b);
        }
        for _ in 0..64 {
            let size = rng.log_size(8, 256);
            labels.push(take(&mut rng, size, 8));
        }
        // Old cells and old labels go, out of order: what is left between the kept ones is the fragmentation.
        while buffers.len() > 16 {
            let i = rng.range(0, buffers.len());
            give(buffers.swap_remove(i));
        }
        while labels.len() > 1 << 13 {
            let i = rng.range(0, labels.len());
            give(labels.swap_remove(i));
        }
    }
    for b in buffers.into_iter().chain(labels) {
        give(b);
    }
}

fn vault(seed: u64, until: &AtomicUsize) {
    let mut rng = Rng(seed);
    loop {
        let job = QUEUE.lock().unwrap().pop();
        match job {
            Some((p, size, tag)) => {
                // A block freed by another thread than the one that took it.
                give(Block { p, size, align: 16, tag });
            }
            None if until.load(Ordering::Acquire) == 0 => break,
            None => {
                let size = rng.log_size(4 << 10, 64 << 10);
                let t = take(&mut rng, size, 16);
                give(t);
            }
        }
    }
}

fn big(seed: u64, times: usize) {
    let mut rng = Rng(seed);
    for _ in 0..times {
        let size = rng.log_size(8 << 20, 32 << 20);
        let b = take(&mut rng, size, 1 << 12);
        std::thread::yield_now();
        give(b);
    }
}

#[test]
fn mahere_shaped_threads_over_a_growing_and_shrinking_arc() {
    // Spawned with 4 MiB as the kernel maps a growing heap, the image's first 64 KiB, its input at 2 MiB, the entry stack and its guard at the top; a gigabyte's ceiling.
    let base = sys::init(4 << 20, 1 << 30);
    for (s, e) in layout::heap_regions(base + (64 << 10), base + (2 << 20), base + (2 << 20) + 4096, base + (4 << 20)) {
        if e > s {
            unsafe { alloc::add_region(s, e - s) };
        }
    }
    let loaders_running = AtomicUsize::new(3);
    std::thread::scope(|s| {
        for i in 0..2 {
            s.spawn(move || draw(0xD7A0 + i, 256));
        }
        for i in 0..3 {
            let lr = &loaders_running;
            s.spawn(move || {
                loader(0x10AD + i, 768);
                lr.fetch_sub(1, Ordering::Release);
            });
        }
        let lr = &loaders_running;
        s.spawn(move || vault(0x7A17, lr));
        s.spawn(|| big(0xB16, 64));
    });
    assert!(SHADOW.lock().unwrap().is_empty());
    let (grows, shrinks) = (sys::GROWS.load(Ordering::Relaxed), sys::SHRINKS.load(Ordering::Relaxed));
    let (peak, end) = (PEAK_TOP.load(Ordering::Relaxed), sys::top());
    eprintln!("arc: {} heap calls; {grows} grows, {shrinks} shrinks; the arc's top peaked at {} MiB and ends at {} MiB; {} free blocks, {} MiB free, the largest {} MiB", OPS.load(Ordering::Relaxed), peak >> 20, end >> 20, alloc::free_blocks(), alloc::free_bytes() >> 20, alloc::largest_free() >> 20);
    assert!(grows > 0 && shrinks > 0, "the arc should have grown and given back");
    assert!(end < peak, "everything freed, the arc should be under its peak");
    // Everything freed: the two spawn regions and the grown one, each one free block.
    assert_eq!(alloc::free_blocks(), 3);
}
