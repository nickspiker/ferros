//! The HSI0 USI SPI controllers (s3c64xx family) in CPU mode with byte transfers, as Android runs them (no `dma-mode` in the tree): what touch (USI2, docs/oracle/TOUCH.md) and the speaker amplifiers (USI1, AUDIO.md) both use.
//!
//! Each controller is clocked from the 24.576 MHz oscillator (÷1; the controller divides by 4 inside: about 6.1 MHz on the wire), its USI switched to SPI in sysreg_hsi0, its USI reset released, and set up as `s3c64xx_spi_hwinit` does. Every transfer fits the 64-byte FIFO.

use core::ptr::{read_volatile, write_volatile};

/// One controller and the clock path in front of it (cmucal-sfr.c; usi-offset from zuma-usi.dtsi).
pub struct Port {
    pub base: usize,
    pub usi_mode: usize,
    pub mux: usize,
    pub div: usize,
    pub gates: [usize; 4],
    pub qch: usize,
    /// CH_CFG's clock polarity and phase (bit 3 CPOL, bit 2 CPHA): 0 for mode 0, G#C for mode 3.
    pub mode: u32,
    /// FB_CLK_SEL, the receive feedback delay (DT `samsung,spi-feedback-delay`).
    pub fb: u32,
}

const CMU_HSI0: usize = 0x1100_0000;
const SYSREG_HSI0: usize = 0x1102_0000;
/// spi_20, HSI0 USI2: the touch controller.
pub const TOUCH: Port = Port { base: 0x111D_0000, usi_mode: SYSREG_HSI0 + 0x101C, mux: CMU_HSI0 + 0x101C, div: CMU_HSI0 + 0x181C, gates: [CMU_HSI0 + 0x2120, CMU_HSI0 + 0x20FC, CMU_HSI0 + 0x2100, CMU_HSI0 + 0x20BC], qch: CMU_HSI0 + 0x30CC, mode: 0, fb: 0 };
/// spi_19, HSI0 USI1: the two CS35L41 speaker amplifiers (CS0 native, CS1 = gpn1-0).
pub const AMPS: Port = Port { base: 0x111C_0000, usi_mode: SYSREG_HSI0 + 0x1018, mux: CMU_HSI0 + 0x1018, div: CMU_HSI0 + 0x1818, gates: [CMU_HSI0 + 0x211C, CMU_HSI0 + 0x20F4, CMU_HSI0 + 0x20F8, CMU_HSI0 + 0x20B0], qch: CMU_HSI0 + 0x30C8, mode: 0, fb: 0 };
/// spi_21, HSI0 USI3: the GNSS chip (BCM4776; zuma-board-gps.dtsi: mode 3, feedback delay 1, 26 MHz at most; cmucal-sfr.c for the clock path).
pub const GNSS: Port = Port { base: 0x111E_0000, usi_mode: SYSREG_HSI0 + 0x1020, mux: CMU_HSI0 + 0x1020, div: CMU_HSI0 + 0x1820, gates: [CMU_HSI0 + 0x2124, CMU_HSI0 + 0x2104, CMU_HSI0 + 0x2108, CMU_HSI0 + 0x20D0], qch: CMU_HSI0 + 0x30D0, mode: 0xC, fb: 1 };

/// Chip select: the controller's own nSS, or a GPIO (bank base, pin), active low, with the native nSS held inactive.
#[derive(Clone, Copy)]
pub enum Cs {
    Native,
    Gpio(usize, u32),
}

const CH_CFG: usize = 0x00;
const MODE_CFG: usize = 0x08;
const SLAVE_SEL: usize = 0x0C;
const INT_EN: usize = 0x10;
const STATUS: usize = 0x14;
const TX_DATA: usize = 0x18;
const RX_DATA: usize = 0x1C;
const PACKET_CNT: usize = 0x20;
const PENDING_CLR: usize = 0x24;
const SWAP_CFG: usize = 0x28;
const FB_CLK: usize = 0x2C;
const USI_CON: usize = 0xC4;
const CH_TX_ON: u32 = 1 << 0;
const CH_RX_ON: u32 = 1 << 1;
const CH_SW_RST: u32 = 1 << 5;
const PACKET_CNT_EN: u32 = 1 << 16;
/// MODE_CFG with byte transfers and the maximum trailing count, the value Android keeps there.
const MODE_BYTE: u32 = 0x1FF8_0000;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn now() -> u64 {
    crate::platform::cntpct()
}
fn ticks_us(us: u64) -> u64 {
    crate::platform::cntfrq() / 1_000_000 * us
}
fn spin_us(us: u64) {
    let t = now() + ticks_us(us);
    while now() < t {}
}

/// A CMU gate is closed only when manual control is on (bit 20) and its value (bit 21) is off.
pub fn gate_open(v: u32) -> bool {
    !(v & (1 << 20) != 0 && v & (1 << 21) == 0)
}

impl Port {
    /// Every gate on the controller's clock path open (its registers are safe to read).
    pub fn clocked(&self) -> bool {
        self.gates.iter().all(|&a| gate_open(rd(a)))
    }

    /// One line of the clock path and, when clocked, the controller's state, read only.
    pub fn describe(&self) -> alloc::string::String {
        let mut s = alloc::format!("mode G#{:X} mux G#{:X} div G#{:X} qch G#{:X} gates", rd(self.usi_mode), rd(self.mux), rd(self.div), rd(self.qch));
        for a in self.gates {
            s.push_str(&alloc::format!(" G#{:X}", rd(a)));
        }
        if self.clocked() {
            s.push_str(&alloc::format!("; USI_CON G#{:X} CH_CFG G#{:X} MODE G#{:X} CS G#{:X} STATUS G#{:X}", rd(self.base + USI_CON), rd(self.base + CH_CFG), rd(self.base + MODE_CFG), rd(self.base + SLAVE_SEL), rd(self.base + STATUS)));
        } else {
            s.push_str("; registers not read, a gate is closed");
        }
        s
    }

    /// Clock from the oscillator, SPI mode, USI reset released, `s3c64xx_spi_hwinit`'s setup.
    pub fn up(&self) {
        wr(self.mux, (rd(self.mux) & !1) | 1);
        wr(self.div, rd(self.div) & !0xF);
        for a in self.gates {
            wr(a, rd(a) | 1 << 20 | 1 << 21);
        }
        wr(self.usi_mode, (rd(self.usi_mode) & !7) | 2);
        let b = self.base;
        wr(b + USI_CON, 0);
        wr(b + SLAVE_SEL, 1);
        wr(b + INT_EN, 0);
        wr(b + MODE_CFG, 0);
        wr(b + PACKET_CNT, 0);
        wr(b + PENDING_CLR, 0x1E);
        wr(b + PENDING_CLR, 0);
        wr(b + SWAP_CFG, 0);
        wr(b + MODE_CFG, MODE_BYTE);
        wr(b + FB_CLK, 0);
        self.flush();
    }

    /// Empty both FIFOs: channels off, packet count off, a software-reset pulse held until the FIFOs read empty.
    fn flush(&self) {
        let b = self.base;
        wr(b + CH_CFG, rd(b + CH_CFG) & !(CH_TX_ON | CH_RX_ON));
        wr(b + PACKET_CNT, 0);
        wr(b + CH_CFG, rd(b + CH_CFG) | CH_SW_RST);
        let t = now() + ticks_us(1000);
        while (rd(b + STATUS) >> 6) & 0x1FF != 0 || (rd(b + STATUS) >> 15) & 0x1FF != 0 {
            if now() > t {
                break;
            }
        }
        wr(b + CH_CFG, rd(b + CH_CFG) & !CH_SW_RST);
    }

    /// Shift `len` bytes: TX from `tx` when given (what comes back is discarded, as Android does), otherwise RX into `rx`. Packet count makes the controller generate exactly `len` bytes of clock.
    fn segment(&self, tx: Option<&[u8]>, rx: Option<&mut [u8]>, len: usize) -> Result<(), &'static str> {
        if len == 0 || len > 64 {
            return Err("segment longer than the FIFO");
        }
        let b = self.base;
        wr(b + PACKET_CNT, 0);
        wr(b + PACKET_CNT, len as u32);
        wr(b + PACKET_CNT, len as u32 | PACKET_CNT_EN);
        wr(b + MODE_CFG, MODE_BYTE);
        let ch = (rd(b + CH_CFG) & !(CH_TX_ON | CH_RX_ON | 0xC)) | self.mode;
        if let Some(t) = tx {
            wr(b + CH_CFG, ch | CH_RX_ON | CH_TX_ON);
            for &x in t {
                wr(b + TX_DATA, x as u32);
            }
        } else {
            wr(b + CH_CFG, ch | CH_RX_ON);
        }
        let t = now() + ticks_us(10_000);
        while ((rd(b + STATUS) >> 15) & 0x1FF) < len as u32 {
            if now() > t {
                self.flush();
                return Err("SPI transfer timed out");
            }
        }
        if let Some(out) = rx {
            for o in out.iter_mut().take(len) {
                *o = rd(b + RX_DATA) as u8;
            }
        }
        self.flush();
        Ok(())
    }

    /// Full duplex, one chip-select-wrapped transfer of up to 64 bytes: `tx` out while `rx` (same length) comes in — for protocols that answer in the same clocks (the GNSS chip's SSI framing).
    pub fn duplex(&self, cs: Cs, tx: &[u8], rx: &mut [u8]) -> Result<(), &'static str> {
        let b = self.base;
        let n = tx.len().min(rx.len());
        wr(b + FB_CLK, self.fb);
        if let Cs::Native = cs {
            wr(b + SLAVE_SEL, 0);
        }
        wr(b + PACKET_CNT, 0);
        wr(b + PACKET_CNT, n as u32);
        wr(b + PACKET_CNT, n as u32 | PACKET_CNT_EN);
        wr(b + MODE_CFG, MODE_BYTE);
        let ch = (rd(b + CH_CFG) & !(CH_TX_ON | CH_RX_ON | 0xC)) | self.mode;
        wr(b + CH_CFG, ch | CH_RX_ON | CH_TX_ON);
        for &x in &tx[..n] {
            wr(b + TX_DATA, x as u32);
        }
        let t = now() + ticks_us(10_000);
        let mut r = Ok(());
        while ((rd(b + STATUS) >> 15) & 0x1FF) < n as u32 {
            if now() > t {
                r = Err("SPI transfer timed out");
                break;
            }
        }
        if r.is_ok() {
            for o in rx.iter_mut().take(n) {
                *o = rd(b + RX_DATA) as u8;
            }
        }
        self.flush();
        spin_us(10);
        if let Cs::Native = cs {
            wr(b + SLAVE_SEL, 1);
        }
        r
    }

    /// One chip-select-wrapped transaction: write `cmd`, then read `out.len()` bytes when non-empty. CS rises 10 µs after the last byte.
    pub fn transact(&self, cs: Cs, cmd: &[u8], out: &mut [u8]) -> Result<(), &'static str> {
        let b = self.base;
        wr(b + FB_CLK, self.fb);
        match cs {
            Cs::Native => wr(b + SLAVE_SEL, 0),
            // As Linux runs the right amplifier (DT chip-select-mode 2, AUTO_CS_MODE_FORCE_QUIESCE): the controller's select logic in auto mode (SLAVE_AUTO | NSC_CNT_2), which it needs to clock at all — with the native select held inactive it never shifts (measured 2026-10-20: every transfer timed out) — and the GPIO doing the selecting.
            Cs::Gpio(bank, pin) => {
                wr(bank + 4, rd(bank + 4) & !(1 << pin));
                wr(b + SLAVE_SEL, rd(b + SLAVE_SEL) | 1 << 1 | 2 << 4);
            }
        }
        let r = self.segment(Some(cmd), None, cmd.len()).and_then(|_| if out.is_empty() { Ok(()) } else { let n = out.len(); self.segment(None, Some(out), n) });
        spin_us(10);
        match cs {
            Cs::Native => wr(b + SLAVE_SEL, 1),
            Cs::Gpio(bank, pin) => {
                wr(bank + 4, rd(bank + 4) | 1 << pin);
                // Quiesce: native select back to inactive, auto off.
                wr(b + SLAVE_SEL, 1);
            }
        }
        r
    }
}
