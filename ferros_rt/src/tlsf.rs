//! The process heap's allocator: TLSF, two-level segregated fit (Masmano, Ripoll, Crespo and Real, 2004).
//! Every free block sits on one of 28 × 32 lists by its size: the first level is its power of two, the second splits that power into 32 equal steps.
//! Two bitmaps say which lists hold a block, so finding one that fits is two count-trailing-zeros, never a walk: an alloc or a free costs the same with ten free blocks or fifty thousand.
//! The first-fit free list it replaced (2026-11-11) walked every free block once a free and twice an alloc, and Mahere's loader, label and vault threads left 20-50 thousand of them.
//!
//! Pure arithmetic over memory it is given, `core` only, so the host tests it as it is (tools/rt-heap): no syscalls and no lock here (alloc.rs holds the lock, and grows and shrinks the arc).
//!
//! A block is a 16-byte header (the block physically before it, its size with the free bit) and its payload, both on 16 bytes, so every pointer handed out is 16-aligned without asking.
//! A free block keeps its two list links in its payload, so the least block is 32 bytes.
//! Each region ends in a sentinel, a used header of size 0, so the step to the next block never leaves the region.
//! A free merges with both neighbours at once, so two free blocks are never neighbours.
//! Good fit, not first fit: a search starts at the class that fits and takes the least class above it that holds a block, so big blocks (the top of the heap among them) are split only when nothing smaller serves.

use core::alloc::Layout;
use core::ptr::null_mut;

/// The grain every block and payload starts on (the widest alignment an ordinary aarch64 type wants).
const GRAIN_LOG2: u32 = 4;
pub const GRAIN: usize = 1 << GRAIN_LOG2;
/// A block's header: the block before, the size and the free bit.
pub const HEADER: usize = 1 << 4;
/// The least block: a header and the two list links a free block keeps.
pub const MIN_BLOCK: usize = 1 << 5;
/// Each power of two is split into 1 << SL_LOG2 classes.
const SL_LOG2: u32 = 5;
const SL: usize = 1 << SL_LOG2;
/// Blocks under 1 << FL_SHIFT bytes (512) share first level 0, its 32 classes one grain apart; above, level f + 1 − FL_SHIFT holds [2^f, 2^(f+1)).
const FL_SHIFT: u32 = SL_LOG2 + GRAIN_LOG2;
/// Blocks are under 1 << FL_MAX_LOG2 bytes (64 GiB, the whole window space ARCS gives out).
const FL_MAX_LOG2: u32 = 36;
const FL: usize = (FL_MAX_LOG2 - FL_SHIFT) as usize + 1;
pub const MAX_BLOCK: usize = 1 << FL_MAX_LOG2;
/// The free bit, in the size word's low bits (sizes are whole grains).
const FREE: usize = 1;

#[repr(C)]
struct Block {
    /// The block physically before, null for a region's first.
    prev: *mut Block,
    /// Bytes, header included, | FREE; 0 for a region's sentinel.
    size: usize,
    /// The free list's links, in the payload: only while free.
    next_free: *mut Block,
    prev_free: *mut Block,
}

#[inline(always)]
unsafe fn size(b: *const Block) -> usize {
    unsafe { (*b).size & !(GRAIN - 1) }
}

#[inline(always)]
unsafe fn is_free(b: *const Block) -> bool {
    unsafe { (*b).size & FREE != 0 }
}

#[inline(always)]
unsafe fn next(b: *const Block) -> *mut Block {
    unsafe { (b as usize + size(b)) as *mut Block }
}

#[inline(always)]
fn align_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

/// The list a free block of `size` bytes lives on: (first level, second level).
#[inline(always)]
fn index(size: usize) -> (usize, usize) {
    if size < 1 << FL_SHIFT {
        (0, size >> GRAIN_LOG2)
    } else {
        let f = usize::BITS - 1 - size.leading_zeros();
        ((f - FL_SHIFT) as usize + 1, (size >> (f - SL_LOG2)) & (SL - 1))
    }
}

/// `size` rounded up to the start of the next class, so every block on its list (and on every list above) is at least `size`.
#[inline(always)]
fn round_class(size: usize) -> usize {
    if size < 1 << FL_SHIFT {
        size
    } else {
        let f = usize::BITS - 1 - size.leading_zeros();
        align_up(size, 1 << (f - SL_LOG2))
    }
}

/// The block a payload of `bytes` needs, header included (`bytes` nonzero: GlobalAlloc never asks for nothing).
#[inline(always)]
fn block_for(bytes: usize) -> usize {
    align_up(bytes, GRAIN) + HEADER
}

/// The block a layout needs, and what a search must ask for to be sure of one at its alignment: an alignment past the grain may need a gap before the payload, and a gap is a free block of its own (none, or at least `MIN_BLOCK`). None past the largest block the lists hold.
#[inline(always)]
fn need(layout: Layout) -> Option<(usize, usize)> {
    debug_assert!(layout.size() != 0, "a zero-sized block could not hold its free links");
    let block = block_for(layout.size());
    let search = if layout.align() <= GRAIN { block } else { block + layout.align() + MIN_BLOCK };
    let search = round_class(search);
    // WHY: the size is the caller's (a Vec asked for a billion elements); a block past the last class has no list, and its index would fall outside the table.
    if search >= MAX_BLOCK { None } else { Some((block, search)) }
}

pub struct Tlsf {
    /// Bit f: some list of first level f holds a block.
    fl_map: u64,
    /// Bit s of word f: list (f, s) holds a block.
    sl_map: [u32; FL],
    heads: [[*mut Block; SL]; FL],
    /// The end of the region added last (its sentinel the header just under it), 0 before any: a region added at exactly this address extends it, and the arc shrinks from it.
    top: usize,
    /// Bytes and blocks on the lists.
    free: usize,
    blocks: usize,
}

// Raw pointers into memory the heap owns; the lock around it serialises every use.
unsafe impl Send for Tlsf {}

impl Tlsf {
    pub const fn empty() -> Self {
        Tlsf { fl_map: 0, sl_map: [0; FL], heads: [[null_mut(); SL]; FL], top: 0, free: 0, blocks: 0 }
    }

    /// Put free block `b` (its size set, its neighbours not free) on its list.
    #[inline(always)]
    unsafe fn insert(&mut self, b: *mut Block) {
        unsafe {
            let n = size(b);
            let (f, s) = index(n);
            let head = self.heads[f][s];
            (*b).next_free = head;
            (*b).prev_free = null_mut();
            if !head.is_null() {
                (*head).prev_free = b;
            }
            self.heads[f][s] = b;
            self.fl_map |= 1 << f;
            self.sl_map[f] |= 1 << s;
            (*b).size = n | FREE;
            self.free += n;
            self.blocks += 1;
        }
    }

    /// Take free block `b` off its list; it is marked used.
    #[inline(always)]
    unsafe fn remove(&mut self, b: *mut Block) {
        unsafe {
            let n = size(b);
            let (f, s) = index(n);
            let (nx, pv) = ((*b).next_free, (*b).prev_free);
            if !nx.is_null() {
                (*nx).prev_free = pv;
            }
            if !pv.is_null() {
                (*pv).next_free = nx;
            } else {
                self.heads[f][s] = nx;
                if nx.is_null() {
                    self.sl_map[f] &= !(1 << s);
                    if self.sl_map[f] == 0 {
                        self.fl_map &= !(1 << f);
                    }
                }
            }
            (*b).size = n;
            self.free -= n;
            self.blocks -= 1;
        }
    }

    /// The head of the least list whose every block is at least `search` bytes (`search` a class start, from `round_class`), null when none holds one.
    #[inline(always)]
    fn find(&self, search: usize) -> *mut Block {
        let (f, s) = index(search);
        let mut sl = self.sl_map[f] & (!0u32 << s);
        let f = if sl != 0 {
            f
        } else {
            let fl = self.fl_map & (!0u64 << (f + 1));
            if fl == 0 {
                return null_mut();
            }
            let f = fl.trailing_zeros() as usize;
            sl = self.sl_map[f];
            f
        };
        self.heads[f][sl.trailing_zeros() as usize]
    }

    /// Free block `b` (off any list, its size set): merged with a free neighbour on either side and put on its list. Returns the merged block.
    #[inline(always)]
    unsafe fn release(&mut self, mut b: *mut Block) -> *mut Block {
        unsafe {
            let n = next(b);
            if is_free(n) {
                self.remove(n);
                (*b).size = size(b) + size(n);
                (*next(b)).prev = b;
            }
            let p = (*b).prev;
            if !p.is_null() && is_free(p) {
                self.remove(p);
                (*p).size = size(p) + size(b);
                (*next(p)).prev = p;
                b = p;
            }
            self.insert(b);
            b
        }
    }

    /// Cut used block `b` to `want` bytes when the rest makes a block of its own, which is freed (merging with a free block behind).
    #[inline(always)]
    unsafe fn trim(&mut self, b: *mut Block, want: usize) -> *mut Block {
        unsafe {
            let have = size(b);
            if have - want < MIN_BLOCK {
                return null_mut();
            }
            let r = (b as usize + want) as *mut Block;
            (*r).prev = b;
            (*r).size = have - want;
            (*next(r)).prev = r;
            (*b).size = want;
            self.release(r)
        }
    }

    /// Whether free block `m` (just released) now reaches the top region's sentinel.
    #[inline(always)]
    unsafe fn at_top(&self, m: *mut Block) -> bool {
        !m.is_null() && unsafe { next(m) as usize } + HEADER == self.top
    }

    /// Add `[start, end)` to the heap (trimmed to the grain). A region starting where the last one ended extends it: the old sentinel becomes the new block's header, which merges with a free block before it. False when the region is too small to hold a block and its sentinel.
    ///
    /// # Safety
    /// The region must be unused, writable, never added before, and live as long as the heap.
    pub unsafe fn add(&mut self, start: usize, end: usize) -> bool {
        let s = align_up(start, GRAIN);
        let e = end & !(GRAIN - 1);
        // WHY: the bounds come from arithmetic over the arc (start.rs: the slack between the input and the entry stack can be a sliver or nothing); a region without room for a block and its sentinel is no region.
        if e < s + MIN_BLOCK + HEADER {
            return false;
        }
        unsafe {
            let b = if s == self.top && s != 0 {
                (s - HEADER) as *mut Block
            } else {
                let b = s as *mut Block;
                (*b).prev = null_mut();
                b
            };
            (*b).size = e - HEADER - b as usize;
            let sentinel = (e - HEADER) as *mut Block;
            (*sentinel).prev = b;
            (*sentinel).size = 0;
            self.top = e;
            self.release(b);
        }
        true
    }

    /// A block for `layout`, null when no free block fits (the caller grows the heap and asks again).
    ///
    /// # Safety
    /// `layout` must have a nonzero size.
    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let Some((want, search)) = need(layout) else { return null_mut() };
        let mut b = self.find(search);
        if b.is_null() {
            return null_mut();
        }
        unsafe {
            self.remove(b);
            if layout.align() > GRAIN {
                // The payload at the alignment: a gap before it of none or a whole least block, which goes back as a free block of its own (`need` asked for room for the worst).
                let p = b as usize + HEADER;
                let mut at = align_up(p, layout.align());
                if at != p && at - p < MIN_BLOCK {
                    at = align_up(p + MIN_BLOCK, layout.align());
                }
                let gap = at - p;
                if gap != 0 {
                    let a = (b as usize + gap) as *mut Block;
                    (*a).prev = b;
                    (*a).size = size(b) - gap;
                    (*next(a)).prev = a;
                    (*b).size = gap;
                    // b's own neighbour before it is used (b was free), and after it is `a`, used: no merge.
                    self.insert(b);
                    b = a;
                }
            }
            self.trim(b, want);
            (b as usize + HEADER) as *mut u8
        }
    }

    /// Free the block at `p`. True when the free block it merged into now reaches the top of the heap (the caller may give the arc's top back).
    ///
    /// # Safety
    /// `p` must come from `alloc` (or a `resize`) on this heap and not have been freed since.
    pub unsafe fn free(&mut self, p: *mut u8) -> bool {
        unsafe {
            let m = self.release((p as usize - HEADER) as *mut Block);
            self.at_top(m)
        }
    }

    /// Resize the block at `p` where it lies: shrinking cuts its tail off, growing takes the free block behind it when that is enough. (done, a freed tail now reaches the top). Not done, the caller moves it.
    ///
    /// # Safety
    /// As `free`; `new_size` nonzero.
    pub unsafe fn resize(&mut self, p: *mut u8, new_size: usize) -> (bool, bool) {
        let b = (p as usize - HEADER) as *mut Block;
        // WHY: the size is the caller's; past the largest block the lists hold it cannot be done in place (nor anywhere).
        if new_size >= MAX_BLOCK {
            return (false, false);
        }
        let want = block_for(new_size);
        unsafe {
            let have = size(b);
            if want <= have {
                let m = self.trim(b, want);
                return (true, self.at_top(m));
            }
            let n = next(b);
            if is_free(n) && have + size(n) >= want {
                self.remove(n);
                (*b).size = have + size(n);
                (*next(b)).prev = b;
                let m = self.trim(b, want);
                return (true, self.at_top(m));
            }
        }
        (false, false)
    }

    /// The bytes a region added on its own must hold for `layout` to fit in it: the search size and the sentinel. None past the largest block.
    pub fn grow_need(layout: Layout) -> Option<usize> {
        need(layout).map(|(_, search)| search + HEADER)
    }

    /// The end of the region added last.
    pub fn top(&self) -> usize {
        self.top
    }

    /// The free block that reaches the top region's end, if there is one: its address.
    pub fn top_free(&self) -> Option<usize> {
        if self.top == 0 {
            return None;
        }
        unsafe {
            let b = (*((self.top - HEADER) as *const Block)).prev;
            if b.is_null() || !is_free(b) { None } else { Some(b as usize) }
        }
    }

    /// Take the top free block off its list while the arc shrinks under it (nothing may allocate it, and a free beside it will not merge into it); `cut` gives back what the arc kept.
    pub fn take_top(&mut self) -> Option<usize> {
        let b = self.top_free()?;
        unsafe { self.remove(b as *mut Block) };
        Some(b)
    }

    /// The top region now ends at `end`: block `b` (from `take_top`) spans [b, end − HEADER) and is free again, the sentinel under `end`. `end` the old top puts it back as it was.
    ///
    /// # Safety
    /// `b` from `take_top`, `end` at most the old top, and the memory past `end` no longer used.
    pub unsafe fn cut(&mut self, b: usize, end: usize) {
        // A cut past the block's least size is a bug in the caller's arithmetic: fail loud.
        assert!(end >= b + MIN_BLOCK + HEADER, "the arc's new top leaves no block");
        unsafe {
            let blk = b as *mut Block;
            (*blk).size = end - HEADER - b;
            let sentinel = (end - HEADER) as *mut Block;
            (*sentinel).prev = blk;
            (*sentinel).size = 0;
            self.top = end;
            self.release(blk);
        }
    }

    /// Bytes on the free lists.
    pub fn free_bytes(&self) -> usize {
        self.free
    }

    /// Blocks on the free lists.
    pub fn free_blocks(&self) -> usize {
        self.blocks
    }

    /// The largest free block's bytes (diagnostics: the biggest allocation that can still succeed without a grow, near enough). Walks one list, the highest.
    pub fn largest_free(&self) -> usize {
        if self.fl_map == 0 {
            return 0;
        }
        let f = 63 - self.fl_map.leading_zeros() as usize;
        let s = 31 - self.sl_map[f].leading_zeros() as usize;
        let mut best = 0;
        let mut b = self.heads[f][s];
        while !b.is_null() {
            unsafe {
                if size(b) > best {
                    best = size(b);
                }
                b = (*b).next_free;
            }
        }
        best
    }

    /// How the free space is cut up (diagnostics: the kernel's `diag heap`): `each(floor, blocks, bytes)` for every first level that holds a free block, from the least, where `floor` is the least size the level holds (0 for level 0, every block under 512 bytes). Walks every free block.
    pub fn levels(&self, mut each: impl FnMut(usize, usize, usize)) {
        for f in 0..FL {
            if self.fl_map & (1 << f) == 0 {
                continue;
            }
            let (mut blocks, mut bytes) = (0, 0);
            for s in 0..SL {
                let mut b = self.heads[f][s];
                while !b.is_null() {
                    unsafe {
                        blocks += 1;
                        bytes += size(b);
                        b = (*b).next_free;
                    }
                }
            }
            each(if f == 0 { 0 } else { 1 << (f as u32 - 1 + FL_SHIFT) }, blocks, bytes);
        }
    }

    /// The host tests' check of every list: each block free, on the list its size says, its links both ways; the counts as the counters say. Returns (blocks, bytes).
    pub fn verify_lists(&self) -> (usize, usize) {
        let (mut blocks, mut bytes) = (0, 0);
        for f in 0..FL {
            assert_eq!(self.fl_map & (1 << f) != 0, self.sl_map[f] != 0, "first-level bit {f} disagrees with its word");
            for s in 0..SL {
                let mut b = self.heads[f][s];
                assert_eq!(self.sl_map[f] & (1 << s) != 0, !b.is_null(), "bit ({f}, {s}) disagrees with its list");
                let mut before: *mut Block = null_mut();
                while !b.is_null() {
                    unsafe {
                        assert!(is_free(b), "a used block on list ({f}, {s})");
                        assert_eq!(index(size(b)), (f, s), "a block of {} bytes on the wrong list", size(b));
                        assert_eq!((*b).prev_free, before, "a broken back link");
                        blocks += 1;
                        bytes += size(b);
                        before = b;
                        b = (*b).next_free;
                    }
                }
            }
        }
        assert_eq!((blocks, bytes), (self.blocks, self.free), "the counters disagree with the lists");
        (blocks, bytes)
    }

    /// The host tests' walk of one region from its first block to its sentinel: every block's back link right, sizes whole grains, no two free blocks side by side. Returns (the sentinel's end, free blocks, free bytes).
    ///
    /// # Safety
    /// `start` must be a region's first block (the trimmed start `add` used).
    pub unsafe fn verify_region(&self, start: usize) -> (usize, usize, usize) {
        let (mut blocks, mut bytes) = (0, 0);
        let mut b = start as *mut Block;
        let mut before: *mut Block = unsafe { (*b).prev };
        let mut was_free = false;
        unsafe {
            loop {
                assert_eq!((*b).prev, before, "a broken physical back link at {b:p}");
                let n = size(b);
                if n == 0 {
                    assert!(!is_free(b), "a free sentinel");
                    return (b as usize + HEADER, blocks, bytes);
                }
                assert!(n >= MIN_BLOCK && n % GRAIN == 0, "a block of {n} bytes");
                if is_free(b) {
                    assert!(!was_free, "two free blocks side by side at {b:p}");
                    blocks += 1;
                    bytes += n;
                }
                was_free = is_free(b);
                before = b;
                b = next(b);
            }
        }
    }
}
