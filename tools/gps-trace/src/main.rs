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
    b.chunks(2).map(|p| v(p[0]) << 4 | v(p[1])).collect()
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
        other => panic!("unknown: {other}"),
    }
}
