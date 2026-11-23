//! The kernel heap's window (docs/NO-CEILINGS.md 1, step 4): past its two fixed regions the heap grows by frames mapped at the window's top as it runs low, and gives its free top back past a cushion.
//! The grower here stands in for the kernel's (kheap.rs): an arena of address space handed out a 2 MiB frame at a time from a cursor, refused past a "pool" the test sets.
//! One test, its phases in order: the grower is one function over statics, as the kernel's is one per boot, and the refusal record is one per binary.
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use ferros_heap::kernel::LockedTlsf;

const MIB: usize = 1 << 20;
const FRAME: usize = 2 * MIB;
const FIRST: usize = 12 * MIB;
const SECOND: usize = 10 * MIB;

static HEAP: LockedTlsf = LockedTlsf::empty();
/// The window's start, its mapped end, and the most it may map (the pool).
static WIN: AtomicUsize = AtomicUsize::new(0);
static TOP: AtomicUsize = AtomicUsize::new(0);
static POOL: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

/// The kernel's grower in miniature: frames enough for `bytes` and one more, mapped at the top and added.
fn grow(bytes: usize) -> bool {
    CALLS.fetch_add(1, SeqCst);
    let top = TOP.load(SeqCst);
    let n = bytes.div_ceil(FRAME) + 1;
    if top + n * FRAME > WIN.load(SeqCst) + POOL.load(SeqCst) {
        return false;
    }
    TOP.store(top + n * FRAME, SeqCst);
    unsafe { HEAP.extend(top, top + n * FRAME) };
    true
}

/// The kernel's trim in miniature: the free top past the cushion given back.
fn trim(cushion: usize) -> usize {
    let Some((b, cut)) = HEAP.trim_begin(cushion, FRAME) else { return 0 };
    let top = TOP.load(SeqCst);
    unsafe { HEAP.trim_end(b, cut) };
    TOP.store(cut, SeqCst);
    top - cut
}

fn lay(size: usize) -> Layout {
    Layout::from_size_align(size, 16).unwrap()
}

#[test]
fn the_window_grows_and_gives_back() {
    // The two fixed regions, then 1 GiB of address space for the window (untouched until written: the host maps it lazily, as the device's window costs nothing until a frame is mapped).
    let base = Box::leak(vec![0u8; FIRST + SECOND + 8192].into_boxed_slice());
    let a = (base.as_ptr() as usize + 0xFFF) & !0xFFF;
    let win = Box::leak(vec![0u8; (1 << 30) + FRAME].into_boxed_slice());
    let w = (win.as_ptr() as usize + FRAME - 1) & !(FRAME - 1);
    WIN.store(w, SeqCst);
    TOP.store(w, SeqCst);
    POOL.store(1 << 29, SeqCst);
    unsafe {
        HEAP.add(a, FIRST);
        HEAP.add(a + FIRST + 4096, SECOND);
    }
    // Without a grower the window never opens: a 64 MiB ask is refused, as the fixed heap refused it.
    let big = lay(64 * MIB);
    assert!(unsafe { HEAP.alloc(big) }.is_null());
    assert_eq!(LockedTlsf::last_fail().size, 64 * MIB);
    HEAP.set_grow(grow);
    // Small allocations held before the window opens: the fixed regions serve them, as they serve the kernel's own records.
    let small: Vec<*mut u8> = (0..64).map(|_| unsafe { HEAP.alloc(lay(4096)) }).collect();
    assert!(small.iter().all(|q| !q.is_null() && (*q as usize) < a + FIRST + SECOND + 4096));
    assert_eq!(CALLS.load(SeqCst), 0);

    // 64 MiB, more than both fixed regions: the window opens and holds it whole, written end to end.
    let p = unsafe { HEAP.alloc(big) };
    assert!(!p.is_null());
    assert!(p as usize >= w && (p as usize) + 64 * MIB <= TOP.load(SeqCst), "the block lies in the window");
    unsafe { core::ptr::write_bytes(p, 0xA5, 64 * MIB) };
    let r = HEAP.report().unwrap();
    assert_eq!(r.window, (w, TOP.load(SeqCst)));
    assert_eq!(r.size, FIRST + SECOND + (TOP.load(SeqCst) - w));
    HEAP.lock().verify();

    // A Vec-like doubling at the window's top grows in place into what each growth adds: never its old and new sizes at once.
    let mut size = 64 * MIB;
    let mut q = p;
    while size < 256 * MIB {
        let n = unsafe { HEAP.realloc(q, lay(size), size * 2) };
        assert!(!n.is_null());
        assert_eq!(n, q, "the top block grew where it lay");
        assert_eq!(unsafe { *n.add(64 * MIB - 1) }, 0xA5);
        q = n;
        size *= 2;
    }
    HEAP.lock().verify();
    let grown_to = TOP.load(SeqCst);

    // Freed: the free top goes back past a 4 MiB cushion, a frame boundary, and the window is one region still.
    unsafe { HEAP.dealloc(q, lay(size)) };
    let back = trim(4 * MIB);
    assert!(back >= 240 * MIB, "gave back {back}");
    let r = HEAP.report().unwrap();
    assert_eq!(r.window.1, TOP.load(SeqCst));
    assert!(r.window.1 - w <= 8 * MIB);
    assert_eq!(r.trimmed, grown_to - TOP.load(SeqCst));
    HEAP.lock().verify();
    // A second trim finds nothing past the cushion.
    assert_eq!(trim(4 * MIB), 0);

    // It grows again from the trimmed top, contiguous with what was kept.
    let calls = CALLS.load(SeqCst);
    let p = unsafe { HEAP.alloc(lay(100 * MIB)) };
    assert!(!p.is_null());
    assert!(CALLS.load(SeqCst) > calls);
    unsafe { core::ptr::write_bytes(p, 0x5A, 100 * MIB) };
    HEAP.lock().verify();

    // Past the pool: refused, the allocation null and recorded, everything held still whole.
    let fails = HEAP.report().unwrap().fails;
    let huge = lay(1 << 29);
    assert!(unsafe { HEAP.alloc(huge) }.is_null());
    let r = HEAP.report().unwrap();
    assert_eq!(r.fails, fails + 1);
    assert_eq!(LockedTlsf::last_fail().size, 1 << 29);
    assert_eq!(unsafe { *p.add(100 * MIB - 1) }, 0x5A);
    assert!(HEAP.fits(lay(MIB)));
    unsafe { HEAP.dealloc(p, lay(100 * MIB)) };
    for s in small {
        unsafe { HEAP.dealloc(s, lay(4096)) };
    }
    HEAP.lock().verify();
}
