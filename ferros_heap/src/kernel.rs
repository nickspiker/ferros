//! The kernel's heap: TLSF (ferros_rt's own tlsf.rs, by path, so the kernel and every std process run one allocator) behind the spinlock, over fixed regions the kernel adds once (the 12 MiB after its stack, then the window's second region; it never grows or shrinks).
//!
//! Until 2026-11-12 the kernel ran the first-fit free list in lib.rs (`LockedHeap`), whose alloc walked every free block under the lock, and whose realloc was always a new block, a copy and a free, so a Vec doubling past half the largest free block failed with the rest of the heap free.
//! Here realloc works in place first (TLSF `resize`: a grow takes the free block behind), an alloc or a free costs the same whatever the number of free blocks, and the report (`report`) gives what a failed allocation needs explained: the size of the regions, what is in use and its peak, the free bytes and blocks, the largest free block, and the free space by size (`State::levels`).
//! A failed allocation is recorded before it is answered with null (`Fail`): its size and alignment, the return address into whoever asked (`aarch64-linux-gnu-addr2line -e ferros_kernel` names it), and the heap as it stood, so the panic that follows says what was asked, from where, and why there was no room.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::tlsf::Tlsf;
use crate::{SpinGuard, SpinLock, IN_USE, PEAK};

/// The most regions the heap keeps a record of (the kernel adds two).
pub const REGIONS: usize = 1 << 2;

/// A failed allocation as it was asked, and the heap at that moment.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Fail {
    pub size: usize,
    pub align: usize,
    /// The return address into the caller of the global allocator (0 off aarch64).
    pub caller: usize,
    pub in_use: usize,
    pub free: usize,
    pub free_blocks: usize,
    pub largest: usize,
}

/// The heap now (`LockedTlsf::report`).
#[derive(Clone, Copy, Default, Debug)]
pub struct Report {
    /// The regions added, [start, end) each, the first `regions` of them.
    pub region: [(usize, usize); REGIONS],
    pub regions: usize,
    /// Bytes in the regions.
    pub size: usize,
    /// Bytes the callers asked for and hold, and the most since `reset_peak`.
    pub in_use: usize,
    pub peak: usize,
    /// Bytes and blocks on the free lists, and the largest block (the most one allocation can get, its 16-byte header aside).
    pub free: usize,
    pub free_blocks: usize,
    pub largest: usize,
    /// Allocations refused since boot, and the last (`LockedTlsf::last_fail`).
    pub fails: usize,
    pub last_fail: Fail,
}

/// The allocator and its regions, under one lock.
pub struct State {
    tlsf: Tlsf,
    region: [(usize, usize); REGIONS],
    regions: usize,
}

impl State {
    /// The free space by size: `each(floor, blocks, bytes)` for every first level of the lists that holds a block (`Tlsf::levels`).
    pub fn levels(&self, each: impl FnMut(usize, usize, usize)) {
        self.tlsf.levels(each);
    }

    /// The host tests' walk of the lists and of every region (tlsf.rs `verify_lists`, `verify_region`): returns the free (blocks, bytes), which both walks must agree on.
    pub fn verify(&self) -> (usize, usize) {
        let lists = self.tlsf.verify_lists();
        let (mut blocks, mut bytes) = (0, 0);
        for &(start, _) in &self.region[..self.regions] {
            // `add` trims the start to the grain; the walk starts where it did.
            let s = (start + crate::tlsf::GRAIN - 1) & !(crate::tlsf::GRAIN - 1);
            let (_, b, n) = unsafe { self.tlsf.verify_region(s) };
            blocks += b;
            bytes += n;
        }
        assert_eq!(lists, (blocks, bytes), "the lists and the regions disagree");
        lists
    }
}

/// Allocations refused since boot, and the last one's words (`Fail`, field by field), outside the lock: the panic handler reads them without it, since it may run on a core that holds it.
static FAILS: AtomicUsize = AtomicUsize::new(0);
static LAST_FAIL: [AtomicUsize; 7] = [const { AtomicUsize::new(0) }; 7];

/// The kernel's global allocator.
pub struct LockedTlsf(SpinLock<State>);

impl LockedTlsf {
    pub const fn empty() -> Self {
        LockedTlsf(SpinLock::new(State { tlsf: Tlsf::empty(), region: [(0, 0); REGIONS], regions: 0 }))
    }

    /// Add `[start, start + size)` to the heap: at boot the first region, later the second.
    ///
    /// # Safety
    /// The region must be unused, mapped as normal memory, live for as long as the kernel runs, and not overlap any other region of the heap.
    pub unsafe fn add(&self, start: usize, size: usize) {
        let mut s = self.0.lock();
        // A region too small for a block, or a fifth region, is a mistake in the kernel's memory map: fail loud.
        assert!(unsafe { s.tlsf.add(start, start + size) }, "a kernel heap region too small to hold a block");
        let n = s.regions;
        s.region[n] = (start, start + size);
        s.regions = n + 1;
    }

    /// The lock, for the host tests and a report that may wait.
    pub fn lock(&self) -> SpinGuard<'_, State> {
        self.0.lock()
    }

    /// The lock only if it is free now: for reporters that must never wait on it (the panic handler, which may run on the core holding it).
    pub fn try_lock(&self) -> Option<SpinGuard<'_, State>> {
        self.0.try_lock()
    }

    /// The heap now; None while the lock is held elsewhere.
    pub fn report(&self) -> Option<Report> {
        let s = self.0.try_lock()?;
        Some(Report {
            region: s.region,
            regions: s.regions,
            size: s.region[..s.regions].iter().map(|r| r.1 - r.0).sum(),
            in_use: IN_USE.load(Ordering::Relaxed),
            peak: PEAK.load(Ordering::Relaxed),
            free: s.tlsf.free_bytes(),
            free_blocks: s.tlsf.free_blocks(),
            largest: s.tlsf.largest_free(),
            fails: FAILS.load(Ordering::Relaxed),
            last_fail: Self::last_fail(),
        })
    }

    /// The last failed allocation (zero size: none since boot), read without the lock.
    pub fn last_fail() -> Fail {
        let w = |i: usize| LAST_FAIL[i].load(Ordering::Relaxed);
        Fail { size: w(0), align: w(1), caller: w(2), in_use: w(3), free: w(4), free_blocks: w(5), largest: w(6) }
    }

    /// Refused: what was asked, by whom, and the heap at that moment.
    #[cold]
    fn refused(s: &State, layout: Layout, caller: usize) {
        FAILS.fetch_add(1, Ordering::Relaxed);
        let words = [layout.size(), layout.align(), caller, IN_USE.load(Ordering::Relaxed), s.tlsf.free_bytes(), s.tlsf.free_blocks(), s.tlsf.largest_free()];
        for (a, v) in LAST_FAIL.iter().zip(words) {
            a.store(v, Ordering::Relaxed);
        }
    }

    /// What the callers hold went from `old` to `new` bytes.
    fn moved(old: usize, new: usize) {
        if new >= old {
            let now = IN_USE.fetch_add(new - old, Ordering::Relaxed) + (new - old);
            PEAK.fetch_max(now, Ordering::Relaxed);
        } else {
            IN_USE.fetch_sub(old - new, Ordering::Relaxed);
        }
    }
}

/// The return address of the function this is inlined into, read before anything can have used the link register (only meaningful as the first thing it does).
#[inline(always)]
fn caller() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        let lr: usize;
        unsafe { core::arch::asm!("mov {}, x30", out(reg) lr, options(nomem, nostack, preserves_flags)) };
        lr
    }
    #[cfg(not(target_arch = "aarch64"))]
    0
}

unsafe impl GlobalAlloc for LockedTlsf {
    // Not inlined, so the link register at its entry is the return address into the code that asked (`caller`).
    #[inline(never)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let lr = caller();
        let mut s = self.0.lock();
        let p = unsafe { s.tlsf.alloc(layout) };
        if p.is_null() {
            Self::refused(&s, layout, lr);
            return p;
        }
        drop(s);
        Self::moved(0, layout.size());
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.lock().tlsf.free(ptr) };
        Self::moved(layout.size(), 0);
    }

    /// Where it lies first (TLSF `resize`): a shrink cuts the tail off, a grow takes the free block behind it, so a Vec doubling over free space moves nothing and never needs its old and new sizes at once. Otherwise a new block, the copy, the old one freed, as ferros_rt's realloc does: the copy outside the lock, the old block still the caller's until it is freed.
    #[inline(never)]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let lr = caller();
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let p = {
            let mut s = self.0.lock();
            if unsafe { s.tlsf.resize(ptr, new_size) }.0 {
                drop(s);
                Self::moved(layout.size(), new_size);
                return ptr;
            }
            let p = unsafe { s.tlsf.alloc(new_layout) };
            if p.is_null() {
                Self::refused(&s, new_layout, lr);
                return p;
            }
            p
        };
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, p, if layout.size() < new_size { layout.size() } else { new_size });
            self.0.lock().tlsf.free(ptr);
        }
        Self::moved(layout.size(), new_size);
        p
    }
}
