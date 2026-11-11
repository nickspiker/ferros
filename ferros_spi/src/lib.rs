//! The HSI0 USI SPI controllers (s3c64xx family) in CPU mode with byte transfers, as Android runs them (no `dma-mode` in the tree): one copy for everything that runs a port (KERNEL-OUT.md move 7). The kernel's power switches set each port up after opening its clocks (ferros_kernel spi.rs); the drivers run the transfers thru their register grant: touch (USI2, docs/oracle/TOUCH.md), GPS (USI3, GPS.md), the speaker amplifiers (USI1, AUDIO.md).
//!
//! The controller is reached thru `Regs`, offsets from the port's base: the kernel's physical addresses, or a process's hardware grant (a syscall per access). Every transfer but `stream` fits the 64-byte FIFO.
#![no_std]

/// A port's registers and the generic timer, as the caller reaches them.
pub trait Regs {
    fn rd(&self, off: usize) -> u32;
    fn wr(&self, off: usize, v: u32);
    /// The timer's count, and its ticks per second.
    fn now(&self) -> u64;
    fn freq(&self) -> u64;
}

/// Chip select: the controller's own nSS, or a pin the caller drives (called with true to select, false to let go), with the native nSS run in auto mode.
#[derive(Clone, Copy)]
pub enum Cs<'a> {
    Native,
    Pin(&'a dyn Fn(bool)),
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
/// The FIFOs' depth, each way.
pub const FIFO: usize = 64;
/// Bytes `stream` keeps in flight, inside either FIFO.
const IN_FLIGHT: usize = 32;

fn tx_level(status: u32) -> u32 {
    (status >> 6) & 0x1FF
}
fn rx_level(status: u32) -> u32 {
    (status >> 15) & 0x1FF
}

/// One port: its registers, CH_CFG's clock polarity and phase (bit 3 CPOL, bit 2 CPHA: 0 for mode 0, G#C for mode 3) and FB_CLK_SEL, the receive feedback delay (DT `samsung,spi-feedback-delay`).
pub struct Spi<R: Regs> {
    pub regs: R,
    pub mode: u32,
    pub fb: u32,
}

impl<R: Regs> Spi<R> {
    fn rd(&self, off: usize) -> u32 {
        self.regs.rd(off)
    }
    fn wr(&self, off: usize, v: u32) {
        self.regs.wr(off, v)
    }
    fn deadline(&self, us: u64) -> u64 {
        self.regs.now() + self.regs.freq() / 1_000_000 * us
    }
    fn spin_us(&self, us: u64) {
        let t = self.deadline(us);
        while self.regs.now() < t {}
    }

    /// `s3c64xx_spi_hwinit`'s setup, once the port's clocks are open and its USI is in SPI mode (the kernel's part, ferros_kernel spi.rs `Port::up`): USI reset released, the native select inactive, no interrupts, byte mode, both FIFOs empty.
    pub fn init(&self) {
        self.wr(USI_CON, 0);
        self.wr(SLAVE_SEL, 1);
        self.wr(INT_EN, 0);
        self.wr(MODE_CFG, 0);
        self.wr(PACKET_CNT, 0);
        self.wr(PENDING_CLR, 0x1E);
        self.wr(PENDING_CLR, 0);
        self.wr(SWAP_CFG, 0);
        self.wr(MODE_CFG, MODE_BYTE);
        self.wr(FB_CLK, 0);
        self.flush();
    }

    /// Empty both FIFOs: channels off, packet count off, a software-reset pulse held until the FIFOs read empty (at most a millisecond).
    pub fn flush(&self) {
        self.wr(CH_CFG, self.rd(CH_CFG) & !(CH_TX_ON | CH_RX_ON));
        self.wr(PACKET_CNT, 0);
        self.wr(CH_CFG, self.rd(CH_CFG) | CH_SW_RST);
        let t = self.deadline(1000);
        while tx_level(self.rd(STATUS)) != 0 || rx_level(self.rd(STATUS)) != 0 {
            if self.regs.now() > t {
                break;
            }
        }
        self.wr(CH_CFG, self.rd(CH_CFG) & !CH_SW_RST);
    }

    /// Packet count on for `n` bytes (the controller generates exactly that many bytes of clock), byte mode, and the channels on: both when sending, receive alone otherwise.
    fn start(&self, n: usize, send: bool) {
        self.wr(PACKET_CNT, 0);
        self.wr(PACKET_CNT, n as u32);
        self.wr(PACKET_CNT, n as u32 | PACKET_CNT_EN);
        self.wr(MODE_CFG, MODE_BYTE);
        let ch = (self.rd(CH_CFG) & !(CH_TX_ON | CH_RX_ON | 0xC)) | self.mode;
        self.wr(CH_CFG, ch | CH_RX_ON | if send { CH_TX_ON } else { 0 });
    }

    /// Shift `len` bytes: TX from `tx` when given (what comes back is discarded, as Android does), otherwise RX into `rx`.
    fn segment(&self, tx: Option<&[u8]>, rx: Option<&mut [u8]>, len: usize) -> Result<(), &'static str> {
        // WHY: the whole segment is loaded into, or drained from, a 64-byte FIFO at once; a longer one would overrun it and the controller drop the excess without a word. A caller's length, so refused rather than panicking the kernel.
        if len == 0 || len > FIFO {
            return Err("segment longer than the FIFO");
        }
        self.start(len, tx.is_some());
        if let Some(t) = tx {
            for &x in t {
                self.wr(TX_DATA, x as u32);
            }
        }
        let t = self.deadline(10_000);
        while (rx_level(self.rd(STATUS)) as usize) < len {
            if self.regs.now() > t {
                self.flush();
                return Err("SPI transfer timed out");
            }
        }
        if let Some(out) = rx {
            for o in out[..len].iter_mut() {
                *o = self.rd(RX_DATA) as u8;
            }
        }
        self.flush();
        Ok(())
    }

    /// One chip-select-wrapped transaction: write `cmd`, then read `out.len()` bytes when non-empty, each inside the FIFO. The select rises 10 µs after the last byte.
    pub fn transact(&self, cs: Cs, cmd: &[u8], out: &mut [u8]) -> Result<(), &'static str> {
        self.wr(FB_CLK, self.fb);
        match cs {
            Cs::Native => self.wr(SLAVE_SEL, 0),
            // As Linux runs the right amplifier (DT chip-select-mode 2, AUTO_CS_MODE_FORCE_QUIESCE): the controller's select logic in auto mode (SLAVE_AUTO | NSC_CNT_2), which it needs to clock at all (with the native select held inactive it never shifts: measured 2026-10-20, every transfer timed out), and the pin doing the selecting.
            Cs::Pin(pin) => {
                pin(true);
                self.wr(SLAVE_SEL, self.rd(SLAVE_SEL) | 1 << 1 | 2 << 4);
            }
        }
        let r = self.segment(Some(cmd), None, cmd.len()).and_then(|_| {
            if out.is_empty() {
                Ok(())
            } else {
                let n = out.len();
                self.segment(None, Some(out), n)
            }
        });
        self.spin_us(10);
        match cs {
            Cs::Native => self.wr(SLAVE_SEL, 1),
            Cs::Pin(pin) => {
                pin(false);
                // Quiesce: native select back to inactive, auto off.
                self.wr(SLAVE_SEL, 1);
            }
        }
        r
    }

    /// Full duplex of any length under the native select: `tx` out and as many bytes into `rx` (which must be at least as long), the FIFOs topped up and drained as they go, at most 32 bytes in flight. The GPS chip's SSI frames run to 8 KB, answering in the same clocks (GPS.md).
    pub fn stream(&self, tx: &[u8], rx: &mut [u8]) -> Result<(), &'static str> {
        let n = tx.len();
        self.wr(FB_CLK, self.fb);
        self.wr(SLAVE_SEL, 0);
        self.start(n, true);
        let (mut ti, mut ri) = (0usize, 0usize);
        let t = self.deadline(200_000);
        let mut r = Ok(());
        while ri < n {
            while ti < n && ti - ri < IN_FLIGHT {
                self.wr(TX_DATA, tx[ti] as u32);
                ti += 1;
            }
            // The receive FIFO holds only bytes clocked out by this transfer and not yet read: at most ti − ri, never past n.
            for _ in 0..rx_level(self.rd(STATUS)) {
                rx[ri] = self.rd(RX_DATA) as u8;
                ri += 1;
            }
            if self.regs.now() > t {
                r = Err("SPI stream timed out");
                break;
            }
        }
        self.flush();
        self.spin_us(10);
        self.wr(SLAVE_SEL, 1);
        r
    }
}
