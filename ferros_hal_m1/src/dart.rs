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
    /// T8020 DART has two register banks that must be programmed identically.
    bases: [usize; 2],
    num_bases: usize,
    sid: u8,
}

impl Dart {
    /// Write a register to ALL DART register banks (T8020 requires both).
    unsafe fn write_all(&self, offset: usize, val: u32) {
        for i in 0..self.num_bases {
            mmio::write32(self.bases[i] + offset, val);
        }
    }

    /// Read a register from the first bank.
    unsafe fn read(&self, offset: usize) -> u32 {
        mmio::read32(self.bases[0] + offset)
    }

    /// Attach to an existing DART setup (e.g. from m1n1) without overwriting page tables.
    /// Reads the existing TTBR to find m1n1's L1 table and adds new mappings to it.
    pub fn attach(base0: usize, base1: usize, sid: u8) -> Self {
        Dart {
            bases: [base0, base1],
            num_bases: 2,
            sid,
        }
    }

    /// Add an identity mapping (iova=phys) to the EXISTING DART page tables.
    /// Reads the current TTBR to find m1n1's L1, allocates L2 from our pool if needed.
    pub fn map_existing(&self, iova: usize, phys: usize, len: usize) -> bool {
        let pages = len / PAGE_SIZE;
        for i in 0..pages {
            if !self.map_page_existing(iova + i * PAGE_SIZE, phys + i * PAGE_SIZE) {
                return false;
            }
        }
        self.invalidate_tlb();
        true
    }

    /// Map a single page using the EXISTING L1 table from TTBR.
    fn map_page_existing(&self, iova: usize, phys: usize) -> bool {
        let l1_idx = (iova >> 25) & 0x1FFF;
        let l2_idx = (iova >> PAGE_SHIFT) & 0x7FF;

        // Read the existing TTBR0 to find m1n1's L1 table
        let ttbr_addr = self.bases[0] + TTBR_OFF + (self.sid as usize) * 16;
        let ttbr = unsafe { mmio::read32(ttbr_addr) };
        if ttbr & TTBR_VALID == 0 {
            return false; // No L1 table configured
        }
        let l1_phys = ((ttbr & TTBR_ADDR_MASK) as usize) << TTBR_SHIFT;
        let l1_ptr = l1_phys as *mut u64;

        unsafe {
            let l1_entry = core::ptr::read_volatile(l1_ptr.add(l1_idx));

            let l2_phys = if l1_entry & PTE_VALID != 0 {
                // L2 already exists — use it
                ((l1_entry >> PTE_OFFSET_SHIFT) << PTE_OFFSET_SHIFT) as usize
            } else {
                // Allocate new L2 from our pool
                let idx = L2_NEXT;
                if idx >= 4 { return false; }
                L2_NEXT += 1;

                let l2_addr = (&raw mut L2_POOL as *mut L2Table).add(idx) as usize;
                let l2_ptr = l2_addr as *mut u64;
                for j in 0..L2_ENTRIES {
                    core::ptr::write_volatile(l2_ptr.add(j), 0);
                }

                // Write L1 entry pointing to new L2
                let l1_val = ((l2_addr as u64) >> PTE_OFFSET_SHIFT) << PTE_OFFSET_SHIFT
                    | PTE_SP_END | PTE_DISABLE_SP | PTE_VALID;
                core::ptr::write_volatile(l1_ptr.add(l1_idx), l1_val);
                mmio::cache_clean(l1_ptr.add(l1_idx) as usize, 8);

                l2_addr
            };

            // Write L2 PTE
            let l2_ptr = l2_phys as *mut u64;
            let pte = ((phys as u64) >> PTE_OFFSET_SHIFT) << PTE_OFFSET_SHIFT | PTE_FLAGS;
            core::ptr::write_volatile(l2_ptr.add(l2_idx), pte);
            mmio::cache_clean(l2_ptr.add(l2_idx) as usize, 8);
        }
        true
    }

    /// Initialize the DART for a single stream ID.
    /// `base0` and `base1` are reg[0] and reg[1] from the ADT.
    pub fn init(base0: usize, base1: usize, sid: u8) -> Self {
        let dart = Dart {
            bases: [base0, base1],
            num_bases: 2,
            sid,
        };
        let sid_bit = 1u32 << (sid & 0x1F);

        unsafe {
            // Enable this stream on both banks
            for i in 0..2 {
                let enabled = mmio::read32(dart.bases[i] + ENABLED_STREAMS);
                mmio::write32(dart.bases[i] + ENABLED_STREAMS, enabled | sid_bit);
            }

            // Check if DART is locked (firmware set it up, we reuse L1)
            let config = dart.read(CONFIG);
            let locked = config & CONFIG_LOCK != 0;

            if !locked {
                // Clear L1 table
                let l1_ptr = &raw mut L1 as *mut u64;
                for i in 0..L1_ENTRIES {
                    core::ptr::write_volatile(l1_ptr.add(i), 0);
                }

                // Program TTBR0 with L1 physical address on BOTH banks
                let l1_phys = &raw const L1 as usize;
                let ttbr_val = TTBR_VALID | ((l1_phys >> TTBR_SHIFT) as u32 & TTBR_ADDR_MASK);
                for b in 0..2 {
                    let ttbr_base = dart.bases[b] + TTBR_OFF + (sid as usize) * 16;
                    mmio::write32(ttbr_base, ttbr_val);
                    for i in 1..TTBR_COUNT {
                        mmio::write32(ttbr_base + i * 4, 0);
                    }
                }

                // Enable translation on BOTH banks
                dart.write_all(TCR_OFF + (sid as usize) * 4, TCR_TRANSLATE_ENABLE);
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

    /// Invalidate TLB for this stream on ALL register banks.
    fn invalidate_tlb(&self) {
        unsafe {
            let sid_bit = 1u32 << (self.sid & 0x1F);
            for b in 0..self.num_bases {
                mmio::write32(self.bases[b] + STREAM_SELECT, sid_bit);
                core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
                mmio::write32(self.bases[b] + STREAM_COMMAND, STREAM_COMMAND_INVALIDATE);
                for _ in 0..100_000u32 {
                    if mmio::read32(self.bases[b] + STREAM_COMMAND) & STREAM_COMMAND_BUSY == 0 {
                        break;
                    }
                }
            }
        }
    }

    /// Read error status (for diagnostics).
    pub fn error_status(&self) -> u32 {
        unsafe { mmio::read32(self.bases[0] + ERROR) }
    }
}
