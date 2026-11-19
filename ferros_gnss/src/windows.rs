//! The BCM4776's orbit windows (`82 81`, docs/GPS.md "The LTO windows are broadcast ephemerides"): a replay's windows rebuilt with ephemerides of ferros's own choosing, the GPS driver's and gps-trace's one path.
//!
//! A window message (from its class, unescaped): class `82 81`, length u16, checksum u16 (the 16-bit sum of the class, the length and the body, the checksum and the trailer left out), a header to G#BC, records of G#A8 bytes from G#BC (their count the word at G#68), the trailer u16.
//! A record is 42 little-endian words; the orbit and clock are IS-GPS-200's, one word a field in the ICD's scaling ([`encode`]); the record's other words (the satellite, its channel code, words 6, 25, 26, 36) are kept from the replay's own window for that satellite.
//! The GPS group's header has a nonzero word at G#4C, the GLONASS group's a zero one.
//!
//! The transport (GPS.md "Captured and decoded"): frames `[sequence, length low, flags, a byte per set flag bit, payload, CRC-8]`, escaped `b0 00 … b0 01`; a window's bytes ride RPC `1e` frames (`1e`, the chunk's length + 2 in one byte, or two with G#80 set in the first, then the chunk's length u16, then the chunk), at most 1000 bytes of the escaped message a frame.

use crate::eph::GpsEph;
use alloc::vec;
use alloc::vec::Vec;

/// The systems a window serves, as the record's group says.
pub const GPS: u8 = 0;
pub const GLONASS: u8 = 1;

/// One satellite's set for a window: its system and number (GPS PRN, GLONASS slot) and the IS-GPS-200 orbit and clock (a GLONASS one fitted to its broadcast vectors, ferros_gnss `glo`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Set {
    pub system: u8,
    pub number: u8,
    pub eph: GpsEph,
}

/// The transport's CRC-8 (polynomial G#4D, MSB first, nibbles swapped).
pub fn crc(bytes: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in bytes {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { c << 1 ^ 0x4D } else { c << 1 };
        }
    }
    c.rotate_left(4)
}

/// Where a frame's payload starts: one byte per set flag bit after the flags, the second flags byte (flag G#80) adding bits 8 and up.
pub fn payload_start(f: &[u8]) -> usize {
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

/// A record's frames, deframed.
pub fn frames(rec: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < rec.len() {
        if rec[i] != 0xB0 || rec[i + 1] != 0x00 {
            i += 1;
            continue;
        }
        let Some(e) = rec[i + 2..].windows(2).position(|w| w == [0xB0, 0x01]).map(|e| e + i + 2) else { break };
        let mut f = Vec::new();
        let mut j = i + 2;
        while j < e {
            if rec[j] == 0xB0 && j + 1 < e {
                f.push(match rec[j + 1] {
                    3 => 0xB0,
                    4 => 0x11,
                    _ => 0x13,
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

/// A frame escaped for the wire.
pub fn escape(f: &[u8]) -> Vec<u8> {
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

/// Whether a record carries the start of a window.
pub fn has_window(rec: &[u8]) -> bool {
    rec.windows(4).any(|w| w == [0xC0, 0xC1, 0x82, 0x81])
}

/// An inner message's checksum.
pub fn checksum(m: &[u8]) -> u16 {
    m[..4].iter().chain(&m[6..m.len() - 2]).fold(0u16, |a, &b| a.wrapping_add(b as u16))
}

fn word(m: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]])
}

/// A record's orbit and clock words from `e` (IS-GPS-200 scaling, semicircles where the ICD has them); `tgd`: the GPS record's word 12 too (a GLONASS record keeps its own).
pub fn encode(rec: &mut [u8], e: &GpsEph, tgd: bool) {
    let pi = core::f64::consts::PI;
    let sc = |x: f64, p: i32| libm::round(x * libm::pow(2., -p as f64)) as i64 as u32;
    let mut put = |k: usize, v: u32| rec[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
    if tgd {
        put(12, sc(e.tgd, -31));
    }
    put(13, libm::round(e.toc / 16.) as u32);
    put(14, sc(e.af2, -55));
    put(15, sc(e.af1, -43));
    put(16, sc(e.af0, -31));
    put(17, sc(e.crs, -5));
    put(18, sc(e.dn / pi, -43));
    put(19, sc(e.m0 / pi, -31));
    put(20, sc(e.cuc, -29));
    put(21, libm::round(e.e * libm::pow(2., 33.)) as u32);
    put(22, sc(e.cus, -29));
    put(23, libm::round(e.sqrt_a * libm::pow(2., 19.)) as u32);
    put(24, libm::round(e.toe / 16.) as u32);
    put(27, sc(e.cic, -29));
    put(28, sc(e.omega0 / pi, -31));
    put(29, sc(e.cis, -29));
    put(30, sc(e.i0 / pi, -31));
    put(31, sc(e.crc, -5));
    put(32, sc(e.omega / pi, -31));
    put(33, sc(e.omega_dot / pi, -43));
    put(34, sc(e.idot / pi, -43));
    put(35, (e.week % 1024) as u32);
}

/// A record read back (the inverse of [`encode`], for the tests and the comparison against gpsd's windows); the week from word 35 is mod 1024.
pub fn decode(rec: &[u8]) -> GpsEph {
    let pi = core::f64::consts::PI;
    let w = |k: usize| word(rec, 4 * k);
    let s = |k: usize, p: i32| w(k) as i32 as f64 * libm::pow(2., p as f64);
    GpsEph {
        prn: w(0) as u8,
        week: w(35) as u16,
        toe: w(24) as f64 * 16.,
        toc: w(13) as f64 * 16.,
        af0: s(16, -31),
        af1: s(15, -43),
        af2: s(14, -55),
        tgd: s(12, -31),
        crs: s(17, -5),
        dn: s(18, -43) * pi,
        m0: s(19, -31) * pi,
        cuc: s(20, -29),
        e: w(21) as f64 * libm::pow(2., -33.),
        cus: s(22, -29),
        sqrt_a: w(23) as f64 * libm::pow(2., -19.),
        cic: s(27, -29),
        omega0: s(28, -31) * pi,
        cis: s(29, -29),
        i0: s(30, -31) * pi,
        crc: s(31, -5),
        omega: s(32, -31) * pi,
        omega_dot: s(33, -43) * pi,
        idot: s(34, -43) * pi,
        ..Default::default()
    }
}

/// What a rebuild did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rebuilt {
    pub messages: u32,
    pub gps: u32,
    pub glonass: u32,
    /// Records of the replay's windows with no set given: kept as they were.
    pub kept: u32,
}

/// The windows' records rebuilt: `prev` the record before the first window (its last reliable frame's sequence and counter go on), `tail` the replay's records from the first window on (each `(gap ms, bytes)`), `sets` the satellites to write. The windows are read back whole from the tail's RPC `1e` frames, every record whose satellite has a set gets its orbit and clock from it, the trailers run on from the first window's, the checksums are redone, and the messages are framed again in chunks of at most 1000 bytes, one frame a record, the first keeping the tail's first gap. None when the tail holds no whole window or `prev` no reliable frame.
pub fn rebuild(prev: &[u8], tail: &[(u16, Vec<u8>)], sets: &[Set]) -> Option<(Vec<(u16, Vec<u8>)>, Rebuilt)> {
    let pf = frames(prev).into_iter().filter(|f| f.len() > 4 && f[2] & 0x02 != 0).last()?;
    let (mut seq, mut counter) = (pf[0], pf[3 + (pf[2] & 1) as usize]);
    let mut stream = Vec::new();
    for (_, r) in tail {
        for f in frames(r) {
            if f.len() < 5 {
                continue;
            }
            let p = &f[payload_start(&f)..f.len() - 1];
            if p.len() < 5 || p[0] != 0x1E {
                continue;
            }
            let h = if p[1] & 0x80 != 0 { 5 } else { 4 };
            stream.extend_from_slice(&p[h..]);
        }
    }
    // The messages, unescaped, from each `c0 c1` to its `c0 c4`.
    let mut msgs: Vec<Vec<u8>> = Vec::new();
    let mut i = 0;
    while i + 1 < stream.len() {
        if stream[i] == 0xC0 && stream[i + 1] == 0xC1 {
            let mut m = Vec::new();
            let mut j = i + 2;
            let mut done = false;
            while j + 1 < stream.len() {
                if stream[j] == 0xC0 && stream[j + 1] == 0xC0 {
                    m.push(0xC0);
                    j += 2;
                } else if stream[j] == 0xC0 && stream[j + 1] == 0xC4 {
                    done = true;
                    j += 2;
                    break;
                } else {
                    m.push(stream[j]);
                    j += 1;
                }
            }
            if done && m.len() > 0xBC && m[..2] == [0x82, 0x81] {
                msgs.push(m);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    if msgs.is_empty() {
        return None;
    }
    let mut done = Rebuilt::default();
    let mut trailer = u16::from_le_bytes([msgs[0][msgs[0].len() - 2], msgs[0][msgs[0].len() - 1]]);
    let mut out: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut gap = tail[0].0;
    for mut m in msgs {
        let system = if word(&m, 0x4C) != 0 { GPS } else { GLONASS };
        let n = word(&m, 0x68) as usize;
        for r in 0..n {
            let base = 0xBC + 0xA8 * r;
            // WHY the bound: the count is the stored replay's word (captured from gpsd), not this code's; a count past the message's records would index past it.
            if base + 0xA8 > m.len() - 2 {
                break;
            }
            let number = word(&m, base) as u8;
            match sets.iter().find(|s| s.system == system && s.number == number) {
                Some(s) => {
                    encode(&mut m[base..base + 0xA8], &s.eph, system == GPS);
                    if system == GPS {
                        done.gps += 1;
                    } else {
                        done.glonass += 1;
                    }
                }
                None => done.kept += 1,
            }
        }
        let l = m.len();
        m[l - 2..].copy_from_slice(&trailer.to_le_bytes());
        trailer = trailer.wrapping_add(1);
        let ck = checksum(&m);
        m[4..6].copy_from_slice(&ck.to_le_bytes());
        done.messages += 1;
        let mut e = vec![0xC0, 0xC1];
        for &x in &m {
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
            out.push((gap, escape(&f)));
            gap = 4;
        }
    }
    Some((out, done))
}
