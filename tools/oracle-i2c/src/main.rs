//! oracle-i2c: an hv oracle capture of an Exynos USI HS-I2C controller (auto mode) turned back into I2C transfers, as tools/oracle-decode-hsi2c.py does, in Rust (Python is not allowed here); and with `--holds`, one sensor's CCS writes read out of them, each grouped-parameter hold on one line.
//!
//! Input: the flushed console (console-ramoops-0), or its `ferros hv: S2 <pa> <R|W><bytes> <value>` lines (HV.md, the register oracle); `ferros hv: S2 rep <N>` repeats the access before it N more times; `ferros-mark: ...` lines written from Android pass thru as `# MARK ...`.
//! Register map as ferros_kernel hsi2c.rs has it: TX_DATA G#34, RX_DATA G#38, AUTO_CONF G#44 (length bits 0-15, READ bit 16, RUN bit 31), ADDR G#70 (the 7-bit target at bits 10-16).
//!
//! Usage: oracle-i2c <capture.log> [--base G#1088_0000] [--max-bytes N] [--holds G#3D]
//! - Transfers (the default): `G#3D W 02 02 53 5E`, one a line; a transfer longer than `--max-bytes` as its first four bytes, its length and its blake3 (a sensor's settings upload is vendor firmware: its size and identity stay checkable, its bytes stay out of any file meant for the repository).
//! - `--holds T`: target T's writes as 16-bit CCS register and data: every hold (G#0104 = 1 ... G#0104 = 0) on one line, `hold: 0702=0000 0704=0000 0202=535E 0340=5372`; every write outside a hold on its own line (stream on and off, mode changes); Samsung's indirect window (G#6028, G#602A, G#6F12) said as the page address it writes; reads as `R`. What a long exposure writes is then read straight off it (docs/CAMERA.md "Long exposures").
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
    let mut k = 0;
    while k < args.len() {
        match args[k].as_str() {
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
        eprintln!("usage: oracle-i2c <capture.log> [--base G#1088_0000] [--max-bytes N] [--holds G#3D]");
        std::process::exit(2);
    };
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let items = transfers(&accesses(&String::from_utf8_lossy(&raw)), base);
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
}
