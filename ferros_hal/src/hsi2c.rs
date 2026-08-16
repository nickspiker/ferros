//! Exynos HSI2C (USI-I2C) master engine — polling / auto-mode.
//!
//! Faithful port of the Linux `i2c-exynos5.c` polling path, PROVEN ON HARDWARE 2026-08-16 (payloads/repeaterprobe read the eUSB2 repeater's REV_ID and full tune set over hsi2c_11).
//! Auto-mode: the controller drives START/ADDR/STOP itself once ADDR + AUTO_CONF.len + MASTER_RUN are set — no manual bit-banging.
//!
//! Design decisions, both from the "do less than the C driver" principle:
//! - `init()` does NOT SW_RST — it ensures only MASTER + AUTO_MODE and reuses ABL's calibrated TIMING_* (the same trick that fixed the eUSB2 PHY).
//! - Polling with our own spin bound; the hardware TIMEOUT_EN is disabled.
//!
//! Pixel 8 (husky) constants from the live DT: the eUSB2 repeater is at 7-bit `G#3E` on hsi2c_11 (`G#10CB_0000`).
//! That bus is SHARED with the battery/USB-C management chain (max77729/max77759/pca9468) — never write blind on it.

// ---- register offsets (i2c-exynos5.c) ----
const CTL: usize = 0x00;
const FIFO_CTL: usize = 0x04;
const TRAILING_CTL: usize = 0x08;
const INT_ENABLE: usize = 0x20;
const INT_STATUS: usize = 0x24;
const FIFO_STATUS: usize = 0x30;
const TX_DATA: usize = 0x34;
const RX_DATA: usize = 0x38;
const CONF: usize = 0x40;
const AUTO_CONF: usize = 0x44;
const TIMEOUT: usize = 0x48;
const TRANS_STATUS: usize = 0x50;
const ADDR: usize = 0x70;

// ---- bit constants ----
const CTL_MASTER: u32 = 1 << 3;
const CTL_RXCHON: u32 = 1 << 6;
const CTL_TXCHON: u32 = 1 << 7;
const FIFO_RXFIFO_EN: u32 = 1 << 0;
const FIFO_TXFIFO_EN: u32 = 1 << 1;
const INT_TRANSFER_DONE: u32 = 1 << 7;
const FIFO_RX_FIFO_EMPTY: u32 = 1 << 24;
const FIFO_TX_FIFO_EMPTY: u32 = 1 << 8;
const FIFO_TX_FIFO_LVL_MASK: u32 = 0x7F;
const CONF_AUTO_MODE: u32 = 1 << 31;
const AUTO_READ_WRITE: u32 = 1 << 16;
const AUTO_STOP_AFTER_TRANS: u32 = 1 << 17;
const AUTO_MASTER_RUN: u32 = 1 << 31;
const TIMEOUT_EN: u32 = 1 << 31;
const TRAILING_COUNT: u32 = 0x00FF_FFFF;
const FIFO_TRIG_CRITERIA: u32 = 8;
const FIFO_SIZE: u32 = 16;
const SPIN_MAX: u32 = 4_000_000; // generous per-phase bound (polling, no jiffies)

/// Pixel 8 (husky): HSI2C bus 11 base — the bus the eUSB2 repeater lives on (live DT, zuma-usi.dtsi).
pub const PIXEL8_HSI2C11_BASE: usize = 0x10CB_0000;
/// Pixel 8 (husky): eUSB2 repeater 7-bit I2C address (live DT `eusb-repeater@3E`).
pub const PIXEL8_EUSB_REPEATER_ADDR: u8 = 0x3E;
/// Pixel 8 (husky): the DT `repeater_tune*` table as (register, value) — Google's calibrated analog tune.
/// The Android kernel driver applies this at probe; ABL does NOT, so on ferros boots the repeater runs untuned unless we write it (measured 2026-08-16: 7 of 8 registers differ from ABL/POR state).
/// Hardware-validated by payloads/repeatertune: all 8 write+readback OK with the USB link live.
pub const PIXEL8_REPEATER_TUNE: [(u8, u8); 8] = [
    (0x50, 0x0A), // eusb_mode_control
    (0x70, 0x3C), // u_tx_adjust_port1
    (0x71, 0x2C), // u_hs_tx_pre_emphasis_p1
    (0x72, 0x90), // u_rx_adjust_port1
    (0x73, 0x83), // u_disconnect_squelch_port1
    (0x77, 0x00), // e_hs_tx_pre_emphasis_p1
    (0x78, 0x0B), // e_tx_adjust_port1
    (0x79, 0x40), // e_rx_adjust_port1
];

pub struct Hsi2c {
    base: usize,
}

impl Hsi2c {
    pub fn new(base: usize) -> Self {
        Hsi2c { base }
    }

    pub fn rd(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + off) as *const u32) }
    }
    fn wr(&self, off: usize, v: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + off) as *mut u32, v);
            core::arch::asm!("dsb sy");
        }
    }

    /// Minimal init: ensure MASTER + AUTO_MODE, preserving ABL's clock/timing calibration (do NOT SW_RST — that would force recomputing timing we deliberately reuse).
    pub fn init(&self) {
        self.wr(CTL, CTL_MASTER);
        self.wr(TRAILING_CTL, TRAILING_COUNT);
        let conf = self.rd(CONF);
        self.wr(CONF, conf | CONF_AUTO_MODE);
    }

    /// One auto-mode message. `stop` = assert STOP after (false = repeated-start for the next msg).
    /// On failure returns TRANS_STATUS for the fingerprint.
    /// Faithful port of the `exynos5_i2c_xfer_msg` polling branches.
    pub fn xfer(&self, addr: u8, buf: &mut [u8], is_read: bool, stop: bool) -> Result<(), u32> {
        let len = buf.len() as u32;
        let mut ctl = self.rd(CTL);

        // Disable the hardware timeout (we poll our own bound).
        let to = self.rd(TIMEOUT) & !TIMEOUT_EN;
        self.wr(TIMEOUT, to);

        // FIFO trigger level, clamped to the criteria.
        let trig = if len >= FIFO_TRIG_CRITERIA { FIFO_TRIG_CRITERIA } else { len };
        self.wr(FIFO_CTL, FIFO_RXFIFO_EN | FIFO_TXFIFO_EN | (trig << 4) | (trig << 16));

        let mut auto_conf = self.rd(AUTO_CONF);
        if is_read {
            ctl &= !CTL_TXCHON;
            ctl |= CTL_RXCHON;
            auto_conf |= AUTO_READ_WRITE;
        } else {
            ctl &= !CTL_RXCHON;
            ctl |= CTL_TXCHON;
            auto_conf &= !AUTO_READ_WRITE;
        }

        // Clear pending interrupt state (write-1-to-clear).
        self.wr(INT_STATUS, self.rd(INT_STATUS));

        if stop {
            auto_conf |= AUTO_STOP_AFTER_TRANS;
        } else {
            auto_conf &= !AUTO_STOP_AFTER_TRANS;
        }

        // Slave address (7-bit at [16:10]); non-HS master-id field = 0x7<<24, per the ref.
        let mut a = self.rd(ADDR);
        a &= !(0x3FF << 10);
        a &= !0x3FF;
        a &= !(0xFF << 24);
        a |= 0x7 << 24;
        a |= (addr as u32 & 0x7F) << 10;
        self.wr(ADDR, a);

        self.wr(CTL, ctl);
        self.wr(INT_ENABLE, INT_TRANSFER_DONE); // status-only in polling mode

        // Program length, then set MASTER_RUN to launch.
        auto_conf &= !0xFFFF;
        auto_conf |= len;
        self.wr(AUTO_CONF, auto_conf);
        self.wr(AUTO_CONF, self.rd(AUTO_CONF) | AUTO_MASTER_RUN);

        let mut ptr = 0usize;
        let mut spins = 0u32;
        if is_read {
            while ptr < buf.len() && spins < SPIN_MAX {
                if self.rd(FIFO_STATUS) & FIFO_RX_FIFO_EMPTY == 0 {
                    buf[ptr] = self.rd(RX_DATA) as u8;
                    ptr += 1;
                }
                spins += 1;
            }
            if ptr >= buf.len() { Ok(()) } else { Err(self.rd(TRANS_STATUS)) }
        } else {
            while ptr < buf.len() && spins < SPIN_MAX {
                if self.rd(FIFO_STATUS) & FIFO_TX_FIFO_LVL_MASK < FIFO_SIZE {
                    self.wr(TX_DATA, buf[ptr] as u32);
                    ptr += 1;
                }
                spins += 1;
            }
            spins = 0;
            while spins < SPIN_MAX {
                let is_ = self.rd(INT_STATUS);
                if is_ & INT_TRANSFER_DONE != 0 && self.rd(FIFO_STATUS) & FIFO_TX_FIFO_EMPTY != 0 {
                    self.wr(INT_STATUS, is_);
                    return Ok(());
                }
                spins += 1;
            }
            Err(self.rd(TRANS_STATUS))
        }
    }

    /// Read one 8-bit register: write the reg pointer (repeated-start), then read the byte.
    pub fn read_reg(&self, addr: u8, reg: u8) -> Result<u8, u32> {
        self.xfer(addr, &mut [reg], false, false)?;
        let mut b = [0u8; 1];
        self.xfer(addr, &mut b, true, true)?;
        Ok(b[0])
    }

    /// Write one 8-bit register: reg pointer + data in a single message.
    pub fn write_reg(&self, addr: u8, reg: u8, val: u8) -> Result<(), u32> {
        self.xfer(addr, &mut [reg, val], false, true)
    }
}
