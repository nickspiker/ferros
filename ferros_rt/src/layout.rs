//! Where the heap lies in the arc at start (start.rs): arithmetic only, so the host can test it without the rest of the crate (its `svc` and register reads are aarch64 assembly): `rustc --edition 2024 --test ferros_rt/src/layout.rs -o /tmp/ferros-rt-layout && /tmp/ferros-rt-layout`.
//!
//! The kernel lays out a process's arc (ferros_kernel process.rs `build_from`): the grants page, the code, the zeroed tail (.bss + 16, then the heap the kernel maps at spawn), the input, then at the arc's top the first thread's 64 KiB stack with one kernel-only page under it (`STACK_GUARD`, owner 2026-11-09: a stack run past its end faults with the process named). Everything between the input's end and that page is the arc's slack, which the heap takes too.

/// The kernel's entry stack at the arc's top (process.rs `STACK`); the heap stops below it.
pub const ENTRY_STACK: usize = 64 * 1024;
/// The kernel-only page under the entry stack (process.rs `STACK_GUARD`): no process may read or write it, so the heap stops below it too.
pub const ENTRY_GUARD: usize = 4 * 1024;

/// The heap's two regions at start, as arc addresses: the zeroed tail after .bss up to the input (the heap the kernel mapped), and the slack from the input's end up to the entry stack's guard page. A region that does not exist is empty (its start at its end).
pub fn heap_regions(bss_end: usize, input: usize, input_end: usize, top: usize) -> [(usize, usize); 2] {
    let first = if input > bss_end { (bss_end, input) } else { (bss_end, bss_end) };
    // The slack ends under the guard page, not at the stack: until 2026-11-11 it ended at the stack, so the guard page was free-list memory, and the first allocation to land there faulted (Mahere on the Pro, its heap growing on demand: a write at G#37E_F2B0, its top G#380_0000).
    let end = top - ENTRY_STACK - ENTRY_GUARD;
    let second = if end > input_end { (input_end, end) } else { (input_end, input_end) };
    [first, second]
}

/// One frame, the kernel's unit of an arc (frames.rs `FRAME`): the least a grow asks for.
pub const FRAME: usize = 1 << 21;
/// The heap keeps an eighth of the arc free at its top when it gives the rest back, and gives back no less than an eighth at a time: a heap that grew by a quarter (`grow_bytes`) and freed it all keeps half of that, so the next allocations of the same size come from it, not from another grow.
const KEEP_LOG2: u32 = 3;
/// A free block and its sentinel must survive the cut (tlsf.rs: `MIN_BLOCK` + `HEADER` is 48 bytes).
const LEAST: usize = 1 << 6;

/// What the arc grows by when the heap cannot serve an allocation whose region needs `need` bytes (tlsf.rs `grow_need`), the arc's top at `top` and its ceiling at `ceiling`.
/// A quarter of the arc at least (a heap that keeps growing asks a few times, not once a frame), one frame at least, and only what the allocation needs when the quarter would pass the ceiling (the kernel would refuse the lot, and `need` alone may fit).
pub fn grow_bytes(need: usize, top: usize, ceiling: usize) -> usize {
    // WHY each choice: policy (the larger of the three asks), not a bound on anyone's value.
    let mut want = if need > FRAME { need } else { FRAME };
    if top >> 2 > want {
        want = top >> 2;
    }
    if top + want > ceiling {
        want = need;
    }
    want
}

/// The arc's new top when the heap's top free block starts at arc offset `free_at` and the arc ends at `top`, or None when that would give back too little to be worth a syscall.
/// It keeps a cushion over the block's start: an eighth of the arc, or `big`, the largest allocation that ever made the heap grow, when that is more (a buffer allocated and freed each frame would otherwise grow and shrink the arc each frame).
pub fn shrink_to(free_at: usize, top: usize, big: usize) -> Option<usize> {
    // WHY each choice: policy (the larger cushion, the larger least give-back), not a bound on anyone's value.
    let cushion = if big > top >> KEEP_LOG2 { big } else { top >> KEEP_LOG2 };
    let keep = (free_at + LEAST + cushion).next_multiple_of(FRAME);
    let least = if top >> KEEP_LOG2 > FRAME { top >> KEEP_LOG2 } else { FRAME };
    if keep + least <= top { Some(keep) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_a_quarter_a_frame_or_the_need() {
        assert_eq!(grow_bytes(100, 4 << 20, 1 << 30), FRAME);
        assert_eq!(grow_bytes(100, 64 << 20, 1 << 30), 16 << 20);
        assert_eq!(grow_bytes(40 << 20, 64 << 20, 1 << 30), 40 << 20);
        // The quarter would pass the ceiling: only the need.
        assert_eq!(grow_bytes(3 << 20, 1000 << 20, 1010 << 20), 3 << 20);
    }

    #[test]
    fn shrinks_only_past_the_cushion() {
        let top = 256 << 20;
        // A free top of a sixteenth: under cushion + least, kept.
        assert_eq!(shrink_to(top - (16 << 20), top, 0), None);
        // Half the arc free: an eighth kept above the block's start, frame-rounded.
        let k = shrink_to(top / 2, top, 0).unwrap();
        assert_eq!(k, (top / 2 + LEAST + top / 8).next_multiple_of(FRAME));
        assert!(k % FRAME == 0 && k < top);
        // The largest grow so far is kept whole.
        let k = shrink_to(top / 4, top, 100 << 20).unwrap();
        assert!(k >= top / 4 + (100 << 20));
        assert_eq!(shrink_to(top / 4, top, 180 << 20), None);
        // A small arc gives back whole frames only.
        assert_eq!(shrink_to(6 << 20, 8 << 20, 0), None);
        assert_eq!(shrink_to(4 << 20, 8 << 20, 0), Some(6 << 20));
    }

    /// The kernel's layout of an arc of `top` bytes from 0: its stack and the guard page under it.
    fn kernel_reserved(top: usize) -> (usize, usize) {
        (top - ENTRY_STACK - ENTRY_GUARD, top)
    }

    #[test]
    fn no_region_touches_the_guard_page_or_the_stack() {
        // Mahere's arc on the Pro (2026-11-11): its initial top G#380_0000, the fault at G#37E_F2B0.
        let top = 0x380_0000;
        let (reserved, _) = kernel_reserved(top);
        let regions = heap_regions(0x340_0000, 0x37C_0000, 0x37C_0000, top);
        for (s, e) in regions {
            assert!(e <= reserved, "a heap region ends at {e:#x}, past the guard page at {reserved:#x}");
            assert!(s <= e);
        }
        let fault = 0x37E_F2B0;
        assert!(regions.iter().all(|&(s, e)| fault < s || fault >= e), "the faulting address lies in a heap region");
    }

    #[test]
    fn regions_end_where_the_kernel_reserved_begins() {
        for top in [0x20_0000, 0x40_0000, 0x380_0000, 0x2B40_0000] {
            let r = heap_regions(0x1_0000, 0x1_8000, 0x1_8100, top);
            assert_eq!(r[1].1, top - ENTRY_STACK - ENTRY_GUARD);
            assert_eq!(r[0], (0x1_0000, 0x1_8000));
        }
    }

    #[test]
    fn missing_regions_are_empty() {
        let r = heap_regions(0x9000, 0x8000, 0x30_0000, 0x20_0000);
        assert_eq!(r[0].0, r[0].1);
        assert_eq!(r[1].0, r[1].1);
    }
}
