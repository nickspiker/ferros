//! ferros HAL — Apple M1 / m1n1 EL2
//!
//! Stub implementations. Apple's USB controller (XHCI variant) is not yet
//! implemented. On M1, m1n1 proxy mode handles USB at the bootloader level —
//! ferros communicates via the framebuffer console until native USB is ready.

#![no_std]

pub mod usb;
