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

/// Voltage codes for the main camera's rails, V = base + code × step (slg51002-regulator.c: LDO1–5 base 1.2 V step 10 mV, LDO6–8 base 0.4 V step 5 mV), targets from the device tree's regulator nodes. The chip's cold defaults are different (LDO1 G#A5 = 2.85 V, LDO8 G#8C = 1.10 V): Linux sets these when it registers the regulators at boot, which is why a camera-open capture never shows them. Each entry: name, VSEL register, code, enable bit, and the range registers' address to check the code against.
const RAILS: [(&str, u16, u8, u32, u16); 4] = [
    ("LDO1 2.25 V", 0x2000, 0x69, 0, 0x2060),
    ("LDO4 2.95 V", 0x2500, 0xAF, 3, 0x2560),
    ("LDO5 2.90 V", 0x2700, 0xAA, 4, 0x2760),
    ("LDO8 0.97 V", 0x3200, 0x72, 7, 0x3260),
];

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
    for (name, vsel, code, _, range) in RAILS {
        let r = (|| -> Result<alloc::string::String, alloc::string::String> {
            let lo = pmic_read(range).map_err(hex)?;
            let hi = pmic_read(range + 1).map_err(hex)?;
            if code < lo || code > hi {
                return Err(alloc::format!("code G#{code:02X} outside range G#{lo:02X}..G#{hi:02X}"));
            }
            let before = pmic_read(vsel).map_err(hex)?;
            pmic_write(vsel, code).map_err(hex)?;
            let after = pmic_read(vsel).map_err(hex)?;
            if after != code {
                return Err(alloc::format!("wrote G#{code:02X}, reads G#{after:02X}"));
            }
            Ok(alloc::format!("G#{before:02X} -> G#{after:02X}"))
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
