//! ferros HAL — Apple M1 / m1n1 EL2
//!
//! Platform-specific drivers for the Apple M1 MacBook Air (T8103).
//! DWC3 USB device mode with DART IOMMU, ATCPHY, PipeHandler init.

#![no_std]

pub mod dart;
pub mod usb;
