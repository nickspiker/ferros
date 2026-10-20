//! The main rear camera (boitata, sensor@0) on husky: what its power-up touches, from the device tree and the hv register oracle (docs/oracle/CAMERA.md).
//!
//! Power order (`module_power_seq@0`): camera-PMIC LDO1, sub-PMIC GPIO2 (sensor enable), LDO8, sub-PMIC LDO26 and LDO12, LDO4, master-clock pin on, ToF reset, LDO5, the GPIO4 switch; then the sensor reset released and 9 ms before the first I2C.
//! The camera PMIC (SLG51002 at G#75) sits on HS-I2C USI14 (G#1098_0000); the sensor (G#3D) on HS-I2C USI1 (G#1088_0000). The sub-PMIC is reached through ACPM (acpm.rs).
//! This first version only reports state, so a cold boot can be measured before anything is written.

use crate::acpm;
use alloc::vec::Vec;
use core::ptr::read_volatile;

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
    show(rep, "sub GPIO2_SET (sensor enable)", acpm::read(acpm::SUB, acpm::T_GPIO, 0x0A));
    show(rep, "sub L12S_CTRL (1.85 V rail)", acpm::read(acpm::SUB, acpm::T_PMIC, 0x3A));
    show(rep, "sub L26S_CTRL (3.3 V rail)", acpm::read(acpm::SUB, acpm::T_PMIC, 0x48));
}
