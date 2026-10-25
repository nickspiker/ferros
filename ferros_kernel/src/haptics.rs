//! The vibration motor's driver: a Cirrus CS40L26 on hsi2c_0 (PERIC1 USI0, G#10C8_0000), target G#43 (zuma-shusky-haptics.dtsi; Google's driver google-modules/amplifiers/cs40l26).
//!
//! Pins: SCL/SDA gpp19-0/1 (function 3, `hsi2c0_bus`), reset gph2-3 (active low, HSI1), trigger gph2-0, interrupt gpa6-3. The bus is shared with the NFC controller. Registers are 32-bit, addressed by a 32-bit big-endian word, values 32-bit big-endian (the driver's regmap). The chip runs its ROM firmware out of reset; the driver's `cirrus,fw-rom-only` mode plays from it with no download: a ROM waveform is queued by writing its index (G#0180_0000 + n, n ≤ G#26) to the DSP's virtual mailbox G#13020, after a "prevent hibernate" command there. The tuning (boost to 11 V, 2.5 A peak, f0 145 Hz, ReDC 13 Ω, DT) is the RAM firmware's business and is not applied here.

use crate::hsi2c;

const BUS: usize = 0x10C8_0000;
/// hsi2c_0's USI mode word: SYSREG_PERIC1 + 0 (`samsung,usi-offset` 0).
const USI_MODE: usize = 0x10C2_1000;
/// GPIO bank gpp19 (PERIC1 pinctrl G#10C4_0000, the first bank): SCL pin 0, SDA pin 1.
const GPP19: usize = 0x10C4_0000;
/// GPIO bank gph2 (HSI1): reset pin 3, trigger pin 0.
const GPH2: usize = 0x1204_0040;
const PD_HSI1_STATUS: usize = 0x1546_2B04;
const CMU_HSI1_GPIO_GATE: usize = 0x1200_205C;
const TARGET: u8 = 0x43;
/// Bus clock divider (as the camera's buses: ~1 MHz from the USI clock).
const FS_DIV: u32 = 7;

const DEVID: u32 = 0x0;
const REVID: u32 = 0x4;
const IRQ1_STATUS: u32 = 0x10004;
const IRQ1_EINT_1: u32 = 0x10010;
const MBOX: u32 = 0x13020;
const CMD_PREVENT_HIBER: u32 = 0x0200_0003;
const ROM_INDEX: u32 = 0x0180_0000;
/// The ROM firmware's power-state word, by silicon revision (cs40l26.c rom_regs): B2, then A1/B0/B1.
const PM_CUR_STATE_B2: u32 = 0x0280_1F98;
const PM_CUR_STATE_A1: u32 = 0x0280_0370;

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
fn hsi1_ok() -> bool {
    rd(PD_HSI1_STATUS) & 1 == 1 && crate::spi::gate_open(rd(CMU_HSI1_GPIO_GATE))
}

/// One register.
pub fn read(reg: u32) -> Result<u32, u32> {
    let mut b = [0u8; 4];
    hsi2c::write_read(BUS, TARGET, &reg.to_be_bytes(), &mut b)?;
    Ok(u32::from_be_bytes(b))
}
pub fn write(reg: u32, v: u32) -> Result<(), u32> {
    let mut w = [0u8; 8];
    w[..4].copy_from_slice(&reg.to_be_bytes());
    w[4..].copy_from_slice(&v.to_be_bytes());
    hsi2c::write_bytes(BUS, TARGET, &w)
}

fn line(out: &mut alloc::vec::Vec<u8>, s: alloc::string::String) {
    out.extend_from_slice(s.as_bytes());
}

/// `diag haptic`: the bus, the pins and (if the bus is up) the chip's IDs and state, read only.
pub fn state(out: &mut alloc::vec::Vec<u8>) {
    line(out, alloc::format!("hsi2c_0: USI mode G#{:X}, ready {}; CTL {:?} CONF {:?}\n", rd(USI_MODE), hsi2c::ready(BUS, USI_MODE), crate::aoc::probe32(BUS).map(|v| alloc::format!("G#{v:X}")), crate::aoc::probe32(BUS + 0x40).map(|v| alloc::format!("G#{v:X}"))));
    line(out, alloc::format!("gpp19 (SCL 0, SDA 1): CON G#{:X} DAT G#{:X} PUD G#{:X}\n", rd(GPP19), rd(GPP19 + 4), rd(GPP19 + 8)));
    if hsi1_ok() {
        line(out, alloc::format!("gph2 (reset 3, trigger 0): CON G#{:X} DAT G#{:X}\n", rd(GPH2), rd(GPH2 + 4)));
    } else {
        line(out, alloc::format!("gph2: HSI1 off (pd G#{:X}, gate G#{:X})\n", rd(PD_HSI1_STATUS), rd(CMU_HSI1_GPIO_GATE)));
    }
    if hsi2c::ready(BUS, USI_MODE) {
        report(out);
    }
}

fn report(out: &mut alloc::vec::Vec<u8>) {
    let h = |r: Result<u32, u32>| match r {
        Ok(v) => alloc::format!("G#{v:X}"),
        Err(e) => alloc::format!("error G#{e:X}"),
    };
    let rev = read(REVID);
    let pm = if rev == Ok(0xB2) { PM_CUR_STATE_B2 } else { PM_CUR_STATE_A1 };
    line(out, alloc::format!("CS40L26: DEVID {} (G#40A260 A, G#40A26B B) REVID {}; IRQ1 status {} EINT1 {}; mailbox {}; ROM power state {} (0 hibernate, 1 shutdown, 2 standby, 3 active)\n", h(read(DEVID)), h(rev), h(read(IRQ1_STATUS)), h(read(IRQ1_EINT_1)), h(read(MBOX)), h(read(pm))));
}

/// `diag haptic up`: pins to the bus, the controller from cold (if the bootloader left it off), the chip out of reset (low 2 ms, high, then 10 ms for its ROM to boot), then its IDs.
pub fn up(out: &mut alloc::vec::Vec<u8>) {
    pin_field(GPP19, 0, 3);
    pin_field(GPP19, 1, 3);
    if !hsi2c::ready(BUS, USI_MODE) {
        hsi2c::bring_up(BUS, USI_MODE, FS_DIV, 0xFF_FFFF);
        line(out, alloc::format!("hsi2c_0 brought up: ready {}\n", hsi2c::ready(BUS, USI_MODE)));
    }
    if !hsi1_ok() {
        line(out, "gph2: HSI1 off; reset not touched\n".into());
    } else {
        pin_field(GPH2, 3, 1);
        wr(GPH2 + 4, rd(GPH2 + 4) & !(1 << 3));
        ms(2);
        wr(GPH2 + 4, rd(GPH2 + 4) | 1 << 3);
        ms(10);
        line(out, alloc::format!("reset released: gph2 CON G#{:X} DAT G#{:X}\n", rd(GPH2), rd(GPH2 + 4)));
    }
    report(out);
}

/// `diag haptic play <n>`: keep the ROM firmware awake, then queue ROM waveform n; the chip's state before and after.
pub fn play(n: u32, out: &mut alloc::vec::Vec<u8>) {
    report(out);
    let a = write(MBOX, CMD_PREVENT_HIBER);
    ms(2);
    let b = write(MBOX, ROM_INDEX + n.min(0x26));
    line(out, alloc::format!("prevent hibernate {a:?}, ROM waveform {n} {b:?}\n"));
    ms(20);
    report(out);
}
