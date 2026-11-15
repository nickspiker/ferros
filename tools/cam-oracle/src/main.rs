//! cam-oracle: a camera's hv oracle captures (docs/oracle/CAMERA.md) turned into what the camera driver replays, the Rust port of the Python tools the Pro's cameras were brought up with (tools/oracle-decode-hsi2c.py, camera-init.py, camera-cpu.py, oracle-decode-pdma.py, camera-pdma-image.py), same inputs, same outputs.
//! Everything it writes carries the sensor vendor's settings: write it under target/ or the scratch disk, never into the repository.
//!
//! - `cam-oracle i2c <capture.log> [--base G#1088_0000] [--max-bytes N]`: one line per I2C transfer, the integrity line on stderr.
//! - `cam-oracle init <decoded.txt> <target G#1A> <out.bin> [--mark NAME] [--stream-on "01 00 01"]`: the sensor init ('W', 'R', 'D' records).
//! - `cam-oracle marks <capture.log>`: the line numbers of the marks and of the PDMA's starts and index writes, for `cpu`'s a, b, c.
//! - `cam-oracle cpu <capture.log> <a> <b> <c> <out prefix> [--link G#1C06] [--name RX_START]`: the CPU setup (.cpr2) and the receiver start (.rx.rs).
//! - `cam-oracle pdma <dump.log> [--list N] [--base G#1C30_0000]`: the PDMA list decoded into register writes.
//! - `cam-oracle pdma-image <dump.log> <out.cpr3> [--list N]`: the job, a 'CPR3', at the October fixed addresses ferros_cam job relocates from.

use std::collections::BTreeMap;
use std::process::exit;

fn die(s: &str) -> ! {
    eprintln!("cam-oracle: {s}");
    exit(2)
}

/// A number written G#hex, hex, or with underscores.
fn hexnum(s: &str) -> u64 {
    let t: String = s.trim_start_matches("G#").trim_start_matches("0x").chars().filter(|&c| c != '_').collect();
    u64::from_str_radix(&t, 16).unwrap_or_else(|_| die(&format!("not a hex number: {s}")))
}

/// Take `--name value` out of the arguments.
fn opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    if i + 1 >= args.len() {
        die(&format!("{name} wants a value"));
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Some(v)
}

fn read_text(path: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")))).into_owned()
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One oracle line's access: `S2 <pa: 16 hex> <R|W><bytes> <value: 16 hex>`.
fn access(line: &str) -> Option<(u64, u8, u64)> {
    let at = line.find("S2 ")?;
    let mut w = line[at + 3..].split_whitespace();
    let pa = w.next()?;
    let dir = w.next()?;
    let val = w.next()?;
    let val = val.get(..16).unwrap_or(val);
    if !is_hex(pa, 16) || !is_hex(val, 16) || dir.len() != 2 || !(dir.starts_with('R') || dir.starts_with('W')) || !dir.as_bytes()[1].is_ascii_digit() {
        return None;
    }
    Some((u64::from_str_radix(pa, 16).ok()?, dir.as_bytes()[0], u64::from_str_radix(val, 16).ok()?))
}

/// `S2 rep <16 hex>`: the previous access repeated that many more times.
fn rep(line: &str) -> Option<u64> {
    let at = line.find("S2 rep ")?;
    let t = line[at + 7..].split_whitespace().next()?;
    let t = t.get(..16).unwrap_or(t);
    if !is_hex(t, 16) {
        return None;
    }
    u64::from_str_radix(t, 16).ok()
}

fn mark(line: &str) -> Option<&str> {
    let at = line.find("ferros-mark: ")?;
    Some(line[at + 13..].trim())
}

enum Ev {
    Mark(String),
    Acc(u64, u8, u64),
}

/// Every access in order with repeat runs expanded, and the marks among them.
fn events(text: &str) -> Vec<Ev> {
    let mut out = Vec::new();
    let mut prev: Option<(u64, u8, u64)> = None;
    for line in text.lines() {
        if let Some(m) = mark(line) {
            out.push(Ev::Mark(m.to_string()));
            continue;
        }
        if let Some(n) = rep(line) {
            if let Some((a, d, v)) = prev {
                for _ in 0..n {
                    out.push(Ev::Acc(a, d, v));
                }
            }
            continue;
        }
        if let Some((a, d, v)) = access(line) {
            prev = Some((a, d, v));
            out.push(Ev::Acc(a, d, v));
        }
    }
    out
}

fn hexbytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// The HS-I2C controller in auto mode (ferros_kernel hsi2c.rs): TX_DATA G#34, RX_DATA G#38, AUTO_CONF G#44 (length 15:0, READ bit 16, RUN bit 31), ADDR G#70 (the 7-bit target at 16:10).
/// The integrity check: every transfer's byte count against the length its AUTO_CONF run was programmed with (memory feedback_oracle_capture_integrity); a mismatch means lines were lost and that transfer must not be replayed.
fn cmd_i2c(mut args: Vec<String>) {
    let maxb = opt(&mut args, "--max-bytes").map(|v| v.parse::<usize>().unwrap_or_else(|_| die("--max-bytes N")));
    let base = opt(&mut args, "--base").map(|v| hexnum(&v)).unwrap_or(0x1088_0000);
    let path = args.first().unwrap_or_else(|| die("i2c <capture.log>"));
    let text = read_text(path);
    let mut out: Vec<String> = Vec::new();
    let (mut target, mut kind, mut expect): (Option<u64>, Option<u8>, Option<u64>) = (None, None, None);
    let (mut tx, mut rx): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    let mut bad = 0u32;
    let mut flush = |target: Option<u64>, kind: &mut Option<u8>, expect: &mut Option<u64>, tx: &mut Vec<u8>, rx: &mut Vec<u8>, out: &mut Vec<String>| {
        if let (Some(k), Some(t)) = (*kind, target) {
            let data = if k == b'W' { &*tx } else { &*rx };
            let body = match maxb {
                Some(m) if data.len() > m => {
                    let h = blake3::hash(data);
                    format!("{} ...[{} bytes, blake3-64 {}]", hexbytes(&data[..4]), data.len(), hexbytes(&h.as_bytes()[..8]).replace(' ', ""))
                }
                _ => hexbytes(data),
            };
            let mut line = format!("G#{t:02X} {} {body}", k as char);
            if let Some(e) = *expect {
                if data.len() as u64 != e {
                    bad += 1;
                    line.push_str(&format!("   # LENGTH MISMATCH: programmed {e}, captured {}", data.len()));
                }
            }
            out.push(line);
        }
        tx.clear();
        rx.clear();
        *kind = None;
        *expect = None;
    };
    for ev in events(&text) {
        match ev {
            Ev::Mark(m) => {
                flush(target, &mut kind, &mut expect, &mut tx, &mut rx, &mut out);
                out.push(format!("# MARK {m}"));
            }
            Ev::Acc(pa, d, val) => {
                if pa & !0xFFF != base {
                    continue;
                }
                match (pa & 0xFFF, d) {
                    (0x70, b'W') => {
                        flush(target, &mut kind, &mut expect, &mut tx, &mut rx, &mut out);
                        target = Some((val >> 10) & 0x7F);
                    }
                    (0x44, b'W') if val & (1 << 31) != 0 => {
                        if kind.is_some() {
                            flush(target, &mut kind, &mut expect, &mut tx, &mut rx, &mut out);
                        }
                        kind = Some(if val & (1 << 16) != 0 { b'R' } else { b'W' });
                        expect = Some(val & 0xFFFF);
                    }
                    (0x34, b'W') => tx.push(val as u8),
                    (0x38, b'R') => rx.push(val as u8),
                    _ => {}
                }
            }
        }
    }
    flush(target, &mut kind, &mut expect, &mut tx, &mut rx, &mut out);
    println!("{}", out.join("\n"));
    let xfers: Vec<&String> = out.iter().filter(|l| !l.starts_with('#')).collect();
    let mut targets: Vec<&str> = xfers.iter().map(|l| l.split(' ').next().unwrap()).collect();
    targets.sort();
    targets.dedup();
    eprintln!("# integrity: {bad} transfer(s) whose byte count differs from the programmed length{}", if bad == 0 { "" } else { " -- DO NOT REPLAY" });
    eprintln!("# {} transfers; targets: {}", xfers.len(), targets.join(", "));
}

/// The sensor init the driver replays (ferros_cam inputs `records`): 'W' len (u16 BE) bytes, an I2C write; 'R' n (u16 BE) addr (2 bytes), an address write and an n-byte read; 'D' µs (u32 BE), a delay.
/// The transfers to one target from a mark onward, up to (not with) its stream-on; delays as the main camera's table has had them since 2026-10-19 (10 ms after each of the first three transfers, 5 ms after a write past 256 bytes, 1 ms after every other write), since a capture has no times.
fn cmd_init(mut args: Vec<String>) {
    let markname = opt(&mut args, "--mark");
    let son = opt(&mut args, "--stream-on").unwrap_or_else(|| "01 00 01".into()).to_uppercase().replace("G#", "");
    if args.len() != 3 {
        die("init <decoded.txt> <target G#1A> <out.bin> [--mark NAME] [--stream-on \"01 00 01\"]");
    }
    let (src, target, outp) = (&args[0], format!("G#{:02X}", hexnum(&args[1])), &args[2]);
    let text = read_text(src);
    let mut started = markname.is_none();
    let mut xfers: Vec<(u8, Vec<u8>)> = Vec::new();
    for l in text.lines() {
        if let Some(m) = l.strip_prefix("# MARK") {
            if markname.as_deref() == Some(m.trim()) {
                started = true;
            }
            continue;
        }
        if !started || l.starts_with('#') || !l.starts_with(&format!("{target} ")) {
            continue;
        }
        if l.contains("...[") {
            die(&format!("summarised transfer in the decode (decode again without --max-bytes): {}", &l[..l.len().min(60)]));
        }
        let body: Vec<&str> = l.split(" #").next().unwrap().split_whitespace().collect();
        if body.len() < 2 {
            continue;
        }
        let data: Vec<u8> = body[2..].iter().map(|x| u8::from_str_radix(x, 16).unwrap_or_else(|_| die(&format!("bad byte in {l}")))).collect();
        let d = body[1].as_bytes()[0];
        if d == b'W' && hexbytes(&data) == son {
            break;
        }
        xfers.push((d, data));
    }
    let mut recs: Vec<u8> = Vec::new();
    let (mut n, mut i) = (0u32, 0usize);
    while i < xfers.len() {
        let (d, data) = (&xfers[i].0, &xfers[i].1);
        if *d == b'W' && data.len() == 2 && i + 1 < xfers.len() && xfers[i + 1].0 == b'R' {
            recs.push(b'R');
            recs.extend_from_slice(&(xfers[i + 1].1.len() as u16).to_be_bytes());
            recs.extend_from_slice(data);
            i += 2;
        } else if *d == b'W' {
            recs.push(b'W');
            recs.extend_from_slice(&(data.len() as u16).to_be_bytes());
            recs.extend_from_slice(data);
            recs.push(b'D');
            let us: u32 = if n < 3 { 10_000 } else if data.len() > 256 { 5_000 } else { 1_000 };
            recs.extend_from_slice(&us.to_be_bytes());
            i += 1;
        } else {
            recs.push(b'R');
            recs.extend_from_slice(&(data.len() as u16).to_be_bytes());
            recs.extend_from_slice(&[0, 0]);
            i += 1;
        }
        n += 1;
    }
    std::fs::write(outp, &recs).unwrap_or_else(|e| die(&format!("{outp}: {e}")));
    println!("{n} transfers before stream-on -> {} bytes in {outp}", recs.len());
}

/// The PDMA engine's page, its registers (ferros_cam).
const PDMA: u64 = 0x1C39_0000;

fn cmd_marks(args: Vec<String>) {
    let path = args.first().unwrap_or_else(|| die("marks <capture.log>"));
    for (k, line) in read_text(path).lines().enumerate() {
        if let Some(m) = mark(line) {
            println!("{}: mark {m}", k + 1);
        } else if let Some((pa, b'W', v)) = access(line) {
            match pa {
                p if p == PDMA => println!("{}: PDMA control <- G#{v:X}{}", k + 1, if v == 1 { " (the start)" } else { "" }),
                p if p == PDMA + 4 => println!("{}: PDMA ring base <- G#{v:X}", k + 1),
                p if p == PDMA + 0x10 && v <= 0x20 => println!("{}: PDMA index <- G#{v:X}", k + 1),
                0x1C02_0500 => println!("{}: SYSREG_ISPFE CSIS select <- G#{v:X}", k + 1),
                p if p >> 16 >= 0x1C04 && p >> 16 <= 0x1C0F && p & 0xFFFF == 0x4 => println!("{}: CSI link G#{:X} +4 <- G#{v:X}", k + 1, p >> 16),
                _ => {}
            }
        }
    }
}

/// The 25 address registers the PDMA program sets every frame (ferros_cam job ADDR_REGS): a CPU-written IOVA would point the front end into nothing before the program's first run, so the CPU program leaves them out.
const ADDR_ALL: [u64; 25] = [
    0x1C32_0510, 0x1C32_05C0, 0x1C32_2110, 0x1C32_21C0, 0x1C34_00D0, 0x1C35_1820, 0x1C35_48A0, 0x1C35_4A84, 0x1C35_4ECC, 0x1C35_4FC8, 0x1C35_5168, 0x1C35_5178, 0x1C35_5180, 0x1C35_5188, 0x1C35_5190,
    0x1C35_5198, 0x1C35_51A0, 0x1C35_51A8, 0x1C35_51B0, 0x1C35_51C0, 0x1C35_51C8, 0x1C35_51E0, 0x1C35_51E8, 0x1C35_51F8, 0x1C35_5200,
];

/// A camera's CPU setup from an ISP front-end capture of its open: lines [a, b) the writes before the PDMA start, (b, c) after it up to the first frame (1-based line numbers, `marks`).
/// <prefix>.cpr2: 'CPR2', 0 buffers, then three sections (u32 count, then (u32 register, u32 value) pairs): before, an empty program, after; the PHY (G#1C10_xxxx), the PDMA's own page and the 25 address registers left out.
/// <prefix>.rx.rs: the receiver start, every write to the PHY, to the link `--link` and to the CSIS select word G#1C02_0500 over [a, c), as a Rust table.
fn cmd_cpu(mut args: Vec<String>) {
    let link = opt(&mut args, "--link").map(|v| hexnum(&v)).unwrap_or(0x1C06);
    let name = opt(&mut args, "--name").unwrap_or_else(|| "RX_START".into());
    if args.len() != 5 {
        die("cpu <capture.log> <a> <b> <c> <out prefix> [--link G#1C06] [--name RX_START]");
    }
    let num = |s: &str| s.parse::<usize>().unwrap_or_else(|_| die("a, b and c are line numbers"));
    let (a, b, c) = (num(&args[1]), num(&args[2]), num(&args[3]));
    let (mut pre, mut post, mut rx) = (Vec::new(), Vec::new(), Vec::new());
    for (k, line) in read_text(&args[0]).lines().enumerate() {
        let n = k + 1;
        if n < a || n >= c {
            continue;
        }
        let Some((reg, b'W', v)) = access(line) else { continue };
        if (0x1C10_0000..0x1C11_0000).contains(&reg) || reg >> 16 == link || reg == 0x1C02_0500 {
            rx.push((reg, v));
        }
        if !(0x1C04_0000..0x1C40_0000).contains(&reg) || (PDMA..PDMA + 0x1000).contains(&reg) || (0x1C10_0000..0x1C11_0000).contains(&reg) || ADDR_ALL.contains(&reg) {
            continue;
        }
        if n < b {
            pre.push((reg, v));
        } else if n > b {
            post.push((reg, v));
        }
    }
    let sec = |l: &[(u64, u64)]| -> Vec<u8> {
        let mut o = (l.len() as u32).to_le_bytes().to_vec();
        for &(r, v) in l {
            o.extend_from_slice(&(r as u32).to_le_bytes());
            o.extend_from_slice(&(v as u32).to_le_bytes());
        }
        o
    };
    let mut cpr = b"CPR2".to_vec();
    cpr.extend_from_slice(&0u32.to_le_bytes());
    cpr.extend(sec(&pre));
    cpr.extend(sec(&[]));
    cpr.extend(sec(&post));
    let out = &args[4];
    std::fs::write(format!("{out}.cpr2"), &cpr).unwrap_or_else(|e| die(&format!("{out}.cpr2: {e}")));
    let body: Vec<String> = rx.iter().map(|&(r, v)| format!("    (0x{:04X}_{:04X}, 0x{v:X}),", r >> 16, r & 0xFFFF)).collect();
    std::fs::write(format!("{out}.rx.rs"), format!("pub const {name}: [(usize, u32); {}] = [\n{}\n];\n", rx.len(), body.join("\n"))).unwrap_or_else(|e| die(&format!("{out}.rx.rs: {e}")));
    println!("{} CPU writes before, {} after; receiver start {} writes -> {out}.cpr2, {out}.rx.rs", pre.len(), post.len(), rx.len());
}

/// Eight 8-hex words after a prefix, as the hv's dump prints them; a word's first eight characters count, since the kernel's own console line can land glued to the end of the hv's (the Pro's front capture, 2026-11-06: `00601A00[   78.661512] lwis ...`), as the Python tool's pattern read it.
fn words8(rest: &str) -> Option<Vec<u32>> {
    let w: Vec<&str> = rest.split_whitespace().take(8).map(|x| x.get(..8).unwrap_or(x)).collect();
    if w.len() != 8 || !w.iter().all(|x| is_hex(x, 8)) {
        return None;
    }
    Some(w.iter().map(|x| u32::from_str_radix(x, 16).unwrap()).collect())
}

/// The hv's PDMA dump (FERROS_HV_PDMA_DUMP): list `which` (1-based: each list starts at a "PDMA buffer" line), its words cut at the buffer's length, and the sources of its block loads by register and offset.
fn pdma_list(text: &str, which: usize) -> (Vec<u32>, BTreeMap<u32, BTreeMap<u32, u32>>) {
    let (mut words, mut loads, mut limit, mut k) = (Vec::new(), BTreeMap::<u32, BTreeMap<u32, u32>>::new(), None, 0usize);
    for ln in text.lines() {
        if ln.contains("PDMA buffer ") && !ln.contains("unmapped") {
            k += 1;
            if k == which {
                limit = ln.find("length ").and_then(|i| ln[i + 7..].split_whitespace().next()).and_then(|t| usize::from_str_radix(t, 16).ok()).map(|b| b / 4);
            }
            continue;
        }
        if k != which {
            continue;
        }
        if let Some(i) = ln.find("PDMA buf +") {
            let rest = &ln[i + 10..];
            let mut sp = rest.splitn(2, ' ');
            let off = sp.next().unwrap_or("");
            if usize::from_str_radix(off, 16).is_ok() {
                if let Some(w) = sp.next().and_then(words8) {
                    words.extend(w);
                }
            }
        }
        if let Some(i) = ln.find("PDMA load ") {
            let mut sp = ln[i + 10..].splitn(3, ' ');
            let (reg, off, rest) = (sp.next().unwrap_or(""), sp.next().unwrap_or(""), sp.next().unwrap_or(""));
            if let (Ok(reg), Some(off), Some(w)) = (u32::from_str_radix(reg, 16), off.strip_prefix('+').and_then(|o| u32::from_str_radix(o, 16).ok()), words8(rest)) {
                for (j, x) in w.into_iter().enumerate() {
                    loads.entry(reg).or_default().insert(off + 4 * j as u32, x);
                }
            }
        }
    }
    if let Some(l) = limit {
        words.truncate(l);
    }
    (words, loads)
}

/// The list's records (worked out from the 2026-10-20 dump): a header word, count = w >> 14 words follow, opcode = w & G#3FFF; opcode 2 (address, value) pairs; 1 a burst from a start address; 9 a block load {IOVA, 0, length, register}; G#B a marker.
fn cmd_pdma(mut args: Vec<String>) {
    let base = opt(&mut args, "--base").map(|v| hexnum(&v)).unwrap_or(0x1C30_0000) as u32;
    let which = opt(&mut args, "--list").map(|v| v.parse().unwrap_or_else(|_| die("--list N"))).unwrap_or(1);
    let path = args.first().unwrap_or_else(|| die("pdma <dump.log>"));
    let (words, loads) = pdma_list(&read_text(path), which);
    let mut writes: Vec<(u32, u32)> = Vec::new();
    let mut i = 0usize;
    while i < words.len() {
        let h = words[i];
        let (n, op) = ((h >> 14) as usize, h & 0x3FFF);
        let body = &words[(i + 1).min(words.len())..(i + 1 + n).min(words.len())];
        match op {
            2 => body.chunks_exact(2).for_each(|p| writes.push((p[0], p[1]))),
            1 if !body.is_empty() => body[1..].iter().enumerate().for_each(|(k, &v)| writes.push((body[0] + 4 * k as u32, v))),
            9 if body.len() == 4 => {
                let (reg, size) = (body[3], body[2]);
                let data = loads.get(&reg);
                let got = data.map(|d| d.keys().filter(|&&o| o < size).count()).unwrap_or(0);
                println!("LOAD G#{size:X} bytes from IOVA G#{:X} to G#{:X}: {}", body[0], base + reg, if got as u32 * 4 >= size { "captured".to_string() } else { format!("only {} bytes captured", got * 4) });
                if let Some(d) = data {
                    d.iter().filter(|(o, _)| **o < size).for_each(|(&o, &v)| writes.push((reg + o, v)));
                }
            }
            0xB => {}
            _ if h == 0 && words[i..(i + 8).min(words.len())].iter().all(|&w| w == 0) => break,
            _ => println!("? word {i}: G#{h:08X} (count {n}, op G#{op:X})"),
        }
        i += 1 + n;
    }
    for (a, v) in writes.iter() {
        println!("W G#{:X} = G#{v:X}", base + a);
    }
    eprintln!("({} register writes, {i} of {} words parsed)", writes.len(), words.len());
}

/// The job laid out at October's fixed addresses (tools/camera-pdma-image.py; ferros_cam job moves it into the driver's region at every open): the descriptor ring at G#8800_0000, the list at G#8801_0000, the block loads' sources from G#8802_0000 a 4 KiB slice each, every address register's 16 MiB granule mapped to its own 16 MiB slot from G#A000_0000 with its offset kept, the image body (G#1C32_05C0) always slot 2.
/// Output: 'CPR3', u32 blob count, then per blob u32 physical address, u32 length, the bytes padded to 4.
fn cmd_pdma_image(mut args: Vec<String>) {
    const RING_PA: u32 = 0x8800_0000;
    const LIST_PA: u32 = 0x8801_0000;
    const LOAD_PA: u32 = 0x8802_0000;
    const BUF_PA: u32 = 0xA000_0000;
    const BUF_SLICE: u32 = 1 << 24;
    const BUF_SLOTS: u32 = 32;
    const LOAD_SLICE: u32 = 1 << 12;
    const ISPFE: u32 = 0x1C30_0000;
    const IMAGE_REG: u32 = 0x1C32_05C0;
    const IMAGE_SLOT: u32 = 2;
    const RING_ENTRIES: usize = 1024;
    let which = opt(&mut args, "--list").map(|v| v.parse().unwrap_or_else(|_| die("--list N"))).unwrap_or(1);
    // --own-image: the image body alone in slot 2, at its start, whatever else shares its captured granule (akita's main puts its PD output at the granule's start and the 20.2 MB image 5 MiB in, 2026-11-15: a ring of image buffers in slot 2 would have the PD output written over buffer 0); every other address keeps its granule's slot and offset as before.
    let own_image = args.iter().position(|a| a == "--own-image").map(|i| args.remove(i)).is_some();
    if args.len() != 2 {
        die("pdma-image <dump.log> <out.cpr3> [--list N] [--own-image]");
    }
    let (mut words, loads) = pdma_list(&read_text(&args[0]), which);
    if words.is_empty() {
        die("no list words in the dump");
    }
    // With --own-image, a last record that would run past the buffer is left out, the only change to the captured bytes: the engine stops at the buffer's length, so it never ran it (akita's main, list 1 of 2026-11-15: its buffer's last four words repeat the record before them under a header G#0004_0000, a stale tail that ferros_cam job's walker refuses as truncated). Zero words are stepped over as single words, not taken as the end: akita's front list has zeros before its last record (the commit writes).
    if own_image {
        let mut i = 0usize;
        while i < words.len() {
            let h = words[i];
            let n = if h == 0 { 0 } else { (h >> 14) as usize };
            if i + 1 + n > words.len() {
                break;
            }
            i += 1 + n;
        }
        if i < words.len() {
            println!("program ends at word {i} of {}: the {} words after it left out (first G#{:08X})", words.len(), words.len() - i, words[i]);
            words.truncate(i);
        }
    }
    let is_addr = |r: u32| ADDR_ALL.contains(&(r as u64));
    // Granule (bits 31:24 of the captured IOVA) to slot, in first-seen order, the image's granule claimed for slot 2 first.
    let mut slot_of: Vec<(u32, u32)> = Vec::new();
    let mut owners: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut j = 0usize;
    while j < words.len() {
        let h = words[j];
        let (n, op) = ((h >> 14) as usize, h & 0x3FFF);
        if op == 2 {
            let mut k2 = j + 2;
            while k2 < j + 1 + n && k2 < words.len() {
                if !own_image && ISPFE + words[k2 - 1] == IMAGE_REG && words[k2] != 0 && !slot_of.iter().any(|s| s.0 == words[k2] >> 24) {
                    slot_of.push((words[k2] >> 24, IMAGE_SLOT));
                }
                k2 += 2;
            }
        } else if h == 0 {
            break;
        }
        j += 1 + n;
    }
    let mut remap = |reg: u32, v: u32, slot_of: &mut Vec<(u32, u32)>| -> u32 {
        if own_image && reg == IMAGE_REG {
            let pa = BUF_PA + IMAGE_SLOT * BUF_SLICE;
            owners.entry(pa).or_default().push(reg);
            return pa;
        }
        let g = v >> 24;
        let slot = match slot_of.iter().find(|s| s.0 == g) {
            Some(s) => s.1,
            None => {
                let s = if reg == IMAGE_REG { IMAGE_SLOT } else { (0..BUF_SLOTS).find(|k| *k != IMAGE_SLOT && !slot_of.iter().any(|s| s.1 == *k)).unwrap_or_else(|| die("more output granules than slots")) };
                slot_of.push((g, s));
                s
            }
        };
        let pa = BUF_PA + slot * BUF_SLICE + (v & 0xFF_FFFF);
        owners.entry(pa).or_default().push(reg);
        pa
    };
    let mut load_order: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut i = 0usize;
    while i < words.len() {
        let h = words[i];
        let (n, op) = ((h >> 14) as usize, h & 0x3FFF);
        if op == 2 {
            let mut k2 = i + 2;
            while k2 < i + 1 + n && k2 < words.len() {
                let reg = ISPFE + words[k2 - 1];
                if is_addr(reg) && words[k2] != 0 {
                    words[k2] = remap(reg, words[k2], &mut slot_of);
                }
                k2 += 2;
            }
        } else if op == 1 && n > 0 {
            for k2 in i + 2..(i + 1 + n).min(words.len()) {
                let reg = ISPFE + words[i + 1] + 4 * (k2 - i - 2) as u32;
                if is_addr(reg) && words[k2] != 0 {
                    words[k2] = remap(reg, words[k2], &mut slot_of);
                }
            }
        } else if op == 9 && n == 4 {
            let (reg, size) = (words[i + 4], words[i + 3]);
            let data = loads.get(&reg);
            let payload: Vec<u8> = (0..size).step_by(4).flat_map(|o| data.and_then(|d| d.get(&o)).copied().unwrap_or(0).to_le_bytes()).collect();
            let pa = LOAD_PA + load_order.iter().map(|(_, p)| (p.len() as u32).div_ceil(LOAD_SLICE)).sum::<u32>() * LOAD_SLICE;
            if data.map(|d| d.keys().filter(|&&o| o < size).count()).unwrap_or(0) as u32 * 4 < size {
                println!("warning: block load for G#{reg:X} has only part of its G#{size:X} bytes captured");
            }
            load_order.push((pa, payload));
            words[i + 1] = pa;
        } else if h == 0 {
            break;
        }
        i += 1 + n;
    }
    let list: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut ring = Vec::with_capacity(RING_ENTRIES * 16);
    for _ in 0..RING_ENTRIES {
        for w in [0xC003u32, LIST_PA, 0, list.len() as u32] {
            ring.extend_from_slice(&w.to_le_bytes());
        }
    }
    let mut blobs = vec![(RING_PA, ring), (LIST_PA, list.clone())];
    blobs.extend(load_order.iter().cloned());
    let mut out = b"CPR3".to_vec();
    out.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    for (pa, b) in blobs.iter() {
        out.extend_from_slice(&pa.to_le_bytes());
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
        out.resize(out.len() + (4 - b.len() % 4) % 4, 0);
    }
    std::fs::write(&args[1], &out).unwrap_or_else(|e| die(&format!("{}: {e}", args[1])));
    println!("list {} B, {} block-load sources, {} output granules to G#{BUF_PA:X}.., G#{BUF_SLICE:X} a slot over {BUF_SLOTS} slots", list.len(), load_order.len(), slot_of.len());
    println!("ring {RING_ENTRIES} entries at G#{RING_PA:X}; total {} bytes -> {}", out.len(), args[1]);
    for (pa, regs) in owners.iter() {
        println!("  slot G#{pa:X}: {}", regs.iter().map(|r| format!("G#{r:X}")).collect::<Vec<_>>().join(" "));
    }
    // For a camera whose outputs are not measured yet (ferros_cam job's sizes and skips): each slot from its lowest named offset to its granule's end, in whole frames of the pool (2 MiB), the image slot left 0 (the driver's ring fills it).
    let mut sizes = [0u32; BUF_SLOTS as usize];
    let mut skips = [0u32; BUF_SLOTS as usize];
    for pa in owners.keys() {
        let (k, off) = (((pa - BUF_PA) / BUF_SLICE) as usize, (pa - BUF_PA) % BUF_SLICE);
        if k as u32 == IMAGE_SLOT && own_image {
            continue;
        }
        if sizes[k] == 0 || off < skips[k] {
            skips[k] = off;
            sizes[k] = (BUF_SLICE - off).div_ceil(1 << 21) << 21;
        }
    }
    let mib = |v: u32| if v == 0 { "0".to_string() } else { format!("{} * MIB", v >> 20) };
    println!("sizes: [{}]", sizes.iter().map(|&v| mib(v)).collect::<Vec<_>>().join(", "));
    println!("skips: [{}]", skips.iter().map(|&v| format!("0x{v:X}")).collect::<Vec<_>>().join(", "));
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        die("i2c | init | marks | cpu | pdma | pdma-image (see the source's header)");
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "i2c" => cmd_i2c(args),
        "init" => cmd_init(args),
        "marks" => cmd_marks(args),
        "cpu" => cmd_cpu(args),
        "pdma" => cmd_pdma(args),
        "pdma-image" => cmd_pdma_image(args),
        _ => die(&format!("unknown command {cmd}")),
    }
}
