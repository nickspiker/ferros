//! A process's heap that grows (docs/NO-CEILINGS.md 0.3, step 1): TLSF (tlsf.rs, the allocator the kernel and every std process run) over the zeroed tail the kernel maps past .bss and the arc's slack, and what the arc grows by when those run out (SYS_ARC, docs/ARCS.md step 2), given back from its top when a large free block lies there.
//!
//! It replaces `LockedHeap` (lib.rs) for the no_std payloads, whose heaps were static arrays in .bss: a reservation and a ceiling at once, since that allocator has no grow path.
//! The policy is ferros_rt's own (its layout.rs, included here by path, one source): where the heap lies at start (`heap_regions`), how much a grow asks (`grow_bytes`), how much a shrink keeps (`shrink_to`).
//! The flow is ferros_rt's alloc.rs, with a spinlock for its futex lock: a payload has no futex of its own to wait on, and `LockedHeap` spun as well.
//!
//! The kernel is reached thru the `Arc` trait, so the host tests it over an arena (tests/arc_heap.rs) and the kernel's own build of this crate needs no syscalls; a payload builds it with the `process` feature and uses `Kernel`.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::layout;
use crate::tlsf::Tlsf;
use crate::{SpinGuard, SpinLock, IN_USE, PEAK};

/// The process's arc as the kernel answers for it (SYS_ARC). Offsets are from the arc's base.
pub trait Arc: Sync {
    /// The arc's base address (x28).
    fn base(&self) -> usize;
    /// (the arc's top, its ceiling) as offsets.
    fn size(&self) -> (usize, usize);
    /// At least `bytes` more: Some((the new top, the old top)), the bytes between new, zeroed and the process's; None refused (the ceiling, or the device's memory).
    fn grow(&self, bytes: usize) -> Option<(usize, usize)>;
    /// Keep the first `keep` bytes and give the rest back: Some(the new top), None refused.
    fn shrink(&self, keep: usize) -> Option<usize>;
}

/// What the heap knows of the arc's top; one grow or shrink at a time. Taken before the heap's lock, never after.
struct Grown {
    /// The arc's top as the last grow or shrink answered (an offset), 0 until the first grow: until then there is nothing over the spawn floor to give back.
    top: usize,
    /// The largest region an allocation made the heap grow for: kept free at the top when it shrinks (`layout::shrink_to`).
    big: usize,
    /// The top free block's offset at the last shrink that gave nothing back; the next try waits until the free top reaches a sixteenth of the arc further down, or a grow.
    tried: usize,
}

/// The heap. `const`, so it can be a `#[global_allocator]` static; `init` once at start, before the first allocation.
pub struct ArcHeap<A: Arc> {
    arc: A,
    heap: SpinLock<Tlsf>,
    grown: SpinLock<Grown>,
    /// Bytes of the arc the heap holds: its regions at start and every grow, less what shrinks gave back (diagnostics: what a "the heap of N" line says now that N is not a constant).
    held: AtomicUsize,
    /// Grows answered and grows refused (diagnostics).
    grows: AtomicUsize,
    refused: AtomicUsize,
}

impl<A: Arc> ArcHeap<A> {
    pub const fn new(arc: A) -> Self {
        ArcHeap {
            arc,
            heap: SpinLock::new(Tlsf::empty()),
            grown: SpinLock::new(Grown { top: 0, big: 0, tried: usize::MAX }),
            held: AtomicUsize::new(0),
            grows: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
        }
    }

    /// The heap's memory at start: the zeroed tail from `bss_end` (an address: the linker's `__bss_end`) up to the spawn input, which the kernel puts at the tail's end (`input`, `input_len`: the entry's x1 and x2, an offset and a length), then the arc's slack past the input up to the entry stack's guard page.
    ///
    /// # Safety
    /// Once, before any allocation; `bss_end` must be the end of the image's .bss in this arc, and nothing else may use the memory between it and the entry stack.
    pub unsafe fn init(&self, bss_end: usize, input: usize, input_len: usize) {
        let base = self.arc.base();
        let (top, _) = self.arc.size();
        // A kernel without SYS_ARC answers ERR (all ones): every flashed kernel since 2026-11-11 has it, so a payload on one without is a bug in what was flashed, said at once rather than a heap laid out to the top of the address space.
        assert!(top != usize::MAX, "the kernel answered no arc size (SYS_ARC): this payload needs a kernel from 2026-11-11 or later");
        for (s, e) in layout::heap_regions(bss_end, base + input, base + input + input_len, base + top) {
            if e > s {
                unsafe { self.add_region(s, e) };
            }
        }
    }

    /// Add `[start, end)` (addresses) to the heap.
    ///
    /// # Safety
    /// The region must be unused, inside the arc, and never handed out twice.
    pub unsafe fn add_region(&self, start: usize, end: usize) {
        if unsafe { self.heap.lock().add(start, end) } {
            self.held.fetch_add(end - start, Ordering::Relaxed);
        }
    }

    /// The heap ran out for `layout`: the arc grows by at least what it needs, and the new bytes join the heap (one region with what the last grow brought). False when the kernel refuses: the allocation fails as it always did.
    fn grow(&self, layout: Layout) -> bool {
        let mut g = self.grown.lock();
        // Another thread may have grown the heap while this one waited.
        {
            let mut h = self.heap.lock();
            let p = unsafe { h.alloc(layout) };
            if !p.is_null() {
                unsafe { h.free(p) };
                return true;
            }
        }
        let Some(need) = Tlsf::grow_need(layout) else { return false };
        let (top, ceiling) = self.arc.size();
        let want = layout::grow_bytes(need, top, ceiling);
        let Some((new, old)) = self.arc.grow(want) else {
            self.refused.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        let base = self.arc.base();
        if unsafe { self.heap.lock().add(base + old, base + new) } {
            self.held.fetch_add(new - old, Ordering::Relaxed);
        }
        self.grows.fetch_add(1, Ordering::Relaxed);
        g.top = new;
        if need > g.big {
            g.big = need;
        }
        g.tried = usize::MAX;
        true
    }

    /// The free block at the heap's top just grew: give the arc's top back past the cushion (`layout::shrink_to`). Skipped while a grow or another shrink is under way.
    fn shrink(&self) {
        let Some(mut g) = self.grown.try_lock() else { return };
        if g.top == 0 {
            return;
        }
        let base = self.arc.base();
        let (b, keep) = {
            let mut h = self.heap.lock();
            // The heap's top is the arc's (the region the grows made), and its last block is free.
            if h.top() != base + g.top {
                return;
            }
            let Some(b) = h.top_free() else { return };
            let at = b - base;
            if at + (g.top >> 4) > g.tried {
                return;
            }
            let Some(keep) = layout::shrink_to(at, g.top, g.big) else { return };
            h.take_top();
            (b, keep)
        };
        // The block is off the lists (nothing allocates it) while the kernel takes the frames past `keep`; the heap is free for everyone else meanwhile.
        let end = match self.arc.shrink(keep) {
            Some(new) if new < g.top => {
                self.held.fetch_sub(g.top - new, Ordering::Relaxed);
                g.top = new;
                base + new
            }
            _ => {
                g.tried = b - base;
                base + g.top
            }
        };
        unsafe { self.heap.lock().cut(b, end) };
    }

    /// Bytes free in the heap now, without growing.
    pub fn free_bytes(&self) -> usize {
        self.heap.lock().free_bytes()
    }

    /// Free blocks now (fragmentation).
    pub fn free_blocks(&self) -> usize {
        self.heap.lock().free_blocks()
    }

    /// The largest free block's bytes.
    pub fn largest_free(&self) -> usize {
        self.heap.lock().largest_free()
    }

    /// (free bytes, free blocks, the largest), or None while the heap's lock is held (a report from a failed allocation must never spin on it).
    pub fn try_stats(&self) -> Option<(usize, usize, usize)> {
        self.heap.try_lock().map(|h: SpinGuard<'_, Tlsf>| (h.free_bytes(), h.free_blocks(), h.largest_free()))
    }

    /// Bytes of the arc the heap holds now.
    pub fn held(&self) -> usize {
        self.held.load(Ordering::Relaxed)
    }

    /// (grows answered, grows refused).
    pub fn grow_counts(&self) -> (usize, usize) {
        (self.grows.load(Ordering::Relaxed), self.refused.load(Ordering::Relaxed))
    }

    /// The arc's (top, ceiling) as the kernel says now.
    pub fn arc_size(&self) -> (usize, usize) {
        self.arc.size()
    }

    fn counted(p: *mut u8, size: usize) -> *mut u8 {
        if !p.is_null() {
            let now = IN_USE.fetch_add(size, Ordering::Relaxed) + size;
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
}

unsafe impl<A: Arc> GlobalAlloc for ArcHeap<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        loop {
            let p = unsafe { self.heap.lock().alloc(layout) };
            if !p.is_null() || !self.grow(layout) {
                return Self::counted(p, layout.size());
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let top = unsafe { self.heap.lock().free(ptr) };
        IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
        if top {
            self.shrink();
        }
    }

    /// Where it lies first: a shrink cuts the tail off, a grow takes the free block behind it; otherwise a new block, the copy, the old one freed.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let (done, top) = unsafe { self.heap.lock().resize(ptr, new_size) };
        if done {
            if new_size > layout.size() {
                let now = IN_USE.fetch_add(new_size - layout.size(), Ordering::Relaxed) + new_size - layout.size();
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                IN_USE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
            if top {
                self.shrink();
            }
            return ptr;
        }
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let p = unsafe { self.alloc(new_layout) };
        if !p.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, p, if layout.size() < new_size { layout.size() } else { new_size });
                self.dealloc(ptr, layout);
            }
        }
        p
    }
}

/// The kernel's SYS_ARC, for a payload (feature `process`).
#[cfg(feature = "process")]
pub struct Kernel;

#[cfg(feature = "process")]
impl Arc for Kernel {
    fn base(&self) -> usize {
        ferros_sys::arc_base() as usize
    }
    fn size(&self) -> (usize, usize) {
        ferros_sys::arc::size()
    }
    fn grow(&self, bytes: usize) -> Option<(usize, usize)> {
        ferros_sys::arc::grow(bytes).ok()
    }
    fn shrink(&self, keep: usize) -> Option<usize> {
        ferros_sys::arc::shrink(keep).ok().map(|(top, _)| top)
    }
}
