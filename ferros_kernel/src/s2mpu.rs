//! The HSI2 S2MPU fence: what the DMA masters behind HSI2 (UFS, PCIe ch1 = the WiFi chip) may reach in DRAM.
//!
//! ABL leaves this S2MPU on; cold_ufs used to turn it off for every VID (CTRL_PROT_EN_PER_VID_CLR = G#FF), which let the WiFi chip — a bus master running proprietary firmware — DMA anywhere, the kernel included. The fence replaces that: deny by default, allow only ferros's own memory.
//!
//! Hardware (S2MPU v9, pkvm-s2mpu kvm_s2mpu.h / hyp s2mpu.c): per VID (8), one L1 entry per GiB of a 36-bit physical space; an entry is either a 1 GiB permission (ATTR bits 2:1 = R/W) or a pointer to a 4 KiB-granule table (SMPT: 4 bits per page, bit 2 R, bit 3 W, 128 KiB per GiB, aligned to its size). GiB 4..33 are fixed no-access in hardware. The table walker is not coherent, so the tables live in `.bss.dma` (non-cacheable).
//!
//! Stage 1 (now): every VID gets the same rules — GiB 2 (G#8000_0000..) through a table allowing only the cold window (kernel image, heap, arc pool, DMA pool, staging: G#8000_0000..G#8240_0000), every other writable GiB no-access. Denied accesses return DECERR and latch the fault registers (FAULT_STATUS bit per VID; FAULT_INFO VID[26:24] / INFO2 stream ID), so the first PCIe DMA that strays names PCIe's VID (zuma's DT doesn't: gs101 put PCIe on VID 1, UFS on 0).
//! Stage 2 (once the VID is measured): that VID gets a table allowing only its DMA region (process.rs `DMA`).

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

/// The cold window, the only memory stage 1 lets any HSI2 master reach.
const WINDOW: (usize, usize) = (0x8000_0000, 0x8240_0000);

/// One GiB of 4 KiB permissions: 262144 nibbles.
#[repr(C, align(131072))]
struct Smpt([u32; 32768]);

#[unsafe(link_section = ".bss.dma")]
static mut WINDOW_TABLE: Smpt = Smpt([0; 32768]);

fn rd(off: usize) -> u32 {
    unsafe { read_volatile((BASE + off) as *const u32) }
}
fn wr(off: usize, v: u32) {
    unsafe { write_volatile((BASE + off) as *mut u32, v) }
}

/// What `fence` did: 1 = fenced, 0 = not yet run, G#E1 = not an S2MPU v9, G#E2 = invalidation never completed.
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
    // The window table: RW (G#C) for every page inside the window, nothing else in its GiB.
    let gb = WINDOW.0 >> 30;
    let first = (WINDOW.0 - (gb << 30)) >> 12;
    let last = (WINDOW.1 - (gb << 30)) >> 12;
    let t = unsafe { &mut *(&raw mut WINDOW_TABLE) };
    for (w, e) in t.0.iter_mut().enumerate() {
        let mut v = 0u32;
        for n in 0..8 {
            let page = w * 8 + n;
            if page >= first && page < last {
                v |= 0xC << (4 * n);
            }
        }
        *e = v;
    }
    unsafe { core::arch::asm!("dsb sy") };
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
            if g == gb {
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
