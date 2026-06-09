//! UART serial output.
//!
//! Two backends:
//! - **QEMU virt**: PL011 at 0x0900_0000 (for testing)
//! - **QCM6490**: GENI Serial Engine UART (for FP5)
//!
//! We only implement TX (print). RX comes later when we need a console. ABL typically leaves the debug UART initialized on QCM6490, so we can start writing to the TX FIFO after cancelling any in-flight command.

use crate::mmio;

/// UART backend selector.
#[derive(Clone, Copy)]
pub enum UartBackend {
    /// No UART — discard all output. Safe on any hardware.
    Null,
    /// ARM PL011 UART (QEMU virt machine).
    Pl011 { base: usize },
    /// Qualcomm GENI SE UART (QCM6490).
    GeniSe { base: usize },
}

/// A UART handle — write bytes out for debug.
pub struct Uart {
    backend: UartBackend,
}

impl Uart {
    pub const fn new(backend: UartBackend) -> Self {
        Self { backend }
    }

    /// Probe the UART — returns true if the hardware seems present. For GENI SE, checks FW_REV (non-zero = firmware loaded). HW_PARAM_0 may read as 0 if QUP wrapper clocks are partially gated.
    pub fn probe(&self) -> bool {
        match self.backend {
            UartBackend::Null => true,
            UartBackend::Pl011 { base } => {
                let id = unsafe { mmio::read32(base + 0xFE0) };
                id & 0xFF == 0x11
            }
            UartBackend::GeniSe { base } => {
                let fw_rev = unsafe { mmio::read32(base + Self::SE_GENI_FW_REV) };
                fw_rev != 0
            }
        }
    }

    /// Initialize UART for TX. Cancels any in-flight GENI command.
    pub fn init(&self) {
        match self.backend {
            UartBackend::Null | UartBackend::Pl011 { .. } => {}
            UartBackend::GeniSe { base } => {
                // Cancel any active command from ABL
                let status = unsafe { mmio::read32(base + Self::SE_GENI_STATUS) };
                if status & 1 != 0 {
                    // M_GENI_CMD_ACTIVE — cancel it
                    unsafe { mmio::write32(base + Self::SE_GENI_M_CMD_CTRL, 0x2) }; // ABORT
                    // Wait for abort to complete
                    for _ in 0..10_000u32 {
                        let s = unsafe { mmio::read32(base + Self::SE_GENI_STATUS) };
                        if s & 1 == 0 {
                            break;
                        }
                        unsafe { core::arch::asm!("nop") };
                    }
                }
                // Clear all pending M IRQs
                unsafe { mmio::write32(base + Self::SE_GENI_M_IRQ_CLEAR, 0xFFFF_FFFF) };
            }
        }
    }

    /// Read FIFO depth (TX words) for diagnostics.
    pub fn fifo_depth(&self) -> u32 {
        match self.backend {
            UartBackend::GeniSe { base } => {
                let hw = unsafe { mmio::read32(base + Self::SE_HW_PARAM_0) };
                (hw >> 2) & 0x7F
            }
            _ => 0,
        }
    }

    /// Write a single byte, blocking until the TX FIFO has space.
    pub fn putc(&self, byte: u8) {
        match self.backend {
            UartBackend::Null => {}
            UartBackend::Pl011 { base } => self.pl011_putc(base, byte),
            UartBackend::GeniSe { base } => self.geni_putc(base, byte),
        }
    }

    /// Write a string.
    pub fn puts(&self, s: &str) {
        for b in s.bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
    }

    /// Write a hex u64 (for debugging addresses/values).
    pub fn put_hex(&self, val: u64) {
        self.puts("G#");
        for i in (0..16).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            let c = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
            self.putc(c);
        }
    }

    // -- PL011 (QEMU) --

    const PL011_DR: usize = 0x000;
    const PL011_FR: usize = 0x018;
    const PL011_FR_TXFF: u32 = 1 << 5;

    fn pl011_putc(&self, base: usize, byte: u8) {
        while (unsafe { mmio::read32(base + Self::PL011_FR) } & Self::PL011_FR_TXFF) != 0 {}
        unsafe { mmio::write32(base + Self::PL011_DR, byte as u32) };
    }

    // -- GENI SE UART (QCM6490) --
    //
    // Register offsets from Qualcomm downstream kernel (qcom-geni-se.h). Base address is the SE (Serial Engine) base, e.g. 0x994000.

    const SE_HW_PARAM_0: usize = 0x050;
    const SE_GENI_FW_REV: usize = 0x068;
    const SE_GENI_STATUS: usize = 0x040;
    const SE_GENI_M_CMD0: usize = 0x600;
    const SE_GENI_M_CMD_CTRL: usize = 0x604;
    const SE_GENI_M_IRQ_STATUS: usize = 0x610;
    const SE_GENI_M_IRQ_CLEAR: usize = 0x618;
    #[allow(non_upper_case_globals)]
    const SE_GENI_TX_FIFOn: usize = 0x700;
    #[allow(dead_code)] const SE_GENI_TX_FIFO_STATUS: usize = 0x800;

    // UART-specific registers (within SE address space)
    const SE_UART_TX_TRANS_LEN: usize = 0x270;

    // M_IRQ bits
    const M_CMD_DONE: u32 = 1 << 0;
    #[allow(dead_code)] const M_CMD_ABORT: u32 = 1 << 5;
    #[allow(dead_code)] const M_TX_FIFO_WATERMARK: u32 = 1 << 30;

    fn geni_putc(&self, base: usize, byte: u8) {
        // Wait for any active command to finish
        for _ in 0..100_000u32 {
            let status = unsafe { mmio::read32(base + Self::SE_GENI_STATUS) };
            if status & 1 == 0 {
                break;
            }
            unsafe { core::arch::asm!("nop") };
        }

        // Clear pending IRQs
        unsafe { mmio::write32(base + Self::SE_GENI_M_IRQ_CLEAR, 0xFFFF_FFFF) };

        // Set transfer length = 1 byte
        unsafe { mmio::write32(base + Self::SE_UART_TX_TRANS_LEN, 1) };

        // Issue UART TX START command (opcode 1 << 27)
        unsafe { mmio::write32(base + Self::SE_GENI_M_CMD0, 1 << 27) };

        // Write the byte to FIFO (must be after command is issued)
        unsafe { mmio::write32(base + Self::SE_GENI_TX_FIFOn, byte as u32) };

        // Wait for M_CMD_DONE
        for _ in 0..100_000u32 {
            let irq = unsafe { mmio::read32(base + Self::SE_GENI_M_IRQ_STATUS) };
            if irq & Self::M_CMD_DONE != 0 {
                unsafe { mmio::write32(base + Self::SE_GENI_M_IRQ_CLEAR, Self::M_CMD_DONE) };
                return;
            }
            unsafe { core::arch::asm!("nop") };
        }

        // Timeout — abort and move on (don't hang the kernel)
        unsafe { mmio::write32(base + Self::SE_GENI_M_CMD_CTRL, 0x2) };
        for _ in 0..10_000u32 {
            let s = unsafe { mmio::read32(base + Self::SE_GENI_STATUS) };
            if s & 1 == 0 {
                break;
            }
            unsafe { core::arch::asm!("nop") };
        }
        unsafe { mmio::write32(base + Self::SE_GENI_M_IRQ_CLEAR, 0xFFFF_FFFF) };
    }
}

impl core::fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.puts(s);
        Ok(())
    }
}
