//! A captured ftrace of the panel driver's DSI traffic (dsi_tx, dsi_rx, dsi_label_scope, te2_update_settings) as a clean command list: relative ms, packet type, bytes, delay, labels. Pure text; carries no secret.
//! `oracle-decode dsi <root>` reads every `<root>/<state>/dsi-trace.txt` (data/oracle/display/husky-hk3/tools/sweep.sh writes them) and writes `<root>/<state>/dsi-commands.txt` beside it.

use crate::{die, read_text};

const KINDS: [&str; 6] = ["dsi_tx", "dsi_rx", "dsi_label_scope", "te2_update_settings", "panel_write_generic", "tracing_mark_write"];

/// `<task> [<cpu>] <flags> <secs>.<frac>: <kind>: <rest>`: the time, the kind and the rest.
fn event(line: &str) -> Option<(f64, &'static str, &str)> {
    let mut s = line.trim_start();
    let token = |s: &mut &str| -> Option<String> {
        let n = s.find(char::is_whitespace)?;
        let t = s[..n].to_string();
        let rest = &s[n..];
        let after = rest.trim_start();
        if after.len() == rest.len() {
            return None;
        }
        *s = after;
        Some(t)
    };
    token(&mut s)?;
    let cpu = token(&mut s)?;
    let cpu = cpu.strip_prefix('[')?.strip_suffix(']')?;
    if cpu.is_empty() || !cpu.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token(&mut s)?;
    // The time: digits, a point, digits, then ": " and the event's name.
    let colon = s.find(':')?;
    let ts = &s[..colon];
    let (a, b) = ts.split_once('.')?;
    if a.is_empty() || b.is_empty() || !a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let s = s[colon + 1..].strip_prefix(' ')?;
    let (name, rest) = s.split_once(": ")?;
    let kind = KINDS.iter().find(|k| **k == name)?;
    Some((ts.parse().ok()?, kind, rest))
}

/// `type=0x<hex> length=<n> last=<d> delay=<n> tx=[<hex and spaces>]`: the type, last, delay and bytes.
fn tx(rest: &str) -> Option<(&str, &str, &str, &str)> {
    let s = rest.strip_prefix("type=0x")?;
    let n = s.bytes().take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)).count();
    let (ty, s) = (&s[..n], s[n..].strip_prefix(" length=")?);
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    let s = s[n..].strip_prefix(" last=").filter(|_| n > 0)?;
    let last = s.get(..1).filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?;
    let s = s[1..].strip_prefix(" delay=")?;
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    let (delay, s) = (&s[..n], s[n..].strip_prefix(" tx=[").filter(|_| n > 0)?);
    let n = s.bytes().take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b) || *b == b' ').count();
    s[n..].strip_prefix(']')?;
    (!ty.is_empty()).then_some((ty, last, delay, &s[..n]))
}

/// One state's trace into its command list; the packet count and the last line's time.
fn decode(src: &str, dst: &str) -> (usize, String) {
    let (mut t0, mut out, mut n_tx) = (None, Vec::new(), 0);
    for line in read_text(src).lines() {
        let Some((ts, kind, rest)) = event(line) else { continue };
        let ms = (ts - *t0.get_or_insert(ts)) * 1000.;
        match kind {
            "dsi_tx" => {
                let Some((ty, last, delay, hx)) = tx(rest) else { continue };
                n_tx += 1;
                let tag = if ty == "39" { String::new() } else { format!(" type {ty}") };
                let last = if last == "1" { "  last" } else { "" };
                let wait = if delay != "0" { format!("  wait {delay} ms") } else { String::new() };
                out.push(format!("{ms:9.3}  tx {hx}{tag}{last}{wait}"));
            }
            "dsi_rx" => out.push(format!("{ms:9.3}  rx {rest}")),
            "dsi_label_scope" | "te2_update_settings" => out.push(format!("{ms:9.3}  -- {rest}")),
            _ => {}
        }
    }
    let text = format!("# {n_tx} DSI packets; times in ms from the first event; bytes are the packet payload (the first byte is the DCS command); type shown when not a long write (39)\n{}\n", out.join("\n"));
    std::fs::write(dst, text).unwrap_or_else(|e| die(&format!("{dst}: {e}")));
    let span = out.last().and_then(|l| l.split_whitespace().next()).unwrap_or("0").to_string();
    (n_tx, span)
}

pub fn run(args: &[String]) {
    let root = args.first().unwrap_or_else(|| die("dsi <root>"));
    let mut dirs: Vec<String> = std::fs::read_dir(root).unwrap_or_else(|e| die(&format!("{root}: {e}"))).filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    dirs.sort();
    for d in dirs {
        let p = format!("{root}/{d}/dsi-trace.txt");
        if std::path::Path::new(&p).is_file() {
            let (n, span) = decode(&p, &format!("{root}/{d}/dsi-commands.txt"));
            println!("{d:<14} {n:5} packets over {span} ms");
        }
    }
}
