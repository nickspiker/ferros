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
}

impl Deframer {
    fn new() -> Self {
        Deframer { raw: alloc::vec::Vec::new(), acked: [false; 256], frames: 0, acks: 0, crc_bad: 0, kinds: alloc::collections::BTreeMap::new(), first: alloc::vec::Vec::new(), last: alloc::collections::VecDeque::new() }
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
    }
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

/// `diag gnss boot`: the chip up from cold and its patch downloaded by ferros — lhd's own opening replayed (`replay`: the vault's `gnss.replay`, records `[len u16][gap ms u16][bytes]` cut from a capture of lhd: the autobaud bursts, the transport sync, and the 239-packet patch download; Google's patch is in it, so it lives in the vault and on Harbor, never in the repo). Each record goes out through the SSI layer; a record with reliable frames waits (up to 300 ms) for the chip to ack every one before the next, reading whenever HOST_REQ is up; the rest wait lhd's own gap (at most 200 ms). Then two seconds of whatever the chip sends.
pub fn boot(replay: &[u8], out: &mut alloc::vec::Vec<u8>) {
    if replay.len() < 4 || &replay[..4] != b"GNR1" {
        out.extend_from_slice(b"no replay (vault key gnss.replay, GNR1 records)\n");
        return;
    }
    up(out);
    set_dat(MCU_REQ, true);
    let mut d = Deframer::new();
    let mut rx = alloc::vec::Vec::new();
    let (mut recs, mut waits, mut late, mut errs) = (0u32, 0u32, 0u32, 0u32);
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
        if ssi(rec, &mut rx).is_err() {
            errs += 1;
        }
        d.feed(&rx);
        let want = reliable_seqs(rec);
        let t0 = crate::platform::cntpct();
        let limit = if want.is_empty() { f / 1000 * gap.min(200) } else { f / 1000 * 60 };
        while crate::platform::cntpct().wrapping_sub(t0) < limit {
            if !want.is_empty() && want.iter().all(|&s| d.acked[s as usize]) {
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
    while crate::platform::cntpct().wrapping_sub(t0) < 2 * f {
        crate::platform::pet_watchdogs();
        if get(HOST_REQ) {
            rx.clear();
            let _ = ssi_read(&mut rx);
            d.feed(&rx);
        }
    }
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
}
