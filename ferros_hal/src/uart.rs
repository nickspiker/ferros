//! UART serial output.
//!
//! Two backends:
//! - **QEMU virt**: PL011 at 0x0900_0000 (for testing)
//! - **QCM6490**: GENI Serial Engine UART (for FP5)
//!
//! We only implement TX (print). RX comes later when we need a console.
//! ABL typically leaves UART3 initialized on QCM6490, so we can just
//! start writing to the TX FIFO.

use crate::mmio;

/// UART backend selector.
#[derive(Clone, Copy)]
pub enum UartBackend {
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

    /// Write a single byte, blocking until the TX FIFO has space.
    pub fn putc(&self, byte: u8) {
        match self.backend {
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
        self.puts("0x");
        for i in (0..16).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            self.putc(c);
        }
    }

    // -- PL011 (QEMU) --

    // PL011 register offsets
    const PL011_DR: usize = 0x000;   // Data Register
    const PL011_FR: usize = 0x018;   // Flag Register
    const PL011_FR_TXFF: u32 = 1 << 5; // TX FIFO Full

    fn pl011_putc(&self, base: usize, byte: u8) {
        // Spin until TX FIFO not full
        while (unsafe { mmio::read32(base + Self::PL011_FR) } & Self::PL011_FR_TXFF) != 0 {}
        unsafe { mmio::write32(base + Self::PL011_DR, byte as u32) };
    }

    // -- GENI SE UART (QCM6490) --

    // GENI SE register offsets (from Qualcomm downstream kernel)
    const GENI_M_CMD0: usize = 0x000;
    #[allow(non_upper_case_globals)]
    const GENI_TX_FIFOn: usize = 0x700; // matches Qualcomm register naming
    const GENI_TX_FIFO_STATUS: usize = 0x800;
    const GENI_M_IRQ_STATUS: usize = 0x010;
    const GENI_M_IRQ_CLEAR: usize = 0x018;

    fn geni_putc(&self, base: usize, byte: u8) {
        // Wait for TX FIFO space.
        // GENI TX FIFO status: bits[3:0] = number of words in FIFO.
        // FIFO depth is typically 16 words.
        while (unsafe { mmio::read32(base + Self::GENI_TX_FIFO_STATUS) } & 0xF) >= 16 {}

        // Write byte (GENI packs up to 4 bytes per FIFO word, we send 1 at a time)
        unsafe { mmio::write32(base + Self::GENI_TX_FIFOn, byte as u32) };

        // Issue UART TX command: opcode=1 (START), param=1 byte
        unsafe { mmio::write32(base + Self::GENI_M_CMD0, (1 << 27) | 1) };

        // Wait for completion
        loop {
            let status = unsafe { mmio::read32(base + Self::GENI_M_IRQ_STATUS) };
            if status & (1 << 0) != 0 { // M_CMD_DONE
                unsafe { mmio::write32(base + Self::GENI_M_IRQ_CLEAR, 1 << 0) };
                break;
            }
        }
    }
}

impl core::fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.puts(s);
        Ok(())
    }
}
