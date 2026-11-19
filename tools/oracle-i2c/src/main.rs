//! oracle-i2c: an hv oracle capture of an Exynos USI HS-I2C controller (auto mode) turned back into I2C transfers, as tools/oracle-decode-hsi2c.py does, in Rust (Python is not allowed here); and with `--holds`, one sensor's CCS writes read out of them, each grouped-parameter hold on one line.
//!
//! Input: the flushed console (console-ramoops-0), or its `ferros hv: S2 <pa> <R|W><bytes> <value>` lines (HV.md, the register oracle); `ferros hv: S2 rep <N>` repeats the access before it N more times; `ferros-mark: ...` lines written from Android pass thru as `# MARK ...`.
//! Register map as ferros_kernel hsi2c.rs has it: TX_DATA G#34, RX_DATA G#38, AUTO_CONF G#44 (length bits 0-15, READ bit 16, RUN bit 31), ADDR G#70 (the 7-bit target at bits 10-16).
//!
//! Usage: oracle-i2c <capture.log> [--base G#1088_0000] [--max-bytes N] [--holds G#3D] [--flat] [--table] [--init <out>]
//! - Transfers (the default): `G#3D W 02 02 53 5E`, one a line; a transfer longer than `--max-bytes` as its first four bytes, its length and its blake3 (a sensor's settings upload is vendor firmware: its size and identity stay checkable, its bytes stay out of any file meant for the repository).
//! - `--holds T`: target T's writes as 16-bit CCS register and data: every hold (G#0104 = 1 ... G#0104 = 0) on one line, `hold: 0702=0000 0704=0000 0202=535E 0340=5372`; every write outside a hold on its own line (stream on and off, mode changes); Samsung's indirect window (G#6028, G#602A, G#6F12) said as the page address it writes; reads as `R`. What a long exposure writes is then read straight off it (docs/CAMERA.md "Long exposures").
//! - `--flat` (with `--holds T`): every write to T one 16-bit word a line at its resolved address, `2001_8D3E=000C` (the page G#FCFC selects, or the indirect window), so two inits diff with sort and join: docs/CAMERA.md "The slit for real" found the side channels this way.
//! - `--table`: the input is an init table (`cam.<key>.init`, or the init inside a `cam.frame` bundle), not a capture, read as transfers to `--holds T`.
//! - `--init <out>` (with `--holds T`): T's transfers up to its first stream on written as an init table the camera driver replays (W, R and D records, ferros_cam inputs), as tools/camera-init.py made them; refused when the capture has no stream on or a transfer before it lost bytes. The main's 1:1 init (`cam.boitata.init.full`) came from the 16 s capture this way (docs/CAMERA.md "1:1 on the main").
//! Integrity on stderr: transfers whose byte count is not the length their AUTO_CONF run was programmed with (lines lost from the console ring: never replay such a transfer; memory feedback_oracle_capture_integrity).

use std::fmt::Write as _;

/// One trapped access, a marker, in the log's order.
enum Access {
    Reg { pa: u64, write: bool, val: u64 },
    Mark(String),
}

/// A transfer: its target, read or write, the bytes, and the length its run was programmed with.
struct Xfer {
    target: u8,
    read: bool,
    data: Vec<u8>,
    expect: Option<usize>,
}

enum Item {
    Xfer(Xfer),
    Mark(String),
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim().trim_start_matches("G#").trim_start_matches("0x").replace('_', "").as_str(), 16).ok()
}

/// The log's accesses with every `rep` run spelled out.
fn accesses(text: &str) -> Vec<Access> {
    let mut out = Vec::new();
    let mut prev: Option<(u64, bool, u64)> = None;
    for line in text.lines() {
        if let Some(k) = line.find("ferros-mark: ") {
            out.push(Access::Mark(line[k + 13..].trim().to_string()));
            continue;
        }
        let Some(k) = line.find("ferros hv: S2 ") else { continue };
        let rest: Vec<&str> = line[k + 14..].split_whitespace().collect();
        if rest.first() == Some(&"rep") {
            if let (Some(n), Some((pa, write, val))) = (rest.get(1).and_then(|s| hex(s)), prev) {
                for _ in 0..n {
                    out.push(Access::Reg { pa, write, val });
                }
            }
            continue;
        }
        if rest.len() < 3 {
            continue;
        }
        let (Some(pa), Some(val)) = (hex(rest[0]), hex(rest[2])) else { continue };
        let write = rest[1].starts_with('W');
        prev = Some((pa, write, val));
        out.push(Access::Reg { pa, write, val });
    }
    out
}

/// The controller at `base`'s accesses as transfers, markers kept in place.
fn transfers(acc: &[Access], base: u64) -> Vec<Item> {
    let mut items = Vec::new();
    let mut target: Option<u8> = None;
    let mut cur: Option<Xfer> = None;
    let flush = |cur: &mut Option<Xfer>, items: &mut Vec<Item>| {
        if let Some(x) = cur.take() {
            items.push(Item::Xfer(x));
        }
    };
    for a in acc {
        match a {
            Access::Mark(m) => {
                flush(&mut cur, &mut items);
                items.push(Item::Mark(m.clone()));
            }
            &Access::Reg { pa, write, val } => {
                if pa & !0xFFF != base {
                    continue;
                }
                match (pa & 0xFFF, write) {
                    (0x70, true) => {
                        flush(&mut cur, &mut items);
                        target = Some((val >> 10 & 0x7F) as u8);
                    }
                    (0x44, true) if val >> 31 & 1 == 1 => {
                        flush(&mut cur, &mut items);
                        if let Some(t) = target {
                            cur = Some(Xfer { target: t, read: val >> 16 & 1 == 1, data: Vec::new(), expect: Some((val & 0xFFFF) as usize) });
                        }
                    }
                    (0x34, true) => {
                        if let Some(x) = cur.as_mut().filter(|x| !x.read) {
                            x.data.push(val as u8);
                        }
                    }
                    (0x38, false) => {
                        if let Some(x) = cur.as_mut().filter(|x| x.read) {
                            x.data.push(val as u8);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    flush(&mut cur, &mut items);
    items
}

fn bytes(d: &[u8], max: Option<usize>) -> String {
    let mut s = String::new();
    match max {
        Some(m) if d.len() > m => {
            for b in &d[..4] {
                let _ = write!(s, "{b:02X} ");
            }
            let h = blake3::hash(d);
            let _ = write!(s, "...[{} bytes, blake3 {}]", d.len(), &h.to_hex()[..16]);
        }
        _ => {
            for (k, b) in d.iter().enumerate() {
                let _ = write!(s, "{}{b:02X}", if k == 0 { "" } else { " " });
            }
        }
    }
    s
}

/// An init table (`cam.<key>.init`, or the init inside a 'CFRM' bundle: ferros_cam inputs) as transfers to `target`, so a captured init and a table read the same way.
/// Records: `W` len (u16 BE) and the bytes; `R` count (u16 BE) and a two-byte register; `D` microseconds (u32 BE); a table that stops parsing panics at the byte (never a guess).
fn table(raw: &[u8], target: u8) -> Vec<Item> {
    let t = if raw.len() >= 8 && &raw[..4] == b"CFRM" { &raw[8..8 + u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]) as usize] } else { raw };
    let mut items = Vec::new();
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            b'W' => {
                let n = u16::from_be_bytes([t[i + 1], t[i + 2]]) as usize;
                items.push(Item::Xfer(Xfer { target, read: false, data: t[i + 3..i + 3 + n].to_vec(), expect: Some(n) }));
                i += 3 + n;
            }
            b'R' => {
                items.push(Item::Xfer(Xfer { target, read: false, data: t[i + 3..i + 5].to_vec(), expect: Some(2) }));
                i += 5;
            }
            b'D' => i += 5,
            _ => panic!("the table stops parsing at byte {i}"),
        }
    }
    items
}

/// One sensor's writes with every address resolved, one 16-bit word a line in order (`2001_8D3E=000C`), so two inits diff with sort and join: a direct write lands at the page G#FCFC selects (G#4000 until one is selected), a write thru the indirect window (G#6028, G#602A, G#6F12) at the window's address, and a run of data bytes advances the address by two a word (one byte alone stays a byte, `4000_0104=01`).
/// An upload longer than `max` is one line, its first word's address, its length and its blake3, as the default output says it.
fn flat(items: &[Item], target: u8, max: Option<usize>) -> Vec<String> {
    let mut out = Vec::new();
    let (mut page, mut hi, mut lo) = (0x4000u16, 0u16, 0u16);
    for it in items {
        let x = match it {
            Item::Mark(m) => {
                out.push(format!("# MARK {m}"));
                continue;
            }
            Item::Xfer(x) if x.target == target && !x.read && x.data.len() >= 3 => x,
            Item::Xfer(_) => continue,
        };
        let a = u16::from_be_bytes([x.data[0], x.data[1]]);
        let d = &x.data[2..];
        let v = if d.len() >= 2 { u16::from_be_bytes([d[0], d[1]]) } else { d[0] as u16 };
        let at: u32 = match a {
            0xFCFC => {
                page = v;
                continue;
            }
            0x6028 => {
                hi = v;
                continue;
            }
            0x602A => {
                lo = v;
                continue;
            }
            0x6F12 => {
                let at = (hi as u32) << 16 | lo as u32;
                lo = lo.wrapping_add(d.len() as u16);
                at
            }
            _ => (page as u32) << 16 | a as u32,
        };
        if max.is_some_and(|m| d.len() > m) {
            out.push(format!("{:04X}_{:04X}=[{} bytes, blake3 {}]", at >> 16, at & 0xFFFF, d.len(), &blake3::hash(d).to_hex()[..16]));
            continue;
        }
        if d.len() == 1 {
            out.push(format!("{:04X}_{:04X}={:02X}", at >> 16, at & 0xFFFF, d[0]));
            continue;
        }
        for (k, w) in d.chunks(2).enumerate() {
            let p = at + 2 * k as u32;
            let s = if w.len() == 2 { format!("{:04X}", u16::from_be_bytes([w[0], w[1]])) } else { format!("{:02X}", w[0]) };
            out.push(format!("{:04X}_{:04X}={s}", p >> 16, p & 0xFFFF));
        }
    }
    out
}

/// Target `target`'s transfers up to its first stream on (a write of G#0100 on the CCS page with a nonzero mode byte, CCS mode_select), not including it, as an init table the camera driver replays (ferros_cam inputs `records`): what tools/camera-init.py made, in Rust.
/// A two-byte write followed by a read is one `R` record (the register and the count read); a read alone is an `R` of register 0.
/// The capture has no times, so the delays are the main's binned table's, which has run since 2026-10-19: 10 ms after each of the first three transfers, 5 ms after a write past 256 bytes (the settings upload), 1 ms after every other write.
/// Refused (None) when the capture holds no stream on, or a transfer whose byte count is not its programmed length (feedback_oracle_capture_integrity: never replay one).
fn init_table(items: &[Item], target: u8) -> Option<(Vec<u8>, usize)> {
    let xs: Vec<&Xfer> = items.iter().filter_map(|it| if let Item::Xfer(x) = it { (x.target == target).then_some(x) } else { None }).collect();
    // The stream on is G#0100 on the CCS page: a Samsung part's G#0100 on another page (G#FCFC = G#4001 selects one, the main's init writes G#0100 = G#FFFF there) is not mode_select.
    let mut page = 0x4000u16;
    let end = xs.iter().position(|x| {
        if x.read || x.data.len() < 3 {
            return false;
        }
        if x.data[..2] == [0xFC, 0xFC] && x.data.len() >= 4 {
            page = u16::from_be_bytes([x.data[2], x.data[3]]);
        }
        page == 0x4000 && x.data[..2] == [0x01, 0x00] && x.data[2] != 0
    })?;
    if xs[..end].iter().any(|x| x.expect.is_some_and(|e| e != x.data.len())) {
        return None;
    }
    let mut t = Vec::new();
    let (mut i, mut n) = (0, 0);
    while i < end {
        let x = xs[i];
        if !x.read && x.data.len() == 2 && i + 1 < end && xs[i + 1].read {
            t.push(b'R');
            t.extend_from_slice(&(xs[i + 1].data.len() as u16).to_be_bytes());
            t.extend_from_slice(&x.data);
            i += 2;
        } else if !x.read {
            t.push(b'W');
            t.extend_from_slice(&(x.data.len() as u16).to_be_bytes());
            t.extend_from_slice(&x.data);
            let us: u32 = if n < 3 { 10_000 } else if x.data.len() > 256 { 5_000 } else { 1_000 };
            t.push(b'D');
            t.extend_from_slice(&us.to_be_bytes());
            i += 1;
        } else {
            t.push(b'R');
            t.extend_from_slice(&(x.data.len() as u16).to_be_bytes());
            t.extend_from_slice(&[0, 0]);
            i += 1;
        }
        n += 1;
    }
    Some((t, n))
}

/// One sensor's writes as CCS registers: holds on one line each, the rest a line each.
fn holds(items: &[Item], target: u8, max: Option<usize>) -> Vec<String> {
    let mut out = Vec::new();
    let (mut hold, mut inside) = (String::new(), false);
    let (mut win_hi, mut win_lo) = (0u16, 0u16);
    for it in items {
        let x = match it {
            Item::Mark(m) => {
                out.push(format!("# MARK {m}"));
                continue;
            }
            Item::Xfer(x) if x.target == target => x,
            Item::Xfer(_) => continue,
        };
        if x.read {
            out.push(format!("R {}", bytes(&x.data, max)));
            continue;
        }
        if x.data.len() < 3 {
            out.push(format!("W {}", bytes(&x.data, max)));
            continue;
        }
        let a = u16::from_be_bytes([x.data[0], x.data[1]]);
        let d = &x.data[2..];
        let v = if d.len() >= 2 { u16::from_be_bytes([d[0], d[1]]) as u32 } else { d[0] as u32 };
        let text = match a {
            0x6028 => {
                win_hi = v as u16;
                format!("6028={v:04X}")
            }
            0x602A => {
                win_lo = v as u16;
                format!("602A={v:04X}")
            }
            0x6F12 => {
                let at = (win_hi as u32) << 16 | win_lo as u32;
                win_lo = win_lo.wrapping_add(d.len() as u16);
                format!("[{at:08X}]={}", bytes(d, max).replace(' ', ""))
            }
            _ => format!("{a:04X}={}", bytes(d, max).replace(' ', "")),
        };
        if a == 0x0104 && d[0] == 1 {
            if inside {
                out.push(format!("hold (no release): {hold}"));
            }
            (hold, inside) = (String::new(), true);
            continue;
        }
        if a == 0x0104 && d[0] == 0 && inside {
            out.push(format!("hold: {hold}"));
            (hold, inside) = (String::new(), false);
            continue;
        }
        if inside {
            if !hold.is_empty() {
                hold.push(' ');
            }
            hold.push_str(&text);
        } else {
            out.push(format!("W {text}"));
        }
    }
    if inside {
        out.push(format!("hold (open at the end): {hold}"));
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut path, mut base, mut max, mut focus) = (None, 0x1088_0000u64, None, None);
    let (mut flat_out, mut is_table) = (false, false);
    let mut init_out: Option<String> = None;
    let mut k = 0;
    while k < args.len() {
        match args[k].as_str() {
            "--init" => {
                init_out = Some(args.get(k + 1).expect("--init <out>").clone());
                k += 2;
            }
            "--flat" => {
                flat_out = true;
                k += 1;
            }
            "--table" => {
                is_table = true;
                k += 1;
            }
            "--base" => {
                base = args.get(k + 1).and_then(|s| hex(s)).expect("--base G#<controller>");
                k += 2;
            }
            "--max-bytes" => {
                max = Some(args.get(k + 1).and_then(|s| s.parse().ok()).expect("--max-bytes N"));
                k += 2;
            }
            "--holds" => {
                focus = Some(args.get(k + 1).and_then(|s| hex(s)).expect("--holds G#<7-bit target>") as u8);
                k += 2;
            }
            p => {
                path = Some(p.to_string());
                k += 1;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: oracle-i2c <capture.log or init table> [--base G#1088_0000] [--max-bytes N] [--holds G#3D] [--flat] [--table] [--init <out>]");
        std::process::exit(2);
    };
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let items = if is_table { table(&raw, focus.expect("--table needs --holds G#<target> or --flat with --holds")) } else { transfers(&accesses(&String::from_utf8_lossy(&raw)), base) };
    if let Some(out) = init_out {
        let t = focus.expect("--init needs --holds G#<target>");
        let Some((table, n)) = init_table(&items, t) else {
            eprintln!("# no init: no stream on to G#{t:02X} in the capture, or a transfer before it whose byte count is not its programmed length -- DO NOT REPLAY");
            std::process::exit(1);
        };
        std::fs::write(&out, &table).unwrap_or_else(|e| panic!("{out}: {e}"));
        eprintln!("# {n} transfers before the stream on -> {} bytes in {out}, blake3 {}", table.len(), blake3::hash(&table).to_hex());
        return;
    }
    let mut bad = 0usize;
    let mut targets: Vec<u8> = Vec::new();
    let mut n = 0usize;
    for it in &items {
        if let Item::Xfer(x) = it {
            n += 1;
            if x.expect.is_some_and(|e| e != x.data.len()) {
                bad += 1;
            }
            if !targets.contains(&x.target) {
                targets.push(x.target);
            }
        }
    }
    let lines = match focus {
        Some(t) if flat_out => flat(&items, t, max),
        Some(t) => holds(&items, t, max),
        None => items
            .iter()
            .map(|it| match it {
                Item::Mark(m) => format!("# MARK {m}"),
                Item::Xfer(x) => {
                    let mut l = format!("G#{:02X} {} {}", x.target, if x.read { 'R' } else { 'W' }, bytes(&x.data, max));
                    if let Some(e) = x.expect.filter(|&e| e != x.data.len()) {
                        let _ = write!(l, "   # LENGTH MISMATCH: programmed {e}, captured {}", x.data.len());
                    }
                    l
                }
            })
            .collect(),
    };
    for l in lines {
        println!("{l}");
    }
    targets.sort();
    let names: Vec<String> = targets.iter().map(|t| format!("G#{t:02X}")).collect();
    eprintln!("# integrity: {bad} transfer(s) whose byte count differs from the programmed length{}", if bad == 0 { "" } else { " -- DO NOT REPLAY" });
    eprintln!("# {n} transfers; targets: {}", names.join(", "));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hold of two writes and a stream on, as the hv logs them: ADDR, AUTO_CONF with RUN, the TX bytes; a `rep` repeating a TX byte.
    #[test]
    fn decodes_holds() {
        let mut log = String::new();
        let mut xfer = |log: &mut String, bytes: &[u8]| {
            let _ = writeln!(log, "ferros hv: S2 0000000010880070 W4 000000000000F400");
            let _ = writeln!(log, "ferros hv: S2 0000000010880044 W4 {:016X}", 0x8002_0000u64 | bytes.len() as u64);
            let mut k = 0;
            while k < bytes.len() {
                let _ = writeln!(log, "ferros hv: S2 0000000010880034 W4 {:016X}", bytes[k]);
                let mut r = 0;
                while k + 1 + r < bytes.len() && bytes[k + 1 + r] == bytes[k] {
                    r += 1;
                }
                if r > 0 {
                    let _ = writeln!(log, "ferros hv: S2 rep {r:016X}");
                }
                k += 1 + r;
            }
        };
        let _ = writeln!(log, "[ 1.0] ferros-mark: sixteen seconds");
        xfer(&mut log, &[0x01, 0x04, 0x01, 0x01]);
        xfer(&mut log, &[0x07, 0x02, 0x07, 0x00]);
        xfer(&mut log, &[0x02, 0x02, 0x00, 0x00]);
        xfer(&mut log, &[0x01, 0x04, 0x00, 0x01]);
        xfer(&mut log, &[0x01, 0x00, 0x01, 0x03]);
        let items = transfers(&accesses(&log), 0x1088_0000);
        let h = holds(&items, 0x3D, None);
        assert_eq!(h, vec!["# MARK sixteen seconds", "hold: 0702=0700 0202=0000", "W 0100=0103"]);
        assert!(items.iter().all(|i| !matches!(i, Item::Xfer(x) if x.expect != Some(x.data.len()))));
    }

    /// An init table from transfers: the identity read as one R record, writes each with its delay (10 ms for the first three), stopped at the stream on; a transfer that lost bytes, or no stream on, refuses.
    #[test]
    fn an_init_table() {
        let w = |d: &[u8]| Item::Xfer(Xfer { target: 0x3D, read: false, data: d.to_vec(), expect: Some(d.len()) });
        let r = |d: &[u8]| Item::Xfer(Xfer { target: 0x3D, read: true, data: d.to_vec(), expect: Some(d.len()) });
        let mut items = vec![w(&[0, 0]), r(&[8, 0xEF]), w(&[0xFC, 0xFC, 0x40, 0x00]), w(&[1, 4, 1, 1]), w(&[2, 2, 0x11, 0xDA]), w(&[1, 0, 1, 3]), w(&[1, 4, 0, 1])];
        let (t, n) = init_table(&items, 0x3D).unwrap();
        assert_eq!(n, 4);
        let mut want = vec![b'R', 0, 2, 0, 0];
        for (d, us) in [(&[0xFCu8, 0xFC, 0x40, 0x00][..], 10_000u32), (&[1, 4, 1, 1], 10_000), (&[2, 2, 0x11, 0xDA], 1_000)] {
            want.push(b'W');
            want.extend_from_slice(&(d.len() as u16).to_be_bytes());
            want.extend_from_slice(d);
            want.push(b'D');
            want.extend_from_slice(&us.to_be_bytes());
        }
        assert_eq!(t, want);
        // The table reads back as the transfers it came from.
        let back = flat(&table(&t, 0x3D), 0x3D, None);
        assert_eq!(back, vec!["4000_0104=0101", "4000_0202=11DA"]);
        items[4] = Item::Xfer(Xfer { target: 0x3D, read: false, data: vec![2, 2, 0x11], expect: Some(4) });
        assert!(init_table(&items, 0x3D).is_none());
        items.truncate(5);
        items[4] = w(&[2, 2, 0x11, 0xDA]);
        assert!(init_table(&items, 0x3D).is_none());
    }

    /// An init table read flat: a direct write on the CCS page until G#FCFC moves it, the page after, the indirect window's words at its address in two-byte steps, a byte write alone.
    #[test]
    fn a_table_reads_flat() {
        let mut t = Vec::new();
        for w in [&[0x07u8, 0x20, 0x00, 0x01][..], &[0xFC, 0xFC, 0x20, 0x01], &[0x8D, 0x3E, 0x00, 0x0C], &[0x60, 0x28, 0x20, 0x00], &[0x60, 0x2A, 0x12, 0xEC], &[0x6F, 0x12, 0x20, 0x21, 0x30, 0x00], &[0x01, 0x04, 0x01]] {
            t.push(b'W');
            t.extend_from_slice(&(w.len() as u16).to_be_bytes());
            t.extend_from_slice(w);
        }
        t.extend_from_slice(&[b'D', 0, 0, 0, 1, b'R', 0, 2, 0x00, 0x05]);
        let items = table(&t, 0x3D);
        assert_eq!(flat(&items, 0x3D, None), vec!["4000_0720=0001", "2001_8D3E=000C", "2000_12EC=2021", "2000_12EE=3000", "2001_0104=01"]);
    }
}
