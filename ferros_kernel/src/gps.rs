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

/// The rails: (name, sub-PMIC control register, its voltage as (base mV, step µV)).
const RAILS: [(&str, u32, u32, u32); 4] = [("L9S GNSS core", 0x37, 725, 12_500), ("L10S GNSS RF", 0x38, 700, 25_000), ("L11S GNSS aux", 0x39, 700, 25_000), ("BUCK6S (core feed on this board?)", 0x18, 0, 0)];


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
pub fn power(on: bool) -> bool {
    set_dat(MCU_REQ, false);
    set_dat(NSTANDBY, false);
    if on {
        set_con(MCU_REQ, 1);
        set_con(NSTANDBY, 1);
        set_con(MCU_RESP, 0);
        set_con(HOST_REQ, 0);
    }
    let mut ok = true;
    for (_, reg, _, step) in RAILS {
        if step != 0 {
            ok &= crate::acpm::update(crate::acpm::SUB, crate::acpm::T_PMIC, reg, if on { 0x80 } else { 0 }, 0x80).is_ok();
        }
    }
    if on {
        ms(5);
        crate::spi::GPS.up();
    }
    ok
}

/// `diag gps off`: the chip powered down (the driver's own STOP does the same; this is for a driver that died with the chip on).
pub fn off(out: &mut alloc::vec::Vec<u8>) {
    let ok = power(false);
    out.extend_from_slice(alloc::format!("GPS rails off: {ok}\n").as_bytes());
}
