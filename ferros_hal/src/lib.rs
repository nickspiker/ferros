// FERROS HAL SOURCE MAP — keep updated when pub items or files change
//
// lib.rs ── no_std HAL, module re-exports, alloc extern
//
// mmio.rs ── volatile MMIO register access read32(addr), write32(addr, val) read16(addr), write16(addr, val) read8(addr), write8(addr, val)
//
// console.rs ── framebuffer text console (8x16 VGA font, 2x scaled) struct Console { fb, stride, x_off, y_off, col, row } ::new(fb, stride, width, height), putc(ch), puts(s), clear() FONT: 95-char ASCII bitmap font (0x20-0x7E)
//
// fb.rs ── raw framebuffer pixel access (ABL splash at G#E1000000) fb_write_pixel(fb, stride, x, y, r, g, b) fb_fill_rect(fb, stride, x, y, w, h, r, g, b)
//
// dtb.rs ── minimal DTB/FDT parser struct Dtb — parse from raw pointer ::bootargs(), memory_regions(), reserved_memory() ::find_node(name), find_prop(node, name) ::find_ramoops() → RamoopsConfig
//
// uart.rs ── GENI UART TX struct Uart { base } ::new(base), probe() → bool, puts(s), putc(u8) enum UartBackend { Geni, None }
//
// pstore.rs ── ramoops persistent log (warm reboot survives) struct Ramoops { base, size } ::write(data), ::init_from_dtb(dtb) → Option<Self> struct RamoopsConfig { base, size, console_size, pmsg_size }
//
// usb.rs ── DWC3 USB device mode struct Dwc3Dev — full device-mode controller state ::init() → Option<Self> ::poll_event() → UsbEvent ::bulk_in_send(data) → bool ::bulk_out_arm(), bulk_out_read(buf) → usize enum UsbEvent { None, Reset, SetupData, TransferComplete, ... } phy_init(), smmu_bypass(), probe() → Dwc3Info Single-TRB model, 1ms pacing for multi-packet inbound
//
// ufs.rs ── UFS host controller (UFSHCI v3.0 at G#13200000, Pixel 8) struct UfsController { base } ::new(base), link_is_up() → bool ::full_init() → InitReport (HCE reset → cal → LINKSTARTUP → fDeviceInit → HS-G4 PMC) ::probe() → UfsProbe ::read_block(lba)/write_block(lba) → OCS ::uic_cmd/dme_set/dme_get 4KB blocks, 232GB LUN0
//
// ufs_cal.rs ── zuma M-PHY/UNIPRO link calibration (port of Samsung ufs-cal-if) pre_link(), post_link(), pre_pmc_hs_b() region bases (STD/HCI/UNIPRO/PMA), udelay() CRITICAL: clear HCI_FORCE_HCS auto clock-stop enables before ANY PMA access — the auto-gate bus-hangs the AP
//
// ring.rs ── vault root ring on UFS (spec in RING.md) (implementation pending)
//
// hsi2c.rs ── Exynos HSI2C (USI-I2C) master, polling/auto-mode (port of i2c-exynos5.c, hardware-proven on Pixel 8 hsi2c_11) struct Hsi2c { base } ::new(base), init() — MASTER+AUTO_MODE, reuses ABL timing (no SW_RST) ::xfer(addr, buf, is_read, stop) → Result<(), trans_status> ::read_reg(addr, reg) → Result<u8, u32>, write_reg(addr, reg, val) PIXEL8_HSI2C11_BASE (G#10CB0000), PIXEL8_EUSB_REPEATER_ADDR (G#3E)

//! Ferros Hardware Abstraction Layer
//!
//! `#![no_std]` — runs bare metal or with alloc provided by the kernel.
//!
//! The `alloc` feature (default) enables modules that require heap allocation (usb, console, hamt). Disable for the seed which has no allocator.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod usb_trait;
pub use usb_trait::{UsbBulk, UsbEvent};

pub mod mmio;
pub mod uart;
pub mod fb;
pub mod dtb;
pub mod ufs;
pub mod ufs_cal;
pub mod qtimer;            // aarch64 QTIMER read (moved here from old vsf_mini.rs)
pub mod ring;
pub mod gic;
pub mod hyp;
pub mod pstore;
pub mod hsi2c;

// Modules requiring alloc
#[cfg(feature = "alloc")]
pub mod console;
#[cfg(feature = "alloc")]
pub mod usb;
#[cfg(feature = "alloc")]
pub mod hamt;
