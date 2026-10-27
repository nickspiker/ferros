//! The process heap: the free-list allocator over the zeroed tail the kernel adds after the image (its size from the header, `--defsym __ferros_heap=`), plus whatever slack the arc's power-of-two rounding left between the input and the kernel's entry stack. Thread stacks and TLS blocks come from here too.

use core::alloc::Layout;

use crate::heap::LockedHeap;

static HEAP: LockedHeap = LockedHeap::empty();

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
    unsafe { HEAP.lock().alloc(layout) }
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
