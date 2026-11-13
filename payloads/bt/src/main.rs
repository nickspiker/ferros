//! The Bluetooth driver (docs/BT.md): an EL0 process that runs the BCM4398's Bluetooth core over H4 on the APM's USI1 UART and advertises Photon's pairing beacon (photon docs/pairing-v2.md, PhotonBeacon.kt): 16 opaque bytes as one 128-bit service UUID, byte 0 the UUID's most significant, in a legacy non-connectable advertisement.
//!
//! The kernel keeps what a process cannot do behind the `bt` power capability (kernel bt.rs): the USI's mode word and the chip's power lines. Everything else is here: the UART through its MMIO grant, the firmware patch read from Android's vendor partition (`/firmware/brcm/BCM.hcd`, the file Android's HAL loads for this module, G5SN V17), randomness for the advertising address.
//!
//! The chip, as Android's HAL brings it up (captured 2026-10-31 from Android's UART writes and reads, the HAL restarted): at 115200 baud HCI_Reset and the vendor's chip-id read (G#FC79); the chip told 4 Mbaud (G#FC18) and the host following; fast download on (G#FE72), Download_Minidriver (G#FC2E), then every Write_RAM record of the patch with no wait for events, and Launch_RAM (G#FC4E) answered; the chip restarts at 115200: HCI_Reset, 4 Mbaud again, and it runs the patch. Then the LE commands the beacon needs. The public address (G#FC01) is not written: that address belongs to an identity, not this device; the beacon goes out from a fresh non-resolvable private address each time it starts.
//!
//! Spawn: ferros-bridge spawn bt.bin --name bt --serve bt --hw apm-usi1-uart --part vendor_a --power bt --entropy
//! Grant slots, in that order: 0 console, 1 the `bt` endpoint (serve), 2 the UART, 3 the vendor partition, 4 the power switch, 5 entropy.
//!
//! The `bt` endpoint, first byte the op:
//! - `0` STATE: text (the firmware, the beacon).
//! - `1` BEACON + 16 bytes: advertise them (replacing any beacon going out). → [0], or [1] if the chip refused.
//! - `2` STOP: no beacon. → [0] or [1].
//! - `3` SCAN: listen for pairing beacons (passive: nothing goes on the air), the sponsor's side of the ceremony (the identity service's add flow). → [0], or [1] if refused (nobody has attested, or the chip said no).
//! - `4` SCAN STOP. → [0] or [1].
//! - `5` HEARD: the 128-bit service UUIDs heard in the last 30 s, newest first, at most 16: [count][count × 16 bytes], byte 0 the most significant (as op 1 takes them). The identity service matches them against its candidates (fgtw::pair::beacon_matches); a UUID that matches nothing is any other device's.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, sleep_until, NOTHING};

const S_BT: u64 = 1;
const S_UART: u64 = 2;
const S_PART: u64 = 3;
const S_POWER: u64 = 4;
const S_ENTROPY: u64 = 5;
/// The UART's interrupt line (kernel `bt-uart`, SPI 71, added after the last grant for a holder of apm-usi1-uart: process.rs `LINES_WITH`, 2026-11-13).
const S_IRQ: u64 = 6;
/// UINTM with the receive and error interrupts open (bits 0 and 1), transmit and modem kept masked.
const UINTM_RX: u32 = 0xC;
/// The serving loop's word when it has no interrupt line: nothing rings it; a wait on it ends on a call or its deadline.
static IDLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

const FIRMWARE: &str = "/firmware/brcm/BCM.hcd";

// The Exynos UART (serial_s3c.h, exynos_tty.c).
const ULCON: u64 = 0x00;
const UCON: u64 = 0x04;
const UFCON: u64 = 0x08;
const UMCON: u64 = 0x0C;
const UERSTAT: u64 = 0x14;
const UFSTAT: u64 = 0x18;
const UMSTAT: u64 = 0x1C;
const UTXH: u64 = 0x20;
const URXH: u64 = 0x24;
const UBRDIV: u64 = 0x28;
const UFRACVAL: u64 = 0x2C;
const UINTP: u64 = 0x30;
const UINTM: u64 = 0x38;
const USI_CON: u64 = 0xC4;
const USI_OPTION: u64 = 0xC8;
const UFSTAT_RX_COUNT: u32 = 0xFF;
const UFSTAT_RX_FULL: u32 = 1 << 8;
const UFSTAT_TX_COUNT_SHIFT: u32 = 16;
const UFSTAT_TX_FULL: u32 = 1 << 24;
const FIFO: u32 = 256;
/// The UART's clock: CMU_APM's APM_FUNC divided by 2 (DIV_CLK_APM_USI1_UART ratio 1), 200 MHz in Google's clock tables (cmucal-vclk.c: 200000).
const CLK: u32 = 200_000_000;
const SLOW: u32 = 115_200;
const FAST: u32 = 4_000_000;

/// How many service UUIDs heard are kept: a sponsor matches a newcomer's beacon among them, and a busy room advertises others; the newcomer's, advertised every 100 ms, is always among the newest.
const HEARD: usize = 32;
/// How long a UUID heard counts (op 5): Photon's nearby window.
const HEARD_FRESH_S: u64 = 30;
/// The scan's interval and window, equal (scanning all the time while the add flow is open): 96 × 0.625 ms = 60 ms.
const SCAN: u16 = 96;

/// The advertising interval: 160 × 0.625 ms = 100 ms, Android's ADVERTISE_MODE_LOW_LATENCY.
const INTERVAL: u16 = 160;

struct Line {
    b: [u8; 512],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

struct Hex<'a>(&'a [u8]);
impl core::fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        for b in self.0 {
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}

fn sleep_ms(n: u64) {
    sleep_until(now() + freq() / 1000 * n);
}

/// The firmware file's chunk buffer: a partition read returns up to 64 KiB a call (kernel partfs.rs PART_CHUNK).
static mut CHUNK: [u8; 1 << 16] = [0; 1 << 16];

struct Bt {
    grants: u64,
    /// Bytes received and not yet taken as a packet.
    rx: [u8; 1024],
    rx_len: usize,
    /// The patch's version line (Read_Local_Name after it runs).
    name: [u8; 64],
    name_len: usize,
    beacon: Option<[u8; 16]>,
    events_in_download: u32,
    dropped: u32,
    /// Scanning for pairing beacons (the identity service's add flow, op 3), and the 128-bit service UUIDs heard, each with when (`heard_uuid`): the newest `HEARD` kept, the oldest replaced.
    scanning: bool,
    /// What the scan has heard since the driver started (measuring the sponsor's nearby path): advertising reports, and those carrying a 128-bit service UUID. Said on the console at each power of two.
    reports: u32,
    uuid_reports: u32,
    heard: [([u8; 16], u64); HEARD],
}

impl Bt {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.grants, S_UART), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.grants, S_UART), off, v);
    }

    /// The divisor for `baud` (exynos_tty.c): div = clock / baud, UBRDIV = div / 16 − 1, UFRACVAL the remaining sixteenths.
    fn baud(&self, baud: u32) {
        let div = CLK / baud;
        self.wr(UBRDIV, div / 16 - 1);
        self.wr(UFRACVAL, div & 15);
    }

    /// The USI out of reset with its clock request on (`exynos_usi_init`), the port reset (`exynos_serial_resetport`): RTS trigger level 1 with automatic flow control, interrupts masked (this driver polls), both FIFOs reset and on, 8N1, at 115200.
    fn uart_up(&self) {
        self.wr(USI_CON, 1);
        sleep_ms(1);
        self.wr(USI_CON, 0);
        sleep_ms(1);
        self.wr(USI_OPTION, 1 << 1);
        self.wr(UMCON, 1 << 5 | 1 << 4);
        self.wr(UINTM, 0xF);
        self.wr(UINTP, 0xF);
        self.wr(UFCON, 0x117);
        sleep_ms(1);
        self.wr(UFCON, 0x111);
        self.wr(UCON, 0x3C5);
        self.wr(ULCON, 3);
        self.baud(SLOW);
    }

    /// Bytes out, the TX FIFO filled as far as it has room each look.
    fn put(&mut self, b: &[u8]) {
        let mut at = 0;
        while at < b.len() {
            let st = self.rd(UFSTAT);
            if st & UFSTAT_TX_FULL != 0 {
                self.pull();
                continue;
            }
            let room = (FIFO - ((st >> UFSTAT_TX_COUNT_SHIFT) & 0xFF)) as usize;
            let n = room.min(b.len() - at);
            for &c in &b[at..at + n] {
                self.wr(UTXH, c as u32);
            }
            at += n;
        }
    }

    /// Waits for the TX FIFO to empty (before a baud change).
    fn drain_tx(&self) {
        let end = now() + freq() / 64;
        while (self.rd(UFSTAT) >> UFSTAT_TX_COUNT_SHIFT) & 0x1FF != 0 && now() < end {}
        sleep_ms(1);
    }

    /// What the RX FIFO holds, into `rx` (taken as packets by `event`). The chip's bytes are external input: past `rx`'s end (nobody taking events) they are counted in `dropped` and not stored; H4 has no resync, so the next command's wait fails and says so.
    fn pull(&mut self) -> usize {
        let st = self.rd(UFSTAT);
        let n = if st & UFSTAT_RX_FULL != 0 { FIFO } else { st & UFSTAT_RX_COUNT } as usize;
        for _ in 0..n {
            let c = self.rd(URXH) as u8;
            if self.rx_len < self.rx.len() {
                self.rx[self.rx_len] = c;
                self.rx_len += 1;
            } else {
                self.dropped += 1;
            }
        }
        n
    }

    /// The next whole H4 event from `rx` (type 4, code, length, parameters) into `e`, its length; any other packet type is dropped.
    fn event(&mut self, e: &mut [u8; 258]) -> Option<usize> {
        loop {
            if self.rx_len == 0 {
                return None;
            }
            if self.rx[0] != 4 {
                say!(self.grants, "bt: dropped a byte G#{:02X} that does not start an event\n", self.rx[0]);
                self.rx.copy_within(1..self.rx_len, 0);
                self.rx_len -= 1;
                continue;
            }
            if self.rx_len < 3 {
                return None;
            }
            let n = 3 + self.rx[2] as usize;
            if self.rx_len < n {
                return None;
            }
            e[..n - 1].copy_from_slice(&self.rx[1..n]);
            self.rx.copy_within(n..self.rx_len, 0);
            self.rx_len -= n;
            return Some(n - 1);
        }
    }

    /// An HCI command and its Command Complete (or Command Status): the event's parameters after the opcode (status first), or None after `ms` with nothing.
    fn cmd(&mut self, op: u16, params: &[u8], ms: u64, e: &mut [u8; 258]) -> Option<usize> {
        let mut p = [0u8; 4 + 255];
        p[0] = 1;
        p[1..3].copy_from_slice(&op.to_le_bytes());
        p[3] = params.len() as u8;
        p[4..4 + params.len()].copy_from_slice(params);
        self.put(&p[..4 + params.len()]);
        let end = now() + freq() / 1000 * ms;
        while now() < end {
            self.pull();
            while let Some(n) = self.event(e) {
                // Command Complete: code G#0E, num packets, opcode, return parameters.
                if e[0] == 0x0E && n >= 5 && u16::from_le_bytes([e[3], e[4]]) == op {
                    e.copy_within(5..n, 0);
                    return Some(n - 5);
                }
                // Command Status: code G#0F, status, num packets, opcode.
                if e[0] == 0x0F && n >= 6 && u16::from_le_bytes([e[4], e[5]]) == op {
                    e[0] = e[2];
                    return Some(1);
                }
                if self.adv_report(&e[..n]) {
                    continue;
                }
                say!(self.grants, "bt: event {}\n", Hex(&e[..n.min(32)]));
            }
        }
        say!(self.grants, "bt: no answer to G#{op:04X} (UMSTAT G#{:X}, UERSTAT G#{:X}, {} bytes waiting: {}, {} dropped)\n", self.rd(UMSTAT), self.rd(UERSTAT), self.rx_len, Hex(&self.rx[..self.rx_len.min(16)]), self.dropped);
        None
    }

    /// The corners' Bluetooth circle (ferros_sys::measure): up, and while the beacon goes out the power it goes out at (LE Read Advertising Physical Channel Tx Power, G#2007: status, then the power, signed dBm). Published at each change: up, a beacon started, stopped or restored.
    fn measures(&mut self) {
        use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
        status(0, w(k::STATE, 1));
        let mut e = [0u8; 258];
        let tx = match self.beacon {
            Some(_) => match self.cmd(0x2007, &[], 256, &mut e) {
                Some(n) if n >= 2 && e[0] == 0 => w(k::TX_POWER, e[1] as i8 as i64 * 1000),
                _ => MEASURE_NONE,
            },
            None => MEASURE_NONE,
        };
        status(1, tx);
    }

    /// A command whose status must be 0.
    fn ok(&mut self, op: u16, params: &[u8], ms: u64) -> bool {
        let mut e = [0u8; 258];
        match self.cmd(op, params, ms, &mut e) {
            Some(n) if n >= 1 && e[0] == 0 => true,
            Some(n) => {
                say!(self.grants, "bt: G#{op:04X} → {}\n", Hex(&e[..n]));
                false
            }
            None => false,
        }
    }

    /// Both ends to `baud`: the chip told (Update_Baudrate, G#FC18: two zero bytes, the rate little-endian), then the host.
    fn speed(&mut self, baud: u32) -> bool {
        let mut p = [0u8; 6];
        p[2..].copy_from_slice(&baud.to_le_bytes());
        if !self.ok(0xFC18, &p, 256) {
            return false;
        }
        self.drain_tx();
        self.baud(baud);
        sleep_ms(4);
        true
    }

    /// The patch: Download_Minidriver, then every record of the file sent as an HCI command without waiting (fast download), the last (Launch_RAM) waited for.
    fn download(&mut self) -> Result<u32, &'static str> {
        let part = cap(self.grants, S_PART);
        let mut req = [0u8; 9 + FIRMWARE.len()];
        req[0] = 1;
        req[1..1 + FIRMWARE.len()].copy_from_slice(FIRMWARE.as_bytes());
        let mut r = [0u8; 8];
        if ferros_sys::call(part, &req[..1 + FIRMWARE.len()], &mut r) != 8 {
            return Err("BCM.hcd not on vendor_a");
        }
        let size = u64::from_le_bytes(r);
        // Fast download: no Command Complete per Write_RAM.
        if !self.ok(0xFE72, &[0x00, 0x01], 256) {
            return Err("fast download refused");
        }
        if !self.ok(0xFC2E, &[], 256) {
            return Err("Download_Minidriver refused");
        }
        sleep_ms(64);
        req[0] = 2;
        req[9..].copy_from_slice(FIRMWARE.as_bytes());
        let chunk = unsafe { &mut *(&raw mut CHUNK) };
        let mut at = 0u64;
        let mut records = 0u32;
        let mut e = [0u8; 258];
        while at < size {
            req[1..9].copy_from_slice(&at.to_le_bytes());
            let got = ferros_sys::call(part, &req, chunk);
            if got == 0 || got == ferros_sys::ERR {
                return Err("BCM.hcd read failed");
            }
            let got = got as usize;
            let mut k = 0;
            // Each record is an HCI command: opcode (2), length (1), parameters.
            while k + 3 <= got && k + 3 + chunk[k + 2] as usize <= got {
                let n = 3 + chunk[k + 2] as usize;
                let op = u16::from_le_bytes([chunk[k], chunk[k + 1]]);
                let mut p = [0u8; 1 + 258];
                p[0] = 1;
                p[1..1 + n].copy_from_slice(&chunk[k..k + n]);
                if op == 0xFC4E {
                    self.put(&p[..1 + n]);
                    let end = now() + freq();
                    let mut launched = false;
                    while now() < end && !launched {
                        self.pull();
                        while let Some(m) = self.event(&mut e) {
                            launched |= e[0] == 0x0E && m >= 6 && u16::from_le_bytes([e[3], e[4]]) == 0xFC4E && e[5] == 0;
                        }
                    }
                    if !launched {
                        return Err("Launch_RAM unanswered");
                    }
                    return Ok(records);
                }
                self.put(&p[..1 + n]);
                if self.pull() > 0 {
                    while self.event(&mut e).is_some() {
                        self.events_in_download += 1;
                    }
                }
                records += 1;
                k += n;
            }
            at += k as u64;
            if k == 0 {
                return Err("BCM.hcd ends inside a record");
            }
        }
        Err("BCM.hcd has no Launch_RAM")
    }

    /// Power, the UART, the patch, and the chip at 4 Mbaud running it.
    fn up(&mut self) -> Result<(), &'static str> {
        if !ferros_sys::power(cap(self.grants, S_POWER), true) {
            return Err("power refused (WiFi's REG_ON low?)");
        }
        self.uart_up();
        sleep_ms(64);
        if !self.ok(0x0C03, &[], 1024) {
            return Err("HCI_Reset unanswered at 115200");
        }
        let mut e = [0u8; 258];
        if let Some(n) = self.cmd(0xFC79, &[], 256, &mut e) {
            say!(self.grants, "bt: chip id {}\n", Hex(&e[..n]));
        }
        if !self.speed(FAST) {
            return Err("4 Mbaud refused");
        }
        let t0 = now();
        let records = self.download()?;
        say!(self.grants, "bt: patch: {} records in {} ms ({} events during)\n", records, (now() - t0) * 1000 / freq(), self.events_in_download);
        // The chip restarts into the patch at 115200 (Android waits a quarter second).
        self.drain_tx();
        self.baud(SLOW);
        sleep_ms(256);
        self.rx_len = 0;
        if !self.ok(0x0C03, &[], 1024) {
            return Err("HCI_Reset unanswered after the patch");
        }
        if !self.speed(FAST) {
            return Err("4 Mbaud refused after the patch");
        }
        if let Some(n) = self.cmd(0x0C14, &[], 256, &mut e) {
            let s = &e[1..n];
            let l = s.iter().position(|&c| c == 0).unwrap_or(s.len()).min(self.name.len());
            self.name[..l].copy_from_slice(&s[..l]);
            self.name_len = l;
        }
        if !self.ok(0x0C03, &[], 1024) {
            return Err("HCI_Reset failed at 4 Mbaud");
        }
        Ok(())
    }

    /// Advertise `b`: advertising off, a fresh non-resolvable private address (the two top bits 00), the parameters (non-connectable, random address, all three channels), the data (one complete list of 128-bit service UUIDs: the UUID little-endian on air, so byte 0, the most significant, last), advertising on.
    fn advertise(&mut self, b: &[u8; 16]) -> bool {
        self.ok(0x200A, &[0], 256);
        let mut a = [0u8; 6];
        if ferros_sys::entropy(cap(self.grants, S_ENTROPY), &mut a) != 6 {
            say!(self.grants, "bt: no entropy for the address\n");
            return false;
        }
        a[5] &= 0x3F;
        if !self.ok(0x2005, &a, 256) {
            return false;
        }
        let i = INTERVAL.to_le_bytes();
        if !self.ok(0x2006, &[i[0], i[1], i[0], i[1], 0x03, 0x01, 0, 0, 0, 0, 0, 0, 0, 0x07, 0], 256) {
            return false;
        }
        let mut d = [0u8; 32];
        d[0] = 18;
        d[1] = 17;
        d[2] = 0x07;
        for k in 0..16 {
            d[3 + k] = b[15 - k];
        }
        if !self.ok(0x2008, &d, 256) {
            return false;
        }
        if !self.ok(0x200A, &[1], 256) {
            return false;
        }
        self.beacon = Some(*b);
        true
    }

    /// An LE Advertising Report (LE Meta, code G#3E, subevent G#02) while scanning: each 128-bit service UUID it carries (AD types G#06 and G#07, little-endian on air, so byte 0, the most significant, last) kept as heard now. Returns whether the event was one (reports are taken here, never printed: a room full of advertisers would flood the console). The report is the chip's, relaying any device in range: every length in it is checked before it is read.
    fn adv_report(&mut self, e: &[u8]) -> bool {
        if e.len() < 4 || e[0] != 0x3E || e[2] != 0x02 {
            return false;
        }
        self.reports += 1;
        if self.reports.is_power_of_two() {
            say!(self.grants, "bt: scan: {} advertising reports heard, {} with a 128-bit service UUID\n", self.reports, self.uuid_reports);
        }
        // One report per event is what this chip sends for legacy scanning; several are laid out field by field (all types, then all addresses, ...), which is not read here.
        if e[3] != 1 || e.len() < 4 + 9 {
            return true;
        }
        let len = e[12] as usize;
        let Some(data) = e.get(13..13 + len) else { return true };
        let mut at = 0;
        while at + 1 < data.len() {
            let l = data[at] as usize;
            if l == 0 || at + 1 + l > data.len() {
                break;
            }
            let (kind, body) = (data[at + 1], &data[at + 2..at + 1 + l]);
            if kind == 0x06 || kind == 0x07 {
                self.uuid_reports += 1;
                for u in body.chunks_exact(16) {
                    let mut id = [0u8; 16];
                    for k in 0..16 {
                        id[k] = u[15 - k];
                    }
                    self.heard_uuid(id);
                }
            }
            at += 1 + l;
        }
        true
    }

    /// `id` heard now: its entry refreshed, or the oldest replaced.
    fn heard_uuid(&mut self, id: [u8; 16]) {
        let t = now();
        let slot = match self.heard.iter().position(|(u, at)| *at != 0 && *u == id) {
            Some(i) => i,
            // HEARD is a nonzero constant, so the range is never empty.
            None => (0..HEARD).min_by_key(|&i| self.heard[i].1).unwrap(),
        };
        self.heard[slot] = (id, t);
    }

    /// Scanning for pairing beacons on (passive: nothing goes on the air; the own address a fresh non-resolvable private one, as the beacon's) or off.
    fn scan(&mut self, on: bool) -> bool {
        if !on {
            if !self.scanning {
                return true;
            }
            self.scanning = false;
            return self.ok(0x200C, &[0, 0], 256);
        }
        // Asked again while scanning (the identity service re-asserts it): nothing to change.
        if self.scanning {
            return true;
        }
        // The LE Meta event (bit 61) is outside the controller's default event mask (bits 0-44, Core 5.x Vol 4 Part E 7.3.1): without it no advertising report ever reaches the host (2026-11-08: the scan heard nothing). The default mask with bit 61, and the LE event mask's default (advertising reports among them).
        let mask: u64 = (1 << 45) - 1 | 1 << 61;
        if !self.ok(0x0C01, &mask.to_le_bytes(), 256) {
            return false;
        }
        if !self.ok(0x2001, &0x1Fu64.to_le_bytes(), 256) {
            return false;
        }
        let mut a = [0u8; 6];
        if self.beacon.is_none() {
            if ferros_sys::entropy(cap(self.grants, S_ENTROPY), &mut a) != 6 {
                say!(self.grants, "bt: no entropy for the address\n");
                return false;
            }
            a[5] &= 0x3F;
            if !self.ok(0x2005, &a, 256) {
                return false;
            }
        }
        let s = SCAN.to_le_bytes();
        if !self.ok(0x200B, &[0, s[0], s[1], s[0], s[1], 0x01, 0x00], 256) {
            return false;
        }
        // No duplicate filtering: a beacon heard again keeps its time fresh.
        if !self.ok(0x200C, &[1, 0], 256) {
            return false;
        }
        self.scanning = true;
        true
    }

    /// The UUIDs heard within `HEARD_FRESH_S`, newest first, at most 16: [count][count × 16 bytes].
    fn heard_reply(&self, out: &mut [u8; 1 + 16 * 16]) -> usize {
        let t = now();
        let fresh = freq() * HEARD_FRESH_S;
        let mut order: [usize; HEARD] = core::array::from_fn(|i| i);
        order.sort_unstable_by_key(|&i| core::cmp::Reverse(self.heard[i].1));
        let mut k = 0;
        for &i in &order {
            let (u, at) = self.heard[i];
            // `k == 16`: the reply holds sixteen (the identity service reads that many); the table keeps more so a noisy room does not push a newcomer out between reads.
            if at == 0 || t - at > fresh || k == 16 {
                break;
            }
            out[1 + k * 16..1 + (k + 1) * 16].copy_from_slice(&u);
            k += 1;
        }
        out[0] = k as u8;
        1 + k * 16
    }

    fn state(&self, out: &mut Line) {
        let _ = write!(out, "{}; beacon {}", core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?"), if self.beacon.is_some() { "advertising" } else { "off" });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn bt_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    // Input: 16 bytes to advertise from the start (a test without a caller). Taken first: a native payload's input lands right after its code, where `.bss` (the firmware chunk) is.
    let mut first = [0u8; 16];
    let has_first = input_len == 16;
    if has_first {
        for (k, v) in first.iter_mut().enumerate() {
            *v = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k as u64) as *const u8) };
        }
    }
    let mut bt = Bt { grants, rx: [0; 1024], rx_len: 0, name: [0; 64], name_len: 0, beacon: None, events_in_download: 0, dropped: 0, scanning: false, heard: [([0; 16], 0); HEARD], reports: 0, uuid_reports: 0 };
    // At boot WiFi's driver may not have raised its REG_ON yet (the kernel refuses Bluetooth alone): tried again every 4 s, saying so once. Any other failure ends the driver, the chip powered down.
    // Started beside WiFi's driver at boot: its REG_ON cycle and bring-up first (it turns this core off before the cycle anyway).
    sleep_ms(16 << 10);
    let mut waited = false;
    loop {
        match bt.up() {
            Ok(()) => break,
            Err(e) if e.starts_with("power refused") => {
                if !waited {
                    say!(grants, "bt: waiting for WiFi's REG_ON\n");
                    waited = true;
                }
                sleep_ms(4096);
            }
            Err(e) => {
                // One shot: the kernel switches the chip off as this process ends (it switched it on).
                say!(grants, "bt: bring-up failed: {e}\n");
                ferros_sys::exit(1);
            }
        }
    }
    let mut l = Line { b: [0; 512], n: 0 };
    bt.state(&mut l);
    say!(grants, "bt: up: {}\n", core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
    if has_first {
        let ok = bt.advertise(&first);
        say!(grants, "bt: beacon {} {}\n", Hex(&first), if ok { "advertising" } else { "refused" });
    }
    bt.measures();
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut checked = now();
    let mut e = [0u8; 258];
    // The UART's interrupt line (a kernel from 2026-11-13), asked with a wait whose deadline has passed: ERR for a slot that is no line.
    let n_grants = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) } as u64;
    let irq = n_grants > S_IRQ && ferros_sys::irq_wait(cap(grants, S_IRQ), 1) != ferros_sys::ERR;
    say!(grants, "bt: {}\n", if irq { "waiting on the UART's interrupt line, a call, and the 8 s look at the chip" } else { "no interrupt line granted (a kernel before 2026-11-13): waiting on a call and the 8 s look, the UART read every 4 ms while scanning" });
    // The loop's wakes, said every 64 s or more: passes, and how many ended a wait.
    let (mut passes, mut waits, mut said_at) = (0u32, 0u32, now());
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        passes += 1;
        if now().wrapping_sub(said_at) >= freq() << 6 {
            say!(grants, "bt: A#{passes} passes, A#{waits} waits in the last A#{} ms\n", (now() - said_at) * 1000 / freq());
            (passes, waits, said_at) = (0, 0, now());
        }
        // WiFi's driver pulling its REG_ON (a restart) resets this core to its ROM, the patch and the beacon gone, the UART answering nothing at 4 Mbaud: every 8 s Read_Local_Version_Information asks, and silence brings the chip up again with the beacon it had.
        if now() - checked > freq() * 8 {
            checked = now();
            if bt.cmd(0x1001, &[], 256, &mut e).is_none() {
                say!(grants, "bt: the chip stopped answering (WiFi restarted?): bringing it up again\n");
                let had = bt.beacon.take();
                bt.rx_len = 0;
                // WiFi's driver turned this core off before cycling its REG_ON: its bring-up (link, firmware) is given 16 s before this one starts, as Android's coordinator lets one finish before the other begins.
                sleep_ms(16 << 10);
                // A chip that keeps failing is tried again 4 s later, the wait doubling to 1024 s (as the modem driver backs off).
                let mut wait = 4u64;
                loop {
                    match bt.up() {
                        Ok(()) => break,
                        Err(e) => {
                            say!(grants, "bt: bring-up failed: {e}; again in {wait} s\n");
                            sleep_ms(wait * 1024);
                            if wait < 1024 {
                                wait *= 2;
                            }
                        }
                    }
                }
                if let Some(b) = had {
                    let ok = bt.advertise(&b);
                    say!(grants, "bt: beacon {}\n", if ok { "restored" } else { "refused" });
                }
                if bt.scanning {
                    let ok = bt.scan(true);
                    say!(grants, "bt: scanning {}\n", if ok { "restored" } else { "refused" });
                }
                bt.measures();
                checked = now();
            }
        }
        if bt.pull() > 0 {
            while let Some(n) = bt.event(&mut e) {
                if bt.adv_report(&e[..n]) {
                    continue;
                }
                say!(grants, "bt: event {}\n", Hex(&e[..n.min(32)]));
            }
        }
        let n = recv(cap(grants, S_BT), &mut msg, &mut token, true);
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            match msg[0] {
                0 => {
                    let mut l = Line { b: [0; 512], n: 0 };
                    bt.state(&mut l);
                    reply(&token, &l.b[..l.n]);
                }
                // While nobody has attested nothing goes on the air: no beacon (AUTH.md "Radios while locked"; the beacon is fgtw's, which only a person starts).
                1 if n == 17 && ferros_sys::facts().0[83] != 2 => {
                    say!(grants, "bt: beacon refused: nobody has attested\n");
                    reply(&token, &[1]);
                }
                1 if n == 17 => {
                    let mut b = [0u8; 16];
                    b.copy_from_slice(&msg[1..17]);
                    let ok = bt.advertise(&b);
                    reply(&token, &[!ok as u8]);
                    bt.measures();
                }
                2 => {
                    let ok = bt.ok(0x200A, &[0], 256);
                    bt.beacon = None;
                    reply(&token, &[!ok as u8]);
                    bt.measures();
                }
                // The sponsor's side (the identity service's add flow): scan for newcomers' pairing beacons, stop, and the beacons heard. Gated like the beacon: nothing is listened for before a person has attested.
                3 if ferros_sys::facts().0[83] != 2 => {
                    reply(&token, &[1]);
                }
                3 => {
                    let ok = bt.scan(true);
                    say!(grants, "bt: scanning for pairing beacons: {}\n", if ok { "on" } else { "refused" });
                    reply(&token, &[!ok as u8]);
                }
                4 => {
                    let ok = bt.scan(false);
                    say!(grants, "bt: scanning stopped\n");
                    reply(&token, &[!ok as u8]);
                }
                5 => {
                    let mut out = [0u8; 1 + 16 * 16];
                    let k = bt.heard_reply(&mut out);
                    reply(&token, &out[..k]);
                }
                _ => {
                    reply(&token, &[]);
                }
            }
            continue;
        }
        // The one deadline: the next look at the chip (above), which finds a core WiFi's driver reset. Until 2026-11-13 the loop also read the UART every 4 ms, some 250 wakes a second.
        let until = checked + freq() * 8 + 1;
        waits += 1;
        if irq {
            // The UART's pending bits cleared and its receive interrupt open, the line acked, then the FIFO read once more before the wait: bytes before the clear are taken now, bytes after raise the line. The wait ends on the line, a call (this thread polls `bt`: back with 2) or the deadline.
            bt.wr(UINTP, 0xF);
            bt.wr(UINTM, UINTM_RX);
            ferros_sys::irq_ack(cap(grants, S_IRQ));
            if bt.pull() > 0 {
                while let Some(n) = bt.event(&mut e) {
                    if bt.adv_report(&e[..n]) {
                        continue;
                    }
                    say!(grants, "bt: event {}\n", Hex(&e[..n.min(32)]));
                }
                continue;
            }
            ferros_sys::irq_wait_world(cap(grants, S_IRQ), until);
        } else if bt.scanning {
            // No line and the scan on: its reports read every 4 ms, as before.
            sleep_ms(4);
        } else {
            // No line and nothing asked of the air: the chip has nothing to say (an unsolicited event waits in the FIFO behind RTS until the next pass); a call or the next look ends the wait.
            ferros_sys::wait_world_or_call(&IDLE, 0, until);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(bt_main);
