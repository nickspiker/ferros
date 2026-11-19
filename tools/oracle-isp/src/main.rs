//! oracle-isp: an hv oracle capture of the ISP front end turned into the camera driver's inputs, in Rust (Python is not allowed here).
//! Three subcommands, each the Python tool it replaces, byte for byte in what they write:
//! - `cpu <capture.log> <a> <b> <c> <out prefix> [--link G#1C06] [--name RX_START_UW]` (tools/camera-cpu.py): from the 42-page capture's lines [a, b) (the CPU setup before the PDMA start) and (b, c) (after it, up to the first frame), `<out>.cpr2` ('CPR2' with no buffers: the CPU writes to the CSI links and the front end before, an empty program, the ones after; the PHY G#1C10_xxxx, the PDMA's page and the 25 address registers left out) and `<out>.rx.rs` (the receiver start as a Rust table: every write to the PHY, to the link `--link` and to the CSIS reset word G#1C02_0500 over [a, c), in the capture's order).
//! - `pdma <dump.log> [--list N] [--base G#1C30_0000]` (tools/oracle-decode-pdma.py): the front end's PDMA register-write list from the hv's dump (FERROS_HV_PDMA_DUMP, `PDMA buf +<off> w0..w7` lines), decoded to `W G#<register> = G#<value>` lines, block loads expanded where the dump carries their sources.
//! - `image <dump.log> <out> [--list N] [--height N]` (tools/camera-pdma-image.py): the PDMA job, a 'CPR3', every address register remapped to a fixed slice, each block load's source in its own 4 KiB-aligned slice.
//! Line numbers (a, b, c) are the file's, from 1, as `grep -n` prints them.
//! Every file these write holds the vendor's settings: they stay local (target/cam, keys/), never in the repository.

use std::collections::BTreeMap;

/// The ISP front end's block (zuma-isp.dtsi lwis_isp_fe); the PDMA list's addresses are offsets into it, and the PDMA itself is at +G#9_0000.
const ISPFE: u64 = 0x1C30_0000;

/// The front end's 25 address registers, measured (the main's list plus the hv's live Linux snapshot, 2026-10-22): the program sets them every frame, so a CPU write of one is never replayed, and the job builder moves each into a slice.
const ADDR_ALL: [u64; 25] = [
    0x1C32_0510, 0x1C32_05C0, 0x1C32_2110, 0x1C32_21C0, 0x1C34_00D0, 0x1C35_1820, 0x1C35_48A0, 0x1C35_4A84, 0x1C35_4ECC, 0x1C35_4FC8, 0x1C35_5168, 0x1C35_5178, 0x1C35_5180, 0x1C35_5188, 0x1C35_5190, 0x1C35_5198, 0x1C35_51A0, 0x1C35_51A8, 0x1C35_51B0, 0x1C35_51C0, 0x1C35_51C8, 0x1C35_51E0, 0x1C35_51E8, 0x1C35_51F8, 0x1C35_5200,
];

fn is_addr(reg: u64) -> bool {
    ADDR_ALL.contains(&reg)
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim().trim_start_matches("G#").trim_start_matches("0x").replace('_', "").as_str(), 16).ok()
}

/// Upper-case hex digits only, as the hv prints them.
fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

// ---- cpu ----

/// `S2 <16 hex> W4 <16 hex>` anywhere in a line: a 4-byte write the oracle trapped (no `rep` runs expanded: camera-cpu.py never did, and a write repeated is the same write).
fn w4(line: &str) -> Option<(u64, u64)> {
    let k = line.find("S2 ")?;
    let mut t = line[k + 3..].split(' ');
    let (pa, op, val) = (t.next()?, t.next()?, t.next()?);
    let val = &val[..val.len().min(16)];
    if op != "W4" || pa.len() != 16 || val.len() != 16 || !is_hex(pa) || !is_hex(val) {
        return None;
    }
    Some((hex(pa)?, hex(val)?))
}

/// The CPU setup and the receiver table out of a 42-page capture's lines; see the header.
pub struct Cpu {
    pub pre: Vec<(u64, u64)>,
    pub post: Vec<(u64, u64)>,
    pub rx: Vec<(u64, u64)>,
    /// Integrity: oracle lines in [a, c) that are neither a whole access (`S2 <pa> <R|W><n> <value>`) nor a `rep` run (bytes lost where the hv and the kernel append to the console ring together: memory feedback_oracle_capture_integrity), and `rep` runs (a repeated access: a repeated write is never in a setup, so any inside the window is worth a look).
    pub garbled: Vec<usize>,
    pub reps: usize,
}

/// A whole oracle access line, `S2 <16 hex> <R|W><width> <16 hex>`, or a `rep` run: anything else carrying `ferros hv: S2` lost bytes.
fn whole(line: &str) -> bool {
    let Some(k) = line.find("ferros hv: S2 ") else { return true };
    let t: Vec<&str> = line[k + 14..].split(' ').collect();
    if t.first() == Some(&"rep") {
        return t.len() == 2 && is_hex(t[1]);
    }
    t.len() == 3 && t[0].len() == 16 && is_hex(t[0]) && (t[1] == "R4" || t[1] == "W4" || t[1] == "R8" || t[1] == "W8" || t[1] == "R2" || t[1] == "W2" || t[1] == "R1" || t[1] == "W1") && t[2].trim_end().len() == 16 && is_hex(t[2].trim_end())
}

/// `sysreg`: the receiver table also takes every other SYSREG_ISPFE word written in the window (G#1C02_0504: the tele's G#4, its PHY's pairing; camera-cpu.py took G#1C02_0500 alone, which the ultrawide's and front's tables were made with).
pub fn cpu(text: &str, a: usize, b: usize, c: usize, link: u64, sysreg: bool) -> Cpu {
    let (mut pre, mut post, mut rx, mut garbled, mut reps) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0);
    for (k, line) in text.split('\n').enumerate() {
        let n = k + 1;
        if n < a || n >= c {
            continue;
        }
        if !whole(line) {
            garbled.push(n);
        }
        if line.contains("ferros hv: S2 rep ") {
            reps += 1;
        }
        let Some((reg, v)) = w4(line) else { continue };
        let phy = (0x1C10_0000..0x1C11_0000).contains(&reg);
        if phy || reg >> 16 == link || reg == 0x1C02_0500 || (sysreg && (0x1C02_0504..0x1C02_0600).contains(&reg)) {
            rx.push((reg, v));
        }
        if !(0x1C04_0000..0x1C40_0000).contains(&reg) || (0x1C39_0000..0x1C39_1000).contains(&reg) || phy || is_addr(reg) {
            continue;
        }
        if n < b {
            pre.push((reg, v));
        } else if n > b {
            post.push((reg, v));
        }
    }
    Cpu { pre, post, rx, garbled, reps }
}

/// 'CPR2', no buffers, the writes before, an empty program, the writes after: each section a u32 count, then (register, value) as u32 pairs, little-endian.
pub fn cpr2(c: &Cpu) -> Vec<u8> {
    let mut out = b"CPR2".to_vec();
    out.extend(0u32.to_le_bytes());
    for sec in [&c.pre[..], &[][..], &c.post[..]] {
        out.extend((sec.len() as u32).to_le_bytes());
        for &(r, v) in sec {
            out.extend((r as u32).to_le_bytes());
            out.extend((v as u32).to_le_bytes());
        }
    }
    out
}

/// The receiver start as ferros_cam rx holds it (Rust source: its literals are Rust's own).
pub fn rx_rs(name: &str, rx: &[(u64, u64)]) -> String {
    let body: Vec<String> = rx.iter().map(|&(r, v)| format!("    (0x{:04X}_{:04X}, 0x{v:X}),", r >> 16, r & 0xFFFF)).collect();
    format!("pub const {name}: [(usize, u32); {}] = [\n{}\n];\n", rx.len(), body.join("\n"))
}

// ---- the dump ----

/// One list of the dump: its words (cut to the length its `PDMA buffer` line names, if it names one) and the sources of its block loads by register, offset to word.
pub struct Dump {
    pub words: Vec<u32>,
    pub limit: Option<usize>,
    pub loads: BTreeMap<u32, BTreeMap<u32, u32>>,
    /// Integrity: list lines whose offset is not the one after the line before (a line lost from the console ring shifts every word after it), and lines naming the dump that did not parse whole.
    pub gaps: Vec<u64>,
    pub bad: usize,
}

/// Eight words of 8 hex digits after a `+<offset>`.
fn eight(rest: &str) -> Option<Vec<u32>> {
    let t: Vec<&str> = rest.split(' ').collect();
    if t.len() < 8 {
        return None;
    }
    let mut w = Vec::with_capacity(8);
    for (j, s) in t[..8].iter().enumerate() {
        let s = if j == 7 { &s[..s.len().min(8)] } else { s };
        if s.len() != 8 || !is_hex(s) {
            return None;
        }
        w.push(u32::from_str_radix(s, 16).ok()?);
    }
    Some(w)
}

/// The dump's lists each begin at a `PDMA buffer <iova> length <bytes>` line (one marked unmapped is not a list); `which` picks the list, from 1, as the Python tools counted (and as every past invocation, `--list 2`, names it).
pub fn dump(text: &str, which: usize) -> Dump {
    let (mut words, mut loads, mut limit, mut k) = (Vec::new(), BTreeMap::<u32, BTreeMap<u32, u32>>::new(), None, 0usize);
    let (mut gaps, mut bad) = (Vec::new(), 0);
    for line in text.split('\n') {
        if line.contains("PDMA buffer ") && !line.contains("unmapped") {
            k += 1;
            if k == which {
                limit = line.find("length ").and_then(|p| {
                    let h: String = line[p + 7..].chars().take_while(|c| c.is_ascii_digit() || ('A'..='F').contains(c)).collect();
                    if h.is_empty() { None } else { u64::from_str_radix(&h, 16).ok().map(|v| (v / 4) as usize) }
                });
            }
            continue;
        }
        if k != which {
            continue;
        }
        if let Some(p) = line.find("PDMA buf +") {
            let rest = &line[p + 10..];
            if let Some(sp) = rest.find(' ')
                && is_hex(&rest[..sp])
                && let Some(w) = eight(&rest[sp + 1..])
            {
                let off = hex(&rest[..sp]).unwrap();
                if off != 4 * words.len() as u64 {
                    gaps.push(off);
                }
                words.extend(w);
            } else {
                bad += 1;
            }
        }
        if let Some(p) = line.find("PDMA load ") {
            let t: Vec<&str> = line[p + 10..].splitn(3, ' ').collect();
            if t.len() == 3
                && is_hex(t[0])
                && let Some(off) = t[1].strip_prefix('+')
                && is_hex(off)
                && let Some(w) = eight(t[2])
            {
                let (reg, off) = (hex(t[0]).unwrap() as u32, hex(off).unwrap() as u32);
                let m = loads.entry(reg).or_default();
                if off as usize != 4 * m.len() {
                    gaps.push(off as u64);
                }
                for (j, x) in w.into_iter().enumerate() {
                    m.insert(off + 4 * j as u32, x);
                }
            } else {
                bad += 1;
            }
        }
    }
    Dump { words, limit, loads, gaps, bad }
}

// ---- pdma (the decoder) ----

/// The list read as register writes (offsets into the front end), with the decoder's notes (block loads, unknown records) in the order met, and how many of the words were parsed.
pub fn decode(d: &Dump, base: u64) -> (Vec<(u32, u32)>, Vec<String>, usize, usize) {
    let words: &[u32] = match d.limit {
        Some(l) => &d.words[..l.min(d.words.len())],
        None => &d.words,
    };
    let (mut writes, mut notes) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < words.len() {
        let h = words[i];
        let (n, op) = ((h >> 14) as usize, h & 0x3FFF);
        let body = &words[(i + 1).min(words.len())..(i + 1 + n).min(words.len())];
        if op == 2 {
            let mut k = 0;
            while k + 1 < body.len() {
                writes.push((body[k], body[k + 1]));
                k += 2;
            }
        } else if op == 1 && !body.is_empty() {
            for (k, &v) in body[1..].iter().enumerate() {
                writes.push((body[0].wrapping_add(4 * k as u32), v));
            }
        } else if op == 9 && body.len() == 4 {
            let (reg, size) = (body[3], body[2]);
            let empty = BTreeMap::new();
            let data = d.loads.get(&reg).unwrap_or(&empty);
            let got = data.keys().filter(|&&o| o < size).count() as u64;
            let how = if got * 4 >= size as u64 { "captured".to_string() } else { format!("only {} bytes captured", got * 4) };
            notes.push(format!("LOAD G#{size:X} bytes from IOVA G#{:X} to G#{:X}: {how}", body[0], base + reg as u64));
            for (&o, &v) in data {
                if o < size {
                    writes.push((reg + o, v));
                }
            }
        } else if op == 0xB {
        } else if h == 0 && words[i..(i + 8).min(words.len())].iter().all(|&w| w == 0) {
            break;
        } else {
            notes.push(format!("? word {i}: G#{h:08X} (count {n}, op G#{op:X})"));
        }
        i += 1 + n;
    }
    (writes, notes, i, words.len())
}

// ---- image (the job) ----

const RING_PA: u32 = 0x8800_0000;
const LIST_PA: u32 = 0x8801_0000;
const LOAD_PA: u32 = 0x8802_0000;
/// 32 slices of 16 MiB from G#A000_0000, one per output (nothing reserved from G#9700_0000 to G#E800_0000); the driver moves them again into its own region (ferros_cam job).
const BUF_PA: u32 = 0xA000_0000;
const BUF_SLICE: u32 = 0x100_0000;
const BUF_SLOTS: u32 = 32;
const LOAD_SLICE: usize = 0x1000;
/// The image body always takes slot 2, so it lands at G#A200_0000 for every camera.
const IMAGE_REG: u64 = 0x1C32_05C0;
const IMAGE_SLOT: u32 = 2;
/// The descriptor ring's length (Linux: 1000).
const RING_ENTRIES: usize = 1024;
/// The captured main mode's height, the one `--height` rewrites from.
const HEIGHT_FULL: u32 = 3072;

/// The job and what it says about itself.
pub struct Job {
    pub file: Vec<u8>,
    pub report: Vec<String>,
}

/// The job from list `d` (see the header): the ring, the list with every nonzero address register moved to its 16 MiB granule's slot (offset kept), each block load's source in its own slice; `height` rewrites the height-derived halves of every other word.
pub fn image(d: &Dump, height: Option<u32>) -> Job {
    let mut words: Vec<u32> = match d.limit {
        Some(l) if l > 0 => d.words[..l.min(d.words.len())].to_vec(),
        _ => d.words.clone(),
    };
    assert!(!words.is_empty(), "no list words in the dump");
    let regeom = |v: u32| -> u32 {
        let Some(h) = height else { return v };
        let m = |x: u32| match x {
            HEIGHT_FULL => h,
            x if x == HEIGHT_FULL / 2 => h / 2,
            x if x == HEIGHT_FULL / 4 => h / 4,
            x => x,
        };
        (m(v >> 16) << 16) | m(v & 0xFFFF)
    };
    // Granule (v >> 24) to slot, in the order first met; buffer value to its new address; new address to the registers that point there.
    let mut slot_of: Vec<(u32, u32)> = Vec::new();
    let mut bufs: BTreeMap<u32, u32> = BTreeMap::new();
    let mut owners: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    let find = |slot_of: &Vec<(u32, u32)>, g: u32| slot_of.iter().find(|e| e.0 == g).map(|e| e.1);
    let mut remap = |slot_of: &mut Vec<(u32, u32)>, reg: u64, v: u32| -> u32 {
        let g = v >> 24;
        let slot = match find(slot_of, g) {
            Some(s) => s,
            None => {
                let s = if reg == IMAGE_REG { IMAGE_SLOT } else { (0..BUF_SLOTS).find(|k| *k != IMAGE_SLOT && !slot_of.iter().any(|e| e.1 == *k)).expect("more granules than slots") };
                slot_of.push((g, s));
                s
            }
        };
        let pa = BUF_PA + slot * BUF_SLICE + (v & 0xFF_FFFF);
        bufs.entry(v).or_insert(pa);
        owners.entry(pa).or_default().push(reg);
        pa
    };
    // The image register's granule claims slot 2 before anything else claims a slot.
    let mut j = 0;
    while j < words.len() {
        let h = words[j];
        let (n, op) = ((h >> 14) as usize, h & 0x3FFF);
        if op == 2 {
            let mut k2 = j + 2;
            while k2 < j + 1 + n {
                if ISPFE + words[k2 - 1] as u64 == IMAGE_REG && words[k2] != 0 && find(&slot_of, words[k2] >> 24).is_none() {
                    slot_of.push((words[k2] >> 24, IMAGE_SLOT));
                }
                k2 += 2;
            }
        } else if h == 0 {
            break;
        }
        j += 1 + n;
    }
    let (mut geom, mut loads): (usize, Vec<(u32, Vec<u8>)>) = (0, Vec::new());
    let mut short = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let h = words[i];
        let (n, op) = ((h >> 14) as usize, h & 0x3FFF);
        if op == 2 || (op == 1 && n > 0) {
            let step = if op == 2 { 2 } else { 1 };
            let mut k2 = i + 2;
            while k2 < i + 1 + n {
                let reg = if op == 2 { ISPFE + words[k2 - 1] as u64 } else { ISPFE + words[i + 1] as u64 + 4 * (k2 - i - 2) as u64 };
                let v = words[k2];
                if is_addr(reg) && v != 0 {
                    words[k2] = remap(&mut slot_of, reg, v);
                } else {
                    let g = regeom(v);
                    if g != v {
                        words[k2] = g;
                        geom += 1;
                    }
                }
                k2 += step;
            }
        } else if op == 9 && n == 4 {
            let (reg, size) = (words[i + 4], words[i + 3]);
            let empty = BTreeMap::new();
            let data = d.loads.get(&reg).unwrap_or(&empty);
            let mut payload = Vec::with_capacity(size as usize + 3);
            let mut got = 0;
            for o in (0..size).step_by(4) {
                match data.get(&o) {
                    Some(&v) => {
                        payload.extend(v.to_le_bytes());
                        got += 4;
                    }
                    None => payload.extend(0u32.to_le_bytes()),
                }
            }
            let slices: usize = loads.iter().map(|(_, p)| p.len().div_ceil(LOAD_SLICE)).sum();
            let pa = LOAD_PA + (slices * LOAD_SLICE) as u32;
            words[i + 1] = pa;
            if got < size {
                short.push(format!("warning: block load for G#{reg:X} has {got} of {size} bytes captured (the rest zeros)"));
            }
            loads.push((pa, payload));
        } else if h == 0 {
            break;
        }
        i += 1 + n;
    }
    let list: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut ring = Vec::with_capacity(RING_ENTRIES * 16);
    for _ in 0..RING_ENTRIES {
        for w in [0xC003, LIST_PA, 0, list.len() as u32] {
            ring.extend(w.to_le_bytes());
        }
    }
    let mut blobs = vec![(RING_PA, ring), (LIST_PA, list.clone())];
    blobs.extend(loads.iter().cloned());
    let mut file = b"CPR3".to_vec();
    file.extend((blobs.len() as u32).to_le_bytes());
    for (pa, b) in &blobs {
        file.extend(pa.to_le_bytes());
        file.extend((b.len() as u32).to_le_bytes());
        file.extend(b);
        file.extend(std::iter::repeat_n(0u8, (4 - b.len() % 4) % 4));
    }
    let mut report = short;
    if height.is_some() {
        report.push(format!("geometry rewritten from height {HEIGHT_FULL} to {} in {geom} words", height.unwrap()));
    }
    report.push(format!(
        "list {} B, {} block-load sources, {} output buffers remapped from G#{:X}.. to G#{BUF_PA:X}.., G#{BUF_SLICE:X} a slice over {BUF_SLOTS} slices",
        list.len(),
        loads.len(),
        bufs.len(),
        bufs.keys().next().copied().unwrap_or(0)
    ));
    report.push(format!("ring {RING_ENTRIES} entries at G#{RING_PA:X}; total {} bytes", file.len()));
    for (pa, regs) in &owners {
        let r: Vec<String> = regs.iter().map(|r| format!("G#{r:X}")).collect();
        report.push(format!("  slice G#{pa:X}: {}", r.join(" ")));
    }
    Job { file, report }
}

/// Every write the job's program (its piece at `LIST_PA`) makes to one of `sets`' registers (offsets into the front end) given that value; how many words changed.
pub fn set_regs(file: &mut [u8], sets: &[(u32, u32)]) -> usize {
    let r32 = |f: &[u8], o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    let n = r32(file, 4) as usize;
    let mut o = 8;
    let mut list = None;
    for _ in 0..n {
        let (pa, len) = (r32(file, o), r32(file, o + 4) as usize);
        if pa == LIST_PA {
            list = Some((o + 8, len / 4));
        }
        o += 8 + len.div_ceil(4) * 4;
    }
    let (at, words) = list.expect("no program piece in the job");
    let mut changed = 0;
    let mut put = |f: &mut [u8], k: usize, reg: u32| {
        if let Some(&(_, v)) = sets.iter().find(|s| s.0 == reg) {
            let p = at + 4 * k;
            if r32(f, p) != v {
                f[p..p + 4].copy_from_slice(&v.to_le_bytes());
                changed += 1;
            }
        }
    };
    let mut i = 0;
    while i < words {
        let h = r32(file, at + 4 * i);
        let (cnt, op) = ((h >> 14) as usize, h & 0x3FFF);
        if h == 0 || i + 1 + cnt > words {
            break;
        }
        if op == 2 {
            let mut k = i + 1;
            while k + 1 <= i + cnt {
                let reg = r32(file, at + 4 * k);
                put(file, k + 1, reg);
                k += 2;
            }
        } else if op == 1 && cnt > 0 {
            let start = r32(file, at + 4 * (i + 1));
            for j in 0..cnt - 1 {
                put(file, i + 2 + j, start + 4 * j as u32);
            }
        }
        i += 1 + cnt;
    }
    changed
}

// ---- main ----

/// The dump's integrity line: the list's words against the length its `PDMA buffer` line names, offsets that skip, lines that did not parse.
fn dump_integrity(d: &Dump) -> String {
    let gaps: Vec<String> = d.gaps.iter().take(8).map(|g| format!("+G#{g:X}")).collect();
    format!(
        "integrity: list words A#{} (its length names {}), offsets out of order A#{} {}, dump lines not whole A#{}, block-load registers A#{}",
        d.words.len(),
        d.limit.map(|l| format!("A#{l}")).unwrap_or_else(|| "none".into()),
        d.gaps.len(),
        gaps.join(" "),
        d.bad,
        d.loads.len()
    )
}

fn flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    let v = args.get(i + 1).unwrap_or_else(|| panic!("{name} wants a value")).clone();
    args.drain(i..i + 2);
    Some(v)
}

fn read(path: &str) -> String {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    String::from_utf8_lossy(&b).into_owned()
}

fn write(path: &str, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "oracle-isp cpu <capture.log> <a> <b> <c> <out prefix> [--link G#1C06] [--name RX_START_UW] | pdma <dump.log> [--list N] [--base G#1C30_0000] | image <dump.log> <out> [--list N] [--height N]";
    if args.is_empty() {
        eprintln!("{usage}");
        std::process::exit(2);
    }
    match args.remove(0).as_str() {
        "cpu" => {
            let link = flag(&mut args, "--link").map(|s| hex(&s).expect("--link G#<link>")).unwrap_or(0x1C06);
            let name = flag(&mut args, "--name").unwrap_or_else(|| "RX_START_UW".to_string());
            let num = |s: &String| s.parse::<usize>().unwrap_or_else(|_| panic!("{s}: a line number"));
            let sysreg = args.iter().position(|x| x == "--sysreg").map(|i| args.remove(i)).is_some();
            let [src, a, b, c, out] = &args[..] else { panic!("{usage}") };
            let r = cpu(&read(src), num(a), num(b), num(c), link, sysreg);
            write(&format!("{out}.cpr2"), &cpr2(&r));
            write(&format!("{out}.rx.rs"), rx_rs(&name, &r.rx).as_bytes());
            println!("{} CPU writes before, {} after; receiver start {} writes -> {out}.cpr2, {out}.rx.rs", r.pre.len(), r.post.len(), r.rx.len());
            let first: Vec<String> = r.garbled.iter().take(8).map(|n| n.to_string()).collect();
            eprintln!("integrity: oracle lines not whole in the window A#{} (lines {}), rep runs A#{}", r.garbled.len(), first.join(" "), r.reps);
        }
        // `cpr2-set <in.cpr2> <out.cpr2> G#<register>=G#<value> ...`: every write a CPU program makes to that register given that value (a device test of one word, docs/CAMERA.md "Every readout, captured").
        "cpr2-set" => {
            let [src, out, sets @ ..] = &args[..] else { panic!("{usage}") };
            let mut b = std::fs::read(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            let r32 = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            assert_eq!(&b[..4], b"CPR2", "{src}: not a CPR2");
            let mut changed = 0;
            for s in sets {
                let (r, v) = s.split_once('=').expect("G#<register>=G#<value>");
                let (r, v) = (hex(r).expect("a register") as u32, hex(v).expect("a value") as u32);
                let mut o = 8;
                for _ in 0..3 {
                    let n = r32(&b, o) as usize;
                    for k in 0..n {
                        let p = o + 4 + 8 * k;
                        if r32(&b, p) == r {
                            b[p + 4..p + 8].copy_from_slice(&v.to_le_bytes());
                            changed += 1;
                        }
                    }
                    o += 4 + 8 * n;
                }
            }
            write(out, &b);
            println!("A#{changed} writes given their set values -> {out}");
        }
        "pdma" => {
            let base = flag(&mut args, "--base").map(|s| hex(&s).expect("--base G#<block>")).unwrap_or(ISPFE);
            let which = flag(&mut args, "--list").map(|s| s.parse().expect("--list N")).unwrap_or(1);
            let text = match args.first() {
                Some(p) => read(p),
                None => std::io::read_to_string(std::io::stdin()).expect("stdin"),
            };
            let d = dump(&text, which);
            eprintln!("{}", dump_integrity(&d));
            let (writes, notes, parsed, total) = decode(&d, base);
            let mut out = String::new();
            for n in &notes {
                out.push_str(n);
                out.push('\n');
            }
            for (a, v) in &writes {
                out.push_str(&format!("W G#{:X} = G#{v:X}\n", base + *a as u64));
            }
            print!("{out}");
            eprintln!("({} register writes, {parsed} of {total} words parsed)", writes.len());
        }
        "image" => {
            let which = flag(&mut args, "--list").map(|s| s.parse().expect("--list N")).unwrap_or(1);
            let height = flag(&mut args, "--height").map(|s| s.parse().expect("--height N"));
            // `--set G#<register>=G#<value>` (repeatable): every write the program makes to that register (pairs and bursts) given that value, after the remap and the height: the tele's image path writes 16-bit samples (format G#1C32_0550 = 2, stride G#1F80), which `--set G#1C320550=G#4 --set G#1C320554=G#13C0` makes RAW10 packed as every other camera's.
            let mut sets = Vec::new();
            while let Some(s) = flag(&mut args, "--set") {
                let (r, v) = s.split_once('=').expect("--set G#<register>=G#<value>");
                sets.push(((hex(r).expect("a register") - ISPFE) as u32, hex(v).expect("a value") as u32));
            }
            let [src, out] = &args[..] else { panic!("{usage}") };
            let d = dump(&read(src), which);
            eprintln!("{}", dump_integrity(&d));
            let mut job = image(&d, height);
            if !sets.is_empty() {
                let n = set_regs(&mut job.file, &sets);
                job.report.push(format!("--set: A#{n} program words given their set values"));
            }
            write(out, &job.file);
            for l in &job.report {
                println!("{l}");
            }
            println!("-> {out}");
        }
        _ => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dump of two lists in the hv's words: list 1 a marker, a pair record (an address register and a geometry word), a burst, a block load with its source; list 2 a single pair.
    fn sample() -> String {
        let l1: [u32; 25] = [
            0x0000_400B, 0, // marker, one word
            0x0001_0002, 0x0002_05C0, 0x1D2A_3000, 0x0002_0554, 0x0C00_0FF0, // pairs: the image body, a frame size
            0x0001_0001, 0x0005_51C0, 0xFA00_0000, 0x0C00_0C00, 0xFA02_EE00, // burst from G#551C0: two address registers around a geometry word
            0x0001_0009, 0x1D00_0000, 0, 0x10, 0x0006_95FC, // block load of 16 bytes
            0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut s = String::from("ferros hv: PDMA buffer 000000001D5D4000 length 0000000000000064\n");
        for (k, c) in l1.chunks(8).enumerate() {
            let mut w: Vec<String> = c.iter().map(|w| format!("{w:08X}")).collect();
            w.resize(8, "00000000".into());
            s += &format!("ferros hv: PDMA buf +{:016X} {}\n", k * 32, w.join(" "));
        }
        s += "ferros hv: PDMA load 00000000000695FC +0000000000000000 00000001 00000002 00000003 00000004 00000005 00000006 00000007 00000008\n";
        s += "ferros hv: PDMA buffer 000000001C5FC000 length 000000000000000C\n";
        s += "ferros hv: PDMA buf +0000000000000000 00008002 00020110 1D2A3000 00000000 00000000 00000000 00000000 00000000\n";
        s
    }

    #[test]
    fn the_decoder() {
        let d = dump(&sample(), 1);
        assert_eq!(d.limit, Some(25));
        let (w, notes, parsed, total) = decode(&d, ISPFE);
        assert_eq!(total, 25);
        assert_eq!(parsed, 17);
        assert_eq!(notes, vec!["LOAD G#10 bytes from IOVA G#1D000000 to G#1C3695FC: captured".to_string()]);
        assert_eq!(w, vec![(0x205C0, 0x1D2A_3000), (0x20554, 0x0C00_0FF0), (0x551C0, 0xFA00_0000), (0x551C4, 0x0C00_0C00), (0x551C8, 0xFA02_EE00), (0x695FC, 1), (0x69600, 2), (0x69604, 3), (0x69608, 4)]);
        let (w2, ..) = decode(&dump(&sample(), 2), ISPFE);
        assert_eq!(w2, vec![(0x20110, 0x1D2A_3000)]);
    }

    #[test]
    fn the_job() {
        let job = image(&dump(&sample(), 1), Some(64));
        let f = &job.file;
        let u = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
        assert_eq!(&f[..4], b"CPR3");
        assert_eq!(u(4), 3);
        // The ring: 1024 entries pointing at the list.
        assert_eq!((u(8), u(12)), (RING_PA, 1024 * 16));
        assert_eq!((u(16), u(20), u(28)), (0xC003, LIST_PA, 25 * 4));
        let l = 16 + 1024 * 16;
        assert_eq!((u(l), u(l + 4)), (LIST_PA, 100));
        let w = |k: usize| u(l + 8 + 4 * k);
        // The image body to slot 2, offset kept; the height 3072 to 64 (both halves of the burst's middle word); the burst's two registers share one granule (G#FA) in the first free slot, 0.
        assert_eq!(w(4), 0xA200_0000 + 0x2A_3000);
        assert_eq!(w(6), 0x0040_0FF0);
        assert_eq!((w(9), w(10), w(11)), (0xA000_0000, 0x0040_0040, 0xA002_EE00));
        // The block load's source at G#8802_0000.
        assert_eq!(w(13), LOAD_PA);
        let s = l + 8 + 100;
        assert_eq!((u(s), u(s + 4), u(s + 8)), (LOAD_PA, 16, 1));
        assert_eq!(f.len(), s + 8 + 16);
    }

    #[test]
    fn the_cpu_setup() {
        let log = [
            "ferros-mark: front",                                         // 1
            "ferros hv: S2 000000001C040004 W4 0000000000005002",         // 2: the link: receiver and before
            "ferros hv: S2 000000001C10131C W4 0000000000000200",         // 3: the PHY: receiver only
            "ferros hv: S2 000000001C3205C0 W4 000000001D000000",         // 4: an address register: neither
            "ferros hv: S2 000000001C390000 W4 0000000000000001",         // 5: b, the PDMA start: neither
            "ferros hv: S2 000000001C3200A0 W4 0000000000000081",         // 6: after
            "ferros hv: S2 000000001C020500 W4 0000000000000001",         // 7: the CSIS reset word: receiver only (outside the front end)
            "ferros hv: S2 000000001C3200A0 R4 0000000000000081",         // 8: a read
            "ferros hv: S2 000000001C3200B0 W4 0000000000000001",         // 9: c, not taken
        ]
        .join("\n");
        let c = cpu(&log, 1, 5, 9, 0x1C04, false);
        assert_eq!(c.pre, vec![(0x1C04_0004, 0x5002)]);
        assert_eq!(c.post, vec![(0x1C32_00A0, 0x81)]);
        assert_eq!(c.rx, vec![(0x1C04_0004, 0x5002), (0x1C10_131C, 0x200), (0x1C02_0500, 1)]);
        let b = cpr2(&c);
        assert_eq!(b.len(), 8 + 4 + 8 + 4 + 4 + 8);
        assert_eq!(rx_rs("RX_START_T", &c.rx[..1]), "pub const RX_START_T: [(usize, u32); 1] = [\n    (0x1C04_0004, 0x5002),\n];\n");
    }
}
