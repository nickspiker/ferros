//! The NFC driver (docs/NFC.md): an EL0 process that runs the ST54L's NFC controller over NCI 2.0 on hsi2c_0 and answers as Photon's pairing tag, the same "dumb tag" Android's PhotonHceService is (photon docs/pairing-v2.md): under Photon's AID, SELECT → 9000, GET DATA → the live pairing secret S ‖ 9000 while a ceremony is live, else 6A82; anything else 6D00.
//!
//! The kernel keeps what a process cannot do behind the `nfc` power capability (kernel nfc.rs): the bus's pin modes and controller from cold, the chip's reset pulse, the PMU clock pad. Everything else is here, through MMIO grants: the bus's registers, and the interrupt line's level (gpa7-6, high while the chip holds a message).
//!
//! The chip, as Android brings it up (captured 2026-10-31 from Android's i2c trace and the August bugreport's NFC snoop): after reset it holds a CORE_RESET notification; CORE_RESET (keep config), CORE_INIT (NCI 2.0), ST's PROP_NFC_MODE_SET on (the chip resets into NFC mode and says so again), CORE_INIT again, ST's CORE_CONF_PROP, then the stack's RF set-up. The eSE (NFCEE G#86) is never touched: every listen route here goes to the host. Firmware and RF config are already the image's (version 02 06 17 50, config 76 DA): nothing is flashed.
//!
//! Spawn: ferros-bridge spawn nfc.bin --name nfc --serve nfc --hw hsi2c0 --hw gpio-alive --power nfc
//! Grant slots, in that order: 0 console, 1 the `nfc` endpoint (serve), 2 hsi2c_0, 3 gpio-alive, 4 the power switch.
//!
//! The `nfc` endpoint, first byte the op:
//! - `0` STATE: text (the chip's versions, discovery, counts).
//! - `1` SERVE + S (32 bytes): a ceremony is live; GET DATA answers S until STOP. → [0].
//! - `2` STOP: no ceremony; GET DATA answers 6A82. → [0].
//! - `3` READS: activations u32, S served u32, field-on events u32 (little-endian, 12 bytes): a ceremony's caller watches S served move to know the sponsor's phone read it.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, sleep_until, NOTHING};

const S_NFC: u64 = 1;
const S_BUS: u64 = 2;
const S_ALIVE: u64 = 3;
const S_POWER: u64 = 4;

const TARGET: u8 = 0x08;
/// gpa7's data word in gpio-alive (the bank at +G#C0): pin 6 is the chip's interrupt.
const GPA7_DAT: u64 = 0xC4;
const IRQ_PIN: u32 = 6;
/// Photon's AID (photon PhotonNfc.AID).
const AID: [u8; 8] = [0xF0, 0x50, 0x48, 0x4F, 0x54, 0x4F, 0x4E, 0x01];

// The HS-I2C controller (kernel hsi2c.rs, i2c-exynos5.c auto mode).
const CTL: u64 = 0x00;
const FIFO_CTL: u64 = 0x04;
const INT_ENABLE: u64 = 0x20;
const INT_STATUS: u64 = 0x24;
const FIFO_STATUS: u64 = 0x30;
const TX_DATA: u64 = 0x34;
const RX_DATA: u64 = 0x38;
const AUTO_CONF: u64 = 0x44;
const TRANS_STATUS: u64 = 0x50;
const ADDR: u64 = 0x70;
const CTL_MASTER: u32 = 1 << 3;
const CTL_RXCHON: u32 = 1 << 6;
const CTL_TXCHON: u32 = 1 << 7;
const INT_TRANSFER_DONE: u32 = 1 << 7;
const INT_ERRORS: u32 = 0xF << 8;
const INT_NODEV: u32 = 1 << 10;
const FIFO_TX_EMPTY: u32 = 1 << 8;
const FIFO_RX_EMPTY: u32 = 1 << 24;
const AUTO_READ: u32 = 1 << 16;
const AUTO_STOP_AFTER_TRANS: u32 = 1 << 17;
const AUTO_MASTER_RUN: u32 = 1 << 31;
const TRANS_MASTER_BUSY: u32 = 1 << 17;
const ADDR_MASTER_ID_7: u32 = 7 << 24;
/// Polls per wait, a syscall each: thousands of microseconds, far past a 400 kHz byte.
const WAIT_POLLS: u32 = 1 << 12;

const E_BUSY: u32 = 2;
const E_NODEV: u32 = 4;
const E_BUS: u32 = 8;
const E_TIMEOUT: u32 = 0x10;

/// An NCI packet: a 3-byte header and up to 255 payload bytes.
type Packet = [u8; 3 + 255];

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

struct Nfc {
    grants: u64,
    /// The live pairing secret, while a ceremony is.
    secret: Option<[u8; 32]>,
    /// Data credits on the static RF connection (conn 0), from the activation and the controller's credit notifications.
    credits: u32,
    /// An incoming APDU being reassembled from segments (PBF set).
    apdu: [u8; 512],
    apdu_len: usize,
    activations: u32,
    served: u32,
    fields: u32,
    tags: u32,
    version: [u8; 4],
    config: [u8; 2],
    discovering: bool,
}

impl Nfc {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(cap(self.grants, S_BUS), off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(cap(self.grants, S_BUS), off, v);
    }
    fn irq(&self) -> bool {
        mmio_read(cap(self.grants, S_ALIVE), GPA7_DAT) as u32 & 1 << IRQ_PIN != 0
    }

    fn fail(&self, st: u32) -> Option<u32> {
        if st & INT_NODEV != 0 {
            self.wr(INT_STATUS, st);
            return Some(E_NODEV);
        }
        if st & INT_ERRORS != 0 {
            self.wr(INT_STATUS, st);
            return Some(E_BUS);
        }
        None
    }

    fn wait_done(&self) -> Result<(), u32> {
        for _ in 0..WAIT_POLLS {
            let st = self.rd(INT_STATUS);
            if let Some(e) = self.fail(st) {
                return Err(e);
            }
            if st & INT_TRANSFER_DONE != 0 {
                self.wr(INT_STATUS, st);
                return Ok(());
            }
        }
        Err(E_TIMEOUT)
    }

    /// One auto-mode transfer with STOP: `tx` written, or `rx` filled. The RX FIFO (64 deep) is drained as the bytes come, so a read of any NCI packet's length works.
    fn transfer(&self, tx: &[u8], rx: &mut [u8]) -> Result<(), u32> {
        let mut idle = false;
        for _ in 0..WAIT_POLLS {
            if self.rd(TRANS_STATUS) & TRANS_MASTER_BUSY == 0 {
                idle = true;
                break;
            }
        }
        if !idle {
            return Err(E_BUSY);
        }
        let read = !rx.is_empty();
        let len = if read { rx.len() } else { tx.len() } as u32;
        self.wr(FIFO_CTL, if read { 0x0002_0023 } else { 0x0001_0013 });
        self.wr(CTL, CTL_MASTER | if read { CTL_RXCHON } else { CTL_TXCHON });
        self.wr(INT_STATUS, self.rd(INT_STATUS));
        self.wr(ADDR, ADDR_MASTER_ID_7 | (TARGET as u32) << 10);
        self.wr(INT_ENABLE, INT_TRANSFER_DONE | INT_ERRORS);
        let auto = len | if read { AUTO_READ } else { 0 } | AUTO_STOP_AFTER_TRANS;
        self.wr(AUTO_CONF, auto);
        self.wr(AUTO_CONF, auto | AUTO_MASTER_RUN);
        if read {
            let mut got = 0;
            let mut polls = 0;
            while got < rx.len() {
                if self.rd(FIFO_STATUS) & FIFO_RX_EMPTY == 0 {
                    rx[got] = self.rd(RX_DATA) as u8;
                    got += 1;
                    polls = 0;
                    continue;
                }
                if let Some(e) = self.fail(self.rd(INT_STATUS)) {
                    return Err(e);
                }
                polls += 1;
                if polls > WAIT_POLLS {
                    return Err(E_TIMEOUT);
                }
            }
            self.wait_done()
        } else {
            for &b in tx {
                let mut room = false;
                for _ in 0..WAIT_POLLS {
                    if self.rd(FIFO_STATUS) & 0x7F < 48 {
                        room = true;
                        break;
                    }
                    if let Some(e) = self.fail(self.rd(INT_STATUS)) {
                        return Err(e);
                    }
                }
                if !room {
                    return Err(E_TIMEOUT);
                }
                self.wr(TX_DATA, b as u32);
            }
            self.wait_done()?;
            for _ in 0..WAIT_POLLS {
                if self.rd(FIFO_STATUS) & FIFO_TX_EMPTY != 0 {
                    break;
                }
            }
            Ok(())
        }
    }

    /// One NCI packet out: the whole packet in one write. The chip NACKs while it is busy waking; the HAL retries, and so does this, every millisecond for 16.
    fn send(&self, p: &[u8]) -> Result<(), u32> {
        let mut r = Err(E_NODEV);
        for _ in 0..16 {
            r = self.transfer(p, &mut []);
            if r != Err(E_NODEV) {
                break;
            }
            sleep_ms(1);
        }
        r
    }

    /// One NCI packet in, if the chip holds one: the 3-byte header, then its payload in a second read (st21nfc.c's two-state read). Leading idle bytes (G#7E) in the header are dropped and made up with more reads; a header all idle is no packet.
    fn read(&self, p: &mut Packet) -> Result<Option<usize>, u32> {
        let mut h = [0u8; 3];
        self.transfer(&[], &mut h)?;
        let idle = h.iter().take_while(|&&b| b == 0x7E).count();
        if idle == 3 {
            return Ok(None);
        }
        if idle > 0 {
            h.copy_within(idle.., 0);
            self.transfer(&[], &mut h[3 - idle..])?;
        }
        p[..3].copy_from_slice(&h);
        let n = h[2] as usize;
        if n > 0 {
            self.transfer(&[], &mut p[3..3 + n])?;
        }
        Ok(Some(3 + n))
    }

    /// Wait up to `ms` for the interrupt line, then read what the chip holds.
    fn next(&mut self, p: &mut Packet, ms: u64) -> Option<usize> {
        let end = now() + freq() / 1000 * ms;
        loop {
            if self.irq() {
                match self.read(p) {
                    Ok(Some(n)) => return Some(n),
                    Ok(None) => {}
                    // Fresh out of reset the chip raises its line before it answers its address: tried again next millisecond.
                    Err(E_NODEV) => {}
                    Err(e) => {
                        say!(self.grants, "nfc: read failed G#{e:X}\n");
                        return None;
                    }
                }
            }
            if now() > end {
                return None;
            }
            sleep_ms(1);
        }
    }

    /// A command and its response (MT 2, the same GID and OID); notifications that come first are handled on the way. None on no response in 512 ms.
    fn cmd(&mut self, c: &[u8], p: &mut Packet) -> Option<usize> {
        if let Err(e) = self.send(c) {
            say!(self.grants, "nfc: write {} failed G#{e:X}\n", Hex(c));
            return None;
        }
        let end = now() + freq() / 2;
        while now() < end {
            let Some(n) = self.next(p, 512) else { break };
            if p[0] >> 5 == 2 && p[0] & 0xF == c[0] & 0xF && p[1] & 0x3F == c[1] & 0x3F {
                return Some(n);
            }
            self.packet(p, n);
        }
        say!(self.grants, "nfc: no response to {}\n", Hex(c));
        None
    }

    /// A command whose response must report status OK (payload byte 0 = 0): its response's length, else a line on the console.
    fn ok(&mut self, c: &[u8], p: &mut Packet) -> Option<usize> {
        let n = self.cmd(c, p)?;
        if n < 4 || p[3] != 0 {
            say!(self.grants, "nfc: {} → {}\n", Hex(c), Hex(&p[..n]));
            return None;
        }
        Some(n)
    }

    /// A packet that is not the awaited response: notifications and data.
    fn packet(&mut self, p: &Packet, n: usize) {
        let mt = p[0] >> 5;
        let pl = &p[3..n];
        match (mt, p[0] & 0xF, p[1] & 0x3F) {
            // Data on the static RF connection: an APDU (segments gathered until PBF clears).
            (0, 0, _) => {
                let k = pl.len().min(self.apdu.len() - self.apdu_len);
                self.apdu[self.apdu_len..self.apdu_len + k].copy_from_slice(&pl[..k]);
                self.apdu_len += k;
                if p[0] & 0x10 == 0 {
                    let mut a = [0u8; 512];
                    let len = self.apdu_len;
                    a[..len].copy_from_slice(&self.apdu[..len]);
                    self.apdu_len = 0;
                    self.apdu(&a[..len]);
                }
            }
            // CORE_RESET_NTF: the chip reset itself (a reason in byte 0: 1 = the host asked, A0 = into NFC mode).
            (3, 0, 0) => {
                if pl.len() >= 2 {
                    say!(self.grants, "nfc: reset notification, reason G#{:02X}, config {}\n", pl[0], if pl[1] == 1 { "kept" } else { "reset" });
                }
                // After the reason, the config status, NCI version and manufacturer id: ST's manufacturer info (its length, then two bytes, the firmware's four, ..., the RF config's two, a mode byte).
                if pl.len() >= 31 {
                    self.version.copy_from_slice(&pl[7..11]);
                    self.config.copy_from_slice(&pl[28..30]);
                }
            }
            // CORE_CONN_CREDITS_NTF: entries of (conn id, credits).
            (3, 0, 6) => {
                for e in pl.get(1..).unwrap_or(&[]).chunks(2) {
                    if e.len() == 2 && e[0] == 0 {
                        self.credits += e[1] as u32;
                    }
                }
            }
            // RF_INTF_ACTIVATED_NTF: a reader selected us; byte 5 is the connection's initial credits.
            (3, 1, 5) => {
                // Byte 3 is the technology and mode: below G#80 we polled and found a tag (or a phone answering as one). Its technology parameters follow byte 6 (their length): for NFC-A the SENS_RES, then the UID's length and the UID. Logged, then the tag let go back to discovery (RF_DEACTIVATE, type 3).
                let mode = pl.get(3).copied().unwrap_or(0);
                if mode < 0x80 {
                    self.tags += 1;
                    let tl = pl.get(6).copied().unwrap_or(0) as usize;
                    let tp = pl.get(7..7 + tl).unwrap_or(&[]);
                    let uid = if mode == 0 { tp.get(2).and_then(|&l| tp.get(3..3 + l as usize)).unwrap_or(&[]) } else { &[] };
                    say!(self.grants, "nfc: tag: mode G#{mode:02X}, protocol G#{:02X}, interface G#{:02X}, UID {}, parameters {}\n", pl.get(2).copied().unwrap_or(0), pl.get(1).copied().unwrap_or(0), Hex(uid), Hex(tp));
                    if let Err(e) = self.send(&[0x21, 0x06, 0x01, 0x03]) {
                        say!(self.grants, "nfc: release failed G#{e:X}\n");
                    }
                    return;
                }
                self.activations += 1;
                self.credits = pl.get(5).copied().unwrap_or(1) as u32;
                self.apdu_len = 0;
                say!(self.grants, "nfc: activated: interface G#{:02X}, protocol G#{:02X}, mode G#{:02X}\n", pl.get(1).copied().unwrap_or(0), pl.get(2).copied().unwrap_or(0), pl.get(3).copied().unwrap_or(0));
            }
            // RF_DEACTIVATE_NTF: the reader went; back to discovery (type 3) the controller carries on by itself, anything else needs discovery started again.
            (3, 1, 6) => {
                let kind = pl.first().copied().unwrap_or(0);
                say!(self.grants, "nfc: deactivated, type G#{kind:02X}\n");
                if kind != 3 {
                    self.discovering = false;
                }
            }
            // RF_FIELD_INFO_NTF: a reader's field came or went.
            (3, 1, 7) => {
                if pl.first() == Some(&1) {
                    self.fields += 1;
                }
            }
            // The RF_DEACTIVATE a tag's release sent: its answer.
            (2, 1, 6) => {}
            // ST's proprietary notifications (G#6F): logging the chip keeps; not ours.
            (3, 0xF, _) => {}
            _ => say!(self.grants, "nfc: {}\n", Hex(&p[..n])),
        }
    }

    /// The tag's two commands (PhotonHceService.processCommandApdu): SELECT by name → 9000 for Photon's AID (6A82 for any other: the controller routes every ISO-DEP AID here), GET DATA → S ‖ 9000 or 6A82, anything else 6D00.
    fn apdu(&mut self, a: &[u8]) {
        let mut r = [0u8; 3 + 34];
        let n = if a.len() >= 4 && a[0] == 0x00 && a[1] == 0xA4 && a[2] == 0x04 {
            let lc = a.get(4).copied().unwrap_or(0) as usize;
            let mine = a.get(5..5 + lc) == Some(&AID[..]);
            r[3..5].copy_from_slice(if mine { &[0x90, 0x00] } else { &[0x6A, 0x82] });
            2
        } else if a.len() >= 2 && a[0] == 0x80 && a[1] == 0xCA {
            match self.secret {
                Some(s) => {
                    r[3..35].copy_from_slice(&s);
                    r[35..37].copy_from_slice(&[0x90, 0x00]);
                    self.served += 1;
                    34
                }
                None => {
                    r[3..5].copy_from_slice(&[0x6A, 0x82]);
                    2
                }
            }
        } else {
            r[3..5].copy_from_slice(&[0x6D, 0x00]);
            2
        };
        r[2] = n as u8;
        // A reply sent with no credit breaks the controller's flow control (NCI 2.0 4.4.4): said, not hidden, and sent anyway, since the reader is waiting on it.
        if self.credits == 0 {
            say!(self.grants, "nfc: replying without a credit\n");
        } else {
            self.credits -= 1;
        }
        if let Err(e) = self.send(&r[..3 + n]) {
            say!(self.grants, "nfc: reply failed G#{e:X}\n");
        }
        say!(self.grants, "nfc: APDU {} → {}\n", Hex(&a[..a.len().min(16)]), if n == 34 { "S" } else { "status" });
    }

    /// Listening as a card on NFC-A and NFC-B, and between listens polling for tags on NFC-A, NFC-B and NFC-V (each entry a technology and mode, then 1: every discovery period), as Android interleaves them.
    fn discover(&mut self, p: &mut Packet) -> bool {
        self.discovering = self.ok(&[0x21, 0x03, 0x0B, 0x05, 0x00, 0x01, 0x01, 0x01, 0x06, 0x01, 0x80, 0x01, 0x81, 0x01], p).is_some();
        self.discovering
    }

    /// From reset to listening (the module comment's sequence).
    fn init(&mut self, p: &mut Packet) -> bool {
        // The reset notification the pulse left.
        if let Some(n) = self.next(p, 64) {
            self.packet(p, n);
        }
        let steps: [&[u8]; 4] = [
            &[0x20, 0x00, 0x01, 0x01],
            &[0x20, 0x01, 0x02, 0x00, 0x00],
            // PROP_NFC_MODE_SET on: the chip resets into NFC mode (a reset notification, reason A0).
            &[0x2F, 0x02, 0x02, 0x02, 0x01],
            &[0x20, 0x01, 0x02, 0x00, 0x00],
        ];
        for (k, c) in steps.iter().enumerate() {
            if self.ok(c, p).is_none() {
                return false;
            }
            if k == 2 {
                if let Some(n) = self.next(p, 64) {
                    self.packet(p, n);
                }
            }
        }
        let rf: [&[u8]; 8] = [
            // CORE_CONF_PROP (libnfc-hal-st.conf).
            &[0x20, 0x02, 0x0A, 0x03, 0xA1, 0x01, 0x1E, 0xA2, 0x01, 0x19, 0x80, 0x01, 0x01],
            // The discover map: ISO-DEP and NFC-DEP to their interfaces, as Android maps them.
            &[0x21, 0x00, 0x07, 0x02, 0x04, 0x03, 0x02, 0x05, 0x03, 0x03],
            // NFCC_CONFIG_CONTROL: the controller may manage its RF configuration.
            &[0x20, 0x02, 0x04, 0x01, 0x85, 0x01, 0x01],
            // Power sub-state: screen on, unlocked.
            &[0x20, 0x09, 0x01, 0x00],
            // Listen routing, all to the host: the empty-AID default, the ISO-DEP protocol, technologies A and B.
            &[0x21, 0x01, 0x15, 0x00, 0x04, 0x52, 0x02, 0x00, 0x11, 0x41, 0x03, 0x00, 0x11, 0x04, 0x00, 0x03, 0x00, 0x39, 0x00, 0x00, 0x03, 0x00, 0x39, 0x01],
            // Listen parameters: SDD bit frame 4, platform 0, SEL_INFO ISO-DEP, SENSB_INFO 1, NFC-F protocol 0.
            &[0x20, 0x02, 0x10, 0x05, 0x30, 0x01, 0x04, 0x31, 0x01, 0x00, 0x32, 0x01, 0x20, 0x38, 0x01, 0x01, 0x50, 0x01, 0x00],
            // Field-info notifications on.
            &[0x20, 0x02, 0x04, 0x01, 0x80, 0x01, 0x01],
            // TOTAL_DURATION 500 ms.
            &[0x20, 0x02, 0x05, 0x01, 0x00, 0x02, 0xF4, 0x01],
        ];
        for c in rf {
            if self.ok(c, p).is_none() {
                return false;
            }
        }
        self.discover(p)
    }

    fn state(&self, out: &mut Line) {
        let _ = write!(out, "firmware {}, RF config {}; {}; S {}; activations {}, S served {}, fields {}, tags {}", Hex(&self.version), Hex(&self.config), if self.discovering { "listening (A, B), polling (A, B, V)" } else { "not listening" }, if self.secret.is_some() { "live" } else { "none" }, self.activations, self.served, self.fields, self.tags);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut nfc = Nfc { grants, secret: None, credits: 0, apdu: [0; 512], apdu_len: 0, activations: 0, served: 0, fields: 0, tags: 0, version: [0; 4], config: [0; 2], discovering: false };
    let mut p: Packet = [0; 258];
    if !ferros_sys::power(cap(grants, S_POWER), true) {
        say!(grants, "nfc: power on failed (HSI1 off?)\n");
        ferros_sys::exit(1);
    }
    let up = nfc.init(&mut p);
    let mut l = Line { b: [0; 512], n: 0 };
    nfc.state(&mut l);
    say!(grants, "nfc: {}: {}\n", if up { "up" } else { "bring-up failed" }, core::str::from_utf8(&l.b[..l.n]).unwrap_or(""));
    let mut msg = [0u8; 64];
    let mut token = [0u8; 32];
    let mut retry_at = 0u64;
    loop {
        if nfc.irq() {
            match nfc.read(&mut p) {
                Ok(Some(n)) => nfc.packet(&p, n),
                Ok(None) => {}
                Err(e) => say!(grants, "nfc: read failed G#{e:X}\n"),
            }
            continue;
        }
        // Discovery stopped (a deactivation to idle, or a bring-up that failed at the end): started again, at most once a second.
        if !nfc.discovering && now() > retry_at {
            retry_at = now() + freq();
            nfc.discover(&mut p);
        }
        let n = recv(cap(grants, S_NFC), &mut msg, &mut token, true);
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            match msg[0] {
                0 => {
                    let mut l = Line { b: [0; 512], n: 0 };
                    nfc.state(&mut l);
                    reply(&token, &l.b[..l.n]);
                }
                1 if n == 33 => {
                    let mut s = [0u8; 32];
                    s.copy_from_slice(&msg[1..33]);
                    nfc.secret = Some(s);
                    reply(&token, &[0]);
                }
                2 => {
                    nfc.secret = None;
                    reply(&token, &[0]);
                }
                3 => {
                    let mut r = [0u8; 12];
                    r[..4].copy_from_slice(&nfc.activations.to_le_bytes());
                    r[4..8].copy_from_slice(&nfc.served.to_le_bytes());
                    r[8..].copy_from_slice(&nfc.fields.to_le_bytes());
                    reply(&token, &r);
                }
                _ => {
                    reply(&token, &[]);
                }
            }
            continue;
        }
        sleep_ms(1);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
