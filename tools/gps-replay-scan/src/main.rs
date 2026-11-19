//! gps-replay-scan: what a GPS replay (the vault's `gps.replay`, `gps.replay.sky`; payloads/gps, docs/GPS.md) sends the BCM4776, message by message, without a byte of what the messages say.
//!
//! A replay is `GNR1` then records `[len u16, gap ms u16, bytes]`, each record the host's bytes as lhd wrote them: packet frames `b0 00 … b0 01` (G#B0, G#11, G#13 escaped as `b0 03/04/05`), each `[sequence, length low, flags, a byte per set flag bit, payload, CRC-8]`.
//! The payload's first two bytes are the RPC (`1e 83`, `05 83`, `02 80` …); an inner `c0 c1 <class> <len u16> <checksum u16> … c0 c4` frame names the message class (`87 81` time, `d0 81` the position aiding, `82 81` LTO windows).
//!
//! Prints, per record: its index, when it goes (the gaps summed, ms), its length, then each frame's sequence, flags, RPC and class; then a table of the kinds with their count, bytes and first and last time.
//! Never prints a payload byte past the RPC and class: the position aiding is a place, and the patch is Broadcom's.
//!
//! Usage: gps-replay-scan <replay> [--records]

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

/// Where a frame's payload starts (payloads/gps `payload_start`).
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

/// The frames in one record, deframed.
fn frames(rec: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < rec.len() {
        if rec[i] != 0xB0 || rec[i + 1] != 0x00 {
            i += 1;
            continue;
        }
        let Some(e) = rec[i + 2..].windows(2).position(|w| w == [0xB0, 0x01]).map(|e| e + i + 2) else {
            break;
        };
        let mut f = Vec::new();
        let mut j = i + 2;
        while j < e {
            if rec[j] == 0xB0 && j + 1 < e {
                f.push(match rec[j + 1] {
                    3 => 0xB0,
                    4 => 0x11,
                    5 => 0x13,
                    _ => 0,
                });
                j += 2;
            } else {
                f.push(rec[j]);
                j += 1;
            }
        }
        out.push(f);
        i = e + 2;
    }
    out
}

/// A frame's kind: flags (and the second flags byte), the RPC, the inner class; and whether its CRC holds.
fn kind(f: &[u8]) -> Option<(String, bool, bool)> {
    if f.len() < 4 {
        return None;
    }
    let ok = crc(&f[1..f.len() - 1]) == f[f.len() - 1];
    let k = payload_start(f);
    let p = &f[k..f.len() - 1];
    let hi = if f[2] & 0x80 != 0 { f[3] } else { 0 };
    let rpc = match p {
        [a, b, ..] => format!("{a:02x}{b:02x}"),
        [a] => format!("{a:02x}"),
        [] => "----".into(),
    };
    let cls = p.iter().take(8).zip(p.iter().skip(1)).position(|(&a, &b)| a == 0xC0 && b == 0xC1).and_then(|c| Some(format!("{:02x}{:02x}", p.get(c + 2)?, p.get(c + 3)?))).unwrap_or_else(|| "----".into());
    let fl = if hi != 0 { format!("{:02x}.{:02x}", f[2], hi) } else { format!("{:02x}", f[2]) };
    Some((format!("{fl}/{rpc}/{cls}"), ok, f[2] & 0x02 != 0))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: gps-replay-scan <replay> [--records]");
        std::process::exit(2);
    };
    let per_record = args.iter().any(|a| a == "--records");
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(b.starts_with(b"GNR1"), "{path}: not GNR1");
    let (mut at, mut t, mut n) = (4usize, 0u64, 0usize);
    // kind -> (count, bytes, first ms, last ms, first record)
    let mut kinds: BTreeMap<String, (u32, usize, u64, u64, usize)> = BTreeMap::new();
    let (mut bad, mut reliable) = (0u32, 0u32);
    while at + 4 <= b.len() {
        let len = u16::from_le_bytes([b[at], b[at + 1]]) as usize;
        let gap = u16::from_le_bytes([b[at + 2], b[at + 3]]) as u64;
        let rec = &b[at + 4..at + 4 + len];
        at += 4 + len;
        t += gap;
        let fs = frames(rec);
        let mut line = format!("rec A#{n:4} t A#{t:6} ms gap A#{gap:5} len A#{len:5}:");
        for f in &fs {
            if let Some((k, ok, rel)) = kind(f) {
                if !ok {
                    bad += 1;
                }
                if rel {
                    reliable += 1;
                }
                line += &format!(" [seq G#{:02X} {k}{}]", f[0], if ok { "" } else { " CRC!" });
                let e = kinds.entry(k).or_insert((0, 0, t, t, n));
                e.0 += 1;
                e.1 += f.len();
                e.3 = t;
            }
        }
        if fs.is_empty() {
            // Not packet frames: lhd's autobaud bursts (G#80) and the like; the first bytes say which.
            line += &format!(" raw {}", rec.iter().take(8).map(|x| format!("{x:02x}")).collect::<String>());
        }
        if per_record {
            println!("{line}");
        }
        // `--hex <record>`: that record's frames deframed, in hex, refused for a record carrying the position aiding (`d0 81`: a place).
        if let Some(i) = args.iter().position(|a| a == "--hex") {
            if args.get(i + 1).and_then(|v| v.parse::<usize>().ok()) == Some(n) {
                for f in &fs {
                    if f.windows(4).any(|w| w == [0xC0, 0xC1, 0xD0, 0x81]) {
                        println!("  a d0 81 frame: not printed (a place)");
                        continue;
                    }
                    println!("  {}", f.iter().map(|x| format!("{x:02x}")).collect::<String>());
                }
            }
        }
        n += 1;
    }
    // The reliable frames' counter (the byte flag G#02 brings: +1 for every reliable host frame, 2026-11-18): the chip refuses a frame whose counter is not the last one it took plus one (its reply's flag G#08 instead of G#04, the counter it holds beside it), and every one after. A replay with messages cut and the sequences closed up but not this counter is refused from the cut on.
    let (mut at3, mut n3, mut last): (usize, usize, Option<u8>) = (4, 0, None);
    let mut jumps = 0u32;
    while at3 + 4 <= b.len() {
        let len = u16::from_le_bytes([b[at3], b[at3 + 1]]) as usize;
        let rec = &b[at3 + 4..at3 + 4 + len];
        at3 += 4 + len;
        for f in frames(rec) {
            if f.len() >= 5 && f[2] & 0x02 != 0 {
                let k = 3 + (f[2] & 0x01) as usize;
                let c = f[k];
                if let Some(l) = last {
                    if c != l.wrapping_add(1) {
                        jumps += 1;
                        println!("reliable counter: rec A#{n3} seq G#{:02X} counter G#{c:02X} after G#{l:02X} (a jump of A#{})", f[0], c.wrapping_sub(l) as i8);
                    }
                }
                last = Some(c);
            }
        }
        n3 += 1;
    }
    println!("reliable counter: A#{jumps} jumps");
    // Every inner message, wherever it starts in a frame (a long RPC carries several, and a message can begin past a frame's first bytes): `c0 c1` not itself an escaped `c0 c0 c1`, then its class.
    let mut inner: BTreeMap<String, (u32, u64, usize)> = BTreeMap::new();
    let (mut at2, mut t2, mut n2) = (4usize, 0u64, 0usize);
    while at2 + 4 <= b.len() {
        let len = u16::from_le_bytes([b[at2], b[at2 + 1]]) as usize;
        t2 += u16::from_le_bytes([b[at2 + 2], b[at2 + 3]]) as u64;
        let rec = &b[at2 + 4..at2 + 4 + len];
        at2 += 4 + len;
        for f in frames(rec) {
            for i in 0..f.len().saturating_sub(4) {
                if f[i] == 0xC0 && f[i + 1] == 0xC1 && (i == 0 || f[i - 1] != 0xC0) {
                    let e = inner.entry(format!("{:02x}{:02x}", f[i + 2], f[i + 3])).or_insert((0, t2, n2));
                    e.0 += 1;
                }
            }
        }
        n2 += 1;
    }
    println!("inner messages (c0 c1 <class>) anywhere in the frames: class, count, first ms, first record");
    for (k, (c, t0, r0)) in &inner {
        println!("  {k} A#{c:4} A#{t0:6} A#{r0:4}");
    }
    // What the driver needs from a replay (GPS.md "No fix, 2026-11-18"): the time it rewrites, the place it rewrites, and the on-chip session's setup; the LTO windows are optional.
    let has = |c: &str| inner.get(c).map_or(0, |e| e.0);
    println!("aiding: time (87 81) A#{}, place (d0 81) A#{}, LTO windows (82 81) A#{}, configuration strings (10 00, 17 00) A#{} A#{}, 09 00 (the on-chip capture's last) A#{}", has("8781"), has("d081"), has("8281"), has("1000"), has("1700"), has("0900"));
    if has("d081") == 0 {
        println!("  no d0 81: the driver can give the chip no place (GPS.md: the chip then searches 5-8 minutes, or never indoors)");
    }
    if has("8781") == 0 || has("0900") == 0 {
        println!("  no 87 81 or no 09 00: not the on-chip session (the chip streams 02 80 measurements to a host engine that is not there, and stops)");
    }
    println!("{path}: A#{n} records over A#{t} ms of lhd's gaps, A#{reliable} reliable frames, A#{bad} CRC failures");
    println!("kind (flags/rpc/class)      count     bytes  first ms   last ms  first record");
    for (k, (c, by, t0, t1, r0)) in &kinds {
        println!("{k:24} A#{c:5} A#{by:7} A#{t0:6} A#{t1:6} A#{r0:4}");
    }
}
