//! The process heap: TLSF (tlsf.rs, O(1) alloc and free) behind a futex lock (lock.rs), over the zeroed tail the kernel adds after the image (its size from the header, `--defsym __ferros_heap=`), the slack the arc's rounding left between the input and the kernel's entry stack, and what the arc grows by when those run out (SYS_ARC, docs/ARCS.md step 2: the declared heap is a ceiling since 2026-11-11, and the kernel maps only the first few MiB of it at spawn). Thread stacks and TLS blocks come from here too.
//!
//! The arc grows at its top and shrinks from its top (ARC_SHRINK; never under what the process was spawned with), so the heap gives memory back when the free block at the top of what it grew is large: a cushion is kept (layout.rs `shrink_to`), the rest goes. A range in the middle is never given back (ARC_RELEASE): a native arc's grow refills only what lies past its old top, so a hole there would be address space lost for good.

use core::alloc::Layout;

use crate::layout;
use crate::lock::Lock;
use crate::sys::{self, ERR, nr};
use crate::tlsf::Tlsf;

static HEAP: Lock<Tlsf> = Lock::new(Tlsf::empty());

/// What the heap knows of the arc's top; one grow or shrink at a time. Taken before HEAP, never after.
static ARC: Lock<Arc> = Lock::new(Arc { top: 0, big: 0, tried: usize::MAX });

struct Arc {
    /// The arc's top as the last grow or shrink answered (an arc offset), 0 until the first grow: until then there is nothing over the spawn floor to give back.
    top: usize,
    /// The largest region an allocation made the heap grow for: kept free at the top when it shrinks (layout.rs `shrink_to`).
    big: usize,
    /// The top free block's start (arc offset) at the last shrink that gave nothing back (refused, or the arc's shape kept it all); the next try waits until the free top reaches a sixteenth of the arc further down, or a grow.
    tried: usize,
}

/// SYS_ARC's ops (kernel process.rs `ARC_GROW`, `ARC_SHRINK`, `ARC_SIZE`).
const ARC_GROW: u64 = 0;
const ARC_SHRINK: u64 = 1;
const ARC_SIZE: u64 = 2;

/// Add a region `[start, start + size)` of the arc to the heap (trimmed to the grain; a sliver is dropped).
///
/// # Safety
/// The region must be unused, zeroed or not, inside the arc, and never handed out twice.
pub unsafe fn add_region(start: usize, size: usize) {
    unsafe { HEAP.lock().add(start, start + size) };
}

pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    loop {
        let p = unsafe { HEAP.lock().alloc(layout) };
        if !p.is_null() || !grow(layout) {
            return p;
        }
    }
}

/// The heap ran out for `layout`: the arc grows by at least what it needs (SYS_ARC), and the new bytes join the heap (one region with what the last grow brought: the old top's sentinel becomes the new block's header). False when the kernel refuses (the ceiling the build declared, or the device's memory, gone): the allocation fails as it always did.
fn grow(layout: Layout) -> bool {
    let mut arc = ARC.lock();
    // Another thread may have grown the heap while this one waited.
    {
        let mut heap = HEAP.lock();
        let p = unsafe { heap.alloc(layout) };
        if !p.is_null() {
            unsafe { heap.free(p) };
            return true;
        }
    }
    let Some(need) = Tlsf::grow_need(layout) else { return false };
    let (top, ceiling) = sys::svc2(nr::ARC, ARC_SIZE, 0);
    // A kernel without SYS_ARC: the heap it mapped is all there is.
    if top == ERR {
        return false;
    }
    let want = layout::grow_bytes(need, top as usize, ceiling as usize);
    let (new, old) = sys::svc2(nr::ARC, ARC_GROW, want as u64);
    if new == ERR {
        return false;
    }
    let base = sys::arc_base() as usize;
    unsafe { HEAP.lock().add(base + old as usize, base + new as usize) };
    arc.top = new as usize;
    if need > arc.big {
        arc.big = need;
    }
    arc.tried = usize::MAX;
    true
}

/// The free block at the heap's top just grew: give the arc's top back past the cushion (layout.rs `shrink_to`). Skipped while a grow or another shrink is under way.
fn shrink() {
    let Some(mut arc) = ARC.try_lock() else { return };
    if arc.top == 0 {
        return;
    }
    let base = sys::arc_base() as usize;
    let (b, keep) = {
        let mut heap = HEAP.lock();
        // The heap's top is the arc's (the region the grows made), and its last block is free.
        if heap.top() != base + arc.top {
            return;
        }
        let Some(b) = heap.top_free() else { return };
        let at = b - base;
        if at + (arc.top >> 4) > arc.tried {
            return;
        }
        let Some(keep) = layout::shrink_to(at, arc.top, arc.big) else { return };
        heap.take_top();
        (b, keep)
    };
    // The block is off the lists (nothing allocates it) while the kernel takes the frames past `keep`; the heap is free for everyone else meanwhile.
    let (new, _) = sys::svc2(nr::ARC, ARC_SHRINK, keep as u64);
    let end = if new == ERR || new as usize >= arc.top {
        arc.tried = b - base;
        base + arc.top
    } else {
        arc.top = new as usize;
        base + new as usize
    };
    unsafe { HEAP.lock().cut(b, end) };
}

pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    let p = unsafe { alloc(layout) };
    if !p.is_null() {
        unsafe { core::ptr::write_bytes(p, 0, layout.size()) };
    }
    p
}

pub unsafe fn dealloc(ptr: *mut u8, _layout: Layout) {
    let top = unsafe { HEAP.lock().free(ptr) };
    if top {
        shrink();
    }
}

/// Where it lies first: a shrink cuts the tail off, a grow takes the free block behind it (a Vec doubling over free space moves nothing); otherwise a new block, the copy, the old one freed.
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let (done, top) = unsafe { HEAP.lock().resize(ptr, new_size) };
    if done {
        if top {
            shrink();
        }
        return ptr;
    }
    let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
    let p = unsafe { alloc(new_layout) };
    if !p.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, p, if layout.size() < new_size { layout.size() } else { new_size });
            dealloc(ptr, layout);
        }
    }
    p
}

/// Bytes free in the heap right now (diagnostics).
pub fn free_bytes() -> usize {
    HEAP.lock().free_bytes()
}

/// Free blocks in the heap right now (diagnostics: fragmentation).
pub fn free_blocks() -> usize {
    HEAP.lock().free_blocks()
}

/// The largest free block's bytes (diagnostics).
pub fn largest_free() -> usize {
    HEAP.lock().largest_free()
}
