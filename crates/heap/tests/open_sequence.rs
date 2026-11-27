//! Replays the shim's `Store::open` allocation sequence on a 4 MiB heap: it must never return null.
use core::alloc::Layout;
use ferros_heap::LockedHeap;

#[test]
fn open_sequence_never_returns_null() {
    const HEAP: usize = 4 * 1024 * 1024;
    let arena = Box::leak(vec![0u8; HEAP + 4096].into_boxed_slice());
    let base = (arena.as_ptr() as usize + 0xFFF) & !0xFFF;
    let heap = LockedHeap::empty();
    unsafe { heap.init(base, HEAP) };
    let seq = [(48usize, 8usize), (4096, 1), (65636, 1), (65636, 1), (64 * 1024, 1), (256, 8)];
    let mut live = Vec::new();
    for (size, align) in seq {
        let l = Layout::from_size_align(size, align).unwrap();
        let p = unsafe { heap.lock().alloc(l) };
        // Take the stats before asserting: two `lock()` temporaries in one expression deadlock.
        let (free, blocks) = { let h = heap.lock(); (h.free_bytes(), h.free_blocks()) };
        assert!(!p.is_null(), "alloc {size}/{align} returned null; free={free} blocks={blocks}");
        live.push((p, l));
    }
    for (p, l) in live { unsafe { heap.lock().dealloc(p, l) } }
    assert_eq!(heap.lock().free_blocks(), 1);
    assert_eq!(heap.lock().free_bytes(), HEAP);
}

/// Regression for the hardware OOM: sizes that are 8 mod 16 must not strand the free region at an address a 16-aligned request can't use.
#[test]
fn sixteen_aligned_request_after_odd_eight_sizes() {
    const HEAP: usize = 64 * 1024;
    let arena = Box::leak(vec![0u8; HEAP + 4096].into_boxed_slice());
    let base = (arena.as_ptr() as usize + 0xFFF) & !0xFFF;
    let heap = LockedHeap::empty();
    unsafe { heap.init(base, HEAP) };
    // 40 + 4096 + 80 = 4216 bytes, the exact live set on hardware when the 512-byte 16-aligned request was refused: without the fix the free region then starts 8 mod 16.
    let odd = Layout::from_size_align(40, 8).unwrap();
    let a = unsafe { heap.lock().alloc(odd) };
    let b = unsafe { heap.lock().alloc(Layout::from_size_align(4096, 1).unwrap()) };
    let c = unsafe { heap.lock().alloc(Layout::from_size_align(80, 8).unwrap()) };
    let d = unsafe { heap.lock().alloc(Layout::from_size_align(512, 16).unwrap()) };
    assert!(!a.is_null() && !b.is_null() && !c.is_null());
    let (free, blocks) = { let h = heap.lock(); (h.free_bytes(), h.free_blocks()) };
    assert!(!d.is_null(), "16-aligned request refused with {free} bytes free in {blocks} block(s)");
    assert_eq!(d as usize % 16, 0);
    for (p, l) in [(a, odd), (b, Layout::from_size_align(4096, 1).unwrap()), (c, Layout::from_size_align(80, 8).unwrap()), (d, Layout::from_size_align(512, 16).unwrap())] {
        unsafe { heap.lock().dealloc(p, l) }
    }
    assert_eq!(heap.lock().free_blocks(), 1);
    assert_eq!(heap.lock().free_bytes(), HEAP);
}
