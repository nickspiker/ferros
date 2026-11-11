//! The two speaker amplifiers' power (AUDIO.md, KERNEL-OUT.md move 7): Cirrus CS35L41 on spi_19 (HSI0 USI1), left on the controller's CS0, right on GPIO gpn1-0. Their registers are the `amp` process's (payloads/amp, through its grants: the port, gpn1's data bit, gpp1's); this is what it may not do. The kernel's `diag amp state|up` went with the move (2026-11-10).
//!
//! From the husky DT (zuma-shusky-audio.dtsi) and Google's driver (private/google-modules/amplifiers/cs35l41): VA = the sub PMIC's buck A, VP = the battery directly; reset gpp1-1 (left) and gph2-1 (right), active high.

use crate::acpm;
use crate::spi::{gate_open, AMPS};
use core::ptr::{read_volatile, write_volatile};

const GPP1: usize = 0x1084_0020; // PERIC0 bank gpp1
const GPH2: usize = 0x1204_0040; // HSI1 bank gph2
const GPN1: usize = 0x1506_0020; // CUSTOM_ALIVE bank gpn1
const PD_HSI1_STATUS: usize = 0x1546_2B04;
const CMU_HSI1_GPIO_GATE: usize = 0x1200_205C;
const BUCKA_CTRL: u32 = 0x29;

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

/// The right amplifier's reset sits in HSI1: only touched when that block is powered and its GPIO clock open. The PMU status word is a plain read (EL3 answers -22 for it, as for pd_ispfe, camera.rs).
fn hsi1_gpio_ok() -> bool {
    rd(PD_HSI1_STATUS) & 1 == 1 && gate_open(rd(CMU_HSI1_GPIO_GATE))
}

/// A pin to output, driven. gpp1, gph2 and gpn1 have no other writer (the modem drives gph0 and gph1, touch gpn0, the display and GPS other PERIC0 banks), so the read-modify-writes race nothing.
fn pin_out(bank: usize, pin: u32, high: bool) {
    let d = rd(bank + 4);
    wr(bank + 4, if high { d | 1 << pin } else { d & !(1 << pin) });
    wr(bank, (rd(bank) & !(0xF << (pin * 4))) | 1 << (pin * 4));
}

/// The `amp` power switch (process.rs POWERS): on = the port's clocks, USI mode and setup (spi.rs), buck A on if it was off, the right's select an output held high (inactive), then both resets as the driver's reset path does them (low 2 ms, high 2 ms); the process then waits for each one's OTP boot. Off = both held in reset; buck A is left as it is (whether anything else hangs on it is not known). False when HSI1 is off (the right's reset unreachable), the port's clocks stay gated, or the PMIC does not answer.
pub fn power(on: bool) -> bool {
    if !on {
        pin_out(GPP1, 1, false);
        if !hsi1_gpio_ok() {
            return false;
        }
        pin_out(GPH2, 1, false);
        return true;
    }
    if !hsi1_gpio_ok() {
        return false;
    }
    AMPS.up();
    if !AMPS.clocked() {
        return false;
    }
    match acpm::read(acpm::SUB, acpm::T_PMIC, BUCKA_CTRL) {
        Ok(c) if c & 0xC0 == 0 => {
            if acpm::update(acpm::SUB, acpm::T_PMIC, BUCKA_CTRL, 0xC0, 0xC0).is_err() {
                return false;
            }
            crate::flashlog::line(&alloc::format!("power amp: buck A was off (G#{c:02X}), enabled"));
            spin_us(2000);
        }
        Ok(_) => {}
        Err(_) => return false,
    }
    pin_out(GPN1, 0, true);
    pin_out(GPP1, 1, false);
    pin_out(GPH2, 1, false);
    spin_us(2100);
    pin_out(GPP1, 1, true);
    pin_out(GPH2, 1, true);
    spin_us(2100);
    true
}
