//! An hv register-oracle capture of an Exynos (s3c64xx family) SPI controller as SPI transactions.
//! Input: pstore lines `ferros hv: S2 <addr> <R|W><size> <value>` and `ferros hv: S2 rep <n>` (the previous access repeated n more times), plus `ferros-mark` lines, passed thru.
//! A transaction runs from slave select active (SLAVE_SEL written with bit 0 clear) to inactive (bit 0 set).
//! Inside it, consecutive TX_DATA writes form one "W" segment and RX_DATA reads one "R" segment, which is how the controller runs in CPU mode (a write-only transfer has RX off, a read-only one TX off).
//! Word-size transfers (MODE_CFG CH_TSZ = word) carry four bytes per access: in wire order when SWAP_CFG swaps bytes and halfwords, most significant byte first otherwise.

use crate::{hex, hex_run, stdin_text};

const OFF_MODE: u64 = 0x08;
const OFF_SS: u64 = 0x0C;
const OFF_TX: u64 = 0x18;
const OFF_RX: u64 = 0x1C;
const OFF_SWAP: u64 = 0x28;

pub enum Ev {
    Mark(String),
    Acc { addr: u64, write: bool, size: u32, val: u64 },
}

/// `S2 <16 hex> <R|W><digit> <16 hex>` at `s`, the start of a candidate.
fn access_at(s: &str) -> Option<(u64, bool, u32, u64)> {
    let (addr, s) = hex_run(s, Some(16))?;
    let s = s.strip_prefix(' ')?;
    let mut c = s.chars();
    let write = match c.next()? {
        'R' => false,
        'W' => true,
        _ => return None,
    };
    let size = c.next()?.to_digit(10)?;
    let s = c.as_str().strip_prefix(' ')?;
    let (val, _) = hex_run(s, Some(16))?;
    Some((addr, write, size, val))
}

/// The leftmost place in `line` where `S2 ` starts a match of `f` (a regular expression's search).
pub fn search<T>(line: &str, f: impl Fn(&str) -> Option<T>) -> Option<T> {
    line.match_indices("S2 ").find_map(|(i, _)| f(&line[i + 3..]))
}

/// Every access in order, repeat runs expanded, marks among them.
pub fn events(text: &str) -> Vec<Ev> {
    let mut out = Vec::new();
    let mut prev: Option<(u64, bool, u32, u64)> = None;
    for line in text.lines() {
        if let Some(i) = line.find("ferros-mark") {
            let rest = &line[i..];
            let m = rest.split_once("ferros-mark:").map(|(_, t)| t.trim()).unwrap_or("");
            out.push(Ev::Mark(m.to_string()));
            continue;
        }
        if let (Some(n), Some((addr, write, size, val))) = (search(line, |s| hex_run(s.strip_prefix("rep ")?, None).map(|(n, _)| n)), prev) {
            for _ in 0..n {
                out.push(Ev::Acc { addr, write, size, val });
            }
            continue;
        }
        if let Some(a) = search(line, access_at) {
            prev = Some(a);
            out.push(Ev::Acc { addr: a.0, write: a.1, size: a.2, val: a.3 });
        }
    }
    out
}

pub fn run() {
    let text = stdin_text();
    let (mut word, mut swap, mut active) = (false, 0u64, false);
    let mut segs: Vec<(char, Vec<u8>)> = Vec::new();
    let mut n = 0u32;
    let flush = |segs: &mut Vec<(char, Vec<u8>)>, n: &mut u32| {
        if !segs.is_empty() {
            *n += 1;
            let s: Vec<String> = segs.iter().map(|(d, b)| format!("{d} {}", hex(b, " "))).collect();
            println!("T{n:04} {}", s.join(" | "));
            segs.clear();
        }
    };
    for ev in events(&text) {
        let (addr, write, size, v) = match ev {
            Ev::Mark(m) => {
                flush(&mut segs, &mut n);
                println!("MARK {m}");
                continue;
            }
            Ev::Acc { addr, write, size, val } => (addr, write, size, val),
        };
        let bytes = || -> Vec<u8> {
            if size == 4 && word {
                let w = v as u32;
                if swap & 0x0C == 0x0C { w.to_le_bytes().to_vec() } else { w.to_be_bytes().to_vec() }
            } else {
                vec![v as u8]
            }
        };
        let mut add = |d: char, b: Vec<u8>| match segs.last_mut() {
            Some((ld, s)) if *ld == d => s.extend(b),
            _ => segs.push((d, b)),
        };
        match (addr & 0xFFF, write) {
            (OFF_MODE, true) => word = (v >> 29) & 3 == 2,
            (OFF_SWAP, true) => swap = v,
            (OFF_SS, true) => {
                if v & 1 != 0 {
                    if active {
                        flush(&mut segs, &mut n);
                    }
                    active = false;
                } else {
                    active = true;
                }
            }
            (OFF_TX, true) => add('W', bytes()),
            (OFF_RX, false) => add('R', bytes()),
            _ => {}
        }
    }
    flush(&mut segs, &mut n);
}
