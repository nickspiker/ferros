//! The torch's power (docs/TORCH.md): a TI LM3644 flash LED driver at G#63 on hsi2c_14 (PERIC0 USI14, G#1098_0000), its hardware enable (HWEN) on gpp8-2, on husky and akita alike (zuma-husky-camera.dtsi, zuma-akita-camera.dtsi: `flash-lm3644`, LWIS, so Linux only passes the HAL's register writes thru). Its registers are the `torch` process's (payloads/torch, thru its `hsi2c14` grant); this is what it may not do: the bus's pins, clock source and controller, and HWEN.
//!
//! hsi2c_14 is also the camera PMIC's bus (SLG51002 at G#75, camera.rs `PMIC_BUS`). One owner at a time: while this switch is on the bus is the torch process's and every `diag cam …` is refused (service.rs); while a camera tool runs the switch cannot come on, because both run on the kernel's core and the service loop runs no process slice (so no SYS_POWER) while it runs a diag. The torch process touches the bus only between its switch's on and off.
//!
//! Off is HWEN driven low: the chip goes to shutdown and its registers to their defaults (standby, flash and torch off, the STROBE and TORCH pins ignored), whatever its I2C state and whatever the process left in it. That is the whole off path: one GPIO write, no bus, no PMIC, nothing that can fail. A process that ends with the switch on (one shot dead) has it turned off by the service loop's next pass (process.rs `power_offs_apply`). Out of any reset the pin is an input and the chip's own HWEN pulldown keeps it in shutdown (the DT's idle state for gpp8-2: input, no pull).

use crate::camera::PMIC_BUS;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

const GPIO_PERIC0: usize = 0x1084_0000;
/// gpp8: pins 0-1 hsi2c_4 (the tele camera's bus, camera.rs), pin 2 the LM3644's HWEN. Banks are G#20 apart in DT order (zuma-pinctrl.dtsi): CON +0 (4 bits a pin), DAT +4, PUD +8.
const GPP8: usize = GPIO_PERIC0 + 0x100;
const HWEN: u32 = 2;
/// gpp16 (after gpp17 in the bank order): pins 0-1 the bus, function 2, no pull (`hsi2c14_bus`).
const GPP16: usize = GPIO_PERIC0 + 0x220;
/// USI14's clock-source mux: bit 4 clear picks the 24.576 MHz oscillator (camera.rs `USI14_MUX`: cold, the IP clock's upstream is not running and the engine never leaves START).
const USI14_MUX: usize = 0x1080_0000 + 0x640;
/// The camera's divider on the oscillator, about 192 kHz on the wire: what this bus has run at under ferros (the DT asks 390 kHz).
const FS_DIV: u32 = 7;

/// The switch's state: on means the bus is the torch process's.
static HELD: AtomicBool = AtomicBool::new(false);

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn pin_field(reg: usize, pin: u32, val: u32) {
    wr(reg, (rd(reg) & !(0xF << (pin * 4))) | (val & 0xF) << (pin * 4));
}
fn spin_us(us: u64) {
    let t = crate::platform::cntpct() + crate::platform::cntfrq() / 1_000_000 * us;
    while crate::platform::cntpct() < t {}
}

/// HWEN an output at `high`, the level set before the mode so it never glitches the other way.
fn hwen(high: bool) {
    let d = rd(GPP8 + 4);
    wr(GPP8 + 4, if high { d | 1 << HWEN } else { d & !(1 << HWEN) });
    pin_field(GPP8 + 8, HWEN, 0);
    pin_field(GPP8, HWEN, 1);
}

/// Whether the bus is the torch process's now (the camera tools refuse while it is).
pub fn held() -> bool {
    HELD.load(Ordering::Acquire)
}

/// The `torch` power switch (process.rs POWERS). On: the bus's clock gates opened if under manual control and closed, its pins to I2C, its clock from the oscillator, the controller brought up (USI mode, reset, timing, auto mode), then HWEN high and 2 ms for the chip to come out of shutdown; the process then programs the torch. Off: HWEN low, and the bus is free again. False when the controller does not come ready.
pub fn power(on: bool) -> bool {
    if !on {
        hwen(false);
        HELD.store(false, Ordering::Release);
        return true;
    }
    for g in PMIC_BUS.gates {
        // WHY: a read into a block whose clock gate is closed can hang the core (camera.rs `state`); the camera tools have always found these open cold, so this only opens one something else closed.
        if !crate::camera::gate_open(rd(g)) {
            wr(g, rd(g) | 1 << 20 | 1 << 21);
        }
    }
    for pin in 0..2 {
        pin_field(GPP16, pin, 2);
        pin_field(GPP16 + 8, pin, 0);
    }
    wr(USI14_MUX, rd(USI14_MUX) & !(1 << 4));
    // The camera's bring-up lets the mux settle for a thousand instructions before touching the controller; 16 µs is longer.
    spin_us(1 << 4);
    crate::hsi2c::bring_up(PMIC_BUS.base, PMIC_BUS.usi_mode, FS_DIV, 0xFF_FFFF);
    if !crate::hsi2c::ready(PMIC_BUS.base, PMIC_BUS.usi_mode) {
        hwen(false);
        return false;
    }
    HELD.store(true, Ordering::Release);
    hwen(true);
    spin_us(1 << 11);
    true
}
