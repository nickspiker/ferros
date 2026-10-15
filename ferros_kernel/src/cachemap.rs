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
//!
//! VHE (HCR_EL2.E2H + TGE, where the cores have it): the kernel runs as an EL2 host and processes run at EL0 under this same table, so a process gets cached memory and working atomics. The table then also carries one static split, never changed per process: EL0 may touch only the arc pool (the window's top 2 MiB) and may execute only there; EL2 may not execute pool code (and architecturally can't anyway: privileged execution of EL0-writable memory is always a permission fault in the EL2&0 regime); the kernel, MMIO, DMA pages and the staging slot are EL2-only, so dev RUN payloads still execute at EL2 from the slot. Spawning a process copies its staged code into its arc in the pool. Isolation between processes stays compiled masking (KERNEL.md, Execution Model).
//! Switching E2H changes the layout of several EL2 registers — CPTR_EL2 then reads in CPACR form, where an unset FPEN traps FP/SIMD at EL2 and the kernel's own memcpy would fault — so the switch sets FPEN explicitly, and RELOAD restores the boot HCR_EL2/CPTR_EL2 before handing over.

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
/// The arc pool: EL0-accessible, 2 MiB aligned, clear of the staging slot (cold: slot G#8208_0000, pool G#8220_0000; handoff: slot G#92A0_0000, stub G#92E8_0000, pool G#92C0_0000).
#[cfg(not(shim_handoff))]
pub const POOL: (usize, usize) = (0x8220_0000, 0x8240_0000);
#[cfg(shim_handoff)]
pub const POOL: (usize, usize) = (0x92C0_0000, 0x92E0_0000);

/// HCR_EL2 / CPTR_EL2 as the loader left them, restored before RELOAD hands over.
static mut BOOT_HCR: u64 = 0;
static mut BOOT_CPTR: u64 = 0;
static mut VHE: bool = false;
const HCR_TGE: u64 = 1 << 27;
const HCR_E2H: u64 = 1 << 34;

const GIB: usize = 1 << 30;
const MIB2: usize = 1 << 21;
const PAGE: usize = 1 << 12;

// MAIR_EL2: Attr0 Device-nGnRnE (G#00), Attr1 Normal write-back read/write-allocate (G#FF), Attr2 Normal non-cacheable (G#44).
const MAIR: u64 = 0x00 | (0xFF << 8) | (0x44 << 16);
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;
const ATTR_NC: u64 = 2 << 2;
// Descriptor bits. Non-VHE EL2 regime: AP[1] (bit 6) is RES1, one XN (bit 54). VHE (EL2&0 regime): AP[1] = EL0 access, UXN (bit 54) / PXN (bit 53). SH inner shareable (bits 9:8), AF (bit 10).
const AP_EL2: u64 = 1 << 6;
const AP_EL0: u64 = 1 << 6;
const SH_INNER: u64 = 3 << 8;
const AF: u64 = 1 << 10;
const XN: u64 = 1 << 54;
const UXN: u64 = 1 << 54;
const PXN: u64 = 1 << 53;
const BLOCK: u64 = 0b01;
const TABLE_OR_PAGE: u64 = 0b11;

fn device(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_DEVICE | AF | if vhe { UXN | PXN } else { AP_EL2 | XN }
}
fn normal(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NORMAL | SH_INNER | AF | if vhe { UXN } else { AP_EL2 }
}
fn pool(vhe: bool, addr: usize, kind: u64) -> u64 {
    if vhe { addr as u64 | kind | ATTR_NORMAL | SH_INNER | AF | AP_EL0 | PXN } else { normal(false, addr, kind) }
}
fn uncached(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NC | SH_INNER | AF | if vhe { UXN | PXN } else { AP_EL2 | XN }
}

/// Whether the cores implement VHE (ID_AA64MMFR1_EL1.VH, bits 11:8).
fn has_vhe() -> bool {
    let v: u64;
    unsafe { asm!("mrs {}, ID_AA64MMFR1_EL1", out(reg) v) };
    (v >> 8) & 0xF != 0
}

/// Whether ferros switched to VHE (processes can run at EL0).
pub fn vhe() -> bool {
    unsafe { VHE }
}

fn dma_range() -> (usize, usize) {
    unsafe { (&raw const __dma_start as usize, &raw const __dma_end as usize) }
}

/// Build the table and turn the MMU and data cache on. Call once, first thing in kernel_main (after the killswitch). Returns false (and changes nothing) if the layout doesn't fit.
pub fn enable() -> bool {
    let (w0, w1) = WINDOW;
    let (d0, d1) = dma_range();
    let (p0, p1) = POOL;
    if w0 / GIB != (w1 - 1) / GIB || d0 < w0 || d1 > w1 || (d1 - 1) / MIB2 - d0 / MIB2 >= 2 || p0 < w0 || p1 > w1 || (d0 < p1 && p0 < d1) {
        return false;
    }
    let vhe = has_vhe();
    unsafe {
        let l1 = &mut *(&raw mut L1);
        let l2 = &mut *(&raw mut L2);
        let l3 = &mut *(&raw mut L3);
        for (i, e) in l1.0.iter_mut().enumerate() {
            *e = device(vhe, i * GIB, BLOCK);
        }
        let gib = w0 / GIB * GIB;
        l1.0[w0 / GIB] = &raw const L2 as u64 | TABLE_OR_PAGE;
        let mut next_l3 = 0;
        for (j, e) in l2.0.iter_mut().enumerate() {
            let a = gib + j * MIB2;
            *e = if a + MIB2 <= w0 || a >= w1 {
                device(vhe, a, BLOCK)
            } else if a >= p0 && a + MIB2 <= p1 {
                pool(vhe, a, BLOCK)
            } else if a + MIB2 > d0 && a < d1 {
                let t = &mut l3[next_l3];
                for (k, p) in t.0.iter_mut().enumerate() {
                    let pa = a + k * PAGE;
                    *p = if pa >= d0 && pa < d1 { uncached(vhe, pa, TABLE_OR_PAGE) } else { normal(vhe, pa, TABLE_OR_PAGE) };
                }
                let d = &raw const t.0 as u64 | TABLE_OR_PAGE;
                next_l3 += 1;
                d
            } else {
                normal(vhe, a, BLOCK)
            };
        }
        // The tables went to DRAM with the MMU off. Discard any stale lines over the window (left by the loader; ours bypassed the cache) before the window becomes cacheable — including the table walker's view of the tables.
        asm!("dsb sy");
        let mut a = w0;
        while a < w1 {
            asm!("dc ivac, {}", in(reg) a);
            a += 64;
        }
        // Record the boot HCR/CPTR, then (VHE) switch E2H + TGE with FP/SIMD explicitly untrapped (CPTR_EL2 in CPACR form: FPEN bits 21:20 = 0b11).
        let hcr: u64;
        let cptr: u64;
        asm!("mrs {}, hcr_el2", "mrs {}, cptr_el2", out(reg) hcr, out(reg) cptr);
        BOOT_HCR = hcr;
        BOOT_CPTR = cptr;
        if vhe {
            asm!(
                "msr hcr_el2, {h}",
                "isb",
                "msr cptr_el2, {c}",
                "isb",
                h = in(reg) hcr | HCR_E2H | HCR_TGE,
                c = in(reg) 3u64 << 20,
            );
            VHE = true;
            // EL0 may read the counters (CNTHCTL_EL2 in its E2H layout: EL0PCTEN b0, EL0VCTEN b1), so processes can time their own delays.
            asm!("mrs {t}, cnthctl_el2", "orr {t}, {t}, #3", "msr cnthctl_el2, {t}", "isb", t = out(reg) _);
        }
        // TCR_EL2. Non-VHE: T0SZ 25, inner/outer write-back walks, inner shareable, 4 KiB granule, 40-bit PA, RES1 bits 23 and 31. VHE (TCR_EL1 layout): the same for TTBR0, TTBR1 walks disabled (EPD1, TG1 = 4 KiB), IPS 40-bit.
        let tcr: u64 = if vhe {
            25 | (1 << 8) | (1 << 10) | (3 << 12) | (25 << 16) | (1 << 23) | (2 << 30) | (2 << 32)
        } else {
            25 | (1 << 8) | (1 << 10) | (3 << 12) | (2 << 16) | (1 << 23) | (1 << 31)
        };
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
            // VHE meanings (RES1/RES0 otherwise): SPAN (23) = 1 so taking an exception never sets PAN (the kernel reads arcs); UMA (9) = 0 so EL0 cannot mask interrupts (the killswitch); UCI (26), DZE (14), UCT (15) = 0.
            "orr {t}, {t}, #(1 << 23)",
            "bic {t}, {t}, #(1 << 9)",
            "bic {t}, {t}, #(1 << 26)",
            "bic {t}, {t}, #(1 << 14)",
            "bic {t}, {t}, #(1 << 15)",
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
            // Back to the loader's register layout (non-VHE), as the next image expects.
            "msr hcr_el2, {h}",
            "isb",
            "msr cptr_el2, {c}",
            "isb",
            t = out(reg) _,
            h = in(reg) BOOT_HCR,
            c = in(reg) BOOT_CPTR,
        );
        VHE = false;
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
