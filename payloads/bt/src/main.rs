//! The Bluetooth driver (docs/BT.md): an EL0 process that runs the BCM4398's Bluetooth core over H4 on the APM's USI1 UART. It advertises Photon's pairing beacon (photon docs/pairing-v2.md, PhotonBeacon.kt), scans for the sponsor's side of pairing, and serves clients as an LE central: scans, connections, and L2CAP LE credit-based channels (link.rs, api.rs).
//!
//! The kernel keeps what a process cannot do behind the `bt` power capability (kernel bt.rs): the USI's mode word and the chip's power lines. Everything else is here: the UART through its MMIO grant, the firmware patch read from Android's vendor partition (`/firmware/brcm/BCM.hcd`, the file Android's HAL loads for this module, G5SN V17), randomness for the addresses and the sessions.
//!
//! The chip, as Android's HAL brings it up (captured 2026-10-31 from Android's UART writes and reads, the HAL restarted): at 115200 baud HCI_Reset and the vendor's chip-id read (G#FC79); the chip told 4 Mbaud (G#FC18) and the host following; fast download on (G#FE72), Download_Minidriver (G#FC2E), then every Write_RAM record of the patch with no wait for events, and Launch_RAM (G#FC4E) answered; the chip restarts at 115200: HCI_Reset, 4 Mbaud again, and it runs the patch. Then only standard HCI commands (BT.md "Commands this driver sends"): no vendor command touches the radio, and no test mode is ever entered.
//! The public address (G#FC01) is never written: a radio address belongs to an identity, not this device. The beacon goes out from a fresh non-resolvable private address each time it starts; scans and connections use a static random address drawn when the driver starts.
//!
//! Spawn: ferros-bridge spawn bt.bin --name bt --serve bt --hw apm-usi1-uart --part vendor_a --power bt --entropy
//! Grant slots, in that order: 0 console, 1 the `bt` endpoint (serve), 2 the UART, 3 the vendor partition, 4 the power switch, 5 entropy, 6 the UART's interrupt line (the kernel adds it).
//!
//! The `bt` endpoint, first byte the op:
//! - `0` STATE: text (the firmware, the beacon, links, sessions, chip restarts).
//! - `1` BEACON + 16 bytes: advertise them (replacing any beacon going out). → [0], or [1] if the chip refused.
//! - `2` STOP: no beacon. → [0] or [1].
//! - `3` SCAN: listen for pairing beacons (passive: nothing goes on the air), the sponsor's side of the ceremony (the identity service's add flow). → [0], or [1] if refused (nobody has attested, or the chip said no).
//! - `4` SCAN STOP. → [0] or [1].
//! - `5` HEARD: the 128-bit service UUIDs heard in the last 30 s, newest first, at most 16: [count][count × 16 bytes], byte 0 the most significant (as op 1 takes them). The identity service matches them against its candidates (fgtw::pair::beacon_matches); a UUID that matches nothing is any other device's.
//! - Ops 1 to 5 are the identity service's and refused to a kist ([1]).
//! - `6` to `15`: the client ops (api.rs, BT.md "The client ops").
#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};
use ferros_sys::{cap, freq, mmio_read, mmio_write, now, reply, sleep_until, NOTHING};

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);

unsafe extern "C" {
    /// The end of .bss (payload.ld): the heap starts there, in the zeroed tail the kernel maps after it.
    static __bss_end: u8;
}

pub struct Line {
    pub b: [u8; 512],
    pub n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // A console line is 512 bytes; a longer one is cut there.
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = $crate::Line { b: [0; 512], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($t)*));
        ferros_sys::console($g, &l.b[..l.n]);
    }};
}

mod api;
mod link;

const S_BT: u64 = 1;
const S_UART: u64 = 2;
const S_PART: u64 = 3;
const S_POWER: u64 = 4;
pub const S_ENTROPY: u64 = 5;
/// The UART's interrupt line (kernel `bt-uart`, SPI 71, added after the last grant for a holder of apm-usi1-uart: process.rs `LINES_WITH`, 2026-11-13).
const S_IRQ: u64 = 6;
/// UINTM with the receive and error interrupts open (bits 0 and 1), transmit and modem kept masked.
const UINTM_RX: u32 = 0xC;
/// The serving loop's doorbell: the UART's interrupt thread counts in its low bits, and the kernel sets bit 63 when the client whose end is watched ends (`ferros_sys::caller_watch`). The loop waits on it with its callers (`wait_world_or_call`).
pub static RING: AtomicU64 = AtomicU64::new(0);
const RING_ENDED: u64 = 1 << 63;
/// The interrupt thread's stack: it only waits and rings.
static mut IRQ_STACK: [u8; 1 << 14] = [0; 1 << 14];

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
/// The scan's interval and window, equal (scanning all the time while anyone asks), and the connection's scan while it initiates: 96 × 0.625 ms = 60 ms.
pub const SCAN: u16 = 96;

/// The advertising interval: 160 × 0.625 ms = 100 ms, Android's ADVERTISE_MODE_LOW_LATENCY.
const INTERVAL: u16 = 160;

/// The LE event mask: Connection Complete (bit 0), Advertising Report (1), Connection Update Complete (2), Read Remote Features Complete (3), Long Term Key Request (4), Data Length Change (6), PHY Update Complete (11). The remote parameter request (5) stays masked, so the controller answers a peer's request itself.
const LE_EVENTS: u64 = 0x1F | 1 << 6 | 1 << 11;

pub struct Hex<'a>(pub &'a [u8]);
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

/// What the controller's one random address register holds: legacy advertising, scanning and initiating all send from it, and it can change only while none of them runs (Core Vol 4 Part E 7.8.4).
#[derive(Clone, Copy, PartialEq)]
pub enum Reg {
    Unset,
    /// The static random address scans and connections use.
    Static,
    /// The beacon's fresh non-resolvable private address.
    Nrpa,
}

/// Work found while taking packets in that needs a command of its own, done from the loop (a command waits for its answer, and packets keep arriving while it does).
pub enum Todo {
    Disconnect(u16, u8),
    Update(u16, [u16; 4]),
    Phy(u16),
    Cancel,
    Scan,
    End(u64),
}

pub struct Bt {
    pub grants: u64,
    /// Bytes received and not yet taken as packets, from `rx_at` on.
    rx: Vec<u8>,
    rx_at: usize,
    /// The patch's version line (Read_Local_Name after it runs).
    name: [u8; 64],
    name_len: usize,
    beacon: Option<[u8; 16]>,
    events_in_download: u32,
    /// Bytes that started no H4 packet (the framing lost): every link is ended when it happens.
    desyncs: u32,
    /// Scanning for pairing beacons (the identity service's add flow, op 3), and the 128-bit service UUIDs heard, each with when (`heard_uuid`): the newest `HEARD` kept, the oldest replaced.
    pair_scan: bool,
    /// What the scan has heard since the driver started (measuring the sponsor's nearby path): advertising reports, and those carrying a 128-bit service UUID. Said on the console at each power of two.
    reports: u32,
    uuid_reports: u32,
    heard: [([u8; 16], u64); HEARD],
    /// The controller's scan: on, active, and paused for a connection attempt the controller would not start while scanning.
    pub scan_on: bool,
    scan_active: bool,
    pub scan_paused: bool,
    /// The static random address (HCI order, least significant byte first) and what the register holds.
    addr: [u8; 6],
    reg: Reg,
    /// The controller's LE ACL buffers (LE_Read_Buffer_Size): each packet's most data, how many, how many free now.
    pub acl_len: usize,
    acl_total: u16,
    pub acl_free: u16,
    pub conns: Vec<link::Conn>,
    pub connecting: Option<link::Pending>,
    pub sessions: Vec<api::Session>,
    /// The session whose process's end the kernel watches (rings `RING`); 0 none.
    pub watched: u64,
    pub todo: VecDeque<Todo>,
    pub ident: u8,
    /// Measures: ACL packets sent, packets for no link of ours, chip restarts found by the 8 s look and when the last was (counter ticks), when the driver came up.
    pub acl_out: u64,
    pub stray_acl: u32,
    /// ACL packets in, and the L2CAP PDUs by channel: signalling, ATT, SMP, data; signalling commands sent.
    pub acl_in: u64,
    pub pdus_in: [u32; 4],
    pub sig_out: u32,
    restarts: u32,
    restart_at: u64,
    up_at: u64,
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

    /// Bytes out, the TX FIFO filled as far as it has room each look; while it is full the RX FIFO is read on (into `rx`, taken later), so the chip is never held up sending to us.
    pub fn put(&mut self, b: &[u8]) {
        let mut at = 0;
        while at < b.len() {
            let st = self.rd(UFSTAT);
            if st & UFSTAT_TX_FULL != 0 {
                self.pull();
                continue;
            }
            // The FIFO's free room this look, and what is left to send.
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

    /// What the RX FIFO holds, onto `rx` (taken as packets by `take`).
    fn pull(&mut self) -> usize {
        let st = self.rd(UFSTAT);
        let n = if st & UFSTAT_RX_FULL != 0 { FIFO } else { st & UFSTAT_RX_COUNT } as usize;
        for _ in 0..n {
            let c = self.rd(URXH) as u8;
            self.rx.push(c);
        }
        n
    }

    /// The next whole H4 packet in `rx`: its type (2 ACL data, 4 event) and its bytes after the type byte. A byte that starts neither is the framing lost (H4 has no resync): it is dropped and counted, and every link ends (`desync`).
    fn take(&mut self) -> Option<(u8, Vec<u8>)> {
        loop {
            let b = &self.rx[self.rx_at..];
            let n = match b.first() {
                None => {
                    self.rx.clear();
                    self.rx_at = 0;
                    return None;
                }
                Some(4) if b.len() >= 3 => 3 + b[2] as usize,
                Some(2) if b.len() >= 5 => 5 + u16::from_le_bytes([b[3], b[4]]) as usize,
                Some(4) | Some(2) => return None,
                Some(&x) => {
                    if self.desyncs.is_power_of_two() || self.desyncs == 0 {
                        say!(self.grants, "bt: dropped a byte G#{x:02X} that starts no packet (A#{} so far)\n", self.desyncs + 1);
                    }
                    self.desyncs += 1;
                    self.rx_at += 1;
                    for ci in 0..self.conns.len() {
                        if !self.conns[ci].ending {
                            self.link_fault(ci, link::WHY_TRANSPORT);
                        }
                    }
                    continue;
                }
            };
            if b.len() < n {
                return None;
            }
            let p = (b[0], b[1..n].to_vec());
            self.rx_at += n;
            // What was taken is let go once it is a good part of the buffer.
            if self.rx_at >= 1 << 12 {
                self.rx.drain(..self.rx_at);
                self.rx_at = 0;
            }
            return Some(p);
        }
    }

    /// Every whole packet waiting, handled.
    fn pump(&mut self) {
        self.pull();
        while let Some((kind, p)) = self.take() {
            self.dispatch(kind, &p);
        }
    }

    fn dispatch(&mut self, kind: u8, p: &[u8]) {
        match kind {
            2 => self.on_acl(p),
            _ => self.on_event(p),
        }
    }

    /// An HCI event: [code][length][parameters].
    fn on_event(&mut self, e: &[u8]) {
        let d = &e[2..];
        match e[0] {
            // LE Meta: the subevent first.
            0x3E if !d.is_empty() => match d[0] {
                0x01 => self.connected(&d[1..]),
                0x02 => self.adv_report(&d[1..]),
                0x03 => self.updated(&d[1..]),
                0x07 => self.length(&d[1..]),
                0x0C => self.phy(&d[1..]),
                0x04 => {}
                _ => say!(self.grants, "bt: event {}\n", Hex(&e[..e.len().min(32)])),
            },
            // Disconnection Complete: [status][handle u16][reason].
            0x05 if d.len() >= 4 => {
                if d[0] == 0 {
                    self.conn_gone(u16::from_le_bytes([d[1], d[2]]) & 0x0FFF, d[3]);
                } else {
                    say!(self.grants, "bt: disconnection failed: status G#{:02X}\n", d[0]);
                }
            }
            0x13 => self.completed(d),
            // A late Command Complete or Status (a command given up on): nothing waits for it.
            0x0E | 0x0F => {}
            _ => say!(self.grants, "bt: event {}\n", Hex(&e[..e.len().min(32)])),
        }
    }

    /// An HCI command and its Command Complete (or Command Status): the event's parameters after the opcode (status first) in `e`, their length, or None after `ms` with nothing. Every other packet arriving meanwhile is handled as it comes.
    pub fn cmd(&mut self, op: u16, params: &[u8], ms: u64, e: &mut [u8; 258]) -> Option<usize> {
        let mut p = [0u8; 4 + 255];
        p[0] = 1;
        p[1..3].copy_from_slice(&op.to_le_bytes());
        p[3] = params.len() as u8;
        p[4..4 + params.len()].copy_from_slice(params);
        self.put(&p[..4 + params.len()]);
        let end = now() + freq() / 1000 * ms;
        while now() < end {
            self.pull();
            while let Some((kind, v)) = self.take() {
                // Command Complete: code G#0E, length, num packets, opcode, return parameters.
                if kind == 4 && v[0] == 0x0E && v.len() >= 5 && u16::from_le_bytes([v[3], v[4]]) == op {
                    let n = v.len() - 5;
                    e[..n].copy_from_slice(&v[5..]);
                    return Some(n);
                }
                // Command Status: code G#0F, length, status, num packets, opcode.
                if kind == 4 && v[0] == 0x0F && v.len() >= 6 && u16::from_le_bytes([v[4], v[5]]) == op {
                    e[0] = v[2];
                    return Some(1);
                }
                self.dispatch(kind, &v);
            }
        }
        say!(self.grants, "bt: no answer to G#{op:04X} (UMSTAT G#{:X}, UERSTAT G#{:X}, A#{} bytes waiting)\n", self.rd(UMSTAT), self.rd(UERSTAT), self.rx.len() - self.rx_at);
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
                        while let Some((kind, e)) = self.take() {
                            launched |= kind == 4 && e[0] == 0x0E && e.len() >= 6 && u16::from_le_bytes([e[3], e[4]]) == 0xFC4E && e[5] == 0;
                        }
                    }
                    if !launched {
                        return Err("Launch_RAM unanswered");
                    }
                    return Ok(records);
                }
                self.put(&p[..1 + n]);
                if self.pull() > 0 {
                    while self.take().is_some() {
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

    /// Power, the UART, the patch, and the chip at 4 Mbaud running it; then what the host side needs of it: the event masks (the LE Meta event, bit 61, is outside the controller's default mask: 2026-11-08 the scan heard nothing without it), the LE ACL buffers, the longest data length suggested for new links, and the static random address.
    fn up(&mut self) -> Result<(), &'static str> {
        if !ferros_sys::power(cap(self.grants, S_POWER), true) {
            return Err("power refused (WiFi's REG_ON low?)");
        }
        self.uart_up();
        sleep_ms(64);
        self.rx.clear();
        self.rx_at = 0;
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
        self.rx.clear();
        self.rx_at = 0;
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
        // The default mask (bits 0 to 44, Core Vol 4 Part E 7.3.1) with LE Meta (bit 61), and the LE events the host reads.
        let mask: u64 = (1 << 45) - 1 | 1 << 61;
        if !self.ok(0x0C01, &mask.to_le_bytes(), 256) || !self.ok(0x2001, &LE_EVENTS.to_le_bytes(), 256) {
            return Err("event masks refused");
        }
        // LE_Read_Buffer_Size: [status][packet length u16][packets u8]; a length of 0 means LE shares the BR/EDR buffers (Read_Buffer_Size: [status][ACL length u16][SCO length][ACL packets u16][SCO packets u16]).
        let (len, total) = match self.cmd(0x2002, &[], 256, &mut e) {
            Some(n) if n >= 4 && e[0] == 0 && e[1] | e[2] != 0 => (u16::from_le_bytes([e[1], e[2]]), e[3] as u16),
            _ => match self.cmd(0x1005, &[], 256, &mut e) {
                Some(n) if n >= 8 && e[0] == 0 => (u16::from_le_bytes([e[1], e[2]]), u16::from_le_bytes([e[4], e[5]])),
                _ => return Err("no ACL buffer size"),
            },
        };
        self.acl_len = len as usize;
        self.acl_total = total;
        self.acl_free = total;
        // LE Write Suggested Default Data Length: 251 octets and 2120 us (the longest at 1M), so a new link carries whole 247-byte K-frames in one LE PDU once the peer agrees.
        if !self.ok(0x2024, &[251, 0, 0x48, 0x08], 256) {
            say!(self.grants, "bt: the longer data length refused\n");
        }
        let a = self.addr;
        if !self.ok(0x2005, &a, 256) {
            return Err("the static random address refused");
        }
        self.reg = Reg::Static;
        say!(self.grants, "bt: LE ACL buffers: A#{} of A#{} bytes; the static random address set\n", total, len);
        Ok(())
    }

    /// The static random address in the register again, when the beacon is not holding it and nothing that uses it runs; the scan paused around the change. False while the beacon's address must stay (scans and connections then go out from it).
    pub fn own_static(&mut self) -> bool {
        if self.reg == Reg::Static {
            return true;
        }
        if self.beacon.is_some() || self.connecting.is_some() {
            return false;
        }
        let was = self.scan_on;
        if was {
            self.scan_set(false, false);
        }
        let a = self.addr;
        let ok = self.ok(0x2005, &a, 256);
        if ok {
            self.reg = Reg::Static;
        }
        if was {
            self.scan_update();
        }
        ok
    }

    /// Advertise `b`: advertising off, a fresh non-resolvable private address (the two top bits 00; the scan paused around the change, which the register allows only while nothing uses it), the parameters (non-connectable, random address, all three channels), the data (one complete list of 128-bit service UUIDs: the UUID little-endian on air, so byte 0, the most significant, last), advertising on.
    fn advertise(&mut self, b: &[u8; 16]) -> bool {
        self.ok(0x200A, &[0], 256);
        if self.connecting.is_some() {
            say!(self.grants, "bt: beacon refused while a connection is being made (the address register is in use)\n");
            return false;
        }
        let mut a = [0u8; 6];
        if ferros_sys::entropy(cap(self.grants, S_ENTROPY), &mut a) != 6 {
            say!(self.grants, "bt: no entropy for the address\n");
            return false;
        }
        a[5] &= 0x3F;
        let was = self.scan_on;
        if was {
            self.scan_set(false, false);
        }
        let ok = self.ok(0x2005, &a, 256);
        if ok {
            self.reg = Reg::Nrpa;
        }
        if was {
            self.scan_update();
        }
        if !ok {
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

    /// LE Advertising Reports (LE Meta 2): [count] then each report [event type][address type][address 6][data length][data][RSSI], one after another (as Linux reads them; this chip sends one a time for legacy scanning). Each goes to the sessions scanning, and while the pairing scan runs each 128-bit service UUID in it (AD types G#06 and G#07, little-endian on air, so byte 0, the most significant, last) is kept as heard now. Reports are never printed: a room full of advertisers would flood the console. Every length is the chip's and checked before it is read.
    fn adv_report(&mut self, d: &[u8]) {
        let Some(&count) = d.first() else { return };
        let mut at = 1;
        for _ in 0..count {
            let Some(head) = d.get(at..at + 9) else { return };
            let (kind, addr_type) = (head[0], head[1]);
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&head[2..8]);
            let len = head[8] as usize;
            let Some(data) = d.get(at + 9..at + 9 + len) else { return };
            let Some(&rssi) = d.get(at + 9 + len) else { return };
            at += 10 + len;
            self.reports += 1;
            if self.reports.is_power_of_two() {
                say!(self.grants, "bt: scan: {} advertising reports heard, {} with a 128-bit service UUID\n", self.reports, self.uuid_reports);
            }
            if self.pair_scan {
                let mut k = 0;
                while k + 1 < data.len() {
                    let l = data[k] as usize;
                    if l == 0 || k + 1 + l > data.len() {
                        break;
                    }
                    let (t, body) = (data[k + 1], &data[k + 2..k + 1 + l]);
                    if t == 0x06 || t == 0x07 {
                        self.uuid_reports += 1;
                        for u in body.chunks_exact(16) {
                            let mut id = [0u8; 16];
                            for j in 0..16 {
                                id[j] = u[15 - j];
                            }
                            self.heard_uuid(id);
                        }
                    }
                    k += 1 + l;
                }
            }
            let data = data.to_vec();
            self.adv_out(kind, addr_type, &addr, rssi, &data);
        }
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

    /// The controller's scan on (active: scan requests from the static address) or off. The duplicate filter stays off: a report heard again keeps its time fresh.
    pub fn scan_set(&mut self, on: bool, active: bool) -> bool {
        if !on {
            if !self.scan_on {
                return true;
            }
            self.scan_on = false;
            return self.ok(0x200C, &[0, 0], 256);
        }
        let s = SCAN.to_le_bytes();
        if !self.ok(0x200B, &[active as u8, s[0], s[1], s[0], s[1], 0x01, 0x00], 256) {
            return false;
        }
        if !self.ok(0x200C, &[1, 0], 256) {
            return false;
        }
        self.scan_on = true;
        self.scan_active = active;
        true
    }

    /// The scan as everyone asks it: on while the pairing scan or any session scans, active when any session asks active (the pairing scan is passive: nothing of it goes on the air), held off while a connection attempt has it paused.
    pub fn scan_update(&mut self) -> bool {
        let want = self.pair_scan || self.sessions.iter().any(|s| s.scan.is_some());
        let active = self.sessions.iter().any(|s| s.scan == Some(true));
        if self.scan_paused {
            return true;
        }
        if !want {
            return self.scan_set(false, false);
        }
        if self.scan_on && self.scan_active == active {
            return true;
        }
        if self.scan_on {
            self.scan_set(false, false);
        }
        if active && self.reg != Reg::Static && !self.own_static() {
            say!(self.grants, "bt: active scan from the beacon's address (the beacon holds the register)\n");
        }
        self.scan_set(true, active)
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
        let up_s = (now() - self.up_at) / freq();
        let _ = write!(out, "{}; beacon {}; links A#{}, channels A#{}, sessions A#{}; ACL buffers A#{} of A#{} free, A#{} packets out; chip restarts A#{}", core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?"), if self.beacon.is_some() { "advertising" } else { "off" }, self.conns.len(), self.conns.iter().map(|c| c.chans.len()).sum::<usize>(), self.sessions.len(), self.acl_free, self.acl_total, self.acl_out, self.restarts);
        if self.restarts > 0 {
            let _ = write!(out, ", the last A#{} s ago", (now() - self.restart_at) / freq());
        }
        let _ = write!(out, "; ACL in A#{}, PDUs in (signalling, ATT, SMP, data) A#{} A#{} A#{} A#{}, signalling out A#{}, stray A#{}", self.acl_in, self.pdus_in[0], self.pdus_in[1], self.pdus_in[2], self.pdus_in[3], self.sig_out, self.stray_acl);
        let _ = write!(out, "; up A#{up_s} s");
    }

    /// The chip lost under us (WiFi's driver cycled its REG_ON): every link gone with it, each client told (`EV_DISCONNECTED` why 1, its channels `WHY_CHIP`), a pending connection failed (`EV_CONNECTED` status G#FF), the ACL buffers and the address forgotten.
    fn chip_lost(&mut self) {
        for ci in 0..self.conns.len() {
            self.chans_gone(ci, link::WHY_CHIP);
        }
        for c in core::mem::take(&mut self.conns) {
            let mut body = [0u8; 4];
            body[..2].copy_from_slice(&c.handle.to_le_bytes());
            body[2] = 1;
            self.event(c.session, api::Ev::new(api::EV_DISCONNECTED, &body));
        }
        if let Some(p) = self.connecting.take() {
            let mut body = [0u8; 16];
            body[0] = 0xFF;
            body[3] = p.peer.0;
            body[4..10].copy_from_slice(&p.peer.1);
            self.event(p.session, api::Ev::new(api::EV_CONNECTED, &body));
        }
        self.todo.clear();
        self.scan_on = false;
        self.scan_paused = false;
        self.reg = Reg::Unset;
        self.acl_free = 0;
    }

    /// The work packets left for a command of its own.
    fn run_todo(&mut self) {
        let mut e = [0u8; 258];
        while let Some(t) = self.todo.pop_front() {
            match t {
                Todo::Disconnect(h, why) => {
                    let p = [(h & 0xFF) as u8, (h >> 8) as u8, why];
                    if self.cmd(0x0406, &p, 256, &mut e).map(|_| e[0]) != Some(0) {
                        say!(self.grants, "bt: disconnect of link G#{h:X} refused: G#{:02X}\n", e[0]);
                    }
                }
                Todo::Update(h, v) => {
                    let mut p = [0u8; 14];
                    p[0..2].copy_from_slice(&h.to_le_bytes());
                    for (k, x) in v.iter().enumerate() {
                        p[2 + 2 * k..4 + 2 * k].copy_from_slice(&x.to_le_bytes());
                    }
                    if self.cmd(0x2013, &p, 256, &mut e).map(|_| e[0]) != Some(0) {
                        say!(self.grants, "bt: parameter update of link G#{h:X} refused: G#{:02X}\n", e[0]);
                    }
                }
                Todo::Phy(h) => {
                    if let Some(n) = self.cmd(0x2030, &h.to_le_bytes(), 256, &mut e) {
                        let d = e[..n].to_vec();
                        self.phy(&d);
                    }
                }
                Todo::Cancel => {
                    self.ok(0x200E, &[], 256);
                }
                Todo::Scan => {
                    self.scan_update();
                }
                Todo::End(id) => self.end_session(id),
            }
        }
    }
}

/// The UART's interrupt, rung into `RING`: the line stays masked from its firing until the loop has cleared the UART's pending bits and acked it, so this thread wakes once for each time the loop waits.
extern "C" fn irq_thread(grants: u64) -> ! {
    loop {
        if ferros_sys::irq_wait_world(cap(grants, S_IRQ), 0) == ferros_sys::ERR {
            ferros_sys::thread_exit(1);
        }
        RING.fetch_add(1, SeqCst);
        ferros_sys::wake(&RING, 1);
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
    unsafe { ALLOC.init((&raw const __bss_end) as usize, input as usize, input_len as usize) };
    // The static random address scans and connections go out from, drawn once for this driver's life (BT.md "Addresses"): the two top bits 11, and its random part neither all zeros nor all ones (Core Vol 6 Part B 1.3.2.1).
    let mut addr = [0u8; 6];
    loop {
        if ferros_sys::entropy(cap(grants, S_ENTROPY), &mut addr) != 6 {
            say!(grants, "bt: no entropy for the static address\n");
            ferros_sys::exit(1);
        }
        addr[5] |= 0xC0;
        let rest_zero = addr[..5].iter().all(|&b| b == 0) && addr[5] == 0xC0;
        let rest_ones = addr[..5].iter().all(|&b| b == 0xFF) && addr[5] == 0xFF;
        if !rest_zero && !rest_ones {
            break;
        }
    }
    let mut bt = Bt { grants, rx: Vec::new(), rx_at: 0, name: [0; 64], name_len: 0, beacon: None, events_in_download: 0, desyncs: 0, pair_scan: false, heard: [([0; 16], 0); HEARD], reports: 0, uuid_reports: 0, scan_on: false, scan_active: false, scan_paused: false, addr, reg: Reg::Unset, acl_len: 0, acl_total: 0, acl_free: 0, conns: Vec::new(), connecting: None, sessions: Vec::new(), watched: 0, todo: VecDeque::new(), ident: 0, acl_out: 0, stray_acl: 0, acl_in: 0, pdus_in: [0; 4], sig_out: 0, restarts: 0, restart_at: 0, up_at: now() };
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
    bt.up_at = now();
    let mut l = Line { b: [0; 512], n: 0 };
    bt.state(&mut l);
    say!(grants, "bt: up: {}\n", core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
    if has_first {
        let ok = bt.advertise(&first);
        say!(grants, "bt: beacon {} {}\n", Hex(&first), if ok { "advertising" } else { "refused" });
    }
    bt.measures();
    // The request buffer: as long as the longest request yet (a SEND carries a whole SDU), grown when a longer one waits (`recv_long`).
    let mut msg: Vec<u8> = alloc::vec![0; 1 << 12];
    let mut token = [0u8; 32];
    let mut checked = now();
    let mut e = [0u8; 258];
    // The UART's interrupt line (a kernel from 2026-11-13), asked with a wait whose deadline has passed: ERR for a slot that is no line. With it, a thread waits on the line and rings `RING`.
    let n_grants = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) } as u64;
    let irq = n_grants > S_IRQ && ferros_sys::irq_wait(cap(grants, S_IRQ), 1) != ferros_sys::ERR && ferros_sys::thread_spawn(irq_thread, unsafe { &mut *(&raw mut IRQ_STACK) }, grants) != ferros_sys::ERR;
    say!(grants, "bt: {}\n", if irq { "waiting on the UART's interrupt line, a call, a client's end, and the 8 s look at the chip" } else { "no interrupt line granted (a kernel before 2026-11-13): waiting on a call and the 8 s look, the UART read every 4 ms while scanning or linked" });
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
        // A watched client ended: its session taken down at once.
        if RING.load(SeqCst) & RING_ENDED != 0 {
            RING.fetch_and(!RING_ENDED, SeqCst);
            bt.sweep(true);
        }
        // WiFi's driver pulling its REG_ON (a restart) resets this core to its ROM, the patch, the beacon and every link gone, the UART answering nothing at 4 Mbaud: every 8 s Read_Local_Version_Information asks, and silence brings the chip up again with the beacon and the scans it had.
        if now() - checked > freq() * 8 {
            checked = now();
            bt.sweep(false);
            if bt.cmd(0x1001, &[], 256, &mut e).is_none() {
                bt.restarts += 1;
                bt.restart_at = now();
                say!(grants, "bt: the chip stopped answering (WiFi restarted?): chip restart A#{}, A#{} s after the driver came up; bringing it up again\n", bt.restarts, (now() - bt.up_at) / freq());
                bt.chip_lost();
                let had = bt.beacon.take();
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
                if bt.pair_scan || bt.sessions.iter().any(|s| s.scan.is_some()) {
                    let ok = bt.scan_update();
                    say!(grants, "bt: scanning {}\n", if ok { "restored" } else { "refused" });
                }
                bt.measures();
                checked = now();
            }
        }
        bt.pump();
        bt.run_todo();
        bt.flush_out();
        let n = ferros_sys::recv_long(cap(grants, S_BT), &mut msg, &mut token, true);
        if let Some(len) = ferros_sys::too_long(n) {
            msg.resize(len, 0);
            continue;
        }
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            let n = n as usize;
            // Ops 1 to 5 are the identity service's (the beacon and the pairing scan): a kist, holding the endpoint thru the Bluetooth right, is refused them.
            let kist = matches!(msg[0], 1..=5) && ferros_sys::caller(&token).is_some_and(|h| h & ferros_sys::CALLER_KIST != 0);
            match msg[0] {
                _ if kist => {
                    reply(&token, &[1]);
                }
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
                    bt.pair_scan = true;
                    let ok = bt.scan_update();
                    if !ok {
                        bt.pair_scan = false;
                    }
                    say!(grants, "bt: scanning for pairing beacons: {}\n", if ok { "on" } else { "refused" });
                    reply(&token, &[!ok as u8]);
                }
                4 => {
                    bt.pair_scan = false;
                    let ok = bt.scan_update();
                    say!(grants, "bt: scanning for pairing beacons stopped\n");
                    reply(&token, &[!ok as u8]);
                }
                5 => {
                    let mut out = [0u8; 1 + 16 * 16];
                    let k = bt.heard_reply(&mut out);
                    reply(&token, &out[..k]);
                }
                6..=15 => {
                    let m = msg[..n].to_vec();
                    bt.op(&m, &token);
                }
                _ => {
                    reply(&token, &[]);
                }
            }
            bt.run_todo();
            bt.flush_out();
            continue;
        }
        // The deadlines: the next look at the chip (above), which finds a core WiFi's driver reset, and an unanswered channel open's RTX (the spec's retransmit, link.rs `rtx`). Until 2026-11-13 the loop also read the UART every 4 ms, some 250 wakes a second.
        let look = checked + freq() * 8 + 1;
        let until = bt.rtx().map_or(look, |r| r.min(look));
        waits += 1;
        // Read before the UART's pending bits are cleared: an interrupt after this changes it, and the wait below returns at once.
        let seen = RING.load(SeqCst);
        if irq {
            // The UART's pending bits cleared and its receive interrupt open, the line acked, then the FIFO read once more before the wait: bytes before the clear are taken now, bytes after raise the line.
            bt.wr(UINTP, 0xF);
            bt.wr(UINTM, UINTM_RX);
            ferros_sys::irq_ack(cap(grants, S_IRQ));
            if bt.pull() > 0 {
                continue;
            }
            ferros_sys::wait_world_or_call(&RING, seen, until);
        } else if bt.pair_scan || !bt.conns.is_empty() || bt.connecting.is_some() || bt.sessions.iter().any(|s| s.scan.is_some()) {
            // No line and something on the air: the UART read every 4 ms, as before.
            ferros_sys::wait_world_or_call(&RING, seen, now() + freq() / 256);
        } else {
            // No line and nothing asked of the air: the chip has nothing to say (an unsolicited event waits in the FIFO behind RTS until the next pass); a call or the next look ends the wait.
            ferros_sys::wait_world_or_call(&RING, seen, until);
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(bt_main);
