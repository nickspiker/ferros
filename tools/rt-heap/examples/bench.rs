//! The heap measured against the free list it replaced (ferros_heap, the copy ferros_rt carried until 2026-11-11, behind its spinlock as alloc.rs held it) and the host's malloc for scale.
//!
//! 0. One thread, the heap fragmented to 0, 20 000 and 50 000 free blocks (pairs of small blocks, every other one freed), then a ring of 512 live blocks of 16 B-4 KiB churned: ns an operation (an alloc or a free).
//! 1. Mahere's shape on four cores: eight threads (a drawing thread, three loaders, three label threads, a vault thread), each writing what it takes, for four seconds, on the heaps part 0 left with 50 000 fragments: the drawing thread's heap time a frame of 570 calls, everyone's operations, and the process's CPU time.
//!
//! taskset -c 0-3 cargo run --release --example bench

use rt_heap::{Rng, alloc as rt, sys};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

trait Heap: Sync {
    fn name(&self) -> &'static str;
    unsafe fn alloc(&self, l: Layout) -> *mut u8;
    unsafe fn free(&self, p: *mut u8, l: Layout);
    fn blocks(&self) -> usize;
}

/// The old heap as ferros_rt's alloc.rs used it: the lock, the walk.
struct Old(ferros_heap::LockedHeap);
impl Heap for Old {
    fn name(&self) -> &'static str {
        "free list + spinlock (old)"
    }
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { self.0.lock().alloc(l) }
    }
    unsafe fn free(&self, p: *mut u8, l: Layout) {
        unsafe { self.0.lock().dealloc(p, l) }
    }
    fn blocks(&self) -> usize {
        self.0.lock().free_blocks()
    }
}

/// ferros_rt's alloc.rs, as std calls it, over the fake arc.
struct New;
impl Heap for New {
    fn name(&self) -> &'static str {
        "TLSF + futex lock (new)"
    }
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { rt::alloc(l) }
    }
    unsafe fn free(&self, p: *mut u8, l: Layout) {
        unsafe { rt::dealloc(p, l) }
    }
    fn blocks(&self) -> usize {
        rt::free_blocks()
    }
}

struct Sys;
impl Heap for Sys {
    fn name(&self) -> &'static str {
        "host malloc (glibc)"
    }
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        unsafe { System.alloc(l) }
    }
    unsafe fn free(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    fn blocks(&self) -> usize {
        0
    }
}

fn lay(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

/// Leave `n` more free blocks: pairs of small blocks, the first of each freed (its neighbour keeps it from merging). The kept ones leak for the run.
fn fragment(h: &dyn Heap, n: usize, rng: &mut Rng) {
    let mut firsts = Vec::with_capacity(n);
    for _ in 0..n {
        let (a, b) = (rng.log_size(32, 256), rng.log_size(32, 256));
        let p = unsafe { h.alloc(lay(a, 8)) };
        let q = unsafe { h.alloc(lay(b, 8)) };
        assert!(!p.is_null() && !q.is_null());
        firsts.push((p, a));
    }
    for (p, a) in firsts {
        unsafe { h.free(p, lay(a, 8)) };
    }
}

/// A ring of 512 live blocks churned for about a second: ns an operation.
fn churn(h: &dyn Heap, rng: &mut Rng) -> f64 {
    let mut ring: Vec<(*mut u8, usize)> = vec![(std::ptr::null_mut(), 0); 1 << 9];
    let t = Instant::now();
    let mut ops = 0u64;
    loop {
        for _ in 0..1 << 8 {
            let i = rng.range(0, ring.len());
            let (p, s) = ring[i];
            if !p.is_null() {
                unsafe { h.free(p, lay(s, 8)) };
                ops += 1;
            }
            let s = rng.log_size(16, 4096);
            let p = unsafe { h.alloc(lay(s, 8)) };
            assert!(!p.is_null());
            unsafe { *p = 1 };
            ring[i] = (p, s);
            ops += 1;
        }
        if t.elapsed() > Duration::from_secs(1) {
            break;
        }
    }
    let ns = t.elapsed().as_nanos() as f64 / ops as f64;
    for (p, s) in ring {
        if !p.is_null() {
            unsafe { h.free(p, lay(s, 8)) };
        }
    }
    ns
}

/// The process's CPU time (user + system) from /proc, in clock ticks of 10 ms.
fn cpu_ticks() -> u64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap();
    let rest = &s[s.rfind(')').unwrap() + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    f[11].parse::<u64>().unwrap() + f[12].parse::<u64>().unwrap()
}

fn touch(p: *mut u8, size: usize) {
    unsafe { std::ptr::write_bytes(p, 7, size) };
}

struct Stats {
    frames: AtomicU64,
    frame_heap_ns: AtomicU64,
    worst_frame_heap_ns: AtomicU64,
    ops: AtomicU64,
}

fn mahere(h: &dyn Heap, seconds: u64) {
    let stop = AtomicBool::new(false);
    let st = Stats { frames: AtomicU64::new(0), frame_heap_ns: AtomicU64::new(0), worst_frame_heap_ns: AtomicU64::new(0), ops: AtomicU64::new(0) };
    let cpu0 = cpu_ticks();
    let t0 = Instant::now();
    std::thread::scope(|s| {
        // The drawing thread: frames of 570 calls on small blocks, each written, half freed within the frame and the rest at its end; its time in the heap a frame.
        s.spawn(|| {
            let mut rng = Rng(0xD7A0);
            let mut live = Vec::with_capacity(1 << 9);
            while !stop.load(Ordering::Relaxed) {
                let mut in_heap = 0u64;
                for k in 0..285 {
                    let size = rng.log_size(16, 4096);
                    let t = Instant::now();
                    let p = unsafe { h.alloc(lay(size, 8)) };
                    in_heap += t.elapsed().as_nanos() as u64;
                    assert!(!p.is_null());
                    touch(p, size);
                    live.push((p, size));
                    if k & 1 == 1 {
                        let (p, s) = live.swap_remove(rng.range(0, live.len()));
                        let t = Instant::now();
                        unsafe { h.free(p, lay(s, 8)) };
                        in_heap += t.elapsed().as_nanos() as u64;
                    }
                }
                for (p, s) in live.drain(..) {
                    let t = Instant::now();
                    unsafe { h.free(p, lay(s, 8)) };
                    in_heap += t.elapsed().as_nanos() as u64;
                }
                st.frames.fetch_add(1, Ordering::Relaxed);
                st.frame_heap_ns.fetch_add(in_heap, Ordering::Relaxed);
                st.worst_frame_heap_ns.fetch_max(in_heap, Ordering::Relaxed);
                st.ops.fetch_add(570, Ordering::Relaxed);
            }
        });
        // Loaders: a cell buffer of 64 KiB-2 MiB filled, and the 64 small long-lived blocks its labels leave; the last 16 cells and 8192 labels kept, the rest freed out of order.
        for i in 0..3 {
            let (stop, st) = (&stop, &st);
            s.spawn(move || {
                let mut rng = Rng(0x10AD + i);
                let (mut cells, mut labels) = (Vec::new(), Vec::new());
                while !stop.load(Ordering::Relaxed) {
                    let size = rng.log_size(64 << 10, 2 << 20);
                    let p = unsafe { h.alloc(lay(size, 16)) };
                    assert!(!p.is_null());
                    touch(p, size);
                    cells.push((p, size));
                    for _ in 0..64 {
                        let size = rng.log_size(8, 256);
                        let p = unsafe { h.alloc(lay(size, 8)) };
                        assert!(!p.is_null());
                        touch(p, size);
                        labels.push((p, size));
                    }
                    let mut n = 65;
                    while cells.len() > 16 {
                        let (p, s) = cells.swap_remove(rng.range(0, cells.len()));
                        unsafe { h.free(p, lay(s, 16)) };
                        n += 1;
                    }
                    while labels.len() > 1 << 13 {
                        let (p, s) = labels.swap_remove(rng.range(0, labels.len()));
                        unsafe { h.free(p, lay(s, 8)) };
                        n += 1;
                    }
                    st.ops.fetch_add(n, Ordering::Relaxed);
                }
                for (p, s) in cells {
                    unsafe { h.free(p, lay(s, 16)) };
                }
                for (p, s) in labels {
                    unsafe { h.free(p, lay(s, 8)) };
                }
            });
        }
        // Label threads: strings made and dropped, a ring of 4096.
        for i in 0..3 {
            let (stop, st) = (&stop, &st);
            s.spawn(move || {
                let mut rng = Rng(0x1AB + i);
                let mut ring: Vec<(*mut u8, usize)> = vec![(std::ptr::null_mut(), 0); 1 << 12];
                while !stop.load(Ordering::Relaxed) {
                    for _ in 0..1 << 6 {
                        let k = rng.range(0, ring.len());
                        let (p, s) = ring[k];
                        if !p.is_null() {
                            unsafe { h.free(p, lay(s, 8)) };
                        }
                        let size = rng.log_size(8, 512);
                        let p = unsafe { h.alloc(lay(size, 8)) };
                        assert!(!p.is_null());
                        touch(p, size);
                        ring[k] = (p, size);
                    }
                    st.ops.fetch_add(1 << 7, Ordering::Relaxed);
                }
                for (p, s) in ring {
                    if !p.is_null() {
                        unsafe { h.free(p, lay(s, 8)) };
                    }
                }
            });
        }
        // The vault thread: a 4-64 KiB value read, written, dropped.
        {
            let (stop, st) = (&stop, &st);
            s.spawn(move || {
                let mut rng = Rng(0x7A17);
                while !stop.load(Ordering::Relaxed) {
                    let size = rng.log_size(4 << 10, 64 << 10);
                    let p = unsafe { h.alloc(lay(size, 16)) };
                    assert!(!p.is_null());
                    touch(p, size);
                    unsafe { h.free(p, lay(size, 16)) };
                    st.ops.fetch_add(2, Ordering::Relaxed);
                }
            });
        }
        std::thread::sleep(Duration::from_secs(seconds));
        stop.store(true, Ordering::Relaxed);
    });
    let wall = t0.elapsed().as_secs_f64();
    let cpu = (cpu_ticks() - cpu0) as f64 / 100.;
    let frames = st.frames.load(Ordering::Relaxed);
    let heap_ms = st.frame_heap_ns.load(Ordering::Relaxed) as f64 / 1e6 / frames.max(1) as f64;
    let ops = st.ops.load(Ordering::Relaxed);
    println!(
        "  {:<28} drawing thread: {:>6} frames, {:>8.3} ms a frame in the heap ({:>7.0} ns a call), worst {:>7.2} ms; all threads {:>6.2} M calls/s; CPU {:.1} s of {:.1} s wall ({:.0} %), {:.0} ns of CPU a call; free blocks after: {}",
        h.name(),
        frames,
        heap_ms,
        heap_ms * 1e6 / 570.,
        st.worst_frame_heap_ns.load(Ordering::Relaxed) as f64 / 1e6,
        ops as f64 / wall / 1e6,
        cpu,
        wall,
        cpu / wall * 100.,
        cpu * 1e9 / ops as f64,
        h.blocks()
    );
}

fn main() {
    let gib = 1usize << 30;
    // The old heap over 4 GiB of the host's (untouched pages cost nothing), the new one over a fake arc of 4 GiB that grows from 4 MiB.
    let region = unsafe { System.alloc(lay(4 * gib, 1 << 12)) } as usize;
    let old = Old(ferros_heap::LockedHeap::empty());
    unsafe { old.0.init(region, 4 * gib) };
    let base = sys::init(4 << 20, 4 * gib);
    for (s, e) in rt_heap::layout::heap_regions(base + (64 << 10), base + (2 << 20), base + (2 << 20) + 4096, base + (4 << 20)) {
        if e > s {
            unsafe { rt::add_region(s, e - s) };
        }
    }
    let heaps: [&dyn Heap; 3] = [&old, &New, &Sys];

    println!("0. one thread, a ring of 512 blocks of 16 B-4 KiB churned (ns an alloc or a free):");
    for h in heaps {
        let mut rng = Rng(0xF2A6);
        let mut line = format!("  {:<28}", h.name());
        let mut have = 0;
        for want in [0usize, 20_000, 50_000] {
            fragment(h, want - have, &mut rng);
            have = want;
            let ns = churn(h, &mut rng);
            line += &format!("  {want:>6} fragments ({:>6} free blocks): {ns:>9.1} ns", h.blocks());
        }
        println!("{line}");
    }

    println!("1. Mahere's shape: 8 threads on the cores taskset gives ({} here), 4 s each, on the heaps part 0 left with 50 000 fragments:", std::thread::available_parallelism().unwrap());
    for h in heaps {
        mahere(h, 4);
    }
}
