//! An hv oracle capture of akita's spi_21 (the Samsung KEPLER GNSS, PIO only) turned back into SPI transfers and the CHPP packets they carry.
//! Input: the `ferros hv: S2` lines and `ferros-mark:` lines of a capture log (tools/akita-hv-camcap.sh gnss-fix / gnss-boot).
//! The controller's data words: TX_DATA (+G#18) writes and RX_DATA (+G#1C) reads, one 32-bit word each at 32 bits a word (the bytes in wire order as the word's little-endian bytes, SWAP_CFG doing the swap), one byte each at 8.
//! A transfer opens with a PACKET_CNT (+G#20) write with bit 16 set (its low half the count) and closes with the write of 0.
//! The AP-to-GNSS handshake is gpp4-3, bit 3 of PERIC0's G#1084_0084.
//! CHPP (the context hub peripheral protocol) frames each packet: the preamble 68 43, an 8-byte transport header (flags, packet code, ack seq, seq, length u16, reserved u16), the payload, a CRC-32 over header and payload.
//! A payload that starts a message (not a continuation) begins with CHPP's app header: handle, type, transaction, error, command u16.
//! Positions: payload bytes are shown only with --hex, and only that many: the session's location reports carry the place.
//! --betp: instead of packets, the datagrams put back together (ferros_kepler) and the BETP records in them, then a tally by word.
//! --gtrace: the input is a gtrace log of gnssd (tools/akita-gnss-trace.sh) instead of an hv capture: whole, nothing lost, the CP's gnss_boot commands as marks.
//! Usage: gnss-oracle <capture.log> [--hex <n>] [--transfers] [--betp] [--gtrace]

use std::collections::BTreeMap;

const SPI: u64 = 0x111E_0000;
const TX_DATA: u64 = SPI + 0x18;
const RX_DATA: u64 = SPI + 0x1C;
const PACKET_CNT: u64 = SPI + 0x20;
const GPIO_DAT: u64 = 0x1084_0084;
const AP2GNSS: u64 = 1 << 3;

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Tx,
    Rx,
}

impl Dir {
    fn name(self) -> &'static str {
        match self {
            Dir::Tx => "AP>GNSS",
            Dir::Rx => "GNSS>AP",
        }
    }
}

enum Event {
    Mark(String),
    Transfer { dir: Dir, words: u64, bytes: Vec<u8> },
    Handshake(bool),
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// The capture's events in order: marks, handshake edges and whole transfers.
fn events(text: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut prev: Option<(u64, bool, u8, u64)> = None;
    let mut open: Option<(u64, Vec<u8>, Vec<u8>)> = None;
    let mut level = false;
    let mut apply = |addr: u64, write: bool, size: u8, val: u64, out: &mut Vec<Event>, open: &mut Option<(u64, Vec<u8>, Vec<u8>)>| {
        match (addr, write) {
            (PACKET_CNT, true) if val & (1 << 16) != 0 => *open = Some((val & 0xFFFF, Vec::new(), Vec::new())),
            (PACKET_CNT, true) if val == 0 => {
                if let Some((words, tx, rx)) = open.take() {
                    if !tx.is_empty() {
                        out.push(Event::Transfer { dir: Dir::Tx, words, bytes: tx });
                    }
                    if !rx.is_empty() {
                        out.push(Event::Transfer { dir: Dir::Rx, words, bytes: rx });
                    }
                }
            }
            (TX_DATA, true) => {
                if let Some((_, tx, _)) = open.as_mut() {
                    tx.extend_from_slice(&(val as u32).to_le_bytes()[..size as usize]);
                }
            }
            (RX_DATA, false) => {
                if let Some((_, _, rx)) = open.as_mut() {
                    rx.extend_from_slice(&(val as u32).to_le_bytes()[..size as usize]);
                }
            }
            (GPIO_DAT, true) => {
                let l = val & AP2GNSS != 0;
                if l != level {
                    level = l;
                    out.push(Event::Handshake(l));
                }
            }
            _ => {}
        }
    };
    for line in text.lines() {
        if let Some(i) = line.find("ferros-mark: ") {
            out.push(Event::Mark(line[i + 13..].trim().to_string()));
            continue;
        }
        let Some(i) = line.find("ferros hv: S2 ") else { continue };
        let f: Vec<&str> = line[i + 14..].split_whitespace().collect();
        if f.len() == 2 && f[0] == "rep" {
            let n = u64::from_str_radix(f[1], 16).unwrap_or(0);
            if let Some((a, w, s, v)) = prev {
                for _ in 0..n {
                    apply(a, w, s, v, &mut out, &mut open);
                }
            }
            continue;
        }
        if f.len() != 3 || f[1].len() != 2 {
            continue;
        }
        let (Ok(a), Ok(v)) = (u64::from_str_radix(f[0], 16), u64::from_str_radix(f[2], 16)) else { continue };
        let w = f[1].starts_with('W');
        let s = f[1][1..].parse::<u8>().unwrap_or(4);
        prev = Some((a, w, s, v));
        apply(a, w, s, v, &mut out, &mut open);
    }
    out
}

/// A gtrace log of gnssd (tools/akita-gnss-trace.sh) as the same events: its writes to `/dev/gnss_ipc` the AP's bytes, its reads the chip's (each read the kernel's whole 1024-byte buffer), every `/dev/gnss_boot` write and read and every GNSS image ioctl as a mark with its bytes (the CP's commands for the core: power, reset, firmware; no place travels there), and gtrace's own `#` lines as marks.
fn gtrace_events(text: &str) -> Vec<Event> {
    let unhex = |h: &str| (0..h.len() / 2).filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect::<Vec<u8>>();
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            out.push(Event::Mark(line[1..].trim().to_string()));
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        let (call, path) = (f[2], f[4]);
        let bytes = f.last().map(|h| if h.contains('=') { Vec::new() } else { unhex(h) }).unwrap_or_default();
        if path.contains("gnss_boot") || f.iter().any(|x| x.starts_with("struct=")) {
            out.push(Event::Mark(format!("{} {} {}", f[0], call, f[4..].join(" "))));
        } else if path.contains("gnss_ipc") && !bytes.is_empty() {
            match call {
                "write" => out.push(Event::Transfer { dir: Dir::Tx, words: (bytes.len() / 4) as u64, bytes }),
                "read" => out.push(Event::Transfer { dir: Dir::Rx, words: (bytes.len() / 4) as u64, bytes }),
                _ => {}
            }
        }
    }
    out
}

struct Packet {
    start: usize,
    flags: u8,
    code: u8,
    ack: u8,
    seq: u8,
    payload: Vec<u8>,
}

/// The CHPP MTU both ends offer in their transport reset (A#1280): a candidate longer than it is not a packet.
const MTU: usize = 1280;

/// Every CHPP packet in one direction's stream, all its transfers end to end (a GNSS-to-AP read is 1024 bytes, so a packet can span two), and how many plausible candidates failed their CRC.
fn packets(b: &[u8]) -> (Vec<Packet>, u64) {
    let mut out = Vec::new();
    let mut bad = 0;
    let mut i = 0;
    while i + 2 + 8 + 4 <= b.len() {
        if b[i] != 0x68 || b[i + 1] != 0x43 {
            i += 1;
            continue;
        }
        let h = &b[i + 2..i + 10];
        let len = u16::from_le_bytes([h[4], h[5]]) as usize;
        let end = i + 10 + len + 4;
        if len > MTU || end > b.len() {
            i += 1;
            continue;
        }
        let crc = u32::from_le_bytes([b[end - 4], b[end - 3], b[end - 2], b[end - 1]]);
        if crc32(&b[i + 2..i + 10 + len]) != crc {
            bad += 1;
            i += 1;
            continue;
        }
        out.push(Packet { start: i, flags: h[0], code: h[1], ack: h[2], seq: h[3], payload: b[i + 10..i + 10 + len].to_vec() });
        i = end;
    }
    (out, bad)
}

fn app_type(t: u8) -> &'static str {
    match t {
        0 => "client request",
        1 => "service response",
        2 => "client notification",
        3 => "service notification",
        4 => "service request",
        5 => "client response",
        _ => "?",
    }
}

/// The datagrams of both directions put back together as the driver does (ferros_kepler `Datagram`: resends dropped, fragments joined), and each BETP record in them: its word, its head's other fields, its length, and with --hex that many of its bytes.
/// Then a tally by direction and word.
fn betp_lines(stream: &[Vec<u8>; 2], owner: &[Vec<usize>; 2], show: usize, lines: &mut Vec<(usize, usize, String)>) {
    use ferros_kepler::{chip_records, App, Datagram, Header, Took};
    let mut tally: BTreeMap<(&'static str, u32), (u64, u64, usize, usize)> = BTreeMap::new();
    for (d, dir) in [Dir::Tx, Dir::Rx].into_iter().enumerate() {
        let (ps, _) = packets(&stream[d]);
        let mut dg: Box<Datagram<{ 1 << 16 }>> = Box::new(Datagram::new());
        for p in ps.iter().filter(|p| !p.payload.is_empty() && p.code & 0xF0 == 0) {
            let h = Header { flags: p.flags, code: p.code, ack: p.ack, seq: p.seq, len: p.payload.len() as u16 };
            if dg.take(&h, &p.payload) != Took::Whole {
                continue;
            }
            let b = dg.bytes();
            let Some(app) = App::read(b) else { continue };
            if app.handle != ferros_kepler::BETP || (app.cmd != ferros_kepler::CMD_AP_BETP && app.cmd != ferros_kepler::CMD_CHIP_BETP) {
                continue;
            }
            let body = &b[App::LEN..];
            let at = owner[d][p.start];
            if dir == Dir::Tx {
                // The AP's 20-byte heads: word, length, a u32, next offset, next size, head size, length.
                let mut o = 0;
                loop {
                    if o + 20 > body.len() {
                        break;
                    }
                    let r = &body[o..];
                    let word = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
                    let len = u32::from_le_bytes([r[4], r[5], r[6], r[7]]) as usize;
                    let next = u16::from_le_bytes([r[12], r[13]]) as usize;
                    let data = &r[20..(20 + len).min(r.len())];
                    let mut line = format!("{} BETP G#{:04X}_{:04X} len A#{}", dir.name(), word >> 16, word & 0xFFFF, len);
                    if show > 0 {
                        line += &format!(" | {}", hex(&data[..show.min(data.len())]));
                    }
                    lines.push((at, 2, line));
                    // GNSS_REPLAY_OUT=<file>: the AP's records in order, each `word u32, length u16, bytes` (little-endian), for the driver to play (gnss_kepler `gps.kepler.session`); G#3348 (the sensors' stream) left out. The file carries what gnssd told the chip, the place among it: keys/ and the device's vault only.
                    if let Ok(p) = std::env::var("GNSS_REPLAY_OUT") {
                        if word & 0xFFFF != 0x3348 {
                            use std::io::Write;
                            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).expect("open GNSS_REPLAY_OUT");
                            let mut rec = word.to_le_bytes().to_vec();
                            rec.extend_from_slice(&(data.len() as u16).to_le_bytes());
                            rec.extend_from_slice(data);
                            f.write_all(&rec).expect("write GNSS_REPLAY_OUT");
                        }
                    }
                    let e = tally.entry((dir.name(), word)).or_insert((0, 0, usize::MAX, 0));
                    e.0 += 1;
                    e.1 += len as u64;
                    e.2 = e.2.min(len);
                    e.3 = e.3.max(len);
                    if next == 0 {
                        break;
                    }
                    o += next;
                }
            } else {
                let mut recs = chip_records(body);
                for r in recs.by_ref() {
                    let mut line = format!("{} BETP G#{:04X}_{:04X} tag G#{:08X} aux G#{:04X} len A#{}", dir.name(), r.word >> 16, r.word & 0xFFFF, r.tag, r.aux, r.data.len());
                    if show > 0 {
                        // The fix's place (ferros_kepler `Pvt`: latitude and longitude at +48 and +52) never shown, even with --hex.
                        let mut shown = r.data[..show.min(r.data.len())].to_vec();
                        if r.word & 0xFFFF == ferros_kepler::PVT {
                            for b in shown.iter_mut().skip(48).take(8) {
                                *b = 0xEE;
                            }
                        }
                        line += &format!(" | {}", hex(&shown));
                    }
                    // The decoded fix (no place: its validity, type, time, error estimates, HDOP) and the satellite list, as the driver reads them.
                    if r.word & 0xFFFF == ferros_kepler::PVT {
                        if let Some(p) = ferros_kepler::Pvt::read(r.data) {
                            line += &format!(" | fix: valid G#{:X} type G#{:X} week A#{} tow A#{} ms, ±A#{} m horizontal, ±A#{} m vertical, HDOP A#{}/5, speed A#{} cm/s", p.valid, p.kind, p.week, p.tow_ms, p.ehpe_cm / 100, p.evpe_cm / 100, p.hdop_fifths, p.speed_cms);
                        }
                    }
                    if r.word & 0xFFFF == ferros_kepler::SVS {
                        let n = ferros_kepler::Sv::count(r.data);
                        line += &format!(" | A#{n} satellites:");
                        for k in 0..n {
                            match ferros_kepler::Sv::read(r.data, k) {
                                Some(s) => line += &format!(" {}{}{} {:.0}dB {:.0}/{:.0} {:.0}MHz;", s.system, if s.flags & ferros_kepler::SV_USED != 0 { "*" } else { ":" }, s.number, s.cn0, s.elevation, s.azimuth, s.carrier_hz / 1e6),
                                None => line += " (short)",
                            }
                        }
                    }
                    lines.push((at, 2, line));
                    let e = tally.entry((dir.name(), r.word)).or_insert((0, 0, usize::MAX, 0));
                    e.0 += 1;
                    e.1 += r.data.len() as u64;
                    e.2 = e.2.min(r.data.len());
                    e.3 = e.3.max(r.data.len());
                }
                if recs.short {
                    lines.push((at, 2, format!("{} BETP: a record ran past its datagram (A#{} bytes)", dir.name(), body.len())));
                }
            }
        }
        lines.push((usize::MAX, 0, format!("# {}: A#{} resends dropped, A#{} sequence skips, A#{} datagrams cut", dir.name(), dg.resends, dg.skips, dg.cut)));
    }
    for ((d, w), (n, b, lo, hi)) in tally {
        lines.push((usize::MAX, 1, format!("#   {d} BETP G#{:04X}_{:04X}: A#{n} records, A#{b} bytes, A#{lo} to A#{hi} each", w >> 16, w & 0xFFFF)));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: gnss-oracle <capture.log> [--hex <n>] [--transfers] [--betp]");
        std::process::exit(2);
    }
    let show = args.iter().position(|a| a == "--hex").and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let transfers = args.iter().any(|a| a == "--transfers");
    let betp = args.iter().any(|a| a == "--betp");
    let text = String::from_utf8_lossy(&std::fs::read(&args[1]).expect("read the capture")).into_owned();
    let ev = if args.iter().any(|a| a == "--gtrace") { gtrace_events(&text) } else { events(&text) };
    // Each direction's stream, and for each byte the event it came in, which orders packets against marks and the other direction.
    let mut stream: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    let mut owner: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    let (mut n, mut raises) = ([0u64; 2], 0u64);
    // What gets printed, keyed by event index then by a tie order: marks and transfer lines first in their event, packets after.
    let mut lines: Vec<(usize, usize, String)> = Vec::new();
    for (k, e) in ev.iter().enumerate() {
        match e {
            Event::Mark(m) => lines.push((k, 0, format!("# MARK {m}"))),
            Event::Handshake(up) => raises += *up as u64,
            Event::Transfer { dir, words, bytes } => {
                let d = *dir as usize;
                n[d] += 1;
                if transfers {
                    lines.push((k, 0, format!("{} transfer, A#{} words, A#{} bytes", dir.name(), words, bytes.len())));
                }
                stream[d].extend_from_slice(bytes);
                owner[d].extend(std::iter::repeat_n(k, bytes.len()));
            }
        }
    }
    let mut kinds: BTreeMap<(&'static str, u8, u8, u16), (u64, u64)> = BTreeMap::new();
    let (mut total, mut bad, mut in_packets) = (0u64, 0u64, [0u64; 2]);
    for (d, dir) in [Dir::Tx, Dir::Rx].into_iter().enumerate() {
        let (ps, b) = packets(&stream[d]);
        bad += b;
        let mut cont = false;
        for p in ps {
            total += 1;
            in_packets[d] += 14 + p.payload.len() as u64;
            let mut line = format!("{} seq G#{:02X} ack G#{:02X} code G#{:02X} flags G#{:02X} len A#{}", dir.name(), p.seq, p.ack, p.code, p.flags, p.payload.len());
            // A transport reset (G#10) or its ack (G#20) carries the link's configuration, not a message; a payload begins a message unless the packet before it in this direction said more follows (flags bit 0).
            if p.code & 0xF0 != 0 {
                line += " | transport reset";
            } else if !cont && p.payload.len() >= 6 {
                let q = &p.payload;
                let cmd = u16::from_le_bytes([q[4], q[5]]);
                line += &format!(" | handle G#{:02X} {} txn G#{:02X} err G#{:02X} cmd G#{:04X}", q[0], app_type(q[1]), q[2], q[3], cmd);
                let e = kinds.entry((dir.name(), q[0], q[1], cmd)).or_insert((0, 0));
                e.0 += 1;
                e.1 += p.payload.len() as u64;
            } else if cont {
                line += " | continued";
            }
            cont = p.flags & 1 != 0;
            if show > 0 {
                line += &format!(" | {}", hex(&p.payload[..show.min(p.payload.len())]));
            }
            lines.push((owner[d][p.start], 1, line));
        }
    }
    if betp {
        lines.retain(|l| l.2.starts_with("# MARK"));
        betp_lines(&stream, &owner, show, &mut lines);
    }
    lines.sort_by_key(|l| (l.0, l.1));
    for l in &lines {
        println!("{}", l.2);
    }
    println!("# A#{} AP>GNSS transfers (A#{} bytes, A#{} in packets), A#{} GNSS>AP transfers (A#{} bytes, A#{} in packets), A#{total} CHPP packets, A#{bad} plausible candidates with a bad CRC, A#{raises} AP-to-GNSS raises", n[0], stream[0].len(), in_packets[0], n[1], stream[1].len(), in_packets[1]);
    println!("# messages by direction, handle, type and command: count, payload bytes");
    for ((d, h, t, c), (n, b)) in kinds {
        println!("#   {d} handle G#{h:02X} {} cmd G#{c:04X}: A#{n}, A#{b}", app_type(t));
    }
}
