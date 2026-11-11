//! The process heap: the free-list allocator over the zeroed tail the kernel adds after the image (its size from the header, `--defsym __ferros_heap=`), plus whatever slack the arc's rounding left between the input and the kernel's entry stack, plus what the arc grows by when that runs out (SYS_ARC, docs/ARCS.md step 2: the declared heap is a ceiling since 2026-11-11, and the kernel maps only the first few MiB of it at spawn). Thread stacks and TLS blocks come from here too.

use core::alloc::Layout;

use crate::heap::{LockedHeap, SpinLock};
use crate::sys::{self, ERR, nr};

static HEAP: LockedHeap = LockedHeap::empty();
/// One grow at a time: a second thread that ran out meanwhile tries the heap again after the first one's grow before asking for its own.
static GROW: SpinLock<()> = SpinLock::new(());

/// SYS_ARC's ops (kernel process.rs `ARC_GROW`, `ARC_SIZE`).
const ARC_GROW: u64 = 0;
const ARC_SIZE: u64 = 2;
/// The least a grow asks for: one frame, the kernel's unit.
const GROW_MIN: usize = 2 << 20;

/// Add a region `[start, start + size)` of the arc to the heap; sizes and starts are trimmed to the 16-byte grain the headers want.
///
/// # Safety
/// The region must be unused, zeroed or not, inside the arc, and never handed out twice.
pub unsafe fn add_region(start: usize, size: usize) {
    let s = (start + 15) & !15;
    let e = (start + size) & !15;
    if e > s + 64 {
        unsafe { HEAP.extend(s, e - s) }
    }
}

pub unsafe fn alloc(layout: Layout) -> *mut u8 {
    loop {
        let p = unsafe { HEAP.lock().alloc(layout) };
        if !p.is_null() || !grow(layout) {
            return p;
        }
    }
}

/// The heap ran out for `layout`: the arc grows by at least what it needs (SYS_ARC), and the new bytes join the heap. False when the kernel refuses (the ceiling the build declared, or the device's memory, gone): the allocation fails as it always did.
fn grow(layout: Layout) -> bool {
    let _one = GROW.lock();
    // Another thread may have grown the heap while this one waited.
    let p = unsafe { HEAP.lock().alloc(layout) };
    if !p.is_null() {
        unsafe { dealloc(p, layout) };
        return true;
    }
    // Enough for the allocation at its alignment, with the free list's headers on either side.
    let need = layout.size() + layout.align() + 64;
    let (top, ceiling) = sys::svc2(nr::ARC, ARC_SIZE, 0);
    // A kernel without SYS_ARC: the heap it mapped is all there is.
    if top == ERR {
        return false;
    }
    // WHY the sizes: a quarter of the arc at least (a heap that keeps growing asks a few times, not once a frame), one frame at least, and only what the ceiling still allows when that is less than the quarter (the kernel would refuse the lot, and `need` alone may fit).
    let mut want = if need > GROW_MIN { need } else { GROW_MIN };
    if (top as usize) / 4 > want {
        want = top as usize / 4;
    }
    if (top as usize) + want > ceiling as usize {
        want = need;
    }
    let (new, old) = sys::svc2(nr::ARC, ARC_GROW, want as u64);
    if new == ERR {
        return false;
    }
    unsafe { add_region(sys::arc_base() as usize + old as usize, (new - old) as usize) };
    true
}

pub unsafe fn alloc_zeroed(layout: Layout) -> *mut u8 {
    let p = unsafe { alloc(layout) };
    if !p.is_null() {
        unsafe { core::ptr::write_bytes(p, 0, layout.size()) };
    }
    p
}

pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    unsafe { HEAP.lock().dealloc(ptr, layout) }
}

pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
    let p = unsafe { alloc(new_layout) };
    if !p.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, p, layout.size().min(new_size));
            dealloc(ptr, layout);
        }
    }
    p
}

/// Bytes on the free list right now (diagnostics).
pub fn free_bytes() -> usize {
    HEAP.lock().free_bytes()
}
