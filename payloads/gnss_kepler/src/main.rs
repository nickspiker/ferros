//! The 8a's GPS driver (docs/GPS.md "akita's KEPLER"): an EL0 process that runs the KEPLER GNSS core of the S5300 modem over spi_21 and serves its fixes on the `gps` endpoint exactly as payloads/gps does on the Pro, so the map's GPS rings, the GPS page and the GPS puck work unchanged.
//!
//! The kernel keeps what a process cannot do: the port's clocks and USI mode, its clock halved to 3.07 MHz, and the two pins' modes, behind the `gps` power switch (kernel gps.rs `kepler_power` on the 8a). Everything else is here thru MMIO grants: the port's registers and the data bits of the two lines, AP to GNSS gpp4-3 (PERIC0 +G#84 bit 3) and GNSS to AP gpa5-3 (FAR_ALIVE +G#4 bit 3, also its interrupt, `gps-host-req`).
//!
//! The wire (gnssif_spi: gnss_link_device.c, the 2026-11-18 captures):
//! - The port runs 32-bit words, each word's little-endian bytes in wire order (SWAP_CFG G#DD, MODE_CFG G#5FFC_0000, the controller's select in auto mode G#23), at most 16 words a transfer.
//! - A send: AP to GNSS up, the chip's ready (GNSS to AP up, Linux waits at most 100 ms), the packet's words, then a read of as many words (the chip's bytes, when it has some, come in it), AP to GNSS down.
//! - A read: the chip raises GNSS to AP; the AP pulses AP to GNSS for 100 µs and reads 1024 bytes in 16 transfers, again while the line stays up.
//! - Above it CHPP (ferros_kepler): the transport reset, discovery, the time, the BETP service's open, then BETP's records; every chip packet with a payload acked by a bare ack, as gnssd does.
//!
//! The session, as gnssd ran it for Maps (the only AP messages either capture holds): at the service's open G#0003_7304 (four zero bytes), then G#0002_12DE (no bytes) and G#0002_04D4 (G#0F); STOP sends G#0002_12DE again, as Maps' close did. The chip then sends a fix a second (G#0143, SiRF's MID 67 SID 1) and its satellites (G#710A), among some 25 other kinds of record, counted by kind (op 4).
//! No aiding goes to the chip: gnssd sent none in the captured session (no time, no place), and the chip fixed in 10 s indoors on what it keeps itself. The rough place feeds only the predicted birds (payloads/gps predict.rs, the same file).
//!
//! The core first: the CP powers it, resets it and loads its firmware (GPS.md "The core is the CP's"), so before the link the driver boots it thru the modem driver's `rmnet` ops in gnssd's order (`core_boot`): the boot count, a BLC, the reset held and released, kepler.bin and its chip configuration copied into GNSS_FW from the vault (`gps.kepler`, `gps.kepler.cfg`: Samsung's, the 8a's own, never in a repo) and loaded, the start BLC. A core with no firmware running never raises its ready line; a failed boot is tried again every `RETRY_S`.
//!
//! Spawn (the kernel's `gps` grants, payloads/gps's order): ferros-bridge spawn gnss_kepler.bin 01 --name gps --serve gps --hw usi3-spi --hw gpio-far-alive --hw gpio-gpp0 --hw gpio-hsi2 --store gps --power gps --irq gps-host-req --use udp --use rmnet
//! Grant slots: 0 console, 1 the `gps` endpoint, 2 the SPI port, 3 gpio-far-alive, 4 gpio-gpp0, 5 gpio-hsi2 (not used on the 8a), 6 the vault scope `gps.`, 7 the power switch, 8 GNSS to AP's interrupt, 9 `udp` (the orbits), 10 `rmnet` (the modem driver: the core's boot).
//! Input byte 0: bit 4 the link alone (reset, discovery, time, open; no session), for a first look at a chip; bit 5 one bare 16-word read first (the port alone, no handshake: does the transfer run, how does the level count); bit 6 no boot (the core already runs); anything else (the kernel's start passes 01) the whole session.
//!
//! The `gps` endpoint, payloads/gps's ops and layouts: 0 FIX (38 bytes), 1 STOP, 2 SATS (25 bytes a signal), 3 WATCH, 4 FRAMES (here the kinds are BETP words: key `00 00` then the word's four bytes, high first; `[4, k]` answers with no frames kept).
//! Measures: slot 0 the state (1 searching, 2 a fix), 1 the error estimate, 2 the satellites seen, 3 those used, 4 the best C/N0, 5 altitude, 6 speed, 7 heading.
#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write;
use ferros_kepler as kp;
use ferros_sys::{cap, console, freq, mmio_read, mmio_write, now, recv, reply, store_set, NOTHING};

#[path = "../../gps/src/net.rs"]
mod net;
#[path = "../../gps/src/predict.rs"]
mod predict;
use predict::Predict;

const S_GPS: u64 = 1;
const S_SPI: u64 = 2;
const S_FAR: u64 = 3;
const S_PERIC0: u64 = 4;
const S_STORE: u64 = 6;
const S_POWER: u64 = 7;
const S_IRQ: u64 = 8;

/// The controller (s3c64xx, ferros_spi's offsets).
const CH_CFG: u64 = 0x00;
const MODE_CFG: u64 = 0x08;
const SLAVE_SEL: u64 = 0x0C;
const STATUS: u64 = 0x14;
const TX_DATA: u64 = 0x18;
const RX_DATA: u64 = 0x1C;
const PACKET_CNT: u64 = 0x20;
const SWAP_CFG: u64 = 0x28;
const FB_CLK: u64 = 0x2C;
const CH_TX: u32 = 1 << 0;
const CH_RX: u32 = 1 << 1;
const CH_SW_RST: u32 = 1 << 5;
const PACKET_CNT_EN: u32 = 1 << 16;
/// Linux's values for a 32-bit transfer (the loud capture): channel and bus 32 bits wide with the most trailing bytes, both directions' bytes and halves swapped, the select automatic.
const MODE_32: u32 = 0x5FFC_0000;
const SWAP_32: u32 = 0xDD;
const SEL_AUTO: u32 = 0x23;
/// Words in one transfer: the 64-byte FIFO.
const WORDS: usize = 1 << 4;
/// A read: 1024 bytes (gnss_spi.h `DEFAULT_SPI_RX_SIZE`).
const READ: usize = 1 << 10;

/// The lines: AP to GNSS in PERIC0's gpp4 data word, GNSS to AP in FAR_ALIVE's gpa5 data word, both pin 3.
const AP2GNSS_DAT: u64 = 0x84;
const GNSS2AP_DAT: u64 = 0x04;
const PIN: u32 = 1 << 3;
/// FAR_ALIVE's external-interrupt registers for gpa5 (payloads/gps): control (a nibble a pin, 1 = high level), mask, pending; the pin's function at +0 and its pull at +8.
const EINT_CON: u64 = 0x700;
const EINT_MASK: u64 = 0x900;
const EINT_PEND: u64 = 0xA00;

/// A dead chip (no ready answer, no reset ack) is tried again this often: a real deadline, the chip says nothing while it waits for its firmware.
const RETRY_S: u64 = 1 << 4;
/// A payload not acked, or a request not answered, is sent again after this (the link's own timeout, A#1000 ms in both ends' reset).
const RESEND_MS: u64 = 1 << 10;
/// Sends of one packet before its step is given up and the link reset.
const TRIES: u8 = 1 << 3;

fn ms(f: u64, n: u64) -> u64 {
    f / 1000 * n
}

/// A small text line for the console.
struct Line {
    b: [u8; 256],
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
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

/// The port and the two lines thru their grants, and what the transfers have counted.
struct Chip {
    spi: u64,
    far: u64,
    peric0: u64,
    /// Transfers whose receive level reached four bytes a word (the level counts bytes), and those that reached only one a word before the limit (it counts words): measured on the first runs.
    lvl_bytes: u32,
    lvl_words: u32,
    /// Transfers that never filled, and sends whose ready answer never came.
    stuck: u32,
    no_ready: u32,
    /// Sends that found GNSS to AP still up after their 2 ms wait.
    busy: u32,
    /// When the last send ended (the gap before the next).
    sent_at: u64,
}

impl Chip {
    fn rd(&self, off: u64) -> u32 {
        mmio_read(self.spi, off) as u32
    }
    fn wr(&self, off: u64, v: u32) {
        mmio_write(self.spi, off, v);
    }
    fn ap2gnss(&self, high: bool) {
        let v = mmio_read(self.peric0, AP2GNSS_DAT) as u32;
        mmio_write(self.peric0, AP2GNSS_DAT, if high { v | PIN } else { v & !PIN });
    }
    fn gnss2ap(&self) -> bool {
        mmio_read(self.far, GNSS2AP_DAT) as u32 & PIN != 0
    }

    /// One transfer of `rx.len() / 4` words (at most `WORDS`): `tx`'s words out when given (what comes back in the same clocks is the chip's and kept), else a read alone. False when the words never came.
    fn words(&mut self, tx: Option<&[u8]>, rx: &mut [u8]) -> bool {
        let n = rx.len() / 4;
        self.wr(CH_CFG, 0);
        self.wr(SWAP_CFG, SWAP_32);
        self.wr(MODE_CFG, MODE_32);
        self.wr(FB_CLK, 0);
        self.wr(SLAVE_SEL, SEL_AUTO);
        self.wr(PACKET_CNT, 0);
        self.wr(PACKET_CNT, n as u32 | PACKET_CNT_EN);
        if let Some(t) = tx {
            for w in t.chunks_exact(4) {
                self.wr(TX_DATA, u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
            }
        }
        self.wr(CH_CFG, if tx.is_some() { CH_TX | CH_RX } else { CH_RX });
        // 16 words at 3.07 MHz take 167 µs; 2^-9 s (about 2 ms) is a transfer that went wrong.
        let (t0, limit) = (now(), freq() >> 9);
        let mut ok = false;
        loop {
            let lvl = ((self.rd(STATUS) >> 15) & 0x1FF) as usize;
            if lvl >= 4 * n {
                self.lvl_bytes += 1;
                ok = true;
                break;
            }
            if now().wrapping_sub(t0) > limit {
                // Linux reads the level as bytes (s3c64xx `wait_for_pio`: `cpy_len / 4` words); a level of one a word at the limit says it counts words here, and the words are read all the same.
                if lvl >= n {
                    self.lvl_words += 1;
                    ok = true;
                }
                break;
            }
        }
        if ok {
            for o in rx.chunks_exact_mut(4) {
                o.copy_from_slice(&self.rd(RX_DATA).to_le_bytes());
            }
        } else {
            self.stuck += 1;
        }
        self.wr(CH_CFG, 0);
        self.wr(PACKET_CNT, 0);
        self.wr(CH_CFG, CH_SW_RST);
        self.wr(CH_CFG, 0);
        ok
    }
}

/// The scanner holds a partial packet and one read's chunk.
type Scan = kp::Scanner<{ kp::PACKET_MAX * 2 + READ }>;
/// The longest datagram the chip sent was 1116 bytes (994 + 122); room for some times that.
type Gram = kp::Datagram<{ 1 << 13 }>;

/// Where the link stands.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Reset,
    Discover,
    Time,
    Open,
    Start,
    /// After a core the CP freshly started: gnssd's two first records (ferros_kepler `AP_FRESH_00E4`, `AP_FRESH_08AD`).
    Fresh0,
    Fresh1,
    /// gnssd's own session as traced with the location HAL connected (the vault's `gps.kepler.session`: each record `word u32, length u16, bytes`, gnss-oracle GNSS_REPLAY_OUT), played a record at a time, each acked, up to its second G#0465 (where gnssd asked again).
    Script,
    SessionOpen,
    SessionAsk,
    Running,
    /// The link alone was asked for (input bit 4): open, nothing more sent.
    Idle,
    /// Nothing answered: the next try at `retry_at`.
    Dead,
}

/// The kinds of BETP record seen: the word, how many, their bytes, when the latest came.
const KINDS: usize = 1 << 5;
#[derive(Clone, Copy, Default)]
struct Kind {
    word: u32,
    count: u32,
    bytes: u32,
    at: u64,
}

/// The fix in payloads/gps's units.
#[derive(Clone, Copy, Default)]
struct Fix {
    lat: i32,
    lon: i32,
    alt: i32,
    err: u32,
    speed: i32,
    track: i32,
    has_track: bool,
}

/// Everything the packets change.
struct State {
    link: kp::Link,
    phase: Phase,
    /// A bare ack owed for the chip's latest payload packet, and a reset ack owed for a reset the chip started.
    owe_ack: bool,
    owe_reset_ack: bool,
    nak: bool,
    /// The step's packet: its bytes, the chip's ack when it went (its sequence), when, how many times.
    pend: [u8; 512],
    pend_n: usize,
    pend_seq: u8,
    pend_at: u64,
    tries: u8,
    acked: bool,
    answered: bool,
    retry_at: u64,
    /// The chip's packets, payload packets, datagrams, CHPP errors it reported.
    packets: u32,
    grams: u32,
    chip_errs: u32,
    chip_time_ns: u64,
    /// The counter when the chip's time came: its clock now is `chip_time_ns` plus the counter since.
    chip_time_at: u64,
    fixes: u32,
    fix: Fix,
    fix_at: u64,
    first_fix_said: bool,
    svs: [kp::Sv; ferros_sys::TRACKS_MAX],
    nsv: usize,
    sv_msgs: u32,
    sv_at: u64,
    kinds: [Kind; KINDS],
    nkinds: usize,
    kinds_over: u32,
    /// The core was just started by the CP thru this driver: gnssd's fresh-boot records go first.
    fresh: bool,
    /// Input bit 7: the first packets each way said by their header.
    trace: bool,
    /// gnssd's traced session (`Phase::Script`): the records, where the next starts, where the play ends, how many went.
    script: alloc::vec::Vec<u8>,
    script_at: usize,
    script_end: usize,
    script_sent: u32,
}

/// GPS time now from the wall clock in the preferences block (payloads/gps `gps_ms_now`): milliseconds since 1980-01-06, UTC + 18 leap seconds; None before the kernel has the time.
fn gps_ms_now() -> Option<u64> {
    let base = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + ferros_sys::PREFS + 16) as *const u64) };
    if base == 0 {
        return None;
    }
    let (f, t) = (freq(), base.wrapping_add(now()));
    Some((t / f * 1000 + t % f * 1000 / f).checked_sub(315_964_800_000)? + 18_000)
}

/// The script's record at `script_at`: its word, where its bytes start, their length; None at the play's end.
fn script_record(s: &State) -> Option<(u32, usize, usize)> {
    let b = &s.script;
    let at = s.script_at;
    if at + 6 > s.script_end {
        return None;
    }
    let word = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let len = u16::from_le_bytes([b[at + 4], b[at + 5]]) as usize;
    // WHY the check: the lengths are the vault's; a record running past the blob ends the play.
    if at + 6 + len > s.script_end {
        return None;
    }
    Some((word, at + 6, len))
}

fn script_next(s: &State) -> usize {
    script_record(s).map_or(s.script_end, |(_, at, len)| at + len)
}

/// Where the play ends: the second G#0465 record (gnssd asked again there, with Maps' close not far after), else the blob's end.
fn script_end(b: &[u8]) -> usize {
    let (mut at, mut seen) = (0usize, 0u32);
    while at + 6 <= b.len() {
        let w = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let len = u16::from_le_bytes([b[at + 4], b[at + 5]]) as usize;
        if w & 0xFFFF == 0x0465 {
            seen += 1;
            if seen == 2 {
                return at;
            }
        }
        at += 6 + len;
    }
    b.len().min(at)
}

/// Every packet the scanner holds, through the link and the datagram, into the state.
fn drain(g: u64, sc: &mut Scan, dg: &mut Gram, s: &mut State) {
    loop {
        let (h, payload) = match sc.next() {
            kp::Found::Packet(h, p) => (h, p),
            kp::Found::More => return,
        };
        s.packets += 1;
        // The first 64 packets said by their header (input bit 7): what the link does, never a payload byte.
        if s.trace && s.packets <= 1 << 6 {
            say!(g, "gps: kepler: in  seq G#{:02X} ack G#{:02X} code G#{:02X} flags G#{:02X} len A#{}\n", h.seq, h.ack, h.code, h.flags, h.len);
        }
        s.link.saw(&h);
        if h.code & 0x0F != 0 {
            s.chip_errs += 1;
        }
        if h.code & 0xF0 == kp::RESET {
            // The chip restarted its end: answer, and open again from discovery.
            s.owe_reset_ack = true;
            if !matches!(s.phase, Phase::Reset | Phase::Dead) {
                say!(g, "gps: kepler: the chip reset its link; opening again\n");
            }
            s.phase = Phase::Discover;
            s.pend_n = 0;
            s.tries = 0;
            dg.restart();
            continue;
        }
        if h.code & 0xF0 == kp::RESET_ACK {
            if s.phase == Phase::Reset {
                s.answered = true;
                dg.restart();
            }
            continue;
        }
        // Any ack past the step's sequence acknowledges its packet (the chip's ack moves with every packet the AP sends, bare acks too: both captures).
        if s.pend_n > 0 && h.ack != s.pend_seq {
            s.acked = true;
        }
        if payload.is_empty() {
            continue;
        }
        s.owe_ack = true;
        let skips = dg.skips;
        let took = dg.take(&h, payload);
        // A sequence that skipped ahead: the ack goes as CHPP's out-of-order error, gnssd's ask for the packet again.
        if dg.skips != skips {
            s.nak = true;
        }
        if took != kp::Took::Whole {
            continue;
        }
        s.grams += 1;
        let b = dg.bytes();
        let Some(app) = kp::App::read(b) else { continue };
        match (app.handle, app.kind) {
            (kp::DISCOVERY, kp::SERVICE_RESPONSE) => {
                if s.phase == Phase::Discover {
                    s.answered = true;
                    say!(g, "gps: kepler: discovery answered: {}\n", if kp::discovered_betp(b) { "its one service is BETP (CHPP 1.0)" } else { "NOT the BETP service the captures name" });
                }
            }
            (kp::TIMESYNC, kp::SERVICE_RESPONSE) => {
                if s.phase == Phase::Time {
                    s.answered = true;
                    if b.len() >= kp::App::LEN + 8 {
                        s.chip_time_ns = u64::from_le_bytes(b[kp::App::LEN..kp::App::LEN + 8].try_into().unwrap());
                        s.chip_time_at = now();
                    }
                }
            }
            (kp::BETP, kp::SERVICE_RESPONSE) if app.cmd == kp::CMD_OPEN => {
                if s.phase == Phase::Open {
                    s.answered = true;
                    say!(g, "gps: kepler: the BETP service opened (error G#{:X})\n", app.err);
                }
            }
            (kp::BETP, kp::SERVICE_NOTIFICATION) if app.cmd == kp::CMD_CHIP_BETP => records(g, &b[kp::App::LEN..], s),
            _ => {}
        }
    }
}

/// One BETP notification's records: counted by kind, the fix and the satellites decoded.
fn records(g: u64, body: &[u8], s: &mut State) {
    let t = now();
    if s.phase == Phase::SessionOpen && s.acked {
        s.answered = true;
    }
    for r in kp::chip_records(body) {
        match s.kinds[..s.nkinds].iter().position(|k| k.word == r.word) {
            Some(i) => {
                let k = &mut s.kinds[i];
                k.count += 1;
                k.bytes += r.data.len() as u32;
                k.at = t;
            }
            None if s.nkinds < KINDS => {
                s.kinds[s.nkinds] = Kind { word: r.word, count: 1, bytes: r.data.len() as u32, at: t };
                s.nkinds += 1;
                say!(g, "gps: kepler: first G#{:04X}_{:04X} ({} B)\n", r.word >> 16, r.word & 0xFFFF, r.data.len());
            }
            None => s.kinds_over += 1,
        }
        match r.word & 0xFFFF {
            kp::PVT => {
                if let Some(p) = kp::Pvt::read(r.data).filter(|p| p.has_place()) {
                    s.fixes += 1;
                    s.fix_at = t;
                    // Speed in 1/64 m/s and course in 1/64 degree, payloads/gps's units; below walking pace (2^7 cm/s) the course is the chip's last, not where the device goes.
                    s.fix = Fix {
                        lat: kp::Pvt::to_units(p.lat_e7),
                        lon: kp::Pvt::to_units(p.lon_e7),
                        alt: p.alt_msl_cm / 100,
                        err: p.ehpe_cm / 100,
                        speed: (p.speed_cms as i32 * 64) / 100,
                        track: (p.course_cdeg as i32 * 64) / 100,
                        has_track: p.speed_cms >= 1 << 7,
                    };
                }
            }
            kp::SVS => {
                let n = kp::Sv::count(r.data).min(s.svs.len());
                let mut k = 0;
                for i in 0..n {
                    if let Some(v) = kp::Sv::read(r.data, i) {
                        s.svs[k] = v;
                        k += 1;
                    }
                }
                s.nsv = k;
                s.sv_msgs += 1;
                s.sv_at = t;
            }
            _ => {}
        }
    }
}

/// The link's sends and reads, with the chip's bytes from each fed and drained.
struct Io {
    chip: Chip,
    sc: Scan,
    dg: Gram,
    /// Read chunks by what they held (`read`), and the latest that held bytes.
    zeros: u32,
    ones: u32,
    other: u32,
    seen: [u8; 16],
}

impl Io {
    /// One packet out: AP to GNSS up, the chip's ready, the words, a read of as many, AP to GNSS down. False without the ready answer.
    fn send(&mut self, g: u64, pkt: &[u8], s: &mut State) -> bool {
        if s.trace && s.packets <= 1 << 6 && pkt.len() >= 10 {
            say!(g, "gps: kepler: out seq G#{:02X} ack G#{:02X} code G#{:02X} len A#{}\n", pkt[5], pkt[4], pkt[3], u16::from_le_bytes([pkt[6], pkt[7]]));
        }
        let f = freq();
        // A gap after the last send (2^-8 s, some 4 ms): a payload sent within a millisecond of a bare ack was dropped by the chip and taken only at its resend a second later, every step of the open (the 8a, 2026-11-19); gnssd's sends came that far apart.
        while now().wrapping_sub(self.chip.sent_at) < f >> 8 {}
        let t0 = now();
        // The line from the last exchange let fall first (at most 2^-9 s, some 2 ms), so the ready below is the chip's answer to this raise and not a level left over: a payload sent right after a bare ack while the line still stood went unanswered until its resend a second later (the 8a, 2026-11-19, every step).
        while self.chip.gnss2ap() && now().wrapping_sub(t0) < f >> 9 {}
        if self.chip.gnss2ap() {
            self.chip.busy += 1;
        }
        self.chip.ap2gnss(true);
        let t0 = now();
        // WHY the spin: the chip's ready answer to the raised line, inside one send's handshake; Linux waits for it as an interrupt for at most 100 ms (gnss_prj.h GNSS_RDY_TIMEOUT), the limit here.
        while !self.chip.gnss2ap() {
            if now().wrapping_sub(t0) > ms(f, 100) {
                self.chip.ap2gnss(false);
                self.chip.no_ready += 1;
                return false;
            }
        }
        let mut rx = [0u8; WORDS * 4];
        for c in pkt.chunks(WORDS * 4) {
            let n = c.len();
            self.chip.words(Some(c), &mut rx[..n]);
        }
        for c in pkt.chunks(WORDS * 4) {
            let n = c.len();
            self.chip.words(None, &mut rx[..n]);
            self.sc.feed(&rx[..n]);
            drain(g, &mut self.sc, &mut self.dg, s);
        }
        self.chip.ap2gnss(false);
        self.chip.sent_at = now();
        true
    }

    /// While the chip holds GNSS to AP up: the 100 µs pulse, then 1024 bytes in 16 transfers.
    fn read(&mut self, g: u64, s: &mut State, mut once: bool) -> usize {
        let mut got = 0;
        let f = freq();
        // `once`: the line fired, so one read whatever the line says now, as gnss_link_device.c's rx_work reads before it looks (a do-while): the chip may drop the line before the AP looks.
        // At most `READS` reads a pass: a line held up with nothing coming must not hold the loop from its endpoint and its report (the level interrupt brings it straight back).
        let mut left = READS;
        while (core::mem::take(&mut once) || self.chip.gnss2ap()) && left > 0 {
            left -= 1;
            self.chip.ap2gnss(true);
            let t = now();
            // The pulse's length is gnss_link_device.c's `udelay(100)`: the chip's cue that the AP is about to read.
            while now().wrapping_sub(t) < f / 10_000 {}
            self.chip.ap2gnss(false);
            let mut rx = [0u8; WORDS * 4];
            for _ in 0..READ / (WORDS * 4) {
                if self.chip.words(None, &mut rx) {
                    // What a read brings, by kind: all zeros (the fill), all ones (nothing driven), else bytes (the first 16 of the latest kept).
                    match (rx.iter().all(|&b| b == 0), rx.iter().all(|&b| b == 0xFF)) {
                        (true, _) => self.zeros += 1,
                        (_, true) => self.ones += 1,
                        _ => {
                            self.other += 1;
                            self.seen.copy_from_slice(&rx[..16]);
                        }
                    }
                    self.sc.feed(&rx);
                    got += rx.len();
                    drain(g, &mut self.sc, &mut self.dg, s);
                }
            }
        }
        got
    }

    /// Reads while the chip has bytes, and the acks owed, until neither is left.
    fn service(&mut self, g: u64, s: &mut State, fired: bool) -> usize {
        let mut got = 0;
        let mut once = fired;
        loop {
            got += self.read(g, s, core::mem::take(&mut once));
            let mut out = [0u8; kp::PACKET_MAX];
            if s.owe_reset_ack {
                s.owe_reset_ack = false;
                let n = kp::frame(0, kp::RESET_ACK, s.link.next_from_chip, 0, &kp::LINK_CONFIG, &mut out);
                self.send(g, &out[..n], s);
                continue;
            }
            if s.owe_ack {
                s.owe_ack = false;
                let n = if s.nak { s.link.nak(&mut out) } else { s.link.ack(&mut out) };
                s.nak = false;
                self.send(g, &out[..n], s);
                continue;
            }
            return got;
        }
    }
}

/// Reads one service pass takes at most (`Io::read`).
const READS: u32 = 1 << 4;

/// The modem driver's `rmnet` (grant slot 10): its GNSS ops, G#10 a command on gnss_boot, G#11 bytes into GNSS_FW.
const S_RMNET: u64 = 10;
/// The images in the vault (`gps.` scope; Samsung's, the 8a's own, never in a repo): kepler.bin whole, and the 912-byte chip configuration gnssd builds from it and gps.cfg (both captured by tools/akita-gnss-trace.sh).
const FW_KEY: &[u8] = b"kepler";
const CFG_KEY: &[u8] = b"kepler.cfg";
/// Bytes a GNSS_FW write carries: the modem's request buffer (4096) less the op and the offset, whole words.
const FW_CHUNK: usize = 4088;

/// One gnss_boot command thru the modem: the CP's answer (its first byte the command's low byte, its third the status), None when it gave none in its 2 s or the modem is not up.
fn core_cmd(g: u64, cmd: &[u8]) -> Option<([u8; 16], usize)> {
    let mut q = [0u8; 16];
    q[0] = 0x10;
    q[1..1 + cmd.len()].copy_from_slice(cmd);
    let mut r = [0u8; 16];
    let n = ferros_sys::call(cap(g, S_RMNET), &q[..1 + cmd.len()], &mut r);
    if n == ferros_sys::ERR || n == 0 {
        return None;
    }
    // WHY the min: the CP's answer is 3 or 7 bytes; the call says the whole length and copies at most the buffer.
    Some((r, (n as usize).min(r.len())))
}

/// An image from the vault into GNSS_FW from offset 0, then the CP told (G#0E05: its type, offset 0, its size) and its answer checked (G#05, status 0). The type: 0 the firmware, 4 the chip configuration.
fn core_load(g: u64, key: &[u8], kind: u32) -> Result<u32, &'static str> {
    let store = cap(g, S_STORE);
    let mut head = [0u8; 8];
    let total = ferros_sys::store_get_at(store, key, 0, &mut head).ok_or("not in the vault")?;
    let mut q = [0u8; 5 + FW_CHUNK];
    q[0] = 0x11;
    let mut at = 0usize;
    while at < total {
        let k = (total - at).min(FW_CHUNK);
        ferros_sys::store_get_at(store, key, at, &mut q[5..5 + k]).ok_or("the vault refused a piece")?;
        q[1..5].copy_from_slice(&(at as u32).to_le_bytes());
        let mut r = [0u8; 4];
        let n = ferros_sys::call(cap(g, S_RMNET), &q[..5 + k], &mut r);
        if n != 1 || r[0] != 0 {
            return Err(if r[0] == 1 { "the modem driver holds no GNSS_FW window (modem-gnss at slot 26)" } else { "a GNSS_FW write was refused" });
        }
        at += k;
    }
    let mut c = [0u8; 14];
    c[0..2].copy_from_slice(&0x0E05u16.to_le_bytes());
    c[2..6].copy_from_slice(&kind.to_le_bytes());
    c[10..14].copy_from_slice(&(total as u32).to_le_bytes());
    match core_cmd(g, &c) {
        Some((r, n)) if n >= 3 && r[0] == 0x05 && r[2] == 0 => Ok(total as u32),
        Some(_) => Err("the CP refused the image"),
        None => Err("no answer to the image's load"),
    }
}

/// The core booted as gnssd boots it (tools/akita-gnss-trace.sh, 2026-11-19): the boot count asked, a BLC G#0102, the reset held and released, the count again, kepler.bin loaded (type 0), the chip configuration (type 4), then the BLC that starts the firmware (G#20, G#2, the entry G#6000_0001). Its answers said on the console; Err stops at the first step that fails.
fn core_boot(g: u64) -> Result<(), &'static str> {
    let count = |r: &[u8; 16], n: usize| if n >= 7 { u32::from_le_bytes([r[3], r[4], r[5], r[6]]) } else { 0 };
    let (r, n) = core_cmd(g, &[0x01, 0x02]).ok_or("no answer to the boot count: the modem not up, or no gnss_boot")?;
    say!(g, "gps: kepler: the CP's GNSS boot count A#{}\n", count(&r, n));
    let (r, n) = core_cmd(g, &[0x09, 0x0E, 0, 0, 0x02, 0x01, 0, 0, 0, 0, 0, 0, 0, 0]).ok_or("no answer to the first BLC")?;
    say!(g, "gps: kepler: BLC G#0102 answered ({} bytes, status G#{:02X})\n", n, r[2]);
    let (r, n) = core_cmd(g, &[0x03, 0x02]).ok_or("no answer to the reset hold")?;
    if n < 3 || r[0] != 3 || r[2] != 0 {
        return Err("the CP refused the reset hold");
    }
    let (r, n) = core_cmd(g, &[0x04, 0x02]).ok_or("no answer to the reset release")?;
    if n < 3 || r[0] != 4 || r[2] != 0 {
        return Err("the CP refused the reset release");
    }
    let (r, n) = core_cmd(g, &[0x01, 0x02]).ok_or("no answer to the second boot count")?;
    let fw = core_load(g, FW_KEY, 0)?;
    let cfg = core_load(g, CFG_KEY, 4)?;
    let (r2, n2) = core_cmd(g, &[0x09, 0x0E, 0x20, 0, 0x02, 0, 0x01, 0, 0, 0x60, 0, 0, 0, 0]).ok_or("no answer to the start BLC")?;
    say!(g, "gps: kepler: reset held and released (boot count now A#{}), kepler.bin (A#{} B) and its configuration (A#{} B) loaded, the start BLC answered status G#{:02X} ({} bytes)\n", count(&r, n), fw, cfg, r2[2], n2);
    if n2 < 3 || r2[0] != 9 || r2[2] != 0 {
        return Err("the CP refused the start BLC");
    }
    Ok(())
}

/// The step's packet built and sent: a transport reset, or a payload with the link's sequence now.
fn step_send(g: u64, io: &mut Io, s: &mut State) {
    let mut payload = [0u8; 512];
    let n = match s.phase {
        Phase::Discover => copy(&mut payload, &kp::DISCOVER),
        Phase::Time => copy(&mut payload, &kp::GET_TIME),
        Phase::Open => copy(&mut payload, &kp::OPEN),
        Phase::Start => kp::ap_datagram(&[kp::AP_START], &mut payload),
        Phase::Fresh0 => kp::ap_datagram(&[kp::ApRecord { word: kp::AP_FRESH_00E4_WORD, data: &kp::AP_FRESH_00E4 }], &mut payload),
        Phase::Fresh1 => kp::ap_datagram(&[kp::ApRecord { word: kp::AP_FRESH_08AD_WORD, data: &kp::AP_FRESH_08AD }], &mut payload),
        Phase::Script => match script_record(s) {
            // WHY the bound: the record's length is the vault's; one past the payload buffer is not sent.
            Some((word, at, len)) if len + 46 <= payload.len() => {
                let mut data: alloc::vec::Vec<u8> = s.script[at..at + len].to_vec();
                // The aids' stamp (G#02D7, G#0D46, G#01D7: their first u64) is the chip's own clock in µs at the moment the aid holds for, as the timesync gave it (the trace: 0.91 s after the open for the first G#0D46, 16.4 s as Maps opened), so the chip's clock now, not the trace's.
                if matches!(word & 0xFFFF, 0x02D7 | 0x0D46 | 0x01D7) && len >= 8 && s.chip_time_at != 0 {
                    let f = freq();
                    let d = now().wrapping_sub(s.chip_time_at);
                    let us = s.chip_time_ns / 1000 + d / f * 1_000_000 + d % f * 1_000_000 / f;
                    data[0..8].copy_from_slice(&us.to_le_bytes());
                }
                // The time aid (G#02D7: the chip's stamp u64, the GPS week u16, the time of week in µs as a u64 high word first, its uncertainty in µs u32, a flag) given the clock's now, not the trace's.
                if word & 0xFFFF == 0x02D7 && len >= 18 {
                    if let Some(ms) = gps_ms_now() {
                        let week = (ms / 604_800_000) as u16;
                        let tow_us = (ms % 604_800_000) * 1000;
                        data[8..10].copy_from_slice(&week.to_le_bytes());
                        data[10..14].copy_from_slice(&((tow_us >> 32) as u32).to_le_bytes());
                        data[14..18].copy_from_slice(&(tow_us as u32).to_le_bytes());
                    }
                }
                if s.tries == 0 {
                    s.script_sent += 1;
                }
                kp::ap_datagram(&[kp::ApRecord { word, data: &data }], &mut payload)
            }
            _ => return,
        },
        Phase::SessionOpen => kp::ap_datagram(&[kp::AP_SESSION_OPEN], &mut payload),
        Phase::SessionAsk => kp::ap_datagram(&[kp::AP_SESSION_ASK], &mut payload),
        Phase::Reset => 0,
        _ => return,
    };
    let mut out = [0u8; 512];
    let m = if s.phase == Phase::Reset { kp::Link::reset(&mut out) } else { s.link.packet(&payload[..n], &mut out) };
    s.pend[..m].copy_from_slice(&out[..m]);
    s.pend_n = m;
    s.pend_seq = s.link.chip_ack;
    s.pend_at = now();
    s.tries += 1;
    s.acked = false;
    if !io.send(g, &out[..m], s) && s.phase == Phase::Reset && s.tries == 1 {
        say!(g, "gps: kepler: no ready answer to the raised line in 100 ms: no firmware running on the core, or the core unpowered (the CP loads it: GPS.md \"akita's KEPLER\")\n");
    }
}

fn copy(out: &mut [u8], b: &[u8]) -> usize {
    out[..b.len()].copy_from_slice(b);
    b.len()
}

/// The next phase once this one's packet is acked and, where it asks, answered.
fn advance(g: u64, s: &mut State, link_only: bool) -> bool {
    // Maps' G#12DE is answered by the chip's records, and gnssd sent G#04D4 only after them (the fix capture): the session's second record waits for the first's answer.
    let need_answer = matches!(s.phase, Phase::Reset | Phase::Discover | Phase::Time | Phase::Open | Phase::SessionOpen);
    if !(s.answered || (!need_answer && s.acked)) {
        return false;
    }
    let next = match s.phase {
        Phase::Reset => Phase::Discover,
        Phase::Discover => Phase::Time,
        Phase::Time => {
            say!(g, "gps: kepler: the chip's clock reads A#{} ms\n", s.chip_time_ns / 1_000_000);
            Phase::Open
        }
        Phase::Open if link_only => {
            say!(g, "gps: kepler: the link is open (input bit 4: no session started)\n");
            Phase::Idle
        }
        Phase::Open if !s.script.is_empty() => Phase::Script,
        Phase::Script => {
            s.script_at = script_next(s);
            if script_record(s).is_some() {
                Phase::Script
            } else {
                say!(g, "gps: kepler: gnssd's session played ({} records); now the chip's\n", s.script_sent);
                Phase::Running
            }
        }
        Phase::Open if s.fresh => Phase::Fresh0,
        Phase::Open => Phase::Start,
        Phase::Fresh0 => Phase::Fresh1,
        Phase::Fresh1 => Phase::SessionOpen,
        Phase::Start => Phase::SessionOpen,
        Phase::SessionOpen => Phase::SessionAsk,
        Phase::SessionAsk => {
            say!(g, "gps: kepler: the session asked for (G#12DE, G#04D4 G#0F, as gnssd for Maps)\n");
            Phase::Running
        }
        p => p,
    };
    s.phase = next;
    s.tries = 0;
    s.answered = false;
    s.acked = false;
    s.pend_n = 0;
    true
}

/// The heap the documents are built and read in (as payloads/gps).
/// The heap's ceiling in the image header (docs/NO-CEILINGS.md steps 1 and 2): the interim ceiling every growing payload declares until step 3 (ferros_heap::INTERIM_CEILING), addresses only.
const HEAP_CEILING: usize = ferros_heap::INTERIM_CEILING;
/// The heap the kernel maps at spawn, as a log2 (the header's start-size word, process.rs `image_start`): 1 << 19, what the static array held until step 1, so the arc starts as it did and grows from there as the heap runs out. A kernel from before step 2 maps its 4 MiB instead.
const HEAP_START_LOG2: usize = 19;
unsafe extern "C" {
    /// The end of .bss (payload.ld): the heap starts there, in the zeroed tail the kernel maps after it.
    static __bss_end: u8;
}

#[global_allocator]
static ALLOC: ferros_heap::ArcHeap<ferros_heap::Kernel> = ferros_heap::ArcHeap::new(ferros_heap::Kernel);
const _: () = assert!(ferros_sys::EAGLE_OSCILLATIONS_PER_SECOND == ferros_kept::vsf::OSCILLATIONS_PER_SECOND);

/// The last fix kept (`gps.last`): latitude and longitude in 2^-23 degrees, and when it came.
fn read_last(store: u64) -> Option<(i32, i32, Option<i64>)> {
    let mut b = [0u8; 1 << 8];
    match ferros_sys::store_read_at(store, b"last", 0, &mut b) {
        NOTHING | ferros_sys::ERR => None,
        // WHY the min: the vault says the whole length and copies at most the buffer.
        n => ferros_kept::gps::parse(&b[..(n as usize).min(b.len())]),
    }
}

const UNIT_RAD: f64 = core::f64::consts::PI / 180. / (1u64 << 23) as f64;
/// The zone's principal place as a circle this wide (payloads/gps `ZONE_UNC_M`).
const ZONE_UNC_M: u32 = 1 << 21;

/// The rough place for the predicted birds (payloads/gps `rough_place`): the last fix, its circle by its age, and the zone's principal place.
fn rough_place(last: Option<(i32, i32, Option<i64>)>, zone: Option<(i32, i32)>) -> Option<ferros_sky::Place> {
    use ferros_gnss::place::{combine, last_fix, Circle};
    let mut c = [Circle { lat: 0., lon: 0., radius: 0. }; 2];
    let mut n = 0;
    if let Some((lat, lon, when)) = last {
        let age = when.filter(|_| ferros_sys::utc_time().is_some()).map(|w| (ferros_sys::eagle_now() - w) as f64 / ferros_sys::EAGLE_OSCILLATIONS_PER_SECOND as f64);
        c[n] = last_fix(lat as f64 * UNIT_RAD, lon as f64 * UNIT_RAD, None, age);
        n += 1;
    }
    if let Some((lat, lon)) = zone {
        c[n] = Circle { lat: lat as f64 * UNIT_RAD, lon: lon as f64 * UNIT_RAD, radius: ZONE_UNC_M as f64 };
        n += 1;
    }
    combine(&c[..n]).map(|v| ferros_sky::Place { lat: v.circle.lat, lon: v.circle.lon })
}

/// The fix kept as it changes (every change hits disk), as payloads/gps keeps it.
fn keep_last(g: u64, s: &State) {
    let when = ferros_sys::utc_time().map(|_| ferros_sys::eagle_now() - (now().wrapping_sub(s.fix_at) as i128 * ferros_sys::EAGLE_OSCILLATIONS_PER_SECOND as i128 / freq() as i128) as i64);
    let Some(doc) = ferros_kept::gps::build(s.fix.lat, s.fix.lon, when, ferros_sys::eagle_now()) else {
        console(g, b"gps: the last fix was not kept: its document did not build\n");
        return;
    };
    if !store_set(cap(g, S_STORE), b"last", &doc) {
        console(g, b"gps: the last fix was not kept: the vault refused gps.last\n");
    }
}

/// The tracks: every satellite of the latest list, then every predicted bird no heard one stands for (payloads/gps `publish_tracks`).
fn publish_tracks(s: &State, pr: &Predict, filled: &mut usize) {
    use ferros_sys::{sat_word, status_track, Sat, MEASURE_NONE, SAT_NO_RATE, TRACKS_MAX};
    let n = s.nsv.min(TRACKS_MAX);
    let mut heard = [(0u8, 0u8, 0i8, 0u16); TRACKS_MAX];
    for (t, v) in s.svs[..n].iter().enumerate() {
        // Whole degrees, as the tracks hold them: the elevation rounded (a satellite above the horizon), the azimuth wrapped at north.
        let sat = Sat {
            system: v.ferros_system(),
            band: v.band(),
            number: v.number,
            used: v.flags & kp::SV_USED != 0,
            elevation: (v.elevation + 0.5) as i8,
            azimuth: ((v.azimuth + 0.5) as u16) % 360,
            cn0: v.cn0 as u8,
            el_rate: SAT_NO_RATE,
            az_rate: SAT_NO_RATE,
        };
        heard[t] = (sat.system, sat.number, sat.elevation, sat.azimuth);
        status_track(t, sat_word(&sat));
    }
    let mut t = n;
    for b in &pr.birds[..pr.n] {
        if t == TRACKS_MAX {
            break;
        }
        if Predict::is_heard(b, &heard[..n]) {
            continue;
        }
        let sat = Sat { system: b.system, band: 0, number: b.number, used: false, elevation: (b.el + 0.5) as i8, azimuth: ((b.az + 0.5) as u16) % 360, cn0: 0, el_rate: SAT_NO_RATE, az_rate: SAT_NO_RATE };
        status_track(t, sat_word(&sat));
        t += 1;
    }
    for k in t..*filled {
        status_track(k, MEASURE_NONE);
    }
    *filled = t;
}

/// The WATCH count: fixes, satellite lists, the chip's packets in steps of 64, each predicted sky.
fn changes(s: &State, pr: &Predict) -> u32 {
    s.fixes.wrapping_add(s.sv_msgs).wrapping_add(s.packets >> 6).wrapping_add(pr.generation)
}

/// (satellites seen, used, best C/N0 in quarter dB-Hz) of the latest list.
fn counts(s: &State) -> (u32, u32, u32) {
    let v = &s.svs[..s.nsv];
    let used = v.iter().filter(|x| x.flags & kp::SV_USED != 0).count() as u32;
    let best = v.iter().map(|x| (x.cn0 * 4.) as u32).max().unwrap_or(0);
    (v.len() as u32, used, best)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw const __bss_end) as usize, input as usize, input_len as usize) };
    ferros_sys::panic_grants(grants);
    let g = grants;
    let input0 = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    let link_only = input0 & 0x10 != 0;
    let f = freq();
    let store = cap(g, S_STORE);

    if !ferros_sys::power(cap(g, S_POWER), true) {
        say!(g, "gps: kepler: the power switch refused\n");
    }
    let mut io = Io { chip: Chip { spi: cap(g, S_SPI), far: cap(g, S_FAR), peric0: cap(g, S_PERIC0), lvl_bytes: 0, lvl_words: 0, stuck: 0, no_ready: 0, busy: 0, sent_at: 0 }, sc: Scan::new(), dg: Gram::new(), zeros: 0, ones: 0, other: 0, seen: [0; 16] };
    // GNSS to AP as a level-high external interrupt: function EINT (G#F), pull-down (the DT's), control nibble 1, pending cleared, unmasked.
    let far = cap(g, S_FAR);
    mmio_write(far, 0x00, (mmio_read(far, 0x00) as u32 & !(0xF << 12)) | 0xF << 12);
    mmio_write(far, 0x08, (mmio_read(far, 0x08) as u32 & !(0xF << 12)) | 1 << 12);
    mmio_write(far, EINT_CON, (mmio_read(far, EINT_CON) as u32 & !(0xF << 12)) | 1 << 12);
    mmio_write(far, EINT_PEND, PIN);
    mmio_write(far, EINT_MASK, mmio_read(far, EINT_MASK) as u32 & !PIN);
    say!(g, "gps: kepler: powered (the port at 3.07 MHz, 32-bit words); GNSS to AP reads {}{}\n", if io.chip.gnss2ap() { "high (the chip has bytes)" } else { "low" }, if link_only { "; the link alone (input bit 4)" } else { "" });
    // Input bit 5: one 16-word read with no handshake first, the port alone (a chip that is not ready leaves its output idle): whether the transfer runs and how the receive level counts, then on as asked.
    if input0 & 0x20 != 0 {
        let mut rx = [0u8; WORDS * 4];
        let t = now();
        let ok = io.chip.words(None, &mut rx);
        say!(g, "gps: kepler: a bare 16-word read {} in {} us (level in bytes {}, in words {}); first word G#{:08X}\n", if ok { "ran" } else { "never filled" }, now().wrapping_sub(t) * 1_000_000 / f, io.chip.lvl_bytes, io.chip.lvl_words, u32::from_le_bytes([rx[0], rx[1], rx[2], rx[3]]));
    }

    let mut s = State {
        link: kp::Link::default(),
        phase: Phase::Reset,
        owe_ack: false,
        owe_reset_ack: false,
        nak: false,
        pend: [0; 512],
        pend_n: 0,
        pend_seq: 0,
        pend_at: 0,
        tries: 0,
        acked: false,
        answered: false,
        retry_at: 0,
        packets: 0,
        grams: 0,
        chip_errs: 0,
        chip_time_ns: 0,
        chip_time_at: 0,
        fixes: 0,
        fix: Fix::default(),
        fix_at: 0,
        first_fix_said: false,
        svs: [kp::Sv::default(); ferros_sys::TRACKS_MAX],
        nsv: 0,
        sv_msgs: 0,
        sv_at: 0,
        kinds: [Kind::default(); KINDS],
        nkinds: 0,
        kinds_over: 0,
        fresh: false,
        trace: input0 & 0x80 != 0,
        script: alloc::vec::Vec::new(),
        script_at: 0,
        script_end: 0,
        script_sent: 0,
    };

    // gnssd's traced session, when the vault holds one (input bit 2 leaves it out): played after the open in place of Maps' two records.
    let mut head = [0u8; 8];
    if input0 & 0x04 == 0 {
        if let Some(total) = ferros_sys::store_get_at(store, b"kepler.session", 0, &mut head) {
            let mut b = alloc::vec![0u8; total];
            let mut at = 0;
            while at < total {
                // WHY the min: a transfer carries at most STORE_MAX.
                let k = (total - at).min(ferros_sys::STORE_MAX);
                if ferros_sys::store_get_at(store, b"kepler.session", at, &mut b[at..at + k]).is_none() {
                    break;
                }
                at += k;
            }
            s.script_end = script_end(&b);
            s.script = b;
            say!(g, "gps: kepler: gnssd's session in the vault: A#{} bytes, played to A#{}\n", total, s.script_end);
        }
    }
    // The predicted sky from the kept orbits and the rough place, before the chip says anything (payloads/gps).
    let last = read_last(store);
    let zone = ferros_sys::zone::place().map(|(la, lo)| ((la as i64 * (1 << 23) / 3600) as i32, (lo as i64 * (1 << 23) / 3600) as i32));
    let rough = rough_place(last, zone);
    let mut pr = Predict::new();
    let had_orbits = pr.load(store);
    let mut tracks_filled = 0usize;
    if let Some(p) = rough {
        pr.sky(p, now());
        publish_tracks(&s, &pr, &mut tracks_filled);
    }
    say!(g, "gps: kepler: orbits {}; {} birds predicted above the horizon (the place from {})\n", if had_orbits { "kept" } else { "none kept yet" }, pr.n, match (last.is_some(), zone.is_some()) { (true, true) => "the last fix and the zone", (true, false) => "the last fix", (false, true) => "the zone in effect", _ => "nowhere: no birds" });
    let mut fetched = false;

    let t_all = now();
    let mut msg = [0u8; 16];
    let mut token = [0u8; 32];
    let mut watches: [Option<([u8; 32], u32)>; 1 << 2] = [None; 1 << 2];
    let mut polled = false;
    let mut said = now();
    let mut published: Option<(u8, u32)> = None;
    let mut published_sky = (u32::MAX, u32::MAX);
    let mut published_fix = u32::MAX;
    let mut kept = (0i32, 0i32);
    let (mut bytes, mut irqs) = (0usize, 0u32);
    // Whether the core is to be booted thru the modem: `rmnet` granted (the grants page's count at G#200 past slot 10) and input bit 6 clear.
    let granted = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + g + 0x200) as *const u32) as u64 };
    let boot_wanted = granted > S_RMNET && input0 & 0x40 == 0;
    let mut booted = false;
    let mut fired = false;
    loop {
        ferros_sys::pet();
        // The step's packet: sent on entering a phase, again after `RESEND_MS` unacked or unanswered, and after `TRIES` the link reset and tried again every `RETRY_S`.
        let t = now();
        // The core booted thru the modem first (the CP holds its power and its firmware), when the modem's `rmnet` was granted and input bit 6 does not say the core already runs; a boot that fails is tried again with the link, every `RETRY_S`.
        if s.phase == Phase::Reset && s.tries == 0 && !booted && boot_wanted {
            match core_boot(g) {
                Ok(()) => {
                    booted = true;
                    // Input bit 3: gnssd's fresh-boot records (G#00E4, G#08AD, as under gtrace with no HAL) in place of its start record G#7304 (as at an Android boot, the 2026-11-18 boot capture).
                    s.fresh = input0 & 0x08 != 0;
                    say!(g, "gps: kepler: the core started by the CP after A#{} ms; the link next\n", now().wrapping_sub(t_all) * 1000 / f);
                }
                Err(why) => {
                    say!(g, "gps: kepler: the core's boot stopped: {}; again in {} s\n", why, RETRY_S);
                    s.phase = Phase::Dead;
                    s.retry_at = t + RETRY_S * f;
                }
            }
        }
        match s.phase {
            Phase::Running | Phase::Idle => {}
            Phase::Dead => {
                if t >= s.retry_at {
                    s.phase = Phase::Reset;
                    s.tries = 0;
                    s.link = kp::Link::default();
                }
            }
            _ => {
                if s.tries == 0 || (s.pend_n > 0 || s.tries > 0) && t.wrapping_sub(s.pend_at) > ms(f, RESEND_MS) {
                    if s.tries >= TRIES {
                        say!(g, "gps: kepler: no answer to the {:?} step after {} sends; the link reset in {} s (ready never came {} times)\n", s.phase, TRIES, RETRY_S, io.chip.no_ready);
                        s.phase = Phase::Dead;
                        s.retry_at = t + RETRY_S * f;
                        s.pend_n = 0;
                    } else {
                        step_send(g, &mut io, &mut s);
                    }
                }
            }
        }
        // Input bit 1: a read on every interrupt whatever the line says (gnss_link_device.c's do-while). Off by default: the line's interrupt fires some 250 times a second with the line low (the 8a, 2026-11-19, as on the Pro: GPS.md "No fix, 2026-11-18"), and a read each time pulsed AP to GNSS at the chip as often.
        bytes += io.service(g, &mut s, core::mem::take(&mut fired) && input0 & 0x02 != 0);
        while advance(g, &mut s, link_only) {
            if !matches!(s.phase, Phase::Running | Phase::Idle | Phase::Dead) {
                step_send(g, &mut io, &mut s);
                bytes += io.service(g, &mut s, false);
            }
        }
        // Fresh orbits once the link runs (payloads/gps: the fetch's second or two holds nothing up).
        if !fetched && s.phase == Phase::Running && pr.stale() && Predict::can_fetch(g) {
            fetched = true;
            match pr.fetch(g, store) {
                Ok((taken, refused)) => say!(g, "gps: orbits fetched from CelesTrak: {} satellites ({} sets refused), kept as gps.orbits\n", taken, refused),
                Err(why) => say!(g, "gps: orbits not fetched: {}\n", why),
            }
            if let Some(p) = rough {
                pr.sky(p, now());
            }
        }
        if pr.n > 0 && now().wrapping_sub(pr.at) >= predict::EVERY_S * f {
            let p = if s.fixes > 0 { Some(ferros_sky::Place { lat: s.fix.lat as f64 * UNIT_RAD, lon: s.fix.lon as f64 * UNIT_RAD }) } else { rough };
            if let Some(p) = p {
                pr.sky(p, now());
            }
        }

        if now().wrapping_sub(said) > 32 * f {
            let (seen, used, cn0) = counts(&s);
            say!(g, "gps: kepler, the last 32 s: {:?}, {} interrupts, {} bytes read; {} packets ({} CRC failures, {} resends, {} skips, {} errors from the chip), {} datagrams; {} fixes; {} satellites, {} used, best {} dB-Hz; transfers: {} level in bytes, {} in words, {} stuck\n", s.phase, irqs, bytes, s.packets, io.sc.bad, io.dg.resends, io.dg.skips, s.chip_errs, s.grams, s.fixes, seen, used, cn0 >> 2, io.chip.lvl_bytes, io.chip.lvl_words, io.chip.stuck);
            say!(g, "gps: kepler: read chunks so far: {} zeros, {} ones, {} with bytes (the latest begins {:02X?})\n", io.zeros, io.ones, io.other, &io.seen[..]);
            (bytes, irqs, said) = (0, 0, now());
        }
        if s.fixes > 0 && !s.first_fix_said {
            s.first_fix_said = true;
            // Coarse on the console on purpose (payloads/gps): one decimal of a degree, never the fix itself.
            say!(g, "gps: first fix {} ms after power-up, ±{} m (lat {}.{} lon {}.{})\n", s.fix_at.wrapping_sub(t_all) * 1000 / f, s.fix.err, s.fix.lat >> 23, ((s.fix.lat.unsigned_abs() & 0x7F_FFFF) * 10) >> 23, s.fix.lon >> 23, ((s.fix.lon.unsigned_abs() & 0x7F_FFFF) * 10) >> 23);
        }
        let now_shown = if s.fixes > 0 { (2, s.fix.err) } else { (1, 0) };
        if published != Some(now_shown) {
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            status(0, w(k::STATE, now_shown.0 as i64));
            status(1, if s.fixes > 0 { w(k::ACCURACY, now_shown.1 as i64 * 1000) } else { MEASURE_NONE });
            published = Some(now_shown);
        }
        if s.fixes != published_fix {
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            published_fix = s.fixes;
            if s.fixes > 0 && (s.fix.lat, s.fix.lon) != kept {
                keep_last(g, &s);
                kept = (s.fix.lat, s.fix.lon);
                ferros_sys::zone::position(s.fix.lat, s.fix.lon);
            }
            let fx = s.fix;
            status(5, if s.fixes > 0 { w(k::ALTITUDE, fx.alt as i64 * 1000) } else { MEASURE_NONE });
            status(6, if s.fixes > 0 { w(k::SPEED, fx.speed as i64 * 1000 >> 6) } else { MEASURE_NONE });
            status(7, if s.fixes > 0 && fx.has_track { w(k::HEADING, fx.track as i64 * 1000 >> 6) } else { MEASURE_NONE });
        }
        if published_sky != (s.sv_msgs, pr.generation) {
            use ferros_sys::{measure as k, measure_word as w, status, MEASURE_NONE};
            let (seen, used, cn0) = counts(&s);
            let none = s.sv_msgs == 0;
            status(2, if none { MEASURE_NONE } else { w(k::SATS_SEEN, seen as i64) });
            status(3, if none { MEASURE_NONE } else { w(k::SATS_USED, used as i64) });
            status(4, if none || seen == 0 { MEASURE_NONE } else { w(k::CN0, cn0 as i64 * 250) });
            publish_tracks(&s, &pr, &mut tracks_filled);
            published_sky = (s.sv_msgs, pr.generation);
        }

        let n = recv(cap(g, S_GPS), &mut msg, &mut token, true);
        if n != NOTHING && n != ferros_sys::ERR && n > 0 {
            match msg[0] {
                0 => {
                    let mut r = [0u8; 38];
                    r[0] = (s.fixes > 0) as u8;
                    r[1..5].copy_from_slice(&s.fixes.to_le_bytes());
                    r[5..9].copy_from_slice(&((now().wrapping_sub(s.fix_at) * 1000 / f) as u32).to_le_bytes());
                    for (k, v) in [s.fix.lat, s.fix.lon, s.fix.alt, s.fix.err as i32, s.fix.speed, s.fix.track].iter().enumerate() {
                        r[9 + 4 * k..13 + 4 * k].copy_from_slice(&v.to_le_bytes());
                    }
                    r[33..37].copy_from_slice(&s.packets.to_le_bytes());
                    r[37] = s.nkinds as u8;
                    reply(&token, &r);
                }
                1 => {
                    // As Maps' close: G#12DE once more, then gone (the kernel turns the switch off as this ends).
                    if s.phase == Phase::Running {
                        let mut p = [0u8; 64];
                        let m = kp::ap_datagram(&[kp::AP_SESSION_OPEN], &mut p);
                        let mut out = [0u8; 128];
                        let k = s.link.packet(&p[..m], &mut out);
                        io.send(g, &out[..k], &mut s);
                    }
                    reply(&token, &[1]);
                    say!(g, "gps: stopped after {} fixes\n", s.fixes);
                    ferros_sys::exit(0);
                }
                2 => {
                    const REC: usize = 25;
                    let mut r = [0u8; 9 + ferros_sys::TRACKS_MAX * REC];
                    r[0] = s.nsv as u8;
                    r[1..5].copy_from_slice(&s.sv_msgs.to_le_bytes());
                    r[5..9].copy_from_slice(&((now().wrapping_sub(s.sv_at) * 1000 / f) as u32).to_le_bytes());
                    for (k, v) in s.svs[..s.nsv].iter().enumerate() {
                        let o = 9 + REC * k;
                        r[o] = v.ferros_system();
                        r[o + 1] = v.band() << 1;
                        r[o + 2] = v.number;
                        r[o + 3] = (v.flags & kp::SV_USED != 0) as u8;
                        r[o + 4] = (v.elevation + 0.5) as i8 as u8;
                        r[o + 5..o + 7].copy_from_slice(&(((v.azimuth + 0.5) as u16) % 360).to_le_bytes());
                        r[o + 11..o + 13].copy_from_slice(&((v.cn0 * 4.) as u16).to_le_bytes());
                        r[o + 17..o + 21].copy_from_slice(&i32::MIN.to_le_bytes());
                        r[o + 21..o + 25].copy_from_slice(&i32::MIN.to_le_bytes());
                    }
                    reply(&token, &r[..9 + REC * s.nsv]);
                }
                3 if n >= 5 => {
                    let seen = u32::from_le_bytes([msg[1], msg[2], msg[3], msg[4]]);
                    let c = changes(&s, &pr);
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
                4 => {
                    if n < 2 {
                        const KREC: usize = 18;
                        let nk = s.nkinds.min(1 << 4);
                        let mut r = [0u8; 9 + (1 << 4) * KREC];
                        r[0] = nk as u8;
                        r[1..5].copy_from_slice(&s.packets.to_le_bytes());
                        r[5..9].copy_from_slice(&io.sc.bad.to_le_bytes());
                        for (i, k) in s.kinds[..nk].iter().enumerate() {
                            let o = 9 + KREC * i;
                            r[o + 2..o + 6].copy_from_slice(&k.word.to_be_bytes());
                            r[o + 6..o + 10].copy_from_slice(&k.count.to_le_bytes());
                            r[o + 10..o + 14].copy_from_slice(&k.bytes.to_le_bytes());
                            r[o + 14..o + 18].copy_from_slice(&((now().wrapping_sub(k.at) * 1000 / f) as u32).to_le_bytes());
                        }
                        reply(&token, &r[..9 + KREC * nk]);
                    } else {
                        reply(&token, &[]);
                    }
                }
                _ => {
                    reply(&token, &[]);
                }
            }
        }
        let c = changes(&s, &pr);
        for w in watches.iter_mut() {
            if w.is_some_and(|(_, seen)| seen != c) {
                if let Some((t, _)) = w.take() {
                    reply(&t, &c.to_le_bytes());
                }
            }
        }

        // Asleep until the chip raises its line or a call comes (one wait for both, no polling; payloads/gps), or the step's resend, the dead chip's retry, the sky's refresh is due.
        let mut due = 0u64;
        let mut at = |d: u64| {
            if d != 0 && (due == 0 || d < due) {
                due = d;
            }
        };
        match s.phase {
            Phase::Running | Phase::Idle => {}
            Phase::Dead => at(s.retry_at),
            _ => at(s.pend_at + ms(f, RESEND_MS) + 1),
        }
        if pr.n > 0 {
            at(pr.at + predict::EVERY_S * f);
        }
        if !io.chip.gnss2ap() && !s.owe_ack && !s.owe_reset_ack {
            // 0: the line fired (the next pass reads once, whatever the line says by then); a deadline or a call answers otherwise.
            fired = ferros_sys::irq_wait_world(cap(g, S_IRQ), if !polled { 1 } else { due }) == 0;
            irqs += fired as u32;
        }
        mmio_write(far, EINT_PEND, PIN);
        ferros_sys::irq_ack(cap(g, S_IRQ));
        polled = true;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

ferros_sys::entry!(payload_main, grows HEAP_CEILING, starts HEAP_START_LOG2);
