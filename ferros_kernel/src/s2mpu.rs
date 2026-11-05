//! The HSI2 S2MPU fence: what the DMA masters behind HSI2 (UFS, PCIe ch1 = the WiFi chip) may reach in DRAM.
//!
//! ABL leaves this S2MPU on; cold_ufs used to turn it off for every VID (CTRL_PROT_EN_PER_VID_CLR = G#FF), which let the WiFi chip — a bus master running proprietary firmware — DMA anywhere, the kernel included. The fence replaces that: deny by default, allow only ferros's own memory.
//!
//! Hardware (S2MPU v9, pkvm-s2mpu kvm_s2mpu.h / hyp s2mpu.c): per VID (8), one L1 entry per GiB of a 36-bit physical space; an entry is either a 1 GiB permission (ATTR bits 2:1 = R/W) or a pointer to a 4 KiB-granule table (SMPT: 4 bits per page, bit 2 R, bit 3 W, 128 KiB per GiB, aligned to its size). GiB 4..33 are fixed no-access in hardware. The table walker is not coherent: the table is cleaned to the point of coherency after it is written.
//!
//! Measured on husky (2026-10-15): the WiFi chip (PCIe ch1, stream 4) and UFS both arrive as **VID 0** — an ioctl whose request buffer sat at G#9000_0000 latched FAULT_INFO G#004F_0002 (permission, read) on VID 0, and UFS kept working with VIDs 1..7 denied. One VID means the S2MPU can't give them different windows (the HSI2 SSMT that could move stream 4 to another VID has no known address).
//!
//! So VID 0 may reach exactly the two things those devices DMA into: UFS's own descriptors and buffers (`.bss.dma.ufs`, on pages of their own) and the DMA regions granted to drivers (cachemap DMA_POOL). Not the kernel image, heap, stack or arc pool, not the USB/PT DMA buffers beside UFS's (a device writing there could forge Photon Transport commands), and not this table (it lives outside `.bss.dma`, or a device could rewrite its own fence). VIDs 1..7 get nothing; every GiB but the one holding the window is no-access. Denied accesses return DECERR and latch the fault registers (FAULT_STATUS bit per VID; FAULT_INFO VID[26:24] / INFO2 stream ID), reported in DIAG.
//! What is left shared: the WiFi chip can reach UFS's DMA buffers (vault data there is authenticated). Separating them needs the PCIe-only SysMMU in front of ch1 (sysmmu_hsi2 G#131C_0000, WIFI.md §12).

use core::ptr::{read_volatile, write_volatile};

const BASE: usize = 0x131F_0000;
const CTRL0: usize = 0x000;
const CFG: usize = 0x010;
const INTERRUPT_ENABLE_PER_VID_SET: usize = 0x020;
const INTERRUPT_CLEAR: usize = 0x02C;
const PROT_EN_PER_VID_SET: usize = 0x050;
const ERR_RESP_PER_VID_SET: usize = 0x070;
const VERSION: usize = 0x060;
const STATUS: usize = 0x068;
const NUM_CONTEXT: usize = 0x100;
const CONTEXT_CFG_VALID_VID: usize = 0x104;
const ALL_INVALIDATION: usize = 0x1000;
const SYNC_CMD: usize = 0x102C;
const SYNC_COMP: usize = 0x1030;
const FAULT_STATUS: usize = 0x2000;
const fn fault_pa_low(vid: usize) -> usize {
    0x2004 + vid * 0x20
}
const fn fault_info(vid: usize) -> usize {
    0x2010 + vid * 0x20
}
const fn fault_info2(vid: usize) -> usize {
    0x2018 + vid * 0x20
}
const fn l1_table(vid: usize, gb: usize) -> usize {
    0x4000 + vid * 0x200 + gb * 8
}
const fn l1_attr(vid: usize, gb: usize) -> usize {
    0x4004 + vid * 0x200 + gb * 8
}
const VIDS: usize = 8;
const GBS: usize = 64;
const L2TABLE_EN: u32 = 1;

unsafe extern "C" {
    static __dma_ufs_start: u8;
    static __dma_ufs_end: u8;
}

/// One GiB of 4 KiB permissions: 262144 nibbles.
#[repr(C, align(131072))]
struct Smpt([u32; 32768]);

static mut WINDOW_TABLE: Smpt = Smpt([0; 32768]);
/// The DPUF S2MPUs' table: both serve the same display, so one window.
static mut DPU_TABLE: Smpt = Smpt([0; 32768]);

/// The DPU's S2MPUs (zuma-s2mpu.dtsi s2mpu_s0_dpuf0/1), inside the DPUF power domains: reachable only while those are on (dpu_power::all_on).
pub const DPU_S2MPUS: [usize; 2] = [0x1987_0000, 0x19C7_0000];

fn rd(base: usize, off: usize) -> u32 {
    unsafe { read_volatile((base + off) as *const u32) }
}
fn wr(base: usize, off: usize, v: u32) {
    unsafe { write_volatile((base + off) as *mut u32, v) }
}

/// What `fence` did: 1 = fenced, 0 = not yet run, G#E1 = not an S2MPU v9, G#E2 = invalidation never completed, G#E3 = the DMA memory doesn't sit in one GiB.
static RESULT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The same for `fence_dpu` (both of the DPU's, the first failure if one failed).
static DPU_RESULT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn result() -> u32 {
    RESULT.load(core::sync::atomic::Ordering::Relaxed)
}

/// Turn the fence on (stage 1). Idempotent: safe to call again after anything that switched protection off.
pub fn fence() -> u32 {
    let r = fence_inner();
    RESULT.store(r, core::sync::atomic::Ordering::Relaxed);
    r
}

fn fence_inner() -> u32 {
    // The table: RW (G#C) for UFS's DMA pages and the DMA pool, nothing else in their GiB.
    let dma = unsafe { (&raw const __dma_ufs_start as usize, &raw const __dma_ufs_end as usize) };
    let pool = crate::cachemap::DMA_POOL;
    let gb = dma.0 >> 30;
    if (dma.1 - 1) >> 30 != gb || (pool.0 != pool.1 && (pool.0 >> 30 != gb || (pool.1 - 1) >> 30 != gb)) {
        return 0xE3;
    }
    fence_at(BASE, &raw mut WINDOW_TABLE, gb, 0xC, 1, &|pa| (pa >= dma.0 && pa < dma.1) || (pa >= pool.0 && pa < pool.1))
}

/// The DPU's fence (owner, 2026-11-04: bound the display's bus master as the HSI2 one is): VID 0, where the layer DMA arrives (its faults latched there), may **read** the two regions the display server points it at — `display-fb` and `display-phases` (cachemap) — and nothing else; it never writes (no write-back DMA). ABL leaves these S2MPUs off, which let the display engine read anything, the kernel included, at an address a process programs; a domain's reset turns them on over a table of power-on noise. Called at boot when the DPU is on and by the `dpu` switch after it powers the domains.
pub fn fence_dpu() -> u32 {
    let (fb, ph) = (crate::cachemap::DISPLAY_FB, crate::cachemap::DISPLAY_PHASES);
    let gb = fb.0 >> 30;
    let r = if [fb.0, fb.1 - 1, ph.0, ph.1 - 1].iter().any(|&a| a >> 30 != gb) {
        0xE3
    } else {
        let allowed = |pa: usize| (pa >= fb.0 && pa < fb.1) || (pa >= ph.0 && pa < ph.1);
        DPU_S2MPUS.iter().map(|&s| fence_at(s, &raw mut DPU_TABLE, gb, 0x4, 1, &allowed)).find(|&r| r != 1).unwrap_or(1)
    };
    DPU_RESULT.store(r, core::sync::atomic::Ordering::Relaxed);
    r
}

/// The CPUCL0 block's S2MPU (zuma-s2mpu.dtsi s2mpu_s0_cpucl0, always on): in front of the CoreSight system's bus master (CSSYS, STM, the debug port's AXI path: the block's IPs in cmucal-sfr.c), not the cores, whose traffic reaches memory through the cluster. ABL and Android leave it open.
pub const CPUCL0_S2MPU: usize = 0x29C5_0000;

/// The MISC block's S2MPU (zuma-s2mpu.dtsi s2mpu_s0_misc, always on): in front of BLK_MISC's bus masters, PDMA1, the secure SPDMA1 and DIT (the packet engine Linux's modem path offloads to; cmucal-sfr.c), none of which ferros uses. ABL leaves it open (measured 2026-11-04: protection 0). Denied all on a cold boot (`deny_all`), as the hv denies it at the jump, under which ferros already runs.
pub const MISC_S2MPU: usize = 0x101F_0000;

/// The HSI1 S2MPU (zuma-s2mpu.dtsi s2mpu_s0_hsi1): in front of PCIe ch0, the cellular modem's link, the one radio any base station can reach before anyone attests. ABL leaves it on with protection off (measured 2026-11-04: G#1209_0058 = 0), so the modem could DMA anywhere in DRAM, the kernel included; until then the modem driver also turned its protection off itself before the ROM boot.
pub const MODEM_S2MPU: usize = 0x1209_0000;
/// HSI1's power-domain status in the PMU (a plain read; bit 0 on): the S2MPU is reachable only while it is on.
const PD_HSI1_STATUS: usize = 0x1546_2B04;
/// The modem's carve-outs (zuma-rmem.dtsi): cp_rmem_1 (32 MiB: pktproc), cp_rmem (12 MiB: the IPC region, the boot image), cp_msi_rmem (one page: the MSI target). cp_aoc_rmem (G#197F_D000, in the AoC's SRAM) is left out: no fault has asked for it.
const MODEM_CARVEOUTS: [(usize, usize); 3] = [(0xE800_0000, 0xEA00_0000), (0xEA40_0000, 0xEB00_0000), (0xF620_0000, 0xF620_1000)];
static mut MODEM_TABLE: Smpt = Smpt([0; 32768]);
static MODEM_RESULT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The modem's fence: read and write to its carve-outs as the device tree has them (zuma-rmem.dtsi: cp_rmem_1, cp_rmem, cp_msi_rmem), nothing else. Wider than the driver's own windows (cachemap `MODEM_WINDOWS`): the CP reads cp_rmem past the 4 MiB the driver maps (the first fence, to the driver's windows, faulted on a read at G#EA80_0000, 2026-11-04). VID 0, as the fault showed PCIe ch0's traffic arrives (as the WiFi chip's does on HSI2). G#E4: HSI1 not powered (left alone: an access with the domain down poisons EL3).
pub fn fence_modem() -> u32 {
    let w = MODEM_CARVEOUTS;
    let gb = w[0].0 >> 30;
    let r = if unsafe { read_volatile(PD_HSI1_STATUS as *const u32) } & 1 == 0 {
        0xE4
    } else if w.iter().any(|&(a, b)| a >> 30 != gb || (b - 1) >> 30 != gb) {
        0xE3
    } else {
        fence_at(MODEM_S2MPU, &raw mut MODEM_TABLE, gb, 0xC, 1, &|pa| w.iter().any(|&(a, b)| pa >= a && pa < b))
    };
    MODEM_RESULT.store(r, core::sync::atomic::Ordering::Relaxed);
    r
}

/// The G3D S2MPU (zuma-s2mpu.dtsi s2mpu_s0_g3d, inside pd_g3d): in front of the GPU's bus master. The GPU is the one bus master a process programs directly (its page tables are the GPU driver's), so its fence is what keeps a GPU job inside the memory ferros gave it.
pub const GPU_S2MPU: usize = 0x1EE7_0000;
/// pd_g3d's status in the PMU (GPU.md: a plain read, bit 0 on): the S2MPU is reachable only while it is on.
const PD_G3D_STATUS: usize = 0x1546_1A84;
static mut GPU_TABLE: Smpt = Smpt([0; 32768]);
static GPU_RESULT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The GPU's fence: read and write to `gpu-mem` (cachemap GPU_MEM), nothing else, for every VID (only the GPU sits behind it; which VID its traffic arrives as is not measured yet). G#E4: pd_g3d not powered (left alone).
pub fn fence_gpu() -> u32 {
    let m = crate::cachemap::GPU_MEM;
    let gb = m.0 >> 30;
    let r = if m.0 == m.1 {
        0xE3
    } else if unsafe { read_volatile(PD_G3D_STATUS as *const u32) } & 1 == 0 {
        0xE4
    } else if (m.1 - 1) >> 30 != gb {
        0xE3
    } else {
        fence_at(GPU_S2MPU, &raw mut GPU_TABLE, gb, 0xC, 0xFF, &|pa| pa >= m.0 && pa < m.1)
    };
    GPU_RESULT.store(r, core::sync::atomic::Ordering::Relaxed);
    r
}

/// Whether `base` is one of the DMA fences: no process writes one (process.rs SYS_MMIO_WRITE refuses it), whatever its grants; the fences are the kernel's (KERNEL-OUT.md "What stays").
pub fn is_fence(base: usize) -> bool {
    base == BASE || base == MODEM_S2MPU || base == MISC_S2MPU || base == GPU_S2MPU || base == CPUCL0_S2MPU || DPU_S2MPUS.contains(&base)
}

/// One S2MPU set to let no VID reach any memory (the hv's `s2mpu_deny_all` at the jump, the same steps): every context valid, every L1 entry no-access, invalidated and synced, then protection on with error responses. 1 done, G#E1 not a v9, G#E2 the sync never completed.
pub fn deny_all(base: usize) -> u32 {
    if rd(base, VERSION) & 0xFFFF_0000 != 0x9000_0000 {
        return 0xE1;
    }
    let n = (rd(base, NUM_CONTEXT) & 0xF) as usize;
    let mut ctx = 0u32;
    for i in 0..n.min(VIDS) {
        ctx |= ((i as u32) | 8) << (4 * i);
    }
    wr(base, CONTEXT_CFG_VALID_VID, ctx);
    for vid in 0..VIDS {
        for g in (0..4).chain(34..GBS) {
            wr(base, l1_attr(vid, g), 0);
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    wr(base, ALL_INVALIDATION, 1);
    wr(base, SYNC_CMD, 1);
    let freq = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while rd(base, SYNC_COMP) & 1 == 0 || rd(base, STATUS) & 3 == 3 {
        if crate::platform::cntpct().wrapping_sub(t0) > freq / 100 {
            return 0xE2;
        }
    }
    wr(base, ERR_RESP_PER_VID_SET, 0xFF);
    wr(base, CTRL0, 0);
    wr(base, PROT_EN_PER_VID_SET, 0xFF);
    wr(base, CFG, 0);
    unsafe { core::arch::asm!("dsb sy") };
    1
}

pub fn dpu_result() -> u32 {
    DPU_RESULT.load(core::sync::atomic::Ordering::Relaxed)
}

/// One S2MPU fenced: the VIDs in mask `vids` reach the pages of GiB `gb` that `allowed` names, with permission `perm` (SMPT nibble: G#4 read, G#8 write), through `table` (one table, shared by those VIDs); every other GiB and every other VID nothing.
fn fence_at(base: usize, table: *mut Smpt, gb: usize, perm: u32, vids: u8, allowed: &dyn Fn(usize) -> bool) -> u32 {
    if rd(base, VERSION) & 0xFFFF_0000 != 0x9000_0000 {
        return 0xE1;
    }
    let gb_base = gb << 30;
    let t = unsafe { &mut *table };
    for (w, e) in t.0.iter_mut().enumerate() {
        let mut v = 0u32;
        for n in 0..8 {
            if allowed(gb_base + ((w * 8 + n) << 12)) {
                v |= perm << (4 * n);
            }
        }
        *e = v;
    }
    // Clean the table to the point of coherency: the walker reads DRAM, not the cache.
    let (t0, t1) = (table as usize, table as usize + core::mem::size_of::<Smpt>());
    unsafe {
        core::arch::asm!("dsb sy");
        let mut a = t0;
        while a < t1 {
            core::arch::asm!("dc cvac, {}", in(reg) a);
            a += 64;
        }
        core::arch::asm!("dsb sy");
    }
    // Contexts: context i serves VID i, all valid (L1 writes for a VID without a context are ignored).
    let n = (rd(base, NUM_CONTEXT) & 0xF) as usize;
    let mut ctx = 0u32;
    for i in 0..n.min(VIDS) {
        ctx |= ((i as u32) | 8) << (4 * i);
    }
    wr(base, CONTEXT_CFG_VALID_VID, ctx);
    let tab = (table as usize >> 4) as u32;
    for vid in 0..VIDS {
        for g in (0..4).chain(34..GBS) {
            if g == gb && vids >> vid & 1 == 1 {
                wr(base, l1_table(vid, g), tab);
                wr(base, l1_attr(vid, g), L2TABLE_EN);
            } else {
                wr(base, l1_attr(vid, g), 0);
            }
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    wr(base, ALL_INVALIDATION, 1);
    wr(base, SYNC_CMD, 1);
    let freq = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while rd(base, SYNC_COMP) & 1 == 0 || rd(base, STATUS) & 3 == 3 {
        if crate::platform::cntpct().wrapping_sub(t0) > freq / 100 {
            return 0xE2;
        }
    }
    // Faults latched before this fence (ABL's, a previous kernel's across a reload) cleared, so the report shows only what this fence refused.
    wr(base, INTERRUPT_CLEAR, 0xFF);
    wr(base, ERR_RESP_PER_VID_SET, 0xFF);
    wr(base, INTERRUPT_ENABLE_PER_VID_SET, 0xFF);
    wr(base, CTRL0, 0);
    wr(base, PROT_EN_PER_VID_SET, 0xFF);
    wr(base, CFG, 0);
    unsafe { core::arch::asm!("dsb sy") };
    1
}
/// DIAG lines: the fence result and any latched faults (per faulting VID: PA, info with the VID in [26:24], info2 with the stream ID in [23:0]).
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    use crate::platform::append_hex;
    append_hex(out, b"S2MPU_FENCE=", result());
    faults(out, BASE);
    append_hex(out, b"S2MPU_MODEM_FENCE=", MODEM_RESULT.load(core::sync::atomic::Ordering::Relaxed));
    if unsafe { read_volatile(PD_HSI1_STATUS as *const u32) } & 1 == 1 {
        faults(out, MODEM_S2MPU);
    }
    append_hex(out, b"S2MPU_MISC_PROT=", rd(MISC_S2MPU, 0x58));
    faults(out, MISC_S2MPU);
    append_hex(out, b"S2MPU_GPU_FENCE=", GPU_RESULT.load(core::sync::atomic::Ordering::Relaxed));
    if unsafe { read_volatile(PD_G3D_STATUS as *const u32) } & 1 == 1 {
        faults(out, GPU_S2MPU);
    }
    append_hex(out, b"S2MPU_DPU_FENCE=", dpu_result());
    // The DPU's only while its domains are on: an access with a domain down poisons EL3 (display.rs).
    if crate::dpu_power::all_on() {
        for s in DPU_S2MPUS {
            faults(out, s);
        }
    }
}

fn faults(out: &mut alloc::vec::Vec<u8>, base: usize) {
    use crate::platform::append_hex;
    let st = rd(base, FAULT_STATUS);
    append_hex(out, b"S2MPU_FAULT_STATUS=", st);
    for vid in 0..VIDS {
        if st & (1 << vid) != 0 {
            append_hex(out, b"S2MPU_FAULT_PA=", rd(base, fault_pa_low(vid)));
            append_hex(out, b"S2MPU_FAULT_INFO=", rd(base, fault_info(vid)));
            append_hex(out, b"S2MPU_FAULT_INFO2=", rd(base, fault_info2(vid)));
        }
    }
}
