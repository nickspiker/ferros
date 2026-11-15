//! The main rear camera (boitata, sensor@0) on husky: the steps of its power-up the `camera` switch runs (camera_power.rs), from the device tree and the hv register oracle (docs/oracle/CAMERA.md).
//!
//! Power order (`module_power_seq@0`): camera-PMIC LDO1, sub-PMIC GPIO2 (sensor enable), LDO8, sub-PMIC LDO26 and LDO12, LDO4, master-clock pin on, ToF reset, LDO5, the GPIO4 switch; then the sensor reset released and 9 ms before the first I2C.
//! The camera PMIC (SLG51002 at G#75) sits on HS-I2C USI14 (G#1098_0000); the sensor (G#3D) on HS-I2C USI1 (G#1088_0000). The sub-PMIC is reached through ACPM (acpm.rs).
//! The bring-up harness that found all of this (`diag cam …`: the probes, the PDMA runs, the viewfinder, the other three cameras' tries) went on 2026-11-14 once the driver (payloads/camera) streamed on the Pro; its record is docs/oracle/CAMERA.md, its code forge's history before that day.

use crate::acpm;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

const CMU_PERIC0: usize = 0x1080_0000;
const SYSREG_PERIC0: usize = 0x1082_1000;
const GPIO_PERIC0: usize = 0x1084_0000;
const CMU_TOP: usize = 0x2604_0000;

/// The two camera I2C controllers: base, USI mode register (sysreg_peric0 + usi-offset) and the IP / bus / reset-sync clock gates of their CMU_PERIC0 clock.
pub struct Bus {
    pub name: &'static str,
    pub base: usize,
    pub usi_mode: usize,
    pub gates: [usize; 3],
}
pub const SENSOR_BUS: Bus = Bus { name: "USI1 (sensor)", base: 0x1088_0000, usi_mode: SYSREG_PERIC0, gates: [CMU_PERIC0 + 0x2070, CMU_PERIC0 + 0x2074, CMU_PERIC0 + 0x20D0] };
pub const PMIC_BUS: Bus = Bus { name: "USI14 (camera PMIC)", base: 0x1098_0000, usi_mode: SYSREG_PERIC0 + 0x28, gates: [CMU_PERIC0 + 0x2068, CMU_PERIC0 + 0x206C, CMU_PERIC0 + 0x20CC] };


fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// The camera PMIC (SLG51002) and its 16-bit, big-endian register addresses (include/linux/mfd/slg51002.h).
pub const PMIC_ADDR: u8 = 0x75;
/// The main sensor.
pub const SENSOR_ADDR: u8 = 0x3D;

/// Each PERIC0 USI's clock-source mux (cmucal-sfr.c PLL_CON0_MUX_CLKCMU_PERIC0_USIx_USI_USER): bit 4 picks CMU_TOP's PERIC0 IP clock, clear picks the 24.576 MHz oscillator. Cold boot leaves bit 4 set, and the I2C engine then never leaves START (measured 2026-10-19: registers answer on the bus clock, TRANS_STATUS state 1, TX FIFO filling, lines idle high) — the IP clock's upstream is not running. The oscillator is always on.
const USI14_MUX: usize = CMU_PERIC0 + 0x640;
const USI1_MUX: usize = CMU_PERIC0 + 0x650;

/// Clock divider on the 24.576 MHz oscillator: ÷8 gives a 325 ns timing unit and about 192 kHz SCL, twice the unit of the hsi2c_11 config the FS1/FS2 field values were measured on (160 ns at 200 MHz ÷32), so every timing ratio is kept and every interval is longer.
pub const FS_DIV: u32 = 7;

/// Set one pin's 4-bit field in a GPIO bank register.
fn pin_field(reg: usize, pin: u32, val: u32) {
    wr(reg, (rd(reg) & !(0xF << (pin * 4))) | (val & 0xF) << (pin * 4));
}

/// Mux both camera I2C buses and bring their controllers up from a cold boot. The pins follow the device tree: the PMIC bus (gpp16-0/1) to function 2 with no pull; the sensor bus (gpp2-0/1) to its "on_i2c" state, function 3 with pull-ups. No I2C device is addressed.
pub fn buses_up(rep: &mut Vec<u8>) {
    let gpp16 = GPIO_PERIC0 + 0x220;
    let gpp2 = GPIO_PERIC0 + 0x40;
    for pin in 0..2 {
        pin_field(gpp16, pin, 2);
        pin_field(gpp16 + 8, pin, 0);
        pin_field(gpp2, pin, 3);
        pin_field(gpp2 + 8, pin, 3);
    }
    for m in [USI14_MUX, USI1_MUX] {
        wr(m, rd(m) & !(1 << 4));
    }
    for _ in 0..1000 {
        unsafe { core::arch::asm!("nop") };
    }
    crate::hsi2c::bring_up(PMIC_BUS.base, PMIC_BUS.usi_mode, FS_DIV, 0xFF_FFFF);
    crate::hsi2c::bring_up(SENSOR_BUS.base, SENSOR_BUS.usi_mode, FS_DIV, 0x2_0000);
    for b in [&PMIC_BUS, &SENSOR_BUS] {
        line(rep, alloc::format!("{}: ready {} (USI mode G#{:X}, USI_CON G#{:X}, CTL G#{:X}, CONF G#{:X}, FS1 G#{:X} FS2 G#{:X} FS3 G#{:X})\n",
            b.name, crate::hsi2c::ready(b.base, b.usi_mode), rd(b.usi_mode), rd(b.base + 0xC4), rd(b.base), rd(b.base + 0x40), rd(b.base + 0x60), rd(b.base + 0x64), rd(b.base + 0x68)));
    }
    line(rep, alloc::format!("gpp16 CON G#{:X} PUD G#{:X} DAT G#{:X}; gpp2 CON G#{:X} PUD G#{:X} DAT G#{:X}; USI muxes 14 G#{:X} 1 G#{:X}\n", rd(gpp16), rd(gpp16 + 8), rd(gpp16 + 4), rd(gpp2), rd(gpp2 + 8), rd(gpp2 + 4), rd(USI14_MUX), rd(USI1_MUX)));
}

/// Read one camera-PMIC register.
pub fn pmic_read(reg: u16) -> Result<u8, u32> {
    let mut v = [0u8; 1];
    crate::hsi2c::write_read(PMIC_BUS.base, PMIC_ADDR, &reg.to_be_bytes(), &mut v)?;
    Ok(v[0])
}


/// A CMU gate lets the clock through unless it is under manual control (bit 20) with its value (bit 21) clear; with manual off, hardware gating (bit 28) ungates on access (cal-if ra.c `ra_get_gate`).
pub fn gate_open(v: u32) -> bool {
    !(v & (1 << 20) != 0 && v & (1 << 21) == 0)
}

fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}


/// Sub-PMIC GPIOx_SET is G#08 + x in its GPIO block (s2mpg15-register.h); bit 6 enables the output, bit 5 is the level (s2mpg1415-gpio.c).
const GPIO_OEN_OUT: u8 = 0x60;
fn sub_gpio_high(n: u32) -> Result<(), u32> {
    acpm::update(acpm::SUB, acpm::T_GPIO, 0x08 + n, GPIO_OEN_OUT, GPIO_OEN_OUT)
}

fn spin_us(us: u64) {
    let t = crate::platform::cntpct() + crate::platform::cntfrq() / 1_000_000 * us;
    while crate::platform::cntpct() < t {}
}

/// Power the camera PMIC. Its I2C interface needs chip select (sub GPIO0) high: Android keeps it high whenever the camera is closed and the chip does not acknowledge without it (measured 2026-10-19: debugfs gpio shows cs hi / bb lo / buck lo closed, all hi open; ferros cold had cs low and every address NACKed). Then the driver's `slg51002_power_on`: buck-boost enable (sub GPIO1) high, 2 ms, buck enable (sub GPIO3) high, 2 ms. Then read its pattern ID.
pub fn pmic_power_on(rep: &mut Vec<u8>) {
    for (n, label) in [(0, "GPIO0 chip select"), (1, "GPIO1 buck-boost"), (3, "GPIO3 buck")] {
        let r = sub_gpio_high(n);
        let v = acpm::read(acpm::SUB, acpm::T_GPIO, 0x08 + n);
        line(rep, alloc::format!("{label}: set {:?}, reads {:?}\n", r.map_err(|e| alloc::format!("G#{e:X}")), v.map(|x| alloc::format!("G#{x:X}")).map_err(|e| alloc::format!("G#{e:X}"))));
        spin_us(2000);
    }
    if !crate::hsi2c::ready(PMIC_BUS.base, PMIC_BUS.usi_mode) {
        line(rep, alloc::format!("PMIC bus not up (`buses_up` first)\n"));
        return;
    }
    // A failed attempt on the unpowered chip leaves bytes in the TX FIFO: reset the controller before the first real transfer, and give the chip a few more milliseconds (the first read straight after the driver's 2 ms delays was not acknowledged).
    crate::hsi2c::bring_up(PMIC_BUS.base, PMIC_BUS.usi_mode, FS_DIV, 0xFF_FFFF);
    spin_us(5000);
    let mut s = alloc::string::String::from("SLG51002 pattern ID:");
    for r in 0x1105u16..=0x1107 {
        match pmic_read(r) {
            Ok(v) => s.push_str(&alloc::format!(" G#{v:02X}")),
            Err(e) => {
                let b = PMIC_BUS.base;
                s.push_str(&alloc::format!(" error G#{e:X}: TRANS_STATUS G#{:X} INT_STATUS G#{:X} FIFO_STATUS G#{:X}", rd(b + 0x50), rd(b + 0x24), rd(b + 0x30)));
                break;
            }
        }
    }
    s.push('\n');
    line(rep, s);
}


/// Camera-PMIC regulator enables: one bit per LDO in SYSCTL_MATRIX_CONF_A (slg51002-regulator.c `enable_mask = BIT(id)`, LDO1 = bit 0 .. LDO8 = bit 7); the GPIO4 load switch is bit 0 of GPIO4_CTRL (= MUXARRAY_INPUT_SEL_19).
const MATRIX_A: u16 = 0x110D;
const GPIO4_CTRL: u16 = 0x1713;

/// One camera-PMIC rail: V = `base_mv` + code × `step_mv` (slg51002-regulator.c: LDO1-5 base 1.2 V step 10 mV, LDO6-8 base 0.4 V step 5 mV).
/// `target_mv` is what the module wants, from the device tree's consumer nodes, and it is the thing an overvoltage is measured against — not the regulator's own range, which is far wider than any rail's consumer can take.
struct Rail {
    name: &'static str,
    vsel: u16,
    code: u8,
    /// The chip's own min/max code for this rail, a register pair.
    range: u16,
    base_mv: u32,
    step_mv: u32,
    target_mv: u32,
}

/// Every rail on the camera PMIC, with the voltage its consumer actually wants.
/// The chip's cold defaults are not the camera's (LDO1 G#A5 = 2.85 V where the module wants 2.25 V, LDO8 G#8C = 1.10 V where it wants 0.97 V): Linux sets these when it registers the regulators at boot, which is why a camera-open capture never shows them, and why writing a wrong code here is an ordinary mistake rather than an exotic one.
/// The targets are the vendor's own, read out of a running Android (2026-10-21): the slg51002 rails are registered with Linux's regulator framework, so every one of them publishes its voltage under /sys/class/regulator, which is a safer source than any capture and settles four rails no capture had ever shown. Register addresses from slg51002.h; LDO1-5 are the 1.2 V/10 mV group and LDO6-8 the 0.4 V/5 mV one.
/// Enable bits are one per LDO in SYSCTL_MATRIX_CONF_A (`enable_mask = BIT(id)`), so LDO1 is bit 0 and LDO8 is bit 7.
const RAILS: [Rail; 8] = [
    Rail { name: "LDO1 2.25 V", vsel: 0x2000, code: 0x69, range: 0x2060, base_mv: 1200, step_mv: 10, target_mv: 2250 },
    Rail { name: "LDO2 2.95 V", vsel: 0x2200, code: 0xAF, range: 0x2260, base_mv: 1200, step_mv: 10, target_mv: 2950 },
    Rail { name: "LDO3 2.25 V", vsel: 0x2300, code: 0x69, range: 0x2360, base_mv: 1200, step_mv: 10, target_mv: 2250 },
    Rail { name: "LDO4 2.95 V", vsel: 0x2500, code: 0xAF, range: 0x2560, base_mv: 1200, step_mv: 10, target_mv: 2950 },
    Rail { name: "LDO5 2.90 V", vsel: 0x2700, code: 0xAA, range: 0x2760, base_mv: 1200, step_mv: 10, target_mv: 2900 },
    Rail { name: "LDO6 1.10 V", vsel: 0x2900, code: 0x8C, range: 0x2960, base_mv: 400, step_mv: 5, target_mv: 1100 },
    Rail { name: "LDO7 1.06 V", vsel: 0x3100, code: 0x84, range: 0x3160, base_mv: 400, step_mv: 5, target_mv: 1060 },
    Rail { name: "LDO8 0.97 V", vsel: 0x3200, code: 0x72, range: 0x3260, base_mv: 400, step_mv: 5, target_mv: 970 },
];


/// Refuse a voltage code that would put more than a tenth over target on a rail.
///
/// This is the only irreversible mistake in the camera path. The sensor's digital core runs at 0.97 V and its regulator will happily deliver 1.675 V — inside the chip's declared range, past anything the die survives, and gone for good with no recovery of any kind. The guard is deliberately asymmetric: undervoltage only means the part does not work, so anything at or below target passes, while overvoltage is capped at ten percent to leave room for a deliberate experiment and none for an encoding slip.
/// Returns the code's millivolts, or the reason it was refused.
fn check_rail(r: &Rail, code: u8) -> Result<u32, alloc::string::String> {
    let mv = r.base_mv + code as u32 * r.step_mv;
    let ceiling = r.target_mv + r.target_mv / 10;
    if mv > ceiling {
        return Err(alloc::format!("REFUSED: code G#{code:02X} is A#{mv} mV, over the A#{ceiling} mV ceiling for a A#{} mV rail", r.target_mv));
    }
    Ok(mv)
}


fn pmic_write(reg: u16, val: u8) -> Result<(), u32> {
    let r = reg.to_be_bytes();
    crate::hsi2c::write_bytes(PMIC_BUS.base, PMIC_ADDR, &[r[0], r[1], val])
}

/// Set one LDO's enable bit and read the matrix back.
fn ldo_on(bit: u32) -> Result<u8, u32> {
    let m = pmic_read(MATRIX_A)?;
    pmic_write(MATRIX_A, m | (1 << bit) as u8)?;
    pmic_read(MATRIX_A)
}

/// Drive a PERIC0 pin as an output at `high`: level first, then direction, so it never glitches the other way.
fn pin_out(bank: usize, pin: u32, high: bool) {
    let a = GPIO_PERIC0 + bank;
    let d = rd(a + 4);
    wr(a + 4, if high { d | (1 << pin) } else { d & !(1 << pin) });
    pin_field(a, pin, 1);
}

/// Power the main camera module in `module_power_seq@0` order with its delays, after programming and verifying the rail voltages. Refuses if any camera-PMIC rail is already on. On any failure it powers back down.
pub fn power_up(rep: &mut Vec<u8>) -> bool {
    pmic_power_on(rep);
    let step = |rep: &mut Vec<u8>, label: &str, r: Result<alloc::string::String, alloc::string::String>| -> bool {
        match r {
            Ok(s) => {
                line(rep, alloc::format!("  {label}: {s}\n"));
                true
            }
            Err(s) => {
                line(rep, alloc::format!("  {label}: FAILED {s}\n"));
                false
            }
        }
    };
    let hex = |e: u32| alloc::format!("G#{e:X}");
    match pmic_read(MATRIX_A) {
        Ok(0) => {}
        Ok(m) => {
            line(rep, alloc::format!("refusing: camera-PMIC rails already on (MATRIX_CONF_A G#{m:02X})\n"));
            return false;
        }
        Err(e) => {
            line(rep, alloc::format!("refusing: camera PMIC not answering (G#{e:X})\n"));
            return false;
        }
    }
    // Voltages first, verified, while every rail is off.
    for rail in RAILS.iter() {
        let (name, vsel, code, range) = (rail.name, rail.vsel, rail.code, rail.range);
        let r = (|| -> Result<alloc::string::String, alloc::string::String> {
            // The overvoltage guard before anything else, so a bad code never reaches the chip.
            let mv = check_rail(rail, code)?;
            let lo = pmic_read(range).map_err(hex)?;
            let hi = pmic_read(range + 1).map_err(hex)?;
            if code < lo || code > hi {
                return Err(alloc::format!("code G#{code:02X} outside the chip's range G#{lo:02X}..G#{hi:02X}"));
            }
            let before = pmic_read(vsel).map_err(hex)?;
            pmic_write(vsel, code).map_err(hex)?;
            let after = pmic_read(vsel).map_err(hex)?;
            if after != code {
                return Err(alloc::format!("wrote G#{code:02X}, reads G#{after:02X}"));
            }
            Ok(alloc::format!("G#{before:02X} -> G#{after:02X}, A#{mv} mV"))
        })();
        if !step(rep, name, r) {
            power_down(rep);
            return false;
        }
    }
    let ldo = |bit: u32| ldo_on(bit).map(|m| alloc::format!("matrix G#{m:02X}")).map_err(hex);
    let sub = |n: u32| sub_gpio_high(n).map(|_| alloc::string::String::from("high")).map_err(hex);
    let sub_ldo = |reg: u32| acpm::update(acpm::SUB, acpm::T_PMIC, reg, 0x80, 0x80).and_then(|_| acpm::read(acpm::SUB, acpm::T_PMIC, reg)).map(|v| alloc::format!("ctrl G#{v:02X}")).map_err(hex);
    let seq: [(&str, u64); 6] = [("LDO1 on", 500), ("sensor enable (sub GPIO2)", 10), ("LDO8 on", 750), ("sub LDO26 on", 0), ("sub LDO12 on", 0), ("LDO4 on", 1000)];
    for (i, (label, us)) in seq.iter().enumerate() {
        let r = match i {
            0 => ldo(0),
            1 => sub(2),
            2 => ldo(7),
            3 => sub_ldo(0x48),
            4 => sub_ldo(0x3A),
            _ => ldo(3),
        };
        if !step(rep, label, r) {
            power_down(rep);
            return false;
        }
        spin_us(*us);
    }
    // Master clock out on gpp9-0 (function 2, drive 2; CIS_CLK3 itself already runs cold), ToF reset high.
    let g9 = GPIO_PERIC0 + 0x120;
    pin_field(g9 + 0xC, 0, 2);
    pin_field(g9 + 0x8, 0, 0); // no pull, as Linux runs it (pinconf-pins: PUD 0, DRV 2); cold boot leaves a pull-down
    pin_field(g9, 0, 2);
    line(rep, alloc::format!("  master clock pin: gpp9 CON G#{:X}\n", rd(g9)));
    pin_out(0x1E0, 0, true);
    line(rep, alloc::format!("  ToF reset high: gpp15 DAT G#{:X}\n", rd(GPIO_PERIC0 + 0x1E4)));
    if !step(rep, "LDO5 on", ldo(4)) {
        power_down(rep);
        return false;
    }
    spin_us(1000);
    let g4 = (|| -> Result<alloc::string::String, alloc::string::String> {
        let v = pmic_read(GPIO4_CTRL).map_err(hex)?;
        pmic_write(GPIO4_CTRL, v | 1).map_err(hex)?;
        Ok(alloc::format!("GPIO4_CTRL G#{:02X}", pmic_read(GPIO4_CTRL).map_err(hex)?))
    })();
    if !step(rep, "GPIO4 switch on", g4) {
        power_down(rep);
        return false;
    }
    spin_us(1000);
    // The sensor's reset released (its resume sequence), then 9 ms before the first I2C.
    pin_out(0x40, 2, true);
    spin_us(9000);
    line(rep, alloc::format!("  sensor reset released: gpp2 DAT G#{:X}\n", rd(GPIO_PERIC0 + 0x44)));
    true
}


/// Power the module down in the reverse of `module_power_seq@0`, then the camera PMIC's supplies (chip select stays high, as Android leaves it). Best effort: every step runs even if one fails.
pub fn power_down(rep: &mut Vec<u8>) {
    pin_out(0x40, 2, false);
    if let Ok(v) = pmic_read(GPIO4_CTRL) {
        let _ = pmic_write(GPIO4_CTRL, v & !1);
    }
    for bit in [4u32] {
        if let Ok(m) = pmic_read(MATRIX_A) {
            let _ = pmic_write(MATRIX_A, m & !(1 << bit) as u8);
        }
    }
    pin_out(0x1E0, 0, false);
    pin_out(0x120, 0, false);
    for bit in [3u32] {
        if let Ok(m) = pmic_read(MATRIX_A) {
            let _ = pmic_write(MATRIX_A, m & !(1 << bit) as u8);
        }
    }
    let _ = acpm::update(acpm::SUB, acpm::T_PMIC, 0x3A, 0, 0x80);
    let _ = acpm::update(acpm::SUB, acpm::T_PMIC, 0x48, 0, 0x80);
    for bit in [7u32, 0] {
        if let Ok(m) = pmic_read(MATRIX_A) {
            let _ = pmic_write(MATRIX_A, m & !(1 << bit) as u8);
        }
    }
    let _ = acpm::update(acpm::SUB, acpm::T_GPIO, 0x0A, 0x40, 0x60);
    let m = pmic_read(MATRIX_A);
    let _ = acpm::update(acpm::SUB, acpm::T_GPIO, 0x0B, 0x40, 0x60);
    let _ = acpm::update(acpm::SUB, acpm::T_GPIO, 0x09, 0x40, 0x60);
    line(rep, alloc::format!("powered down: MATRIX_CONF_A before supplies off {:?}; sub GPIO1/2/3 {:?} {:?} {:?}\n",
        m.map(|v| alloc::format!("G#{v:02X}")).map_err(|e| alloc::format!("G#{e:X}")),
        acpm::read(acpm::SUB, acpm::T_GPIO, 0x09).map(|v| alloc::format!("G#{v:02X}")).ok(),
        acpm::read(acpm::SUB, acpm::T_GPIO, 0x0A).map(|v| alloc::format!("G#{v:02X}")).ok(),
        acpm::read(acpm::SUB, acpm::T_GPIO, 0x0B).map(|v| alloc::format!("G#{v:02X}")).ok()));
}

/// The ISP front end's power domain (zuma-pm-domains.dtsi pd_ispfe; flexpmu_cal_local_zuma.h ispfe_on): CONFIGURATION bit 0 on, wait for STATUS bit 0. As in Linux's CAL engine (pmucal_rae.c `pmucal_write_reg`), PMU writes go through EL3 (SMC G#82000504, option 1) and PMU reads are plain reads; EL3 refuses a read request for these registers (returned -22, measured 2026-10-19). Then the domain's prepare call exactly as Linux's exynos-pd makes it (exynos-pd_el3.c `exynos_pd_tz_restore`: SMC G#82000410, wake-up = 1, the domain's `need_smc` address, group 2).
const PD_ISPFE_STATUS: u64 = 0x1546_2484;
const PD_ISPFE_NEED_SMC: u64 = 0x1C01_0204;
const CMU_ISPFE: usize = 0x1C00_0000;
const SYSREG_ISPFE: usize = 0x1C02_0000;
/// CSI link 1 (the main camera's; G#1C05_0000 in the `csis-link-phy` space): its version register reads G#5000.
pub const CSIS_LINK1: usize = 0x1C05_0000;

fn pd_prepare(addr: u64) -> u64 {
    let ret: u64;
    unsafe {
        core::arch::asm!("smc #0",
            inout("x0") 0x8200_0410u64 => ret, inout("x1") 1u64 => _, inout("x2") addr => _, inout("x3") 2u64 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _);
    }
    ret
}

/// Whether the ISP front end's power domain reports on.
pub fn fe_powered() -> bool {
    rd(PD_ISPFE_STATUS as usize) & 1 == 1
}

/// S1S_VDD_CAM (sub PMIC BUCK1S, output G#0F, 200 mV + 6.25 mV a step) to 600 mV, where the camera's DVFS puts it while Linux streams (measured 2026-10-21: 537.5 mV idle, 600 mV streaming; INT stays at 568.75 mV either way). A cold boot leaves it at 575 mV. Only ever raised, never lowered.
fn cam_rail(rep: &mut Vec<u8>) {
    let (sub, t) = (crate::acpm::SUB, crate::acpm::T_PMIC);
    let before = crate::acpm::read(sub, t, 0x0F);
    let mut wrote = None;
    if let Ok(c) = before {
        if c < 0x40 {
            wrote = Some(crate::acpm::write(sub, t, 0x0F, 0x40));
            spin_us(1000);
        }
    }
    let after = crate::acpm::read(sub, t, 0x0F);
    let h = |r: Result<u8, u32>| match r { Ok(c) => alloc::format!("G#{c:02X}"), Err(e) => alloc::format!("error G#{e:X}") };
    line(rep, alloc::format!("VDD_CAM: {} -> {} (G#40 = 600 mV, Linux streaming); write {:?}\n", h(before), h(after), wrote));
}

/// CMU_TOP's ISPFE_NOC clock, the source of the front end's bus, the CSI links' ACLK and the D-PHY clock (cmucal-node.c), set as Linux streams it: PLL_SHARED1_D4 undivided, 465 MHz.
/// Cold boot leaves it at PLL_SHARED3_D1 divided by six, 111 MHz. Linux's camera open never writes these registers from the CPU — its DVFS asks the ACPM firmware, which does — so no register capture could show it; the hv's live snapshot of a Linux stream against a cold ferros run did (2026-10-21), with the link counting frames and the core silent at a quarter of the clock.
/// The mux moves first, so the clock only ever slows on the way (SHARED1_D4 ÷ 6, 77 MHz) instead of passing through SHARED3_D1 undivided; each step waits for its busy bit (16).
fn ispfe_noc_clock(rep: &mut Vec<u8>) {
    const MUX: usize = CMU_TOP + 0x10BC;
    const DIV: usize = CMU_TOP + 0x18B4;
    let (m0, d0) = (rd(MUX), rd(DIV));
    let settle = |a: usize| {
        let t = crate::platform::cntpct() + crate::platform::cntfrq() / 1000;
        while rd(a) >> 16 & 1 == 1 && crate::platform::cntpct() < t {}
        rd(a) >> 16 & 1 == 0
    };
    wr(MUX, (m0 & !7) | 4);
    let mok = settle(MUX);
    wr(DIV, d0 & !0xF);
    let dok = settle(DIV);
    line(rep, alloc::format!("CMU_TOP ISPFE_NOC: mux G#{m0:X} -> G#{:X} ({}), divider G#{d0:X} -> G#{:X} ({}): PLL_SHARED1_D4 undivided, as Linux streams\n", rd(MUX), if mok { "settled" } else { "BUSY" }, rd(DIV), if dok { "settled" } else { "BUSY" }));
}


/// Whether the domain prepare call (the secure world's restore, SMC G#8200_0410 with 1) has been made this boot, by the `camera` switch's domain on (pd.rs `on`, the same call) or by `prepare_once`: a second one never returns (2026-10-21).
static PREPARED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The `camera` switch made the prepare call itself (pd.rs `on`, after the boot's gating saved the domain): `prepare_once` must not make it again.
pub fn mark_prepared() {
    PREPARED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// The prepare call itself, once per boot (`PREPARED`): for the `camera` switch when the domain was never gated (a switch on before the boot's gating at 16 s), as the harness's `fe_up` made it on October's cold boots.
pub fn prepare_once() -> Option<u64> {
    (!PREPARED.swap(true, core::sync::atomic::Ordering::Relaxed)).then(|| pd_prepare(PD_ISPFE_NEED_SMC))
}

/// The front end's clocks once its domain is on, as a camera open sets them (oracle capture 2026-10-19), with no PMU access: VDD_CAM to 600 mV, the ISPFE_NOC clock as Linux streams it, CMU_ISPFE's dividers, bus-clock mux, control word and every Q-channel requesting its clock, and SYSREG_ISPFE's two words. the harness's `fe_up`'s second half, now the `camera` switch's.
pub fn fe_clocks(rep: &mut Vec<u8>) {
    crate::flashlog::line("camera: VDD_CAM over ACPM");
    cam_rail(rep);
    crate::flashlog::line("camera: ISPFE_NOC clock");
    ispfe_noc_clock(rep);
    crate::flashlog::line("camera: CMU_ISPFE");
    wr(CMU_ISPFE + 0x1800, 3);
    wr(CMU_ISPFE + 0x1804, 1);
    wr(CMU_ISPFE + 0x0600, 0x10);
    wr(CMU_ISPFE + 0x0800, 0xF11F_F03F);
    let mut q = 0x301C;
    while q <= 0x30D0 {
        if q != 0x30C8 {
            wr(CMU_ISPFE + q, 2);
        }
        q += 4;
    }
    wr(SYSREG_ISPFE + 0x104, 0x3FF_FFFF);
    wr(SYSREG_ISPFE + 0x108, 0);
    line(rep, alloc::format!("CMU_ISPFE div G#{:X}/G#{:X} mux G#{:X} ctl G#{:X}; SYSREG_ISPFE G#{:X}; CSI link 1 version G#{:X}\n",
        rd(CMU_ISPFE + 0x1800), rd(CMU_ISPFE + 0x1804), rd(CMU_ISPFE + 0x600), rd(CMU_ISPFE + 0x800), rd(SYSREG_ISPFE + 0x104), rd(CSIS_LINK1 + 4)));
}

const ISPFE_SYSMMUS: [(usize, usize); 3] = [(0x1C11_0000, 0x213C), (0x1C15_0000, 0x2094), (0x1C19_0000, 0x2098)];
/// ISPFE core clock gate (CLK_CON_GAT_GOUT_BLK_ISPFE_UID_ISPFE_IPCLKPORT_CLK_ISPFE).
const ISPFE_CORE_GATE: usize = CMU_ISPFE + 0x20B8;

/// The front end's SYSMMUs in bypass (VM 0's translation off), so a bus address is a physical address, and its core clock on, for the `camera` switch, which fences the S2MPUs (the harness turned their protection off for every VID) to the camera's frames instead (s2mpu.rs `fence_cam`).
pub fn sysmmu_bypass(rep: &mut Vec<u8>) {
    for (m, gate) in ISPFE_SYSMMUS {
        let g = rd(CMU_ISPFE + gate);
        if !gate_open(g) {
            line(rep, alloc::format!("SYSMMU G#{m:X}: clock gated (G#{g:X}), left alone\n"));
            continue;
        }
        let (ctrl, vm) = (rd(m), rd(m + 0x8000));
        if vm & 1 != 0 {
            wr(m + 0x8000, vm & !1);
        }
        line(rep, alloc::format!("SYSMMU G#{m:X}: CTRL G#{ctrl:X} CTRL_VM0 G#{vm:X} -> G#{:X}\n", rd(m + 0x8000)));
    }
    let g = rd(ISPFE_CORE_GATE);
    if !gate_open(g) {
        wr(ISPFE_CORE_GATE, g | 1 << 20 | 1 << 21);
    }
    line(rep, alloc::format!("ISPFE core gate G#{g:X} -> G#{:X}\n", rd(ISPFE_CORE_GATE)));
}
