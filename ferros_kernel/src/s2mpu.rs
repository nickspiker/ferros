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

fn rd(off: usize) -> u32 {
    unsafe { read_volatile((BASE + off) as *const u32) }
}
fn wr(off: usize, v: u32) {
    unsafe { write_volatile((BASE + off) as *mut u32, v) }
}

/// What `fence` did: 1 = fenced, 0 = not yet run, G#E1 = not an S2MPU v9, G#E2 = invalidation never completed, G#E3 = the DMA memory doesn't sit in one GiB.
static RESULT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

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
    if rd(VERSION) & 0xFFFF_0000 != 0x9000_0000 {
        return 0xE1;
    }
    // The table: RW (G#C) for UFS's DMA pages and the DMA pool, nothing else in their GiB.
    let dma = unsafe { (&raw const __dma_ufs_start as usize, &raw const __dma_ufs_end as usize) };
    let pool = crate::cachemap::DMA_POOL;
    let gb = dma.0 >> 30;
    if (dma.1 - 1) >> 30 != gb || (pool.0 != pool.1 && (pool.0 >> 30 != gb || (pool.1 - 1) >> 30 != gb)) {
        return 0xE3;
    }
    let base = gb << 30;
    let allowed = |pa: usize| (pa >= dma.0 && pa < dma.1) || (pa >= pool.0 && pa < pool.1);
    let t = unsafe { &mut *(&raw mut WINDOW_TABLE) };
    for (w, e) in t.0.iter_mut().enumerate() {
        let mut v = 0u32;
        for n in 0..8 {
            if allowed(base + ((w * 8 + n) << 12)) {
                v |= 0xC << (4 * n);
            }
        }
        *e = v;
    }
    // Clean the table to the point of coherency: the walker reads DRAM, not the cache.
    let (t0, t1) = (&raw const WINDOW_TABLE as usize, &raw const WINDOW_TABLE as usize + core::mem::size_of::<Smpt>());
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
    let n = (rd(NUM_CONTEXT) & 0xF) as usize;
    let mut ctx = 0u32;
    for i in 0..n.min(VIDS) {
        ctx |= ((i as u32) | 8) << (4 * i);
    }
    wr(CONTEXT_CFG_VALID_VID, ctx);
    let table = (&raw const WINDOW_TABLE as usize >> 4) as u32;
    for vid in 0..VIDS {
        for g in (0..4).chain(34..GBS) {
            if g == gb && vid == 0 {
                wr(l1_table(vid, g), table);
                wr(l1_attr(vid, g), L2TABLE_EN);
            } else {
                wr(l1_attr(vid, g), 0);
            }
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    wr(ALL_INVALIDATION, 1);
    wr(SYNC_CMD, 1);
    let freq = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while rd(SYNC_COMP) & 1 == 0 || rd(STATUS) & 3 == 3 {
        if crate::platform::cntpct().wrapping_sub(t0) > freq / 100 {
            return 0xE2;
        }
    }
    wr(ERR_RESP_PER_VID_SET, 0xFF);
    wr(INTERRUPT_ENABLE_PER_VID_SET, 0xFF);
    wr(CTRL0, 0);
    wr(PROT_EN_PER_VID_SET, 0xFF);
    wr(CFG, 0);
    unsafe { core::arch::asm!("dsb sy") };
    1
}

/// DIAG lines: the fence result and any latched faults (per faulting VID: PA, info with the VID in [26:24], info2 with the stream ID in [23:0]).
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    use crate::platform::append_hex;
    append_hex(out, b"S2MPU_FENCE=", result());
    let st = rd(FAULT_STATUS);
    append_hex(out, b"S2MPU_FAULT_STATUS=", st);
    for vid in 0..VIDS {
        if st & (1 << vid) != 0 {
            append_hex(out, b"S2MPU_FAULT_PA=", rd(fault_pa_low(vid)));
            append_hex(out, b"S2MPU_FAULT_INFO=", rd(fault_info(vid)));
            append_hex(out, b"S2MPU_FAULT_INFO2=", rd(fault_info2(vid)));
        }
    }
}
