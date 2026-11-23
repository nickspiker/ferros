//! The kernel's heap: TLSF (ferros_rt's own tlsf.rs, by path, so the kernel and every std process run one allocator) behind the spinlock, over the fixed regions the kernel adds once (the 12 MiB after its stack, then the window's second region) and a window that grows (docs/NO-CEILINGS.md 1, step 4).
//! The window: a reach of addresses the kernel maps frames of its pool into at the top as the heap runs low (`set_grow`'s hook, which calls `extend`), and gives back from the top past a cushion (`trim_begin`, `trim_end`), so one allocation can be as large as the pool and the heap holds about what it uses.
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
    /// The window's mapped part, [start, end) ((0, 0) before its first growth).
    pub window: (usize, usize),
    /// Bytes in the regions and the window.
    pub size: usize,
    /// The window's growths and the bytes given back from its top since boot.
    pub grows: usize,
    pub trimmed: usize,
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
    /// The window's mapped part: its first address and its end, both 0 until the first `extend`.
    window: (usize, usize),
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
        let window = if self.window.1 != 0 { Some(self.window) } else { None };
        for &(start, _) in self.region[..self.regions].iter().chain(window.iter()) {
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

/// The window's growths and the bytes trimmed from it, outside the lock (diagnostics).
static GROWS: AtomicUsize = AtomicUsize::new(0);
static TRIMMED: AtomicUsize = AtomicUsize::new(0);

/// The kernel's global allocator.
pub struct LockedTlsf {
    lock: SpinLock<State>,
    /// The window's grower (`set_grow`): `fn(bytes) -> bool` as a word, 0 while there is none.
    grow: AtomicUsize,
}

impl LockedTlsf {
    pub const fn empty() -> Self {
        LockedTlsf { lock: SpinLock::new(State { tlsf: Tlsf::empty(), region: [(0, 0); REGIONS], regions: 0, window: (0, 0) }), grow: AtomicUsize::new(0) }
    }

    /// The window's grower: called with the heap's lock let go when an allocation finds no free block, `grow(bytes)` maps at least `bytes` more at the window's top and adds them (`extend`), true when it did. It may refuse (false): the allocation then fails as it always did.
    /// The kernel decides there whether this core may take frames now (docs/NO-CEILINGS.md 1: never under a storage lock, never from inside a growth).
    pub fn set_grow(&self, grow: fn(usize) -> bool) {
        self.grow.store(grow as usize, Ordering::Release);
    }

    /// Ask the grower for room for `layout`; false when there is none or it refused.
    fn grown(&self, layout: Layout) -> bool {
        let g = self.grow.load(Ordering::Acquire);
        let Some(need) = Tlsf::grow_need(layout) else { return false };
        if g == 0 {
            return false;
        }
        // SAFETY: the word was stored from a `fn(usize) -> bool` by `set_grow` and is never anything else.
        let g: fn(usize) -> bool = unsafe { core::mem::transmute(g) };
        g(need)
    }

    /// Whether an allocation of `layout` would succeed now without growing (taken and given back at once, under the lock): the grower's test after a growth, as another core may have used what it added.
    pub fn fits(&self, layout: Layout) -> bool {
        let mut s = self.lock.lock();
        let p = unsafe { s.tlsf.alloc(layout) };
        if p.is_null() {
            return false;
        }
        unsafe { s.tlsf.free(p) };
        true
    }

    /// `[start, end)` added at the window's top: the first call opens the window there, every later one must start where the last ended (its old sentinel becomes the new bytes' header and merges with a free block before it), so the window stays one region.
    ///
    /// # Safety
    /// The bytes must be mapped as normal memory, unused, and the kernel's for as long as they are in the heap.
    pub unsafe fn extend(&self, start: usize, end: usize) {
        let mut s = self.lock.lock();
        // A growth that does not start at the window's end is a bug in the kernel's mapping: fail loud.
        assert!(s.window.1 == 0 || start == s.window.1, "the kernel heap's window grew somewhere other than its top");
        assert!(s.window.1 == 0 || s.tlsf.top() == s.window.1, "the kernel heap's window is not its last region");
        if unsafe { s.tlsf.add(start, end) } {
            if s.window.1 == 0 {
                s.window.0 = start;
            }
            s.window.1 = end;
            GROWS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The window's free top past `cushion` free bytes, to give back: off the free lists (nothing may allocate it meanwhile), and `Some((block, cut))` where the caller unmaps `[cut, the window's end)` and then calls `trim_end(block, cut)`. `cut` is a multiple of `grain` (the kernel's frame). None when the window's last block is in use or the free top is within the cushion.
    pub fn trim_begin(&self, cushion: usize, grain: usize) -> Option<(usize, usize)> {
        let mut s = self.lock.lock();
        let end = s.window.1;
        if end == 0 || s.tlsf.top() != end {
            return None;
        }
        let b = s.tlsf.top_free()?;
        // The cut keeps the free block a least block, its header and its sentinel, and the cushion past them.
        let keep = b + crate::tlsf::MIN_BLOCK + 2 * crate::tlsf::HEADER + cushion;
        let cut = (keep + grain - 1) & !(grain - 1);
        if cut >= end {
            return None;
        }
        s.tlsf.take_top();
        Some((b, cut))
    }

    /// The bytes of the free block at the window's top (0 when the window is not open, not the heap's last region, or its last block is in use): the kernel's cheap look before it takes its lock to trim.
    pub fn top_free_bytes(&self) -> usize {
        let s = self.lock.lock();
        if s.window.1 == 0 || s.tlsf.top() != s.window.1 {
            return 0;
        }
        s.tlsf.top_free().map_or(0, |b| s.window.1 - b)
    }

    /// The window now ends at `cut` (`trim_begin`'s): block `b` free again up to it, the bytes past it the kernel's to unmap.
    ///
    /// # Safety
    /// `b` and `cut` from `trim_begin`, and nothing past `cut` used any more.
    pub unsafe fn trim_end(&self, b: usize, cut: usize) {
        let mut s = self.lock.lock();
        let end = s.window.1;
        unsafe { s.tlsf.cut(b, cut) };
        s.window.1 = cut;
        TRIMMED.fetch_add(end - cut, Ordering::Relaxed);
    }

    /// Add `[start, start + size)` to the heap: at boot the first region, later the second.
    ///
    /// # Safety
    /// The region must be unused, mapped as normal memory, live for as long as the kernel runs, and not overlap any other region of the heap.
    pub unsafe fn add(&self, start: usize, size: usize) {
        let mut s = self.lock.lock();
        // A region too small for a block, or a fifth region, is a mistake in the kernel's memory map: fail loud.
        assert!(unsafe { s.tlsf.add(start, start + size) }, "a kernel heap region too small to hold a block");
        let n = s.regions;
        s.region[n] = (start, start + size);
        s.regions = n + 1;
    }

    /// The lock, for the host tests and a report that may wait.
    pub fn lock(&self) -> SpinGuard<'_, State> {
        self.lock.lock()
    }

    /// The lock only if it is free now: for reporters that must never wait on it (the panic handler, which may run on the core holding it).
    pub fn try_lock(&self) -> Option<SpinGuard<'_, State>> {
        self.lock.try_lock()
    }

    /// The heap now; None while the lock is held elsewhere.
    pub fn report(&self) -> Option<Report> {
        let s = self.lock.try_lock()?;
        Some(Report {
            region: s.region,
            regions: s.regions,
            window: s.window,
            size: s.region[..s.regions].iter().map(|r| r.1 - r.0).sum::<usize>() + (s.window.1 - s.window.0),
            grows: GROWS.load(Ordering::Relaxed),
            trimmed: TRIMMED.load(Ordering::Relaxed),
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
        loop {
            let mut s = self.lock.lock();
            let p = unsafe { s.tlsf.alloc(layout) };
            if !p.is_null() {
                drop(s);
                Self::moved(0, layout.size());
                return p;
            }
            drop(s);
            // The window grown with the lock let go (the grower takes frames and adds them thru `extend`), then asked again: another core may take what it added first.
            if !self.grown(layout) {
                Self::refused(&self.lock.lock(), layout, lr);
                return core::ptr::null_mut();
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.lock.lock().tlsf.free(ptr) };
        Self::moved(layout.size(), 0);
    }

    /// Where it lies first (TLSF `resize`): a shrink cuts the tail off, a grow takes the free block behind it, so a Vec doubling over free space moves nothing and never needs its old and new sizes at once. Otherwise a new block, the copy, the old one freed, as ferros_rt's realloc does: the copy outside the lock, the old block still the caller's until it is freed.
    #[inline(never)]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let lr = caller();
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let p = loop {
            let mut s = self.lock.lock();
            if unsafe { s.tlsf.resize(ptr, new_size) }.0 {
                drop(s);
                Self::moved(layout.size(), new_size);
                return ptr;
            }
            let p = unsafe { s.tlsf.alloc(new_layout) };
            if !p.is_null() {
                break p;
            }
            drop(s);
            // The window grown, then both asked again: a block at the window's top grows in place into what was added.
            if !self.grown(new_layout) {
                Self::refused(&self.lock.lock(), new_layout, lr);
                return core::ptr::null_mut();
            }
        };
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, p, if layout.size() < new_size { layout.size() } else { new_size });
            self.lock.lock().tlsf.free(ptr);
        }
        Self::moved(layout.size(), new_size);
        p
    }
}
