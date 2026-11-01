//! The touch controller's power: an ST FTM5 on the HSI0 USI2 SPI controller (docs/oracle/TOUCH.md). The controller itself is the touch process's (payloads/touch: the ready event, the GPIO6 fix-up, the chip ID, scan mode, the events); the kernel keeps only what EL0 may not do — the SPI port's clocks and mode, the PMIC rails, the reset line — as the `touch` power switch (KERNEL-OUT.md move 1). The kernel's own bring-up and its `diag touch …` tools (up, watch, paint, map, raw, decay, gamma, info, state, off) went on 2026-11-01, once a finger had typed on the attest screen all day through the process.

use crate::acpm;
use core::ptr::{read_volatile, write_volatile};

/// Reset: gpp21-2 (`pinctrl@10C40000` bank G#40).
const GPP21: usize = 0x10C4_0040;
const RESET_PIN: u32 = 2;

/// The two rails on S2MPG14 (s2mpg14-register.h, s2mpg14-regulator.c): bit 7 enables, [5:0] selects the voltage.
const L25M_CTRL: u32 = 0x43;
const L4M_CTRL: u32 = 0x2E;
/// 1.8 V for DVDD: (1800000 - 700000) / 25000. 3.3 V for AVDD: (3300000 - 1800000) / 25000.
const L25M_1V8: u8 = 0x2C;
const L4M_3V3: u8 = 0x3C;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn spin_us(us: u64) {
    let t = crate::platform::cntpct() + crate::platform::cntfrq() / 1_000_000 * us;
    while crate::platform::cntpct() < t {}
}

fn set_reset(high: bool) {
    let d = rd(GPP21 + 4);
    wr(GPP21 + 4, if high { d | 1 << RESET_PIN } else { d & !(1 << RESET_PIN) });
    let c = rd(GPP21);
    wr(GPP21, (c & !(0xF << (RESET_PIN * 4))) | 1 << (RESET_PIN * 4));
}

/// Program a rail's voltage, then enable it, then read it back.
fn rail_on(reg: u32, vsel: u8) -> Result<u8, u32> {
    acpm::update(acpm::MAIN, acpm::T_PMIC, reg, vsel, 0x3F)?;
    acpm::update(acpm::MAIN, acpm::T_PMIC, reg, 0x80, 0x80)?;
    acpm::read(acpm::MAIN, acpm::T_PMIC, reg)
}

/// The `touch` power switch (process.rs POWERS; KERNEL-OUT.md move 1): what only the kernel may do for the touch controller, in `fts_chip_powercycle`'s order — the SPI port's clocks and mode (the HSI0 CMU, USI2's mode word), the reset line held low, DVDD, 1 ms, AVDD, 15 ms, reset high. The touch process then waits for the controller-ready event and sets it scanning (payloads/touch). Off: reset low, AVDD off, DVDD off. False when the port's clock path stays gated or a rail fails.
pub fn power(on: bool) -> bool {
    if !on {
        set_reset(false);
        let _ = acpm::update(acpm::MAIN, acpm::T_PMIC, L4M_CTRL, 0, 0x80);
        let _ = acpm::update(acpm::MAIN, acpm::T_PMIC, L25M_CTRL, 0, 0x80);
        return true;
    }
    crate::spi::TOUCH.up();
    if !crate::spi::TOUCH.clocked() {
        return false;
    }
    set_reset(false);
    if rail_on(L25M_CTRL, L25M_1V8).is_err() {
        return false;
    }
    spin_us(1000);
    if rail_on(L4M_CTRL, L4M_3V3).is_err() {
        return false;
    }
    spin_us(15_000);
    set_reset(true);
    true
}
