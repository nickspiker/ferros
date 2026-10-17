//! A polled master for the Exynos USI-v2 HS-I2C controllers (soc/gs i2c-exynos5.c, auto mode), enough for register reads and writes on the PMIC-side buses. PMIC.md has the register map and the measured bring-up state.
//!
//! hsi2c_11 (G#10CB_0000) fronts the MAX77759 battery gauge at target G#36; the bootloader leaves it configured (USI mode 4, mux/gates on, 400 kHz timing for its 400 MHz clock, AUTO_MODE set), so nothing here touches clocks or pins: `up()` checks those signs and refuses if any is missing.

use core::ptr::{read_volatile, write_volatile};

pub const HSI2C_11: usize = 0x10CB_0000;
const SYSREG_PERIC1_USI11: usize = 0x10C2_100C;

const CTL: usize = 0x00;
const FIFO_CTL: usize = 0x04;
const INT_ENABLE: usize = 0x20;
const INT_STATUS: usize = 0x24;
const FIFO_STATUS: usize = 0x30;
const TX_DATA: usize = 0x34;
const RX_DATA: usize = 0x38;
const CONF: usize = 0x40;
const AUTO_CONF: usize = 0x44;
const TRANS_STATUS: usize = 0x50;
const TIMING_FS3: usize = 0x68;
const ADDR: usize = 0x70;
const USI_CON: usize = 0xC4;

const CTL_MASTER: u32 = 1 << 3;
const CTL_RXCHON: u32 = 1 << 6;
const CTL_TXCHON: u32 = 1 << 7;
const INT_TRANSFER_DONE: u32 = 1 << 7;
const INT_ERRORS: u32 = 0xF << 8;
const INT_NODEV: u32 = 1 << 10;
const FIFO_TX_EMPTY: u32 = 1 << 8;
const FIFO_RX_EMPTY: u32 = 1 << 24;
const CONF_AUTO_MODE: u32 = 1 << 31;
const AUTO_READ: u32 = 1 << 16;
const AUTO_STOP_AFTER_TRANS: u32 = 1 << 17;
const AUTO_MASTER_RUN: u32 = 1 << 31;
const TRANS_MASTER_BUSY: u32 = 1 << 17;
const ADDR_MASTER_ID_7: u32 = 7 << 24;

/// Polls per wait, at a few MMIO reads each: ~2^16 is tens of milliseconds, far past a 400 kHz byte.
const WAIT_POLLS: u32 = 1 << 16;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// Error codes: G#1 controller not brought up, G#2 master busy before the transfer, G#4 no device (NACK on the address), G#8 bus error bits, G#10 timed out waiting for transfer done, G#20 the read returned nothing.
pub const E_NOT_UP: u32 = 1;
pub const E_BUSY: u32 = 2;
pub const E_NODEV: u32 = 4;
pub const E_BUS: u32 = 8;
pub const E_TIMEOUT: u32 = 0x10;
pub const E_EMPTY: u32 = 0x20;

/// Whether the bootloader (or Linux before a handoff) left the controller running: USI in I2C mode, not in USI reset, auto mode on, a clock divider programmed, master idle.
pub fn up(base: usize) -> bool {
    rd(SYSREG_PERIC1_USI11) & 7 == 4
        && rd(base + USI_CON) & 1 == 0
        && rd(base + CONF) & CONF_AUTO_MODE != 0
        && (rd(base + TIMING_FS3) >> 16) & 0xFF != 0
        && rd(base + TRANS_STATUS) & TRANS_MASTER_BUSY == 0
}

fn wait_done(base: usize) -> Result<(), u32> {
    for _ in 0..WAIT_POLLS {
        let st = rd(base + INT_STATUS);
        if st & INT_NODEV != 0 {
            wr(base + INT_STATUS, st);
            return Err(E_NODEV);
        }
        if st & INT_ERRORS != 0 {
            wr(base + INT_STATUS, st);
            return Err(E_BUS);
        }
        if st & INT_TRANSFER_DONE != 0 {
            wr(base + INT_STATUS, st);
            return Ok(());
        }
    }
    Err(E_TIMEOUT)
}

/// One auto-mode transfer: `tx` bytes written to `target`, or `rx.len()` bytes read from it (one or the other per call), with STOP after it unless `hold` keeps the bus for a following read.
fn transfer(base: usize, target: u8, tx: &[u8], rx: &mut [u8], hold: bool) -> Result<(), u32> {
    if rd(base + TRANS_STATUS) & TRANS_MASTER_BUSY != 0 {
        return Err(E_BUSY);
    }
    let read = !rx.is_empty();
    let len = if read { rx.len() } else { tx.len() } as u32;
    // The driver's FIFO settings: TX trigger 1 / RX trigger 2 with the FIFOs enabled, then the channel direction.
    wr(base + FIFO_CTL, if read { 0x0002_0023 } else { 0x0001_0013 });
    wr(base + CTL, CTL_MASTER | if read { CTL_RXCHON } else { CTL_TXCHON });
    wr(base + INT_STATUS, rd(base + INT_STATUS));
    wr(base + ADDR, ADDR_MASTER_ID_7 | (target as u32) << 10);
    wr(base + INT_ENABLE, INT_TRANSFER_DONE);
    let mut auto = len | if read { AUTO_READ } else { 0 } | if hold { 0 } else { AUTO_STOP_AFTER_TRANS };
    wr(base + AUTO_CONF, auto);
    auto |= AUTO_MASTER_RUN;
    wr(base + AUTO_CONF, auto);
    if read {
        wait_done(base)?;
        for b in rx.iter_mut() {
            if rd(base + FIFO_STATUS) & FIFO_RX_EMPTY != 0 {
                return Err(E_EMPTY);
            }
            *b = rd(base + RX_DATA) as u8;
        }
    } else {
        for &b in tx {
            wr(base + TX_DATA, b as u32);
        }
        wait_done(base)?;
        for _ in 0..WAIT_POLLS {
            if rd(base + FIFO_STATUS) & FIFO_TX_EMPTY != 0 {
                break;
            }
        }
    }
    Ok(())
}

/// A 16-bit little-endian register read (SMBus read-word): the register byte written without STOP, then two bytes read.
pub fn read_word(base: usize, target: u8, reg: u8) -> Result<u16, u32> {
    if !up(base) {
        return Err(E_NOT_UP);
    }
    transfer(base, target, &[reg], &mut [], true)?;
    let mut v = [0u8; 2];
    transfer(base, target, &[], &mut v, false)?;
    Ok(u16::from_le_bytes(v))
}

/// An 8-bit register read: the register byte without STOP, then one byte.
pub fn read_byte(base: usize, target: u8, reg: u8) -> Result<u8, u32> {
    if !up(base) {
        return Err(E_NOT_UP);
    }
    transfer(base, target, &[reg], &mut [], true)?;
    let mut v = [0u8; 1];
    transfer(base, target, &[], &mut v, false)?;
    Ok(v[0])
}

/// An 8-bit register write: register byte and value in one transfer with STOP.
pub fn write_byte(base: usize, target: u8, reg: u8, val: u8) -> Result<(), u32> {
    if !up(base) {
        return Err(E_NOT_UP);
    }
    transfer(base, target, &[reg, val], &mut [], false)
}

/// The MAX77759 charger, on the same bus at G#69: its MODE field (CHG_CNFG_00 [3:0]; 5 = charger + buck on, 4 = buck only, 0 = all off). With the buck off the system runs from the battery while USB data carries on, so the gauge's current is the system's draw: how display power gets measured on a tethered phone.
pub mod charger {
    pub const TARGET: u8 = 0x69;
    pub const CHG_CNFG_00: u8 = 0xB9;
    pub const MODE_MASK: u8 = 0x0F;

    pub fn mode() -> Result<u8, u32> {
        super::read_byte(super::HSI2C_11, TARGET, CHG_CNFG_00)
    }

    /// Writes MODE and leaves the other bits; returns the previous register value for the restore.
    pub fn set_mode(mode: u8) -> Result<u8, u32> {
        let prev = mode_raw()?;
        super::write_byte(super::HSI2C_11, TARGET, CHG_CNFG_00, (prev & !MODE_MASK) | (mode & MODE_MASK))?;
        Ok(prev)
    }

    fn mode_raw() -> Result<u8, u32> {
        super::read_byte(super::HSI2C_11, TARGET, CHG_CNFG_00)
    }

    pub fn restore(raw: u8) -> Result<(), u32> {
        super::write_byte(super::HSI2C_11, TARGET, CHG_CNFG_00, raw)
    }

    /// CHG_CNFG_12: CHGINSEL (bit 5) selects the wired input at all; cleared, the CHGIN path is open regardless of MODE. The stronger "battery only" for a measurement (2026-10-17: MODE 0 alone read 40 % less system current than the day before at the same screen, so something still fed the system from USB).
    pub const CHG_CNFG_12: u8 = 0xC5;
    pub const CHGINSEL: u8 = 1 << 5;

    pub fn input_select(sel: bool) -> Result<u8, u32> {
        let prev = super::read_byte(super::HSI2C_11, TARGET, CHG_CNFG_12)?;
        super::write_byte(super::HSI2C_11, TARGET, CHG_CNFG_12, if sel { prev | CHGINSEL } else { prev & !CHGINSEL })?;
        Ok(prev)
    }

    pub fn restore_input(raw: u8) -> Result<(), u32> {
        super::write_byte(super::HSI2C_11, TARGET, CHG_CNFG_12, raw)
    }
}

/// The MAX77759 fuel gauge (PMIC.md): one sample of what matters for power profiling.
pub mod gauge {
    pub const TARGET: u8 = 0x36;
    pub const REP_SOC: u8 = 0x06;
    pub const TEMP: u8 = 0x08;
    pub const VCELL: u8 = 0x09;
    pub const CURRENT: u8 = 0x0A;
    pub const AVG_CURRENT: u8 = 0x0B;
    pub const AVG_VCELL: u8 = 0x19;

    pub struct Sample {
        /// Per cent, from RepSOC's high byte.
        pub soc: u8,
        /// Hundredths of a degree C.
        pub temp_cc: i32,
        /// Microvolts, 78.125 µV per LSB.
        pub vcell_uv: u32,
        /// Microamps, 312.5 µA per LSB with the 5 mΩ sense; positive = charging.
        pub current_ua: i32,
        pub avg_current_ua: i32,
        pub avg_vcell_uv: u32,
    }

    pub fn sample() -> Result<Sample, u32> {
        let r = |reg| super::read_word(super::HSI2C_11, TARGET, reg);
        let uv = |raw: u16| ((raw as u64) * 78125 / 1000) as u32;
        let ua = |raw: u16| (raw as i16 as i32) * 3125 / 10;
        Ok(Sample {
            soc: (r(REP_SOC)? >> 8) as u8,
            temp_cc: (r(TEMP)? as i16 as i32) * 100 / 256,
            vcell_uv: uv(r(VCELL)?),
            current_ua: ua(r(CURRENT)?),
            avg_current_ua: ua(r(AVG_CURRENT)?),
            avg_vcell_uv: uv(r(AVG_VCELL)?),
        })
    }
}
