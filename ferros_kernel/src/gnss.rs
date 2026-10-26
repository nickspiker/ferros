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

// ── The transport (GNSS.md "The transport"): the SSI stream under it, Broadcom's packet layer above. ──

/// One SSI transfer, full duplex (bcm_gps_spi.c, SSI_MODE 1, SSI_LEN 2: control byte G#50 = full duplex | 2-byte length | stream | write): out `[G#50, len lo, len hi, data]`, in the same clocks `[status, its len lo, hi, its data]`. The chip's bytes go to `sink`; returns the status byte.
fn ssi(data: &[u8], sink: &mut alloc::vec::Vec<u8>) -> Result<u8, &'static str> {
    let n = data.len();
    let mut tx = alloc::vec![0u8; n + 3];
    tx[0] = 0x50;
    tx[1] = n as u8;
    tx[2] = (n >> 8) as u8;
    tx[3..].copy_from_slice(data);
    let mut rx = alloc::vec![0u8; n + 3];
    crate::spi::GNSS.stream(&tx, &mut rx)?;
    let have = (rx[1] as usize | (rx[2] as usize) << 8).min(n);
    sink.extend_from_slice(&rx[3..3 + have]);
    Ok(rx[0])
}

/// What the chip has waiting (HOST_REQ high): a three-byte probe for its length, then a transfer of that length (bcm_ssi_rx).
fn ssi_read(sink: &mut alloc::vec::Vec<u8>) -> Result<usize, &'static str> {
    let mut rx = [0u8; 3];
    crate::spi::GNSS.stream(&[0x50, 0, 0], &mut rx)?;
    let len = (rx[1] as usize | (rx[2] as usize) << 8).min(8 * 1024 - 3);
    if len == 0 {
        return Ok(0);
    }
    let before = sink.len();
    ssi(&alloc::vec![0u8; len], sink)?;
    Ok(sink.len() - before)
}

/// Broadcom's packet CRC (lhd: a table-driven CRC-8, polynomial G#4D MSB-first, initial 0, over everything after the sequence byte), the result's nibbles swapped.
fn crc(bytes: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in bytes {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { c << 1 ^ 0x4D } else { c << 1 };
        }
    }
    c.rotate_left(4)
}

/// The chip's side of the packet stream, deframed: `b0 00` opens, `b0 01` closes, `b0 03/04/05` stand for G#B0/G#11/G#13. Each frame: sequence, length low, flags, one byte per set flag bit in bit order (G#40 the length's high byte, G#80 the flags' high byte), the payload, the CRC. An ack (flags G#05 or G#25, no payload) names the host sequence it acknowledges in its first field.
struct Deframer {
    raw: alloc::vec::Vec<u8>,
    acked: [bool; 256],
    frames: u32,
    acks: u32,
    crc_bad: u32,
    kinds: alloc::collections::BTreeMap<u16, u32>,
    /// The first 16 and the latest 8 frames as received (sequence, flags, payload head), for `boot`'s report.
    first: alloc::vec::Vec<alloc::string::String>,
    last: alloc::collections::VecDeque<alloc::string::String>,
    /// The on-chip engine's fixes (`Fix`): how many, the first's counter time, the latest.
    fixes: u32,
    first_fix_at: u64,
    fix: Option<Fix>,
}

/// One fix from the chip's own position engine (GNSS.md "The on-chip engine"): the inner message `01 81` with id G#16, a bit stream read as little-endian 32-bit words, most significant bit first (gpsd's reader, G#49C6F0), fields at fixed bit offsets from the message's first byte. Lat/lon in 2⁻²³ degrees, altitude in metres, the error estimate in metres, speed and track in 1/64ths (m/s, degrees), Lms the chip's millisecond clock.
#[derive(Clone, Copy)]
pub struct Fix {
    pub lms: u32,
    pub lat: i32,
    pub lon: i32,
    pub alt: i32,
    pub err: u32,
    pub speed: i32,
    pub track: i32,
}

/// The latest fix, from the service loop's GNSS session.
pub static mut LAST_FIX: Option<Fix> = None;

/// `n` bits at bit `o` of a stream of little-endian 32-bit words, MSB first.
fn bits(b: &[u8], o: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for k in o..o + n {
        let byte = k / 32 * 4 + 3 - k % 32 / 8;
        v = v << 1 | (b.get(byte).copied().unwrap_or(0) >> (7 - k % 8) & 1) as u32;
    }
    v
}
fn sbits(b: &[u8], o: usize, n: usize) -> i32 {
    (bits(b, o, n) << (32 - n)) as i32 >> (32 - n)
}

impl Fix {
    /// From a chip payload (`1f 80 …` then the inner `c0 c1 … c0 c4` frame, G#C0 escaped as `c0 c0`).
    fn parse(p: &[u8]) -> Option<Fix> {
        let c = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1)?;
        let mut m = alloc::vec::Vec::with_capacity(p.len());
        let mut j = c + 2;
        while j < p.len() {
            if p[j] == 0xC0 && j + 1 < p.len() {
                if p[j + 1] != 0xC0 {
                    break;
                }
                j += 1;
            }
            m.push(p[j]);
            j += 1;
        }
        if m.len() < 60 || m[0] != 0x01 || m[1] != 0x81 || m[9] != 0x16 {
            return None;
        }
        let fix = Fix { err: bits(&m, 221, 10), lms: bits(&m, 231, 32), lat: sbits(&m, 311, 32), lon: sbits(&m, 343, 32), alt: sbits(&m, 375, 16), speed: sbits(&m, 407, 16), track: sbits(&m, 439, 16) };
        (fix.lat.unsigned_abs() <= 90 << 23 && fix.lon.unsigned_abs() <= 180 << 23 && fix.err < 1023 && (fix.lat, fix.lon) != (0, 0)).then_some(fix)
    }
    pub fn line(&self) -> alloc::string::String {
        let deg = |v: i32| v as f64 / (1u64 << 23) as f64;
        alloc::format!("lat {:.6} lon {:.6} alt {} m ±{} m, {:.2} m/s track {:.1}°, chip ms {}", deg(self.lat), deg(self.lon), self.alt, self.err, self.speed as f64 / 64.0, self.track as f64 / 64.0, self.lms)
    }
}

impl Deframer {
    fn new() -> Self {
        Deframer { raw: alloc::vec::Vec::new(), acked: [false; 256], frames: 0, acks: 0, crc_bad: 0, kinds: alloc::collections::BTreeMap::new(), first: alloc::vec::Vec::new(), last: alloc::collections::VecDeque::new(), fixes: 0, first_fix_at: 0, fix: None }
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.raw.extend_from_slice(bytes);
        loop {
            let Some(s) = self.raw.windows(2).position(|w| w == [0xB0, 0x00]) else {
                self.raw.clear();
                return;
            };
            let Some(e) = self.raw[s + 2..].windows(2).position(|w| w == [0xB0, 0x01]).map(|e| e + s + 2) else {
                self.raw.drain(..s);
                return;
            };
            let mut f = alloc::vec::Vec::with_capacity(e - s);
            let mut j = s + 2;
            while j < e {
                if self.raw[j] == 0xB0 && j + 1 < e {
                    f.push(match self.raw[j + 1] {
                        3 => 0xB0,
                        4 => 0x11,
                        5 => 0x13,
                        _ => 0,
                    });
                    j += 2;
                } else {
                    f.push(self.raw[j]);
                    j += 1;
                }
            }
            self.raw.drain(..e + 2);
            self.frame(&f);
        }
    }
    fn frame(&mut self, f: &[u8]) {
        if f.len() < 4 {
            return;
        }
        if crc(&f[1..f.len() - 1]) != f[f.len() - 1] {
            self.crc_bad += 1;
            return;
        }
        let shown = alloc::format!("{:02X?}", &f[..f.len().min(24)]);
        if self.first.len() < 16 {
            self.first.push(shown);
        } else {
            if self.last.len() == 8 {
                self.last.pop_front();
            }
            self.last.push_back(shown);
        }
        self.frames += 1;
        let mut flags = f[2] as u16;
        let mut i = 3;
        let mut first = None;
        for bit in 0..16 {
            if bit == 8 && flags & 0x80 == 0 {
                break;
            }
            if flags & 1 << bit != 0 && i < f.len() - 1 {
                if first.is_none() {
                    first = Some(f[i]);
                }
                if bit == 7 {
                    flags |= (f[i] as u16) << 8;
                }
                i += 1;
            }
        }
        // An ack: flags bits 0 and 2 (the acknowledged host sequence first) and no payload; under ferros the chip also sets bit 5 (a third field, a window or credit count), Android's capture showed G#05 alone.
        if flags & 0x05 == 0x05 && i == f.len() - 1 {
            self.acks += 1;
            if let Some(seq) = first {
                self.acked[seq as usize] = true;
            }
            return;
        }
        let p = &f[i..f.len() - 1];
        if p.len() >= 2 {
            *self.kinds.entry((p[0] as u16) << 8 | p[1] as u16).or_insert(0) += 1;
        }
        if let Some(fix) = Fix::parse(p) {
            if self.fixes == 0 {
                self.first_fix_at = crate::platform::cntpct();
            }
            self.fixes += 1;
            self.fix = Some(fix);
            unsafe { LAST_FIX = Some(fix) };
        }
    }
}

// ── Aiding: what the chip is told before it looks at the sky (GNSS.md "Aiding") ──
//
// The replay carries gpsd's aiding from the capture: the time (rewritten below), the last position, the ephemerides gpsd kept, and an LTO file.
// LTO ("Long Term Orbits") is Broadcom's (from Global Locate, hence glpals.com and gpsd's Gl* names): every satellite's orbit and clock predicted days ahead, so a start without fresh ephemerides takes seconds instead of a cold search.
// Where it comes from, as measured 2026-10-26: the Pixel's /data/vendor/gps/lto2.dat is byte-for-byte the file at http://gllto2.glpals.com/7day/v5/latest/lto2.dat (~188 KB).
// The same server answers over plain HTTP and HTTPS, with no key, account, signature or terms shown; it is regenerated a few times a day (Last-Modified) and gpsd re-fetches daily (gps.xml HttpSyncLto, LtoSyncThresholdDays=1; ltoStatus.txt holds the unix time of the last fetch).
// Variants on the same path: 7day/v2 59 KB, v3 103 KB, v4 146 KB, v5 188 KB, 4day/v5 123 KB (gpsd's compiled-in URL is the 4day one; growing with the constellations covered, by inference); http://gllto.glpals.com/rtistatus5.dat (240 bytes, real-time satellite health); gpsd's rto.dat (real-time orbits) URL answers 404.
// The file as read: magic ff ca de 00, u32 length (file − 8), u32 1, 16 bytes that change per file (likely the "LTO file id" gpsd logs), u32 G#75A0DE5 and u32 5292 (the header's end) in every variant, u32 70, then 70 × (index u32, offset u32) into G#44-byte records before the bulk; gpsd logs "LTO under license to 'google', license #69" when it reads one.
// Our stance: the file is published openly, unposted and unauthenticated, as the Pixel itself fetches it; ferros fetches it the same way for its own chip, and if Broadcom or Google ask, this comes down.
// Not done yet: feeding a fresh file to the chip (gpsd's upload is the `1e83`/`1e82` messages in the replay; one capture with the exact file uploaded shows their layout).

/// GPS time now from the wall clock (rtc.rs): milliseconds since the GPS epoch (1980-01-06), UTC plus the 18 leap seconds; None before the RTC has been read.
fn gps_ms_now() -> Option<u64> {
    let base = crate::rtc::BASE.load(core::sync::atomic::Ordering::Relaxed);
    if base == 0 {
        return None;
    }
    let (f, t) = (crate::platform::cntfrq(), base.wrapping_add(crate::platform::cntpct()));
    Some((t / f * 1000 + t % f * 1000 / f).checked_sub(315_964_800_000)? + 18_000)
}

/// A record carrying gpsd's time aiding, rewritten with the time now: in the capture it is the capture's time, and a replay hours later would tell the chip a wrong one. Two messages carry it (GNSS.md): `87 81` (the GPS week u16 then the time of week in ms u32, 8 bytes after the inner `c0 c1`) and `d0 81` (position and time; the week 58 bytes after it, two zero bytes, then the time of week). The inner checksum (the 16-bit sum of the class, length and body, 6 bytes after `c0 c1`) is adjusted by the bytes' difference; a time of week whose bytes or new checksum would hold G#C0 (the inner escape) moves on a millisecond. Then the packet's CRC is redone and the frame escaped again. None when the record holds neither message or the clock is not set.
fn patch_time(rec: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    let now = gps_ms_now()?;
    let mut out = alloc::vec::Vec::with_capacity(rec.len() + 8);
    let mut changed = false;
    let mut i = 0;
    while i < rec.len() {
        let (true, Some(e)) = (rec[i..].starts_with(&[0xB0, 0x00]), rec[i + 2.min(rec.len() - i)..].windows(2).position(|w| w == [0xB0, 0x01]).map(|e| e + i + 2)) else {
            out.push(rec[i]);
            i += 1;
            continue;
        };
        let mut f = alloc::vec::Vec::with_capacity(e - i);
        let mut j = i + 2;
        while j < e {
            if rec[j] == 0xB0 && j + 1 < e {
                f.push(match rec[j + 1] { 3 => 0xB0, 4 => 0x11, _ => 0x13 });
                j += 2;
            } else {
                f.push(rec[j]);
                j += 1;
            }
        }
        i = e + 2;
        if patch_frame(&mut f, now) {
            changed = true;
        }
        out.extend_from_slice(&[0xB0, 0x00]);
        for &b in &f {
            match b {
                0xB0 => out.extend_from_slice(&[0xB0, 0x03]),
                0x11 => out.extend_from_slice(&[0xB0, 0x04]),
                0x13 => out.extend_from_slice(&[0xB0, 0x05]),
                _ => out.push(b),
            }
        }
        out.extend_from_slice(&[0xB0, 0x01]);
    }
    changed.then_some(out)
}

fn patch_frame(f: &mut [u8], now: u64) -> bool {
    if f.len() < 4 {
        return false;
    }
    let mut flags = f[2] as u16;
    let mut k = 3;
    for bit in 0..16 {
        if bit == 8 && flags & 0x80 == 0 {
            break;
        }
        if flags & 1 << bit != 0 && k < f.len() - 1 {
            if bit == 7 {
                flags |= (f[k] as u16) << 8;
            }
            k += 1;
        }
    }
    let end = f.len() - 1;
    let p = &mut f[k..end];
    let Some(c) = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1) else {
        return false;
    };
    let (week_at, tow_at) = match p.get(c + 2..c + 4) {
        Some([0x87, 0x81]) => (c + 8, c + 10),
        Some([0xD0, 0x81]) => (c + 58, c + 62),
        _ => return false,
    };
    // Offsets hold only if nothing before the fields was escaped.
    if p.len() < tow_at + 4 || p[c + 2..tow_at + 4].contains(&0xC0) {
        return false;
    }
    let old: u32 = p[week_at..week_at + 2].iter().chain(&p[tow_at..tow_at + 4]).map(|&b| b as u32).sum();
    let ck = u16::from_le_bytes([p[c + 6], p[c + 7]]);
    let week = (now / 604_800_000) as u16;
    let mut tow = (now % 604_800_000) as u32;
    for _ in 0..16 {
        let (w, t) = (week.to_le_bytes(), tow.to_le_bytes());
        let new: u32 = w.iter().chain(&t).map(|&b| b as u32).sum();
        let ck2 = (ck as u32).wrapping_sub(old).wrapping_add(new) as u16;
        if !w.iter().chain(&t).chain(&ck2.to_le_bytes()).any(|&b| b == 0xC0) {
            p[week_at..week_at + 2].copy_from_slice(&w);
            p[tow_at..tow_at + 4].copy_from_slice(&t);
            p[c + 6..c + 8].copy_from_slice(&ck2.to_le_bytes());
            f[end] = crc(&f[1..end]);
            return true;
        }
        tow += 1;
    }
    false
}

/// The host frames in a record that want an ack (flags bit G#02): their sequence numbers.
fn reliable_seqs(rec: &[u8]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    let mut i = 0;
    while i + 4 < rec.len() {
        if rec[i] == 0xB0 && rec[i + 1] == 0x00 {
            // The sequence and flags bytes, unescaped.
            let un = |k: usize| -> (u8, usize) { if rec[k] == 0xB0 { (match rec[k + 1] { 3 => 0xB0, 4 => 0x11, _ => 0x13 }, 2) } else { (rec[k], 1) } };
            let (seq, a) = un(i + 2);
            let (_len, b) = un(i + 2 + a);
            let (flags, _) = un(i + 2 + a + b);
            if flags & 0x02 != 0 {
                out.push(seq);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// `diag gnss boot`: the chip up from cold and its patch downloaded by ferros — lhd's own opening replayed (`replay`: the vault's `gnss.replay`, records `[len u16][gap ms u16][bytes]` cut from a capture of lhd: the autobaud bursts, the transport sync, and the 239-packet patch download; Google's patch is in it, so it lives in the vault and on Harbor, never in the repo). Each record goes out through the SSI layer; a record with reliable frames waits (up to 300 ms) for the chip to ack every one before the next, reading whenever HOST_REQ is up; the rest wait lhd's own gap (at most 200 ms). Then ten seconds of whatever the chip sends, and its rate.
pub fn boot(replay: &[u8], tail_s: u64, out: &mut alloc::vec::Vec<u8>) {
    if replay.len() < 4 || &replay[..4] != b"GNR1" {
        out.extend_from_slice(b"no replay (vault key gnss.replay, GNR1 records)\n");
        return;
    }
    up(out);
    set_dat(MCU_REQ, true);
    let mut d = Deframer::new();
    let mut rx = alloc::vec::Vec::new();
    let (mut recs, mut waits, mut late, mut errs, mut timed) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let f = crate::platform::cntfrq();
    let mut i = 4;
    let t_all = crate::platform::cntpct();
    while i + 4 <= replay.len() {
        let len = u16::from_le_bytes([replay[i], replay[i + 1]]) as usize;
        let gap = u16::from_le_bytes([replay[i + 2], replay[i + 3]]) as u64;
        let rec = &replay[i + 4..(i + 4 + len).min(replay.len())];
        i += 4 + len;
        recs += 1;
        crate::platform::pet_watchdogs();
        rx.clear();
        let patched = patch_time(rec);
        if patched.is_some() {
            timed += 1;
        }
        if ssi(patched.as_deref().unwrap_or(rec), &mut rx).is_err() {
            errs += 1;
        }
        d.feed(&rx);
        let want = reliable_seqs(rec);
        let t0 = crate::platform::cntpct();
        // lhd's own gap to the next record (up to a second), and for reliable packets at least 60 ms for their acks: the session after the download is paced in real time, as gpsd's configuration and start went.
        let limit = f / 1000 * gap.min(1000).max(if want.is_empty() { 0 } else { 60 });
        let pace = f / 1000 * gap.min(1000);
        while crate::platform::cntpct().wrapping_sub(t0) < limit {
            if !want.is_empty() && want.iter().all(|&s| d.acked[s as usize]) && crate::platform::cntpct().wrapping_sub(t0) >= pace {
                break;
            }
            if get(HOST_REQ) {
                rx.clear();
                if ssi_read(&mut rx).is_err() {
                    errs += 1;
                }
                d.feed(&rx);
            }
        }
        if !want.is_empty() {
            waits += 1;
            if !want.iter().all(|&s| d.acked[s as usize]) {
                late += 1;
            }
        }
    }
    let t0 = crate::platform::cntpct();
    let mut tail_bytes = 0usize;
    let mut shown = 0;
    while crate::platform::cntpct().wrapping_sub(t0) < tail_s * f {
        crate::platform::pet_watchdogs();
        if get(HOST_REQ) {
            rx.clear();
            let _ = ssi_read(&mut rx);
            tail_bytes += rx.len();
            d.feed(&rx);
            // The first fixes as they come, then only the count and the latest.
            if d.fixes > shown && shown < 4 {
                shown = d.fixes;
                if let Some(x) = d.fix {
                    out.extend_from_slice(alloc::format!("fix {shown} at {} ms after power-up: {}\n", crate::platform::cntpct().wrapping_sub(t_all) * 1000 / f, x.line()).as_bytes());
                }
            }
        }
    }
    out.extend_from_slice(alloc::format!("the last {tail_s} s: {tail_bytes} bytes from the chip ({} B/s)\n", tail_bytes as u64 / tail_s.max(1)).as_bytes());
    match d.fix {
        Some(x) => out.extend_from_slice(alloc::format!("{} fixes from the chip's own engine, the first {} ms after power-up; the latest: {}\n", d.fixes, d.first_fix_at.wrapping_sub(t_all) * 1000 / f, x.line()).as_bytes()),
        None => out.extend_from_slice(b"no fix from the chip\n"),
    }
    out.extend_from_slice(alloc::format!("time aiding: {timed} records rewritten with the time now ({})\n", if gps_ms_now().is_some() { "from the RTC" } else { "no wall clock: the capture's time went" }).as_bytes());
    let ms = crate::platform::cntpct().wrapping_sub(t_all) * 1000 / f;
    out.extend_from_slice(alloc::format!("replayed {recs} records in {ms} ms ({errs} SPI errors); {waits} waited for acks, {late} not acked in time; the chip: {} frames, {} acks, {} CRC failures\n", d.frames, d.acks, d.crc_bad).as_bytes());
    out.extend_from_slice(b"the chip's message kinds (first two payload bytes: count):");
    for (k, n) in &d.kinds {
        out.extend_from_slice(alloc::format!(" {k:04X}:{n}").as_bytes());
    }
    out.extend_from_slice(b"\n");
    for (k, l) in d.first.iter().enumerate() {
        out.extend_from_slice(alloc::format!("first {k}: {l}\n").as_bytes());
    }
    for l in &d.last {
        out.extend_from_slice(alloc::format!("last: {l}\n").as_bytes());
    }
    set_dat(MCU_REQ, false);
    // The chip keeps navigating: the service loop goes on reading it (`poll`) until `diag gnss off`.
    d.first.clear();
    d.last.clear();
    unsafe { SESSION = Some(d) };
}

/// The session `boot` left running, read from the service loop.
static mut SESSION: Option<Deframer> = None;

/// From the service loop: while a session runs and the chip has data (HOST_REQ), wake it (MCU_REQ, up to 4 ms for MCU_RESP: no longer, the loop has other work) and read; its fixes land in `LAST_FIX`.
pub fn poll() {
    let Some(d) = (unsafe { (*(&raw mut SESSION)).as_mut() }) else {
        return;
    };
    if !get(HOST_REQ) {
        return;
    }
    set_dat(MCU_REQ, true);
    let (f, t0) = (crate::platform::cntfrq(), crate::platform::cntpct());
    while !get(MCU_RESP) && crate::platform::cntpct().wrapping_sub(t0) < f / 250 {
        core::hint::spin_loop();
    }
    let mut rx = alloc::vec::Vec::new();
    if get(MCU_RESP) && ssi_read(&mut rx).is_ok() {
        d.feed(&rx);
    }
    set_dat(MCU_REQ, false);
}

/// `diag gnss fix`: the running session's fixes, the latest and its age.
pub fn fix(out: &mut alloc::vec::Vec<u8>) {
    let Some(d) = (unsafe { (*(&raw mut SESSION)).as_ref() }) else {
        out.extend_from_slice(b"no session (diag gnss boot starts one)\n");
        return;
    };
    out.extend_from_slice(alloc::format!("{} fixes, {} frames, {} CRC failures\n", d.fixes, d.frames, d.crc_bad).as_bytes());
    if let Some(x) = d.fix {
        out.extend_from_slice(alloc::format!("latest: {}\n", x.line()).as_bytes());
    }
}

/// `diag gnss off`: the session dropped, nSTANDBY low, the three rails off.
pub fn off(out: &mut alloc::vec::Vec<u8>) {
    unsafe { SESSION = None };
    set_dat(MCU_REQ, false);
    set_dat(NSTANDBY, false);
    for (name, reg, _, step) in RAILS {
        if step == 0 {
            continue;
        }
        let r = crate::acpm::update(crate::acpm::SUB, crate::acpm::T_PMIC, reg, 0, 0x80);
        out.extend_from_slice(alloc::format!("{name} off: {r:?}\n").as_bytes());
    }
}
