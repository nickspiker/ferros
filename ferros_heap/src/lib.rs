// FERROS HEAP SOURCE MAP — keep updated when pub items or files change
//
// lib.rs ── free-list (linked-list) heap allocator, no_std, no alloc
//
//   struct ListNode { size, next } — free-block header stored inside the free memory itself start_addr(&self) → usize end_addr(&self) → usize
//
//   struct Heap { head: ListNode } — the allocator; free list is address-sorted so neighbors coalesce ::empty() → Heap  (const, for static globals) ::init(heap_start, heap_size) — add whole region as one free block ::alloc(layout) → *mut u8  (null on OOM) ::dealloc(ptr, layout) — free + coalesce with adjacent neighbors add_free_region(addr, size) — insert address-sorted, merge contiguous neighbors find_region(size, align) → Option<(&mut ListNode, alloc_start)> — first-fit + split remainder size_align(layout) → (size, align) — round size ≥ size_of ListNode, align ≥ align_of ListNode
//
//   struct SpinLock<T> — tiny AtomicBool spinlock, no spin-crate dependency ::new(T), ::lock() → Guard, Guard derefs to T
//
//   struct LockedHeap(SpinLock<Heap>) — GlobalAlloc wrapper ::empty() → LockedHeap  (const) ::init(start, size) via &self ::lock() → guard  impl GlobalAlloc (alloc returns null on OOM, never panics)

//! # ferros heap
//!
//! Classic linked-list (free-list) allocator for the ferros kernel, replacing the leak-forever bump allocator. Every freed block is returned to an address-sorted free list and merged with physically-adjacent free neighbors, so reclaimed memory is reusable and fragmentation is bounded. `no_std`, and does not depend on `alloc`.

#![cfg_attr(not(test), no_std)]

use core::alloc::{GlobalAlloc, Layout};
use core::mem::{align_of, size_of};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

/// A free-block header, stored in-place inside the free memory it describes.
pub struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}

impl ListNode {
    const fn new(size: usize) -> Self {
        ListNode { size, next: None }
    }

    fn start_addr(&self) -> usize {
        self as *const Self as usize
    }

    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

/// A free-list heap allocator with coalescing.
pub struct Heap {
    /// Sentinel head node; its `next` points at the first real free block.
    head: ListNode,
}

impl Heap {
    /// Creates an empty heap; const so it can back a `static` global allocator.
    pub const fn empty() -> Self {
        Heap {
            head: ListNode::new(0),
        }
    }

    /// Adds `[heap_start, heap_start + heap_size)` to the heap as one free block.
    ///
    /// # Safety
    /// The caller must ensure the region is valid, unused, and lives for `'static`, and must call `init` exactly once.
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        unsafe {
            self.add_free_region(heap_start, heap_size);
        }
    }

    /// Inserts a free region into the address-sorted list, merging any physically-contiguous neighbors.
    ///
    /// # Safety
    /// `addr` must point to an unused region of at least `size` bytes that can hold a `ListNode` and lives for `'static`.
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        // A freed block must be large enough and aligned to hold its own header.
        assert_eq!(align_up(addr, align_of::<ListNode>()), addr);
        assert!(size >= size_of::<ListNode>());

        // Walk the sorted list to the insertion point: the last node whose start is below `addr`.
        let mut cursor = &mut self.head;
        while let Some(ref next) = cursor.next {
            if next.start_addr() >= addr {
                break;
            }
            cursor = cursor.next.as_mut().unwrap();
        }

        // Detach the tail that will follow the freed block so we never drop `next.next`.
        let mut node_size = size;
        let mut successor = cursor.next.take();
        if let Some(next) = successor {
            if addr + node_size == next.start_addr() {
                // Physically contiguous with the following block; absorb it and inherit its successor.
                node_size += next.size;
                successor = next.next.take();
            } else {
                // Not contiguous; it stays as our successor.
                successor = Some(next);
            }
        }

        // Try to merge with the preceding node (`cursor` itself, when it is a real block).
        if cursor.size != 0 && cursor.end_addr() == addr {
            cursor.size += node_size;
            cursor.next = successor;
            return;
        }

        // Write our header in place and splice it into the list after `cursor`.
        let mut node = ListNode::new(node_size);
        node.next = successor;
        let node_ptr = addr as *mut ListNode;
        unsafe {
            node_ptr.write(node);
            cursor.next = Some(&mut *node_ptr);
        }
    }

    /// First-fit search: finds a free block that fits `size` at `align`, unlinks it, and returns it plus the aligned alloc start.
    ///
    /// Any remainder before/after the allocation is returned to the free list when it is big enough to hold a header.
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut ListNode, usize)> {
        let mut cursor = &mut self.head;
        while cursor.next.is_some() {
            let fit = Self::alloc_from_region(cursor.next.as_ref().unwrap(), size, align);
            if let Ok(alloc_start) = fit {
                // Fits: unlink this node from the list and hand it back with the aligned start.
                let region = cursor.next.take().unwrap();
                cursor.next = region.next.take();
                return Some((region, alloc_start));
            }
            cursor = cursor.next.as_mut().unwrap();
        }
        None
    }

    /// Checks whether `size`/`align` fit inside `region`; returns the aligned alloc start on success.
    fn alloc_from_region(region: &ListNode, size: usize, align: usize) -> Result<usize, ()> {
        let alloc_start = align_up(region.start_addr(), align);
        let alloc_end = alloc_start.checked_add(size).ok_or(())?;

        if alloc_end > region.end_addr() {
            // Too small even after alignment.
            return Err(());
        }

        // Any head gap created by alignment must be either zero or large enough to hold its own header.
        // A sub-header gap is unrecoverable (dealloc only knows the returned pointer), so reject this region.
        let head_gap = alloc_start - region.start_addr();
        if head_gap > 0 && head_gap < size_of::<ListNode>() {
            return Err(());
        }

        // Any leftover tail must likewise be either zero or large enough to hold its own header.
        let excess = region.end_addr() - alloc_end;
        if excess > 0 && excess < size_of::<ListNode>() {
            return Err(());
        }

        Ok(alloc_start)
    }

    /// Allocates per `layout`; returns null on OOM (never panics).
    ///
    /// # Safety
    /// Standard `GlobalAlloc` contract: `layout` must have nonzero size.
    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let (size, align) = Self::size_align(layout);

        if let Some((region, alloc_start)) = self.find_region(size, align) {
            let region_end = region.end_addr();
            let alloc_end = alloc_start + size;

            // Return any tail past the allocation to the free list.
            let excess = region_end - alloc_end;
            if excess > 0 {
                unsafe {
                    self.add_free_region(alloc_end, excess);
                }
            }

            // Return any head between the region start and the aligned start.
            // alloc_from_region guarantees this gap is either zero or a full header-sized block, so it is always recoverable.
            let head_gap = alloc_start - region.start_addr();
            if head_gap > 0 {
                unsafe {
                    self.add_free_region(region.start_addr(), head_gap);
                }
            }
            alloc_start as *mut u8
        } else {
            ptr::null_mut()
        }
    }

    /// Frees `ptr` and coalesces with adjacent free neighbors.
    ///
    /// # Safety
    /// `ptr`/`layout` must come from a prior `alloc` on this heap.
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _align) = Self::size_align(layout);
        unsafe {
            self.add_free_region(ptr as usize, size);
        }
    }

    /// Rounds `layout` so every freed block is header-sized and header-aligned.
    fn size_align(layout: Layout) -> (usize, usize) {
        let layout = layout
            .align_to(align_of::<ListNode>())
            .expect("adjusting alignment failed")
            .pad_to_align();
        let size = layout.size().max(size_of::<ListNode>());
        (size, layout.align())
    }
}

/// Rounds `addr` up to the nearest multiple of `align` (must be a power of two).
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// A minimal spinlock: an `AtomicBool` gate spun with `spin_loop`, no external crate.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: core::cell::UnsafeCell<T>,
}

// The lock serializes all access, so sharing across threads/cores is sound.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Wraps `value` in a new, unlocked spinlock.
    pub const fn new(value: T) -> Self {
        SpinLock {
            locked: AtomicBool::new(false),
            value: core::cell::UnsafeCell::new(value),
        }
    }

    /// Acquires the lock, spinning until it is free; returns an RAII guard.
    pub fn lock(&self) -> SpinGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinGuard { lock: self }
    }
}

/// RAII guard releasing the `SpinLock` on drop.
pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> core::ops::Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> core::ops::DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// A `GlobalAlloc`-compatible heap guarded by a spinlock.
pub struct LockedHeap(SpinLock<Heap>);

impl LockedHeap {
    /// Creates an empty locked heap; const so it can be a `static` global allocator.
    pub const fn empty() -> Self {
        LockedHeap(SpinLock::new(Heap::empty()))
    }

    /// Initializes the underlying heap with a memory region.
    ///
    /// # Safety
    /// Same contract as `Heap::init`: valid, unused, `'static` region, called once.
    pub unsafe fn init(&self, heap_start: usize, heap_size: usize) {
        unsafe {
            self.0.lock().init(heap_start, heap_size);
        }
    }

    /// Locks the heap and returns a guard for direct access (mainly for tests).
    pub fn lock(&self) -> SpinGuard<'_, Heap> {
        self.0.lock()
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.0.lock().alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.lock().dealloc(ptr, layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Backing store for a heap: a heap-allocated, header-aligned byte region.
    struct TestArena {
        // Kept alive to own the backing memory the heap points into; not read directly.
        #[allow(dead_code)]
        buf: Vec<u8>,
        start: usize,
        size: usize,
    }

    impl TestArena {
        /// Allocates `size` bytes plus slack, then aligns the usable window to a `ListNode` boundary.
        fn new(size: usize) -> Self {
            let slack = align_of::<ListNode>();
            let buf = vec![0u8; size + slack];
            let raw = buf.as_ptr() as usize;
            let start = align_up(raw, align_of::<ListNode>());
            TestArena { buf, size, start }
        }

        fn heap(&self) -> Heap {
            let mut heap = Heap::empty();
            unsafe { heap.init(self.start, self.size) };
            heap
        }

        /// Total bytes currently free across the whole list.
        fn free_total(heap: &Heap) -> usize {
            let mut total = 0;
            let mut node = &heap.head;
            while let Some(ref next) = node.next {
                total += next.size;
                node = next;
            }
            total
        }

        /// Number of distinct free blocks in the list.
        fn free_blocks(heap: &Heap) -> usize {
            let mut count = 0;
            let mut node = &heap.head;
            while let Some(ref next) = node.next {
                count += 1;
                node = next;
            }
            count
        }
    }

    #[test]
    fn alloc_dealloc_realloc_reuses_space() {
        let arena = TestArena::new(1 << 16);
        let mut heap = arena.heap();
        let layout = Layout::from_size_align(1024, 8).unwrap();

        let p1 = unsafe { heap.alloc(layout) };
        assert!(!p1.is_null());
        unsafe { heap.dealloc(p1, layout) };

        let p2 = unsafe { heap.alloc(layout) };
        assert_eq!(p1, p2, "freed space must be reused, heap must not grow");
        unsafe { heap.dealloc(p2, layout) };
    }

    #[test]
    fn many_small_frees_coalesce_into_one_big() {
        let arena = TestArena::new(1 << 16);
        let mut heap = arena.heap();
        let small = Layout::from_size_align(64, 8).unwrap();

        let mut ptrs = Vec::new();
        for _ in 0..256 {
            let p = unsafe { heap.alloc(small) };
            assert!(!p.is_null());
            ptrs.push(p);
        }
        // Free in a scrambled order to exercise both-neighbor coalescing.
        let mut order: Vec<usize> = (0..ptrs.len()).collect();
        // Simple deterministic shuffle.
        for i in 0..order.len() {
            let j = (i * 2654435761) % order.len();
            order.swap(i, j);
        }
        for &i in &order {
            unsafe { heap.dealloc(ptrs[i], small) };
        }

        assert_eq!(
            TestArena::free_blocks(&heap),
            1,
            "all fragments must coalesce back to a single block"
        );

        // A single large alloc spanning almost the whole heap must now succeed.
        let big = Layout::from_size_align(60 * 1024, 8).unwrap();
        let p = unsafe { heap.alloc(big) };
        assert!(!p.is_null(), "coalesced heap must satisfy one large alloc");
    }

    #[test]
    fn alignments_are_honored() {
        let arena = TestArena::new(1 << 18);
        let mut heap = arena.heap();
        let region_start = arena.start;
        let region_end = arena.start + arena.size;

        for &align in &[1usize, 8, 64, 4096] {
            let layout = Layout::from_size_align(100, align).unwrap();
            let p = unsafe { heap.alloc(layout) };
            assert!(!p.is_null(), "alloc failed for align {}", align);
            let addr = p as usize;
            assert_eq!(addr % align, 0, "pointer not aligned to {}", align);
            assert!(addr >= region_start && addr + 100 <= region_end, "pointer out of region");
            unsafe { heap.dealloc(p, layout) };
        }
    }

    #[test]
    fn oom_returns_null_not_panic() {
        let arena = TestArena::new(1 << 12);
        let mut heap = arena.heap();
        // Ask for more than the whole heap.
        let layout = Layout::from_size_align(1 << 20, 8).unwrap();
        let p = unsafe { heap.alloc(layout) };
        assert!(p.is_null(), "over-sized alloc must return null");
    }

    /// SplitMix64 — a tiny deterministic PRNG seeded from a constant, no external randomness.
    struct SplitMix64(u64);

    impl SplitMix64 {
        fn new(seed: u64) -> Self {
            SplitMix64(seed)
        }

        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }

        fn range(&mut self, lo: usize, hi: usize) -> usize {
            lo + (self.next_u64() as usize) % (hi - lo)
        }
    }

    /// A live allocation tracked by the fuzzer.
    struct Live {
        ptr: *mut u8,
        layout: Layout,
        id: u8,
        len: usize,
    }

    #[test]
    fn fuzz_stress() {
        const HEAP_SIZE: usize = 1 << 20;
        let arena = TestArena::new(HEAP_SIZE);
        let mut heap = arena.heap();
        let initial_free = TestArena::free_total(&heap);

        let mut rng = SplitMix64::new(0xF3_1105_CAFE_D00D);
        let mut live: Vec<Live> = Vec::new();
        let aligns = [1usize, 8, 16, 64];
        let mut next_id: u8 = 1;

        for _ in 0..50_000 {
            // Bias toward alloc when the list is small so we build pressure.
            let do_alloc = live.is_empty() || (rng.next_u64() & 1 == 0);

            if do_alloc {
                let size = rng.range(1, 8192);
                let align = aligns[rng.range(0, aligns.len())];
                let layout = Layout::from_size_align(size, align).unwrap();
                let p = unsafe { heap.alloc(layout) };
                if p.is_null() {
                    // OOM is fine — just skip this alloc.
                    continue;
                }
                let addr = p as usize;
                assert_eq!(addr % align, 0, "fuzz: misaligned pointer");
                assert!(
                    addr >= arena.start && addr + size <= arena.start + HEAP_SIZE,
                    "fuzz: pointer out of region"
                );

                let id = next_id;
                next_id = next_id.wrapping_add(1);
                if next_id == 0 {
                    next_id = 1;
                }
                // Stamp a unique pattern across the whole block.
                let pattern = id;
                unsafe {
                    for i in 0..size {
                        p.add(i).write(pattern ^ (i as u8));
                    }
                }

                // Overlap check against every live block.
                for other in &live {
                    let a0 = addr;
                    let a1 = addr + size;
                    let b0 = other.ptr as usize;
                    let b1 = b0 + other.len;
                    assert!(a1 <= b0 || b1 <= a0, "fuzz: overlapping allocations");
                }

                live.push(Live {
                    ptr: p,
                    layout,
                    id,
                    len: size,
                });
            } else {
                let idx = rng.range(0, live.len());
                let block = live.swap_remove(idx);
                // Verify the pattern is intact before freeing (catches corruption/overlap).
                unsafe {
                    for i in 0..block.len {
                        let got = block.ptr.add(i).read();
                        let want = block.id ^ (i as u8);
                        assert_eq!(got, want, "fuzz: pattern corruption at offset {}", i);
                    }
                    heap.dealloc(block.ptr, block.layout);
                }
            }
        }

        // Free everything that remains, verifying patterns as we go.
        for block in live.drain(..) {
            unsafe {
                for i in 0..block.len {
                    let got = block.ptr.add(i).read();
                    let want = block.id ^ (i as u8);
                    assert_eq!(got, want, "fuzz: final pattern corruption");
                }
                heap.dealloc(block.ptr, block.layout);
            }
        }

        // The whole heap must coalesce back to a single free block near the original size.
        let blocks = TestArena::free_blocks(&heap);
        let free = TestArena::free_total(&heap);
        assert_eq!(blocks, 1, "fuzz: heap did not coalesce to one block (got {})", blocks);
        assert_eq!(
            free, initial_free,
            "fuzz: coalesced free total {} != initial {}",
            free, initial_free
        );
    }
}
