//! Volatile MMIO register access.
//!
//! Every hardware register read/write goes through these primitives.
//! The compiler must never optimize away, reorder, or cache these.
//!
//! No unsafe wrappers around unsafe — the callers are all hardware
//! drivers that inherently know what address they're poking.

use core::ptr;

/// Read a 32-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address.
#[inline(always)]
pub unsafe fn read32(addr: usize) -> u32 {
    unsafe { ptr::read_volatile(addr as *const u32) }
}

/// Write a 32-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address.
#[inline(always)]
pub unsafe fn write32(addr: usize, val: u32) {
    unsafe { ptr::write_volatile(addr as *mut u32, val) }
}

/// Read a 64-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address.
#[inline(always)]
pub unsafe fn read64(addr: usize) -> u64 {
    unsafe { ptr::read_volatile(addr as *const u64) }
}

/// Write a 64-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address.
#[inline(always)]
pub unsafe fn write64(addr: usize, val: u64) {
    unsafe { ptr::write_volatile(addr as *mut u64, val) }
}

/// Read a 16-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address (2-byte aligned).
#[inline(always)]
pub unsafe fn read16(addr: usize) -> u16 {
    unsafe { ptr::read_volatile(addr as *const u16) }
}

/// Write a 16-bit MMIO register.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address (2-byte aligned).
#[inline(always)]
pub unsafe fn write16(addr: usize, val: u16) {
    unsafe { ptr::write_volatile(addr as *mut u16, val) }
}

/// Read a byte from an MMIO address.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address.
#[inline(always)]
pub unsafe fn read8(addr: usize) -> u8 {
    unsafe { ptr::read_volatile(addr as *const u8) }
}

/// Write a byte to an MMIO address.
///
/// # Safety
/// `addr` must be a valid, mapped MMIO register address.
#[inline(always)]
pub unsafe fn write8(addr: usize, val: u8) {
    unsafe { ptr::write_volatile(addr as *mut u8, val) }
}

/// A typed MMIO register at a fixed address.
///
/// Wraps a base address + offset into a reusable handle.
/// Prevents fat-fingering addresses across a driver.
pub struct Reg32 {
    addr: usize,
}

impl Reg32 {
    /// Create a register handle.
    ///
    /// # Safety
    /// The address must be a valid MMIO register for the lifetime of this handle.
    pub const unsafe fn new(addr: usize) -> Self {
        Self { addr }
    }

    #[inline(always)]
    pub fn read(&self) -> u32 {
        unsafe { read32(self.addr) }
    }

    #[inline(always)]
    pub fn write(&self, val: u32) {
        unsafe { write32(self.addr, val) }
    }

    /// Read-modify-write: set bits in mask.
    #[inline(always)]
    pub fn set_bits(&self, mask: u32) {
        self.write(self.read() | mask);
    }

    /// Read-modify-write: clear bits in mask.
    #[inline(always)]
    pub fn clear_bits(&self, mask: u32) {
        self.write(self.read() & !mask);
    }
}
