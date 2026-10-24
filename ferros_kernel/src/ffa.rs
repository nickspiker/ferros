//! FF-A (Arm Firmware Framework) client: the transport ferros uses to reach the already-running secure world (Trusty), for starting the AoC without a TEE of our own (SENSORS.md). Modelled on Linux's Trusty driver (google-modules/trusty `trusty.c`), the only part ferros needs.
//!
//! Trusty is loaded and executing before ferros (the bootloader starts it); ferros only talks to it. The channel is proven: VERSION answers 1.1, ID_GET succeeds, ferros is partition 0, the secure endpoint is G#8000.

use core::ptr::{read_volatile, write_volatile};

/// The shared-memory FF-A fast calls: 0x8400_0000 | n (SMC_FASTCALL_NR_SHARED_MEMORY, arm_ffa.h); the 64-bit forms set bit 30 (0xC400_0000).
pub const FFA_ERROR: u64 = 0x8400_0060;
pub const FFA_SUCCESS: u64 = 0x8400_0061;
pub const FFA_VERSION: u64 = 0x8400_0063;
pub const FFA_FEATURES: u64 = 0x8400_0064;
pub const FFA_RXTX_MAP64: u64 = 0xC400_0066;
pub const FFA_RXTX_UNMAP: u64 = 0x8400_0067;
pub const FFA_ID_GET: u64 = 0x8400_0069;
pub const FFA_MEM_SHARE: u64 = 0x8400_0073;
pub const FFA_MEM_SHARE64: u64 = 0xC400_0073;
pub const FFA_MEM_RECLAIM: u64 = 0x8400_0077;

/// Major 1, minor 0: Linux's Trusty driver asks for 1.0, and the memory-transaction descriptor `mem_share` lays out is the 1.0 format (1.1 added fields to the MTD head). Requesting 1.0 makes the SPMC interpret our descriptor as 1.0. The secure world here reports 1.1 but honours a 1.0 caller.
pub const CURRENT_VERSION: u64 = 1 << 16;
/// The secure-world endpoint id (trusty.c `ffa_remote_id = 0x8000`).
pub const SECURE_ID: u64 = 0x8000;
pub const PAGE: usize = 4096;

/// FF-A's own RX/TX pair, one 4 KiB page each, shared with the secure world by RXTX_MAP. In ferros's non-cacheable DMA memory (identity-mapped, no cache maintenance), as every buffer a device or the secure world reads must be.
#[repr(C, align(4096))]
pub struct Page(pub [u8; PAGE]);
#[unsafe(link_section = ".bss.dma")]
pub static mut FFA_TX: Page = Page([0; PAGE]);
#[unsafe(link_section = ".bss.dma")]
pub static mut FFA_RX: Page = Page([0; PAGE]);

/// This endpoint's id, filled by `init`.
pub static mut LOCAL_ID: u16 = 0;

/// The 8-register SMC (SMCCC: x0..x7 in and out), which FF-A needs (RXTX_MAP returns in x2, MEM_SHARE the handle in x2/x3).
pub fn smc8(a: [u64; 8]) -> [u64; 8] {
    let (mut r0, mut r1, mut r2, mut r3, mut r4, mut r5, mut r6, mut r7);
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") a[0] => r0, inout("x1") a[1] => r1, inout("x2") a[2] => r2, inout("x3") a[3] => r3,
            inout("x4") a[4] => r4, inout("x5") a[5] => r5, inout("x6") a[6] => r6, inout("x7") a[7] => r7,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _,
        );
    }
    [r0, r1, r2, r3, r4, r5, r6, r7]
}

fn call(a: [u64; 8]) -> Result<[u64; 8], i32> {
    let r = smc8(a);
    if r[0] == FFA_ERROR {
        Err(r[2] as i32)
    } else {
        Ok(r)
    }
}

/// Bring FF-A up as Linux's Trusty driver does (`trusty_init_api_version` path, trusty.c): check the version and that MEM_SHARE is implemented, learn our id, and map the RX/TX pair. Returns the negotiated version.
pub fn init() -> Result<u32, i32> {
    let v = smc8([FFA_VERSION, CURRENT_VERSION, 0, 0, 0, 0, 0, 0])[0];
    if (v >> 16) & 0x7FFF != 1 {
        return Err(-1);
    }
    call([FFA_FEATURES, FFA_MEM_SHARE, 0, 0, 0, 0, 0, 0])?;
    let id = call([FFA_ID_GET, 0, 0, 0, 0, 0, 0, 0])?[2];
    unsafe { LOCAL_ID = id as u16 };
    let tx = (&raw const FFA_TX) as u64;
    let rx = (&raw const FFA_RX) as u64;
    // A warm ferros reload does not reset the secure world, so a prior boot's RX/TX pair may still be mapped (RXTX_MAP would then return DENIED); unmap first, ignoring the error on a cold boot where nothing is mapped.
    let _ = smc8([FFA_RXTX_UNMAP, 0, 0, 0, 0, 0, 0, 0]);
    // RXTX_MAP (64-bit): tx page, rx page, count in 4 KiB pages.
    call([FFA_RXTX_MAP64, tx, rx, 1, 0, 0, 0, 0])?;
    Ok(v as u32)
}

/// Read/write the TX buffer as bytes (where a MEM_SHARE descriptor is laid out before the call).
pub fn tx_ptr() -> *mut u8 {
    (&raw mut FFA_TX) as *mut u8
}
pub fn rx_ptr() -> *const u8 {
    (&raw const FFA_RX) as *const u8
}

/// Helpers for little-endian writes into a buffer.
pub fn w16(p: *mut u8, off: usize, v: u16) {
    unsafe { write_volatile(p.add(off) as *mut u16, v) }
}
pub fn w32(p: *mut u8, off: usize, v: u32) {
    unsafe { write_volatile(p.add(off) as *mut u32, v) }
}
pub fn w64(p: *mut u8, off: usize, v: u64) {
    unsafe { write_volatile(p.add(off) as *mut u64, v) }
}
pub fn r32(p: *const u8, off: usize) -> u32 {
    unsafe { read_volatile(p.add(off) as *const u32) }
}

/// FF-A attributes for the shared buffers: Normal memory, write-back cacheable, inner-shareable (FFA_MEM_ATTR_NORMAL_MEMORY_CACHED_WB | INNER_SHAREABLE = 0x2C | 0x03), matching Linux's Trusty driver and ferros's own cacheable window, so the secure world and ferros stay coherent on the same cores. Read+write permission.
const MEM_ATTR_WB_INNER: u8 = 0x2F;
const MEM_PERM_RW: u8 = 0x2;

/// Share a physically contiguous, 4 KiB-aligned region [pa, pa+pages*4096) with the secure world (FFA_MEM_SHARE, trusty.c `trusty_transfer_memory`). The descriptor is laid out in the mapped TX buffer; the call returns the 64-bit memory handle Trusty refers to the region by. ferros is identity-mapped, so a buffer's address is its physical address.
pub fn mem_share(pa: usize, pages: u32) -> Result<u64, i32> {
    let tx = tx_ptr();
    unsafe { core::ptr::write_bytes(tx, 0, 80) };
    // ffa_mtd head (32 bytes).
    w16(tx, 0, unsafe { LOCAL_ID }); // sender_id
    unsafe { *tx.add(2) = MEM_ATTR_WB_INNER }; // memory_region_attributes
    w32(tx, 4, 0); // flags = 0 (MEM_SHARE)
    w64(tx, 8, 0); // handle = 0
    w64(tx, 16, 0); // tag
    w32(tx, 28, 1); // emad_count
    // ffa_emad[0] (16 bytes) at off 32.
    w16(tx, 32, SECURE_ID as u16); // endpoint_id
    unsafe { *tx.add(34) = MEM_PERM_RW }; // memory_access_permissions
    unsafe { *tx.add(35) = 0 }; // flags
    w32(tx, 36, 48); // comp_mrd_offset = offsetof(mtd, emad[1]) = 32 + 16
    // ffa_comp_mrd at off 48 (16-byte head).
    w32(tx, 48, pages); // total_page_count
    w32(tx, 52, 1); // address_range_count
    // ffa_cons_mrd[0] at off 64 (16 bytes).
    w64(tx, 64, pa as u64); // address
    w32(tx, 72, pages); // page_count
    let total_len = 80u64;
    unsafe { core::arch::asm!("dsb sy") };
    let r = smc8([FFA_MEM_SHARE, total_len, total_len, 0, 0, 0, 0, 0]);
    if r[0] == FFA_SUCCESS {
        Ok(r[2] | (r[3] << 32))
    } else {
        Err(r[2] as i32)
    }
}

/// Hand a shared region back (teardown; not in the critical path).
pub fn mem_reclaim(handle: u64) -> Result<(), i32> {
    let r = smc8([FFA_MEM_RECLAIM, handle & 0xFFFF_FFFF, handle >> 32, 0, 0, 0, 0, 0]);
    if r[0] == FFA_SUCCESS { Ok(()) } else { Err(r[2] as i32) }
}
