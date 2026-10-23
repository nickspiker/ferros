//! The static cache map (KERNEL.md, Execution Model): one identity translation table, written once at boot, so ferros's own working memory is cacheable.
//!
//! It translates nothing (virtual = physical) and isolates nothing — isolation is compiled masking plus EL0. It exists because on ARM, with the MMU off, every data access is Device memory: no data cache, exclusives (atomics) not guaranteed, unaligned accesses fault. Measured on husky with the MMU off: a 4 MiB copy in 157 ms, BLAKE3 at 1.6 MB/s.
//!
//! Layout (4 KiB granule, 39-bit VA, walk starts at level 1):
//! - Every 1 GiB block is Device-nGnRnE, exactly what the MMU-off world was, so nothing outside the window changes and nothing there is ever speculatively touched (secure carveouts stay safe).
//! - The GiB holding the window gets a level-2 table: the window's 2 MiB blocks are Normal write-back; the rest stay Device.
//! - The 2 MiB blocks holding the device DMA buffers (`.bss.dma`) get level-3 tables: those 4 KiB pages are Normal non-cacheable, so DMA never needs cache maintenance.
//! - DMA_POOL (cold only): 2 MiB of Normal non-cacheable memory a driver process reaches directly (EL0 read/write, never executable at any level) — the memory its device DMAs into (rings, buffers). It is granted like hardware (process.rs `DMA`), and the device's IOMMU/S2MPU window is meant to cover exactly it, so the device reaches nothing else.
//!
//! The window is ferros's image, stack, heap and staging slot: 36 MiB from the cold link base, or the whole 12 MiB handoff carveout (0x9240_0000..0x9300_0000, tpu_fw_reserved after it).
//!
//! VHE (HCR_EL2.E2H + TGE, where the cores have it): the kernel runs as an EL2 host and processes run at EL0 under this same table, so a process gets cached memory and working atomics. The table then also carries one static split, never changed per process: EL0 may touch only the arc pool (POOL) and may execute only there; EL2 may not execute pool code (and architecturally can't anyway: privileged execution of EL0-writable memory is always a permission fault in the EL2&0 regime); the kernel, MMIO, DMA pages and the staging slot are EL2-only, so dev RUN payloads still execute at EL2 from the slot. Spawning a process copies its staged code into its arc in the pool. Isolation between processes stays compiled masking (KERNEL.md, Execution Model).
//! Switching E2H changes the layout of several EL2 registers — CPTR_EL2 then reads in CPACR form, where an unset FPEN traps FP/SIMD at EL2 and the kernel's own memcpy would fault — so the switch sets FPEN explicitly, and RELOAD restores the boot HCR_EL2/CPTR_EL2 before handing over.

use core::arch::asm;

#[repr(C, align(4096))]
struct Table([u64; 512]);

static mut L1: Table = Table([0; 512]);
static mut L2: Table = Table([0; 512]);
/// The level-2 table for the GiB above the window's (G#C000_0000-G#FFFF_FFFF): every 2 MiB block Device, as the 1 GiB block was, except APP_SURFACE.
static mut L2_HI: Table = Table([0; 512]);
static mut L3: [Table; 2] = [Table([0; 512]), Table([0; 512])];
/// 4 KiB pages for the arc pool (POOL), so a process's pages carry per-arc W^X (SANDBOX.md): the cold pool is 8 MiB = 4 blocks, the shim pool 2 MiB = 1.
static mut POOL_L3: [Table; 4] = [Table([0; 512]), Table([0; 512]), Table([0; 512]), Table([0; 512])];

unsafe extern "C" {
    static __dma_start: u8;
    static __dma_end: u8;
}

#[cfg(not(shim_handoff))]
const WINDOW: (usize, usize) = (0x8000_0000, 0x8240_0000);
#[cfg(shim_handoff)]
const WINDOW: (usize, usize) = (0x9240_0000, 0x9300_0000);
/// The arc pool: EL0-accessible, a power of two aligned to its size, clear of the kernel image/stack/heap and the staging slot. Cold: 8 MiB at G#8100_0000 (heap ends below it; staging slot G#8208_0000) — big enough for a driver process carrying multi-MiB firmware. Handoff (oracle only): 2 MiB at G#92C0_0000 (slot G#92A0_0000, stub G#92E8_0000).
#[cfg(not(shim_handoff))]
pub const POOL: (usize, usize) = (0x8100_0000, 0x8180_0000);
#[cfg(shim_handoff)]
pub const POOL: (usize, usize) = (0x92C0_0000, 0x92E0_0000);

/// Device DMA memory for driver processes (see the module doc): cold G#8180_0000, the 2 MiB above the pool, below the reload stub (G#8207_0000). None on the handoff path.
#[cfg(not(shim_handoff))]
pub const DMA_POOL: (usize, usize) = (0x8180_0000, 0x81A0_0000);
#[cfg(shim_handoff)]
pub const DMA_POOL: (usize, usize) = (0, 0);

/// The display's framebuffers (cold only): 32 MiB of Normal non-cacheable, EL0 read/write memory above the window (two WQHD BGRX8888 surfaces of 15.3 MiB), the DPU's DMA target once ferros stops painting on ABL's splash surface. Granted as the DMA region `display-fb`. Free DRAM on a cold boot: Linux's image would be here, and the first reserved carveout (zuma-rmem.dtsi) is at G#9000_0000.
#[cfg(not(shim_handoff))]
pub const DISPLAY_FB: (usize, usize) = (0x8400_0000, 0x8600_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_FB: (usize, usize) = (0, 0);

/// A cacheable heap for the display server (cold only): 32 MiB of Normal write-back, EL0 read/write, never executable DRAM above the framebuffers — a process heap outside its arc, so the 8 MiB arc pool doesn't have to carry it and the non-cacheable framebuffer region doesn't slow it (a typeface parse over non-cacheable memory ran for minutes, 2026-10-16). Its top 16 MiB is the display server's back buffer, where frames are composited before one write-only copy to the framebuffer. It ends where the camera's buffers begin (G#8800_0000). Granted as the region `display-heap`; nothing DMAs into it.
#[cfg(not(shim_handoff))]
pub const DISPLAY_HEAP: (usize, usize) = (0x8600_0000, 0x8800_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_HEAP: (usize, usize) = (0, 0);

/// The display's temporal dither phases (cold only): 64 MiB of Normal non-cacheable, EL0 read/write memory for four 16 MiB frames the display server writes once per content change and the layer flips between every 100 ms (DISPLAY-CALIB.md, the present stage). Free DRAM on a cold boot: above the camera's diagnostic buffers (which end at G#8B10_0000) and below the first firmware carveout (G#9000_0000). Granted as the DMA region `display-phases`.
#[cfg(not(shim_handoff))]
pub const DISPLAY_PHASES: (usize, usize) = (0x8C00_0000, 0x9000_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_PHASES: (usize, usize) = (0, 0);

/// The apps' surface (cold only): 32 MiB of Normal write-back, EL0 read/write, never executable memory at the bottom of the GiB above the window — a full panel's 16-bit pixels (1344 × 2992 × 8 bytes), which an app draws and the display server publishes (SHELL.md, apps as places). Free on a cold boot: Android's reserved carveouts in this GiB start at G#E800_0000 (zuma-rmem.dtsi). Granted as the region `app-surface` to the display server and, through the map, to the app in front.
#[cfg(not(shim_handoff))]
pub const APP_SURFACE: (usize, usize) = (0xC000_0000, 0xC200_0000);
#[cfg(shim_handoff)]
pub const APP_SURFACE: (usize, usize) = (0, 0);

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
/// AP[2]: read-only (for either EL in the VHE regime).
const AP_RO: u64 = 1 << 7;
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
fn dma_pool(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NC | SH_INNER | AF | if vhe { AP_EL0 | UXN | PXN } else { AP_EL2 | XN }
}
/// A pool page (VHE EL0): Normal cacheable, EL0-accessible, never executable at EL2 (PXN). `writable` clears AP[2]; `exec` clears UXN so EL0 may execute it. Default (the state a freed/fresh arc page is left in) is writable, non-executable — data.
fn pool_page(vhe: bool, addr: usize, writable: bool, exec: bool) -> u64 {
    if !vhe {
        return normal(false, addr, TABLE_OR_PAGE);
    }
    let mut d = addr as u64 | TABLE_OR_PAGE | ATTR_NORMAL | SH_INNER | AF | AP_EL0 | PXN;
    if !writable {
        d |= AP_RO;
    }
    if !exec {
        d |= UXN;
    }
    d
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
    let (q0, q1) = DMA_POOL;
    let (f0, f1) = DISPLAY_FB;
    let (h0, h1) = DISPLAY_HEAP;
    let (t0, t1) = DISPLAY_PHASES;
    let dma_ok = q0 == q1 || (q0 % MIB2 == 0 && q1 % MIB2 == 0 && q0 >= w0 && q1 <= w1 && (q1 <= p0 || q0 >= p1) && (q1 <= d0 || q0 >= d1));
    // The framebuffers and the display heap sit outside the window but in its GiB (so the level-2 table covers them), 2 MiB aligned, apart from each other.
    let outside = |a: usize, b: usize| a == b || (a % MIB2 == 0 && b % MIB2 == 0 && a / GIB == w0 / GIB && (b - 1) / GIB == w0 / GIB && (b <= w0 || a >= w1));
    let fb_ok = outside(f0, f1) && outside(h0, h1) && (f1 <= h0 || h1 <= f0 || f0 == f1 || h0 == h1) && outside(t0, t1) && (t0 == t1 || (t0 >= f1.max(h1)));
    if w0 / GIB != (w1 - 1) / GIB || d0 < w0 || d1 > w1 || (d1 - 1) / MIB2 - d0 / MIB2 >= 2 || p0 < w0 || p1 > w1 || (d0 < p1 && p0 < d1) || !dma_ok || !fb_ok {
        return false;
    }
    let vhe = has_vhe();
    unsafe {
        let l1 = &mut *(&raw mut L1);
        let l2 = &mut *(&raw mut L2);
        let l3 = &mut *(&raw mut L3);
        let pool_l3 = &mut *(&raw mut POOL_L3);
        for (i, e) in l1.0.iter_mut().enumerate() {
            *e = device(vhe, i * GIB, BLOCK);
        }
        let gib = w0 / GIB * GIB;
        l1.0[w0 / GIB] = &raw const L2 as u64 | TABLE_OR_PAGE;
        let mut next_l3 = 0;
        for (j, e) in l2.0.iter_mut().enumerate() {
            let a = gib + j * MIB2;
            *e = if (f0 != f1 && a >= f0 && a + MIB2 <= f1) || (t0 != t1 && a >= t0 && a + MIB2 <= t1) {
                dma_pool(vhe, a, BLOCK)
            } else if h0 != h1 && a >= h0 && a + MIB2 <= h1 {
                pool(vhe, a, BLOCK) | UXN
            } else if a + MIB2 <= w0 || a >= w1 {
                device(vhe, a, BLOCK)
            } else if a >= p0 && a + MIB2 <= p1 {
                // The pool at 4 KiB granularity (per-arc W^X). A fresh/freed page is data: writable, non-executable; spawn flips a process's code pages to read-only + executable.
                let t = &mut pool_l3[(a - p0) / MIB2];
                for (k, pg) in t.0.iter_mut().enumerate() {
                    *pg = pool_page(vhe, a + k * PAGE, true, false);
                }
                &raw const t.0 as u64 | TABLE_OR_PAGE
            } else if a >= q0 && a + MIB2 <= q1 {
                dma_pool(vhe, a, BLOCK)
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
        // The GiB above: Device blocks as before (the ramoops block, ABL's splash, the carveouts all live there), the apps' surface Normal.
        let (s0, s1) = APP_SURFACE;
        if s0 != s1 && s0 / GIB == w0 / GIB + 1 && s0 % MIB2 == 0 && s1 % MIB2 == 0 && (s1 - 1) / GIB == s0 / GIB {
            let l2h = &mut *(&raw mut L2_HI);
            let base = s0 / GIB * GIB;
            for (j, e) in l2h.0.iter_mut().enumerate() {
                let a = base + j * MIB2;
                *e = if a >= s0 && a + MIB2 <= s1 { pool(vhe, a, BLOCK) | UXN } else { device(vhe, a, BLOCK) };
            }
            l1.0[s0 / GIB] = &raw const L2_HI as u64 | TABLE_OR_PAGE;
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
/// Set the W^X attributes of an arc's pool pages (SANDBOX.md), then invalidate the TLB for them. A masked process marks `[exec_start, exec_end)` read-only + executable (its checked `.text`) and leaves every other page writable + non-executable (grants, read-only data, input, stack). A native dev process (`native`) gets the whole arc RWX. Call after writing the code, before entering; `arc_reset` returns every page to data on free.
pub fn set_arc_wx(base: usize, size: usize, exec_start: usize, exec_end: usize, native: bool) {
    if !vhe() {
        return;
    }
    let (p0, p1) = POOL;
    if base < p0 || base + size > p1 {
        return;
    }
    unsafe {
        let pool_l3 = &mut *(&raw mut POOL_L3);
        let mut a = base;
        while a < base + size {
            let idx = (a - p0) / PAGE;
            let exec = native || (a >= exec_start && a < exec_end);
            let writable = native || !exec;
            pool_l3[idx / 512].0[idx % 512] = pool_page(true, a, writable, exec);
            a += PAGE;
        }
        asm!("dsb ish");
        let mut a = base;
        while a < base + size {
            asm!("tlbi vae1, {x}", x = in(reg) (a as u64) >> 12);
            a += PAGE;
        }
        asm!("dsb ish", "isb");
    }
}

/// Return an arc's pages to the default (writable data, non-executable) when it is freed.
pub fn arc_reset(base: usize, size: usize) {
    set_arc_wx(base, size, 0, 0, false);
}

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
