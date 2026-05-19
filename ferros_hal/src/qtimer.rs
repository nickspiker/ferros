//! aarch64 QTIMER access (CNTPCT_EL0).
//!
//! Lives in ferros_hal instead of vsf_mini because reading this register is
//! an aarch64-specific operation, not part of the VSF format. The vsf-mini
//! crate is target-agnostic; this module is target-specific.

/// Read the QTIMER counter (CNTPCT_EL0) — monotonic, 19.2 MHz on QCM6490.
#[inline]
pub fn read_qtimer() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) val) };
    val
}
