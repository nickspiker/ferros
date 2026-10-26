//! The GNSS driver (GNSS.md): an EL0 process that runs the BCM4776 — its SPI transport, the session replay with ferros's own aiding, the chip's on-chip fixes — and serves them on the `gnss` endpoint. What was the kernel's `gnss.rs` session, as a process.
//!
//! The kernel keeps only what a process cannot do: the chip's three rails (ACPM), the SPI port's clocks and USI mode, and the lines' pin modes, behind the `gnss` power capability (`SYS_POWER`, kernel gnss.rs `power`). Everything else is here, through MMIO grants: the port's registers, and the data bits of the four lines (HOST_REQ gpa5-3, MCU_REQ gpp4-3, MCU_RESP gph3-2, nSTANDBY gph3-3).
//!
//! The session: the vault's `gnss.replay` (or `gnss.replay.sky`, the same without the LTO orbits) is gpsd's on-chip session cut from a capture — the patch download, the second firmware stage, the configuration, the aiding — played record by record, each reliable packet waiting for the chip's ack. Before it goes out, the aiding is ferros's own: the time from the RTC, the position from the last fix (`gnss.last`). Then the chip navigates alone and sends a fix a second; the latest is kept, and written back to `gnss.last` with the first fix and every 64 s.
//!
//! Spawn: ferros-bridge spawn gnss.bin <flags> --name gnss --serve gnss --hw usi3-spi --hw gpio-far-alive --hw gpio-gpp0 --hw gpio-hsi2 --store gnss --power gnss --irq gnss-host-req
//! Grant slots, in that order: 0 console, 1 the `gnss` endpoint (serve), 2 the SPI port, 3 gpio-far-alive, 4 gpio-gpp0 (PERIC0), 5 gpio-hsi2, 6 the vault scope `gnss.`, 7 the power switch, 8 HOST_REQ's interrupt. Input byte 0: bit 0 = the sky replay.
//!
//! The `gnss` endpoint, first byte the op:
//! - `0` FIX: `valid u8`, `fixes u32`, `age ms u32` (since the latest), `lat i32`, `lon i32` (2⁻²³ degrees), `alt i32` (m), `err u32` (m), `speed i32`, `track i32` (1/64 m/s, 1/64 °) — little-endian, 33 bytes.
//! - `1` STOP: the chip powered down, the last fix kept; the process exits after replying.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, store_get_at, store_set, NOTHING};

const S_GNSS: u64 = 1;
const S_SPI: u64 = 2;
const S_FAR: u64 = 3;
const S_PERIC0: u64 = 4;
const S_HSI2: u64 = 5;
const S_STORE: u64 = 6;
const S_POWER: u64 = 7;
/// HOST_REQ's interrupt (kernel `gnss-host-req`): the loop sleeps on it instead of polling the line, which missed the chip's requests (2026-10-26: a fix, then nothing).
const S_IRQ: u64 = 8;
/// The far-alive bank's external-interrupt registers (pinctrl-exynos.h; gpa5 is the bank's only one): control (a nibble per pin, 1 = high level), mask, pending.
const EINT_CON: u64 = 0x700;
const EINT_MASK: u64 = 0x900;
const EINT_PEND: u64 = 0xA00;

// The s3c64xx SPI registers (ferros_kernel/src/spi.rs); this port runs mode 3 with feedback delay 1.
const CH_CFG: u64 = 0x00;
const MODE_CFG: u64 = 0x08;
const SLAVE_SEL: u64 = 0x0C;
const STATUS: u64 = 0x14;
const TX_DATA: u64 = 0x18;
const RX_DATA: u64 = 0x1C;
const PACKET_CNT: u64 = 0x20;
const FB_CLK: u64 = 0x2C;
const CH_TX_ON: u32 = 1 << 0;
const CH_RX_ON: u32 = 1 << 1;
const CH_SW_RST: u32 = 1 << 5;
const PACKET_CNT_EN: u32 = 1 << 16;
const MODE_BYTE: u32 = 0x1FF8_0000;
const SPI_MODE: u32 = 0xC;
const SPI_FB: u32 = 1;

/// (grant slot, data register offset, pin) for each line; a bank's data register is at +4.
const HOST_REQ: (u64, u64, u32) = (S_FAR, 0x04, 3);
const MCU_REQ: (u64, u64, u32) = (S_PERIC0, 0x84, 3);
const MCU_RESP: (u64, u64, u32) = (S_HSI2, 0x04, 2);
const NSTANDBY: (u64, u64, u32) = (S_HSI2, 0x04, 3);

struct Chip {
    grants: u64,
}

impl Chip {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.grants, S_SPI), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.grants, S_SPI), off, v);
    }
    fn get(&self, l: (u64, u64, u32)) -> bool {
        (mmio_read(cap(self.grants, l.0), l.1) as u32 >> l.2) & 1 == 1
    }
    fn set(&self, l: (u64, u64, u32), high: bool) {
        let c = cap(self.grants, l.0);
        let v = mmio_read(c, l.1) as u32;
        mmio_write(c, l.1, if high { v | 1 << l.2 } else { v & !(1 << l.2) });
    }
    fn flush(&self) {
        self.wr(CH_CFG, self.rd(CH_CFG) & !(CH_TX_ON | CH_RX_ON));
        self.wr(PACKET_CNT, 0);
        self.wr(CH_CFG, self.rd(CH_CFG) | CH_SW_RST);
        let t = now() + freq() / 1000;
        while (self.rd(STATUS) >> 6) & 0x1FF != 0 || (self.rd(STATUS) >> 15) & 0x1FF != 0 {
            if now() > t {
                break;
            }
        }
        self.wr(CH_CFG, self.rd(CH_CFG) & !CH_SW_RST);
    }
    /// A streaming full-duplex transfer under the port's own chip select, at most 32 bytes in flight (kernel spi.rs `stream`).
    fn stream(&self, tx: &[u8], rx: &mut [u8]) -> bool {
        let n = tx.len().min(rx.len());
        self.wr(FB_CLK, SPI_FB);
        self.wr(SLAVE_SEL, 0);
        self.wr(PACKET_CNT, 0);
        self.wr(PACKET_CNT, n as u32);
        self.wr(PACKET_CNT, n as u32 | PACKET_CNT_EN);
        self.wr(MODE_CFG, MODE_BYTE);
        let ch = (self.rd(CH_CFG) & !(CH_TX_ON | CH_RX_ON | 0xC)) | SPI_MODE;
        self.wr(CH_CFG, ch | CH_RX_ON | CH_TX_ON);
        let (mut ti, mut ri) = (0usize, 0usize);
        let t = now() + freq() / 5;
        let mut ok = true;
        while ri < n {
            while ti < n && ti - ri < 32 {
                self.wr(TX_DATA, tx[ti] as u32);
                ti += 1;
            }
            let have = ((self.rd(STATUS) >> 15) & 0x1FF) as usize;
            for _ in 0..have.min(n - ri) {
                rx[ri] = self.rd(RX_DATA) as u8;
                ri += 1;
            }
            if now() > t {
                ok = false;
                break;
            }
        }
        self.flush();
        let t = now() + freq() / 100_000;
        while now() < t {}
        self.wr(SLAVE_SEL, 1);
        ok
    }
    /// One SSI transfer (bcm_gps_spi.c): out `[G#50, len lo, len hi, data]`, in the same clocks `[status, its len lo, hi, its data]`; the chip's bytes are appended to `sink`.
    fn ssi(&self, data: &[u8], sink: &mut Buf) -> bool {
        let n = data.len().min(TX_MAX - 3);
        let mut tx = [0u8; TX_MAX];
        tx[0] = 0x50;
        tx[1] = n as u8;
        tx[2] = (n >> 8) as u8;
        tx[3..3 + n].copy_from_slice(&data[..n]);
        let mut rx = [0u8; TX_MAX];
        let ok = self.stream(&tx[..n + 3], &mut rx[..n + 3]);
        let have = (rx[1] as usize | (rx[2] as usize) << 8).min(n);
        sink.push(&rx[3..3 + have]);
        ok
    }
    /// What the chip has waiting: a three-byte probe for its length, then a transfer of that length.
    fn ssi_read(&self, sink: &mut Buf) -> bool {
        let mut rx = [0u8; 3];
        if !self.stream(&[0x50, 0, 0], &mut rx) {
            return false;
        }
        let len = (rx[1] as usize | (rx[2] as usize) << 8).min(TX_MAX - 3);
        if len == 0 {
            return true;
        }
        let zeros = [0u8; TX_MAX];
        self.ssi(&zeros[..len], sink)
    }
    /// MCU_REQ up and MCU_RESP within `ms`, as bcm_gps_spi.c waits (re-pulsed every 20 polls).
    fn wake(&self, polls: u32) -> bool {
        self.set(MCU_REQ, true);
        for i in 0..polls {
            if self.get(MCU_RESP) {
                return true;
            }
            if i % 20 == 19 {
                self.set(MCU_REQ, false);
                sleep_ms(1);
                self.set(MCU_REQ, true);
            }
            sleep_ms(2);
        }
        false
    }
}

const TX_MAX: usize = 4200;

fn sleep_ms(ms: u64) {
    ferros_sys::sleep_until(now() + freq() / 1000 * ms);
}

/// A byte buffer of fixed capacity: what the chip sent, waiting to be deframed.
struct Buf {
    b: [u8; 8192],
    n: usize,
}
impl Buf {
    fn push(&mut self, d: &[u8]) {
        let room = self.b.len() - self.n;
        let d = &d[..d.len().min(room)];
        self.b[self.n..self.n + d.len()].copy_from_slice(d);
        self.n += d.len();
    }
    fn drop_front(&mut self, k: usize) {
        self.b.copy_within(k..self.n, 0);
        self.n -= k;
    }
}

/// Broadcom's packet CRC: CRC-8, polynomial G#4D MSB-first, initial 0, over everything after the sequence byte, nibbles swapped.
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

/// Where a frame's payload starts: after sequence, length, flags and one byte per set flag bit (G#80 brings a second flags byte).
fn payload_start(f: &[u8]) -> usize {
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
    k
}

/// One on-chip fix (kernel gnss.rs `Fix`): lat/lon in 2⁻²³ degrees, altitude and error in metres, speed and track in 1/64ths, Lms the chip's millisecond clock.
#[derive(Clone, Copy, Default)]
struct Fix {
    lms: u32,
    lat: i32,
    lon: i32,
    alt: i32,
    err: u32,
    speed: i32,
    track: i32,
}

/// `n` bits at bit `o` of a stream of little-endian 32-bit words, MSB first (gpsd's reader).
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

/// A fix from a chip payload: the inner `c0 c1 … c0 c4` frame (G#C0 escaped as `c0 c0`), class `01 81`, id G#16, the position-valid, altitude and speed flags set (bits 210, 212, 215; GNSS.md "The on-chip engine").
fn parse_fix(p: &[u8]) -> Option<Fix> {
    let c = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1)?;
    let mut m = [0u8; 512];
    let (mut j, mut n) = (c + 2, 0usize);
    while j < p.len() && n < m.len() {
        if p[j] == 0xC0 && j + 1 < p.len() {
            if p[j + 1] != 0xC0 {
                break;
            }
            j += 1;
        }
        m[n] = p[j];
        n += 1;
        j += 1;
    }
    let m = &m[..n];
    if m.len() < 60 || m[0] != 0x01 || m[1] != 0x81 || m[9] != 0x16 {
        return None;
    }
    if bits(m, 212, 1) == 0 || bits(m, 210, 1) == 0 || bits(m, 215, 1) == 0 {
        return None;
    }
    let track = if bits(m, 213, 1) == 1 { sbits(m, 439, 16) } else { 0 };
    let x = Fix { err: bits(m, 221, 10), lms: bits(m, 231, 32), lat: sbits(m, 311, 32), lon: sbits(m, 343, 32), alt: sbits(m, 375, 16), speed: sbits(m, 407, 16), track };
    (x.lat.unsigned_abs() <= 90 << 23 && x.lon.unsigned_abs() <= 180 << 23 && x.err < 1023 && (x.lat, x.lon) != (0, 0)).then_some(x)
}

/// The chip's side, deframed: acks by sequence, fixes, CRC failures.
struct Deframer {
    acked: [bool; 256],
    frames: u32,
    crc_bad: u32,
    fixes: u32,
    fix: Fix,
    fix_at: u64,
}

impl Deframer {
    fn feed(&mut self, raw: &mut Buf) {
        loop {
            let r = &raw.b[..raw.n];
            let Some(s) = r.windows(2).position(|w| w == [0xB0, 0x00]) else {
                raw.n = 0;
                return;
            };
            let Some(e) = r[s + 2..].windows(2).position(|w| w == [0xB0, 0x01]).map(|e| e + s + 2) else {
                raw.drop_front(s);
                return;
            };
            let mut f = [0u8; 2048];
            let (mut j, mut n) = (s + 2, 0usize);
            while j < e && n < f.len() {
                if r[j] == 0xB0 && j + 1 < e {
                    f[n] = match r[j + 1] { 3 => 0xB0, 4 => 0x11, 5 => 0x13, _ => 0 };
                    j += 2;
                } else {
                    f[n] = r[j];
                    j += 1;
                }
                n += 1;
            }
            raw.drop_front(e + 2);
            self.frame(&f[..n]);
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
        self.frames += 1;
        let k = payload_start(f);
        // An ack: flags bits 0 and 2, the acknowledged host sequence first, no payload.
        let flags = f[2] as u16;
        if flags & 0x05 == 0x05 && k == f.len() - 1 {
            self.acked[f[3] as usize] = true;
            return;
        }
        if let Some(x) = parse_fix(&f[k..f.len() - 1]) {
            self.fixes += 1;
            self.fix = x;
            self.fix_at = now();
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
// Not done yet: feeding a fresh file to the chip. gpsd's upload (`82 81`) is not the file but window blocks computed from it (GNSS.md "Aiding"); the plan is gpsd's own routine run in an aarch64 interpreter capsule.

/// GPS time now from the wall clock in the preferences block (the kernel's base: unix seconds × frequency − the counter): milliseconds since 1980-01-06, UTC + 18 leap seconds.
fn gps_ms_now() -> Option<u64> {
    let base = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + ferros_sys::PREFS + 16) as *const u64) };
    if base == 0 {
        return None;
    }
    let (f, t) = (freq(), base.wrapping_add(now()));
    Some((t / f * 1000 + t % f * 1000 / f).checked_sub(315_964_800_000)? + 18_000)
}

/// gpsd's aiding in a record rewritten with ferros's own (kernel gnss.rs `patch_aiding`): `87 81` time (week at +8, time of week at +10 from the inner `c0 c1`), `d0 81` position and time (latitude u32 2⁻¹⁶ ° at +10, longitude u32 2⁻¹⁵ ° west-positive at +22, week +58, time of week +62). The inner checksum moves by the bytes' difference; a value whose bytes would need the C0 escape moves on by one unit. Returns the new record's length in `out`, or None when nothing changed.
fn patch_aiding(rec: &[u8], pos: Option<(i32, i32)>, out: &mut [u8]) -> Option<usize> {
    let now = gps_ms_now()?;
    let (mut i, mut o, mut changed) = (0usize, 0usize, false);
    let mut put = |out: &mut [u8], o: &mut usize, b: u8| {
        if *o < out.len() {
            out[*o] = b;
            *o += 1;
        }
    };
    while i < rec.len() {
        let e = if rec[i..].starts_with(&[0xB0, 0x00]) { rec[i + 2..].windows(2).position(|w| w == [0xB0, 0x01]).map(|e| e + i + 2) } else { None };
        let Some(e) = e else {
            put(out, &mut o, rec[i]);
            i += 1;
            continue;
        };
        let mut f = [0u8; 2048];
        let (mut j, mut n) = (i + 2, 0usize);
        while j < e && n < f.len() {
            if rec[j] == 0xB0 && j + 1 < e {
                f[n] = match rec[j + 1] { 3 => 0xB0, 4 => 0x11, _ => 0x13 };
                j += 2;
            } else {
                f[n] = rec[j];
                j += 1;
            }
            n += 1;
        }
        i = e + 2;
        if patch_frame(&mut f[..n], now, pos) {
            changed = true;
        }
        put(out, &mut o, 0xB0);
        put(out, &mut o, 0x00);
        for &b in &f[..n] {
            match b {
                0xB0 => { put(out, &mut o, 0xB0); put(out, &mut o, 0x03) }
                0x11 => { put(out, &mut o, 0xB0); put(out, &mut o, 0x04) }
                0x13 => { put(out, &mut o, 0xB0); put(out, &mut o, 0x05) }
                _ => put(out, &mut o, b),
            }
        }
        put(out, &mut o, 0xB0);
        put(out, &mut o, 0x01);
    }
    changed.then_some(o)
}

fn patch_frame(f: &mut [u8], now: u64, pos: Option<(i32, i32)>) -> bool {
    if f.len() < 4 {
        return false;
    }
    let k = payload_start(f);
    let end = f.len() - 1;
    let p = &mut f[k..end];
    let Some(c) = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1) else {
        return false;
    };
    let (week_at, tow_at, place) = match p.get(c + 2..c + 4) {
        Some([0x87, 0x81]) => (c + 8, c + 10, None),
        Some([0xD0, 0x81]) => (c + 58, c + 62, pos.map(|(lat, lon)| (c + 10, (lat >> 7) as u32, c + 22, (-(lon >> 8)) as u32))),
        _ => return false,
    };
    if p.len() < tow_at + 4 || p[c + 2..tow_at + 4].contains(&0xC0) {
        return false;
    }
    let week = (now / 604_800_000) as u16;
    let tow = (now % 604_800_000) as u32;
    let ck = u16::from_le_bytes([p[c + 6], p[c + 7]]);
    for d in 0..16u32 {
        // (offset, the new bytes, how many)
        let mut at = [(0usize, [0u8; 4], 0usize); 4];
        at[0] = (week_at, [week as u8, (week >> 8) as u8, 0, 0], 2);
        at[1] = (tow_at, (tow + d).to_le_bytes(), 4);
        let mut na = 2;
        if let Some((lat_at, lat, lon_at, lon)) = place {
            at[2] = (lat_at, (lat + d).to_le_bytes(), 4);
            at[3] = (lon_at, (lon + d).to_le_bytes(), 4);
            na = 4;
        }
        let at = &at[..na];
        let old: u32 = at.iter().map(|(o, _, n)| p[*o..*o + n].iter().map(|&b| b as u32).sum::<u32>()).sum();
        let new: u32 = at.iter().map(|(_, v, n)| v[..*n].iter().map(|&b| b as u32).sum::<u32>()).sum();
        let ck2 = (ck as u32).wrapping_sub(old).wrapping_add(new) as u16;
        if !at.iter().any(|(_, v, n)| v[..*n].contains(&0xC0)) && !ck2.to_le_bytes().contains(&0xC0) {
            for (o, v, n) in at {
                p[*o..*o + n].copy_from_slice(&v[..*n]);
            }
            p[c + 6..c + 8].copy_from_slice(&ck2.to_le_bytes());
            f[end] = crc(&f[1..end]);
            return true;
        }
    }
    false
}

/// The host frames in a record that want an ack (flags bit G#02): their sequence numbers.
fn reliable_seqs(rec: &[u8], out: &mut [u8; 16]) -> usize {
    let mut n = 0;
    let mut i = 0;
    let un = |k: usize| -> (u8, usize) {
        if rec[k] == 0xB0 { (match rec.get(k + 1) { Some(3) => 0xB0, Some(4) => 0x11, _ => 0x13 }, 2) } else { (rec[k], 1) }
    };
    while i + 6 < rec.len() {
        if rec[i] == 0xB0 && rec[i + 1] == 0x00 {
            let (seq, a) = un(i + 2);
            let (_len, b) = un(i + 2 + a);
            let (flags, _) = un(i + 2 + a + b);
            if flags & 0x02 != 0 && n < out.len() {
                out[n] = seq;
                n += 1;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    n
}

/// A small text line for the console.
struct Line {
    b: [u8; 160],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let s = s.as_bytes();
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 160], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

fn keep_last(grants: u64, d: &Deframer) {
    if d.fixes == 0 {
        return;
    }
    let mut v = [0u8; 8];
    v[..4].copy_from_slice(&d.fix.lat.to_le_bytes());
    v[4..].copy_from_slice(&d.fix.lon.to_le_bytes());
    store_set(cap(grants, S_STORE), b"last", &v);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    // Linked -pie: the image's own relocations first, before anything reaches a vtable.
    unsafe { ferros_sys::relocate() };
    let chip = Chip { grants };
    let sky = input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } & 1 == 1;
    let key: &[u8] = if sky { b"replay.sky" } else { b"replay" };
    let store = cap(grants, S_STORE);
    let mut last = [0u8; 8];
    let pos = (ferros_sys::store_get(store, b"last", &mut last) == Some(8)).then(|| (i32::from_le_bytes([last[0], last[1], last[2], last[3]]), i32::from_le_bytes([last[4], last[5], last[6], last[7]])));
    let mut head = [0u8; 4];
    let Some(total) = store_get_at(store, key, 0, &mut head) else {
        say!(grants, "gnss: no {} in the vault\n", if sky { "gnss.replay.sky" } else { "gnss.replay" });
        ferros_sys::exit(1);
    };
    if &head != b"GNR1" {
        say!(grants, "gnss: the replay is not GNR1\n");
        ferros_sys::exit(1);
    }
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say!(grants, "gnss: power on failed\n");
    }
    chip.set(NSTANDBY, true);
    sleep_ms(250);
    let woke = chip.wake(100);
    say!(grants, "gnss: powered, the chip {} its wake; replaying {} B ({}), position from {}\n", if woke { "answered" } else { "did not answer" }, total, if sky { "without the LTO" } else { "with the capture's LTO" }, if pos.is_some() { "the last fix" } else { "the capture" });

    let t_all = now();
    let f = freq();
    let mut d = Deframer { acked: [false; 256], frames: 0, crc_bad: 0, fixes: 0, fix: Fix::default(), fix_at: 0 };
    let mut raw = Buf { b: [0; 8192], n: 0 };
    let (mut at, mut recs, mut late, mut errs) = (4usize, 0u32, 0u32, 0u32);
    let mut rec = [0u8; 4096];
    let mut patched = [0u8; 8192];
    while at + 4 <= total {
        let mut h = [0u8; 4];
        store_get_at(store, key, at, &mut h);
        let len = u16::from_le_bytes([h[0], h[1]]) as usize;
        let gap = u16::from_le_bytes([h[2], h[3]]) as u64;
        let len = len.min(rec.len());
        let mut got = 0;
        while got < len {
            let k = (len - got).min(4096);
            store_get_at(store, key, at + 4 + got, &mut rec[got..got + k]);
            got += k;
        }
        at += 4 + len;
        recs += 1;
        let out: &[u8] = match patch_aiding(&rec[..len], pos, &mut patched) {
            Some(n) => &patched[..n],
            None => &rec[..len],
        };
        let mut seqs = [0u8; 16];
        let ns = reliable_seqs(&rec[..len], &mut seqs);
        for &s in &seqs[..ns] {
            d.acked[s as usize] = false;
        }
        if !chip.ssi(out, &mut raw) {
            errs += 1;
        }
        d.feed(&mut raw);
        let t0 = now();
        let limit = f / 1000 * gap.min(1000).max(if ns == 0 { 0 } else { 60 });
        let pace = f / 1000 * gap.min(1000);
        while now().wrapping_sub(t0) < limit {
            if ns > 0 && seqs[..ns].iter().all(|&s| d.acked[s as usize]) && now().wrapping_sub(t0) >= pace {
                break;
            }
            if chip.get(HOST_REQ) {
                if !chip.ssi_read(&mut raw) {
                    errs += 1;
                }
                d.feed(&mut raw);
            }
        }
        if ns > 0 && !seqs[..ns].iter().all(|&s| d.acked[s as usize]) {
            late += 1;
        }
    }
    chip.set(MCU_REQ, false);
    say!(grants, "gnss: replayed {} records in {} ms ({} SPI errors, {} not acked in time); the chip: {} frames, {} CRC failures\n", recs, now().wrapping_sub(t_all) * 1000 / f, errs, late, d.frames, d.crc_bad);

    // HOST_REQ as a level-high external interrupt: pin function EINT (G#F), its control nibble 1, pending cleared, unmasked.
    let far = cap(grants, S_FAR);
    mmio_write(far, 0x00, (mmio_read(far, 0x00) as u32 & !(0xF << 12)) | 0xF << 12);
    mmio_write(far, EINT_CON, (mmio_read(far, EINT_CON) as u32 & !(0xF << 12)) | 1 << 12);
    mmio_write(far, EINT_PEND, 1 << 3);
    mmio_write(far, EINT_MASK, mmio_read(far, EINT_MASK) as u32 & !(1 << 3));
    // Navigating: read the chip whenever it asks (HOST_REQ's interrupt), answer the endpoint, keep the last fix.
    let mut msg = [0u8; 16];
    let mut token = [0u8; 32];
    let mut shown = 0u32;
    let mut kept_at = 0u64;
    // (HOST_REQ seen, MCU_RESP not answering, bytes read), and when they were last reported.
    let (mut asks, mut silent, mut bytes, mut said) = (0u32, 0u32, 0usize, now());
    loop {
        // Asleep until the chip asks or 1/64 s passes (the endpoint is looked at between).
        ferros_sys::irq_wait(cap(grants, S_IRQ), now() + f / 64);
        while chip.get(HOST_REQ) {
            asks += 1;
            chip.set(MCU_REQ, true);
            let t = now();
            while !chip.get(MCU_RESP) && now().wrapping_sub(t) < f / 250 {}
            if chip.get(MCU_RESP) {
                let before = raw.n;
                chip.ssi_read(&mut raw);
                bytes += raw.n.saturating_sub(before);
                d.feed(&mut raw);
            } else {
                silent += 1;
            }
            chip.set(MCU_REQ, false);
        }
        mmio_write(far, EINT_PEND, 1 << 3);
        ferros_sys::irq_ack(cap(grants, S_IRQ));
        if now().wrapping_sub(said) > 32 * f {
            say!(grants, "gnss: the last 32 s: HOST_REQ {} times, MCU_RESP silent {}, {} bytes read; {} frames, {} fixes in all\n", asks, silent, bytes, d.frames, d.fixes);
            (asks, silent, bytes, said) = (0, 0, 0, now());
        }
        if d.fixes > shown {
            if shown == 0 {
                // Coarse on the console on purpose: the precise fix is for the endpoint, not the log.
                say!(grants, "gnss: first fix {} ms after power-up, ±{} m (lat {}.{} lon {}.{})\n", d.fix_at.wrapping_sub(t_all) * 1000 / f, d.fix.err, d.fix.lat >> 23, ((d.fix.lat.unsigned_abs() & 0x7F_FFFF) * 10) >> 23, d.fix.lon >> 23, ((d.fix.lon.unsigned_abs() & 0x7F_FFFF) * 10) >> 23);
                keep_last(grants, &d);
                kept_at = now();
            }
            shown = d.fixes;
        }
        if d.fixes > 0 && now().wrapping_sub(kept_at) > 64 * f {
            keep_last(grants, &d);
            kept_at = now();
        }
        let n = recv(cap(grants, S_GNSS), &mut msg, &mut token, true);
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            match msg[0] {
                0 => {
                    let mut r = [0u8; 33];
                    r[0] = (d.fixes > 0) as u8;
                    r[1..5].copy_from_slice(&d.fixes.to_le_bytes());
                    r[5..9].copy_from_slice(&((now().wrapping_sub(d.fix_at) * 1000 / f) as u32).to_le_bytes());
                    for (k, v) in [d.fix.lat, d.fix.lon, d.fix.alt, d.fix.err as i32, d.fix.speed, d.fix.track].iter().enumerate() {
                        r[9 + 4 * k..13 + 4 * k].copy_from_slice(&v.to_le_bytes());
                    }
                    reply(&token, &r);
                }
                1 => {
                    keep_last(grants, &d);
                    chip.set(NSTANDBY, false);
                    let ok = ferros_sys::power(cap(grants, S_POWER), false);
                    reply(&token, &[ok as u8]);
                    say!(grants, "gnss: stopped after {} fixes\n", d.fixes);
                    ferros_sys::exit(0);
                }
                _ => {
                    reply(&token, &[]);
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
