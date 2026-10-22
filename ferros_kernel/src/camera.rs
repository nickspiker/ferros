//! The main rear camera (boitata, sensor@0) on husky: what its power-up touches, from the device tree and the hv register oracle (docs/oracle/CAMERA.md).
//!
//! Power order (`module_power_seq@0`): camera-PMIC LDO1, sub-PMIC GPIO2 (sensor enable), LDO8, sub-PMIC LDO26 and LDO12, LDO4, master-clock pin on, ToF reset, LDO5, the GPIO4 switch; then the sensor reset released and 9 ms before the first I2C.
//! The camera PMIC (SLG51002 at G#75) sits on HS-I2C USI14 (G#1098_0000); the sensor (G#3D) on HS-I2C USI1 (G#1088_0000). The sub-PMIC is reached through ACPM (acpm.rs).
//! This first version only reports state, so a cold boot can be measured before anything is written.

use crate::acpm;
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};

const CMU_PERIC0: usize = 0x1080_0000;
const SYSREG_PERIC0: usize = 0x1082_1000;
const GPIO_PERIC0: usize = 0x1084_0000;
const CMU_TOP: usize = 0x2604_0000;

/// The two camera I2C controllers: base, USI mode register (sysreg_peric0 + usi-offset), CMU_PERIC0 divider and its IP / bus / reset-sync clock gates.
pub struct Bus {
    pub name: &'static str,
    pub base: usize,
    pub usi_mode: usize,
    pub div: usize,
    pub gates: [usize; 3],
}
pub const SENSOR_BUS: Bus = Bus { name: "USI1 (sensor)", base: 0x1088_0000, usi_mode: SYSREG_PERIC0, div: CMU_PERIC0 + 0x1810, gates: [CMU_PERIC0 + 0x2070, CMU_PERIC0 + 0x2074, CMU_PERIC0 + 0x20D0] };
pub const PMIC_BUS: Bus = Bus { name: "USI14 (camera PMIC)", base: 0x1098_0000, usi_mode: SYSREG_PERIC0 + 0x28, div: CMU_PERIC0 + 0x180C, gates: [CMU_PERIC0 + 0x2068, CMU_PERIC0 + 0x206C, CMU_PERIC0 + 0x20CC] };

/// PERIC0 GPIO banks the camera uses (pinctrl-gs.c zuma_pin_peric0): CON at +0 (4 bits per pin), DAT +4, PUD +8, DRV +C.
const BANKS: [(&str, usize); 4] = [("gpp2 (sensor I2C 0-1, reset 2)", 0x40), ("gpp9 (master clock 0)", 0x120), ("gpp15 (ToF reset 0)", 0x1E0), ("gpp16 (PMIC I2C 0-1)", 0x220)];

/// CIS_CLK3, the sensor's 26 MHz master clock (cmucal-sfr.c): mux, divider, gate, and the clock-gating control Linux writes 2 at camera open and 0 at close.
const CIS_CLK3: [(&str, usize); 4] = [("mux", CMU_TOP + 0x101C), ("div", CMU_TOP + 0x181C), ("gate", CMU_TOP + 0x2044), ("qch", CMU_TOP + 0x3010)];

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
const FS_DIV: u32 = 7;

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

/// The camera PMIC's identity and the four regulators the main camera uses, read only: chip pattern ID, the power-sequencer resource enables, and each LDO's voltage select, range, configuration and actual select.
pub fn pmic_dump(rep: &mut Vec<u8>) {
    if !crate::hsi2c::ready(PMIC_BUS.base, PMIC_BUS.usi_mode) {
        line(rep, alloc::format!("PMIC bus not up: run `diag cam bus` first\n"));
        return;
    }
    let mut s = alloc::string::String::from("SLG51002 pattern ID:");
    for r in 0x1105u16..=0x1107 {
        match pmic_read(r) {
            Ok(v) => s.push_str(&alloc::format!(" G#{v:02X}")),
            Err(e) => {
                let b = PMIC_BUS.base;
                s.push_str(&alloc::format!(" error G#{e:X} (G#4 no acknowledge, G#10 never finished): TRANS_STATUS G#{:X} INT_STATUS G#{:X} FIFO_STATUS G#{:X}", rd(b + 0x50), rd(b + 0x24), rd(b + 0x30)));
                s.push('\n');
                line(rep, s);
                return;
            }
        }
    }
    s.push('\n');
    line(rep, s);
    let mut s = alloc::string::String::from("resource enables G#1900..G#1905:");
    for r in 0x1900u16..=0x1905 {
        s.push_str(&match pmic_read(r) { Ok(v) => alloc::format!(" G#{v:02X}"), Err(e) => alloc::format!(" err G#{e:X}") });
    }
    s.push('\n');
    line(rep, s);
    for (name, regs) in [("LDO1", [0x2000u16, 0x2060, 0x2061, 0x2064, 0x2065, 0x2066]), ("LDO4", [0x2500, 0x2560, 0x2561, 0x2564, 0x2565, 0x2566]), ("LDO5", [0x2700, 0x2760, 0x2761, 0x2764, 0x2765, 0x2766]), ("LDO8", [0x3200, 0x3260, 0x3261, 0x3265, 0x3266, 0x3267])] {
        let mut s = alloc::format!("{name} vsel/minv/maxv/conf1/conf2/actual:");
        for r in regs {
            s.push_str(&match pmic_read(r) { Ok(v) => alloc::format!(" G#{v:02X}"), Err(e) => alloc::format!(" err G#{e:X}") });
        }
        s.push('\n');
        line(rep, s);
    }
}

/// A CMU gate lets the clock through unless it is under manual control (bit 20) with its value (bit 21) clear; with manual off, hardware gating (bit 28) ungates on access (cal-if ra.c `ra_get_gate`).
pub fn gate_open(v: u32) -> bool {
    !(v & (1 << 20) != 0 && v & (1 << 21) == 0)
}

fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}

/// Everything the power-up depends on, read only. An I2C controller's registers are read only when its bus clock gate is open: a read into a gated block can hang the core (the hv's display readback, HV.md).
pub fn state(rep: &mut Vec<u8>) {
    for b in [&SENSOR_BUS, &PMIC_BUS] {
        let g: [u32; 3] = [rd(b.gates[0]), rd(b.gates[1]), rd(b.gates[2])];
        line(rep, alloc::format!("{}: USI mode G#{:X}, div G#{:X}, gates ip G#{:X} bus G#{:X} rst G#{:X}\n", b.name, rd(b.usi_mode), rd(b.div), g[0], g[1], g[2]));
        if gate_open(g[1]) {
            line(rep, alloc::format!("  regs: USI_CON G#{:X} CTL G#{:X} CONF G#{:X} FS3 G#{:X} TRANS G#{:X}\n", rd(b.base + 0xC4), rd(b.base), rd(b.base + 0x40), rd(b.base + 0x68), rd(b.base + 0x50)));
        } else {
            line(rep, alloc::format!("  regs: not read, bus clock gated\n"));
        }
    }
    for (name, off) in BANKS {
        let a = GPIO_PERIC0 + off;
        line(rep, alloc::format!("{name}: CON G#{:X} DAT G#{:X} PUD G#{:X} DRV G#{:X}\n", rd(a), rd(a + 4), rd(a + 8), rd(a + 0xC)));
    }
    let mut s = alloc::string::String::from("CIS_CLK3:");
    for (name, a) in CIS_CLK3 {
        s.push_str(&alloc::format!(" {name} G#{:X}", rd(a)));
    }
    s.push('\n');
    line(rep, s);
    line(rep, acpm::describe(acpm::PMIC_CHANNEL));
    let show = |rep: &mut Vec<u8>, label: &str, r: Result<u8, u32>| match r {
        Ok(v) => line(rep, alloc::format!("  {label}: G#{v:X}\n")),
        Err(e) => line(rep, alloc::format!("  {label}: error G#{e:X}\n")),
    };
    show(rep, "main PMIC top G#0B", acpm::read(acpm::MAIN, acpm::T_TOP, 0x0B));
    show(rep, "sub PMIC top G#0B", acpm::read(acpm::SUB, acpm::T_TOP, 0x0B));
    show(rep, "sub GPIO0_SET (camera PMIC chip select)", acpm::read(acpm::SUB, acpm::T_GPIO, 0x08));
    show(rep, "sub GPIO1_SET (camera PMIC buck-boost enable)", acpm::read(acpm::SUB, acpm::T_GPIO, 0x09));
    show(rep, "sub GPIO2_SET (sensor enable)", acpm::read(acpm::SUB, acpm::T_GPIO, 0x0A));
    show(rep, "sub GPIO3_SET (camera PMIC buck enable)", acpm::read(acpm::SUB, acpm::T_GPIO, 0x0B));
    show(rep, "sub L12S_CTRL (1.85 V rail)", acpm::read(acpm::SUB, acpm::T_PMIC, 0x3A));
    show(rep, "sub L26S_CTRL (3.3 V rail)", acpm::read(acpm::SUB, acpm::T_PMIC, 0x48));
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
        line(rep, alloc::format!("PMIC bus not up: run `diag cam bus` first\n"));
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

/// Probe every 7-bit address on the camera-PMIC bus with a one-byte read and list the ones that acknowledge. The flash driver (LM3644, G#63) shares the bus and is expected to answer whatever the camera PMIC's state; if nothing answers, the bus itself is at fault.
pub fn scan(rep: &mut Vec<u8>) {
    crate::hsi2c::bring_up(PMIC_BUS.base, PMIC_BUS.usi_mode, FS_DIV, 0xFF_FFFF);
    let mut found = alloc::string::String::new();
    let mut other = 0u32;
    for a in 0x08u8..0x78 {
        let mut b = [0u8; 1];
        match crate::hsi2c::read_one(PMIC_BUS.base, a, &mut b) {
            Ok(()) => found.push_str(&alloc::format!(" G#{a:02X}")),
            Err(e) => {
                if e != crate::hsi2c::E_NODEV {
                    other += 1;
                }
                crate::hsi2c::bring_up(PMIC_BUS.base, PMIC_BUS.usi_mode, FS_DIV, 0xFF_FFFF);
            }
        }
    }
    line(rep, alloc::format!("PMIC bus scan: acknowledged{}; non-NACK failures {other}\n", if found.is_empty() { " none".into() } else { found }));
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
    bit: u32,
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
    Rail { name: "LDO1 2.25 V", vsel: 0x2000, code: 0x69, bit: 0, range: 0x2060, base_mv: 1200, step_mv: 10, target_mv: 2250 },
    Rail { name: "LDO2 2.95 V", vsel: 0x2200, code: 0xAF, bit: 1, range: 0x2260, base_mv: 1200, step_mv: 10, target_mv: 2950 },
    Rail { name: "LDO3 2.25 V", vsel: 0x2300, code: 0x69, bit: 2, range: 0x2360, base_mv: 1200, step_mv: 10, target_mv: 2250 },
    Rail { name: "LDO4 2.95 V", vsel: 0x2500, code: 0xAF, bit: 3, range: 0x2560, base_mv: 1200, step_mv: 10, target_mv: 2950 },
    Rail { name: "LDO5 2.90 V", vsel: 0x2700, code: 0xAA, bit: 4, range: 0x2760, base_mv: 1200, step_mv: 10, target_mv: 2900 },
    Rail { name: "LDO6 1.10 V", vsel: 0x2900, code: 0x8C, bit: 5, range: 0x2960, base_mv: 400, step_mv: 5, target_mv: 1100 },
    Rail { name: "LDO7 1.06 V", vsel: 0x3100, code: 0x84, bit: 6, range: 0x3160, base_mv: 400, step_mv: 5, target_mv: 1060 },
    Rail { name: "LDO8 0.97 V", vsel: 0x3200, code: 0x72, bit: 7, range: 0x3260, base_mv: 400, step_mv: 5, target_mv: 970 },
];

/// One camera module: which bus answers it, where its pins are, and which rails it needs.
/// Everything here is from the device tree (`zuma-husky-camera.dtsi` for the module, `zuma-pinctrl` for the pins): the four HS-I2C controllers are the same IP at G#1088/1089/108A/108B_0000 with their buses on gpp2/4/6/8 pins 0 and 1 at function 3, and each module has its own reset pin, its own master-clock pin at function 2, and its own load switch on the PMIC.
/// `wide_regs` is false for the one module that answers 8-bit register values where the others answer 16 — without it that sensor looks dead.
pub struct Module {
    pub name: &'static str,
    pub role: &'static str,
    pub bus: usize,
    pub bus_bank: usize,
    pub addr: u8,
    pub reset_bank: usize,
    pub reset_pin: u32,
    pub mclk_bank: usize,
    /// This bus's USI software-configuration register, an offset into SYSREG_PERIC0: hsi2c_1 to hsi2c_4 are +0, +4, +8 and +G#C (zuma-usi.dtsi `samsung,usi-offset`).
    pub usi_off: usize,
    pub clock: &'static str,
    pub wide_regs: bool,
    /// Which of RAILS this module needs, by index.
    pub rails: &'static [usize],
    /// Its load switch on the PMIC: the GPIO control register.
    pub switch: u16,
    /// Its enable line on the sub-PMIC, if it has one (GPIO number in the sub block).
    pub sub_enable: Option<u8>,
    /// Whether it takes the sub-PMIC's LDO26 as well as LDO12: the two rear modules do, the other two do not (their device-tree supply lists).
    pub sub_ldo26: bool,
}

/// GPP bank offsets inside GPIO_PERIC0, as the pin names map to them.
const GPP2: usize = 0x40;
const GPP3: usize = 0x60;
const GPP4: usize = 0x80;
const GPP5: usize = 0xA0;
const GPP6: usize = 0xC0;
const GPP7: usize = 0xE0;
const GPP8: usize = 0x100;
const GPP9: usize = 0x120;

pub const MODULES: [Module; 4] = [
    Module { name: "boitata", role: "main wide", bus: 0x1088_0000, bus_bank: GPP2, addr: 0x3D, reset_bank: GPP2, reset_pin: 2, mclk_bank: GPP9, usi_off: 0x0, clock: "CIS_CLK3 26 MHz", wide_regs: true, rails: &[0, 3, 4, 7], switch: 0x1713, sub_enable: Some(2), sub_ldo26: true },
    Module { name: "dokkaebi", role: "tele", bus: 0x1089_0000, bus_bank: GPP4, addr: 0x10, reset_bank: GPP4, reset_pin: 2, mclk_bank: GPP5, usi_off: 0x4, clock: "CIS_CLK1 24 MHz", wide_regs: true, rails: &[0, 7], switch: 0x1712, sub_enable: Some(4), sub_ldo26: true },
    Module { name: "oksoko", role: "ultrawide", bus: 0x108A_0000, bus_bank: GPP6, addr: 0x1A, reset_bank: GPP6, reset_pin: 3, mclk_bank: GPP3, usi_off: 0x8, clock: "CIS_CLK0 24 MHz", wide_regs: false, rails: &[0, 1, 3, 5, 7], switch: 0x1711, sub_enable: None, sub_ldo26: false },
    Module { name: "kraken", role: "front", bus: 0x108B_0000, bus_bank: GPP8, addr: 0x2D, reset_bank: GPP6, reset_pin: 2, mclk_bank: GPP7, usi_off: 0xC, clock: "CIS_CLK2 24 MHz", wide_regs: true, rails: &[0, 2, 4, 6, 7], switch: 0x1710, sub_enable: None, sub_ldo26: false },
];

/// The front end's own CSIS core, configured by ferros alone: every write Linux makes into G#1C32_xxxx, in its order, and nothing else.
/// The raw frame path is the CSI link (G#1C05_xxxx) into this core and out through its write DMA, and both are CPU-writable — the core held 100% of what the readback test wrote. The PDMA engine and the region at G#1C36_xxxx that only it can reach serve the ISP's *processing* blocks downstream (bayer, statistics, LUTs). So a raw dump should not need the vendor's 4495-write program at all, and running without it says whether those downstream blocks were what held the frame up.
/// Read out of the capture's CPU writes and the decoded PDMA list (tools/camera-prog.py's CPR2 sections), with the three buffer addresses moved to ferros's own 16 MiB slices. The words worth knowing: +G#474 = G#0C000FF0 sizes the image channel to 4080 x 3072, link channel 0; +G#2074 = G#03000FF0 sizes the PD channel to 4080 x 768, link channel 1; and +G#48C / +G#208C differ only in bit 12, which is which link channel each one takes.
const CORE_ONLY: [(usize, u32); 64] = [
    (0x1C320008, 0xF),
    (0x1C320010, 0x1),
    (0x1C320094, 0x3FFFFFFF),
    (0x1C32011C, 0x1),
    (0x1C322154, 0x1400),
    (0x1C322150, 0x4),
    (0x1C322140, 0x1),
    (0x1C3221C0, 0x88100000),  // our buffer
    (0x1C3221C4, 0x0),
    (0x1C322108, 0x8F),
    (0x1C32210C, 0x0),
    (0x1C322110, 0x89100000),  // our buffer
    (0x1C322114, 0x0),
    (0x1C320554, 0x1400),
    (0x1C320550, 0x4),
    (0x1C320544, 0x0),
    (0x1C320540, 0x1),
    (0x1C3205C0, 0x2000000),
    (0x1C3205C4, 0x0),
    (0x1C320508, 0x8F),
    (0x1C32050C, 0x0),
    (0x1C320510, 0x8A100000),  // our buffer
    (0x1C320514, 0x0),
    (0x1C32041C, 0x1),
    (0x1C32201C, 0x1),
    (0x1C322010, 0x7),
    (0x1C322030, 0x12),
    (0x1C322044, 0x3FFF),
    (0x1C322058, 0x3),
    (0x1C320410, 0x7),
    (0x1C320430, 0x12),
    (0x1C320444, 0x3FFF),
    (0x1C320458, 0x3),
    (0x1C320418, 0x1),
    (0x1C3200B0, 0x1),
    (0x1C320478, 0x14C1E38),
    (0x1C32047C, 0x180),
    (0x1C320460, 0x1),
    (0x1C320464, 0x1),
    (0x1C32046C, 0x1),
    (0x1C320470, 0x1),
    (0x1C32048C, 0x1000200),
    (0x1C320488, 0x0),
    (0x1C320474, 0xC000FF0),
    (0x1C32042C, 0x1B),
    (0x1C322018, 0x1),
    (0x1C3200B0, 0x80),
    (0x1C322078, 0x14C1E38),
    (0x1C32207C, 0x180),
    (0x1C322060, 0x1),
    (0x1C322064, 0x1),
    (0x1C32206C, 0x1),
    (0x1C322070, 0x1),
    (0x1C32208C, 0x1001200),
    (0x1C322088, 0x0),
    (0x1C322074, 0x3000FF0),
    (0x1C32202C, 0x1B),
    (0x1C329004, 0x0),
    (0x1C329000, 0x1),
    (0x1C329008, 0x0),
    (0x1C328804, 0x0),
    (0x1C328800, 0x1),
    (0x1C320430, 0x1A),
    (0x1C322030, 0x1A),
];

/// `diag cam raw`: a raw frame dump built from the ground up — the CSI link and the front end's CSIS core, configured by ferros, and nothing else.
/// No PDMA program, no G#1C33/G#1C35/G#1C36 processing blocks: only CORE_ONLY's 64 writes into the core, with RX_START for the link between them as Linux orders it, then the sensor streaming. If the core's local channel starts a frame here, the vendor pipeline downstream was what held it; if it does not, the core genuinely depends on the processing blocks. Either way it is the smallest configuration that has been tried, and it is ours.
/// Per-frame reads stay on the link and core pages: a read into the front end's other pages mid-stream has hung the core before.
pub fn raw_probe(rep: &mut Vec<u8>, init: Option<Vec<u8>>) {
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let Some(init) = init else {
        return line(rep, alloc::format!("refusing: no sensor init table\n"));
    };
    fe_up(rep);
    if !fe_powered() {
        return;
    }
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed\n"));
    if fail.is_some() {
        return line(rep, alloc::format!("sensor init failed\n"));
    }
    let iso = 0x1546_3EBCusize;
    let _ = crate::platform::priv_reg(iso as u64, true, rd(iso) | 1);
    ispfe_dma_open(rep);
    // Our three slices, patterned so a write anywhere shows.
    const SLICES: [usize; 3] = [0x8810_0000, 0x8910_0000, 0x8A10_0000];
    for &base in SLICES.iter() {
        crate::platform::pet_watchdogs();
        for o in (0..PDMA_BUF_SLICE).step_by(256) {
            wr(base + o, PATTERN);
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    // The core in Linux's order: its setup, then the link, then the channel configuration that follows.
    for &(a, v) in CORE_ONLY[..25].iter() {
        wr(a, v);
    }
    for &(a, v) in crate::camera_rx::RX_START.iter() {
        wr(a, v);
    }
    for &(a, v) in CORE_ONLY[25..].iter() {
        wr(a, v);
    }
    line(rep, alloc::format!("core configured by ferros alone: A#{} writes, no PDMA, no processing blocks\n", CORE_ONLY.len()));
    let son = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    // Only the core's own status words, read then written back as Linux acknowledges them.
    const CORE_ACK: [usize; 10] = [0x1C32_008C, 0x1C32_2004, 0x1C32_2024, 0x1C32_2038, 0x1C32_204C, 0x1C32_0404, 0x1C32_0424, 0x1C32_0438, 0x1C32_044C, 0x1C32_0110];
    let mut series = alloc::string::String::new();
    for f in 1..=10u32 {
        spin_us(50_000);
        crate::platform::pet_watchdogs();
        let (fs, fe, err) = (rd(CSIS_LINK1 + 0x24), rd(CSIS_LINK1 + 0x2C), rd(CSIS_LINK1 + 0x14));
        let (cmn, img, pd) = (rd(0x1C32_008C), rd(0x1C32_0424), rd(0x1C32_2024));
        series.push_str(&alloc::format!("  frame A#{f}: link start G#{fs:X} end G#{fe:X} err G#{err:X} | core cmn G#{cmn:X} img G#{img:X} pd G#{pd:X}\n"));
        for &a in CORE_ACK.iter() {
            let v = rd(a);
            wr(a, v);
        }
    }
    let soff = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    line(rep, series);
    line(rep, alloc::format!("stream on {:?} off {:?}; image channel holds G#{:X}, PD channel G#{:X}\n", son.is_ok(), soff.is_ok(), rd(0x1C32_0510), rd(0x1C32_2110)));
    for &(a, v) in crate::camera_rx::RX_STOP.iter() {
        wr(a, v);
    }
    let mut any = false;
    for (n, &base) in SLICES.iter().enumerate() {
        crate::platform::pet_watchdogs();
        let (mut moved, mut zeroed, mut first, mut last) = (0usize, 0usize, None, 0usize);
        for o in (0..PDMA_BUF_SLICE).step_by(256) {
            let v = rd(base + o);
            if v != PATTERN {
                moved += 1;
                last = o;
                if v == 0 { zeroed += 1 }
                first.get_or_insert(o);
            }
        }
        if let Some(f) = first {
            any = true;
            line(rep, alloc::format!("slice A#{n} at G#{base:X}: A#{moved} probe points written (A#{zeroed} to zero), +G#{f:X} to +G#{last:X}; at +G#{f:X}: G#{:08X} G#{:08X} G#{:08X} G#{:08X}\n", rd(base + f), rd(base + f + 4), rd(base + f + 8), rd(base + f + 12)));
        }
    }
    if !any {
        line(rep, alloc::format!("nothing written to any slice\n"));
    }
}

/// `diag cam lochs`: is the link feeding a local channel other than the one we are watching?
/// The core has seven local channels with image and phase-detect paths each — loch0 to loch6, both paths at a G#400 stride from +G#20424 and +G#22024 (camera_events.rs). The aggregate word's bits run image paths 0-6 then PD paths from 7, which is why it has always read G#81: loch0's image and PD paths, the two the replay configures.
/// So this arms every path's mask, clears every status, streams without acknowledging anything — so a frame start on any channel stays latched — and reads all fourteen once the stream has stopped. Reading after the stop, not during, keeps clear of the timing-dependent hang a mid-stream read into this block has caused.
pub fn lochs_probe(rep: &mut Vec<u8>, init: Option<Vec<u8>>) {
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let Some(init) = init else {
        return line(rep, alloc::format!("refusing: no sensor init table\n"));
    };
    fe_up(rep);
    if !fe_powered() {
        return;
    }
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed\n"));
    if fail.is_some() {
        return line(rep, alloc::format!("sensor init failed\n"));
    }
    let iso = 0x1546_3EBCusize;
    let _ = crate::platform::priv_reg(iso as u64, true, rd(iso) | 1);
    ispfe_dma_open(rep);
    for &(a, v) in CORE_ONLY[..25].iter() {
        wr(a, v);
    }
    for &(a, v) in crate::camera_rx::RX_START.iter() {
        wr(a, v);
    }
    for &(a, v) in CORE_ONLY[25..].iter() {
        wr(a, v);
    }
    // Every path's status word: image paths from +G#20424, phase-detect paths from +G#22024, G#400 apart. Mask at +G#C.
    let path = |n: usize, pd: bool| 0x1C30_0000 + if pd { 0x22024 } else { 0x20424 } + n * 0x400;
    for n in 0..7 {
        for pd in [false, true] {
            let a = path(n, pd);
            // Every event enabled — SOF, EOF, EOL, SHADOW_UPDATE, DMA_DONE, FRAME_DONE — then the status cleared, before anything streams.
            wr(a + 0xC, 0x3F);
            let v = rd(a);
            wr(a, v);
        }
    }
    let cmn = 0x1C32_008C;
    let v = rd(cmn);
    wr(cmn, v);
    line(rep, alloc::format!("all fourteen paths armed and cleared; aggregate now G#{:X}\n", rd(cmn)));
    let son = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    // Ten frames' worth, touching nothing in the core.
    for _ in 0..10 {
        spin_us(50_000);
        crate::platform::pet_watchdogs();
    }
    let link = (rd(CSIS_LINK1 + 0x24), rd(CSIS_LINK1 + 0x2C), rd(CSIS_LINK1 + 0x14));
    let soff = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    spin_us(50_000);
    line(rep, alloc::format!("streamed {:?}/{:?}; link saw frame start G#{:X}, end G#{:X}, errors G#{:X}\n", son.is_ok(), soff.is_ok(), link.0, link.1, link.2));
    const NAMES: [&str; 6] = ["SOF", "EOF", "EOL", "SHADOW_UPDATE", "DMA_DONE", "FRAME_DONE"];
    let mut any_frame = false;
    for n in 0..7 {
        for pd in [false, true] {
            let a = path(n, pd);
            crate::flashlog::line(&alloc::format!("cam lochs: reading G#{a:X}"));
            let st = rd(a);
            let mut bits = alloc::string::String::new();
            for b in 0..6 {
                if st >> b & 1 == 1 {
                    if !bits.is_empty() { bits.push('|'); }
                    bits.push_str(NAMES[b]);
                }
            }
            if st & 0b110111 != 0 {
                any_frame = true;
            }
            line(rep, alloc::format!("  loch{n} {:5} G#{a:X}: G#{st:X} {}\n", if pd { "pd" } else { "image" }, if bits.is_empty() { alloc::string::String::from("-") } else { bits }));
        }
    }
    line(rep, alloc::format!("aggregate after the run: G#{:X}\n", rd(cmn)));
    line(rep, alloc::format!("{}\n", if any_frame { "A FRAME EVENT LANDED on at least one path above" } else { "no path saw any frame event: the link's output is not reaching the core at all" }));
    for &(a, v) in crate::camera_rx::RX_STOP.iter() {
        wr(a, v);
    }
}

/// Translate one IOVA through the front end's SYSMMU page table rooted at `flpt`, as the hv's PDMA dump does (samsung-iommu v9): a first-level entry per MiB, type 2 a 1 MiB section, type 1 a pointer to a second level whose entries are 64 KiB (type 1) or 4 KiB (type 2) pages. Physical addresses are stored shifted right by four. Every step must stay in DRAM: a read into a hole or a register block can hang the core.
fn sysmmu_translate(flpt: usize, iova: usize) -> Option<usize> {
    let dram = |pa: usize| (0x8000_0000..0xC_0000_0000).contains(&pa);
    let a1 = flpt + (iova >> 20) * 4;
    if !dram(a1) {
        return None;
    }
    let e1 = rd(a1) as usize;
    let pa = match e1 & 7 {
        2 => ((e1 & !0xFFFF) << 4) | (iova & 0xF_FFFF),
        1 => {
            let a2 = ((e1 & !0x3F) << 4) + ((iova & 0xF_FFFF) >> 12) * 4;
            if !dram(a2) {
                return None;
            }
            let e2 = rd(a2) as usize;
            match e2 & 3 {
                1 => ((e2 & !0xFFF) << 4) | (iova & 0xFFFF),
                2 => ((e2 & !0xFF) << 4) | (iova & 0xFFF),
                _ => return None,
            }
        }
        _ => return None,
    };
    if dram(pa) { Some(pa) } else { None }
}

/// `diag cam iova`: every buffer the camera's IOMMU maps, found by walking the page table itself.
/// The core's three buffer registers turned out to hold metadata, and a 4080 x 768 frame sitting at the sensor's black level even with the camera aimed at a bright scene — link channel 1's geometry, the phase-detect stream, not the picture. The picture the camera app shows is a processed output, and the program writes a cluster of buffer addresses into G#1C35 that have never been read. Reading those registers mid-stream has hung the core, so this does not touch them: the page table is in DRAM, and walking it is only memory reads.
/// It lists every contiguous mapped IOVA run of at least 256 KiB with its size and the mean and maximum of a sample of its bytes. A processed image is large and bright; metadata is small; a raw black-level stream is large and dark.
pub fn iova_scan(rep: &mut Vec<u8>) {
    const MMU: usize = 0x1C11_0000;
    let flpt = ((rd(MMU + 0x8404) & 0xFF_FFFF) as usize) << 12;
    if flpt == 0 {
        return line(rep, alloc::format!("SYSMMU context 0 has no table\n"));
    }
    line(rep, alloc::format!("page table at G#{flpt:X}; IOVA runs of 256 KiB and up:\n"));
    let dram = |pa: usize| (0x8000_0000..0xC_0000_0000).contains(&pa);
    let mut run_start: Option<usize> = None;
    let mut run_pages = 0usize;
    let mut found = 0u32;
    let mut report = |rep: &mut Vec<u8>, start: usize, pages: usize| {
        if pages * 4 < 256 {
            return;
        }
        // A sample of 256 bytes spread evenly across the run.
        let (mut sum, mut max, mut n) = (0u64, 0u32, 0u64);
        for k in 0..256usize {
            let va = start + (pages * 0x1000 / 256) * k;
            if let Some(pa) = sysmmu_translate(flpt, va) {
                let b = unsafe { core::ptr::read_volatile(pa as *const u8) } as u32;
                sum += b as u64;
                max = max.max(b);
                n += 1;
            }
        }
        line(rep, alloc::format!("  IOVA G#{start:X}: A#{} KiB, sampled mean A#{} max A#{max}\n", pages * 4, if n > 0 { sum / n } else { 0 }));
        found += 1;
    };
    for l1 in 0..4096usize {
        if l1 % 256 == 0 {
            crate::platform::pet_watchdogs();
        }
        let a1 = flpt + l1 * 4;
        if !dram(a1) {
            continue;
        }
        let e1 = rd(a1) as usize;
        // Each first-level entry covers one MiB: 256 small pages. Count which of them map, in order.
        let mapped: [bool; 256] = match e1 & 7 {
            2 => [true; 256],
            1 => {
                let base = (e1 & !0x3F) << 4;
                let mut m = [false; 256];
                if dram(base) {
                    for (i, slot) in m.iter_mut().enumerate() {
                        let e2 = rd(base + i * 4) as usize;
                        *slot = matches!(e2 & 3, 1 | 2);
                    }
                }
                m
            }
            _ => [false; 256],
        };
        for (i, &on) in mapped.iter().enumerate() {
            let va = (l1 << 20) + i * 0x1000;
            if on {
                if run_start.is_none() {
                    run_start = Some(va);
                    run_pages = 0;
                }
                run_pages += 1;
            } else if let Some(st) = run_start.take() {
                report(rep, st, run_pages);
            }
        }
    }
    if let Some(st) = run_start {
        report(rep, st, run_pages);
    }
    line(rep, alloc::format!("A#{found} runs of 256 KiB or more\n"));
}

/// `diag cam preview`: the camera app's own preview frame, painted 1:1.
/// Walking the IOMMU table after a live handoff finds a ring of buffers of exactly 497 pages — 1988 KiB — with a well-exposed sampled mean around 91. That size is 1344 x 1008 NV12 exactly: 1,354,752 bytes of luma and 677,376 of interleaved chroma, 2,032,128 bytes in all. 1344 is the panel's own width, so this is the preview the camera app renders for this display, and its luma plane is a finished greyscale picture that lands on the panel pixel for pixel, with nothing to demosaic or scale.
/// The first such buffer in the table is painted; an 84 x 32 character rendering of it comes back in the report so the scene can be checked without looking at the phone.
pub fn preview(rep: &mut Vec<u8>) {
    const MMU: usize = 0x1C11_0000;
    const W: usize = 1344;
    const H: usize = 1008;
    const PAGES: usize = 497;
    let flpt = ((rd(MMU + 0x8404) & 0xFF_FFFF) as usize) << 12;
    if flpt == 0 {
        return line(rep, alloc::format!("SYSMMU context 0 has no table\n"));
    }
    // The first run of exactly PAGES mapped pages.
    let mut iova = 0usize;
    let mut run = 0usize;
    let mut start = 0usize;
    'scan: for va in (0..0x1_0000_0000usize).step_by(0x1000) {
        if va % 0x400_0000 == 0 {
            crate::platform::pet_watchdogs();
        }
        if sysmmu_translate(flpt, va).is_some() {
            if run == 0 {
                start = va;
            }
            run += 1;
        } else {
            if run == PAGES {
                iova = start;
                break 'scan;
            }
            run = 0;
        }
    }
    if iova == 0 {
        return line(rep, alloc::format!("no 497-page preview buffer mapped\n"));
    }
    line(rep, alloc::format!("preview buffer at IOVA G#{iova:X}, 1344 x 1008 NV12; painting its luma plane\n"));
    let mut page = (usize::MAX, 0usize);
    let mut luma = |x: usize, y: usize| -> u8 {
        let va = iova + y * W + x;
        let vp = va & !0xFFF;
        if page.0 != vp {
            page = (vp, sysmmu_translate(flpt, vp).unwrap_or(0));
        }
        if page.1 == 0 { 0 } else { unsafe { core::ptr::read_volatile((page.1 + (va & 0xFFF)) as *const u8) } }
    };
    // The 84 x 32 rendering first, so the scene is in the report even if the panel will not take it.
    const RAMP: &[u8] = b" .:-=+*#%@";
    let mut art = alloc::string::String::from("preview, 84 x 32:\n");
    for ay in 0..32 {
        art.push_str("  ");
        for ax in 0..84 {
            let v = luma(ax * W / 84, ay * H / 32) as usize;
            art.push(RAMP[v * (RAMP.len() - 1) / 255] as char);
        }
        art.push('\n');
    }
    line(rep, art);
    let freq = crate::platform::cntfrq();
    let mut scratch = Vec::new();
    let Some(p) = crate::display::adopt(freq, &mut scratch) else {
        line(rep, alloc::format!("no panel to paint on; adoption said:\n"));
        rep.extend_from_slice(&scratch);
        return;
    };
    let (w, h, pstride, fb) = (p.width, p.rows, p.stride, p.fb);
    for y in 0..h {
        if y % 256 == 0 {
            crate::platform::pet_watchdogs();
        }
        for x in 0..w {
            let v = if x < W && y < H { luma(x, y) as u32 } else { 0 };
            unsafe { core::ptr::write_volatile((fb + (y * pstride + x) * 4) as *mut u32, v << 16 | v << 8 | v) };
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    p.present(&mut scratch);
    line(rep, alloc::format!("painted the preview's luma 1:1 at the top of the panel ({w} x {h})\n"));
}

/// `diag cam inherit`: put a real camera frame on the panel, by taking over a pipeline Linux already has running.
/// Run after a live handoff (tools/husky-hotload.sh --live) with the camera app open: Linux hands the CPUs over without shutting down, so the sensor, the link, the front end and its write DMA are all still configured the way the working HAL left them, and the last frames are still sitting in Linux's buffers.
/// The front end's image channel holds its current buffer as an IOVA (+G#510 low, +G#514 high); the SYSMMU in front of it maps that to physical pages, which are not contiguous, so each 4 KiB page is translated as it is reached. The frame is RAW10 packed four pixels to five bytes, 4080 x 3072 at a G#1400 stride, so each pixel's eight most significant bits sit at byte `(x / 4) * 5 + x % 4` of its line.
/// The preview averages each 2 x 2 Bayer quad into one grey level and takes every fourth quad, which gives a 1020 x 768 picture of the scene, painted centred at the top of the panel.
pub fn inherit_probe(rep: &mut Vec<u8>) {
    if !fe_powered() {
        return line(rep, alloc::format!("ISP front end is not powered: hand off from Android with the camera app open\n"));
    }
    crate::flashlog::line("cam inherit: reading the image channel's buffer");
    // The core holds three buffer addresses. The image channel's is not a frame: at a handoff it points at an 8 KiB structure of metadata (first words G#EA, G#01, G#E9, G#BB9597B2), which is what painted as garbage lines on the first try. The frame-sized one is whichever maps the most pages, so the preview takes that — on 2026-10-21 the PD side, 961 pages, 769 lines at the G#1400 stride: 4080 x 768 RAW10, link channel 1's geometry.
    let stride = rd(0x1C32_0554) as usize;
    let candidates = [("image channel", 0x1C32_0510usize), ("PD channel", 0x1C32_2110), ("PD side", 0x1C32_21C0)];
    let mut iova = 0usize;
    let mut chosen = "";
    let mut best = 0usize;
    let mmu_probe = 0x1C11_0000usize;
    let probe_flpt = ((rd(mmu_probe + 0x8404) & 0xFF_FFFF) as usize) << 12;
    for (name, lo) in candidates {
        let v = (rd(lo) as usize) | ((rd(lo + 4) as usize) << 32);
        let mut n = 0usize;
        while v != 0 && probe_flpt != 0 && n < 8192 && sysmmu_translate(probe_flpt, (v & !0xFFF) + n * 0x1000).is_some() {
            n += 1;
        }
        line(rep, alloc::format!("{name}: IOVA G#{v:X}, A#{n} pages mapped\n"));
        if n > best {
            best = n;
            iova = v;
            chosen = name;
        }
    }
    line(rep, alloc::format!("taking the {chosen} buffer, the largest: G#{iova:X}, stride G#{stride:X}\n"));
    if iova == 0 {
        return line(rep, alloc::format!("no buffer programmed: the camera was not streaming at the handoff\n"));
    }
    // Find the SYSMMU context whose table maps it. Only s0 is safe to read; s1 hung the core once.
    const MMU: usize = 0x1C11_0000;
    crate::flashlog::line("cam inherit: reading the SYSMMU");
    let nvm = ((rd(MMU + 0x100) & 0x1F) as usize).clamp(1, 8);
    let mut flpt = 0usize;
    for vid in 0..nvm {
        let ctrl = rd(MMU + 0x8000 + vid * 0x1000);
        let base = (rd(MMU + 0x8404 + vid * 0x1000) & 0xFF_FFFF) as usize;
        let t = base << 12;
        line(rep, alloc::format!("  SYSMMU context A#{vid}: ctrl G#{ctrl:X} table G#{t:X}\n"));
        if ctrl & 1 == 1 && base != 0 && sysmmu_translate(t, iova).is_some() {
            flpt = t;
            break;
        }
    }
    if flpt == 0 {
        return line(rep, alloc::format!("no SYSMMU context maps the buffer\n"));
    }
    let stride = if stride == 0 { 0x1400 } else { stride };
    // The first page translated on its own, so it can be reported before the cache below takes the table over.
    let Some(first_pa) = sysmmu_translate(flpt, iova & !0xFFF) else {
        return line(rep, alloc::format!("the buffer's first page does not translate\n"));
    };
    line(rep, alloc::format!("buffer's first page maps to physical G#{first_pa:X}\n"));
    // How far the buffer is actually mapped. The core is configured for a 4080 x 3072 frame, but the first run read only the top ~380 rows before the translation ran out, so the real extent is measured rather than assumed: the first unmapped 4 KiB page, and the size it implies in lines.
    let mut mapped = 0usize;
    while mapped < 8192 && sysmmu_translate(flpt, (iova & !0xFFF) + mapped * 0x1000).is_some() {
        mapped += 1;
    }
    let lines_mapped = mapped * 0x1000 / stride.max(1);
    line(rep, alloc::format!("buffer mapped for A#{mapped} contiguous pages (A#{} KiB), A#{lines_mapped} lines at this stride\n", mapped * 4));
    // The raw bytes, before any interpretation: the first 128, then 32 at the start of lines 1, 2, 16 and 64. Packed RAW10 shows a fifth-byte rhythm (every fifth byte holds four pixels' two low bits); a compressed buffer shows a header and no such period; a wrong stride shows the same content drifting between lines.
    let hex_at = |off: usize, len: usize| -> alloc::string::String {
        let mut out = alloc::string::String::new();
        for i in 0..len {
            let va = (iova & !0xFFF) + off + i;
            let b = sysmmu_translate(flpt, va).map(|pa| unsafe { core::ptr::read_volatile(pa as *const u8) });
            out.push_str(&match b { Some(b) => alloc::format!("{b:02X}"), None => alloc::string::String::from("--") });
            if i % 4 == 3 { out.push(' '); }
        }
        out
    };
    line(rep, alloc::format!("first 128 bytes:\n  {}\n  {}\n  {}\n  {}\n", hex_at(0, 32), hex_at(32, 32), hex_at(64, 32), hex_at(96, 32)));
    for ln in [1usize, 2, 16, 64] {
        line(rep, alloc::format!("line A#{ln} (+G#{:X}): {}\n", ln * stride, hex_at(ln * stride, 32)));
    }
    // A one-page translation cache: lines run along IOVA pages, so most reads reuse the last one.
    let mut page = (iova & !0xFFF, first_pa);
    let mut byte = |off: usize| -> Option<u8> {
        let va = iova + off;
        let vp = va & !0xFFF;
        if page.0 != vp {
            page = (vp, sysmmu_translate(flpt, vp)?);
        }
        let pa = page.1 + (va & 0xFFF);
        Some(unsafe { core::ptr::read_volatile(pa as *const u8) })
    };
    // The preview: 1020 x 768 grey, each sample the mean of one Bayer quad's four MSBs.
    const OW: usize = 1020;
    // Every fourth source line, stopping at the last one the mapping holds.
    let oh = (lines_mapped / 4).clamp(1, 768);
    let mut grey = alloc::vec![0u8; OW * oh];
    let (mut lo, mut hi, mut sum, mut missing) = (255u32, 0u32, 0u64, 0u32);
    for oy in 0..oh {
        if oy % 64 == 0 {
            crate::platform::pet_watchdogs();
        }
        let sy = oy * 4;
        for ox in 0..OW {
            let sx = ox * 4;
            let col = |x: usize| (x / 4) * 5 + x % 4;
            let q = [byte(sy * stride + col(sx)), byte(sy * stride + col(sx + 1)), byte((sy + 1) * stride + col(sx)), byte((sy + 1) * stride + col(sx + 1))];
            let mut t = 0u32;
            let mut ok = true;
            for v in q.iter() {
                match v { Some(b) => t += *b as u32, None => ok = false }
            }
            if !ok {
                missing += 1;
                continue;
            }
            let g = t / 4;
            grey[oy * OW + ox] = g as u8;
            lo = lo.min(g);
            hi = hi.max(g);
            sum += g as u64;
        }
    }
    let n = (OW * oh) as u64 - missing as u64;
    line(rep, alloc::format!("frame read: A#{n} samples, grey A#{lo}..A#{hi}, mean A#{}, A#{missing} unmapped\n", if n > 0 { sum / n } else { 0 }));
    // Stretch whatever range the scene has to the full scale, so a dim room still reads.
    let span = (hi.saturating_sub(lo)).max(1);
    let freq = crate::platform::cntfrq();
    let mut scratch = Vec::new();
    let Some(p) = crate::display::adopt(freq, &mut scratch) else {
        line(rep, alloc::format!("no panel to paint on; adoption said:\n"));
        rep.extend_from_slice(&scratch);
        return;
    };
    let (w, h, pstride, fb) = (p.width, p.rows, p.stride, p.fb);
    let x0 = (w.saturating_sub(OW)) / 2;
    for y in 0..h {
        for x in 0..w {
            unsafe { core::ptr::write_volatile((fb + (y * pstride + x) * 4) as *mut u32, 0) };
        }
    }
    for oy in 0..oh.min(h) {
        for ox in 0..OW.min(w) {
            let g = ((grey[oy * OW + ox] as u32).saturating_sub(lo) * 255 / span).min(255);
            let v = g << 16 | g << 8 | g;
            unsafe { core::ptr::write_volatile((fb + (oy * pstride + x0 + ox) * 4) as *mut u32, v) };
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    p.present(&mut scratch);
    line(rep, alloc::format!("painted 1020 x A#{oh} at the top of the panel, contrast stretched from A#{lo}..A#{hi}\n"));
}

/// The four sensor master clocks as CMU_TOP has them, read only.
/// All four read zero, and that is their correct state, not a misread: `camera-main-powerup-cmu.txt` shows Linux reading these same words as zero and writing zero back, so the mux selects source 0 and the divider divides by one. The only live traffic in that capture is at G#2604_3004, G#2604_300C and G#2604_3010, each written 2 and then 0 — a transient access request around the register reads, not a persistent enable.
/// So the master clocks need nothing from us beyond muxing each module's pin, and the two modules that stay silent are not silent for want of a clock configured differently from the two that answer.
fn clocks_report(rep: &mut Vec<u8>) {
    const CMU_TOP: usize = 0x2604_0000;
    for n in 0..4usize {
        let (mux, div) = (CMU_TOP + 0x1010 + n * 4, CMU_TOP + 0x1810 + n * 4);
        line(rep, alloc::format!("CIS_CLK{n}: mux G#{:08X} divider G#{:08X}\n", rd(mux), rd(div)));
    }
}

/// Power every camera module down, whichever one was up.
/// `power_down` reverses the main camera's own sequence and knows only its four rails, so after a module that uses any of the others it leaves them enabled — which both spoils the next module's clean start and makes the next `cam up` refuse, since it checks that the matrix is clear (seen 2026-10-21: MATRIX_CONF_A read G#66, the ultrawide's and the front camera's rails still on).
/// This clears all four reset lines, all four load switches, every rail in the enable matrix, both sub-PMIC rails and all four master-clock pins, so the chip is back to the state a cold boot leaves it in bar the programmed voltages.
pub fn power_down_all(rep: &mut Vec<u8>) {
    for m in MODULES.iter() {
        pin_out(m.reset_bank, m.reset_pin, false);
        if let Ok(v) = pmic_read(m.switch) {
            let _ = pmic_write(m.switch, v & !1);
        }
    }
    let _ = pmic_write(MATRIX_A, 0);
    let _ = acpm::update(acpm::SUB, acpm::T_PMIC, 0x3A, 0, 0x80);
    let _ = acpm::update(acpm::SUB, acpm::T_PMIC, 0x48, 0, 0x80);
    for m in MODULES.iter() {
        pin_out(m.mclk_bank, 0, false);
    }
    line(rep, alloc::format!("  all modules down: MATRIX_CONF_A G#{}\n", match pmic_read(MATRIX_A) { Ok(v) => alloc::format!("{v:02X}"), Err(e) => alloc::format!("read err G#{e:X}") }));
}

/// Power one camera module and read its chip identifier, then leave it powered.
/// The sequence is the main camera's, from `module_power_seq@0`, generalised over the module table: its own rails programmed and verified while everything is off, then LDO1, its sub-PMIC enable line if it has one, LDO8, the sub-PMIC rails, its remaining rails, its master-clock pin, its load switch, and finally reset released with the 9 ms the sequence calls for.
/// Only the three rear modules share a sequence; the front one declares none in the device tree, and neither do the two side modules, so LWIS brings those up by enabling their supply list — which is what this does.
fn power_module(rep: &mut Vec<u8>, m: &Module) -> bool {
    let hex = |e: u32| alloc::format!("G#{e:X}");
    let step = |rep: &mut Vec<u8>, label: &str, r: Result<alloc::string::String, alloc::string::String>| -> bool {
        match r {
            Ok(s) => { line(rep, alloc::format!("    {label}: {s}\n")); true }
            Err(s) => { line(rep, alloc::format!("    {label}: FAILED {s}\n")); false }
        }
    };
    // Voltages first, each through the overvoltage guard, while every rail of this module is still off.
    for &i in m.rails {
        let rail = &RAILS[i];
        let r = (|| -> Result<alloc::string::String, alloc::string::String> {
            let mv = check_rail(rail, rail.code)?;
            let before = pmic_read(rail.vsel).map_err(hex)?;
            pmic_write(rail.vsel, rail.code).map_err(hex)?;
            let after = pmic_read(rail.vsel).map_err(hex)?;
            if after != rail.code {
                return Err(alloc::format!("wrote G#{:02X}, reads G#{after:02X}", rail.code));
            }
            Ok(alloc::format!("G#{before:02X} -> G#{after:02X}, A#{mv} mV"))
        })();
        if !step(rep, rail.name, r) {
            return false;
        }
    }
    let ldo = |bit: u32| ldo_on(bit).map(|x| alloc::format!("matrix G#{x:02X}")).map_err(hex);
    let sub_ldo = |reg: u32| acpm::update(acpm::SUB, acpm::T_PMIC, reg, 0x80, 0x80).and_then(|_| acpm::read(acpm::SUB, acpm::T_PMIC, reg)).map(|v| alloc::format!("ctrl G#{v:02X}")).map_err(hex);
    // LDO1 (the shared analogue rail) and LDO8 (the shared core) bracket the module's enable line, as the captured sequence does.
    if !step(rep, "LDO1 on", ldo(0)) { return false }
    spin_us(500);
    if let Some(g) = m.sub_enable {
        if !step(rep, "module enable (sub GPIO)", sub_gpio_high(g as u32).map(|_| alloc::string::String::from("high")).map_err(hex)) { return false }
        spin_us(10);
    }
    if !step(rep, "LDO8 on", ldo(7)) { return false }
    spin_us(750);
    if m.sub_ldo26 && !step(rep, "sub LDO26 on", sub_ldo(0x48)) { return false }
    if !step(rep, "sub LDO12 on", sub_ldo(0x3A)) { return false }
    for &i in m.rails {
        if i != 0 && i != 7 {
            if !step(rep, RAILS[i].name, ldo(RAILS[i].bit)) { return false }
            spin_us(1000);
        }
    }
    // The master clock out on this module's pin: drive and no pull as Linux runs it, then function 2. The CIS clocks themselves already run on a cold boot.
    let mc = GPIO_PERIC0 + m.mclk_bank;
    pin_field(mc + 0xC, 0, 2);
    pin_field(mc + 0x8, 0, 0);
    pin_field(mc, 0, 2);
    line(rep, alloc::format!("    master clock pin: CON G#{:X} ({})\n", rd(mc), m.clock));
    let sw = pmic_read(m.switch).and_then(|v| pmic_write(m.switch, v | 1).map(|_| v)).map_err(hex).map(|v| alloc::format!("G#{v:02X} -> on"));
    if !step(rep, "load switch", sw) { return false }
    spin_us(1000);
    pin_out(m.reset_bank, m.reset_pin, true);
    line(rep, alloc::format!("    reset released: DAT G#{:X}\n", rd(GPIO_PERIC0 + m.reset_bank + 4)));
    spin_us(9000);
    true
}

/// `diag cam scan`: bring every camera module up in turn and ask it who it is.
/// Each module gets its bus muxed and clocked, its own rails programmed through the overvoltage guard, its power sequence run, and then a read of register G#0000 — the model identifier, which is the one answer that proves the whole chain to that sensor works. Everything is powered back down between modules so only one is ever live.
/// The identifier is two bytes on three of the four; the ultrawide answers eight-bit register values, and without allowing for that it reads as a dead part.
pub fn scan_all(rep: &mut Vec<u8>) {
    // The camera PMIC is on its own bus, number fourteen, which the four sensor buses do not include and every rail write needs.
    let gpp16 = GPIO_PERIC0 + 0x220;
    for pin in 0..2 {
        pin_field(gpp16, pin, 2);
        pin_field(gpp16 + 8, pin, 0);
    }
    wr(USI14_MUX, rd(USI14_MUX) & !(1 << 4));
    crate::hsi2c::bring_up(PMIC_BUS.base, PMIC_BUS.usi_mode, FS_DIV, 0xFF_FFFF);
    line(rep, alloc::format!("camera PMIC bus G#{:X}: ready {}\n", PMIC_BUS.base, crate::hsi2c::ready(PMIC_BUS.base, PMIC_BUS.usi_mode)));
    buses_all(rep);
    clocks_report(rep);
    pmic_power_on(rep);
    // Start from a clear matrix: a previous scan leaves its last module's rails on, and nothing else resets them.
    power_down_all(rep);
    // What each module answered, for the panel at the end.
    let mut found: [alloc::string::String; 4] = [alloc::string::String::new(), alloc::string::String::new(), alloc::string::String::new(), alloc::string::String::new()];
    for (slot, m) in MODULES.iter().enumerate() {
        line(rep, alloc::format!("{} ({}), bus G#{:X} address G#{:02X}:\n", m.name, m.role, m.bus, m.addr));
        if power_module(rep, m) {
            // The controller has to be brought up again now the module is powered: it was configured against lines nothing was holding high, and `sensor_id` does the same before addressing the main camera.
            let usi = SYSREG_PERIC0 + m.usi_off;
            crate::hsi2c::bring_up(m.bus, usi, FS_DIV, 0x2_0000);
            let mut id = [0u8; 2];
            let r = crate::hsi2c::write_read(m.bus, m.addr, &[0, 0], &mut id);
            let ready = crate::hsi2c::ready(m.bus, usi);
            match r {
                Ok(()) => {
                    line(rep, alloc::format!("  ANSWERS: model ID G#{:02X}{:02X} (bus ready {ready})\n", id[0], id[1]));
                    found[slot] = alloc::format!("answers G#{:02X}{:02X}", id[0], id[1]);
                }
                Err(e) => {
                    line(rep, alloc::format!("  silent: G#{e:X} (bus ready {ready})\n"));
                    found[slot] = alloc::format!("silent, G#{e:X}");
                }
            }
            // A part that acknowledges but reads zero, or does not acknowledge at G#0000 at all, may keep its identifier elsewhere: CCS puts a module identifier at G#0000 and a sensor one at G#0016, and the ultrawide answers eight-bit register values where the rest answer sixteen.
            if r.is_err() || id == [0, 0] {
                let mut probe = alloc::string::String::from("  identifier probe:");
                for reg in [0x0000u16, 0x0002, 0x0016, 0x0018, 0x300A] {
                    let mut b = [0u8; 2];
                    let got = crate::hsi2c::write_read(m.bus, m.addr, &[(reg >> 8) as u8, reg as u8], &mut b);
                    probe.push_str(&match got {
                        Ok(()) => alloc::format!(" G#{reg:04X}=G#{:02X}{:02X}", b[0], b[1]),
                        Err(e) => alloc::format!(" G#{reg:04X}=err{e:X}"),
                    });
                }
                probe.push('\n');
                line(rep, probe);
                // A part whose identifier only lives at the CCS sensor register still counts as found.
                let mut b = [0u8; 2];
                if crate::hsi2c::write_read(m.bus, m.addr, &[0x00, 0x16], &mut b).is_ok() && b != [0, 0] {
                    found[slot] = alloc::format!("answers G#{:02X}{:02X} at G#0016", b[0], b[1]);
                }
            }
        }
        power_down_all(rep);
        spin_us(20_000);
    }
    // The same answers on the panel, so the cluster can be read off the phone itself.
    let mut screen = alloc::string::String::from("ferros camera cluster\n\n");
    for (slot, m) in MODULES.iter().enumerate() {
        screen.push_str(&alloc::format!("{:9} {:11} bus G#{:X} addr G#{:02X}\n            {}\n", m.name, m.role, m.bus, m.addr, if found[slot].is_empty() { "not reached" } else { found[slot].as_str() }));
    }
    screen.push_str("\nrails, the vendor's own targets:\n");
    for r in RAILS.iter() {
        screen.push_str(&alloc::format!("  {} code G#{:02X}, refused above A#{} mV\n", r.name, r.code, r.target_mv + r.target_mv / 10));
    }
    let shown = crate::display::show(screen.as_bytes(), crate::platform::cntfrq());
    line(rep, alloc::format!("panel painted ({} bytes of adoption report)\n", shown.len()));
}

/// `diag cam buses`: mux and bring up all four camera I2C controllers and report what each says.
/// They are the same IP as the one the main camera already uses, at G#1088/1089/108A/108B_0000, with their buses on gpp2/4/6/8 pins 0 and 1 at function 3 (zuma-pinctrl) and their USI software-configuration words at +0, +4, +8 and +G#C in SYSREG_PERIC0 (zuma-usi.dtsi).
/// Nothing is powered and no device is addressed, so this cannot harm anything: a controller whose clock is not running simply reports not ready, which is the question being asked — whether the other three buses are clocked on a cold boot the way the first one is.
pub fn buses_all(rep: &mut Vec<u8>) {
    for m in MODULES.iter() {
        let bank = GPIO_PERIC0 + m.bus_bank;
        for pin in 0..2 {
            pin_field(bank, pin, 3);
            pin_field(bank + 8, pin, 3);
        }
        let usi = SYSREG_PERIC0 + m.usi_off;
        // USI_CON out of reset, then the controller as the main camera's bus is brought up.
        wr(m.bus + 0xC4, 0);
        crate::hsi2c::bring_up(m.bus, usi, FS_DIV, 0x2_0000);
        let ready = crate::hsi2c::ready(m.bus, usi);
        line(rep, alloc::format!("{:9} {:10} bus G#{:X}: ready {ready:5} USI mode G#{:X} USI_CON G#{:X} CTL G#{:X} CONF G#{:X}; pins CON G#{:X} PUD G#{:X}\n",
            m.name, m.role, m.bus, rd(usi), rd(m.bus + 0xC4), rd(m.bus), rd(m.bus + 0x40), rd(bank), rd(bank + 8)));
    }
}

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

/// `diag cam clamp`: prove the overvoltage guard refuses what it is there to refuse, without touching the chip.
/// Every code is checked through `check_rail` alone — no I2C, no writes — so this is safe to run at any time, and it is the only way to know the guard works: one that has never been seen to fire is not a guard.
/// The dangerous cases are the real ones. The chip's own cold default for LDO1 is G#A5, which is 2.85 V on a rail the module wants at 2.25 V; LDO8's regulator reaches 1.675 V on a 0.97 V digital core, and the device tree permits it.
pub fn clamp_test(rep: &mut Vec<u8>) {
    line(rep, alloc::format!("the overvoltage guard, checked against the codes it must pass and refuse:\n"));
    let (mut pass, mut fail) = (0u32, 0u32);
    for rail in RAILS.iter() {
        // The code ferros uses, the chip's cold default, the rail's own maximum, and a plainly wrong byte.
        for (what, code, want_ok) in [
            ("ferros's own code", rail.code, true),
            ("a tenth under target", ((rail.target_mv - rail.target_mv / 10 - rail.base_mv) / rail.step_mv) as u8, true),
            ("the chip's cold default G#A5", 0xA5, rail.base_mv + 0xA5 * rail.step_mv <= rail.target_mv + rail.target_mv / 10),
            ("the rail's maximum code", 0xFF, false),
            ("a tenth over target", ((rail.target_mv + rail.target_mv / 5 - rail.base_mv) / rail.step_mv) as u8, false),
        ] {
            let got = check_rail(rail, code);
            let ok = got.is_ok() == want_ok;
            if ok { pass += 1 } else { fail += 1 }
            let verdict = match &got {
                Ok(mv) => alloc::format!("allowed at A#{mv} mV"),
                Err(_) => alloc::string::String::from("refused"),
            };
            line(rep, alloc::format!("  {} {:30} G#{code:02X}: {verdict}{}\n", if ok { "ok  " } else { "BAD " }, what, if ok { "" } else { " <- the guard disagrees with what it should do" }));
        }
    }
    line(rep, alloc::format!("A#{pass} checks behaved, A#{fail} did not\n"));
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
pub fn power_up(rep: &mut Vec<u8>) {
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
            return;
        }
        Err(e) => {
            line(rep, alloc::format!("refusing: camera PMIC not answering (G#{e:X})\n"));
            return;
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
            return;
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
            return;
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
        return;
    }
    spin_us(1000);
    let g4 = (|| -> Result<alloc::string::String, alloc::string::String> {
        let v = pmic_read(GPIO4_CTRL).map_err(hex)?;
        pmic_write(GPIO4_CTRL, v | 1).map_err(hex)?;
        Ok(alloc::format!("GPIO4_CTRL G#{:02X}", pmic_read(GPIO4_CTRL).map_err(hex)?))
    })();
    if !step(rep, "GPIO4 switch on", g4) {
        power_down(rep);
        return;
    }
    spin_us(1000);
    // The sensor's reset released (its resume sequence), then 9 ms before the first I2C.
    pin_out(0x40, 2, true);
    spin_us(9000);
    line(rep, alloc::format!("  sensor reset released: gpp2 DAT G#{:X}\n", rd(GPIO_PERIC0 + 0x44)));
}

/// Read the main sensor's model ID (register G#0000, two bytes; Linux reads G#08EF). The sensor bus is brought up again first: its pads only go high once the module's rails are on.
pub fn sensor_id(rep: &mut Vec<u8>) {
    crate::hsi2c::bring_up(SENSOR_BUS.base, SENSOR_BUS.usi_mode, FS_DIV, 0x2_0000);
    line(rep, alloc::format!("sensor bus: ready {} (gpp2 DAT G#{:X})\n", crate::hsi2c::ready(SENSOR_BUS.base, SENSOR_BUS.usi_mode), rd(GPIO_PERIC0 + 0x44)));
    let mut id = [0u8; 2];
    match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x00, 0x00], &mut id) {
        Ok(()) => line(rep, alloc::format!("main sensor (G#3D) model ID: G#{:02X}{:02X}{}\n", id[0], id[1], if id == [0x08, 0xEF] { " — matches Linux" } else { "" })),
        Err(e) => line(rep, alloc::format!("main sensor (G#3D): error G#{e:X} (G#4 no acknowledge)\n")),
    }
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
const PD_ISPFE_CONF: u64 = 0x1546_2480;
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

/// Power the ISP front end and restore its clocks the way a camera open does (oracle capture 2026-10-19): CMU_ISPFE dividers and bus-clock mux, its control word, every Q-channel requesting its clock, and SYSREG_ISPFE's two words. Reads CSI link 1's version only once the domain reports on: a read into a powered-down block can hang the core.
pub fn fe_up(rep: &mut Vec<u8>) {
    let before = rd(PD_ISPFE_STATUS as usize);
    if before & 1 == 0 {
        let conf = rd(PD_ISPFE_CONF as usize);
        let w = crate::platform::priv_reg(PD_ISPFE_CONF, true, conf | 1);
        line(rep, alloc::format!("PD ISPFE configuration G#{conf:X} -> write G#{:X} returned G#{w:X}\n", conf | 1));
        let t = crate::platform::cntpct() + crate::platform::cntfrq() / 100;
        while !fe_powered() && crate::platform::cntpct() < t {}
    }
    let after = rd(PD_ISPFE_STATUS as usize);
    line(rep, alloc::format!("PD ISPFE status G#{before:X} -> G#{after:X}\n"));
    if after & 1 == 0 {
        line(rep, alloc::format!("domain did not come on; nothing else touched\n"));
        return;
    }
    let r = pd_prepare(PD_ISPFE_NEED_SMC);
    line(rep, alloc::format!("prepare call returned G#{r:X}\n"));
    cam_rail(rep);
    ispfe_noc_clock(rep);
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

/// Replay a sensor init table: records `W` len(u16 BE) bytes (an I2C write), `R` n(u16) addr(2) (register read, value discarded), `D` microseconds (u32 BE). The table is generated from an oracle capture and kept in the vault (key `cam.boitata.init`), not in the source: it carries the sensor vendor's settings file. Returns (writes, reads, first failure as (record index, error)).
fn replay_init(t: &[u8]) -> (u32, u32, Option<(u32, u32)>) {
    let (mut i, mut n, mut w, mut r) = (0usize, 0u32, 0u32, 0u32);
    while i < t.len() {
        let k = t[i];
        let res = match k {
            b'W' if i + 3 <= t.len() => {
                let len = u16::from_be_bytes([t[i + 1], t[i + 2]]) as usize;
                let d = &t[i + 3..(i + 3 + len).min(t.len())];
                i += 3 + len;
                w += 1;
                crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, d)
            }
            b'R' if i + 5 <= t.len() => {
                let cnt = u16::from_be_bytes([t[i + 1], t[i + 2]]) as usize;
                let a = [t[i + 3], t[i + 4]];
                i += 5;
                r += 1;
                let mut buf = [0u8; 8];
                crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &a, &mut buf[..cnt.min(8)])
            }
            b'D' if i + 5 <= t.len() => {
                spin_us(u32::from_be_bytes([t[i + 1], t[i + 2], t[i + 3], t[i + 4]]) as u64);
                i += 5;
                Ok(())
            }
            _ => return (w, r, Some((n, 0xBAD))),
        };
        if let Err(e) = res {
            return (w, r, Some((n, e)));
        }
        n += 1;
    }
    (w, r, None)
}

/// Link 1's register words at G#000..G#1FC and G#600..G#6FC (the ranges the camera stack touches), 192 words.
fn link_off(i: usize) -> usize {
    if i < 128 { i * 4 } else { 0x600 + (i - 128) * 4 }
}
fn link_snapshot() -> [u32; 192] {
    let mut s = [0u32; 192];
    for (i, v) in s.iter_mut().enumerate() {
        *v = rd(CSIS_LINK1 + link_off(i));
    }
    s
}

/// The sensor's frame counter (CCS frame_count, register G#0005, one byte; the camera stack reads it): it counts while the sensor streams.
fn sensor_frame_count() -> Option<u8> {
    let mut b = [0u8; 1];
    crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x00, 0x05], &mut b).ok().map(|_| b[0])
}

/// `diag cam slitsweep`: find how narrow a slit this sensor will actually give, and how fast.
/// Nothing states the floor. CCS keeps a minimum output height at G#118A, and on this part that register reads zero along with the rest of the limit block, so the only way to know is to ask the sensor and watch its own frame counter.
/// Two sweeps. The first narrows the window with a generous frame period, to find the smallest height that still streams. The second then shortens the frame period at that height, to find the shortest frame — which is where the rate actually comes from: the pixel clock never moves, so the rate is about `pixel_clock / (frame_length_lines x line_length_pck)`, and at 4080 x 64 with a period of 128 lines that worked out to roughly 958 Mpix/s, the same clock the full mode runs.
/// Only sensor registers are touched, all of them standard CCS geometry and timing, so no rail and no voltage is involved.
pub fn slit_sweep(rep: &mut Vec<u8>, init: Option<Vec<u8>>) {
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let Some(init) = init else {
        return line(rep, alloc::format!("refusing: no sensor init table\n"));
    };
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed\n"));
    if fail.is_some() {
        return line(rep, alloc::format!("sensor init failed\n"));
    }
    let w16 = |a: u16, v: u16| crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[(a >> 8) as u8, a as u8, (v >> 8) as u8, v as u8]);
    // Frames the sensor itself counted over a quarter second: its own counter, so nothing downstream can flatter the number.
    let rate = || -> u32 {
        let t0 = crate::platform::cntpct();
        let window = crate::platform::cntfrq() / 4;
        let mut last = sensor_frame_count();
        let mut seen = 0u32;
        while crate::platform::cntpct() - t0 < window {
            let c = sensor_frame_count();
            if c != last {
                seen += 1;
                last = c;
            }
        }
        seen * 4
    };
    let set = |lines: u32, period: u32| {
        let rows = (lines * 4).min(12288);
        let _ = w16(0x0104, 1);
        let _ = w16(0x0346, 0x0010);
        let _ = w16(0x034A, (0x0010 + rows - 1) as u16);
        let _ = w16(0x034E, lines as u16);
        let _ = w16(0x0340, period as u16);
        let _ = w16(0x0104, 0);
    };
    // The sensor takes exactly one geometry change per stream session: the first lands, every later one reads back correctly and leaves it not streaming (measured 2026-10-21). And geometry set before stream-on never starts at all. So each step gets its own session — captured geometry restored, stream off, stream on, then the one change that is allowed.
    let session = |lines: u32, period: u32| {
        let _ = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
        spin_us(50_000);
        let _ = w16(0x0104, 1);
        let _ = w16(0x0346, 0x0010);
        let _ = w16(0x034A, 0x300F);
        let _ = w16(0x034E, 0x0C00);
        let _ = w16(0x0340, 0x2958);
        let _ = w16(0x0104, 0);
        let _ = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
        spin_us(200_000);
        set(lines, period);
        spin_us(150_000);
    };
    let _ = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    spin_us(200_000);
    line(rep, alloc::format!("captured mode 4080 x 3072: A#{} frames a second\n\nnarrowing the window, one stream session per step, frame period the height plus 64:\n", rate()));
    let mut narrowest = 0u32;
    for lines in [512u32, 256, 128, 64, 32, 16, 8, 4, 2, 1] {
        session(lines, lines + 64);
        let fps = rate();
        let mut back = [0u8; 2];
        let read = crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x03, 0x4E], &mut back).is_ok();
        let got = if read { (back[0] as u32) << 8 | back[1] as u32 } else { 0 };
        line(rep, alloc::format!("  4080 x {:4}: A#{fps:6} a second, y_output_size reads back A#{got}{}\n", lines, if fps == 0 { "  <- stopped streaming" } else { "" }));
        if fps > 0 && got == lines {
            narrowest = lines;
        }
    }
    if narrowest == 0 {
        line(rep, alloc::format!("no window below the captured mode streamed; stopping\n"));
    } else {
        line(rep, alloc::format!("\nshortest window that streams: 4080 x A#{narrowest}. Now shortening the frame period at that height:\n"));
        for period in [narrowest + 64, narrowest + 32, narrowest + 16, narrowest + 8, narrowest + 4, narrowest + 2, narrowest + 1, narrowest] {
            session(narrowest, period);
            let fps = rate();
            let mut back = [0u8; 2];
            let read = crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x03, 0x40], &mut back).is_ok();
            let got = if read { (back[0] as u32) << 8 | back[1] as u32 } else { 0 };
            line(rep, alloc::format!("  period A#{:4} lines: A#{fps:6} a second, frame_length_lines reads back A#{got}{}\n", period, if fps == 0 { "  <- stopped streaming" } else { "" }));
        }
    }
    let _ = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    line(rep, alloc::format!("\nstream off. The sensor needs a cold boot before the next run: a geometry change mid-stream leaves it stuck (2026-10-20).\n"));
}

/// `diag cam slit`: set our own window and frame rate on the sensor, rather than replaying the captured mode.
/// The sensor speaks MIPI CCS, so geometry and timing are standard registers: `y_addr_start`/`y_addr_end` choose the rows read off the array, `y_output_size` the rows sent, and `frame_length_lines` the total frame period. Shrink the window and the frame gets shorter, so the rate goes up — which is both the proof that we drive the sensor rather than echo it, and the shape slit-scan wants: a narrow band read as fast as the sensor will give it.
/// Measured against the captured mode's 4080 x 3072 at about fourteen frames a second. The sensor's own frame counter (G#0005) is the clock: it steps once a frame regardless of what the receiver downstream does, so this stands on its own even while the front end's routing is unsolved.
/// `lines` is the slit height in output rows, binned four to one off the array as the captured mode is.
pub fn slit(rep: &mut Vec<u8>, init: Option<Vec<u8>>, lines: u32) {
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let Some(init) = init else {
        return line(rep, alloc::format!("refusing: no sensor init table\n"));
    };
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed\n"));
    if fail.is_some() {
        return line(rep, alloc::format!("sensor init failed\n"));
    }
    // One write of a 16-bit CCS register, big-endian as the bus is.
    let w16 = |a: u16, v: u16| crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[(a >> 8) as u8, a as u8, (v >> 8) as u8, v as u8]);
    let rate = |rep: &mut Vec<u8>, what: &str| {
        // Count the sensor's own frames over a quarter second.
        let t0 = crate::platform::cntpct();
        let window = crate::platform::cntfrq() / 4;
        let first = sensor_frame_count();
        let mut last = first;
        let mut seen = 0u32;
        while crate::platform::cntpct() - t0 < window {
            let c = sensor_frame_count();
            if c != last {
                seen += 1;
                last = c;
            }
        }
        line(rep, alloc::format!("{what}: A#{seen} frames in 250 ms (counter {:?} to {:?})\n", first, last));
        seen
    };
    let son = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    line(rep, alloc::format!("stream on {:?}; the captured mode is 4080 x 3072\n", son.is_ok()));
    spin_us(200_000);
    let before = rate(rep, "captured mode");
    // The slit. The array rows are binned four to one, so the window is four times the output height; the frame period needs a little more than the rows it carries, and the captured mode's line length is kept so the pixel clock does not move.
    let rows = (lines * 4).min(12288);
    let hold = w16(0x0104, 1);
    let a = w16(0x0346, 0x0010);
    let b = w16(0x034A, (0x0010 + rows - 1) as u16);
    let c = w16(0x034E, lines as u16);
    let d = w16(0x0340, (lines + 64) as u16);
    let rel = w16(0x0104, 0);
    line(rep, alloc::format!("slit of A#{lines} output rows from A#{rows} array rows: hold {:?} y_start {:?} y_end {:?} y_out {:?} frame_len {:?} release {:?}\n", hold.is_ok(), a.is_ok(), b.is_ok(), c.is_ok(), d.is_ok(), rel.is_ok()));
    spin_us(200_000);
    let after = rate(rep, "slit mode");
    // What the sensor says it is doing now, read back from its own registers.
    let mut g = [0u8; 2];
    let rd16 = |a: u16, g: &mut [u8; 2]| -> alloc::string::String {
        match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[(a >> 8) as u8, a as u8], g) {
            Ok(()) => alloc::format!("G#{:X}", (g[0] as u32) << 8 | g[1] as u32),
            Err(e) => alloc::format!("err G#{e:X}"),
        }
    };
    line(rep, alloc::format!("sensor now: y_output_size {} frame_length_lines {} x_output_size {} mode_select {}\n",
        rd16(0x034E, &mut g), rd16(0x0340, &mut g), rd16(0x034C, &mut g), rd16(0x0100, &mut g)));
    let _ = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    if after > before {
        line(rep, alloc::format!("the window is ours: A#{before} frames became A#{after} in the same 250 ms\n"));
    } else {
        line(rep, alloc::format!("the rate did not rise (A#{before} then A#{after}); the sensor either refused the geometry or needs the mode set before stream-on\n"));
    }
}

/// `diag cam preexist`: does the front end hold state that nothing in the replay sets up?
/// The oracle was armed sixty seconds into a boot, on purpose, to skip the boot probe — so any one-time initialisation done before a camera app ever runs was never captured and is not in anything ferros replays. The capture still shows it indirectly: a register Linux **reads as non-zero before it ever writes it** was set up by something earlier.
/// This reads those registers on a cold ferros and reports every one that disagrees. A mismatch is missing setup; a match means the value is either a reset default or something the replay does cover.
/// The read-only version words (every block's +G#4 reads G#5000) and one free-running counter are left out.
pub fn preexisting(rep: &mut Vec<u8>) {
    const EXPECT: [(usize, u32); 45] = [
        (0x1C00701C, 0x4),
        (0x1C007020, 0x4),
        (0x1C007024, 0x4),
        (0x1C007028, 0x5),
        (0x1C00702C, 0x4),
        (0x1C007030, 0x4),
        (0x1C007034, 0x4),
        (0x1C007038, 0x4),
        (0x1C00703C, 0x4),
        (0x1C007040, 0x4),
        (0x1C007044, 0x4),
        (0x1C007078, 0x4),
        (0x1C00707C, 0x4),
        (0x1C007080, 0x4),
        (0x1C007084, 0x4),
        (0x1C007088, 0x4),
        (0x1C00708C, 0x4),
        (0x1C007090, 0x4),
        (0x1C007094, 0x4),
        (0x1C00709C, 0x4),
        (0x1C0070A0, 0x4),
        (0x1C0070A4, 0x4),
        (0x1C0070A8, 0x4),
        (0x1C0070AC, 0x4),
        (0x1C0070B0, 0x4),
        (0x1C0070B4, 0x4),
        (0x1C0070B8, 0x4),
        (0x1C0070BC, 0x4),
        (0x1C0070C0, 0x4),
        (0x1C0070C4, 0x4),
        (0x1C0070CC, 0x4),
        (0x1C0070D0, 0x4),
        (0x1C040000, 0x8010000),
        (0x1C32008C, 0x81),
        (0x1C320124, 0x75800),
        (0x1C320424, 0x8),
        (0x1C322024, 0x8),
        (0x1C330000, 0x3004005),
        (0x1C337300, 0x4),
        (0x1C350000, 0x1003AA),
        (0x1C395180, 0x1),
        (0x1C395280, 0x1),
        (0x1C395380, 0x1),
        (0x1C395480, 0x1),
        (0x1C395580, 0x1),
    ];
    if !fe_powered() {
        return line(rep, alloc::format!("ISP front end is not powered (run `diag cam fe`)\n"));
    }
    let (mut same, mut diff) = (0u32, 0u32);
    for &(a, want) in EXPECT.iter() {
        crate::flashlog::line(&alloc::format!("cam preexist: reading G#{a:X}"));
        let got = rd(a);
        if got == want {
            same += 1;
        } else {
            diff += 1;
            line(rep, alloc::format!("  G#{a:X}: Linux G#{want:X}, ferros G#{got:X}\n"));
        }
    }
    line(rep, alloc::format!("A#{same} of A#{} match what Linux had before its capture began; A#{diff} differ\n", EXPECT.len()));
}

/// `diag cam probe`: is a capability read that disagrees with reality the sensor's answer or ours?
/// Several words in the CCS limit block came back impossible — `max_line_length_pck` 511 against a working mode of 6320, `min_frame_length_lines` 16704 against a mode of 10584 — and a minimum cannot exceed the value in use. Either the sensor does not implement those addresses, or this driver's multi-byte read is returning something else.
/// The test interleaves a register that reads correctly with one that does not, and repeats each: a value that follows whatever was read before it is stale data from the previous transfer, and a value that is the same no matter what preceded it is the sensor's own answer, implemented or not.
pub fn read_probe(rep: &mut Vec<u8>) {
    let r = |a: u16, w: usize| -> alloc::string::String {
        let mut b = [0u8; 4];
        match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[(a >> 8) as u8, a as u8], &mut b[..w]) {
            Ok(()) => {
                let mut v = 0u32;
                for i in 0..w {
                    v = v << 8 | b[i] as u32;
                }
                alloc::format!("G#{v:X}")
            }
            Err(e) => alloc::format!("err G#{e:X}"),
        }
    };
    // G#0000 is known good (the model ID), G#1140 is one of the impossible ones, G#1104 is the four-byte word whose first half G#1140 appeared to echo.
    line(rep, alloc::format!("interleaved, 2 bytes each: G#0000={} G#1140={} G#0000={} G#1140={}\n", r(0x0000, 2), r(0x1140, 2), r(0x0000, 2), r(0x1140, 2)));
    line(rep, alloc::format!("after a different word:    G#1104(4)={} G#1140={} G#0080={} G#1140={}\n", r(0x1104, 4), r(0x1140, 2), r(0x0080, 2), r(0x1140, 2)));
    line(rep, alloc::format!("widths on a known word:    G#0000(1)={} G#0000(2)={} G#0000(4)={}\n", r(0x0000, 1), r(0x0000, 2), r(0x0000, 4)));
    line(rep, alloc::format!("a register the mode uses:  G#0340(2)={} G#0342(2)={} G#034C(2)={} G#0136(2)={}\n", r(0x0340, 2), r(0x0342, 2), r(0x034C, 2), r(0x0136, 2)));
}

/// `diag cam caps`: the sensor's own statement of what it can do, read from the MIPI CCS capability block.
/// The sensor speaks CCS (SMIA++), a public standard: of the 1347 writes in the captured init only twenty define the mode, and those twenty are standard registers. The rest is vendor tuning. So rates, windows and modes are ours to compute rather than replay — and this block is where the silicon says what the vendor allows, instead of us inferring an envelope from one captured mode.
/// Addresses from the mainline CCS driver's generated register map (drivers/media/i2c/ccs/ccs-regs.h). A sensor need not implement all of it; a zero here means "not implemented", which is itself worth knowing.
/// Needs only `diag cam up` — no receiver, no ISP.
pub fn caps_dump(rep: &mut Vec<u8>) {
    // (address, width in bytes, name). Widths are CCS's: the block is big-endian throughout.
    const CAPS: [(u16, u8, &str); 49] = [
        (0x0000, 2, "module_model_id"),
        (0x0002, 1, "module_revision_major"),
        (0x000E, 2, "module_manufacturer_id"),
        (0x0016, 2, "sensor_model_id"),
        (0x0018, 1, "sensor_revision"),
        (0x0040, 1, "frame_format_model_type"),
        (0x0041, 1, "frame_format_model_subtype"),
        (0x0080, 2, "analog_gain_capability"),
        (0x00C0, 1, "data_format_model_type"),
        (0x00C1, 1, "data_format_model_subtype"),
        (0x1000, 2, "integration_time_capability"),
        (0x1004, 2, "coarse_integration_time_min"),
        (0x1006, 2, "coarse_integration_time_max_margin"),
        (0x1008, 2, "fine_integration_time_min"),
        (0x100A, 2, "fine_integration_time_max_margin"),
        (0x1081, 1, "digital_gain_capability"),
        (0x10F0, 1, "adc_capability"),
        (0x10F4, 4, "adc_bit_depth_capability"),
        // The clock tree's limits: what pixel clock, and so what lane rate, the sensor will accept.
        (0x1100, 4, "min_ext_clk_freq_mhz"),
        (0x1104, 4, "max_ext_clk_freq_mhz"),
        (0x1108, 2, "min_pre_pll_clk_div"),
        (0x110A, 2, "max_pre_pll_clk_div"),
        (0x110C, 4, "min_pll_ip_clk_freq_mhz"),
        (0x1110, 4, "max_pll_ip_clk_freq_mhz"),
        (0x1114, 2, "min_pll_multiplier"),
        (0x1116, 2, "max_pll_multiplier"),
        (0x1118, 4, "min_pll_op_clk_freq_mhz"),
        (0x111C, 4, "max_pll_op_clk_freq_mhz"),
        (0x1120, 2, "min_vt_sys_clk_div"),
        (0x1122, 2, "max_vt_sys_clk_div"),
        (0x112C, 4, "min_vt_pix_clk_freq_mhz"),
        (0x1130, 4, "max_vt_pix_clk_freq_mhz"),
        (0x1134, 2, "min_vt_pix_clk_div"),
        (0x1136, 2, "max_vt_pix_clk_div"),
        // Frame timing: the slit-scan knobs. A narrow window is a short frame, and a short frame is a fast one.
        (0x1140, 2, "min_frame_length_lines"),
        (0x1142, 2, "max_frame_length_lines"),
        (0x1144, 2, "min_line_length_pck"),
        (0x1146, 2, "max_line_length_pck"),
        (0x1148, 2, "min_line_blanking_pck"),
        (0x114A, 2, "min_frame_blanking_lines"),
        (0x114C, 1, "min_line_length_pck_step_size"),
        (0x114D, 1, "timing_mode_capability"),
        (0x1188, 2, "min_x_output_size"),
        (0x118A, 2, "min_y_output_size"),
        (0x118C, 2, "max_x_output_size"),
        (0x118E, 2, "max_y_output_size"),
        (0x11C0, 2, "min_even_inc"),
        (0x11C4, 2, "max_even_inc"),
        (0x11C6, 2, "max_odd_inc"),
    ];
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let (mut ok, mut zero, mut err) = (0u32, 0u32, 0u32);
    for &(a, w, name) in CAPS.iter() {
        let mut b = [0u8; 4];
        match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[(a >> 8) as u8, a as u8], &mut b[..w as usize]) {
            Ok(()) => {
                let mut v = 0u32;
                for i in 0..w as usize {
                    v = v << 8 | b[i] as u32;
                }
                if v == 0 {
                    zero += 1;
                } else {
                    ok += 1;
                }
                line(rep, alloc::format!("  G#{a:04X} {name:34} = A#{v} (G#{v:X})\n"));
            }
            Err(e) => {
                err += 1;
                line(rep, alloc::format!("  G#{a:04X} {name:34} = read failed G#{e:X}\n"));
            }
        }
    }
    // The four clock-frequency limits are CCS_FL_FLOAT_IREAL: a 32-bit value is either a plain integer or an IEEE-754 single, chosen per sensor, so the raw word above is the honest thing to print.
    line(rep, alloc::format!("A#{ok} registers answered non-zero, A#{zero} read zero (not implemented), A#{err} failed; the *_freq_mhz words are CCS float-or-integer, read them raw\n"));
}

/// Every front-end and link register Linux reads while its camera streams, with the values it saw (capture lines 1573-12920 of the full ISPFE oracle log). `n` is how many distinct values: one means configuration, more means a status word that moves per frame.
/// Generated from target/hv/ispfe-full-raw.log; the D-PHY page is left out, since its words are set by camera_rx.rs and checked elsewhere.
const LINUX_LIVE: [(usize, u8, [u32; 4]); 66] = [
        (0x1C050004, 2, [0x5000, 0x205E01, 0x205E01, 0x205E01]),
        (0x1C050010, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C050018, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C32008C, 2, [0x1, 0x81, 0x81, 0x81]),
        (0x1C320110, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C320114, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C320124, 4, [0x74000, 0x74800, 0x75000, 0x75800]),
        (0x1C320404, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C320408, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C320424, 4, [0x1, 0x8, 0x12, 0x1A]),
        (0x1C320428, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C320430, 2, [0x12, 0x1A, 0x1A, 0x1A]),
        (0x1C320438, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C32043C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C32044C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C320450, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C32049C, 84, [0x0, 0x1, 0x2, 0x3]),
        (0x1C322004, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C322008, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C322024, 3, [0x8, 0x13, 0x1B, 0x1B]),
        (0x1C322028, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C322030, 1, [0x12, 0x12, 0x12, 0x12]),
        (0x1C322038, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C32203C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C32204C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C322050, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C331300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C331304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C333300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C333304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C335300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C335304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C337300, 1, [0x4, 0x4, 0x4, 0x4]),
        (0x1C337304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33730C, 1, [0x7F8, 0x7F8, 0x7F8, 0x7F8]),
        (0x1C339300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C339304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33B300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33B304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33C300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33C304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33D300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33D304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33E300, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C33E304, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350058, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C35005C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350088, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C35008C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C3500B8, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C3500BC, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C3500E8, 4, [0x1, 0x2, 0x3, 0x10]),
        (0x1C3500EC, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C3500F0, 1, [0x103FFFFE, 0x103FFFFE, 0x103FFFFE, 0x103FFFFE]),
        (0x1C350118, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C35011C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350148, 4, [0x2, 0x8, 0x10, 0x12]),
        (0x1C35014C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350150, 1, [0x3EF, 0x3EF, 0x3EF, 0x3EF]),
        (0x1C350160, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350164, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350178, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C35017C, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350190, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C350194, 1, [0x0, 0x0, 0x0, 0x0]),
        (0x1C3501AC, 1, [0x0, 0x0, 0x0, 0x0]),
];

/// Compare the front end against Linux's own live state, register by register, while ferros's stream runs.
/// The rest of the diffing so far compared writes, or registers read before they were ever written. This compares what Linux *reads back during streaming* — the state a running camera actually holds — against what ferros's running camera holds. A configuration word that differs is a candidate for the one hop that does not work; a status word that moves under Linux and sits still under ferros says which stage is idle.
#[allow(dead_code)]
fn live_compare(rep: &mut Vec<u8>) {
    let (mut same, mut diff, mut idle) = (0u32, 0u32, 0u32);
    for &(a, n, vals) in LINUX_LIVE.iter() {
        crate::flashlog::line(&alloc::format!("cam live: reading G#{a:X}"));
        let got = rd(a);
        let seen = &vals[..n as usize];
        if seen.contains(&got) {
            same += 1;
        } else if n == 1 {
            diff += 1;
            line(rep, alloc::format!("  CONFIG  G#{a:X}: Linux G#{:X}, ferros G#{got:X}  (differs by G#{:X})\n", vals[0], vals[0] ^ got));
        } else {
            idle += 1;
            let mut v = alloc::string::String::new();
            for x in seen { v.push_str(&alloc::format!(" G#{x:X}")); }
            line(rep, alloc::format!("  STATUS  G#{a:X}: Linux moves through{v}, ferros G#{got:X}\n"));
        }
    }
    line(rep, alloc::format!("live state against Linux: A#{same} agree, A#{diff} configuration words differ, A#{idle} status words sit where Linux's move\n"));
}

/// Read every register of Linux's live snapshot (camera_snap.rs: 2914 registers the hv read sixteen frames into a Linux stream, all known to exist) and report each one ferros holds differently, then a per-page tally. The question it answers: with the link counting frames and the core silent, which configuration word does Linux's streaming front end hold that ours does not.
/// The flash log names each 4 KiB page before its first read: a read that stalls the bus then leaves its page as the log's last line.
fn snap_compare(rep: &mut Vec<u8>) {
    let (mut page, mut same, mut diff) = (0u32, 0u32, 0u32);
    let mut tally = alloc::string::String::new();
    let mut out = alloc::string::String::new();
    for &(a, linux) in crate::camera_snap::LINUX_SNAP.iter() {
        if a & !0xFFF != page {
            if page != 0 {
                tally.push_str(&alloc::format!("  G#{page:X}: A#{same} agree, A#{diff} differ\n"));
            }
            page = a & !0xFFF;
            (same, diff) = (0, 0);
            crate::flashlog::line(&alloc::format!("cam snap: page G#{page:X}"));
            crate::platform::pet_watchdogs();
        }
        let got = rd(a as usize);
        if got == linux {
            same += 1;
        } else {
            diff += 1;
            out.push_str(&alloc::format!("  G#{a:X}: Linux G#{linux:X}, ferros G#{got:X}\n"));
        }
    }
    tally.push_str(&alloc::format!("  G#{page:X}: A#{same} agree, A#{diff} differ\n"));
    line(rep, alloc::format!("live snapshot against Linux, every word that differs:\n{out}per page:\n{tally}"));
}

/// Read every interrupt source in the ISP pipeline and report the ones that are not zero, with their set bits named (camera_events.rs, generated from the Pixel kernel's LWIS event tables by tools/gen-isp-events.py).
/// This is the pipeline's stethoscope: 203 sources across the front end, the CSI link and PHY, and the PDMA engine, so one run says which stage is alive and where it falls silent — instead of diffing words whose meaning has to be guessed.
/// Each node is named in the flash log before it is read: some front-end pages stall the bus in some states, and the log's last line then names the one that did it.
pub fn events_dump(rep: &mut Vec<u8>, quiet_too: bool) {
    if !fe_powered() {
        return line(rep, alloc::format!("ISP front end is not powered\n"));
    }
    let mut live = 0u32;
    for src in crate::camera_events::SOURCES.iter() {
        crate::flashlog::line(&alloc::format!("cam events: {}", src.name));
        let v = rd(src.base + src.off);
        let m = if src.mask_off != 0 { rd(src.base + src.mask_off) } else { 0 };
        // An armed mask is as telling as a live status: it says the program enabled this channel and expects events from it. A channel with a mask and no status is one that was set up and never fired.
        if v == 0 && m == 0 && !quiet_too {
            continue;
        }
        if v != 0 {
            live += 1;
        }
        let mut names = alloc::string::String::new();
        for &(b, n) in src.bits.iter() {
            if v >> b & 1 == 1 {
                if !names.is_empty() {
                    names.push('|');
                }
                names.push_str(n);
            }
        }
        line(rep, alloc::format!("  {} ({}) +G#{:X} = G#{v:X} [{names}] mask G#{m:X}\n", src.name, src.kind, src.off));
    }
    line(rep, alloc::format!("A#{live} of A#{} interrupt sources are reporting\n", crate::camera_events::SOURCES.len()));
}

/// `diag cam link`: every word of CSI link 1 that the camera stack touches, as a flat dump.
/// The point is a diff. Cold ferros gets the sensor streaming and the receiver counting frames, and the front end still writes nothing, so what differs has to be in how the receiver is told where to send those frames. Running this on a cold ferros and again on a ferros chainloaded out of a streaming Android (the Linux handoff register oracle, HV.md) puts the two side by side with nothing else changed.
/// Only the receiver is read: the front end's own G#1C34_xxxx and G#1C35_xxxx pages stall the bus in some states (2026-10-20), and the question here is the input side.
pub fn link_dump(rep: &mut Vec<u8>) {
    if !fe_powered() {
        return line(rep, alloc::format!("ISP front end is not powered\n"));
    }
    dump_link_words(rep);
}

/// The dump itself, so the stream probe can take one while the sensor is actually streaming — the state a dump taken out of a live Android is in, and the only one worth diffing against it.
fn dump_link_words(rep: &mut Vec<u8>) {
    line(rep, alloc::format!("CSI link 1 at G#{CSIS_LINK1:X}, the G#000..G#1FC and G#600..G#6FC words:\n"));
    let s = link_snapshot();
    let mut row = alloc::string::String::new();
    for (i, v) in s.iter().enumerate() {
        if i % 8 == 0 {
            row.push_str(&alloc::format!("+G#{:03X}:", link_off(i)));
        }
        row.push_str(&alloc::format!(" G#{v:08X}"));
        if i % 8 == 7 {
            row.push('\n');
            line(rep, core::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        row.push('\n');
        line(rep, row);
    }
    line(rep, alloc::format!("D-PHY lanes: G#{:X} G#{:X} G#{:X}; common G#{:X}\n", rd(0x1C10_1800), rd(0x1C10_1900), rd(0x1C10_1A00), rd(0x1C10_1000)));
}

/// Stream the main sensor into CSI link 1 and see whether frames arrive: requires `cam up` (the sensor answering), brings the ISP front end up, replays the sensor init from the vault, configures the receiver (camera_rx.rs), snapshots link 1's registers, starts the sensor for 300 ms, snapshots again and reports every register that changed, then stops the sensor and the receiver. No DMA is set up: this only asks whether the link sees the stream.
pub fn stream_probe(rep: &mut Vec<u8>, init: Option<Vec<u8>>) {
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
        return;
    }
    let init = match init {
        Some(t) => t,
        None => {
            line(rep, alloc::format!("refusing: no `cam.boitata.init` in the vault (bridge store put cam.boitata.init @table)\n"));
            return;
        }
    };
    fe_up(rep);
    if !fe_powered() {
        return;
    }
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed{}\n", match fail { None => alloc::string::String::new(), Some((n, e)) => alloc::format!("; FAILED at record {n} with G#{e:X}") }));
    if fail.is_some() {
        return;
    }
    // The combo D-PHY sits behind a PMU isolation that its driver bypasses before use (phy-exynos-mipi.c `__set_phy_isolation`: PMU + the node's `isolation` offset G#3EBC, bit 0 = bypass). It is a PMU write, so the oracle never saw it; without it the PHY's lane registers do not hold what is written (measured 2026-10-19: they read 0 after the replay wrote 1).
    let iso = 0x1546_3EBCusize;
    let iso_before = rd(iso);
    let iso_ret = crate::platform::priv_reg(iso as u64, true, iso_before | 1);
    line(rep, alloc::format!("PHY isolation G#{iso_before:X} -> G#{:X} (EL3 write returned G#{iso_ret:X})\n", rd(iso)));
    for &(a, v) in crate::camera_rx::RX_START.iter() {
        wr(a, v);
    }
    let before = link_snapshot();
    let phy = |rep: &mut Vec<u8>, when: &str| line(rep, alloc::format!("{when}: PHY lanes G#{:X} G#{:X} G#{:X}; link 1 ctl G#{:X} +14 G#{:X} +1C G#{:X} +20 G#{:X}\n",
        rd(0x1C10_1800), rd(0x1C10_1900), rd(0x1C10_1A00), rd(CSIS_LINK1 + 4), rd(CSIS_LINK1 + 0x14), rd(CSIS_LINK1 + 0x1C), rd(CSIS_LINK1 + 0x20)));
    phy(rep, "receiver configured");
    let fc0 = sensor_frame_count();
    let son = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    let mut series = alloc::string::String::new();
    for _ in 0..20 {
        spin_us(50_000);
        let mut b = [0u8; 2];
        match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x00, 0x05], &mut b) {
            Ok(()) => series.push_str(&alloc::format!(" {:02X}{:02X}", b[0], b[1])),
            Err(e) => series.push_str(&alloc::format!(" e{e:X}")),
        }
    }
    line(rep, alloc::format!("sensor G#0005..6 every 50 ms after stream-on:{series}\n"));
    // The receiver as it stands with frames actually coming in, for the diff against a live Android's.
    dump_link_words(rep);
    let mut ms = [0u8; 2];
    let msr = crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00], &mut ms);
    // A running 26 MHz master clock on gpp9-0 reads back through the pad as a mix of 0s and 1s; a steady value means no clock on the pin.
    let mut ones = 0u32;
    for _ in 0..128 {
        ones += rd(GPIO_PERIC0 + 0x124) & 1;
    }
    line(rep, alloc::format!("sensor mode_select G#0100..1 reads {:?}; master-clock pad sampled high {ones}/128 (gpp9 CON G#{:X})\n", msr.map(|_| alloc::format!("G#{:02X}{:02X}", ms[0], ms[1])).map_err(|e| alloc::format!("G#{e:X}")), rd(GPIO_PERIC0 + 0x120)));
    phy(rep, "after 1 s streaming");
    let (fc1, fc2) = (None::<u8>, None::<u8>);
    let after = link_snapshot();
    let soff = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    spin_us(50_000);
    for &(a, v) in crate::camera_rx::RX_STOP.iter() {
        wr(a, v);
    }
    line(rep, alloc::format!("stream on {:?}, off {:?}; sensor frame count {:?} -> {:?} (100 ms) -> {:?} (300 ms)\n", son.map_err(|e| alloc::format!("G#{e:X}")), soff.map_err(|e| alloc::format!("G#{e:X}")), fc0, fc1, fc2));
    line(rep, alloc::format!("link 1 registers that changed while streaming:\n"));
    let mut any = false;
    for i in 0..192 {
        if before[i] != after[i] {
            any = true;
            line(rep, alloc::format!("  +G#{:03X}: G#{:X} -> G#{:X}\n", link_off(i), before[i], after[i]));
        }
    }
    if !any {
        line(rep, alloc::format!("  none\n"));
    }
}

/// The front end's DMA path (zuma-isp.dtsi): three S2MPUs and three SYSMMUs in front of its masters, and the gates CMU_ISPFE keeps on them (cmucal-sfr.c).
const ISPFE_S2MPUS: [usize; 3] = [0x1C14_0000, 0x1C18_0000, 0x1C1C_0000];
const ISPFE_SYSMMUS: [(usize, usize); 3] = [(0x1C11_0000, 0x213C), (0x1C15_0000, 0x2094), (0x1C19_0000, 0x2098)];
/// ISPFE core clock gate (CLK_CON_GAT_GOUT_BLK_ISPFE_UID_ISPFE_IPCLKPORT_CLK_ISPFE).
const ISPFE_CORE_GATE: usize = CMU_ISPFE + 0x20B8;
const ISPFE: usize = 0x1C30_0000;
const PATTERN: u32 = 0xA5A5_A5A5;
/// The front end's status words Linux's per-frame handler reads (ispfe-full capture, 2026-10-19), with their values under Linux: channel interrupt status G#1C32_0424 / G#1C32_2024 (1, 8, G#12, G#13), G#1C32_008C (G#81 / 1), a counter G#1C32_0124 (G#7_5000 / G#7_5800) and what looks like a data checksum G#1C32_013C.
const ISPFE_STATUS: [usize; 15] = [0x1C32_003C, 0x1C32_008C, 0x1C32_0094, 0x1C32_0110, 0x1C32_011C, 0x1C32_0124, 0x1C32_013C, 0x1C32_0424, 0x1C32_0430, 0x1C32_0444, 0x1C32_2024, 0x1C32_2030, 0x1C32_2044, 0x1C33_0000, 0x1C35_0000];
const PROBE_BYTES: usize = 16 * 1024;

/// Let the front end's masters reach physical memory: S2MPU protection off for every VID, SYSMMU VM 0 translation off (bypass), the ISPFE core clock on.
fn ispfe_dma_open(rep: &mut Vec<u8>) {
    for s in ISPFE_S2MPUS {
        let v = rd(s + 0x60);
        if v & 0xFFFF_0000 == 0x9000_0000 {
            wr(s + 0x54, 0xFF);
            line(rep, alloc::format!("S2MPU G#{s:X}: protection off (G#{:X})\n", rd(s)));
        } else {
            line(rep, alloc::format!("S2MPU G#{s:X}: version G#{v:X}, left alone\n"));
        }
    }
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

/// Parse a `cam.ispfe.prog` (tools/camera-prog.py): 'CPRG', nbuf, buffer PAs, npairs, (register, value) pairs; or 'CPR2' with three such sections: the CPU's writes before the PDMA start, the PDMA program, the CPU's writes after it.
fn parse_prog(p: &[u8]) -> Option<(Vec<usize>, [Vec<(usize, u32)>; 3])> {
    let w = |i: usize| p.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let v2 = match p.get(..4)? {
        b"CPRG" => false,
        b"CPR2" => true,
        _ => return None,
    };
    let nb = w(1)? as usize;
    let bufs = (0..nb).map(|i| w(2 + i).map(|v| v as usize)).collect::<Option<Vec<_>>>()?;
    let mut at = 2 + nb;
    let mut section = || -> Option<Vec<(usize, u32)>> {
        let n = w(at)? as usize;
        let v = (0..n).map(|i| Some((w(at + 1 + 2 * i)? as usize, w(at + 2 + 2 * i)?))).collect::<Option<Vec<_>>>()?;
        at += 1 + 2 * n;
        Some(v)
    };
    if v2 {
        let pre = section()?;
        let main = section()?;
        let post = section()?;
        Some((bufs, [pre, main, post]))
    } else {
        Some((bufs, [Vec::new(), section()?, Vec::new()]))
    }
}

/// `diag cam frame`: everything `cam stream` does up to the receiver, then the front end's DMA program (the HAL's PDMA list, decoded, its output buffers moved to physical memory filled with a pattern), stream for half a second, and report which output buffers the front end wrote.
pub fn frame_probe(rep: &mut Vec<u8>, init: Option<Vec<u8>>, prog: Option<Vec<u8>>) {
    let Some((mut bufs, sections)) = prog.as_deref().and_then(parse_prog) else {
        return line(rep, alloc::format!("refusing: no valid `cam.ispfe.prog` in the vault (tools/camera-prog.py, then bridge store put cam.ispfe.prog @file)\n"));
    };
    bufs.sort();
    bufs.dedup();
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let Some(init) = init else {
        return line(rep, alloc::format!("refusing: no `cam.boitata.init` in the vault\n"));
    };
    fe_up(rep);
    if !fe_powered() {
        return;
    }
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed\n"));
    if fail.is_some() {
        return line(rep, alloc::format!("sensor init failed\n"));
    }
    let iso = 0x1546_3EBCusize;
    let _ = crate::platform::priv_reg(iso as u64, true, rd(iso) | 1);
    for &(a, v) in crate::camera_rx::RX_START.iter() {
        wr(a, v);
    }
    ispfe_dma_open(rep);
    for &b in &bufs {
        for o in (0..PROBE_BYTES).step_by(4) {
            wr(b + o, PATTERN);
        }
    }
    for (name, pairs) in ["CPU setup before the PDMA start", "PDMA program", "CPU setup after the PDMA start"].iter().zip(sections.iter()) {
        let mut applied = 0u32;
        let mut skipped = 0u32;
        for &(a, v) in pairs {
            // The ISPFE block, the CSI links and the PDMA wrapper; never the PDMA engine page (its descriptor ring is not used) or anything outside the front end.
            // G#1C36_xxxx is not CPU-writable (2026-10-20): it ignores CPU writes and part of it changes on its own while the sensor streams, so the PDMA's block-load records target hardware the CPU cannot stand in for. Those writes are dropped rather than scattered.
            if (0x1C04_0000..ISPFE + 0x10_0000).contains(&a) && !(0x1C39_0000..0x1C39_1000).contains(&a) && !(0x1C36_0000..0x1C37_0000).contains(&a) {
                wr(a, v);
                applied += 1;
            } else {
                skipped += 1;
            }
        }
        line(rep, alloc::format!("{name}: A#{applied} writes applied, A#{skipped} outside the front end skipped\n"));
    }
    line(rep, alloc::format!("A#{} output buffers patterned\n", bufs.len()));
    // Which of the written registers actually hold their value: a whole page that does not is a block still gated or in reset, and that is the thing stopping the frame reaching memory.
    let mut pages: Vec<(usize, u32, u32)> = Vec::new();
    for pairs in sections.iter() {
        for &(a, v) in pairs {
            if !(0x1C04_0000..ISPFE + 0x10_0000).contains(&a) || (0x1C39_0000..0x1C39_1000).contains(&a) {
                continue;
            }
            let page = a & !0xFFF;
            let ok = rd(a) == v;
            match pages.iter_mut().find(|p| p.0 == page) {
                Some(p) => {
                    p.1 += 1;
                    p.2 += ok as u32;
                }
                None => pages.push((page, 1, ok as u32)),
            }
        }
    }
    pages.sort();
    let mut s = alloc::string::String::from("written registers that read back (per page):");
    for (page, n, ok) in pages {
        s.push_str(&alloc::format!(" G#{page:X}: A#{ok}/A#{n}"));
    }
    s.push('\n');
    line(rep, s);
    let snap = || ISPFE_STATUS.map(|a| rd(a));
    let before = snap();
    // Whole pages, to see what moves at all while streaming: the two output channels and CSI link 1.
    const PAGES: [usize; 3] = [0x1C32_0000, 0x1C32_2000, CSIS_LINK1];
    let page_snap = || -> Vec<u32> { PAGES.iter().flat_map(|&p| (0..1024).map(move |i| rd(p + i * 4))).collect() };
    let pages_before = page_snap();
    let son = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    let mut series = alloc::string::String::new();
    for _ in 0..10 {
        spin_us(50_000);
        let mut b = [0u8; 2];
        match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x00, 0x05], &mut b) {
            Ok(()) => series.push_str(&alloc::format!(" {:02X}", b[0])),
            Err(e) => series.push_str(&alloc::format!(" e{e:X}")),
        }
    }
    let after = snap();
    let pages_after = page_snap();
    let soff = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    spin_us(50_000);
    for &(a, v) in crate::camera_rx::RX_STOP.iter() {
        wr(a, v);
    }
    line(rep, alloc::format!("stream on {:?} off {:?}; sensor frame counter:{series}\n", son.is_ok(), soff.is_ok()));
    let mut st = alloc::string::String::from("front-end status before -> while streaming:");
    for (i, a) in ISPFE_STATUS.iter().enumerate() {
        st.push_str(&alloc::format!(" G#{:X}: G#{:X}{}", a & 0xF_FFFF, before[i], if before[i] != after[i] { alloc::format!("->G#{:X}", after[i]) } else { alloc::string::String::new() }));
    }
    st.push('\n');
    line(rep, st);
    let mut moved = alloc::string::String::from("words that changed while streaming:");
    let mut n = 0;
    for (i, (b, a)) in pages_before.iter().zip(pages_after.iter()).enumerate() {
        if b != a {
            n += 1;
            if n <= 48 {
                moved.push_str(&alloc::format!(" G#{:X}: G#{b:X}->G#{a:X}", PAGES[i / 1024] + (i % 1024) * 4));
            }
        }
    }
    moved.push_str(&alloc::format!(" (A#{n} in all)\n"));
    line(rep, moved);
    // The G#1C36_xxxx region (the front end's coefficient/LUT memory, which the HAL fills through the PDMA's block-load records, never with CPU writes) holds none of what the CPU puts there: probe it directly so the failure mode is named.
    let mut probe = alloc::string::String::from("LUT region probe:");
    for a in [0x1C36_1D48usize, 0x1C36_95FC, 0x1C36_A02C, 0x1C36_C8B8] {
        let r1 = rd(a);
        spin_us(20_000);
        let r2 = rd(a);
        probe.push_str(&alloc::format!(" G#{a:X}: G#{r1:X} then G#{r2:X} 20 ms later{};", if r1 == r2 { "" } else { " CHANGED BY ITSELF" }));
    }
    probe.push('\n');
    line(rep, probe);
    let mut any = false;
    for &b in &bufs {
        let mut changed = 0usize;
        let mut first = None;
        for o in (0..PROBE_BYTES).step_by(4) {
            if rd(b + o) != PATTERN {
                changed += 1;
                first.get_or_insert(o);
            }
        }
        if changed > 0 {
            any = true;
            let f = first.unwrap();
            line(rep, alloc::format!("buffer G#{b:X}: A#{changed} of A#{} words written, first at +G#{f:X}: G#{:08X} G#{:08X} G#{:08X} G#{:08X}\n", PROBE_BYTES / 4, rd(b + f), rd(b + f + 4), rd(b + f + 8), rd(b + f + 12)));
        }
    }
    if !any {
        line(rep, alloc::format!("no output buffer changed in its first A#{} KiB\n", PROBE_BYTES / 1024));
    }
}

/// The PDMA engine's registers (G#1C39_0000), in the order a camera open drives them (oracle capture 2026-10-19).
const PDMA: usize = 0x1C39_0000;
/// The descriptor ring and the rest of the job live at fixed physical addresses the builder and the kernel agree on (tools/camera-pdma-image.py).
const PDMA_RING_PA: usize = 0x8800_0000;
const PDMA_BUF_PA: usize = 0x8810_0000;
/// A slice has to hold a frame: the captured mode is 4080 x 3072 RAW10, which is 15 MiB, so the 2 MiB slices this started with could never have held one.
/// Seven of them, which is what fits between PDMA_BUF_PA and the first reserved carveout — zuma-rmem.dtsi puts that at G#9000_0000 and the hv's own addresses all sit below G#8400_0000, so the range is free. tools/camera-pdma-image.py wraps the list's distinct addresses over the same seven and must agree with this number.
const PDMA_BUF_SLICE: usize = 0x100_0000;
const PDMA_BUFS: usize = 7;

/// `diag cam pdma`: the receive side as `cam frame` brings it up, the CPU's own setup writes from the capture, and the front end's register program handed to the **PDMA engine** rather than replayed by the CPU — the only way to reach the G#1C36_xxxx region the CPU cannot write (docs/oracle/CAMERA.md). The job comes from a build-time blob ('CPR3': physical address + bytes per piece).
pub fn pdma_probe(rep: &mut Vec<u8>, init: Option<Vec<u8>>, prog: Option<Vec<u8>>, slit_lines: u32, fast: bool) {
    let job: &[u8] = if cfg!(cam_pdma_blob) { include_bytes!(concat!(env!("OUT_DIR"), "/cam_pdma.bin")) } else { &[] };
    if job.len() < 8 || &job[..4] != b"CPR3" {
        return line(rep, alloc::format!("refusing: no PDMA job built in (FERROS_CAM_PDMA=<tools/camera-pdma-image.py output>)\n"));
    }
    let mut id = [0u8; 2];
    if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0, 0], &mut id).is_err() || id != [0x08, 0xEF] {
        return line(rep, alloc::format!("refusing: sensor not answering its ID (run `diag cam up`)\n"));
    }
    let Some(init) = init else {
        return line(rep, alloc::format!("refusing: no sensor init table\n"));
    };
    fe_up(rep);
    if !fe_powered() {
        return;
    }
    let (w, r, fail) = replay_init(&init);
    line(rep, alloc::format!("sensor init: {w} writes, {r} reads replayed\n"));
    if fail.is_some() {
        return line(rep, alloc::format!("sensor init failed\n"));
    }
    let iso = 0x1546_3EBCusize;
    let _ = crate::platform::priv_reg(iso as u64, true, rd(iso) | 1);
    ispfe_dma_open(rep);
    // Lay the job out in DRAM. The SysMMUs are in bypass, so what the engine reads as a bus address is this physical address.
    let w32 = |i: usize| u32::from_le_bytes([job[i], job[i + 1], job[i + 2], job[i + 3]]) as usize;
    let n = w32(4);
    let mut at = 8;
    let mut placed = 0u32;
    for _ in 0..n {
        if at + 8 > job.len() {
            return line(rep, alloc::format!("job truncated\n"));
        }
        let (pa, len) = (w32(at), w32(at + 4));
        at += 8;
        if at + len > job.len() || !(0x8800_0000..0x9000_0000).contains(&pa) {
            return line(rep, alloc::format!("job piece at G#{pa:X} len G#{len:X} out of range\n"));
        }
        for i in 0..len {
            unsafe { core::ptr::write_volatile((pa + i) as *mut u8, job[at + i]) };
        }
        at += (len + 3) & !3;
        placed += 1;
    }
    unsafe { core::arch::asm!("dsb sy") };
    line(rep, alloc::format!("job placed: A#{placed} pieces, ring at G#{PDMA_RING_PA:X}\n"));
    // The CPU's own setup writes, in the capture's order: the ones before the PDMA start now, the ones after it once the engine is running. Without them the engine applies the program but the frame writer stays idle (2026-10-20).
    let cpu = prog.as_deref().and_then(parse_prog);
    let apply = |rep: &mut Vec<u8>, what: &str, pairs: Option<&Vec<(usize, u32)>>| {
        let Some(pairs) = pairs else { return };
        let mut n = 0u32;
        for &(a, v) in pairs {
            if (0x1C04_0000..ISPFE + 0x10_0000).contains(&a) && !(0x1C39_0000..0x1C39_1000).contains(&a) && !(0x1C36_0000..0x1C37_0000).contains(&a) {
                wr(a, v);
                n += 1;
            }
        }
        line(rep, alloc::format!("{what}: A#{n} CPU writes\n"));
    };
    apply(rep, "CPU setup before the start", cpu.as_ref().map(|c| &c.1[0]));
    // Pattern the head of every output slice so a write anywhere in it shows.
    // Every probe point is patterned first, so a point that later reads anything else — including zero — was written. Sampling every 256 bytes maps where a frame lands without writing the whole 36 MiB.
    for b in 0..PDMA_BUFS {
        // Seven 16 MiB slices probed every 256 bytes is getting on for half a million accesses; without a pet in here the watchdog takes the phone before the engine ever starts (2026-10-20).
        crate::platform::pet_watchdogs();
        let base = PDMA_BUF_PA + b * PDMA_BUF_SLICE;
        for o in (0..PROBE_BYTES).step_by(4) {
            wr(base + o, PATTERN);
        }
        for o in (0..PDMA_BUF_SLICE).step_by(256) {
            wr(base + o, PATTERN);
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    // Start the engine exactly as the capture shows a camera open starting it.
    wr(PDMA + 0x2C, 1);
    wr(PDMA + 0x40, 0x1F);
    wr(PDMA + 0x20, 0);
    wr(PDMA + 0x04, PDMA_RING_PA as u32);
    wr(PDMA + 0x08, 0);
    wr(PDMA + 0x0C, 16);
    wr(PDMA + 0x10, 0);
    wr(PDMA + 0x00, 1);
    wr(PDMA + 0x34, 0);
    wr(PDMA + 0x28, 1);
    // The receiver last, as a camera open does it: the capture starts the PDMA at line 1421 and only enables the link (SYSREG_ISPFE + G#500 = 2, the end of RX_START) at 1573.
    // Enabling the link first — which is what ferros did until now — left the front end latching its configuration and never seeing a frame start: the only event either channel ever raised was SHADOW_UPDATE, never SOF (zuma-isp-event-info.dtsi names the bits).
    for &(a, v) in crate::camera_rx::RX_START.iter() {
        wr(a, v);
    }
    apply(rep, "CPU setup after the start", cpu.as_ref().map(|c| &c.1[2]));
    line(rep, alloc::format!("PDMA started: base G#{:X} count A#{} ctrl G#{:X} +G#2C G#{:X} +G#40 G#{:X}\n", rd(PDMA + 4), rd(PDMA + 0xC), rd(PDMA), rd(PDMA + 0x2C), rd(PDMA + 0x40)));
    // The receiver's own words before the stream starts. The program's CPU writes land in the same G#1C04_0000.. range the receiver lives in, so they can undo what RX_START configured: if link 1's counters do not move here the way they move under `cam stream`, the frame writer is idle because nothing is arriving, not because the write path is off.
    let link_before = link_snapshot();
    let son = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x01, 0x03]);
    // Narrow the window only once the sensor is streaming. Set before stream-on it simply does not start — the frame counter never leaves zero and nothing reaches the receiver — while changed mid-stream it takes it at once, which is how `diag cam slit` measured 4080 x 64 at about 1184 frames a second (2026-10-20).
    // The front end has to be built for the same height or it is expecting 3072 lines where 64 arrive: tools/camera-pdma-image.py --height.
    if slit_lines > 0 {
        spin_us(100_000);
        let w16 = |a: u16, v: u16| crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[(a >> 8) as u8, a as u8, (v >> 8) as u8, v as u8]);
        let rows = (slit_lines * 4).min(12288);
        let _ = w16(0x0104, 1);
        let _ = w16(0x0346, 0x0010);
        let _ = w16(0x034A, (0x0010 + rows - 1) as u16);
        let _ = w16(0x034E, slit_lines as u16);
        let _ = w16(0x0340, (slit_lines + 64) as u16);
        let _ = w16(0x0104, 0);
        line(rep, alloc::format!("sensor narrowed mid-stream to 4080 x A#{slit_lines}, A#{} bytes a frame at RAW10\n", 4080 * slit_lines * 10 / 8));
    }
    // Advance the descriptor index per frame, as the HAL does (+G#10 steps by G#10).
    // The front end's own per-frame status words are sampled alongside. Under Linux these are read and written straight back every frame — an interrupt acknowledgement, not configuration, which is why the captured CPU program leaves them out — and the values it acknowledges are non-zero: G#1C32_008C reads G#81 and G#1 on alternate frames, +G#424 reads G#8/G#12/G#1A, +G#124 reads G#75000. If ferros reads zero in all of them while the sensor streams and the receiver counts, the front end is raising no frame events at all, and the idle writer is a symptom of its input, not of its output.
    // Only the G#1C32 page is sampled: G#1C34 and G#1C35 stall the bus in some states (2026-10-20).
    const FRAME_STATUS: [usize; 6] = [0x1C32_008C, 0x1C32_0110, 0x1C32_0124, 0x1C32_0424, 0x1C32_2024, 0x1C32_049C];
    // The front end's per-frame interrupt acknowledgement, in the order a camera open does it: read the word, write the same value straight back.
    // Under Linux these look like bookkeeping, which is why the captured CPU program leaves them out — it stops deliberately "before the per-frame interrupt handling". They are not bookkeeping. Unacknowledged, the front end latches its first frame and never advances: G#1C32_008C stays at G#81 instead of alternating with G#1, the buffer index at +G#49C stays 0 instead of stepping 0-3, and the write pointer at +G#124 stays put instead of stepping by G#800 through four buffers. Nothing is ever written out (2026-10-20).
    const FRAME_ACK: [usize; 28] = [
        0x1C35_0160, 0x1C35_0178, 0x1C35_0190,
        0x1C32_008C,
        0x1C32_2004, 0x1C32_2024, 0x1C32_2038, 0x1C32_204C,
        0x1C32_0404, 0x1C32_0424, 0x1C32_0438, 0x1C32_044C,
        0x1C32_0110,
        0x1C35_0058, 0x1C35_0088, 0x1C35_00B8, 0x1C35_00E8, 0x1C35_0118, 0x1C35_01AC,
        0x1C33_1300, 0x1C33_3300, 0x1C33_5300, 0x1C33_7300, 0x1C33_9300, 0x1C33_B300, 0x1C33_C300, 0x1C33_D300, 0x1C33_E300,
    ];
    let mut series = alloc::string::String::new();
    // `cam pdma fast`: answer the front end the way Linux's interrupt handler does, within microseconds rather than on a 50 ms tick. Linux never touches the link's frame words; it runs off the core's aggregate (G#1C32_008C), acknowledging and advancing the PDMA as soon as it fires. If the core needs that answer inside the gap between frames, polling at 50 ms would miss it every time and leave exactly what we see: one SHADOW_UPDATE and silence.
    if fast {
        crate::flashlog::line("cam pdma fast: polling the core aggregate");
        let f = crate::platform::cntfrq();
        let t0 = crate::platform::cntpct();
        let (mut events, mut idx, mut log) = (0u32, 0u32, alloc::string::String::new());
        let mut last_pet = t0;
        while crate::platform::cntpct().wrapping_sub(t0) < f && events < 64 {
            let cmn = rd(0x1C32_008C);
            if cmn != 0 {
                let t = crate::platform::cntpct().wrapping_sub(t0) * 1_000_000 / f;
                let (img, pd) = (rd(0x1C32_0424), rd(0x1C32_2024));
                for &a in FRAME_ACK.iter() {
                    let v = rd(a);
                    wr(a, v);
                }
                idx += 0x10;
                wr(PDMA + 0x10, idx);
                events += 1;
                log.push_str(&alloc::format!("  t+A#{t} us: aggregate G#{cmn:X} img G#{img:X} pd G#{pd:X}; answered, PDMA index G#{idx:X}\n"));
            }
            if crate::platform::cntpct().wrapping_sub(last_pet) > f / 16 {
                crate::platform::pet_watchdogs();
                last_pet = crate::platform::cntpct();
            }
        }
        line(rep, alloc::format!("fast loop, one second: A#{events} core events answered\n{log}"));
        let mut b = [0u8; 2];
        if crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x00, 0x05], &mut b).is_ok() {
            series.push_str(&alloc::format!(" {:02X}", b[0]));
        }
    }
    crate::flashlog::line("cam pdma: sampling the G#1C32 per-frame status words while the engine runs");
    let mut status = [[0u32; FRAME_STATUS.len()]; 10];
    for f in 1..=(if fast { 0 } else { 10u32 }) {
        spin_us(50_000);
        wr(PDMA + 0x10, f * 0x10);
        for (k, &a) in FRAME_STATUS.iter().enumerate() {
            status[f as usize - 1][k] = rd(a);
        }
        // Acknowledge the frame so the front end moves on to the next buffer.
        for &a in FRAME_ACK.iter() {
            let v = rd(a);
            wr(a, v);
        }
        // Then enable loch0's image and PD paths: Linux writes G#81 to G#1C32_00A0 right after acknowledging the first SHADOW_UPDATE (capture access 531, after the index write at 458), and the image path's SOF follows. A plain write, not a read-back acknowledgement, so neither the setup extraction nor FRAME_ACK carried it; the live snapshot showed it (Linux G#81, ferros 0). Its pair G#1C32_00B0 (written 1 then G#80 in setup) is the request, G#1C32_00A4 the stop.
        if f == 1 {
            wr(0x1C32_00A0, 0x81);
        }
        // Once, a few frames in, read the whole pipeline by name while the engine is live — the state Linux reads these in.
        if f == 3 {
            line(rep, alloc::format!("every reporting interrupt source, three frames in:\n"));
            events_dump(rep, false);
            snap_compare(rep);
            // `live_compare` is not called here. It stalled the bus and reset the phone reading G#1C32_0430, the loch0 image mask — a word the event dump above reads without trouble moments earlier in the same run — so front-end reads mid-stream are timing-dependent rather than simply safe or unsafe (2026-10-21). Its useful result was already in the event dump: see docs/oracle/CAMERA.md.
        }
        let mut b = [0u8; 2];
        match crate::hsi2c::write_read(SENSOR_BUS.base, SENSOR_ADDR, &[0x00, 0x05], &mut b) {
            Ok(()) => series.push_str(&alloc::format!(" {:02X}", b[0])),
            Err(e) => series.push_str(&alloc::format!(" e{e:X}")),
        }
    }
    for (k, &a) in FRAME_STATUS.iter().enumerate() {
        if fast {
            break;
        }
        let mut row = alloc::format!("front end G#{:05X} per frame:", a & 0xF_FFFF);
        for f in 0..10 {
            row.push_str(&alloc::format!(" G#{:X}", status[f][k]));
        }
        row.push('\n');
        line(rep, row);
    }
    let soff = crate::hsi2c::write_bytes(SENSOR_BUS.base, SENSOR_ADDR, &[0x01, 0x00, 0x00, 0x00]);
    spin_us(50_000);
    line(rep, alloc::format!("stream on {:?} off {:?}; sensor frame counter:{series}\n", son.is_ok(), soff.is_ok()));
    line(rep, alloc::format!("PDMA after: ctrl G#{:X} index G#{:X} +G#2C G#{:X} +G#30 G#{:X} +G#34 G#{:X} +G#38 G#{:X} +G#3C G#{:X} +G#44 G#{:X}\n", rd(PDMA), rd(PDMA + 0x10), rd(PDMA + 0x2C), rd(PDMA + 0x30), rd(PDMA + 0x34), rd(PDMA + 0x38), rd(PDMA + 0x3C), rd(PDMA + 0x44)));
    wr(PDMA + 0x00, 0);
    wr(PDMA + 0x2C, 0);
    wr(PDMA + 0x40, 0);
    // Read the receiver with the engine stopped but the receiver still configured: once RX_STOP runs its counters are gone, and reading while the engine ran risks the bus stall that G#1C34/G#1C35 showed.
    let link_after = link_snapshot();
    let mut moved = 0u32;
    let mut which = alloc::string::String::new();
    for i in 0..link_before.len() {
        if link_before[i] != link_after[i] {
            moved += 1;
            if moved <= 12 {
                which.push_str(&alloc::format!(" +G#{:X}: G#{:X}->G#{:X}", link_off(i), link_before[i], link_after[i]));
            }
        }
    }
    line(rep, alloc::format!("link 1 over the run: A#{moved} of A#{} words moved;{}\n", link_before.len(), if moved == 0 { alloc::string::String::from(" nothing arrived at the receiver") } else { which }));
    for &(a, v) in crate::camera_rx::RX_STOP.iter() {
        wr(a, v);
    }
    // Did the engine execute the list? These registers are set only by the program, and their values would now be the remapped G#88xx_xxxx slices. Read only once the engine and the receiver are stopped: reading one while the engine was running stalled the bus until the watchdog (2026-10-20).
    let mut ran = alloc::string::String::from("registers the program sets:");
    // Only these are safe to read once the engine has stopped: G#1C34_xxxx and G#1C35_xxxx both stall the bus until the watchdog at that point (2026-10-20), so the proof that the engine ran comes from the two channel registers, which hold the buffer addresses only the engine could have written.
    for a in [0x1C32_0510usize, 0x1C32_2110] {
        // Each address is named in the flash log before it is read: a read that stalls the bus takes the whole report with it, and the log's last line is then the culprit.
        crate::flashlog::line(&alloc::format!("cam pdma: reading G#{a:X}"));
        let v = rd(a);
        crate::flashlog::line(&alloc::format!("cam pdma: G#{a:X} = G#{v:X}"));
        ran.push_str(&alloc::format!(" G#{:X}=G#{v:X}", a & 0xF_FFFF));
    }
    ran.push('\n');
    line(rep, ran);
    let mut any = false;
    for b in 0..PDMA_BUFS {
        crate::platform::pet_watchdogs();
        let base = PDMA_BUF_PA + b * PDMA_BUF_SLICE;
        // The pattern covers the head of the slice; a frame could land anywhere in it, so the whole slice is sampled as well.
        let mut changed = 0usize;
        let mut first = None;
        for o in (0..PROBE_BYTES).step_by(4) {
            if rd(base + o) != PATTERN {
                changed += 1;
                first.get_or_insert(o);
            }
        }
        // Count every probe point that no longer holds the pattern, zeros included: a frame of a dark scene writes mostly zeroes, and excluding them hid exactly the result this test exists to find (2026-10-20).
        let (mut moved, mut zeroed, mut last) = (0usize, 0usize, 0usize);
        for o in (0..PDMA_BUF_SLICE).step_by(256) {
            let v = rd(base + o);
            if v != PATTERN {
                moved += 1;
                last = o;
                if v == 0 {
                    zeroed += 1;
                }
                first.get_or_insert(o);
            }
        }
        if changed > 0 || moved > 0 {
            any = true;
            let f = first.unwrap();
            line(rep, alloc::format!("buffer A#{b} at G#{base:X}: A#{changed} words in the head; A#{moved} of A#{} probe points written (A#{zeroed} to zero), from +G#{f:X} to +G#{last:X}; at +G#{f:X}: G#{:08X} G#{:08X} G#{:08X} G#{:08X}\n", PDMA_BUF_SLICE / 256, rd(base + f), rd(base + f + 4), rd(base + f + 8), rd(base + f + 12)));
        }
    }
    if !any {
        line(rep, alloc::format!("no output slice changed at any probe point\n"));
    }
}
