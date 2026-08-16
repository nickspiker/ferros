//! eUSB2 repeater probe — bring up HSI2C and read the repeater's REV_ID (G#B0).
//!
//! First bring-up milestone for [REPEATER.md]. A successful REV_ID read proves the HSI2C
//! engine works and the repeater bus is ours; everything after is tuning writes.
//!
//! The HSI2C engine below is a faithful port of the Linux `i2c-exynos5.c` POLLING path
//! (auto-mode: the controller drives START/ADDR/STOP itself once ADDR + AUTO_CONF.len +
//! MASTER_RUN are set). Register map and bit constants are copied verbatim from that driver.
//! It is written self-contained so it lifts straight into `ferros_hal::hsi2c` for the kernel's
//! enumeration path once proven here.
//!
//! Runs over the RUN channel against the already-enumerated device — no kernel reflash, no
//! re-rolling enumeration per iteration.
//!
//! !!! TWO CONSTANTS BELOW ARE PLACEHOLDERS !!! Fill from the live DT (Android + Magisk root),
//! see REPEATER.md "The two unknowns": HSI2C_BASE (one of the zuma-usi.dtsi bases) and
//! REPEATER_ADDR (the eusb-repeater node's 7-bit 'reg').

#![no_std]
#![no_main]

// ---- FILL THESE FROM DT (REPEATER.md) ----
const HSI2C_BASE: usize = 0xDEAD_0000; // TODO: parent i2c bus 'reg' base (a zuma-usi.dtsi hsi2c@ addr)
const REPEATER_ADDR: u8 = 0x00; // TODO: eusb-repeater 'reg' (7-bit)
// ------------------------------------------

const REV_ID: u8 = 0xB0; // repeater chip-revision register — the bring-up target

// ---- HSI2C register offsets (i2c-exynos5.c) ----
const CTL: usize = 0x00;
const FIFO_CTL: usize = 0x04;
const TRAILING_CTL: usize = 0x08;
const INT_ENABLE: usize = 0x20;
const INT_STATUS: usize = 0x24;
const ERR_STATUS: usize = 0x2C;
const FIFO_STATUS: usize = 0x30;
const TX_DATA: usize = 0x34;
const RX_DATA: usize = 0x38;
const CONF: usize = 0x40;
const AUTO_CONF: usize = 0x44;
const TIMEOUT: usize = 0x48;
const TRANS_STATUS: usize = 0x50;
const TIMING_FS1: usize = 0x60;
const TIMING_FS3: usize = 0x68;
const TIMING_SLA: usize = 0x6C;
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
const SPIN_MAX: u32 = 4_000_000; // generous per-phase timeout (polling, no jiffies)

struct Hsi2c {
    base: usize,
}

impl Hsi2c {
    fn rd(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + off) as *const u32) }
    }
    fn wr(&self, off: usize, v: u32) {
        unsafe {
            core::ptr::write_volatile((self.base + off) as *mut u32, v);
            core::arch::asm!("dsb sy");
        }
    }

    /// Minimal init: ensure MASTER + AUTO_MODE, preserving ABL's clock/timing calibration
    /// (do NOT SW_RST — that would force recomputing timing we deliberately reuse).
    fn init(&self) {
        self.wr(CTL, CTL_MASTER);
        self.wr(TRAILING_CTL, TRAILING_COUNT);
        let conf = self.rd(CONF);
        self.wr(CONF, conf | CONF_AUTO_MODE);
    }

    /// One auto-mode message. `stop` = assert STOP after (false = repeated-start for the next msg).
    /// Faithful port of the `exynos5_i2c_xfer_msg` polling branches.
    fn xfer(&self, addr: u8, buf: &mut [u8], is_read: bool, stop: bool) -> Result<(), u32> {
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
    fn read_reg(&self, addr: u8, reg: u8) -> Result<u8, u32> {
        self.xfer(addr, &mut [reg], false, false)?;
        let mut b = [0u8; 1];
        self.xfer(addr, &mut b, true, true)?;
        Ok(b[0])
    }
}

struct Out {
    ptr: *mut u8,
    cap: usize,
    n: usize,
}

impl Out {
    fn put(&mut self, b: u8) {
        if self.n < self.cap {
            unsafe { *self.ptr.add(self.n) = b; }
            self.n += 1;
        }
    }
    fn s(&mut self, s: &[u8]) {
        for &b in s {
            self.put(b);
        }
    }
    fn hex(&mut self, val: u32) {
        let h = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(h[((val >> (i * 4)) & 0xF) as usize]);
        }
    }
    fn line(&mut self, label: &[u8], val: u32) {
        self.s(label);
        self.put(b'=');
        self.hex(val);
        self.put(b'\n');
    }
}

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };

    if HSI2C_BASE == 0xDEAD_0000 || REPEATER_ADDR == 0 {
        o.s(b"HSI2C_BASE / REPEATER_ADDR are placeholders - fill from DT (REPEATER.md)\n");
        return o.n as u64;
    }

    let i2c = Hsi2c { base: HSI2C_BASE };

    // Recon first: dump what ABL left in the controller (confirms the right base + calibrated timing).
    o.line(b"CTL", i2c.rd(CTL));
    o.line(b"CONF", i2c.rd(CONF));
    o.line(b"TIMING_FS1", i2c.rd(TIMING_FS1));
    o.line(b"TIMING_FS3", i2c.rd(TIMING_FS3));
    o.line(b"TIMING_SLA", i2c.rd(TIMING_SLA));
    o.line(b"TRANS_STATUS", i2c.rd(TRANS_STATUS));

    // Ensure MASTER + AUTO_MODE (preserves ABL timing), then read REV_ID.
    i2c.init();
    match i2c.read_reg(REPEATER_ADDR, REV_ID) {
        Ok(rev) => {
            o.line(b"REV_ID", rev as u32);
            o.s(b"REPEATER BUS UP\n");
        }
        Err(trans) => {
            o.line(b"REV_ID_FAIL_TRANS_STATUS", trans);
            o.line(b"ERR_STATUS", i2c.rd(ERR_STATUS));
            o.s(b"no ACK / timeout - wrong base or addr?\n");
        }
    }

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
