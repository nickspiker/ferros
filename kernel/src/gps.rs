//! GPS: the Broadcom BCM4776 (L1 + L5) on spi_21 (HSI0 USI3, G#111E_0000, mode 3), Google's bcm47765 driver (google-modules/gps/broadcom/bcm47765) and the board's zuma-board-gps.dtsi; the plan and its evidence in GPS.md.
//!
//! The chip runs from ROM plus a RAM patch the host downloads every boot (/vendor/firmware/SensorHub.patch). Four lines: HOST_REQ gpa5-3 (in, the chip has data), MCU_REQ gpp4-3 (out, the AP wants the chip awake), MCU_RESP gph3-2 (in, the chip is awake), nSTANDBY gph3-3 (out, high = run; its only power/reset control). Three sub-PMIC rails: L9S_GNSS_CORE (G#37), L10S_GNSS_RF (G#38), L11S_GNSS_AUX (G#39) — enable bit 7, voltage bits 5:0.

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}

/// GPIO banks (CON +0, DAT +4, PUD +8): gpa5 in FAR_ALIVE, gpp4 in PERIC0, gph3 in HSI2.
const GPA5: usize = 0x154E_0000;
const GPP4: usize = 0x1084_0080;
const GPH3: usize = 0x1304_0000;
const HOST_REQ: (usize, u32) = (GPA5, 3);
const MCU_REQ: (usize, u32) = (GPP4, 3);
const MCU_RESP: (usize, u32) = (GPH3, 2);
const NSTANDBY: (usize, u32) = (GPH3, 3);

/// The three rails' sub-PMIC control registers: L9S_GNSS_CORE, L10S_GNSS_RF, L11S_GNSS_AUX (their voltages ABL's; the bring-up's readback of them and of BUCK6S left with `diag gps off`, 2026-11-10).
const RAILS: [u32; 3] = [0x37, 0x38, 0x39];


fn wr(a: usize, v: u32) {
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
fn ms(n: u64) {
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct().wrapping_sub(t0) < crate::platform::cntfrq() / 1000 * n {
        core::hint::spin_loop();
    }
}
fn set_con(p: (usize, u32), v: u32) {
    let (bank, n) = p;
    wr(bank, (rd(bank) & !(0xF << (n * 4))) | v << (n * 4));
}
fn set_dat(p: (usize, u32), high: bool) {
    let (bank, n) = p;
    wr(bank + 4, if high { rd(bank + 4) | 1 << n } else { rd(bank + 4) & !(1 << n) });
}
/// The GPS driver's power switch (process.rs `POWERS`, `SYS_POWER`): on = the lines' modes (MCU_REQ and nSTANDBY outputs, low; MCU_RESP and HOST_REQ inputs), the three rails on at their programmed voltages, the SPI port's clocks and USI mode; the driver then raises nSTANDBY itself. Off = MCU_REQ and nSTANDBY low, the rails off.
/// On the 8a the same switch is the KEPLER's (`kepler_power`), whose driver (payloads/gnss_kepler) takes the same grants.
pub fn power(on: bool) -> bool {
    if crate::board::akita() {
        return kepler_power(on);
    }
    set_dat(MCU_REQ, false);
    set_dat(NSTANDBY, false);
    if on {
        set_con(MCU_REQ, 1);
        set_con(NSTANDBY, 1);
        set_con(MCU_RESP, 0);
        set_con(HOST_REQ, 0);
    }
    let mut ok = true;
    for reg in RAILS {
        ok &= crate::acpm::update(crate::acpm::SUB, crate::acpm::T_PMIC, reg, if on { 0x80 } else { 0 }, 0x80).is_ok();
    }
    if on {
        ms(5);
        crate::spi::GPS.up();
    }
    ok
}

/// The 8a's GNSS: the KEPLER core of its S5300 modem (docs/GPS.md "akita's KEPLER"), on the same spi_21 and the same two pins as husky's HOST_REQ and MCU_REQ (zuma-cp-s5300-gnss.dtsi): gpp4-3 AP to GNSS (an output, no pull), gpa5-3 GNSS to AP (its interrupt, pull-down; the driver makes it an EINT).
/// No rails and no reset here: the core is powered, reset and given its firmware by the CP (gnssd's `pcie_power_on`, `pcie_release_reset`, `pcie_load_firmware` over the modem's `gnss_boot` channel); the 8a's device tree gives it neither husky's gph3 lines nor the sub-PMIC's GNSS LDOs, so neither is touched.
/// On = the lines' modes and the port's clocks, USI mode and setup, then its clock halved: 3.07 MHz on the wire, under the DT's `spi-max-frequency` of 3.125 MHz. Off = AP to GNSS low.
fn kepler_power(on: bool) -> bool {
    const AP2GNSS: (usize, u32) = MCU_REQ;
    const GNSS2AP: (usize, u32) = HOST_REQ;
    set_dat(AP2GNSS, false);
    if on {
        set_con(AP2GNSS, 1);
        set_pud(AP2GNSS, 0);
        set_con(GNSS2AP, 0);
        set_pud(GNSS2AP, 1);
        crate::spi::GPS.up();
        crate::spi::GPS.halved();
    }
    true
}

/// A pin's pull, 4 bits a pin at +8: 0 none, 1 down.
fn set_pud(p: (usize, u32), v: u32) {
    let (bank, n) = p;
    wr(bank + 8, (rd(bank + 8) & !(0xF << (n * 4))) | v << (n * 4));
}
