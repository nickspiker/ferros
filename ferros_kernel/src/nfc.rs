//! The NFC controller's power (docs/NFC.md): an ST54L (ST21NFC NCI interface plus the eSE) on hsi2c_0 (PERIC1 USI0, G#10C8_0000) at target G#08, the bus the vibration motor shares (haptics.rs). The driver is the `nfc` process (payloads/nfc), which talks NCI over the bus grant; this is what it cannot do: the bus's pin modes and controller from cold, the chip's reset (gph1-6, in the bank whose other pins are the modem's), and the PMU clock pad the chip gates its own reference clock with.
//!
//! Pins (zuma-shusky-nfc.dtsi): reset gph1-6 (HSI1, high = running), interrupt gpa7-6 (GPIO_ALIVE, high while the chip holds a message, read as a level by the driver), the eSE's reset gph1-5 never touched. ABL leaves the chip out of reset and the bus up (measured 2026-10-31: gph1 DAT G#E7, hsi2c_0 in I2C mode).

use crate::hsi2c;

const BUS: usize = 0x10C8_0000;
const USI_MODE: usize = 0x10C2_1000;
const GPP19: usize = 0x10C4_0000;
const GPH1: usize = 0x1204_0020;
const RESET_PIN: u32 = 6;
const PD_HSI1_STATUS: usize = 0x1546_2B04;
const CMU_HSI1_GPIO_GATE: usize = 0x1200_205C;
/// The PMU's CLKOUT1 word (G#1546_3E84, clkout_zuma.c) through its atomic aliases, as `exynos_pmu_update` reaches a word below G#4000 (set at +G#C000, clear at +G#8000, the value written being the bit's index): bit 16, EXTPAD_CONTROL_EN, lets the chip's clock-request pin gate the TCXO out to it (st21nfc.c, `st,extpad_control_en`).
const CLKOUT1_SET: u64 = 0x1546_FE84;
const CLKOUT1_CLEAR: u64 = 0x1546_BE84;
const EXTPAD_CONTROL_EN: u32 = 16;
/// The bus divider for 400 kHz (the DT's clock-frequency): haptics' 7 runs it near 1 MHz, so (7 + 1) × 2.5.
const FS_DIV: u32 = 19;

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
fn ms(n: u64) {
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < crate::platform::cntfrq() / 1000 * n {
        core::hint::spin_loop();
    }
}
fn pin_field(reg: usize, pin: u32, val: u32) {
    wr(reg, (rd(reg) & !(0xF << (pin * 4))) | (val & 0xF) << (pin * 4));
}
fn reset(high: bool) {
    let d = rd(GPH1 + 4);
    wr(GPH1 + 4, if high { d | 1 << RESET_PIN } else { d & !(1 << RESET_PIN) });
}

/// On: the bus pins and controller (brought up only if nothing has: the motor may be using it), the clock pad under the chip's control, then the reset pulsed as the HAL's `ST21NFC_PULSE_RESET` does (low 20 ms, high 10 ms, low 20 ms, high: the second pulse takes the chip out of its quick-boot mode) and 16 ms for its ROM, after which it holds a CORE_RESET notification for the driver. Off: the clock pad back to the PMU and the chip held in reset. False if HSI1, the reset's bank, is off.
pub fn power(on: bool) -> bool {
    if !(rd(PD_HSI1_STATUS) & 1 == 1 && crate::spi::gate_open(rd(CMU_HSI1_GPIO_GATE))) {
        return false;
    }
    if !on {
        reset(false);
        crate::platform::priv_reg(CLKOUT1_CLEAR, true, EXTPAD_CONTROL_EN);
        return true;
    }
    pin_field(GPP19, 0, 3);
    pin_field(GPP19, 1, 3);
    if !hsi2c::ready(BUS, USI_MODE) {
        hsi2c::bring_up(BUS, USI_MODE, FS_DIV, 0xFF_FFFF);
    }
    crate::platform::priv_reg(CLKOUT1_SET, true, EXTPAD_CONTROL_EN);
    pin_field(GPH1, RESET_PIN, 1);
    reset(false);
    ms(20);
    reset(true);
    ms(10);
    reset(false);
    ms(20);
    reset(true);
    ms(16);
    hsi2c::ready(BUS, USI_MODE)
}
