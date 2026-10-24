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
pub const FFA_MEM_SHARE64: u64 = 0xC400_0073;
pub const FFA_MEM_RECLAIM: u64 = 0x8400_0077;

/// Major 1, minor 1 (what Trusty here reports; Linux asks 1.0 and accepts a higher minor).
pub const CURRENT_VERSION: u64 = (1 << 16) | 1;
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
    call([FFA_FEATURES, FFA_MEM_SHARE64, 0, 0, 0, 0, 0, 0])?;
    let id = call([FFA_ID_GET, 0, 0, 0, 0, 0, 0, 0])?[2];
    unsafe { LOCAL_ID = id as u16 };
    let tx = (&raw const FFA_TX) as u64;
    let rx = (&raw const FFA_RX) as u64;
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
