//! The static cache map (KERNEL.md, Execution Model): one identity translation table, written once at boot, so ferros's own working memory is cacheable.
//!
//! It translates nothing (virtual = physical) and isolates nothing — isolation is compiled masking plus EL0. It exists because on ARM, with the MMU off, every data access is Device memory: no data cache, exclusives (atomics) not guaranteed, unaligned accesses fault. Measured on husky with the MMU off: a 4 MiB copy in 157 ms, BLAKE3 at 1.6 MB/s.
//!
//! Layout (4 KiB granule, 39-bit VA, walk starts at level 1):
//! - Every 1 GiB block is Device-nGnRnE, exactly what the MMU-off world was, so nothing outside the window changes and nothing there is ever speculatively touched (secure carveouts stay safe).
//! - The GiB holding the window gets a level-2 table: the window's 2 MiB blocks are Normal write-back; the rest stay Device.
//! - The 2 MiB blocks holding the device DMA buffers (`.bss.dma`) get level-3 tables: those 4 KiB pages are Normal non-cacheable, so DMA never needs cache maintenance.
//!
//! The window is ferros's image, stack, heap and staging slot: 36 MiB from the cold link base, or the whole 12 MiB handoff carveout (0x9240_0000..0x9300_0000, tpu_fw_reserved after it).

use core::arch::asm;

#[repr(C, align(4096))]
struct Table([u64; 512]);

static mut L1: Table = Table([0; 512]);
static mut L2: Table = Table([0; 512]);
static mut L3: [Table; 2] = [Table([0; 512]), Table([0; 512])];

unsafe extern "C" {
    static __dma_start: u8;
    static __dma_end: u8;
}

#[cfg(not(shim_handoff))]
const WINDOW: (usize, usize) = (0x8000_0000, 0x8240_0000);
#[cfg(shim_handoff)]
const WINDOW: (usize, usize) = (0x9240_0000, 0x9300_0000);

const GIB: usize = 1 << 30;
const MIB2: usize = 1 << 21;
const PAGE: usize = 1 << 12;

// MAIR_EL2: Attr0 Device-nGnRnE (G#00), Attr1 Normal write-back read/write-allocate (G#FF), Attr2 Normal non-cacheable (G#44).
const MAIR: u64 = 0x00 | (0xFF << 8) | (0x44 << 16);
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;
const ATTR_NC: u64 = 2 << 2;
// Descriptor bits: AP[1] (bit 6) is RES1 in the EL2 regime, SH inner shareable (bits 9:8), AF (bit 10), XN (bit 54).
const AP_EL2: u64 = 1 << 6;
const SH_INNER: u64 = 3 << 8;
const AF: u64 = 1 << 10;
const XN: u64 = 1 << 54;
const BLOCK: u64 = 0b01;
const TABLE_OR_PAGE: u64 = 0b11;

fn device(addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_DEVICE | AP_EL2 | AF | XN
}
fn normal(addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NORMAL | AP_EL2 | SH_INNER | AF
}
fn uncached(addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NC | AP_EL2 | SH_INNER | AF | XN
}

fn dma_range() -> (usize, usize) {
    unsafe { (&raw const __dma_start as usize, &raw const __dma_end as usize) }
}

/// Build the table and turn the MMU and data cache on. Call once, first thing in kernel_main (after the killswitch). Returns false (and changes nothing) if the layout doesn't fit.
pub fn enable() -> bool {
    let (w0, w1) = WINDOW;
    let (d0, d1) = dma_range();
    if w0 / GIB != (w1 - 1) / GIB || d0 < w0 || d1 > w1 || (d1 - 1) / MIB2 - d0 / MIB2 >= 2 {
        return false;
    }
    unsafe {
        let l1 = &mut *(&raw mut L1);
        let l2 = &mut *(&raw mut L2);
        let l3 = &mut *(&raw mut L3);
        for (i, e) in l1.0.iter_mut().enumerate() {
            *e = device(i * GIB, BLOCK);
        }
        let gib = w0 / GIB * GIB;
        l1.0[w0 / GIB] = &raw const L2 as u64 | TABLE_OR_PAGE;
        let mut next_l3 = 0;
        for (j, e) in l2.0.iter_mut().enumerate() {
            let a = gib + j * MIB2;
            *e = if a + MIB2 <= w0 || a >= w1 {
                device(a, BLOCK)
            } else if a + MIB2 > d0 && a < d1 {
                let t = &mut l3[next_l3];
                for (k, p) in t.0.iter_mut().enumerate() {
                    let pa = a + k * PAGE;
                    *p = if pa >= d0 && pa < d1 { uncached(pa, TABLE_OR_PAGE) } else { normal(pa, TABLE_OR_PAGE) };
                }
                let d = &raw const t.0 as u64 | TABLE_OR_PAGE;
                next_l3 += 1;
                d
            } else {
                normal(a, BLOCK)
            };
        }
        // The tables went to DRAM with the MMU off. Discard any stale lines over the window (left by the loader; ours bypassed the cache) before the window becomes cacheable — including the table walker's view of the tables.
        asm!("dsb sy");
        let mut a = w0;
        while a < w1 {
            asm!("dc ivac, {}", in(reg) a);
            a += 64;
        }
        // TCR_EL2 (non-VHE): T0SZ 25, inner/outer write-back walks, inner shareable, 4 KiB granule, 40-bit PA, RES1 bits 23 and 31.
        let tcr: u64 = 25 | (1 << 8) | (1 << 10) | (3 << 12) | (2 << 16) | (1 << 23) | (1 << 31);
        asm!(
            "dsb sy",
            "msr mair_el2, {mair}",
            "msr tcr_el2, {tcr}",
            "msr ttbr0_el2, {ttbr}",
            "isb",
            "tlbi alle2",
            "dsb sy",
            "isb",
            "mrs {t}, sctlr_el2",
            "orr {t}, {t}, #(1 << 0)",
            "orr {t}, {t}, #(1 << 2)",
            "orr {t}, {t}, #(1 << 12)",
            "msr sctlr_el2, {t}",
            "isb",
            mair = in(reg) MAIR,
            tcr = in(reg) tcr,
            ttbr = in(reg) &raw const L1 as u64,
            t = out(reg) _,
        );
    }
    true
}

/// Whether the MMU is on (SCTLR_EL2.M).
pub fn is_on() -> bool {
    let s: u64;
    unsafe { asm!("mrs {}, sctlr_el2", out(reg) s) };
    s & 1 != 0
}

/// Before RELOAD's copy stub: write every dirty line in the window back to DRAM and drop it, then turn the MMU and caches off, so the stub copies the staged image with the MMU off (as the next image expects) and no stale line can later be written back over it. IRQs must already be masked.
pub fn off_for_handover() {
    if !is_on() {
        return;
    }
    let (w0, w1) = WINDOW;
    unsafe {
        asm!("dsb sy");
        let mut a = w0;
        while a < w1 {
            asm!("dc civac, {}", in(reg) a);
            a += 64;
        }
        asm!(
            "dsb sy",
            "mrs {t}, sctlr_el2",
            "bic {t}, {t}, #(1 << 0)",
            "bic {t}, {t}, #(1 << 2)",
            "msr sctlr_el2, {t}",
            "isb",
            "tlbi alle2",
            "ic iallu",
            "dsb sy",
            "isb",
            t = out(reg) _,
        );
    }
}

/// Make freshly written code at [addr, addr + len) executable: clean it to the point of unification, then invalidate the I-cache.
pub fn sync_code(addr: usize, len: usize) {
    unsafe {
        let mut a = addr & !63;
        while a < addr + len {
            asm!("dc cvau, {}", in(reg) a);
            a += 64;
        }
        asm!("dsb ish", "ic iallu", "dsb ish", "isb");
    }
}
