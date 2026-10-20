//! The two speaker amplifiers: Cirrus CS35L41 on spi_19 (HSI0 USI1), left on the controller's CS0, right on GPIO gpn1-0 (AUDIO.md).
//!
//! From the husky DT (zuma-shusky-audio.dtsi) and Google's driver (private/google-modules/amplifiers/cs35l41): VA = the sub PMIC's buck A, VP = the battery directly; reset gpp1-1 (left) and gph2-1 (right), active high; registers are 32-bit, addressed by a 32-bit word with bit 31 set for a read, then 16 bits of padding, then the 32-bit value big-endian.
//! Bring-up as the driver's reset path does it: reset low 2 ms, high 2 ms, poll IRQ1_STATUS4 for OTP_BOOT_DONE, then the ID registers (DEVID G#35A40, or G#35B40 for the R part).
//! Audio itself reaches the amplifiers over TDM from the always-on DSP (AoC); this is only the control side.

use crate::acpm;
use crate::spi::{Cs, AMPS};
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

const GPP1: usize = 0x1084_0020; // PERIC0 bank gpp1
const GPH2: usize = 0x1204_0040; // HSI1 bank gph2
const GPN1: usize = 0x1506_0020; // CUSTOM_ALIVE bank gpn1
const PD_HSI1_STATUS: u64 = 0x1546_2B04;
const CMU_HSI1_GPIO_GATE: usize = 0x1200_205C;
const BUCKA_CTRL: u32 = 0x29;
const BUCKA_OUT: u32 = 0x2A;

const DEVID: u32 = 0x0;
const REVID: u32 = 0x4;
const FABID: u32 = 0x8;
const IRQ1_STATUS4: u32 = 0x1_001C;
const OTP_BOOT_DONE: u32 = 0x2;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}
fn spin_us(us: u64) {
    let t = crate::platform::cntpct() + crate::platform::cntfrq() / 1_000_000 * us;
    while crate::platform::cntpct() < t {}
}

/// The right amplifier's reset sits in HSI1: only touched when that block is powered and its GPIO clock open.
fn hsi1_gpio_ok() -> bool {
    // The PMU status word is a plain read (EL3 answers -22 for it, as for pd_ispfe, camera.rs).
    rd(PD_HSI1_STATUS as usize) & 1 == 1 && crate::spi::gate_open(rd(CMU_HSI1_GPIO_GATE))
}

#[derive(Clone, Copy)]
struct Amp {
    name: &'static str,
    cs: Cs,
    reset: (usize, u32),
}
const LEFT: Amp = Amp { name: "left", cs: Cs::Native, reset: (GPP1, 1) };
const RIGHT: Amp = Amp { name: "right", cs: Cs::Gpio(GPN1, 0), reset: (GPH2, 1) };

fn pin_out(bank: usize, pin: u32, high: bool) {
    let d = rd(bank + 4);
    wr(bank + 4, if high { d | 1 << pin } else { d & !(1 << pin) });
    wr(bank, (rd(bank) & !(0xF << (pin * 4))) | 1 << (pin * 4));
}

fn read32(a: &Amp, reg: u32) -> Result<u32, &'static str> {
    let r = (reg | 0x8000_0000).to_be_bytes();
    let mut v = [0u8; 4];
    AMPS.transact(a.cs, &[r[0], r[1], r[2], r[3], 0, 0], &mut v)?;
    Ok(u32::from_be_bytes(v))
}

fn hex(r: Result<u8, u32>) -> alloc::string::String {
    match r {
        Ok(v) => alloc::format!("G#{v:02X}"),
        Err(e) => alloc::format!("error G#{e:X}"),
    }
}

/// `diag amp state`: the controller's clock path, the pins, buck A — read only.
pub fn state(rep: &mut Vec<u8>) {
    line(rep, alloc::format!("USI1: {}\n", AMPS.describe()));
    line(rep, alloc::format!("gpp1 (left reset = pin 1): CON G#{:X} DAT G#{:X}\n", rd(GPP1), rd(GPP1 + 4)));
    line(rep, alloc::format!("gpn1 (right CS = pin 0): CON G#{:X} DAT G#{:X}\n", rd(GPN1), rd(GPN1 + 4)));
    if hsi1_gpio_ok() {
        line(rep, alloc::format!("gph2 (right reset = pin 1): CON G#{:X} DAT G#{:X}\n", rd(GPH2), rd(GPH2 + 4)));
    } else {
        line(rep, alloc::format!("gph2: not read: pd_hsi1 status G#{:X}, HSI1 GPIO gate G#{:X}\n", rd(PD_HSI1_STATUS as usize), rd(CMU_HSI1_GPIO_GATE)));
    }
    line(rep, alloc::format!("buck A (VA): ctrl {} out {}\n", hex(acpm::read(acpm::SUB, acpm::T_PMIC, BUCKA_CTRL)), hex(acpm::read(acpm::SUB, acpm::T_PMIC, BUCKA_OUT))));
}

/// `diag amp up`: SPI up, buck A on if it was off, then each amplifier through its reset, boot-done and ID registers.
pub fn up(rep: &mut Vec<u8>) {
    AMPS.up();
    if !AMPS.clocked() {
        return line(rep, alloc::format!("SPI clock path still gated; stopping\n"));
    }
    line(rep, alloc::format!("SPI up: {}\n", AMPS.describe()));
    match acpm::read(acpm::SUB, acpm::T_PMIC, BUCKA_CTRL) {
        Ok(c) if c & 0xC0 == 0 => {
            let r = acpm::update(acpm::SUB, acpm::T_PMIC, BUCKA_CTRL, 0xC0, 0xC0);
            line(rep, alloc::format!("buck A was off (G#{c:02X}); enabled: {:?} -> {}\n", r, hex(acpm::read(acpm::SUB, acpm::T_PMIC, BUCKA_CTRL))));
            spin_us(2000);
        }
        r => line(rep, alloc::format!("buck A ctrl {}\n", hex(r))),
    }
    pin_out(GPN1, 0, true);
    for a in [LEFT, RIGHT] {
        if a.reset.0 == GPH2 && !hsi1_gpio_ok() {
            line(rep, alloc::format!("{}: skipped (HSI1 GPIO not reachable)\n", a.name));
            continue;
        }
        pin_out(a.reset.0, a.reset.1, false);
        spin_us(2100);
        pin_out(a.reset.0, a.reset.1, true);
        spin_us(2100);
        let t0 = crate::platform::cntpct();
        let mut boot = None;
        for _ in 0..100 {
            match read32(&a, IRQ1_STATUS4) {
                Ok(s) if s & OTP_BOOT_DONE != 0 => {
                    boot = Some(s);
                    break;
                }
                Ok(_) => spin_us(1000),
                Err(e) => {
                    line(rep, alloc::format!("{}: SPI {e}\n", a.name));
                    break;
                }
            }
        }
        let ms = (crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000);
        match boot {
            Some(s) => line(rep, alloc::format!("{}: OTP boot done after A#{ms} ms (IRQ1_STATUS4 G#{s:X})\n", a.name)),
            None => line(rep, alloc::format!("{}: no OTP_BOOT_DONE in A#{ms} ms\n", a.name)),
        }
        let id = |r| read32(&a, r).map(|v| alloc::format!("G#{v:X}")).unwrap_or_else(|e| alloc::string::String::from(e));
        line(rep, alloc::format!("{}: DEVID {} (CS35L41 is G#35A40, R part G#35B40) REVID {} FABID {}\n", a.name, id(DEVID), id(REVID), id(FABID)));
    }
    // Both report the same IDs: prove the right one is answering on its own by holding the left in reset while reading it.
    if hsi1_gpio_ok() {
        pin_out(LEFT.reset.0, LEFT.reset.1, false);
        spin_us(2100);
        let r = read32(&RIGHT, DEVID).map(|v| alloc::format!("G#{v:X}")).unwrap_or_else(|e| alloc::string::String::from(e));
        let l = read32(&LEFT, DEVID).map(|v| alloc::format!("G#{v:X}")).unwrap_or_else(|e| alloc::string::String::from(e));
        pin_out(LEFT.reset.0, LEFT.reset.1, true);
        spin_us(2100);
        line(rep, alloc::format!("with the left held in reset: right DEVID {r}, left DEVID {l} (expect the right to answer and the left not to)\n"));
    }
}
