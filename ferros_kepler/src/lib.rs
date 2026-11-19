//! The 8a's GNSS link (docs/GPS.md "akita's KEPLER"): what goes over spi_21 between the AP and the KEPLER GNSS core, decoded from the hv captures of 2026-11-18 (keys/akita/captures-2026-11-18, local) and gnssd's own symbols (it ships unstripped in the factory image).
//!
//! Three layers, each its own part here:
//! - **CHPP's transport** ([`Header`], [`frame`], [`Scanner`]): the preamble `68 43`, an 8-byte header (flags, packet code, ack, sequence, length u16, reserved u16), the payload, a CRC-32 over header and payload, little-endian thruout.
//!   Every packet of both captures checks.
//!   On the wire each packet is padded with zeros to whole 32-bit words (the port runs 32-bit words, each word's little-endian bytes in wire order); a read is mostly zeros after its packets.
//! - **CHPP's app layer** ([`App`]): a 6-byte header (handle, type, transaction, error, command u16) on the first packet of each datagram; handle G#0F discovery, G#02 timesync, G#10 the one service the chip offers, named "BETP" in its discovery answer.
//! - **BETP records** ([`ApRecord`], [`chip_records`]): inside the service's notifications, Samsung's (SiRF-born) BETP messages, each a record with a 32-bit word naming it and its bytes.
//!   The AP's records (gnssd `chppBetpClientSendGnssMessage`) have a 20-byte head, the chip's (gnssd `chppDispatchBetpNotification`) a 16-byte one.
//!
//! The link's habits, as gnssd ran it (both captures):
//! - The AP's payload packet takes as its sequence the chip's latest ack, and its ack is the chip's latest sequence plus one.
//! - The AP answers every chip packet with a payload by a bare ack (sequence 0, length 0, ack = that sequence plus one).
//! - The chip resends a packet until it is acked (seq G#07 three times in the boot capture), so a sequence seen twice is a resend, dropped and acked again.
//! - The transport reset (code G#10) and its ack (G#20) carry each end's configuration: CHPP 1.0.0, an MTU of A#1280, a window of 1, a timeout of A#1000 ms.
//!
//! Positions: nothing here decodes a place; [`Fix`]'s decoder is in this crate only once its fields are known, and no test data carries a position (the capture's location messages stay in keys/).
#![no_std]

#[cfg(test)]
mod tests;

/// The packet preamble.
pub const PREAMBLE: [u8; 2] = [0x68, 0x43];
/// Preamble, header and CRC: a packet's bytes besides its payload.
pub const OVERHEAD: usize = 2 + 8 + 4;
/// The MTU both ends offer in their transport reset: a header claiming more is not a packet.
pub const MTU: usize = 1280;
/// The longest packet on the wire, padded to whole words.
pub const PACKET_MAX: usize = (OVERHEAD + MTU + 3) & !3;

/// Header flags bit 0: more of this datagram follows in the next packet.
pub const MORE: u8 = 1;
/// Packet codes: the high nibble an attribute, the low nibble an error.
pub const RESET: u8 = 0x10;
pub const RESET_ACK: u8 = 0x20;
/// The error CHPP calls out of order (the AP sent it A#21 times in the fix session, gnssd's ask for a resend).
pub const ERR_ORDER: u8 = 0x05;

/// CRC-32 (IEEE 802.3, reflected, G#EDB8_8320, initial and final inversion): CHPP's `chppCrc32`, over the 8 header bytes and the payload.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

const TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// CHPP's transport header (`ChppTransportHeader`); `reserved` is always 0 on this link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Header {
    pub flags: u8,
    pub code: u8,
    pub ack: u8,
    pub seq: u8,
    pub len: u16,
}

/// One packet framed into `out`: preamble, header, payload, CRC, then zeros to the next whole 32-bit word (the port's word). Returns the padded length; `out` must hold it (at most [`PACKET_MAX`]).
pub fn frame(flags: u8, code: u8, ack: u8, seq: u8, payload: &[u8], out: &mut [u8]) -> usize {
    let n = payload.len();
    out[0] = PREAMBLE[0];
    out[1] = PREAMBLE[1];
    out[2] = flags;
    out[3] = code;
    out[4] = ack;
    out[5] = seq;
    out[6..8].copy_from_slice(&(n as u16).to_le_bytes());
    out[8] = 0;
    out[9] = 0;
    out[10..10 + n].copy_from_slice(payload);
    let c = crc32(&out[2..10 + n]);
    out[10 + n..14 + n].copy_from_slice(&c.to_le_bytes());
    let padded = (OVERHEAD + n + 3) & !3;
    for b in &mut out[OVERHEAD + n..padded] {
        *b = 0;
    }
    padded
}

/// What [`Scanner::next`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Found<'a> {
    /// A packet whose CRC checked: its header and payload.
    Packet(Header, &'a [u8]),
    /// Nothing more in what has been fed: the bytes from the last possible packet start are kept for the next feed.
    More,
}

/// A direction's byte stream (every read appended as it came) cut into CHPP packets: a packet can span two reads, so the bytes from a preamble whose packet is not yet whole are kept.
/// The fill between packets (zeros) is skipped; a preamble whose header claims more than the MTU, or whose CRC fails, is not a packet and the scan moves on by one byte (`bad` counts the CRC failures).
pub struct Scanner<const N: usize> {
    buf: [u8; N],
    /// Bytes held, and where the scan stands in them.
    n: usize,
    at: usize,
    pub bad: u32,
}

impl<const N: usize> Default for Scanner<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Scanner<N> {
    pub const fn new() -> Self {
        Scanner { buf: [0; N], n: 0, at: 0, bad: 0 }
    }

    /// The bytes of one read appended. What was scanned past is dropped first; the caller feeds no more than `N` less what is held (a read of A#1024 into a scanner of at least [`PACKET_MAX`] + A#1024 always fits).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.copy_within(self.at..self.n, 0);
        self.n -= self.at;
        self.at = 0;
        self.buf[self.n..self.n + bytes.len()].copy_from_slice(bytes);
        self.n += bytes.len();
    }

    /// The next packet in what has been fed, or [`Found::More`].
    pub fn next(&mut self) -> Found<'_> {
        while self.at + 2 <= self.n {
            let b = &self.buf[self.at..self.n];
            if b[0] != PREAMBLE[0] || b[1] != PREAMBLE[1] {
                self.at += 1;
                continue;
            }
            if b.len() < 10 {
                return Found::More;
            }
            let len = u16::from_le_bytes([b[6], b[7]]) as usize;
            if len > MTU {
                self.at += 1;
                continue;
            }
            let end = OVERHEAD + len;
            if b.len() < end {
                return Found::More;
            }
            let crc = u32::from_le_bytes([b[end - 4], b[end - 3], b[end - 2], b[end - 1]]);
            if crc32(&b[2..10 + len]) != crc {
                self.bad += 1;
                self.at += 1;
                continue;
            }
            let h = Header { flags: b[2], code: b[3], ack: b[4], seq: b[5], len: len as u16 };
            let start = self.at + 10;
            self.at += end;
            return Found::Packet(h, &self.buf[start..start + len]);
        }
        Found::More
    }
}

/// A transport reset's payload, and its ack's (both ends send the same): CHPP 1.0.0, MTU A#1280, window 1, timeout A#1000 ms (`ChppTransportConfiguration`).
pub const LINK_CONFIG: [u8; 10] = [1, 0, 0, 0, 0x00, 0x05, 0x01, 0x00, 0xE8, 0x03];

/// CHPP's app header types.
pub const CLIENT_REQUEST: u8 = 0;
pub const SERVICE_RESPONSE: u8 = 1;
pub const CLIENT_NOTIFICATION: u8 = 2;
pub const SERVICE_NOTIFICATION: u8 = 3;

/// The handles: timesync and discovery are CHPP's fixed ones, G#10 the first (and the chip's only) negotiated service.
pub const TIMESYNC: u8 = 0x02;
pub const DISCOVERY: u8 = 0x0F;
pub const BETP: u8 = 0x10;

/// The commands used: discovery's "discover all" and timesync's "get time" are 1, a service's open 0; a BETP datagram is a client notification with command 2 (AP to chip) or a service notification with command 3 (chip to AP).
pub const CMD_DISCOVER: u16 = 1;
pub const CMD_TIME: u16 = 1;
pub const CMD_OPEN: u16 = 0;
pub const CMD_AP_BETP: u16 = 2;
pub const CMD_CHIP_BETP: u16 = 3;

/// CHPP's app header, on a datagram's first packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    pub handle: u8,
    pub kind: u8,
    pub txn: u8,
    pub err: u8,
    pub cmd: u16,
}

impl App {
    pub const LEN: usize = 6;
    pub fn read(b: &[u8]) -> Option<App> {
        if b.len() < Self::LEN {
            return None;
        }
        Some(App { handle: b[0], kind: b[1], txn: b[2], err: b[3], cmd: u16::from_le_bytes([b[4], b[5]]) })
    }
    pub fn write(&self, out: &mut [u8]) {
        out[0] = self.handle;
        out[1] = self.kind;
        out[2] = self.txn;
        out[3] = self.err;
        out[4..6].copy_from_slice(&self.cmd.to_le_bytes());
    }
}

/// The chip's one service as its discovery answer names it: the 16-byte UUID, then the name "BETP" in a 16-byte field (CHPP's `ChppServiceDescriptor`), then the version (major u8, minor u8, patch u16).
pub const BETP_UUID: [u8; 16] = [0xB4, 0x34, 0xC1, 0x4D, 0x9C, 0x66, 0x46, 0xC7, 0x8F, 0xF9, 0xF5, 0xD9, 0x10, 0xA4, 0xD2, 0xBD];

/// The discovery answer's service: its name field read, and whether its UUID is BETP's.
pub fn discovered_betp(answer: &[u8]) -> bool {
    answer.len() >= App::LEN + 16 + 16 + 4 && answer[App::LEN..App::LEN + 16] == BETP_UUID && &answer[App::LEN + 16..App::LEN + 20] == b"BETP"
}

/// One BETP message the AP sends: its word (the low half the message's id, gnssd's "midsid"; the high half its protocol) and its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApRecord<'a> {
    pub word: u32,
    pub data: &'a [u8],
}

/// The AP's record head: word u32, length u32, a u32 (0 in every capture), the offset to the next record u16 and that record's size u16 (both 0 on the last), the head's size u16 (A#20), the length again u16.
pub const AP_HEAD: usize = 20;
/// gnssd's datagram is sized for one more head than it fills (`chppBetpClientSendGnssMessage`: A#26 + A#20 a record + their bytes), sent as zeros.
pub const AP_SLACK: usize = 20;

/// A BETP datagram for the AP's records: the app header (handle G#10, a client notification, command 2), the records, the slack. Returns its length; `out` must hold A#26 + A#20 a record + their bytes.
pub fn ap_datagram(records: &[ApRecord], out: &mut [u8]) -> usize {
    App { handle: BETP, kind: CLIENT_NOTIFICATION, txn: 0, err: 0, cmd: CMD_AP_BETP }.write(out);
    let mut at = App::LEN;
    for (i, r) in records.iter().enumerate() {
        let n = r.data.len();
        let (next, size) = match records.get(i + 1) {
            Some(nx) => ((AP_HEAD + n) as u16, (AP_HEAD + nx.data.len()) as u16),
            None => (0, 0),
        };
        out[at..at + 4].copy_from_slice(&r.word.to_le_bytes());
        out[at + 4..at + 8].copy_from_slice(&(n as u32).to_le_bytes());
        out[at + 8..at + 12].copy_from_slice(&0u32.to_le_bytes());
        out[at + 12..at + 14].copy_from_slice(&next.to_le_bytes());
        out[at + 14..at + 16].copy_from_slice(&size.to_le_bytes());
        out[at + 16..at + 18].copy_from_slice(&(AP_HEAD as u16).to_le_bytes());
        out[at + 18..at + 20].copy_from_slice(&(n as u16).to_le_bytes());
        out[at + AP_HEAD..at + AP_HEAD + n].copy_from_slice(r.data);
        at += AP_HEAD + n;
    }
    for b in &mut out[at..at + AP_SLACK] {
        *b = 0;
    }
    at + AP_SLACK
}

/// One BETP message from the chip: its word, the head's second word (G#FFFF_FFFF on the first record of the boot capture's notification, gnssd keeps it beside the word), the head's u16 at +A, and its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChipRecord<'a> {
    pub word: u32,
    pub tag: u32,
    pub aux: u16,
    pub data: &'a [u8],
}

/// The chip's record head: word u32, tag u32, the offset to the next record u16 (0 on the last), a u16, the data's offset u16 (A#16), the data's length u16.
pub const CHIP_HEAD: usize = 16;

/// The records of a service notification's body (after the app header), in order. A record whose head or data runs past the body ends the walk (`Records::short` then says so).
pub fn chip_records(body: &[u8]) -> Records<'_> {
    Records { b: body, at: 0, done: false, short: false }
}

pub struct Records<'a> {
    b: &'a [u8],
    at: usize,
    done: bool,
    pub short: bool,
}

impl<'a> Iterator for Records<'a> {
    type Item = ChipRecord<'a>;
    fn next(&mut self) -> Option<ChipRecord<'a>> {
        if self.done {
            return None;
        }
        let b = &self.b[self.at..];
        if b.len() < CHIP_HEAD {
            self.done = true;
            self.short = !b.is_empty();
            return None;
        }
        let rd16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
        let word = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let tag = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        let (next, aux, off, len) = (rd16(8), rd16(10) as u16, rd16(12), rd16(14));
        if off + len > b.len() {
            self.done = true;
            self.short = true;
            return None;
        }
        let data = &b[off..off + len];
        if next == 0 {
            self.done = true;
        } else {
            self.at += next;
        }
        Some(ChipRecord { word, tag, aux, data })
    }
}

/// A datagram from the chip put back together from its packets: the packets in sequence (a resend, the sequence seen just before, is dropped), the fragments joined while the header says more follows.
/// `N` bounds a datagram; one longer is cut there and counted (`cut`).
pub struct Datagram<const N: usize> {
    buf: [u8; N],
    n: usize,
    /// The sequence of the last payload packet taken, None before the first.
    pub last: Option<u8>,
    /// Packets dropped as resends, datagrams cut at `N`, packets whose sequence skipped ahead (one or more lost: the datagram they belonged to is dropped).
    pub resends: u32,
    pub cut: u32,
    pub skips: u32,
    open: bool,
    /// The rest of a datagram that lost a packet, dropped until its last.
    dropping: bool,
}

/// What a payload packet did to the datagram being put together.
#[derive(Debug, PartialEq, Eq)]
pub enum Took {
    /// A resend of the last packet: nothing new.
    Resend,
    /// Taken; more of its datagram to come.
    Part,
    /// Taken, and it completed its datagram: [`Datagram::bytes`] holds it until the next packet.
    Whole,
}

impl<const N: usize> Default for Datagram<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Datagram<N> {
    pub const fn new() -> Self {
        Datagram { buf: [0; N], n: 0, last: None, resends: 0, cut: 0, skips: 0, open: false, dropping: false }
    }

    /// The link was reset: the chip's sequences start again.
    pub fn restart(&mut self) {
        self.last = None;
        self.open = false;
        self.dropping = false;
        self.n = 0;
    }

    /// A payload packet from the chip.
    pub fn take(&mut self, h: &Header, payload: &[u8]) -> Took {
        if self.last == Some(h.seq) {
            self.resends += 1;
            return Took::Resend;
        }
        // A sequence that is not the last plus one: a packet was lost between, so the datagram it fell in is not whole. A first packet starts afresh; a continuation is dropped, with the rest of its datagram up to the packet that ends it.
        let skipped = self.last.is_some_and(|l| l.wrapping_add(1) != h.seq);
        self.last = Some(h.seq);
        let more = h.flags & MORE != 0;
        if skipped {
            self.skips += 1;
            if self.open {
                self.dropping = true;
            }
        }
        if self.dropping {
            self.open = more;
            self.dropping = more;
            self.n = 0;
            return Took::Part;
        }
        if !self.open {
            self.n = 0;
        }
        let room = N - self.n;
        let k = payload.len().min(room);
        // WHY the min: a datagram's length is the chip's to choose (fragments joined until it stops saying more); past N it is cut, counted, never written past the buffer.
        if k < payload.len() {
            self.cut += 1;
        }
        self.buf[self.n..self.n + k].copy_from_slice(&payload[..k]);
        self.n += k;
        self.open = more;
        if more { Took::Part } else { Took::Whole }
    }

    /// The datagram just completed.
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.n]
    }
}

/// The AP's half of the transport, as gnssd ran it: the sequences and acks for the next packet out.
#[derive(Clone, Copy, Debug, Default)]
pub struct Link {
    /// The chip's latest ack field, which the AP's next payload packet takes as its sequence.
    pub chip_ack: u8,
    /// The chip's latest payload sequence, plus one: the AP's ack.
    pub next_from_chip: u8,
}

impl Link {
    /// Every packet from the chip: its ack moves the AP's next sequence; a payload packet moves the AP's ack.
    pub fn saw(&mut self, h: &Header) {
        self.chip_ack = h.ack;
        if h.len > 0 || h.code & 0xF0 != 0 {
            self.next_from_chip = h.seq.wrapping_add(1);
        }
    }
    /// A bare ack for what has come.
    pub fn ack(&self, out: &mut [u8]) -> usize {
        frame(0, 0, self.next_from_chip, 0, &[], out)
    }
    /// A bare ack asking for a resend (CHPP's out-of-order error), gnssd's answer to a packet that skipped ahead.
    pub fn nak(&self, out: &mut [u8]) -> usize {
        frame(0, ERR_ORDER, self.next_from_chip, 0, &[], out)
    }
    /// A payload packet (a datagram of at most one MTU: every AP datagram in the captures fits).
    pub fn packet(&self, payload: &[u8], out: &mut [u8]) -> usize {
        frame(0, 0, self.next_from_chip, self.chip_ack, payload, out)
    }
    /// The transport reset that opens the link (sequence 0, ack 0).
    pub fn reset(out: &mut [u8]) -> usize {
        frame(0, RESET, 0, 0, &LINK_CONFIG, out)
    }
}

/// The AP's requests at gnssd's start, in order (the boot capture): discovery (sent twice, the first answer arriving while the second went out), the time, the BETP service's open.
pub const DISCOVER: [u8; 6] = [DISCOVERY, CLIENT_REQUEST, 0, 0, 1, 0];
pub const GET_TIME: [u8; 6] = [TIMESYNC, CLIENT_REQUEST, 0, 0, 1, 0];
pub const OPEN: [u8; 6] = [BETP, CLIENT_REQUEST, 0, 0, 0, 0];

/// The AP's BETP messages seen in the captures (the words gnssd sent and their bytes; their names are gnssd's to find: not yet tied to its message table):
/// - at gnssd's start, once the service opened: word G#0003_7304 with four zero bytes;
/// - as Maps asked for location: G#0002_12DE (no bytes), then G#0002_04D4 with G#0F;
/// - some 50 s on, G#0002_0398 with G#8E;
/// - as Maps closed: G#0002_12DE again.
pub const AP_START: ApRecord<'static> = ApRecord { word: 0x0003_7304, data: &[0, 0, 0, 0] };
pub const AP_SESSION_OPEN: ApRecord<'static> = ApRecord { word: 0x0002_12DE, data: &[] };
pub const AP_SESSION_ASK: ApRecord<'static> = ApRecord { word: 0x0002_04D4, data: &[0x0F] };
pub const AP_SESSION_LATER: ApRecord<'static> = ApRecord { word: 0x0002_0398, data: &[0x8E] };

/// The chip's fix: the low half of its word, SiRF's MID 67 SID 1 ("GNSS navigation data", gnssd's `SIRF_MSG_SSB_GNSS_NAV_DATA`), A#160 bytes.
pub const PVT: u32 = 0x0143;
/// The satellites the chip sees: the low half of a protocol-3 word, A#37 bytes of head then A#29 a satellite.
pub const SVS: u32 = 0x710A;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

/// The fix (MID 67 SID 1), laid out as SiRF's geodetic navigation message (MID 41) grown to 32-bit flags, read from the fix session's eight (the place itself never printed or kept here):
///
/// | Offset | Field |
/// |---|---|
/// | 0 | navigation valid u32: 0 a full fix; bit 0 (MID 41's) a fix not yet over-determined, the first two of the session |
/// | 4 | navigation type u32 (G#204 or G#206 once fixed, 0 in the first two) |
/// | 8 | GPS week u16 (A#2445 on 2026-11-19) |
/// | 10 | time of week u32, ms |
/// | 22 | UTC year u16, month u8, day u8, hour u8, minute u8, second u16 in ms, then the leap seconds u16 (A#18) |
/// | 48, 52 | latitude, longitude: i32, 10^-7 degrees |
/// | 56, 60 | altitude above the ellipsoid, above mean sea level: i32, cm |
/// | 64 | speed over the ground u16, cm/s |
/// | 66 | course over the ground u16, 0.01 degree |
/// | 68 | climb i16, cm/s |
/// | 80, 84 | the horizontal and vertical error estimates u32, cm (A#65 m falling to A#14 m indoors) |
/// | 90 | GDOP, PDOP, HDOP, VDOP, TDOP: u8 each in fifths (PDOP² = HDOP² + VDOP² and GDOP² = PDOP² + TDOP² hold in every record) |
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pvt {
    pub valid: u32,
    pub kind: u32,
    pub week: u16,
    pub tow_ms: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub alt_ellipsoid_cm: i32,
    pub alt_msl_cm: i32,
    pub speed_cms: u16,
    pub course_cdeg: u16,
    pub climb_cms: i16,
    pub ehpe_cm: u32,
    pub evpe_cm: u32,
    pub hdop_fifths: u8,
}

impl Pvt {
    pub const LEN: usize = 160;
    /// A fix message's bytes; None when shorter than the layout.
    pub fn read(d: &[u8]) -> Option<Pvt> {
        if d.len() < Self::LEN {
            return None;
        }
        Some(Pvt {
            valid: u32_at(d, 0),
            kind: u32_at(d, 4),
            week: u16_at(d, 8),
            tow_ms: u32_at(d, 10),
            lat_e7: u32_at(d, 48) as i32,
            lon_e7: u32_at(d, 52) as i32,
            alt_ellipsoid_cm: u32_at(d, 56) as i32,
            alt_msl_cm: u32_at(d, 60) as i32,
            speed_cms: u16_at(d, 64),
            course_cdeg: u16_at(d, 66),
            climb_cms: u16_at(d, 68) as i16,
            ehpe_cm: u32_at(d, 80),
            evpe_cm: u32_at(d, 84),
            hdop_fifths: d[92],
        })
    }
    /// Whether the message holds a place: the chip sends it only once it has one (none came before the first fix), and a zero place is none.
    pub fn has_place(&self) -> bool {
        self.lat_e7 != 0 || self.lon_e7 != 0
    }
    /// 10^-7 degrees to 2^-23 degrees (payloads/gps's unit): × 2^23 / 10^7, in i64 (180° is 2^30.7 in 10^-7 degrees, the product fits).
    pub fn to_units(e7: i32) -> i32 {
        ((e7 as i64) << 23).div_euclid(10_000_000) as i32
    }
}

/// One satellite of the chip's list (word G#710A), its 29 bytes: number u8, system u8 (Android's `GnssConstellationType`: 1 GPS, 2 SBAS, 3 GLONASS, 4 QZSS, 5 BeiDou, 6 Galileo, 7 NavIC), a u8 not known, flags u16 (Android's `GnssSvFlags`: 1 ephemeris, 2 almanac, 4 used in the fix, 8 carrier frequency known), then C/N0 (dB-Hz), elevation and azimuth (degrees) and the carrier (Hz) as f32, and an f64 not known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sv {
    pub number: u8,
    pub system: u8,
    pub flags: u16,
    pub cn0: f32,
    pub elevation: f32,
    pub azimuth: f32,
    pub carrier_hz: f32,
}

pub const SV_HEAD: usize = 37;
pub const SV_LEN: usize = 29;
/// Android's flag for a satellite used in the fix.
pub const SV_USED: u16 = 4;

impl Sv {
    /// The number of satellites the head says, and the k-th; None past the message.
    pub fn count(d: &[u8]) -> usize {
        if d.len() < SV_HEAD { 0 } else { d[SV_HEAD - 1] as usize }
    }
    pub fn read(d: &[u8], k: usize) -> Option<Sv> {
        let o = SV_HEAD + SV_LEN * k;
        if o + SV_LEN > d.len() {
            return None;
        }
        Some(Sv { number: d[o], system: d[o + 1], flags: u16_at(d, o + 3), cn0: f32_at(d, o + 5), elevation: f32_at(d, o + 9), azimuth: f32_at(d, o + 13), carrier_hz: f32_at(d, o + 17) })
    }
    /// The system as `ferros_sys::Sat` numbers it: 0 GPS, 1 GLONASS, 2 Galileo, 3 BeiDou, 4 QZSS, 5 NavIC, 7 other (SBAS among them).
    pub fn ferros_system(&self) -> u8 {
        match self.system {
            1 => 0,
            3 => 1,
            6 => 2,
            5 => 3,
            4 => 4,
            7 => 5,
            _ => 7,
        }
    }
    /// The band, as `ferros_sys::Sat`: 1 for L5, E5a and B2a (carriers below 1.3 GHz), 0 for the L1 group.
    pub fn band(&self) -> u8 {
        (self.carrier_hz > 0. && self.carrier_hz < 1.3e9) as u8
    }
}
