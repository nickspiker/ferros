//! The power of hsi2c_0's two devices (docs/NFC.md, KERNEL-OUT.md decision 8): an ST54L NFC controller (ST21NFC NCI interface plus the eSE) at target G#08 and the vibration motor's CS40L26 at G#43, on PERIC1 USI0 (G#10C8_0000). Both are run by the `nfc` process (payloads/nfc), the bus's one owner, over its bus grant; this is what it cannot do: the bus's pin modes and controller from cold, the NFC chip's reset (gph1-6, in the bank whose other pins are the modem's) and the PMU clock pad it gates its own reference clock with, and the motor's reset (gph2-3, HSI1 too). The kernel's haptics.rs and its `diag haptic` tools went on 2026-11-10: the motor's protocol is the process's (ops 7 and 8).
//!
//! Pins (zuma-shusky-nfc.dtsi, zuma-shusky-haptics.dtsi): NFC reset gph1-6 (HSI1, high = running), interrupt gpa7-6 (GPIO_ALIVE, high while the chip holds a message, read as a level by the driver), the eSE's reset gph1-5 never touched; the motor's reset gph2-3 (active low), its trigger gph2-0 and interrupt gpa6-3 unused. ABL leaves the NFC chip out of reset and the bus up (measured 2026-10-31: gph1 DAT G#E7, hsi2c_0 in I2C mode). Nothing else drives gph2 (the modem drives gph0 and gph1).

use crate::hsi2c;

const BUS: usize = 0x10C8_0000;
const USI_MODE: usize = 0x10C2_1000;
const GPP19: usize = 0x10C4_0000;
const GPH1: usize = 0x1204_0020;
const RESET_PIN: u32 = 6;
const GPH2: usize = 0x1204_0040;
const MOTOR_RESET_PIN: u32 = 3;
const PD_HSI1_STATUS: usize = 0x1546_2B04;
const CMU_HSI1_GPIO_GATE: usize = 0x1200_205C;
/// The PMU's CLKOUT1 word (G#1546_3E84, clkout_zuma.c) through its atomic aliases, as `exynos_pmu_update` reaches a word below G#4000 (set at +G#C000, clear at +G#8000, the value written being the bit's index): bit 16, EXTPAD_CONTROL_EN, lets the chip's clock-request pin gate the TCXO out to it (st21nfc.c, `st,extpad_control_en`).
const CLKOUT1_SET: u64 = 0x1546_FE84;
const CLKOUT1_CLEAR: u64 = 0x1546_BE84;
const EXTPAD_CONTROL_EN: u32 = 16;
/// The bus divider for 400 kHz (the DT's clock-frequency): 7 ran it near 1 MHz (the kernel's motor tools did), so (7 + 1) × 2.5.
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

fn motor_reset(high: bool) {
    let d = rd(GPH2 + 4);
    wr(GPH2 + 4, if high { d | 1 << MOTOR_RESET_PIN } else { d & !(1 << MOTOR_RESET_PIN) });
}

/// The `nfc` power switch, the bus's (process.rs POWERS). On: the bus pins and controller (brought up only if nothing has), the clock pad under the NFC chip's control, then its reset pulsed as the HAL's `ST21NFC_PULSE_RESET` does (low 20 ms, high 10 ms, low 20 ms, high: the second pulse takes the chip out of its quick-boot mode) and 16 ms for its ROM, after which it holds a CORE_RESET notification for the driver. The motor's reset goes low with the first pulse and high at its end: low 20 ms (its driver holds 2), then 46 ms for its ROM to boot (it needs 10), inside the NFC chip's own waits. Off: the clock pad back to the PMU and both chips held in reset. False if HSI1, the resets' bank, is off.
pub fn power(on: bool) -> bool {
    if !(rd(PD_HSI1_STATUS) & 1 == 1 && crate::spi::gate_open(rd(CMU_HSI1_GPIO_GATE))) {
        return false;
    }
    if !on {
        reset(false);
        motor_reset(false);
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
    pin_field(GPH2, MOTOR_RESET_PIN, 1);
    reset(false);
    motor_reset(false);
    ms(20);
    reset(true);
    motor_reset(true);
    ms(10);
    reset(false);
    ms(20);
    reset(true);
    ms(16);
    hsi2c::ready(BUS, USI_MODE)
}
