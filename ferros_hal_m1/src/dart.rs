//! Apple DART (Device Address Resolution Table) — T8020 variant.
//!
//! Minimal IOMMU driver for USB DMA buffer mapping on M1.
//! Only supports the T8020 DART used by `dart-usb0`.
//!
//! Reference: m1n1/src/dart.c (Asahi Linux)

use ferros_hal::mmio;

// ---------------------------------------------------------------------------
// T8020 DART register offsets
// ---------------------------------------------------------------------------

const STREAM_COMMAND: usize = 0x20;
const STREAM_COMMAND_INVALIDATE: u32 = 1 << 20;
const STREAM_COMMAND_BUSY: u32 = 1 << 2;

const STREAM_SELECT: usize = 0x34;
const ERROR: usize = 0x40;
const ERROR_FLAG: u32 = 1 << 31;

const CONFIG: usize = 0x60;
const CONFIG_LOCK: u32 = 1 << 15;

const STREAM_REMAP: usize = 0x80;
const ENABLED_STREAMS: usize = 0xFC;

/// TCR (Translation Control Register) per-stream, offset = 0x100 + 4*sid
const TCR_OFF: usize = 0x100;
const TCR_TRANSLATE_ENABLE: u32 = 1 << 7;
const TCR_BYPASS_DART: u32 = 1 << 8;
const TCR_BYPASS_DAPF: u32 = 1 << 12;

/// TTBR (Translation Table Base Register) per-stream, offset = 0x200 + 16*sid + 4*idx
const TTBR_OFF: usize = 0x200;
const TTBR_VALID: u32 = 1 << 31;
const TTBR_ADDR_MASK: u32 = 0x7FFF_FFFF; // bits [30:0]
const TTBR_SHIFT: u32 = 12;

// Page table entry bits
const PTE_VALID: u64 = 1 << 0;
const PTE_DISABLE_SP: u64 = 1 << 1;
const PTE_OFFSET_SHIFT: u32 = 14;
/// SP_START and SP_END fields (subpage protection, set to full range)
const PTE_SP_START: u64 = 0;
const PTE_SP_END: u64 = 0xFFF << 40;

/// Standard PTE flags for T8020 (full access, no subpage restriction)
const PTE_FLAGS: u64 = PTE_SP_END | PTE_DISABLE_SP | PTE_VALID;

// Page and table sizes
/// DART page size: 16KB
pub const PAGE_SIZE: usize = 16384;
const PAGE_SHIFT: u32 = 14;
/// L1 table: 2048 entries × 8 bytes = 16KB
const L1_ENTRIES: usize = 2048;
/// L2 table: 2048 entries × 8 bytes = 16KB
const L2_ENTRIES: usize = 2048;
/// Number of TTBRs per stream
const TTBR_COUNT: usize = 4;

// ---------------------------------------------------------------------------
// Static page tables (in BSS, guaranteed DRAM)
// ---------------------------------------------------------------------------

/// L1 page table — one per TTBR slot (we use TTBR0 only).
/// Must be 16KB aligned for DART hardware.
#[repr(C, align(16384))]
struct L1Table {
    entries: [u64; L1_ENTRIES],
}

/// L2 page table — allocated on demand for IOVA ranges we map.
/// We pre-allocate 4 L2 tables, enough for 4 × 2048 × 16KB = 128MB of IOVA space.
#[repr(C, align(16384))]
struct L2Table {
    entries: [u64; L2_ENTRIES],
}

static mut L1: L1Table = L1Table { entries: [0; L1_ENTRIES] };
/// Pre-allocated L2 tables. Index tracks which are in use.
static mut L2_POOL: [L2Table; 4] = [
    L2Table { entries: [0; L2_ENTRIES] },
    L2Table { entries: [0; L2_ENTRIES] },
    L2Table { entries: [0; L2_ENTRIES] },
    L2Table { entries: [0; L2_ENTRIES] },
];
static mut L2_NEXT: usize = 0;

// ---------------------------------------------------------------------------
// DART device state
// ---------------------------------------------------------------------------

pub struct Dart {
    base: usize,
    sid: u8,
}

impl Dart {
    /// Initialize the DART for a single stream ID.
    /// Clears any existing translation, sets up L1, enables translation.
    pub fn init(base: usize, sid: u8) -> Self {
        let dart = Dart { base, sid };
        let sid_bit = 1u32 << (sid & 0x1F);

        unsafe {
            // Enable this stream
            let enabled = mmio::read32(base + ENABLED_STREAMS);
            mmio::write32(base + ENABLED_STREAMS, enabled | sid_bit);

            // Check if DART is locked (firmware set it up, we reuse L1)
            let config = mmio::read32(base + CONFIG);
            let locked = config & CONFIG_LOCK != 0;

            if !locked {
                // Clear L1 table
                let l1_ptr = &raw mut L1 as *mut u64;
                for i in 0..L1_ENTRIES {
                    core::ptr::write_volatile(l1_ptr.add(i), 0);
                }

                // Program TTBR0 with L1 physical address
                let l1_phys = &raw const L1 as usize;
                let ttbr_val = TTBR_VALID | ((l1_phys >> TTBR_SHIFT) as u32 & TTBR_ADDR_MASK);
                let ttbr_base = base + TTBR_OFF + (sid as usize) * 16;
                mmio::write32(ttbr_base, ttbr_val);
                // Clear remaining TTBRs
                for i in 1..TTBR_COUNT {
                    mmio::write32(ttbr_base + i * 4, 0);
                }

                // Enable translation
                let tcr_addr = base + TCR_OFF + (sid as usize) * 4;
                mmio::write32(tcr_addr, TCR_TRANSLATE_ENABLE);
            }

            // Invalidate TLB
            dart.invalidate_tlb();
        }

        dart
    }

    /// Map a contiguous physical region to an IOVA range.
    /// Both `iova` and `phys` must be 16KB aligned. `len` must be a multiple of 16KB.
    pub fn map(&self, iova: usize, phys: usize, len: usize) -> bool {
        debug_assert!(iova & (PAGE_SIZE - 1) == 0, "IOVA not page-aligned");
        debug_assert!(phys & (PAGE_SIZE - 1) == 0, "phys not page-aligned");
        debug_assert!(len & (PAGE_SIZE - 1) == 0, "len not page-aligned");

        let pages = len / PAGE_SIZE;
        for i in 0..pages {
            let page_iova = iova + i * PAGE_SIZE;
            let page_phys = phys + i * PAGE_SIZE;
            if !self.map_page(page_iova, page_phys) {
                return false;
            }
        }

        self.invalidate_tlb();
        true
    }

    /// Map a single 16KB page.
    fn map_page(&self, iova: usize, phys: usize) -> bool {
        let l1_idx = (iova >> 25) & 0x1FFF;
        let l2_idx = (iova >> PAGE_SHIFT) & 0x7FF;

        // Get or allocate L2 table for this L1 entry
        let l2_phys = unsafe {
            let l1_ptr = &raw mut L1 as *mut u64;
            let l1_entry = core::ptr::read_volatile(l1_ptr.add(l1_idx));

            if l1_entry & PTE_VALID != 0 {
                // L2 table already exists — extract its physical address
                ((l1_entry & !3) << (PAGE_SHIFT - PTE_OFFSET_SHIFT)) as usize
            } else {
                // Allocate a new L2 table from the pool
                let idx = L2_NEXT;
                if idx >= 4 {
                    return false; // Out of L2 tables
                }
                L2_NEXT += 1;

                let l2_addr = (&raw mut L2_POOL as *mut L2Table).add(idx) as usize;

                // Zero the L2 table
                let l2_ptr = l2_addr as *mut u64;
                for j in 0..L2_ENTRIES {
                    core::ptr::write_volatile(l2_ptr.add(j), 0);
                }

                // Write L1 entry pointing to this L2
                let l1_val = ((l2_addr as u64) >> PTE_OFFSET_SHIFT) << PTE_OFFSET_SHIFT
                    | PTE_SP_END | PTE_DISABLE_SP | PTE_VALID;
                core::ptr::write_volatile(l1_ptr.add(l1_idx), l1_val);

                // Cache clean the L1 entry
                mmio::cache_clean(l1_ptr.add(l1_idx) as usize, 8);

                l2_addr
            }
        };

        // Write L2 PTE
        unsafe {
            let l2_ptr = l2_phys as *mut u64;
            let pte = ((phys as u64) >> PTE_OFFSET_SHIFT) << PTE_OFFSET_SHIFT | PTE_FLAGS;
            core::ptr::write_volatile(l2_ptr.add(l2_idx), pte);
            mmio::cache_clean(l2_ptr.add(l2_idx) as usize, 8);
        }

        true
    }

    /// Invalidate TLB for this stream.
    fn invalidate_tlb(&self) {
        unsafe {
            mmio::write32(self.base + STREAM_SELECT, 1u32 << (self.sid & 0x1F));
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
            mmio::write32(self.base + STREAM_COMMAND, STREAM_COMMAND_INVALIDATE);

            // Wait for invalidation to complete
            for _ in 0..100_000u32 {
                if mmio::read32(self.base + STREAM_COMMAND) & STREAM_COMMAND_BUSY == 0 {
                    return;
                }
            }
            // Timeout — continue anyway (best effort)
        }
    }

    /// Read error status (for diagnostics).
    pub fn error_status(&self) -> u32 {
        unsafe { mmio::read32(self.base + ERROR) }
    }
}
