//! gps-trace: a gtrace capture of lhd (tools/gtrace: every read and write on /dev/ttyBCM with its bytes, the nSTANDBY writes) read back as the BCM4776's packet transport, both directions (docs/GPS.md "Captured and decoded").
//!
//! Time zero is lhd's nSTANDBY `1` (the chip's power-up), or the first write to ttyBCM when the capture has none; every time printed is milliseconds after it.
//!
//! Usage:
//! - `gps-trace <trace> frames [--hex] [--from <ms>] [--to <ms>]`: one line a frame, `>` host to chip, `<` chip to host: time, sequence, flags, RPC, the inner classes it carries, its length; `--hex` adds the deframed bytes. A frame carrying `d0 81` (a place) is never printed in hex.
//! - `gps-trace <trace> classes`: per direction, every inner class (`c0 c1 <class>`), its count, its sizes and its first and last time.
//! - `gps-trace <trace> msgs <class> [--place]`: every inner message of that class (`8781`, `8281` …), unescaped (`c0 c0` is G#C0), in hex, one a line with its time; `d0 81` only with `--place` (the output then holds a place: keys/ only).
//! - `gps-trace <trace> replay <out> <from ms> <to ms>`: the host's writes in that window as a GNR1 replay for the driver (`[len u16, gap ms u16, bytes]` a record, each record one write, the gap since the write before it).
//! - `gps-trace <trace> fixes`: the chip's on-chip fix messages (`01 81`, id G#16) by time, with the error estimate only (no place).
//!
//! Captures hold Google's patch, Broadcom's LTO and the capture's place: they stay in keys/, as does anything this prints with `--hex` or `--place`.

use std::collections::BTreeMap;

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

/// Where a frame's payload starts (payloads/gps `payload_start`): one byte per set flag bit after the flags, the second flags byte (flag G#80) adding bits 8 and up.
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

fn unhex(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let v = |c: u8| match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("not hex: {s}"),
    };
    // A line cut short (the capture still being written) ends on half a byte: that half is dropped.
    b.chunks_exact(2).map(|p| v(p[0]) << 4 | v(p[1])).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// One frame, deframed: when its last byte came (ms after time zero), the direction (true: host to chip), its bytes.
struct Frame {
    ms: f64,
    host: bool,
    b: Vec<u8>,
}

/// A direction's byte stream cut into frames (`b0 00 … b0 01`, G#B0, G#11, G#13 escaped as `b0 03/04/05`), across reads and writes.
#[derive(Default)]
struct Deframer {
    cur: Option<Vec<u8>>,
    esc: bool,
    prev_b0: bool,
}

impl Deframer {
    fn feed(&mut self, data: &[u8], ms: f64, host: bool, out: &mut Vec<Frame>) {
        for &c in data {
            if self.prev_b0 {
                self.prev_b0 = false;
                match c {
                    0x00 => {
                        self.cur = Some(Vec::new());
                        continue;
                    }
                    0x01 => {
                        if let Some(f) = self.cur.take() {
                            out.push(Frame { ms, host, b: f });
                        }
                        continue;
                    }
                    3 | 4 | 5 => {
                        if let Some(f) = self.cur.as_mut() {
                            f.push(match c { 3 => 0xB0, 4 => 0x11, _ => 0x13 });
                        }
                        continue;
                    }
                    _ => {
                        // A G#B0 followed by anything else: not a frame we know; drop what was open.
                        self.cur = None;
                        continue;
                    }
                }
            }
            if c == 0xB0 {
                self.prev_b0 = true;
                continue;
            }
            if let Some(f) = self.cur.as_mut() {
                f.push(c);
            }
        }
        let _ = self.esc;
    }
}

/// The capture: every frame both ways, and the host's writes (for a replay) with their times.
struct Capture {
    frames: Vec<Frame>,
    writes: Vec<(f64, Vec<u8>)>,
}

fn read_capture(path: &str) -> Capture {
    let text = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let text = String::from_utf8_lossy(&text);
    let mut lines: Vec<(i64, String, String, Vec<u8>)> = Vec::new();
    for l in text.lines() {
        if l.starts_with('#') {
            continue;
        }
        let w: Vec<&str> = l.split(' ').collect();
        if w.len() < 4 {
            continue;
        }
        let Ok(ns) = w[0].parse::<i64>() else { continue };
        let call = w[2].to_string();
        let path = w.iter().find(|x| x.starts_with('/')).map(|s| s.to_string()).unwrap_or_default();
        // A write's bytes follow `n=`; a read's follow `ret=`.
        let data = match call.as_str() {
            "write" => w.iter().position(|x| x.starts_with("n=")).and_then(|i| w.get(i + 1)).map(|h| unhex(h)).unwrap_or_default(),
            "read" => w.iter().position(|x| x.starts_with("ret=")).and_then(|i| w.get(i + 1)).map(|h| unhex(h)).unwrap_or_default(),
            _ => Vec::new(),
        };
        lines.push((ns, call, path, data));
    }
    // Time zero: nSTANDBY's `1`, else the first write to ttyBCM.
    let t0 = lines.iter().find(|(_, c, p, d)| c == "write" && p.contains("nstandby") && d.first() == Some(&b'1')).or_else(|| lines.iter().find(|(_, c, p, _)| c == "write" && p.contains("ttyBCM"))).map(|l| l.0).expect("no write to the chip in the capture");
    let mut cap = Capture { frames: Vec::new(), writes: Vec::new() };
    let (mut host, mut chip) = (Deframer::default(), Deframer::default());
    for (ns, call, path, data) in &lines {
        if !path.contains("ttyBCM") || data.is_empty() {
            continue;
        }
        let ms = (ns - t0) as f64 / 1e6;
        match call.as_str() {
            "write" => {
                host.feed(data, ms, true, &mut cap.frames);
                cap.writes.push((ms, data.clone()));
            }
            "read" => chip.feed(data, ms, false, &mut cap.frames),
            _ => {}
        }
    }
    cap
}

/// A frame's payload (after the flag bytes, before the CRC) and whether its CRC holds.
fn payload(f: &[u8]) -> Option<(&[u8], bool)> {
    if f.len() < 4 {
        return None;
    }
    let ok = crc(&f[1..f.len() - 1]) == f[f.len() - 1];
    let k = payload_start(f);
    Some((&f[k.min(f.len() - 1)..f.len() - 1], ok))
}

/// Every inner message in a payload: from a `c0 c1` (not an escaped `c0 c0` before it) to its `c0 c4`, unescaped. A message that runs past the payload is returned as far as it goes, marked unfinished.
fn inner(p: &[u8]) -> Vec<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < p.len() {
        if p[i] == 0xC0 && p[i + 1] == 0xC1 {
            let mut m = Vec::new();
            let mut j = i + 2;
            let mut done = false;
            while j < p.len() {
                if p[j] == 0xC0 && j + 1 < p.len() {
                    match p[j + 1] {
                        0xC0 => {
                            m.push(0xC0);
                            j += 2;
                            continue;
                        }
                        0xC4 => {
                            done = true;
                            j += 2;
                            break;
                        }
                        _ => {
                            m.push(0xC0);
                            j += 1;
                            continue;
                        }
                    }
                }
                m.push(p[j]);
                j += 1;
            }
            out.push((m, done));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn class(m: &[u8]) -> String {
    if m.len() >= 2 { format!("{:02x}{:02x}", m[0], m[1]) } else { "----".into() }
}

fn rpc(p: &[u8]) -> String {
    match p {
        [a, b, ..] => format!("{a:02x}{b:02x}"),
        [a] => format!("{a:02x}"),
        [] => "----".into(),
    }
}

fn arg_f(args: &[String], k: &str) -> Option<f64> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: gps-trace <trace> frames|classes|msgs <class>|replay <out> <from> <to>|fixes");
        std::process::exit(2);
    }
    if args[1] == "brdc" {
        brdc(&args);
        return;
    }
    let cap = read_capture(&args[1]);
    match args[2].as_str() {
        "frames" => {
            let show_hex = args.iter().any(|a| a == "--hex");
            let from = arg_f(&args, "--from").unwrap_or(f64::MIN);
            let to = arg_f(&args, "--to").unwrap_or(f64::MAX);
            for f in cap.frames.iter().filter(|f| f.ms >= from && f.ms < to) {
                let Some((p, ok)) = payload(&f.b) else { continue };
                let hi = if f.b[2] & 0x80 != 0 { format!(".{:02x}", f.b[3]) } else { String::new() };
                let ms: Vec<String> = inner(p).iter().map(|(m, d)| format!("{}{}", class(m), if *d { "" } else { "+" })).collect();
                let place = p.windows(4).any(|w| w == [0xC0, 0xC1, 0xD0, 0x81]);
                print!("{:10.3} {} seq G#{:02X} fl {:02x}{hi} rpc {} [{}] len A#{}{}", f.ms, if f.host { '>' } else { '<' }, f.b[0], f.b[2], rpc(p), ms.join(" "), f.b.len(), if ok { "" } else { " CRC!" });
                if show_hex {
                    if place {
                        print!(" (a place: not printed)");
                    } else {
                        print!(" {}", hex(&f.b));
                    }
                }
                println!();
            }
        }
        "classes" => {
            // (direction, class) -> (count, min len, max len, first ms, last ms)
            let mut t: BTreeMap<(char, String), (u32, usize, usize, f64, f64)> = BTreeMap::new();
            let (mut bad, mut n) = ([0u32; 2], [0u32; 2]);
            for f in &cap.frames {
                let Some((p, ok)) = payload(&f.b) else { continue };
                n[f.host as usize] += 1;
                if !ok {
                    bad[f.host as usize] += 1;
                }
                for (m, _) in inner(p) {
                    let e = t.entry((if f.host { '>' } else { '<' }, class(&m))).or_insert((0, usize::MAX, 0, f.ms, f.ms));
                    e.0 += 1;
                    e.1 = e.1.min(m.len());
                    e.2 = e.2.max(m.len());
                    e.4 = f.ms;
                }
            }
            println!("frames: host A#{} (A#{} CRC failures), chip A#{} (A#{} CRC failures); host writes A#{}", n[1], bad[1], n[0], bad[0], cap.writes.len());
            println!("dir class  count  bytes (unescaped, class to trailer)  first ms  last ms");
            for ((d, c), (k, lo, hi, t0, t1)) in &t {
                println!("{d} {c} A#{k:6} A#{lo}..A#{hi} {t0:10.3} {t1:10.3}");
            }
        }
        "msgs" => {
            let want = args.get(3).expect("msgs <class>");
            let place_ok = args.iter().any(|a| a == "--place");
            if want == "d081" && !place_ok {
                eprintln!("d0 81 is a place: only with --place, and only into keys/");
                std::process::exit(2);
            }
            for f in &cap.frames {
                let Some((p, _)) = payload(&f.b) else { continue };
                for (m, done) in inner(p) {
                    if &class(&m) == want {
                        println!("{:10.3} {} {}{}", f.ms, if f.host { '>' } else { '<' }, hex(&m), if done { "" } else { " +" });
                    }
                }
            }
        }
        "replay" => {
            let out = args.get(3).expect("replay <out> <from ms> <to ms>");
            let from: f64 = args.get(4).and_then(|v| v.parse().ok()).expect("from ms");
            let to: f64 = args.get(5).and_then(|v| v.parse().ok()).expect("to ms");
            let mut b = b"GNR1".to_vec();
            let mut last = None::<f64>;
            let mut n = 0;
            for (ms, d) in cap.writes.iter().filter(|(ms, _)| *ms >= from && *ms < to) {
                let gap = last.map_or(0., |l| ms - l);
                last = Some(*ms);
                let gap = gap.round() as u64;
                // A replay's gap is a u16 of ms; a longer pause in lhd's writes is not something a replay can say, so it stops the cut.
                assert!(gap <= u16::MAX as u64, "a gap of A#{gap} ms at {ms} ms: cut the window before it");
                assert!(d.len() <= u16::MAX as usize, "a write of A#{} bytes", d.len());
                b.extend_from_slice(&(d.len() as u16).to_le_bytes());
                b.extend_from_slice(&(gap as u16).to_le_bytes());
                b.extend_from_slice(d);
                n += 1;
            }
            std::fs::write(out, &b).unwrap_or_else(|e| panic!("{out}: {e}"));
            println!("{out}: A#{n} records, A#{} bytes", b.len());
        }
        "fixes" => {
            // The fix message (GPS.md "The fix message"): `01 81`, body id G#16 at the tenth byte from the class; the error estimate a u10 at bit 221 of little-endian 32-bit words read MSB first from the class.
            let bits = |m: &[u8], o: usize, n: usize| -> u32 {
                let mut v = 0u32;
                for i in o..o + n {
                    let w = i / 32;
                    let word = u32::from_le_bytes([m[w * 4], m[w * 4 + 1], m[w * 4 + 2], m[w * 4 + 3]]);
                    v = v << 1 | (word >> (31 - i % 32)) & 1;
                }
                v
            };
            let mut n = 0;
            for f in cap.frames.iter().filter(|f| !f.host) {
                let Some((p, _)) = payload(&f.b) else { continue };
                for (m, done) in inner(p) {
                    if done && m.len() > 60 && m[0] == 0x01 && m[1] == 0x81 && m[9] == 0x16 {
                        n += 1;
                        if n <= 4 || n % 64 == 0 {
                            println!("{:10.3} fix A#{n} error A#{} m", f.ms, bits(&m, 221, 10));
                        }
                    }
                }
            }
            println!("fix messages: A#{n}");
        }
        "lto" => {
            // The LTO windows (`82 81`), each whole: a long one starts in one frame and runs on in the next host RPC frames (their headers dropped) until its `c0 c4`; then unescaped (`c0 c0` is G#C0). Printed as little-endian 32-bit words, eight a line, with their offsets from the class.
            let mut cur: Option<Vec<u8>> = None;
            let mut msgs: Vec<(f64, Vec<u8>)> = Vec::new();
            let finish = |raw: &[u8], ms: f64, msgs: &mut Vec<(f64, Vec<u8>)>| {
                let mut m = Vec::new();
                let mut j = 2;
                while j < raw.len() {
                    if raw[j] == 0xC0 && j + 1 < raw.len() {
                        match raw[j + 1] {
                            0xC0 => {
                                m.push(0xC0);
                                j += 2;
                                continue;
                            }
                            0xC4 => break,
                            _ => {}
                        }
                    }
                    m.push(raw[j]);
                    j += 1;
                }
                msgs.push((ms, m));
            };
            for f in cap.frames.iter().filter(|f| f.host) {
                let Some((p, _)) = payload(&f.b) else { continue };
                // A continuation: an RPC `1e` frame while a window is open (its header: `1e`, the chunk's length + 2 in one byte, or two with G#80 set in the first, then the chunk's length u16; a chunk can come in a `1e 80` frame as well as a `1e 82`).
                let h = if p.len() > 1 && p[1] & 0x80 != 0 { 5 } else { 4 };
                if p.len() > h && p[0] == 0x1E && cur.is_some() && !p[h..].starts_with(&[0xC0, 0xC1]) {
                    if let Some(c) = cur.as_mut() {
                        c.extend_from_slice(&p[h..]);
                        if p.ends_with(&[0xC0, 0xC4]) {
                            let raw = cur.take().unwrap();
                            finish(&raw, f.ms, &mut msgs);
                        }
                    }
                    continue;
                }
                if let Some(c) = p.windows(4).position(|w| w == [0xC0, 0xC1, 0x82, 0x81]) {
                    let raw = p[c..].to_vec();
                    if p.ends_with(&[0xC0, 0xC4]) {
                        finish(&raw, f.ms, &mut msgs);
                    } else {
                        cur = Some(raw);
                    }
                }
            }
            for (ms, m) in &msgs {
                let len = u16::from_le_bytes([m[2], m[3]]);
                println!("{ms:10.3} 82 81 length A#{len} (A#{} bytes from the class)", m.len());
                for (k, w) in m.chunks(4).enumerate() {
                    if k % 8 == 0 {
                        print!("  +{:04x}", k * 4);
                    }
                    let v = if w.len() == 4 { u32::from_le_bytes([w[0], w[1], w[2], w[3]]) } else { 0 };
                    print!(" {v:08x}");
                    if k % 8 == 7 {
                        println!();
                    }
                }
                println!();
            }
        }
        other => panic!("unknown: {other}"),
    }
}

/// The frames of a GNR1 replay's record, deframed.
fn rec_frames(rec: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut d = Deframer::default();
    let mut fr = Vec::new();
    d.feed(rec, 0., true, &mut fr);
    for f in fr {
        out.push(f.b);
    }
    out
}

/// A frame escaped for the wire: `b0 00`, the bytes with G#B0, G#11, G#13 as `b0 03/04/05`, `b0 01`.
fn escape_frame(f: &[u8]) -> Vec<u8> {
    let mut o = vec![0xB0, 0x00];
    for &b in f {
        match b {
            0xB0 => o.extend_from_slice(&[0xB0, 0x03]),
            0x11 => o.extend_from_slice(&[0xB0, 0x04]),
            0x13 => o.extend_from_slice(&[0xB0, 0x05]),
            _ => o.push(b),
        }
    }
    o.extend_from_slice(&[0xB0, 0x01]);
    o
}

/// The checksum of an inner message (`87 81`'s rule, here for `82 81`): the 16-bit sum of the class, the length and the body, the checksum's own two bytes and the trailer left out.
fn inner_ck(m: &[u8]) -> u16 {
    m[..4].iter().chain(&m[6..m.len() - 2]).fold(0u16, |a, &b| a.wrapping_add(b as u16))
}

/// `gps-trace brdc <in.gnr> <rinex> <out.gnr> [--keep-glonass]` (docs/GPS.md "The LTO windows are ephemerides"): the replay's `82 81` messages rebuilt with every GPS record's ephemeris and clock taken from the broadcast ephemerides in the RINEX file (the latest set per satellite), in IS-GPS-200's integer scaling, one 32-bit word a field; the GLONASS group dropped unless asked, so a fix that follows is the broadcast ephemerides' alone. The frames from the first `82 81` on are built again: sequences and the reliable counter running on from the frame before, the trailer counters running on from the first `82 81`'s, each message in chunks of at most 1000 bytes as gpsd sent them.
pub fn brdc(args: &[String]) {
    let (inp, rinex, out) = (&args[2], &args[3], &args[4]);
    let keep_glonass = args.iter().any(|a| a == "--keep-glonass");
    let glonass = args.iter().any(|a| a == "--glonass");
    let (mut glo_written, mut glo_kept) = (0, 0);
    let b = std::fs::read(inp).unwrap_or_else(|e| panic!("{inp}: {e}"));
    assert!(b.starts_with(b"GNR1"));
    let mut recs: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut at = 4;
    while at + 4 <= b.len() {
        let len = u16::from_le_bytes([b[at], b[at + 1]]) as usize;
        let gap = u16::from_le_bytes([b[at + 2], b[at + 3]]);
        recs.push((gap, b[at + 4..at + 4 + len].to_vec()));
        at += 4 + len;
    }
    let first = recs.iter().position(|(_, r)| rec_frames(r).iter().any(|f| f.windows(4).any(|w| w == [0xC0, 0xC1, 0x82, 0x81]))).expect("no 82 81 in the replay");
    // The frame before the windows: its sequence and reliable counter go on.
    let prev = rec_frames(&recs[first - 1].1).into_iter().filter(|f| f[2] & 0x02 != 0).last().expect("no reliable frame before the windows");
    let (mut seq, mut counter) = (prev[0], prev[3 + (prev[2] & 1) as usize]);
    // The inner stream of the windows' records: each frame's payload after its RPC header (`1e`, the length + 2 as one byte, or two with G#80 set in the first, then the chunk's length u16).
    let mut stream = Vec::new();
    for (_, r) in &recs[first..] {
        for f in rec_frames(r) {
            let p = &f[payload_start(&f)..f.len() - 1];
            if p.first() != Some(&0x1E) {
                continue;
            }
            let h = if p[1] & 0x80 != 0 { 5 } else { 4 };
            stream.extend_from_slice(&p[h..]);
        }
    }
    let msgs: Vec<Vec<u8>> = inner(&stream).into_iter().filter(|(m, done)| *done && m.starts_with(&[0x82, 0x81])).map(|(m, _)| m).collect();
    for m in &msgs {
        assert_eq!(inner_ck(m), u16::from_le_bytes([m[4], m[5]]), "the checksum rule does not hold for 82 81");
    }
    let mut eph: Vec<ferros_gnss::eph::GpsEph> = Vec::new();
    let text = std::fs::read(rinex).unwrap_or_else(|e| panic!("{rinex}: {e}"));
    let mut glo = Vec::new();
    ferros_gnss::glo::each_glo(&text, |e| glo.push(e));
    ferros_gnss::eph::each_gps(&text, |e| {
        if let Some(i) = eph.iter().position(|x| x.prn == e.prn) {
            if e.week as f64 * 604_800. + e.toe > eph[i].week as f64 * 604_800. + eph[i].toe {
                eph[i] = e;
            }
        } else {
            eph.push(e);
        }
    });
    let pi = std::f64::consts::PI;
    let sc = |x: f64, e: i32| (x * 2f64.powi(-e)).round() as i64 as u32;
    let mut trailer = u16::from_le_bytes([msgs[0][msgs[0].len() - 2], msgs[0][msgs[0].len() - 1]]);
    let (mut written, mut kept, mut dropped) = (0, 0, 0);
    let mut new_msgs = Vec::new();
    for m in &msgs {
        let w = |k: usize| u32::from_le_bytes([m[k * 4], m[k * 4 + 1], m[k * 4 + 2], m[k * 4 + 3]]);
        // The GPS group's header carries a word at +G#4C (G#1207 with a first-of-group bit); the GLONASS group's is 0.
        if w(0x4C / 4) == 0 && !keep_glonass && !glonass {
            dropped += 1;
            continue;
        }
        let mut m = m.clone();
        if w(0x4C / 4) == 0 && glonass {
            // GLONASS: each slot's broadcast vectors carried by the ICD's Runge-Kutta and fitted with IS-GPS-200's ephemeris over toe ± 1 hour (ferros_gnss::glo), toe the slot's latest vector's time; the clock -τn + γn (t - tb), drift γn.
            let n = w(0x68 / 4) as usize;
            for r in 0..n {
                let base = 0xBC + 0xA8 * r;
                let slot = u32::from_le_bytes([m[base], m[base + 1], m[base + 2], m[base + 3]]) as u8;
                let recs: Vec<_> = glo.iter().copied().filter(|e| e.slot == slot && e.health == 0).collect();
                let Some(last) = recs.iter().copied().max_by(|a, b| a.tb.partial_cmp(&b.tb).unwrap()) else {
                    glo_kept += 1;
                    continue;
                };
                let rw = |m: &Vec<u8>, k: usize| u32::from_le_bytes([m[base + 4 * k], m[base + 4 * k + 1], m[base + 4 * k + 2], m[base + 4 * k + 3]]);
                // The window as gpsd sent it, read back the same way, for the comparison below (printed only as distances).
                let old = ferros_gnss::eph::GpsEph { toe: rw(&m, 24) as f64 * 16., sqrt_a: rw(&m, 23) as f64 * 2f64.powi(-19), e: rw(&m, 21) as f64 * 2f64.powi(-33), m0: rw(&m, 19) as i32 as f64 * 2f64.powi(-31) * pi, dn: rw(&m, 18) as i32 as f64 * 2f64.powi(-43) * pi, crs: rw(&m, 17) as i32 as f64 / 32., cuc: rw(&m, 20) as i32 as f64 * 2f64.powi(-29), cus: rw(&m, 22) as i32 as f64 * 2f64.powi(-29), cic: rw(&m, 27) as i32 as f64 * 2f64.powi(-29), omega0: rw(&m, 28) as i32 as f64 * 2f64.powi(-31) * pi, cis: rw(&m, 29) as i32 as f64 * 2f64.powi(-29), i0: rw(&m, 30) as i32 as f64 * 2f64.powi(-31) * pi, crc: rw(&m, 31) as i32 as f64 / 32., omega: rw(&m, 32) as i32 as f64 * 2f64.powi(-31) * pi, omega_dot: rw(&m, 33) as i32 as f64 * 2f64.powi(-43) * pi, idot: rw(&m, 34) as i32 as f64 * 2f64.powi(-43) * pi, ..Default::default() };
                let toe = (last.tb / 16.).round() * 16.;
                let pos = |t: f64| ferros_gnss::glo::orbit_at(&recs, t).unwrap();
                let (e, rms) = ferros_gnss::glo::fit(&pos, last.week, toe, 3600.);
                let d = |a: [f64; 3], b: [f64; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                let at = old.toe;
                println!("R{slot:02}: fit RMS {rms:.1} m; gpsd's window against the broadcast orbit at its toe: {:.0} m", d(ferros_gnss::eph::gps_ecef(&old, at), pos(at)));
                let put = |m: &mut Vec<u8>, k: usize, v: u32| m[base + 4 * k..base + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
                put(&mut m, 13, (toe / 16.) as u32);
                put(&mut m, 14, 0);
                put(&mut m, 15, sc(last.gamma, -43));
                put(&mut m, 16, sc(last.tau + last.gamma * (toe - last.tb), -31));
                put(&mut m, 17, sc(e.crs, -5));
                put(&mut m, 18, sc(e.dn / pi, -43));
                put(&mut m, 19, sc(e.m0 / pi, -31));
                put(&mut m, 20, sc(e.cuc, -29));
                put(&mut m, 21, (e.e * 2f64.powi(33)).round() as u32);
                put(&mut m, 22, sc(e.cus, -29));
                put(&mut m, 23, (e.sqrt_a * 2f64.powi(19)).round() as u32);
                put(&mut m, 24, (toe / 16.) as u32);
                put(&mut m, 27, sc(e.cic, -29));
                put(&mut m, 28, sc(e.omega0 / pi, -31));
                put(&mut m, 29, sc(e.cis, -29));
                put(&mut m, 30, sc(e.i0 / pi, -31));
                put(&mut m, 31, sc(e.crc, -5));
                put(&mut m, 32, sc(e.omega / pi, -31));
                put(&mut m, 33, sc(e.omega_dot / pi, -43));
                put(&mut m, 34, sc(e.idot / pi, -43));
                put(&mut m, 35, (last.week % 1024) as u32);
                glo_written += 1;
            }
        }
        if w(0x4C / 4) != 0 {
            let n = w(0x68 / 4) as usize;
            for r in 0..n {
                let base = 0xBC + 0xA8 * r;
                let prn = u32::from_le_bytes([m[base], m[base + 1], m[base + 2], m[base + 3]]) as u8;
                let Some(e) = eph.iter().find(|e| e.prn == prn) else {
                    kept += 1;
                    continue;
                };
                // gpsd's window against the broadcast set at the window's toe (printed as a distance only).
                let rw = |m: &Vec<u8>, k: usize| u32::from_le_bytes([m[base + 4 * k], m[base + 4 * k + 1], m[base + 4 * k + 2], m[base + 4 * k + 3]]);
                let old = ferros_gnss::eph::GpsEph { toe: rw(&m, 24) as f64 * 16., sqrt_a: rw(&m, 23) as f64 * 2f64.powi(-19), e: rw(&m, 21) as f64 * 2f64.powi(-33), m0: rw(&m, 19) as i32 as f64 * 2f64.powi(-31) * pi, dn: rw(&m, 18) as i32 as f64 * 2f64.powi(-43) * pi, crs: rw(&m, 17) as i32 as f64 / 32., cuc: rw(&m, 20) as i32 as f64 * 2f64.powi(-29), cus: rw(&m, 22) as i32 as f64 * 2f64.powi(-29), cic: rw(&m, 27) as i32 as f64 * 2f64.powi(-29), omega0: rw(&m, 28) as i32 as f64 * 2f64.powi(-31) * pi, cis: rw(&m, 29) as i32 as f64 * 2f64.powi(-29), i0: rw(&m, 30) as i32 as f64 * 2f64.powi(-31) * pi, crc: rw(&m, 31) as i32 as f64 / 32., omega: rw(&m, 32) as i32 as f64 * 2f64.powi(-31) * pi, omega_dot: rw(&m, 33) as i32 as f64 * 2f64.powi(-43) * pi, idot: rw(&m, 34) as i32 as f64 * 2f64.powi(-43) * pi, ..Default::default() };
                let (a, b) = (ferros_gnss::eph::gps_ecef(&old, old.toe), ferros_gnss::eph::gps_ecef(e, old.toe));
                println!("G{prn:02}: gpsd's window against the broadcast set at its toe: {:.0} m", ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt());
                let put = |m: &mut Vec<u8>, k: usize, v: u32| m[base + 4 * k..base + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
                put(&mut m, 12, sc(e.tgd, -31));
                put(&mut m, 13, (e.toc / 16.).round() as u32);
                put(&mut m, 14, sc(e.af2, -55));
                put(&mut m, 15, sc(e.af1, -43));
                put(&mut m, 16, sc(e.af0, -31));
                put(&mut m, 17, sc(e.crs, -5));
                put(&mut m, 18, sc(e.dn / pi, -43));
                put(&mut m, 19, sc(e.m0 / pi, -31));
                put(&mut m, 20, sc(e.cuc, -29));
                put(&mut m, 21, (e.e * 2f64.powi(33)).round() as u32);
                put(&mut m, 22, sc(e.cus, -29));
                put(&mut m, 23, (e.sqrt_a * 2f64.powi(19)).round() as u32);
                put(&mut m, 24, (e.toe / 16.).round() as u32);
                put(&mut m, 27, sc(e.cic, -29));
                put(&mut m, 28, sc(e.omega0 / pi, -31));
                put(&mut m, 29, sc(e.cis, -29));
                put(&mut m, 30, sc(e.i0 / pi, -31));
                put(&mut m, 31, sc(e.crc, -5));
                put(&mut m, 32, sc(e.omega / pi, -31));
                put(&mut m, 33, sc(e.omega_dot / pi, -43));
                put(&mut m, 34, sc(e.idot / pi, -43));
                put(&mut m, 35, (e.week % 1024) as u32);
                written += 1;
            }
        }
        let l = m.len();
        m[l - 2..].copy_from_slice(&trailer.to_le_bytes());
        trailer = trailer.wrapping_add(1);
        let ck = inner_ck(&m);
        m[4..6].copy_from_slice(&ck.to_le_bytes());
        new_msgs.push(m);
    }
    // The frames again: each message escaped (G#C0 as `c0 c0`) inside `c0 c1 … c0 c4`, in chunks of at most 1000 bytes.
    let mut new_recs: Vec<(u16, Vec<u8>)> = recs[..first].to_vec();
    let mut gap = recs[first].0;
    for m in &new_msgs {
        let mut e = vec![0xC0, 0xC1];
        for &x in m {
            if x == 0xC0 {
                e.extend_from_slice(&[0xC0, 0xC0]);
            } else {
                e.push(x);
            }
        }
        e.extend_from_slice(&[0xC0, 0xC4]);
        for chunk in e.chunks(1000) {
            let n = chunk.len();
            let mut p = vec![0x1E];
            if n + 2 >= 0x80 {
                p.push(0x80 | ((n + 2) >> 8) as u8);
            }
            p.push((n + 2) as u8);
            p.extend_from_slice(&(n as u16).to_le_bytes());
            p.extend_from_slice(chunk);
            seq = seq.wrapping_add(1);
            counter = counter.wrapping_add(1);
            let big = p.len() >= 256;
            let mut f = vec![seq, p.len() as u8, if big { 0x42 } else { 0x02 }, counter];
            if big {
                f.push((p.len() >> 8) as u8);
            }
            f.extend_from_slice(&p);
            let c = crc(&f[1..]);
            f.push(c);
            new_recs.push((gap, escape_frame(&f)));
            gap = 4;
        }
    }
    let mut o = b"GNR1".to_vec();
    for (g, r) in &new_recs {
        o.extend_from_slice(&(r.len() as u16).to_le_bytes());
        o.extend_from_slice(&g.to_le_bytes());
        o.extend_from_slice(r);
    }
    std::fs::write(out, &o).unwrap_or_else(|e| panic!("{out}: {e}"));
    println!("{out}: A#{} 82 81 messages ({dropped} GLONASS dropped), A#{written} GPS records from the broadcast ephemerides, A#{kept} kept as they were (no broadcast set), A#{glo_written} GLONASS records fitted to the broadcast vectors (A#{glo_kept} with no healthy vector kept as they were); A#{} records, A#{} bytes", new_msgs.len(), new_recs.len(), o.len());
}
