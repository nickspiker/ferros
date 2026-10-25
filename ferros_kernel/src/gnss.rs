//! GNSS: the Broadcom BCM4776 (L1 + L5) on spi_21 (HSI0 USI3, G#111E_0000, mode 3), Google's bcm47765 driver (google-modules/gps/broadcom/bcm47765) and the board's zuma-board-gps.dtsi; the plan and its evidence in GNSS.md.
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

fn pin(p: (usize, u32)) -> alloc::string::String {
    let (bank, n) = p;
    alloc::format!("con {} dat {} pud {}", (rd(bank) >> (n * 4)) & 0xF, (rd(bank + 4) >> n) & 1, (rd(bank + 8) >> (n * 4)) & 0xF)
}

/// `diag gnss`: the rails, the four lines and the SPI port's clock, read only.
pub fn state(out: &mut alloc::vec::Vec<u8>) {
    for (name, reg, base, step) in RAILS {
        let r = crate::acpm::read(crate::acpm::SUB, crate::acpm::T_PMIC, reg);
        let v = match r {
            Ok(c) if step != 0 => alloc::format!("G#{c:02X}: {} , {} mV", if c & 0x80 != 0 { "on" } else { "off" }, base + (c & 0x3F) as u32 * step / 1000),
            Ok(c) => alloc::format!("G#{c:02X}"),
            Err(e) => alloc::format!("error G#{e:X}"),
        };
        out.extend_from_slice(alloc::format!("{name} (G#{reg:02X}): {v}\n").as_bytes());
    }
    for (name, p) in [("HOST_REQ gpa5-3", HOST_REQ), ("MCU_REQ gpp4-3", MCU_REQ), ("MCU_RESP gph3-2", MCU_RESP), ("nSTANDBY gph3-3", NSTANDBY)] {
        out.extend_from_slice(alloc::format!("{name}: {}\n", pin(p)).as_bytes());
    }
    out.extend_from_slice(alloc::format!("spi_21: {}\n", crate::spi::GNSS.describe()).as_bytes());
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
fn set_con(p: (usize, u32), v: u32) {
    let (bank, n) = p;
    wr(bank, (rd(bank) & !(0xF << (n * 4))) | v << (n * 4));
}
fn set_dat(p: (usize, u32), high: bool) {
    let (bank, n) = p;
    wr(bank + 4, if high { rd(bank + 4) | 1 << n } else { rd(bank + 4) & !(1 << n) });
}
fn get(p: (usize, u32)) -> bool {
    (rd(p.0 + 4) >> p.1) & 1 == 1
}

/// The chip awake: MCU_REQ up, MCU_RESP answering (as bcm_gps_spi.c: up to 100 polls of 1–2 ms, MCU_REQ pulsed again every 20). Returns the polls it took.
fn wake() -> Option<u32> {
    set_dat(MCU_REQ, true);
    for i in 0..100 {
        if get(MCU_RESP) {
            return Some(i);
        }
        if i % 20 == 19 {
            set_dat(MCU_REQ, false);
            ms(1);
            set_dat(MCU_REQ, true);
        }
        ms(2);
    }
    None
}

/// An indirect read of a chip address (bcm_gps_regs.c): [G#80, G#24, 5, address LE, G#02], then [G#A0, eleven zeros] full duplex; the reply echoes the address in bytes 1–4 and holds the value in 5–8.
fn indirect(addr: u32) -> Result<([u8; 12], u32), &'static str> {
    let a = addr.to_le_bytes();
    let cmd = [0x80, 0x24, 5, a[0], a[1], a[2], a[3], 0x02];
    let mut sink = [0u8; 8];
    crate::spi::GNSS.duplex(crate::spi::Cs::Native, &cmd, &mut sink)?;
    let mut tx = [0u8; 12];
    tx[0] = 0xA0;
    let mut rx = [0u8; 12];
    crate::spi::GNSS.duplex(crate::spi::Cs::Native, &tx, &mut rx)?;
    Ok((rx, u32::from_le_bytes([rx[5], rx[6], rx[7], rx[8]])))
}

/// `diag gnss up`: lines set (MCU_REQ and nSTANDBY outputs low; MCU_RESP and HOST_REQ inputs), the three rails on at the voltages already programmed, the SPI port up in mode 3, nSTANDBY high and 250 ms (lhd.conf CustGpioDelayMs), then the wake handshake and an indirect read of HSI_CTRL and HSI_RESETN, and a second of watching HOST_REQ.
pub fn up(out: &mut alloc::vec::Vec<u8>) {
    set_con(MCU_REQ, 1);
    set_dat(MCU_REQ, false);
    set_con(NSTANDBY, 1);
    set_dat(NSTANDBY, false);
    set_con(MCU_RESP, 0);
    set_con(HOST_REQ, 0);
    for (name, reg, _, step) in RAILS {
        if step == 0 {
            continue;
        }
        let r = crate::acpm::update(crate::acpm::SUB, crate::acpm::T_PMIC, reg, 0x80, 0x80).and_then(|_| crate::acpm::read(crate::acpm::SUB, crate::acpm::T_PMIC, reg));
        out.extend_from_slice(alloc::format!("{name} on: {:?}\n", r.map(|c| alloc::format!("G#{c:02X}"))).as_bytes());
    }
    ms(5);
    crate::spi::GNSS.up();
    set_dat(NSTANDBY, true);
    ms(250);
    out.extend_from_slice(alloc::format!("nSTANDBY high; HOST_REQ {}, MCU_RESP {}\n", get(HOST_REQ), get(MCU_RESP)).as_bytes());
    let w = wake();
    out.extend_from_slice(alloc::format!("wake: MCU_RESP after {w:?} polls\n").as_bytes());
    for (name, a) in [("HSI_CTRL", 0x4010_4080u32), ("HSI_RESETN", 0x4010_4090)] {
        match indirect(a) {
            Ok((rx, v)) => out.extend_from_slice(alloc::format!("{name} (G#{a:X}): raw {rx:02X?}, value G#{v:X}\n").as_bytes()),
            Err(e) => out.extend_from_slice(alloc::format!("{name}: {e}\n").as_bytes()),
        }
    }
    set_dat(MCU_REQ, false);
    let t0 = crate::platform::cntpct();
    let f = crate::platform::cntfrq();
    let mut rose = None;
    while crate::platform::cntpct().wrapping_sub(t0) < f {
        if get(HOST_REQ) {
            rose = Some(crate::platform::cntpct().wrapping_sub(t0) * 1000 / f);
            break;
        }
    }
    out.extend_from_slice(alloc::format!("HOST_REQ within 1 s: {rose:?} ms\n").as_bytes());
    state(out);
}
