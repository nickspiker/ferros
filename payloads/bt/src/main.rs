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
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, sleep_until, NOTHING};

const S_BT: u64 = 1;
const S_UART: u64 = 2;
const S_PART: u64 = 3;
const S_POWER: u64 = 4;
const S_ENTROPY: u64 = 5;

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
                say!(self.grants, "bt: event {}\n", Hex(&e[..n.min(32)]));
            }
        }
        say!(self.grants, "bt: no answer to G#{op:04X} (UMSTAT G#{:X}, UERSTAT G#{:X}, {} bytes waiting: {}, {} dropped)\n", self.rd(UMSTAT), self.rd(UERSTAT), self.rx_len, Hex(&self.rx[..self.rx_len.min(16)]), self.dropped);
        None
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

    fn state(&self, out: &mut Line) {
        let _ = write!(out, "{}; beacon {}", core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?"), if self.beacon.is_some() { "advertising" } else { "off" });
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    // Input: 16 bytes to advertise from the start (a test without a caller). Taken first: a native payload's input lands right after its code, where `.bss` (the firmware chunk) is.
    let mut first = [0u8; 16];
    let has_first = input_len == 16;
    if has_first {
        for (k, v) in first.iter_mut().enumerate() {
            *v = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k as u64) as *const u8) };
        }
    }
    let mut bt = Bt { grants, rx: [0; 1024], rx_len: 0, name: [0; 64], name_len: 0, beacon: None, events_in_download: 0, dropped: 0 };
    if let Err(e) = bt.up() {
        say!(grants, "bt: bring-up failed: {e}\n");
        ferros_sys::power(cap(grants, S_POWER), false);
        ferros_sys::exit(1);
    }
    let mut l = Line { b: [0; 512], n: 0 };
    bt.state(&mut l);
    say!(grants, "bt: up: {}\n", core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
    if has_first {
        let ok = bt.advertise(&first);
        say!(grants, "bt: beacon {} {}\n", Hex(&first), if ok { "advertising" } else { "refused" });
    }
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut e = [0u8; 258];
    loop {
        if bt.pull() > 0 {
            while let Some(n) = bt.event(&mut e) {
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
                1 if n == 17 => {
                    let mut b = [0u8; 16];
                    b.copy_from_slice(&msg[1..17]);
                    let ok = bt.advertise(&b);
                    reply(&token, &[!ok as u8]);
                }
                2 => {
                    let ok = bt.ok(0x200A, &[0], 256);
                    bt.beacon = None;
                    reply(&token, &[!ok as u8]);
                }
                _ => {
                    reply(&token, &[]);
                }
            }
            continue;
        }
        sleep_ms(4);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
