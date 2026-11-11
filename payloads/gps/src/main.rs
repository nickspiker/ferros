//! The GPS driver (GPS.md): an EL0 process that runs the BCM4776 — its SPI transport, the session replay with ferros's own aiding, the chip's on-chip fixes — and serves them on the `gps` endpoint. What was the kernel's `gps.rs` session, as a process.
//!
//! The kernel keeps only what a process cannot do: the chip's three rails (ACPM), the SPI port's clocks and USI mode, and the lines' pin modes, behind the `gps` power capability (`SYS_POWER`, kernel gps.rs `power`). Everything else is here, through MMIO grants: the port's registers, and the data bits of the four lines (HOST_REQ gpa5-3, MCU_REQ gpp4-3, MCU_RESP gph3-2, nSTANDBY gph3-3).
//!
//! The session: the vault's `gps.replay` (or `gps.replay.sky`, the same without the LTO orbits) is gpsd's on-chip session cut from a capture (the patch download, the second firmware stage, the configuration, the aiding), played record by record, each reliable packet waiting for the chip's ack. Before it goes out, the aiding is ferros's own: the time from the RTC, the position from the last fix (`gps.last`), else the zone in effect's principal place (2026-11-11), else none. Then the chip navigates alone and sends a fix a second; the latest is kept, and written back to `gps.last` whenever it moves (every change hits disk; nothing is written on the way out).
//!
//! Spawn: ferros-bridge spawn gps.bin <flags> --name gps --serve gps --hw usi3-spi --hw gpio-far-alive --hw gpio-gpp0 --hw gpio-hsi2 --store gps --power gps --irq gps-host-req
//! Grant slots, in that order: 0 console, 1 the `gps` endpoint (serve), 2 the SPI port, 3 gpio-far-alive, 4 gpio-gpp0 (PERIC0), 5 gpio-hsi2, 6 the vault scope `gps.`, 7 the power switch, 8 HOST_REQ's interrupt. Input byte 0: bit 0 = the sky replay. Bit 1: scrub the replays of the capture's place and end. Bit 2: with the zone's place as the aid, also write the zone's width into the uncertainty candidate (`Aid`), the on-device test of that field.
//!
//! The `gps` endpoint, first byte the op:
//! - `0` FIX: `valid u8`, `fixes u32`, `age ms u32` (since the latest), `lat i32`, `lon i32` (2⁻²³ degrees), `alt i32` (m), `err u32` (m), `speed i32`, `track i32` (1/64 m/s, 1/64 °), little-endian, 33 bytes; then (2026-11-11) `frames u32` (every frame from the chip so far, acks and all) and `kinds u8` (the kinds among them, op 4), 38 in all. A caller asking 33 gets the fix alone.
//! - `1` STOP: the process exits after replying; the kernel powers the chip down as it ends (the last fix is kept whenever it changes).
//! - `2` SATS: the signals the chip tracked at the latest fix message (sky.rs, GPS.md "The satellites"): `n u8`, `msgs u32` (fix messages decoded so far), `age ms u32` (since the latest), then `n` records of 25 bytes — `system u8` (the chip's code: 0 GPS, 8 GLONASS), `band u8` (0 L1 or GLONASS G1, 2 L5), `prn u8` (GLONASS: the orbital slot), `flags u8` (bit 0 used in the fix, bit 1 the chip's unnamed flag), `el i8`, `az u16` (degrees), `d_el i16`, `d_az i16` (degrees since the previous fix that had the signal, the azimuth the short way round north), `cn0 u16` (quarter dB-Hz), `d_ms u32` (the chip's ms between the two fixes, 0 for a signal new in this one), `el_rate i32`, `az_rate i32` (millidegrees a second between the first and the latest one-degree step seen, `i32::MIN` until there are two) — little-endian, at most 64 records.
//! - `3` WATCH (2026-11-10): `count u32`, the change count the caller last saw (its fixes plus its fix messages, and since 2026-11-11 its frames in steps of 64) → `count u32` now, held until it differs: the doorbell the map's GPS page waits on, thru the kernel (SYS_CLAIM op 3), instead of asking every second.
//! - `4` FRAMES (2026-11-11, GPS.md "What the chip sends before a fix"): every frame from the chip is counted by its kind (`tally`: flags, flags' second byte, the payload's first two bytes, the inner frame's class), 16 kinds at most, the latest two of each kept (256 bytes each; never a fix message, which carries the person's position). `[4]` → `kinds u8`, `frames u32`, `crc_bad u32`, then per kind `key [u8; 6]`, `count u32`, `bytes u32`, `age ms u32` (since its latest). `[4, k]` → kind k's `key [u8; 6]`, `n u8`, then n frames, the latest first, each `len u16` (its whole length, deframed: sequence, length, flags, the flag bytes, the payload, the CRC), `kept u16`, the first `kept` bytes. Not thru the kernel's op 3: a caller holding the `gps` endpoint (payloads/gpstest `04`).
//!
//! The loop sleeps in one wait for HOST_REQ's interrupt and a call to `gps` (the kernel wakes the endpoint's poller out of it), no deadline.
//!
//! Measures (ferros_sys::status, the corners' GPS circle): slot 0 the state, 1 the error estimate, 2 the satellites tracked, 3 those used in the fix, 4 the best C/N0.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, store_get_at, store_set, store_set_at, NOTHING};

mod sky;
use sky::{Sky, REC, SATS};

const S_GPS: u64 = 1;
const S_SPI: u64 = 2;
const S_FAR: u64 = 3;
const S_PERIC0: u64 = 4;
const S_HSI2: u64 = 5;
const S_STORE: u64 = 6;
const S_POWER: u64 = 7;
/// FIX's reply: the fix (33 bytes), then the chip's frames so far and their kinds (5).
const FIX_LEN: usize = 33 + 5;
/// FRAMES' record for a kind: key 6, count, bytes, age.
const KIND_REC: usize = 6 + 4 + 4 + 4;
/// HOST_REQ's interrupt (kernel `gps-host-req`): the loop sleeps on it instead of polling the line, which missed the chip's requests (2026-10-26: a fix, then nothing).
const S_IRQ: u64 = 8;
/// The far-alive bank's external-interrupt registers (pinctrl-exynos.h; gpa5 is the bank's only one): control (a nibble per pin, 1 = high level), mask, pending.
const EINT_CON: u64 = 0x700;
const EINT_MASK: u64 = 0x900;
const EINT_PEND: u64 = 0xA00;

/// The port (ferros_spi, the kernel's power switch runs the same code to set it up) runs mode 3 with feedback delay 1.
const SPI_MODE: u32 = 0xC;
const SPI_FB: u32 = 1;

/// The SPI port thru its hardware grant.
struct Port(u64);
impl ferros_spi::Regs for Port {
    fn rd(&self, off: usize) -> u32 {
        mmio_read(self.0, off as u64) as u32
    }
    fn wr(&self, off: usize, v: u32) {
        mmio_write(self.0, off as u64, v);
    }
    fn now(&self) -> u64 {
        now()
    }
    fn freq(&self) -> u64 {
        freq()
    }
}

/// (grant slot, data register offset, pin) for each line; a bank's data register is at +4.
const HOST_REQ: (u64, u64, u32) = (S_FAR, 0x04, 3);
const MCU_REQ: (u64, u64, u32) = (S_PERIC0, 0x84, 3);
const MCU_RESP: (u64, u64, u32) = (S_HSI2, 0x04, 2);
const NSTANDBY: (u64, u64, u32) = (S_HSI2, 0x04, 3);

struct Chip {
    grants: u64,
    spi: ferros_spi::Spi<Port>,
}

impl Chip {
    fn new(grants: u64) -> Chip {
        Chip { grants, spi: ferros_spi::Spi { regs: Port(cap(grants, S_SPI)), mode: SPI_MODE, fb: SPI_FB } }
    }
    fn get(&self, l: (u64, u64, u32)) -> bool {
        (mmio_read(cap(self.grants, l.0), l.1) as u32 >> l.2) & 1 == 1
    }
    fn set(&self, l: (u64, u64, u32), high: bool) {
        let c = cap(self.grants, l.0);
        let v = mmio_read(c, l.1) as u32;
        mmio_write(c, l.1, if high { v | 1 << l.2 } else { v & !(1 << l.2) });
    }
    /// A streaming full-duplex transfer under the port's own chip select, at most 32 bytes in flight (ferros_spi `stream`).
    fn stream(&self, tx: &[u8], rx: &mut [u8]) -> bool {
        self.spi.stream(tx, rx).is_ok()
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

/// One on-chip fix (kernel gps.rs `Fix`): lat/lon in 2⁻²³ degrees, altitude and error in metres, speed and track in 1/64ths, Lms the chip's millisecond clock.
#[derive(Clone, Copy, Default)]
struct Fix {
    lms: u32,
    lat: i32,
    lon: i32,
    alt: i32,
    err: u32,
    speed: i32,
    track: i32,
    /// The message carried a track (its flag, bit 213): without it `track` is 0, not north.
    has_track: bool,
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

/// The fix message in a chip payload, unescaped into `m` from the class on; its length.
/// The inner `c0 c1 … c0 c4` frame (G#C0 escaped as `c0 c0`), class `01 81`, id G#16 (GPS.md "The on-chip engine").
fn fix_msg(p: &[u8], m: &mut [u8; 512]) -> Option<usize> {
    let c = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1)?;
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
    if n < 60 || m[0] != 0x01 || m[1] != 0x81 || m[9] != 0x16 {
        return None;
    }
    Some(n)
}

/// The position in a fix message (`fix_msg`), when the position-valid, altitude and speed flags are set (bits 210, 212, 215).
fn parse_fix(m: &[u8]) -> Option<Fix> {
    if bits(m, 212, 1) == 0 || bits(m, 210, 1) == 0 || bits(m, 215, 1) == 0 {
        return None;
    }
    let has_track = bits(m, 213, 1) == 1;
    let track = if has_track { sbits(m, 439, 16) } else { 0 };
    let x = Fix { err: bits(m, 221, 10), lms: bits(m, 231, 32), lat: sbits(m, 311, 32), lon: sbits(m, 343, 32), alt: sbits(m, 375, 16), speed: sbits(m, 407, 16), track, has_track };
    (x.lat.unsigned_abs() <= 90 << 23 && x.lon.unsigned_abs() <= 180 << 23 && x.err < 1023 && (x.lat, x.lon) != (0, 0)).then_some(x)
}

/// The kinds of frame the chip sends, counted, the latest of each kept (GPS.md "What the chip sends before a fix"; owner, 2026-11-11: "I'd like to see what's actually going on in this thing"). The Pro sat for hours at some 90 frames a second, 0 fix messages, and the 2026-10-25 capture has no message at all from the chip in on-chip mode before its first fix, so what those frames are is read here, on the device.
/// A kind is `[flags, flags' second byte (0 without one), the payload's first two bytes (the RPC: 1f 80 from the chip), the inner frame's class (0 0 without a c0 c1 frame)]`: acks, sync packets and each message class apart.
const KINDS: usize = 1 << 4;
/// Frames kept per kind, the latest; and the bytes kept of each (a frame is at most 2048 after deframing; the first 256 hold an RPC header and a message's start).
const KEEP: usize = 1 << 1;
const KEEP_LEN: usize = 1 << 8;

#[derive(Clone, Copy)]
struct Kind {
    key: [u8; 6],
    count: u32,
    /// Bytes of deframed frame; the count and the bytes at the last 32 s report.
    bytes: u32,
    said: u32,
    bytes_said: u32,
    /// The counter at the latest.
    at: u64,
    kept: [[u8; KEEP_LEN]; KEEP],
    len: [u16; KEEP],
    /// Where the next one goes in `kept`, and a bit for each slot written.
    next: u8,
    filled: u8,
}

const KIND0: Kind = Kind { key: [0; 6], count: 0, bytes: 0, said: 0, bytes_said: 0, at: 0, kept: [[0; KEEP_LEN]; KEEP], len: [0; KEEP], next: 0, filled: 0 };

/// In .bss, off the stack: 16 kinds of 548 bytes (the session's buffers already take some 30 KiB of the 64 KiB stack). Only the main thread touches it.
static mut KIND_T: [Kind; KINDS] = [KIND0; KINDS];
/// Kinds in use, and frames whose kind did not fit (a 17th kind: counted, not kept).
static mut KIND_N: usize = 0;
static mut KIND_OVER: u32 = 0;

fn kinds() -> &'static mut [Kind; KINDS] {
    unsafe { &mut *(&raw mut KIND_T) }
}
fn kinds_n() -> usize {
    unsafe { KIND_N }
}

/// One frame (`f`, deframed, CRC good, its payload from `k`) into its kind. A fix message (`fix`) is counted and never kept: its position is the person's, served on FIX alone.
fn tally(f: &[u8], k: usize, fix: bool) {
    // f is at least 4 bytes (`frame`), so its fourth is there when the flags say a second byte follows.
    let hi = if f[2] & 0x80 != 0 { f[3] } else { 0 };
    let p = &f[k..f.len() - 1];
    // WHY the defaults: an ack or a sync packet has no payload, or one shorter than an RPC header; those bytes read as 0 in the kind.
    let (p0, p1) = (p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0));
    // WHY the get: the chip's payload can end right after its `c0 c1`; a class cut short reads as none.
    let cls = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1).and_then(|c| Some((*p.get(c + 2)?, *p.get(c + 3)?))).unwrap_or((0, 0));
    let key = [f[2], hi, p0, p1, cls.0, cls.1];
    let t = kinds();
    let n = kinds_n();
    let i = match t[..n].iter().position(|x| x.key == key) {
        Some(i) => i,
        // WHY the bound: the kinds are the chip's to choose and the table is fixed; a kind past it is counted in KIND_OVER instead of written past the table.
        None if n < KINDS => {
            t[n].key = key;
            unsafe { KIND_N = n + 1 };
            n
        }
        None => {
            unsafe { KIND_OVER += 1 };
            return;
        }
    };
    let x = &mut t[i];
    x.count = x.count.wrapping_add(1);
    x.bytes = x.bytes.wrapping_add(f.len() as u32);
    x.at = now();
    if !fix {
        let s = x.next as usize;
        // WHY the min: a frame is up to 2048 bytes deframed; the first KEEP_LEN are kept, its whole length beside them.
        let m = f.len().min(KEEP_LEN);
        x.kept[s][..m].copy_from_slice(&f[..m]);
        x.len[s] = f.len() as u16;
        x.next = ((s + 1) % KEEP) as u8;
        x.filled |= 1 << s;
    }
}

/// The chip's side, deframed: acks by sequence, fixes and the satellites that came with them, CRC failures.
struct Deframer {
    acked: [bool; 256],
    frames: u32,
    crc_bad: u32,
    fixes: u32,
    fix: Fix,
    fix_at: u64,
    sky: Sky,
    sky_at: u64,
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
        let mut m = [0u8; 512];
        let fixm = fix_msg(&f[k..f.len() - 1], &mut m);
        tally(f, k, fixm.is_some());
        if flags & 0x05 == 0x05 && k == f.len() - 1 {
            self.acked[f[3] as usize] = true;
            return;
        }
        let Some(n) = fixm else {
            return;
        };
        self.sky.take(&m[..n]);
        self.sky_at = now();
        if let Some(x) = parse_fix(&m[..n]) {
            self.fixes += 1;
            self.fix = x;
            self.fix_at = now();
        }
    }
}

// ── Aiding: what the chip is told before it looks at the sky (GPS.md "Aiding") ──
//
// The replay carries gpsd's aiding from the capture: the time (rewritten below), the last position, the ephemerides gpsd kept, and an LTO file.
// LTO ("Long Term Orbits") is Broadcom's (from Global Locate, hence glpals.com and gpsd's Gl* names): every satellite's orbit and clock predicted days ahead, so a start without fresh ephemerides takes seconds instead of a cold search.
// Where it comes from, as measured 2026-10-26: the Pixel's /data/vendor/gps/lto2.dat is byte-for-byte the file at http://gllto2.glpals.com/7day/v5/latest/lto2.dat (~188 KB).
// The same server answers over plain HTTP and HTTPS, with no key, account, signature or terms shown; it is regenerated a few times a day (Last-Modified) and gpsd re-fetches daily (gps.xml HttpSyncLto, LtoSyncThresholdDays=1; ltoStatus.txt holds the unix time of the last fetch).
// Variants on the same path: 7day/v2 59 KB, v3 103 KB, v4 146 KB, v5 188 KB, 4day/v5 123 KB (gpsd's compiled-in URL is the 4day one; growing with the constellations covered, by inference); http://gllto.glpals.com/rtistatus5.dat (240 bytes, real-time satellite health); gpsd's rto.dat (real-time orbits) URL answers 404.
// The file as read: magic ff ca de 00, u32 length (file − 8), u32 1, 16 bytes that change per file (likely the "LTO file id" gpsd logs), u32 G#75A0DE5 and u32 5292 (the header's end) in every variant, u32 70, then 70 × (index u32, offset u32) into G#44-byte records before the bulk; gpsd logs "LTO under license to 'google', license #69" when it reads one.
// Our stance: the file is published openly, unposted and unauthenticated, as the Pixel itself fetches it; ferros fetches it the same way for its own chip, and if Broadcom or Google ask, this comes down.
// Not done yet: feeding a fresh file to the chip. gpsd's upload (`82 81`) is not the file but window blocks computed from it (GPS.md "Aiding"); the plan is gpsd's own routine run in an aarch64 interpreter capsule.

/// GPS time now from the wall clock in the preferences block (the kernel's base: unix seconds × frequency − the counter): milliseconds since 1980-01-06, UTC + 18 leap seconds.
fn gps_ms_now() -> Option<u64> {
    let base = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + ferros_sys::PREFS + 16) as *const u64) };
    if base == 0 {
        return None;
    }
    let (f, t) = (freq(), base.wrapping_add(now()));
    Some((t / f * 1000 + t % f * 1000 / f).checked_sub(315_964_800_000)? + 18_000)
}

/// What the `d0 81` aiding says of the place: a position (2^-23 degrees; None leaves the replay's), and the uncertainty candidate in whole metres (None leaves the replay's).
/// The candidate (GPS.md "The position aiding, d0 81"): the 32.32 word pair at +46/+50 read a few tens (of metres, if metres) in the only two captured sessions, plausible for gpsd's horizontal uncertainty minutes after a fix, but no log names it, and it could as well be the altitude; so it is written only when the driver is asked to try it (input bit 2).
#[derive(Clone, Copy)]
struct Aid {
    pos: Option<(i32, i32)>,
    unc: Option<u32>,
}

/// A zone's whole width as the aid's uncertainty, in metres (input bit 2): 1 << 21, some 2100 km, covers nearly every zone from its principal place (the Pacific zone from Los Angeles to Seattle is 1550 km); a few are wider (China's single zone, Russia's Moscow zone), where the chip still starts from the right half of the sky.
const ZONE_UNC_M: u32 = 1 << 21;

/// gpsd's aiding in a record rewritten with ferros's own (kernel gps.rs `patch_aiding`): `87 81` time (week at +8, time of week at +10 from the inner `c0 c1`), `d0 81` position and time (latitude u32 2⁻¹⁶ ° at +10, longitude u32 2⁻¹⁵ ° west-positive at +22, week +58, time of week +62). The inner checksum moves by the bytes' difference; a value whose bytes would need the C0 escape moves on by one unit. Returns the new record's length in `out`, or None when nothing changed.
fn patch_aiding(rec: &[u8], aid: Aid, out: &mut [u8]) -> Option<usize> {
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
        if patch_frame(&mut f[..n], now, aid) {
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

fn patch_frame(f: &mut [u8], now: u64, aid: Aid) -> bool {
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
        Some([0xD0, 0x81]) => (c + 58, c + 62, aid.pos.map(|(lat, lon)| (c + 10, (lat >> 7) as u32, c + 22, (-(lon >> 8)) as u32))),
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
        let mut at = [(0usize, [0u8; 4], 0usize); 6];
        at[0] = (week_at, [week as u8, (week >> 8) as u8, 0, 0], 2);
        at[1] = (tow_at, (tow + d).to_le_bytes(), 4);
        let mut na = 2;
        if let Some((lat_at, lat, lon_at, lon)) = place {
            at[2] = (lat_at, (lat + d).to_le_bytes(), 4);
            at[3] = (lon_at, (lon + d).to_le_bytes(), 4);
            na = 4;
            // The uncertainty candidate (`Aid::unc`): whole metres in the word at +46, its fraction word at +50 zeroed.
            if let Some(u) = aid.unc {
                at[4] = (c + 46, (u + d).to_le_bytes(), 4);
                at[5] = (c + 50, [0; 4], 4);
                na = 6;
            }
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

/// The 32 s report's second line: each kind of frame the chip sent since the last, as `flags[.second]/rpc/class x<count> (<mean bytes>)`, the kind's key in hex (`tally`), so a console capture says what the chip is sending without a byte of what it says.
fn say_kinds(grants: u64) {
    let mut l = Line { b: [0; 160], n: 0 };
    let _ = write!(l, "gps: frames by kind, the last 32 s:");
    let t = kinds();
    let mut any = false;
    for x in t[..kinds_n()].iter_mut() {
        let new = x.count.wrapping_sub(x.said);
        let new_bytes = x.bytes.wrapping_sub(x.bytes_said);
        (x.said, x.bytes_said) = (x.count, x.bytes);
        if new == 0 {
            continue;
        }
        any = true;
        let k = x.key;
        let mut w = Line { b: [0; 160], n: 0 };
        let _ = write!(w, " {:02x}", k[0]);
        if k[1] != 0 {
            let _ = write!(w, ".{:02x}", k[1]);
        }
        let _ = write!(w, "/{:02x}{:02x}/{:02x}{:02x} x{} ({} B)", k[2], k[3], k[4], k[5], new, new_bytes / new);
        // A line full: out, and the rest on the next. Every append below leaves room for this newline (l.n + w.n + 1 at most the line), so l.b[l.n] is inside it.
        if l.n + w.n + 1 > l.b.len() {
            l.b[l.n] = b'\n';
            console(grants, &l.b[..l.n + 1]);
            l.n = 0;
            let _ = write!(l, "gps: frames by kind, more:");
        }
        let _ = l.write_str(core::str::from_utf8(&w.b[..w.n]).unwrap_or(""));
    }
    let over = unsafe { KIND_OVER };
    if over > 0 {
        let _ = write!(l, " (and {} of kinds past the table)", over);
    }
    if !any {
        let _ = write!(l, " none");
    }
    let _ = l.write_str("\n");
    console(grants, &l.b[..l.n]);
}

fn keep_last(grants: u64, d: &Deframer) {
    if d.fixes == 0 {
        return;
    }
    let mut v = [0u8; 8];
    v[..4].copy_from_slice(&d.fix.lat.to_le_bytes());
    v[4..].copy_from_slice(&d.fix.lon.to_le_bytes());
    // Said when refused, not silent (2026-11-10: the Pro started with no `gps.last` after a day of fixes, so whether these writes land is the question).
    if !store_set(cap(grants, S_STORE), b"last", &v) {
        console(grants, b"gps: the last fix was not kept: the vault refused gps.last\n");
    }
}

/// The capture's place out of a stored replay (owner, 2026-11-04: no place in the device's half, and no GPS while nobody has attested): every `d0 81` position zeroed through `patch_aiding` (its checksum and CRC redone, as at every run; the time fields take the clock's now, which every run rewrites anyway), the replay written back in pieces. The chip is never powered: this takes no fix. The message stays in the replay: cut or zeroed the chip waits 5-8 minutes alike (GPS.md), and zeroed keeps the record sequence the chip acknowledges; the person's own last fix (`gps.last`, the identity class) fills it whenever there is one.
fn scrub(grants: u64, store: u64, key: &[u8]) {
    let mut head = [0u8; 4];
    let Some(total) = store_get_at(store, key, 0, &mut head) else {
        return say!(grants, "gps: scrub: no {} in the vault\n", core::str::from_utf8(key).unwrap_or("?"));
    };
    if &head != b"GNR1" || gps_ms_now().is_none() {
        return say!(grants, "gps: scrub: {} is not GNR1, or the clock is not set: left as it was\n", core::str::from_utf8(key).unwrap_or("?"));
    }
    let mut ok = store_set_at(store, key, 0, b"GNR1", true);
    let (mut at, mut out_at, mut places) = (4usize, 4usize, 0u32);
    let mut rec = [0u8; 4096];
    let mut patched = [0u8; 8192];
    while at + 4 <= total {
        let mut h = [0u8; 4];
        store_get_at(store, key, at, &mut h);
        let len = u16::from_le_bytes([h[0], h[1]]) as usize;
        let mut got = 0;
        while got < len {
            let k = (len - got).min(4096);
            store_get_at(store, key, at + 4 + got, &mut rec[got..got + k]);
            got += k;
        }
        at += 4 + len;
        let body: &[u8] = match patch_aiding(&rec[..len], Aid { pos: Some((0, 0)), unc: None }, &mut patched) {
            Some(n) => &patched[..n],
            None => &rec[..len],
        };
        if body.windows(4).any(|w| w == [0xC0, 0xC1, 0xD0, 0x81]) {
            places += 1;
        }
        let hdr = [body.len() as u8, (body.len() >> 8) as u8, h[2], h[3]];
        ok &= store_set_at(store, key, out_at, &hdr, true);
        ok &= store_set_at(store, key, out_at + 4, body, at + 4 <= total);
        out_at += 4 + body.len();
    }
    say!(grants, "gps: scrub: {}: {} position messages zeroed, {} B written back ({})\n", core::str::from_utf8(key).unwrap_or("?"), places, out_at, if ok { "stored" } else { "the vault refused a piece" });
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    // Linked -pie: the image's own relocations first, before anything reaches a vtable.
    unsafe { ferros_sys::relocate() };
    let chip = Chip::new(grants);
    let sky = input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } & 1 == 1;
    let key: &[u8] = if sky { b"replay.sky" } else { b"replay" };
    let store = cap(grants, S_STORE);
    // Input bit 1: scrub both replays of the capture's place and end, the chip never powered (`scrub`).
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } & 2 == 2 {
        scrub(grants, store, b"replay");
        scrub(grants, store, b"replay.sky");
        ferros_sys::exit(0);
    }
    let mut last = [0u8; 8];
    let last = (ferros_sys::store_get(store, b"last", &mut last) == Some(8)).then(|| (i32::from_le_bytes([last[0], last[1], last[2], last[3]]), i32::from_le_bytes([last[4], last[5], last[6], last[7]])));
    // The aid's place (owner, 2026-11-11: "can't we figure that out from the orbital data anyway?"; the chip picks its sky from where it is told it is, and a zeroed place points it at the wrong one for 5-8 minutes): the person's own last fix; else the zone in effect's principal place (kernel zone.rs, the zones document's city: never anyone's place, only which part of the Earth); else none, the replay's zeroed one.
    // Arc-seconds to 2^-23 degrees: × 2^23 / 3600, in i64 (a latitude of 90° is 2^28.3 in 2^-23 degrees, the product fits).
    let zone = if last.is_none() { ferros_sys::zone::place().map(|(la, lo)| ((la as i64 * (1 << 23) / 3600) as i32, (lo as i64 * (1 << 23) / 3600) as i32)) } else { None };
    // Input bit 2: also write the zone's width into the uncertainty candidate (`Aid`), the on-device test of which field it is.
    let try_unc = input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } & 4 == 4;
    let aid = Aid { pos: last.or(zone), unc: (zone.is_some() && try_unc).then_some(ZONE_UNC_M) };
    let mut head = [0u8; 4];
    let Some(total) = store_get_at(store, key, 0, &mut head) else {
        say!(grants, "gps: no {} in the vault\n", if sky { "gps.replay.sky" } else { "gps.replay" });
        ferros_sys::exit(1);
    };
    if &head != b"GNR1" {
        say!(grants, "gps: the replay is not GNR1\n");
        ferros_sys::exit(1);
    }
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say!(grants, "gps: power on failed\n");
    }
    chip.set(NSTANDBY, true);
    sleep_ms(250);
    let woke = chip.wake(100);
    say!(grants, "gps: powered, the chip {} its wake; replaying {} B ({})\n", if woke { "answered" } else { "did not answer" }, total, if sky { "without the LTO" } else { "with the capture's LTO" });
    // Which place the aid carries, by its source and the zone's name, never its numbers (the zone's city is public, but the line says where the person is to a zone).
    if last.is_some() {
        say!(grants, "gps: position from the last fix\n");
    } else if zone.is_some() {
        let id = ferros_sys::zone::state().map(|s| s.effective).filter(|z| !z.is_none());
        say!(grants, "gps: position from the zone in effect, {} (its principal place){}\n", id.as_ref().map_or("?", |z| z.as_str()), if aid.unc.is_some() { ", the zone's width written as the uncertainty candidate (input bit 2)" } else { "" });
    } else {
        say!(grants, "gps: position from none: no last fix and no zone in effect (the replay's is zeroed: no place is kept in the device's half)\n");
    }

    let t_all = now();
    let f = freq();
    let mut d = Deframer { acked: [false; 256], frames: 0, crc_bad: 0, fixes: 0, fix: Fix::default(), fix_at: 0, sky: Sky::new(), sky_at: 0 };
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
        let out: &[u8] = match patch_aiding(&rec[..len], aid, &mut patched) {
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
    say!(grants, "gps: replayed {} records in {} ms ({} SPI errors, {} not acked in time); the chip: {} frames, {} CRC failures\n", recs, now().wrapping_sub(t_all) * 1000 / f, errs, late, d.frames, d.crc_bad);

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
    // The last fix as kept in the vault (`keep_last`), kept again only when it moves.
    let mut kept = (0i32, 0i32);
    // (HOST_REQ seen, MCU_RESP not answering, bytes read), and when they were last reported.
    let (mut asks, mut silent, mut bytes, mut said) = (0u32, 0u32, 0usize, now());
    // The corners' GPS circle (ferros_sys::measure): on and searching, then holding a fix with its error estimate; published when either changes.
    let mut published: Option<(u8, u32)> = None;
    // The satellites' three (tracked, used, best C/N0), published with each fix message (the count of those decoded and refused, `u32::MAX` before the first pass empties the slots).
    let mut published_sky = u32::MAX;
    // The tracks the last publish filled, so the next empties only past its own.
    let mut tracks_filled = 0usize;
    // The fix count the altitude, speed and heading were last published for (u32::MAX: never, so the first pass empties their slots).
    let mut published_fix = u32::MAX;
    // The WATCH calls held (op 3): each its reply token and the change count its caller last saw, answered when the count moves. Four at most; only the map's GPS page watches today.
    let mut watches: [Option<([u8; 32], u32)>; 1 << 2] = [None; 1 << 2];
    // Whether the endpoint has been polled once: from then the kernel knows this thread as its poller and wakes it out of the interrupt wait for a call (process.rs wake_server).
    let mut polled = false;
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        // Asleep until the chip asks (HOST_REQ) or a call comes to `gps`, one wait for both (owner, 2026-11-10: no polling): the kernel wakes the endpoint's poller out of an interrupt wait for a call, and returns at once when one was already queued. Until then the endpoint was looked at every 1/64 s. The first pass, before the kernel knows the poller, does not wait (a deadline already past).
        ferros_sys::irq_wait_world(cap(grants, S_IRQ), if polled { 0 } else { 1 });
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
            let (seen, used, cn0) = d.sky.counts();
            say!(grants, "gps: the last 32 s: HOST_REQ {} times, MCU_RESP silent {}, {} bytes read; {} frames, {} fixes in all; {} satellites tracked, {} in the fix, best C/N0 {} dB-Hz ({} fix messages laid out otherwise)\n", asks, silent, bytes, d.frames, d.fixes, seen, used, cn0 >> 2, d.sky.bad);
            say_kinds(grants);
            (asks, silent, bytes, said) = (0, 0, 0, now());
        }
        if d.fixes > shown {
            if shown == 0 {
                // Coarse on the console on purpose: the precise fix is for the endpoint, not the log.
                say!(grants, "gps: first fix {} ms after power-up, ±{} m (lat {}.{} lon {}.{})\n", d.fix_at.wrapping_sub(t_all) * 1000 / f, d.fix.err, d.fix.lat >> 23, ((d.fix.lat.unsigned_abs() & 0x7F_FFFF) * 10) >> 23, d.fix.lon >> 23, ((d.fix.lon.unsigned_abs() & 0x7F_FFFF) * 10) >> 23);
            }
            shown = d.fixes;
        }
        let now_shown = if d.fixes > 0 { (2, d.fix.err) } else { (1, 0) };
        if published != Some(now_shown) {
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            status(0, w(k::STATE, now_shown.0 as i64));
            status(1, if d.fixes > 0 { w(k::ACCURACY, now_shown.1 as i64 * 1000) } else { MEASURE_NONE });
            published = Some(now_shown);
        }
        // With each new fix, how it moves (docs/RADIO-CONTROLS.md): altitude (metres to mm), ground speed (64ths of a m/s to mm/s) and heading (64ths of a degree to millidegrees; only when the message carried a track). Not the position: that is the person's, served on the `gps` endpoint, never in a measure anyone can read.
        if d.fixes != published_fix {
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            published_fix = d.fixes;
            // The last fix kept as it changes (every change hits disk; owner, 2026-11-08: nothing is kept on the way out, an end is one shot dead).
            if d.fixes > 0 && (d.fix.lat, d.fix.lon) != kept {
                keep_last(grants, &d);
                kept = (d.fix.lat, d.fix.lon);
                // The zone (kernel zone.rs; owner, 2026-11-10: "cellular, then GPS"): each new position to the kernel thru the clock's narrow right, never a measure anyone reads; it finds the zone again once the device has moved some 30 km.
                ferros_sys::zone::position(d.fix.lat, d.fix.lon);
            }
            let fx = d.fix;
            status(5, if d.fixes > 0 { w(k::ALTITUDE, fx.alt as i64 * 1000) } else { MEASURE_NONE });
            status(6, if d.fixes > 0 { w(k::SPEED, fx.speed as i64 * 1000 >> 6) } else { MEASURE_NONE });
            status(7, if d.fixes > 0 && fx.has_track { w(k::HEADING, fx.track as i64 * 1000 >> 6) } else { MEASURE_NONE });
        }
        if published_sky != d.sky.msgs + d.sky.bad {
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            let (seen, used, cn0) = d.sky.counts();
            let none = d.sky.msgs == 0;
            status(2, if none { MEASURE_NONE } else { w(k::SATS_SEEN, seen as i64) });
            status(3, if none { MEASURE_NONE } else { w(k::SATS_USED, used as i64) });
            // Quarter dB-Hz to milli-dB-Hz.
            status(4, if none || seen == 0 { MEASURE_NONE } else { w(k::CN0, cn0 as i64 * 250) });
            // Each signal as a track (owner, 2026-11-07: where they are overhead and how they move; ferros_sys::Sat): the console's GPS orb draws its sky from these. The slots past the last signal emptied, up to what the last publish filled.
            let sats = d.sky.sats();
            let n = sats.len().min(ferros_sys::TRACKS_MAX);
            for (t, s) in sats[..n].iter().enumerate() {
                // WHY the saturation: a rate field holds six bits in steps of two millidegrees a second (±62); a faster one (an azimuth near the zenith) shows as the fastest it can hold rather than wrapping to the other sign.
                let rate = |r: i32| if r == sky::NO_RATE { ferros_sys::SAT_NO_RATE } else { (r / 2).clamp(-31, 31) as i8 };
                let sat = ferros_sys::Sat {
                    system: match s.sys { 0 => 0, 8 => 1, _ => 7 },
                    band: (s.band == 2) as u8,
                    number: s.prn,
                    used: s.flags & 1 != 0,
                    elevation: s.el,
                    azimuth: s.az,
                    cn0: (s.cn0 >> 2) as u8,
                    el_rate: rate(s.el_steps.rate()),
                    az_rate: rate(s.az_steps.rate()),
                };
                ferros_sys::status_track(t, ferros_sys::sat_word(&sat));
            }
            for t in n..tracks_filled {
                ferros_sys::status_track(t, MEASURE_NONE);
            }
            tracks_filled = n;
            published_sky = d.sky.msgs + d.sky.bad;
        }
        let n = recv(cap(grants, S_GPS), &mut msg, &mut token, true);
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            match msg[0] {
                0 => {
                    let mut r = [0u8; FIX_LEN];
                    r[0] = (d.fixes > 0) as u8;
                    r[1..5].copy_from_slice(&d.fixes.to_le_bytes());
                    r[5..9].copy_from_slice(&((now().wrapping_sub(d.fix_at) * 1000 / f) as u32).to_le_bytes());
                    for (k, v) in [d.fix.lat, d.fix.lon, d.fix.alt, d.fix.err as i32, d.fix.speed, d.fix.track].iter().enumerate() {
                        r[9 + 4 * k..13 + 4 * k].copy_from_slice(&v.to_le_bytes());
                    }
                    // What the chip is doing, for a page with no fix yet (2026-11-11): its frames so far and their kinds. A caller asking 33 bytes gets the fix alone (a reply is cut to what the caller asked).
                    r[33..37].copy_from_slice(&d.frames.to_le_bytes());
                    r[37] = kinds_n() as u8;
                    reply(&token, &r);
                }
                // STOP: one shot (owner, 2026-11-08): the reply, then gone; the kernel powers the chip down as this process ends (it switched it on), and the last fix was kept when it came.
                1 => {
                    reply(&token, &[1]);
                    say!(grants, "gps: stopped after {} fixes\n", d.fixes);
                    ferros_sys::exit(0);
                }
                2 => {
                    let mut r = [0u8; 9 + SATS * REC];
                    let n = d.sky.sats().len();
                    r[0] = n as u8;
                    r[1..5].copy_from_slice(&d.sky.msgs.to_le_bytes());
                    r[5..9].copy_from_slice(&((now().wrapping_sub(d.sky_at) * 1000 / f) as u32).to_le_bytes());
                    for k in 0..n {
                        d.sky.record(k, &mut r[9 + REC * k..9 + REC * (k + 1)]);
                    }
                    reply(&token, &r[..9 + REC * n]);
                }
                // WATCH (owner, 2026-11-10: "Needs a doorbell on the driver's fix counter, for sure"): `count u32`, the change count the caller last saw → `count u32` now, answered at once when it differs, else held until it moves (below). A fifth watch while four are held answers the oldest with the count as it is, which asks again.
                3 if n >= 5 => {
                    let seen = u32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]);
                    let c = changes(&d);
                    if c != seen {
                        reply(&token, &c.to_le_bytes());
                    } else {
                        if watches.iter().all(|w| w.is_some()) {
                            if let Some((t, _)) = watches[0].take() {
                                reply(&t, &c.to_le_bytes());
                            }
                            watches.rotate_left(1);
                        }
                        if let Some(w) = watches.iter_mut().find(|w| w.is_none()) {
                            *w = Some((token, seen));
                        }
                    }
                }
                // FRAMES (2026-11-11, GPS.md "What the chip sends before a fix"): `[4]` the kinds, `[4, k]` kind k's latest frames (main.rs's header has both layouts).
                4 => {
                    let t = kinds();
                    let nk = kinds_n();
                    if n < 2 {
                        let mut r = [0u8; 9 + KINDS * KIND_REC];
                        r[0] = nk as u8;
                        r[1..5].copy_from_slice(&d.frames.to_le_bytes());
                        r[5..9].copy_from_slice(&d.crc_bad.to_le_bytes());
                        for (i, x) in t[..nk].iter().enumerate() {
                            let o = 9 + KIND_REC * i;
                            r[o..o + 6].copy_from_slice(&x.key);
                            r[o + 6..o + 10].copy_from_slice(&x.count.to_le_bytes());
                            r[o + 10..o + 14].copy_from_slice(&x.bytes.to_le_bytes());
                            r[o + 14..o + 18].copy_from_slice(&((now().wrapping_sub(x.at) * 1000 / f) as u32).to_le_bytes());
                        }
                        reply(&token, &r[..9 + KIND_REC * nk]);
                    } else if (msg[1] as usize) < nk {
                        // WHY the bound: the index is the caller's; past the kinds in use it names none, and is answered empty below.
                        let x = &t[msg[1] as usize];
                        let mut r = [0u8; 7 + KEEP * (4 + KEEP_LEN)];
                        r[..6].copy_from_slice(&x.key);
                        let mut o = 7;
                        // The latest first: the slot before `next`, then round.
                        for j in 1..=KEEP {
                            let s = (x.next as usize + KEEP - j) % KEEP;
                            if x.filled & 1 << s == 0 {
                                continue;
                            }
                            // WHY the min: `len` is the frame's whole length, up to 2048; KEEP_LEN of it were kept (`tally`).
                            let kept = (x.len[s] as usize).min(KEEP_LEN);
                            r[o..o + 2].copy_from_slice(&x.len[s].to_le_bytes());
                            r[o + 2..o + 4].copy_from_slice(&(kept as u16).to_le_bytes());
                            r[o + 4..o + 4 + kept].copy_from_slice(&x.kept[s][..kept]);
                            o += 4 + kept;
                            r[6] += 1;
                        }
                        reply(&token, &r[..o]);
                    } else {
                        reply(&token, &[]);
                    }
                }
                _ => {
                    reply(&token, &[]);
                }
            }
        }
        polled = true;
        // The held watches whose count has moved: answered with the count now.
        let c = changes(&d);
        for w in watches.iter_mut() {
            if w.is_some_and(|(_, seen)| seen != c) {
                if let Some((t, _)) = w.take() {
                    reply(&t, &c.to_le_bytes());
                }
            }
        }
    }
}

/// The driver's change count for WATCH: its fixes and the fix messages decoded or refused, and its frames in steps of 64 (2026-11-11: a page with no fix yet shows what the chip sends, so it moves while the chip talks; at the Pro's 90 frames a second before a fix, under twice a second), summed (each only grows, so any change moves the sum; it wraps, and a watcher compares for difference only).
fn changes(d: &Deframer) -> u32 {
    d.fixes.wrapping_add(d.sky.msgs).wrapping_add(d.sky.bad).wrapping_add(d.frames >> 6)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
