//! Ferros Hardware Abstraction Layer
//!
//! MMIO registers, UART, framebuffer, DTB parsing, SD/MMC.
//! `#![no_std]` — runs bare metal or with alloc provided by the kernel.
//!
//! ## Modules
//!
//! - [`mmio`]: Volatile MMIO register access primitives.
//! - [`uart`]: UART serial output (QCM6490 GENI SE or PL011 for QEMU).
//! - [`fb`]: SimpleFB — map ABL's framebuffer and push pixels.
//! - [`dtb`]: Minimal DTB parser — find simplefb node and reserved-memory.
//! - [`sdmmc`]: SD/MMC controller driver for microSD access.

#![no_std]

extern crate alloc;

pub mod mmio;
pub mod uart;
pub mod fb;
pub mod dtb;
pub mod sdmmc;
pub mod console;
pub mod dpu;
pub mod pstore;
pub mod spmi;
pub mod usb;
pub mod gcc;
pub mod rpmh;
