//! The C allocator's sizes (cabi.rs): arithmetic only, so the host can test it without the C symbols (`cargo test` in tools/rt-heap, which takes this file by path).
//!
//! Every block carries a 16-byte header before what the caller gets (its size, for `free` and `realloc`).
//! The size a C caller asks for is untrusted: a size near SIZE_MAX, or a `calloc` count times size past it, is a refusal (None, so null to the caller), never a sum or product that wraps.

use core::alloc::Layout;

/// The header before each block, and the blocks' alignment.
pub const HEADER: usize = 16;

/// The heap block for a `malloc(size)`: the header and the size, 16-aligned; None when no such block can exist.
/// WHY: `size` comes from C code (a length read off the network, a count times a size), and `size + HEADER` near SIZE_MAX wraps to a few bytes; a `Layout` past isize::MAX is undefined behaviour besides.
/// PROOF: malloc(SIZE_MAX) would have asked the heap for 15 bytes and handed back a pointer the caller then writes SIZE_MAX bytes through: memory corruption. `checked_add` refuses the wrap, and `Layout::from_size_align` refuses any size whose rounding passes isize::MAX.
pub fn block(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HEADER)?, HEADER).ok()
}

/// The bytes a `calloc(count, size)` asks for; None when the product does not fit.
/// WHY: both factors come from C code, and their product wraps silently.
/// PROOF: calloc(2^33, 2^31) would have asked for 0 bytes (2^64 wrapped) and zeroed none, and the caller would then write 2^64 bytes past the block: memory corruption. `checked_mul` refuses it.
pub fn calloc_bytes(count: usize, size: usize) -> Option<usize> {
    count.checked_mul(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_sizes_get_the_header() {
        for size in [0, 1, 15, 16, 17, 4096, 1 << 30] {
            let l = block(size).unwrap();
            assert_eq!(l.size(), size + HEADER);
            assert_eq!(l.align(), HEADER);
        }
    }

    #[test]
    fn a_size_that_would_wrap_is_refused() {
        assert_eq!(block(usize::MAX), None);
        for k in 0..=HEADER {
            assert_eq!(block(usize::MAX - k), None, "SIZE_MAX - {k}");
        }
    }

    #[test]
    fn a_size_past_isize_max_is_refused() {
        assert_eq!(block(isize::MAX as usize), None);
        assert_eq!(block(isize::MAX as usize - HEADER + 1), None);
        // The largest size whose block, rounded to 16, still fits isize::MAX.
        let top = (isize::MAX as usize & !(HEADER - 1)) - HEADER;
        assert_eq!(block(top).unwrap().size(), top + HEADER);
        assert_eq!(block(top + 1), None);
    }

    #[test]
    fn calloc_products() {
        assert_eq!(calloc_bytes(0, usize::MAX), Some(0));
        assert_eq!(calloc_bytes(1 << 10, 1 << 10), Some(1 << 20));
        assert_eq!(calloc_bytes(1 << 33, 1 << 31), None);
        assert_eq!(calloc_bytes(usize::MAX, 2), None);
        assert_eq!(calloc_bytes(usize::MAX, 1), Some(usize::MAX));
    }

    #[test]
    fn calloc_then_block_refuses_a_huge_product() {
        // A product that fits usize but not a block: refused at the block.
        assert_eq!(calloc_bytes(usize::MAX, 1).and_then(block), None);
    }
}
