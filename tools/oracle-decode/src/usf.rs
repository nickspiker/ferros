//! A USF capture: the AoC sensor service's messages as the kernel's kprobes saw them (docs/SENSORS.md "Capturing USF").
//! Input: an ftrace dump with `usf_tx` / `usf_rx` kprobe events whose fields a, b, c, d are x32[64] arrays (1 KiB of the message). Each message is [u32 channel][a FlatBuffer]; the FlatBuffer is decoded without a schema: every table field is shown as its raw 32-bit word, and a word that is a plausible offset to a table, vector or string is followed.
//! Everything read out of a message is bounds-checked (`get`): the capture is external input, cut at 1 KiB, and the offsets in it are guesses.

use crate::{die, hex, read_text};

struct Msg {
    task: String,
    ts: f64,
    tx: bool,
    len: u64,
    data: Vec<u8>,
}

/// `{0x1,0x2,...}` at the start of `s`: the words, and the rest after the brace.
fn words(s: &str) -> Option<(Vec<u32>, &str)> {
    let s = s.strip_prefix('{')?;
    let end = s.find('}')?;
    let mut w = Vec::new();
    for t in s[..end].split(',').filter(|t| !t.is_empty()) {
        let t = t.trim();
        let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
        w.push(u32::from_str_radix(t, 16).ok()?);
    }
    Some((w, &s[end + 1..]))
}

/// `len=0x<hex> a={..} b={..} c={..} d={..}` at `s` (just past "len=").
fn fields_at(s: &str) -> Option<(u64, Vec<u32>)> {
    let s = s.strip_prefix("0x")?;
    let n = s.bytes().take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)).count();
    if n == 0 {
        return None;
    }
    let len = u64::from_str_radix(&s[..n], 16).ok()?;
    let mut s = &s[n..];
    let mut all = Vec::new();
    for name in ["a=", "b=", "c=", "d="] {
        let t = s.trim_start();
        if t.len() == s.len() {
            return None;
        }
        let (w, rest) = words(t.strip_prefix(name)?)?;
        all.extend(w);
        s = rest;
    }
    Some((len, all))
}

/// One event line: `<task>-<pid> [<cpu>] <flags> <ts>: usf_tx|usf_rx: ... len=0x.. a={..} b={..} c={..} d={..}`.
fn parse_line(line: &str) -> Option<Msg> {
    let mut t = line.split_whitespace();
    let (task, pid) = t.next()?.rsplit_once('-')?;
    if task.is_empty() || pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let cpu = t.next()?.strip_prefix('[')?.strip_suffix(']')?;
    if cpu.is_empty() || !cpu.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.next()?;
    let ts = t.next()?.strip_suffix(':')?;
    if ts.is_empty() || !ts.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let kind = t.next()?;
    let tx = if kind.starts_with("usf_tx:") {
        true
    } else if kind.starts_with("usf_rx:") {
        false
    } else {
        return None;
    };
    let after = &line[line.find(if tx { "usf_tx:" } else { "usf_rx:" })? + 7..];
    let (len, w) = after.match_indices("len=").find_map(|(i, _)| fields_at(&after[i + 4..]))?;
    let mut data: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    data.truncate(len.min(1024) as usize);
    Some(Msg { task: task.to_string(), ts: ts.parse().ok()?, tx, len, data })
}

fn u32_at(b: &[u8], o: i64) -> Option<u32> {
    let o = usize::try_from(o).ok()?;
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().unwrap()))
}

fn i32_at(b: &[u8], o: i64) -> Option<i32> {
    u32_at(b, o).map(|v| v as i32)
}

fn u16_at(b: &[u8], o: i64) -> Option<u32> {
    let o = usize::try_from(o).ok()?;
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().unwrap()) as u32)
}

/// Python's `%.6g`.
fn g6(f: f64) -> String {
    let e: i32 = format!("{f:.5e}").split_once('e').unwrap().1.parse().unwrap();
    let trim = |s: String| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if !(-4..6).contains(&e) {
        let (m, _) = format!("{f:.5e}").split_once('e').map(|(m, x)| (m.to_string(), x.to_string())).unwrap();
        format!("{}e{}{:02}", trim(m), if e < 0 { '-' } else { '+' }, e.abs())
    } else {
        trim(format!("{f:.*}", (5 - e) as usize))
    }
}

/// A word as an f32, when it reads as a plausible one.
fn as_float(v: u32) -> Option<String> {
    let f = f32::from_bits(v) as f64;
    (v != 0 && 1e-6 < f.abs() && f.abs() < 1e7).then(|| g6(f))
}

fn looks_table(b: &[u8], t: i64) -> bool {
    let Some(so) = i32_at(b, t) else { return false };
    let vt = t - so as i64;
    if vt < 0 || vt + 4 > b.len() as i64 || vt % 2 != 0 {
        return false;
    }
    let (Some(vsz), Some(osz)) = (u16_at(b, vt), u16_at(b, vt + 2)) else { return false };
    (4..=128).contains(&vsz) && vsz % 2 == 0 && (4..=1024).contains(&osz) && vt + vsz as i64 <= b.len() as i64 && t + osz as i64 <= b.len() as i64 + 64
}

fn pad(depth: usize) -> String {
    "  ".repeat(depth)
}

fn show_vector(b: &[u8], at: i64, depth: usize, out: &mut Vec<String>) {
    let n = u32_at(b, at).unwrap();
    let start = (at + 4) as usize;
    let body = &b[start.min(b.len())..(start + n as usize).min(b.len())];
    let end = start + n as usize;
    if n > 0 && n < 512 && body.iter().all(|&c| (32..127).contains(&c)) && end < b.len() && b[end] == 0 {
        out.push(format!("{}string \"{}\"", pad(depth), String::from_utf8_lossy(body)));
        return;
    }
    out.push(format!("{}vector n={n}: {}", pad(depth), hex(&body[..body.len().min(48)], "")));
    // A [ubyte] vector that is itself a FlatBuffer (USF nests its payload this way).
    if n >= 12 && n as usize <= b.len() {
        if let Some(root) = u32_at(body, 0) {
            if root < n && looks_table(body, root as i64) {
                out.push(format!("{}  nested:", pad(depth)));
                table(body, root as i64, depth + 2, out, &mut Vec::new());
            }
        }
    }
}

fn table(b: &[u8], t: i64, depth: usize, out: &mut Vec<String>, seen: &mut Vec<i64>) {
    if seen.contains(&t) || depth > 12 {
        return;
    }
    seen.push(t);
    let vt = t - i32_at(b, t).unwrap() as i64;
    let vsz = u16_at(b, vt).unwrap() as i64;
    for i in 0..(vsz - 4) / 2 {
        let fo = match u16_at(b, vt + 4 + 2 * i) {
            Some(fo) if fo != 0 => fo as i64,
            _ => continue,
        };
        let at = t + fo;
        let Some(v) = u32_at(b, at) else {
            out.push(format!("{}f{i} @+{fo}: (past capture)", pad(depth)));
            continue;
        };
        let tgt = at + v as i64;
        let mut desc = format!("{}f{i} @+{fo}: G#{v:X} ({v})", pad(depth));
        if let Some(f) = as_float(v) {
            desc += &format!(" f32 {f}");
        }
        out.push(desc);
        if (4..4096).contains(&v) && tgt < b.len() as i64 {
            if looks_table(b, tgt) {
                out.push(format!("{}  -> table @{tgt}", pad(depth)));
                table(b, tgt, depth + 2, out, seen);
            } else if u32_at(b, tgt).is_some_and(|n| (n as i64) < b.len() as i64 - tgt) {
                show_vector(b, tgt, depth + 1, out);
            }
        }
    }
}

fn decode(m: &Msg) -> (u32, Vec<String>) {
    let b = &m.data;
    let ch = u32_at(b, 0).unwrap_or_else(|| die("a message shorter than its channel word"));
    let fb = &b[4..];
    let mut out = Vec::new();
    match u32_at(fb, 0) {
        Some(root) if (root as usize) < fb.len() && looks_table(fb, root as i64) => table(fb, root as i64, 1, &mut out, &mut Vec::new()),
        _ => out.push(format!("  (not a FlatBuffer at the root) {}", hex(&fb[..fb.len().min(64)], ""))),
    }
    (ch, out)
}

pub fn run(args: &[String]) {
    let raw = args.iter().any(|a| a == "--raw");
    let only = args.iter().position(|a| a == "--only").map(|i| args.get(i + 1).unwrap_or_else(|| die("--only tx|rx")).as_str());
    let skip = args.iter().any(|a| a == "--skip-streams");
    let path = args.iter().find(|a| !a.starts_with("--") && *a != "tx" && *a != "rx").unwrap_or_else(|| die("usf <capture.txt> [--raw] [--only tx|rx] [--skip-streams]"));
    let mut t0: Option<f64> = None;
    for m in read_text(path).lines().filter_map(parse_line) {
        if only.is_some_and(|o| o != if m.tx { "tx" } else { "rx" }) {
            continue;
        }
        let (ch, out) = decode(&m);
        if skip && !m.tx && ch & 0x8000_0000 != 0 {
            continue;
        }
        let t0 = *t0.get_or_insert(m.ts);
        println!("{:9.3} {} {:<22} ch G#{ch:X} len {}", m.ts - t0, if m.tx { "AP->AoC" } else { "AoC->AP" }, m.task, m.len);
        if raw {
            println!("  {}", hex(&m.data, ""));
        }
        for l in out {
            println!("{l}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g6_as_python() {
        for (f, s) in [(1., "1"), (0.5, "0.5"), (1234567., "1.23457e+06"), (0.0001234, "0.0001234"), (0.00001, "1e-05"), (-2.5, "-2.5"), (123456., "123456"), (9.80665, "9.80665")] {
            assert_eq!(g6(f), s);
        }
    }

    /// A hand-built message: channel 1, then a FlatBuffer whose root table has a float field (1.0) and an offset to the string "hello".
    #[test]
    fn walks_a_flatbuffer() {
        let mut w = vec!["0x1", "0xc", "0xc0008", "0x80004", "0x8", "0x3f800000", "0x8", "0x0", "0x5", "0x6c6c6568", "0x6f"];
        w.resize(64, "0x0");
        let a = w.join(",");
        let z = vec!["0x0"; 64].join(",");
        let line = format!("     kworker/1:2-345     [001] d..1.  1234.500000: usf_tx: (aoc_service_write+0x0/0x1c0) len=0x2c a={{{a}}} b={{{z}}} c={{{z}}} d={{{z}}}");
        let m = parse_line(&line).unwrap();
        assert_eq!((m.task.as_str(), m.tx, m.len, m.data.len()), ("kworker/1:2", true, 44, 44));
        let (ch, out) = decode(&m);
        assert_eq!(ch, 1);
        assert_eq!(out, ["  f0 @+4: G#3F800000 (1065353216) f32 1", "  f1 @+8: G#8 (8)", "    string \"hello\""]);
    }
}
