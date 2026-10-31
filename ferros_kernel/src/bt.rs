//! Bluetooth's power (docs/BT.md): the BCM4398's Bluetooth core, on the APM's USI1 UART (serial_18, G#155D_0000, `/dev/ttySAC18` in Android). The driver is the `bt` process (payloads/bt), which owns the UART; this is what it cannot do: the USI's mode word in the APM sysreg and the chip's power lines, in the order Google's `nitrous` driver uses them (nitrous.c `nitrous_rfkill_set_power`).
//!
//! Lines (zuma-board-bluetooth.dtsi): BT_REG_ON gpn8-0 and DEV_WAKE gpn9-0 (outputs), HOST_WAKE gpn6-0 (input, pulled down), BLE_DBO gpp24-3 (a flip-flop's clock: pulsed at every REG_ON change so the chip's off-mode latch follows). The UART's clocks are already running from boot (CMU_APM mux 1, divider 2, gates on automatic: measured 2026-10-31), and its pads have no pin modes to set. Bluetooth on wants WiFi's REG_ON up first (the coordinator's WLAN_ON_FIRST_ALWAYS), which the WiFi driver holds while it runs.

/// SYSREG_APM's USI1 mode word (`samsung,usi-offset` 4 from G#1542_04E0): 1 = UART.
const USI_MODE: usize = 0x1542_04E4;
const GPN6: usize = 0x1506_00C0;
const GPN8: usize = 0x1506_0100;
const GPN9: usize = 0x1506_0120;
const GPP24: usize = 0x10C4_0060;
const DBO_PIN: u32 = 3;
/// WL_REG_ON: gph3-4 (gpio-hsi2, the bank at G#1304_0000), WiFi's power line, read only here.
const GPH3: usize = 0x1304_0000;

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
fn us(n: u64) {
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < crate::platform::cntfrq() / 1_000_000 * n + 1 {
        core::hint::spin_loop();
    }
}
fn pin_field(reg: usize, pin: u32, val: u32) {
    wr(reg, (rd(reg) & !(0xF << (pin * 4))) | (val & 0xF) << (pin * 4));
}
fn dat(bank: usize, pin: u32, high: bool) {
    let d = rd(bank + 4);
    wr(bank + 4, if high { d | 1 << pin } else { d & !(1 << pin) });
}
/// The flip-flop's clock: low, high, low, a microsecond apart (`toggle_dbo_ff`).
fn dbo_pulse() {
    dat(GPP24, DBO_PIN, false);
    us(1);
    dat(GPP24, DBO_PIN, true);
    us(1);
    dat(GPP24, DBO_PIN, false);
}

/// On: the UART's USI in UART mode, the lines' modes (DBO first driven high, as nitrous's first rfkill call does), then REG_ON low with a DBO pulse, 30 ms, REG_ON high with a DBO pulse, DEV_WAKE high (the chip awake: ferros does not run its low-power protocol yet), and 20 ms for the chip's ROM. Off: DEV_WAKE low, REG_ON low with a DBO pulse, 20 ms. False if WiFi's REG_ON is low (the chip's Bluetooth core is not to be powered alone).
pub fn power(on: bool) -> bool {
    let ms = |n: u64| us(n * 1000);
    if on && rd(GPH3 + 4) & 1 << 4 == 0 {
        return false;
    }
    if on {
        wr(USI_MODE, (rd(USI_MODE) & !7) | 1);
        dat(GPP24, DBO_PIN, true);
        pin_field(GPP24, DBO_PIN, 1);
        pin_field(GPN8, 0, 1);
        pin_field(GPN9, 0, 1);
        pin_field(GPN6, 0, 0);
        dat(GPN8, 0, false);
        dbo_pulse();
        ms(30);
        dat(GPN8, 0, true);
        dbo_pulse();
        dat(GPN9, 0, true);
    } else {
        dat(GPN9, 0, false);
        dat(GPN8, 0, false);
        dbo_pulse();
    }
    ms(20);
    true
}
