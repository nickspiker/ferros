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

#[cfg(test)]
mod tests {
    use super::*;

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
