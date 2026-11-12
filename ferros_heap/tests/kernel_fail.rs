//! The kernel heap's refusals, one test in its own binary: the refusal record (`LockedTlsf::last_fail`) is one per binary, as on the device.
use core::alloc::{GlobalAlloc, Layout};
use ferros_heap::kernel::LockedTlsf;
use ferros_heap::LockedHeap;

const MIB: usize = 1 << 20;

fn arena(bytes: usize) -> usize {
    let a = Box::leak(vec![0u8; bytes + 4096].into_boxed_slice());
    (a.as_ptr() as usize + 0xFFF) & !0xFFF
}

fn lay(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).unwrap()
}

/// Grow one buffer by doubling from 4 KiB to `top` through `GlobalAlloc::realloc`; the size it reached.
fn double(h: &dyn GlobalAlloc, top: usize) -> usize {
    let mut size = 1 << 12;
    let mut p = unsafe { h.alloc(lay(size, 1)) };
    while size < top {
        let q = unsafe { h.realloc(p, lay(size, 1), size * 2) };
        if q.is_null() {
            break;
        }
        p = q;
        size *= 2;
    }
    unsafe { h.dealloc(p, lay(size, 1)) };
    size
}

/// Fill the heap with 4 KiB blocks, then free all but the first one in every `stride` of addresses: the small long-lived blocks a busy kernel leaves scattered (a process's tables, a queued message), here at their worst, every hole between them a little under `stride`. Returns the pins.
fn pin_every(h: &dyn GlobalAlloc, stride: usize) -> Vec<*mut u8> {
    let l = lay(1 << 12, 16);
    let mut all = Vec::new();
    loop {
        let p = unsafe { h.alloc(l) };
        if p.is_null() {
            break;
        }
        all.push(p);
    }
    all.sort();
    let mut pins = Vec::new();
    let mut next = 0;
    for p in all {
        if p as usize >= next {
            pins.push(p);
            next = p as usize + stride;
        } else {
            unsafe { h.dealloc(p, l) };
        }
    }
    pins
}

fn unpin(h: &dyn GlobalAlloc, pins: Vec<*mut u8>) {
    for p in pins {
        unsafe { h.dealloc(p, lay(1 << 12, 16)) };
    }
}

#[test]
fn refusals() {
    // ---- the record: a request past the largest block is refused, and says what, and how the heap stood.
    let a = arena(12 * MIB + (1 << 16) + 10 * MIB);
    let h = Box::leak(Box::new(LockedTlsf::empty()));
    unsafe {
        h.add(a, 12 * MIB);
        h.add(a + 12 * MIB + (1 << 16), 10 * MIB);
    }
    let p = unsafe { h.alloc(lay(16 * MIB, 64)) };
    assert!(p.is_null());
    let r = h.report().unwrap();
    assert_eq!(r.fails, 1);
    let f = r.last_fail;
    assert_eq!((f.size, f.align), (16 * MIB, 64));
    assert_eq!((f.free, f.free_blocks, f.largest), (r.free, r.free_blocks, r.largest));
    // On the host there is no link register to read.
    assert_eq!(f.caller, 0);
    assert_eq!(LockedTlsf::last_fail(), f);

    // ---- fragmentation, not exhaustion: a small block left every 2.5 MiB across both regions leaves no 3 MB hole with nearly all 22 MiB free. A 2.8 MB capsule read whole is refused; the same bytes in 512 KiB pieces (and the three pieces a vault put holds at once) are not.
    let capsule = 2_800_000;
    let piece = 1 << 19;
    let pins = pin_every(h, 5 * MIB / 2);
    let r = h.report().unwrap();
    assert!(r.free > 21 * MIB, "nearly all the heap is free: {r:?}");
    assert!(r.largest < 3 * MIB, "but no 3 MiB in one piece: {r:?}");
    let fails = r.fails;
    let whole = unsafe { h.alloc(lay(capsule, 1)) };
    assert!(whole.is_null(), "a whole capsule found room it should not have");
    let r = h.report().unwrap();
    assert_eq!((r.fails, r.last_fail.size), (fails + 1, capsule));
    assert!(r.last_fail.free > capsule && r.last_fail.largest < capsule, "refused for want of one piece, not of bytes: {r:?}");
    let pieces: Vec<*mut u8> = (0..capsule.div_ceil(piece)).map(|_| unsafe { h.alloc(lay(piece, 1)) }).collect();
    assert!(pieces.iter().all(|p| !p.is_null()), "pieces refused: {:?}", h.report());
    for p in pieces {
        unsafe { h.dealloc(p, lay(piece, 1)) };
    }
    unpin(h, pins);
    assert_eq!(h.report().unwrap().free_blocks, 2);
    h.lock().verify();

    // The free list, the same way: the capsule refused there too; the pieces not.
    let fl = Box::leak(Box::new(LockedHeap::empty()));
    unsafe { fl.init(arena(12 * MIB), 12 * MIB) };
    let pins = pin_every(fl, 5 * MIB / 2);
    assert!(fl.lock().free_bytes() > 11 * MIB && fl.lock().largest_free() < 3 * MIB);
    assert!(unsafe { fl.alloc(lay(capsule, 1)) }.is_null(), "the free list found a hole it should not have");
    let p = unsafe { fl.alloc(lay(piece, 1)) };
    assert!(!p.is_null());
    unsafe { fl.dealloc(p, lay(piece, 1)) };
    unpin(fl, pins);

    // ---- a doubling Vec in one 12 MiB region (the kernel's console between two reads): the free list's realloc is a new block and a copy, so the step from 4 to 8 MiB needs 12 MiB with the old block still in it, and it stops at 4 MiB. TLSF grows in place and reaches 8 MiB.
    let fl = Box::leak(Box::new(LockedHeap::empty()));
    unsafe { fl.init(arena(12 * MIB), 12 * MIB) };
    assert_eq!(double(fl, 8 * MIB), 4 * MIB);
    let t = Box::leak(Box::new(LockedTlsf::empty()));
    unsafe { t.add(arena(12 * MIB), 12 * MIB) };
    assert_eq!(double(t, 8 * MIB), 8 * MIB);
}
