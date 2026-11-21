//! oracle-i2c: an hv oracle capture of an Exynos USI HS-I2C controller (auto mode) turned back into I2C transfers, as tools/oracle-decode-hsi2c.py did, in Rust (Python is not allowed here); and with `--holds`, one sensor's CCS writes read out of them, each grouped-parameter hold on one line.
//!
//! Input: the flushed console (console-ramoops-0), or its `ferros hv: S2 <pa> <R|W><bytes> <value>` lines (HV.md, the register oracle); `ferros hv: S2 rep <N>` repeats the access before it N more times; `ferros-mark: ...` lines written from Android pass thru as `# MARK ...`.
//! Register map as ferros_kernel hsi2c.rs has it: TX_DATA G#34, RX_DATA G#38, AUTO_CONF G#44 (length bits 0-15, READ bit 16, RUN bit 31), ADDR G#70 (the 7-bit target at bits 10-16).
//!
//! Usage: oracle-i2c <capture.log> [--base G#1088_0000] [--max-bytes N] [--holds G#3D] [--flat] [--table] [--init <out>]; or oracle-i2c <dir> --ftrace [--adapter N] ...
//! - `--ftrace`: the input is a directory of the kernel's i2c tracepoints captured on Android with root and no hv (`cpu<N>.raw` ring pages and `format/`; docs/CAMERA.md "The I2C side without a flash"), read as the same transfers, every message whole; `--adapter N` keeps the kernel's bus i2c-N alone. Its integrity line counts pages that missed events, writes whose bytes are not their length, reads unanswered or short.
//! - `--from <text>`: start at the first `ferros-mark` holding the text, so each open of a session gives its own init and holds.
//! - `--stream N` (with `--init`): the init between stream on N - 1 and stream on N (from 0), for an open that follows a close which streamed on for a moment.
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

/// One event's fields by name (offset, size), and its ID, from a tracefs `format` file.
struct Format {
    id: u16,
    fields: Vec<(String, usize, usize)>,
}

impl Format {
    fn read(path: &std::path::Path) -> Format {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut id = None;
        let mut fields = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("ID:") {
                id = v.trim().parse().ok();
            }
            let Some(rest) = line.strip_prefix("field:") else { continue };
            let parts: Vec<&str> = rest.split(';').map(str::trim).collect();
            let name = parts[0].split_whitespace().last().unwrap_or("").trim_end_matches("[]").to_string();
            let num = |key: &str| parts.iter().find_map(|p| p.strip_prefix(key)).and_then(|v| v.parse::<usize>().ok());
            if let (Some(o), Some(s)) = (num("offset:"), num("size:")) {
                fields.push((name, o, s));
            }
        }
        Format { id: id.unwrap_or_else(|| panic!("{}: no ID", path.display())), fields }
    }
    fn at(&self, name: &str) -> (usize, usize) {
        self.fields.iter().find(|f| f.0 == name).map(|f| (f.1, f.2)).unwrap_or_else(|| panic!("no field {name}"))
    }
}

fn le(d: &[u8], off: usize, size: usize) -> u64 {
    let mut v = 0u64;
    for k in 0..size {
        v |= (d[off + k] as u64) << (8 * k);
    }
    v
}

/// What an ftrace capture said about itself: pages read, pages flagged with missed events, events by kind, writes whose array length is not their `len`, reads with no reply or a reply of another length, transfers whose result was not every message, events per adapter.
#[derive(Default)]
struct FtraceReport {
    pages: usize,
    missed: usize,
    events: usize,
    short_writes: usize,
    bad_replies: usize,
    failed: usize,
    adapters: Vec<(i64, usize)>,
}

/// An ftrace capture of the kernel's i2c tracepoints (Android with root, no hv: docs/CAMERA.md "The I2C side without a flash") as transfers, markers in place, in time order across the CPUs.
/// `dir` holds `cpu<N>.raw` (each CPU's `per_cpu/cpuN/trace_pipe_raw`, whole ring pages) and `format/` (the events' tracefs `format` files: i2c_write, i2c_read, i2c_reply, print).
/// The ring buffer as kernel/trace/ring_buffer.c writes it: 4096-byte pages, a header of the page's time (u64) and its commit (u64: the data's bytes, bit 31 events missed before the page); each event a u32 of type_len (5 bits) and time delta (27), type_len 1 to 28 data of 4 x type_len bytes, 0 data whose length (counting the length word) is the next u32, 29 padding, 30 a time extend (delta = the next u32 << 27 | delta), 31 an absolute time.
/// A write's bytes are its `buf` array (`__data_loc`: offset in the low 16 bits, length in the high), whole however long, where the text output cuts every message at 64 bytes; a read is its `i2c_reply`'s bytes with the length its `i2c_read` asked.
/// `adapter`: only that bus (the kernel's i2c-N), else every bus the capture filtered in.
fn ftrace(dir: &std::path::Path, adapter: Option<i64>) -> (Vec<Item>, FtraceReport) {
    let f = |n: &str| Format::read(&dir.join("format").join(n));
    let (fw, fr, fp, fs) = (f("i2c_write"), f("i2c_reply"), f("print"), f("i2c_read"));
    let fres = f("i2c_result");
    let mut rep = FtraceReport::default();
    // (time, cpu, sequence, item or pending read): every event, then sorted by time.
    let mut evs: Vec<(u64, usize, usize, Ev)> = Vec::new();
    for cpu in 0..64 {
        let Ok(raw) = std::fs::read(dir.join(format!("cpu{cpu}.raw"))) else { continue };
        let mut seq = 0;
        for page in raw.chunks(4096) {
            if page.len() < 16 {
                continue;
            }
            rep.pages += 1;
            let mut t = le(page, 0, 8);
            let commit = le(page, 8, 8);
            if commit >> 31 & 1 == 1 {
                rep.missed += 1;
            }
            let size = (commit & 0xF_FFFF) as usize;
            let end = (16 + size).min(page.len());
            let mut i = 16;
            while i + 4 <= end {
                let h = le(page, i, 4) as u32;
                let (tl, delta) = (h & 0x1F, (h >> 5) as u64);
                match tl {
                    29 => {
                        // A null padding (no delta) is the rest of the page, empty.
                        if delta == 0 {
                            break;
                        }
                        let n = le(page, i + 4, 4) as usize;
                        t += delta;
                        i += 4 + n.max(4);
                        continue;
                    }
                    30 => {
                        t += le(page, i + 4, 4) << 27 | delta;
                        i += 8;
                        continue;
                    }
                    31 => {
                        t = (t & !((1u64 << 59) - 1)) | (le(page, i + 4, 4) << 27 | delta);
                        i += 8;
                        continue;
                    }
                    _ => {}
                }
                t += delta;
                let (data, next) = if tl == 0 {
                    let n = le(page, i + 4, 4) as usize;
                    (&page[i + 8..i + 4 + n], i + 4 + n)
                } else {
                    (&page[i + 4..i + 4 + 4 * tl as usize], i + 4 + 4 * tl as usize)
                };
                i = next;
                rep.events += 1;
                let ty = le(data, 0, 2) as u16;
                let get = |fm: &Format, name: &str| {
                    let (o, s) = fm.at(name);
                    le(data, o, s)
                };
                let bus = |fm: &Format| get(fm, "adapter_nr") as u32 as i32 as i64;
                let ev = if ty == fp.id {
                    let (o, _) = fp.at("buf");
                    let s = String::from_utf8_lossy(&data[o..]).trim_end_matches(['\0', '\n']).to_string();
                    match s.find("ferros-mark: ") {
                        Some(k) => Ev::Mark(s[k + 13..].to_string()),
                        None => continue,
                    }
                } else if ty == fw.id || ty == fr.id || ty == fs.id {
                    let fm = if ty == fw.id { &fw } else if ty == fr.id { &fr } else { &fs };
                    let b = bus(fm);
                    if adapter.is_some_and(|a| a != b) {
                        continue;
                    }
                    match rep.adapters.iter_mut().find(|a| a.0 == b) {
                        Some(a) => a.1 += 1,
                        None => rep.adapters.push((b, 1)),
                    }
                    let (target, len, nr) = (get(fm, "addr") as u8, get(fm, "len") as usize, get(fm, "msg_nr") as u16);
                    if ty == fs.id {
                        Ev::Read { bus: b, nr, target, len }
                    } else {
                        let loc = get(fm, "buf") as u32;
                        let (o, n) = ((loc & 0xFFFF) as usize, (loc >> 16) as usize);
                        let bytes = data[o..o + n].to_vec();
                        if ty == fw.id { Ev::Write { target, len, bytes } } else { Ev::Reply { bus: b, nr, len, bytes } }
                    }
                } else if ty == fres.id {
                    let b = bus(&fres);
                    if adapter.is_some_and(|a| a != b) {
                        continue;
                    }
                    Ev::Result { bus: b, nr_msgs: get(&fres, "nr_msgs") as i64, ret: get(&fres, "ret") as u16 as i16 as i64 }
                } else {
                    continue;
                };
                evs.push((t, cpu, seq, ev));
                seq += 1;
            }
        }
    }
    evs.sort_by_key(|e| (e.0, e.1, e.2));
    let mut items = Vec::new();
    // A read asked for (bus, message number) waits for its reply, which i2c_transfer traces after the whole transfer.
    let mut asked: Vec<(i64, u16, u8, usize, usize)> = Vec::new();
    for (_, _, _, ev) in evs {
        match ev {
            Ev::Mark(m) => items.push(Item::Mark(m)),
            Ev::Write { target, len, bytes } => {
                if bytes.len() != len {
                    rep.short_writes += 1;
                }
                items.push(Item::Xfer(Xfer { target, read: false, data: bytes, expect: Some(len) }));
            }
            Ev::Read { bus, nr, target, len } => {
                asked.retain(|a| !(a.0 == bus && a.1 == nr));
                asked.push((bus, nr, target, len, items.len()));
                items.push(Item::Xfer(Xfer { target, read: true, data: Vec::new(), expect: Some(len) }));
            }
            Ev::Reply { bus, nr, len, bytes } => match asked.iter().position(|a| a.0 == bus && a.1 == nr) {
                Some(k) => {
                    let a = asked.remove(k);
                    if a.3 != len || bytes.len() != len {
                        rep.bad_replies += 1;
                    }
                    if let Item::Xfer(x) = &mut items[a.4] {
                        x.data = bytes;
                    }
                }
                None => rep.bad_replies += 1,
            },
            Ev::Result { bus, nr_msgs, ret } => {
                // Results carry no address: only a bus that carried a filtered message counts, and a failure there may be another device on it (a lens, an EEPROM), so it is said, not refused.
                if ret != nr_msgs && rep.adapters.iter().any(|a| a.0 == bus) {
                    rep.failed += 1;
                }
            }
        }
    }
    // A read never answered keeps no bytes: its length check says so.
    rep.bad_replies += asked.len();
    rep.adapters.sort();
    (items, rep)
}

enum Ev {
    Mark(String),
    Write { target: u8, len: usize, bytes: Vec<u8> },
    Read { bus: i64, nr: u16, target: u8, len: usize },
    Reply { bus: i64, nr: u16, len: usize, bytes: Vec<u8> },
    Result { bus: i64, nr_msgs: i64, ret: i64 },
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
/// `nth`: the transfers between stream on `nth - 1` and stream on `nth` (counted from 0; 0 from the start), for a session whose camera streamed before the open wanted (the ultrawide's close streams on for a moment, 2026-11-18).
fn init_table(items: &[Item], target: u8, nth: usize) -> Option<(Vec<u8>, usize)> {
    let xs: Vec<&Xfer> = items.iter().filter_map(|it| if let Item::Xfer(x) = it { (x.target == target).then_some(x) } else { None }).collect();
    // The stream on is G#0100 on the CCS page: a Samsung part's G#0100 on another page (G#FCFC = G#4001 selects one, the main's init writes G#0100 = G#FFFF there) is not mode_select.
    let mut page = 0x4000u16;
    let ons: Vec<usize> = xs
        .iter()
        .enumerate()
        .filter(|(_, x)| {
            if x.read || x.data.len() < 3 {
                return false;
            }
            if x.data[..2] == [0xFC, 0xFC] && x.data.len() >= 4 {
                page = u16::from_be_bytes([x.data[2], x.data[3]]);
            }
            page == 0x4000 && x.data[..2] == [0x01, 0x00] && x.data[2] != 0
        })
        .map(|(k, _)| k)
        .collect();
    let end = *ons.get(nth)?;
    let start = if nth == 0 { 0 } else { ons[nth - 1] + 1 };
    if xs[start..end].iter().any(|x| x.expect.is_some_and(|e| e != x.data.len())) {
        return None;
    }
    let mut t = Vec::new();
    let (mut i, mut n) = (start, 0);
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
    let (mut flat_out, mut is_table, mut ftrace_in) = (false, false, false);
    let mut adapter: Option<i64> = None;
    let mut from: Option<String> = None;
    let mut nth = 0usize;
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
            "--ftrace" => {
                ftrace_in = true;
                k += 1;
            }
            "--stream" => {
                nth = args.get(k + 1).and_then(|s| s.parse().ok()).expect("--stream N (the N-th stream on, from 0)");
                k += 2;
            }
            "--from" => {
                from = Some(args.get(k + 1).expect("--from <text of a mark>").clone());
                k += 2;
            }
            "--adapter" => {
                adapter = Some(args.get(k + 1).and_then(|s| s.parse().ok()).expect("--adapter N (the kernel's i2c-N)"));
                k += 2;
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
        eprintln!("usage: oracle-i2c <capture.log, init table, or ftrace dir> [--base G#1088_0000] [--max-bytes N] [--holds G#3D] [--flat] [--table] [--ftrace [--adapter N]] [--init <out>]");
        std::process::exit(2);
    };
    let items = if ftrace_in {
        let (items, r) = ftrace(std::path::Path::new(&path), adapter);
        let buses: Vec<String> = r.adapters.iter().map(|a| format!("i2c-{} {}", a.0, a.1)).collect();
        eprintln!("# ftrace: {} pages ({} flagged missed events), {} events; messages by bus: {}", r.pages, r.missed, r.events, buses.join(", "));
        eprintln!("# ftrace integrity: {} write(s) whose bytes are not their length, {} read(s) unanswered or answered short, {} transfer(s) on these buses not completed whole{}", r.short_writes, r.bad_replies, r.failed, if r.missed + r.short_writes + r.bad_replies == 0 { "" } else { " -- DO NOT REPLAY" });
        if r.missed != 0 && init_out.is_some() {
            eprintln!("# no init: the ring missed events -- DO NOT REPLAY");
            std::process::exit(1);
        }
        items
    } else {
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        if is_table { table(&raw, focus.expect("--table needs --holds G#<target> or --flat with --holds")) } else { transfers(&accesses(&String::from_utf8_lossy(&raw)), base) }
    };
    // `--from`: the items from the first mark whose text holds it, so one session's later opens each give their own init (a camera opened twice, another readout).
    let items = match &from {
        Some(f) => {
            let Some(k) = items.iter().position(|it| matches!(it, Item::Mark(m) if m.contains(f.as_str()))) else {
                eprintln!("# no mark holds {f:?}");
                std::process::exit(1);
            };
            items.into_iter().skip(k).collect()
        }
        None => items,
    };
    if let Some(out) = init_out {
        let t = focus.expect("--init needs --holds G#<target>");
        let Some((table, n)) = init_table(&items, t, nth) else {
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
        let (t, n) = init_table(&items, 0x3D, 0).unwrap();
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
        assert!(init_table(&items, 0x3D, 0).is_none());
        items.truncate(5);
        items[4] = w(&[2, 2, 0x11, 0xDA]);
        assert!(init_table(&items, 0x3D, 0).is_none());
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

    /// Ring pages as the kernel writes them, read back: two CPUs merged by time (a time extend on one), a write long enough for a length word (type_len 0, past the text output's 64 bytes), a read answered by its reply, a mark, another bus left out by `--adapter`; then `--stream 1` cuts the init after a close's brief stream on.
    #[test]
    fn ftrace_pages() {
        let dir = std::env::temp_dir().join(format!("oracle-i2c-ftrace-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("format")).unwrap();
        let fmt = |name: &str, id: u16, fields: &str| std::fs::write(dir.join("format").join(name), format!("name: {name}\nID: {id}\nformat:\n\tfield:unsigned short common_type;\toffset:0;\tsize:2;\tsigned:0;\n{fields}")).unwrap();
        let msg = "\tfield:int adapter_nr;\toffset:8;\tsize:4;\tsigned:1;\n\tfield:__u16 msg_nr;\toffset:12;\tsize:2;\tsigned:0;\n\tfield:__u16 addr;\toffset:14;\tsize:2;\tsigned:0;\n\tfield:__u16 flags;\toffset:16;\tsize:2;\tsigned:0;\n\tfield:__u16 len;\toffset:18;\tsize:2;\tsigned:0;\n";
        let arr = "\tfield:__data_loc __u8[] buf;\toffset:20;\tsize:4;\tsigned:0;\n";
        fmt("i2c_write", 840, &format!("{msg}{arr}"));
        fmt("i2c_read", 841, msg);
        fmt("i2c_reply", 842, &format!("{msg}{arr}"));
        fmt("i2c_result", 843, "\tfield:int adapter_nr;\toffset:8;\tsize:4;\tsigned:1;\n\tfield:__u16 nr_msgs;\toffset:12;\tsize:2;\tsigned:0;\n\tfield:__s16 ret;\toffset:14;\tsize:2;\tsigned:1;\n");
        fmt("print", 5, "\tfield:unsigned long ip;\toffset:8;\tsize:8;\tsigned:0;\n\tfield:char buf[];\toffset:16;\tsize:0;\tsigned:0;\n");
        // An event's payload: the i2c message fields and its bytes at offset 24.
        let ev = |ty: u16, bus: i32, nr: u16, addr: u16, d: &[u8]| {
            let mut p = vec![0u8; 24];
            p[..2].copy_from_slice(&ty.to_le_bytes());
            p[8..12].copy_from_slice(&bus.to_le_bytes());
            p[12..14].copy_from_slice(&nr.to_le_bytes());
            p[14..16].copy_from_slice(&addr.to_le_bytes());
            p[18..20].copy_from_slice(&(d.len() as u16).to_le_bytes());
            p[20..24].copy_from_slice(&(24u32 | (d.len() as u32) << 16).to_le_bytes());
            p.extend_from_slice(d);
            p
        };
        let mark = |s: &str| {
            let mut p = vec![0u8; 16];
            p[..2].copy_from_slice(&5u16.to_le_bytes());
            p.extend_from_slice(format!("ferros-mark: {s}\n\0").as_bytes());
            p
        };
        // A page: its time, then (delta, payload) events; a payload over 112 bytes takes the length word (type_len 0).
        let page = |t0: u64, evs: &[(u32, Vec<u8>)], extend: bool| {
            let mut d = Vec::new();
            if extend {
                d.extend_from_slice(&(30u32 | 0 << 5).to_le_bytes());
                d.extend_from_slice(&1u32.to_le_bytes());
            }
            for (delta, p) in evs {
                let mut p = p.clone();
                while p.len() % 4 != 0 {
                    p.push(0);
                }
                if p.len() <= 112 {
                    d.extend_from_slice(&((p.len() / 4) as u32 | delta << 5).to_le_bytes());
                } else {
                    d.extend_from_slice(&(delta << 5).to_le_bytes());
                    d.extend_from_slice(&((p.len() + 4) as u32).to_le_bytes());
                }
                d.extend_from_slice(&p);
            }
            let mut pg = vec![0u8; 4096];
            pg[..8].copy_from_slice(&t0.to_le_bytes());
            pg[8..16].copy_from_slice(&(d.len() as u64).to_le_bytes());
            pg[16..16 + d.len()].copy_from_slice(&d);
            pg
        };
        let long: Vec<u8> = (0..200u16).map(|k| k as u8).collect();
        let mut upload = vec![0x6F, 0x12];
        upload.extend_from_slice(&long);
        // CPU 0: the mark, the identity read and its reply, the upload, the stream on, the close's stream off and brief on, a write on another bus.
        let cpu0 = page(100, &[(0, mark("open")), (1, ev(840, 3, 0, 0x1A, &[0, 0x16])), (0, ev(841, 3, 1, 0x1A, &[0, 0])), (2, ev(842, 3, 1, 0x1A, &[7, 0x87])), (1, ev(840, 3, 0, 0x1A, &upload)), (1, ev(840, 3, 0, 0x1A, &[1, 0, 1])), (1, ev(840, 3, 0, 0x1A, &[1, 0, 0])), (1, ev(840, 3, 0, 0x1A, &[1, 0, 1])), (0, ev(840, 2, 0, 0x1A, &[9, 9, 9]))], false);
        std::fs::write(dir.join("cpu0.raw"), &cpu0).unwrap();
        // CPU 1, past CPU 0's events by a time extend (1 << 27): the next open's mode write and its stream on.
        let cpu1 = page(100, &[(0, ev(840, 3, 0, 0x1A, &[3, 0x4C, 0x1F])), (1, ev(840, 3, 0, 0x1A, &[1, 0, 1]))], true);
        std::fs::write(dir.join("cpu1.raw"), &cpu1).unwrap();
        let (items, rep) = ftrace(&dir, Some(3));
        assert_eq!((rep.pages, rep.missed, rep.short_writes, rep.bad_replies), (2, 0, 0, 0));
        assert_eq!(rep.adapters, vec![(3, 9)]);
        let h = holds(&items, 0x1A, None);
        assert_eq!(h[0], "# MARK open");
        assert_eq!(h[1], "W 00 16");
        assert_eq!(h[2], "R 07 87");
        assert!(h[3].starts_with("W [00000000]=000102") && h[3].len() > 400, "{}", h[3]);
        assert_eq!(&h[4..], ["W 0100=01", "W 0100=00", "W 0100=01", "W 034C=1F", "W 0100=01"]);
        let (t, n) = init_table(&items, 0x1A, 0).unwrap();
        assert_eq!((n, t[0]), (2, b'R'));
        let (t, n) = init_table(&items, 0x1A, 2).unwrap();
        assert_eq!((n, &t[..6]), (1, &[b'W', 0, 3, 3, 0x4C, 0x1F][..]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
