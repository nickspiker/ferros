//! The hv's modem boot-channel trace (FERROS_HV_MODEM_RING) as the boot protocol.
//! Input: pstore lines `ferros hv: RING T|R <len> <old> <hex>` (T = a frame the AP put in the RAW TX ring, R = frames it consumed from the RAW RX ring; hex = the first 48 / 128 bytes).
//! Each frame is a 12-byte SIT header (G#ABCD, seq, cfg, length, channel G#F1, channel seq, 2 spare) and a payload whose first u16 is the command; boot-download data frames (G#A12B) carry {u16 cmd, u16 chunk+8, u32 total, u32 offset, data}.
//! Runs of data frames are summarised (count, total, offsets) instead of printed; with --data-head, each run's first 16 data bytes are shown so the chunk can be found in an image file.
//! Raw traces stay local (the boot channel can carry per-unit NV data); this summary prints no payload beyond command words, sizes and offsets unless asked.

use crate::{hex, stdin_text};

/// Upper-case hex digits only, as the hv prints them: the run at the start of `s` (possibly empty).
fn upper_hex(s: &str) -> &str {
    &s[..s.bytes().take_while(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b)).count()]
}

/// `RING <T|R> <hex> <hex> <hex or nothing>` at `s` (just past "RING ").
fn ring_at(s: &str) -> Option<(char, Vec<u8>)> {
    let mut c = s.chars();
    let d = c.next().filter(|d| *d == 'T' || *d == 'R')?;
    let s = c.as_str().strip_prefix(' ')?;
    let len = upper_hex(s);
    let s = s[len.len()..].strip_prefix(' ').filter(|_| !len.is_empty())?;
    let old = upper_hex(s);
    let s = s[old.len()..].strip_prefix(' ').filter(|_| !old.is_empty())?;
    let h = upper_hex(s);
    let bytes = (0..h.len() / 2).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect();
    Some((d, bytes))
}

fn u16_at(b: &[u8], o: usize) -> u32 {
    b[o] as u32 | (b[o + 1] as u32) << 8
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u16_at(b, o) | u16_at(b, o + 2) << 16
}

/// A run of data frames: direction, command, total, first and last offset, count, the first 16 data bytes.
struct Run {
    d: char,
    cmd: u32,
    total: u32,
    first: u32,
    last: u32,
    n: u32,
    head: Vec<u8>,
}

pub fn run(args: &[String]) {
    let show_head = args.iter().any(|a| a == "--data-head");
    let text = stdin_text();
    let frames: Vec<(char, Vec<u8>)> = text.lines().filter_map(|l| l.match_indices("RING ").find_map(|(i, _)| ring_at(&l[i + 5..]))).collect();
    let mut run: Option<Run> = None;
    let flush = |run: &mut Option<Run>| {
        if let Some(r) = run.take() {
            let extra = if show_head { format!(" first data {}", hex(&r.head, "")) } else { String::new() };
            println!("{} G#{:04X} data x{}: total G#{:X}, offsets G#{:X}..G#{:X}{extra}", r.d, r.cmd, r.n, r.total, r.first, r.last);
        }
    };
    let mut acks = 0u32;
    for (d, b) in &frames {
        let d = *d;
        // A frame consumed from RX may be several CP frames back to back.
        let mut pos = 0usize;
        while pos + 12 <= b.len() {
            if u16_at(b, pos) != 0xABCD {
                println!("{d} (no SIT header at +{pos}): {}", hex(&b[pos..(pos + 16).min(b.len())], ""));
                break;
            }
            let flen = u16_at(b, pos + 6) as usize;
            // The slices clip at the capture's end, as the Python's did: a frame's length runs past the 48 (T) or 128 (R) bytes the hv logs of it.
            let p = &b[pos + 12..(pos + flen.max(12)).min(b.len())];
            // No command word: -1 in the Python, which reads as G#FFFF and never as a data or ack command.
            let cmd: i64 = if p.len() >= 2 { u16_at(p, 0) as i64 } else { -1 };
            if cmd & 0xF000 == 0xA000 && p.len() >= 12 && u16_at(p, 2) > 8 && flen > 64 {
                let (total, off) = (u32_at(p, 4), u32_at(p, 8));
                match &mut run {
                    Some(r) if r.d == d && r.cmd as i64 == cmd && r.total == total => {
                        r.last = off;
                        r.n += 1;
                    }
                    _ => {
                        flush(&mut run);
                        run = Some(Run { d, cmd: cmd as u32, total, first: off, last: off, n: 1, head: p[12..28.min(p.len())].to_vec() });
                    }
                }
            } else if run.as_ref().is_some_and(|r| d == 'R' && cmd == ((r.cmd & 0x0FFF) | 0xC000) as i64) && p.len() <= 4 {
                acks += 1;
            } else {
                flush(&mut run);
                println!("{d} G#{:04X} len {flen} payload {}", cmd & 0xFFFF, hex(&p[2.min(p.len())..p.len().min(2 + 32)], ""));
            }
            pos += flen.max(12);
            pos = (pos + 3) & !3;
            if d == 'T' {
                break;
            }
        }
    }
    flush(&mut run);
    println!("({acks} data acks folded into the runs)");
}
