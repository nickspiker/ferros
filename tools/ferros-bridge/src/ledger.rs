//! The flash log on the host (LEDGER.md "Reading it"): `diag log` rendered, `log-render` for a saved answer, and `log-legacy`, the one-shot converter of the 2026-10-20 text log.
//!
//! `diag log [n]` answers with VSF documents (ferros_ledger::view); they are shown as lines, `b<boot> <local wall time> <writer>: <words>`, the Eagle time turned into the host's local time for display only.
//! `--raw` writes the answer's documents as they came, for a file to keep or `log-render` later.
//!
//! `log-legacy <out.vsf> [--erase]` reads the old log's raw blocks (`diag "log legacy"`), keeps every line as a `ledger_legacy` document (its boot, its counter milliseconds and its words as typed fields, chained), checks the file it wrote, and with `--erase` zeroes the old blocks on the device.
//! It is the only reader of the old text encoding, and it runs once per device: nothing on the device or here parses that text again (AGENT.md: no compatibility reader for a text encoding).

use std::io::{self, Write};

use ferros_ledger::entry::{self, Hash};
use ferros_ledger::view;

/// The old log: block 0 its header, then text blocks, `diag "log legacy"` hands out 2^6 a call.
const OLD_BLOCK: usize = 4096;
const OLD_BLOCKS: u32 = 1 << 14;
const STEP: u32 = 1 << 6;

/// An Eagle time as the host's local wall time, for display only.
fn wall(osc: i64) -> String {
    match vsf::EagleTime::from_oscillations(osc).to_datetime_opt() {
        Some(t) => t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("(Eagle time A#{osc} out of range)"),
    }
}

/// Whether a `diag` parameter asks for the log's entries (not its legacy blocks or its bench, which answer in their own form).
pub fn is_entries(params: &[u8]) -> bool {
    let p = String::from_utf8_lossy(params);
    let mut w = p.split_whitespace();
    w.next() == Some("log") && !matches!(w.next(), Some("legacy") | Some("bench"))
}

/// An answer to `diag log` on stdout: lines, or (`raw`) the documents as they came.
pub fn show(data: &[u8], raw: bool) {
    if raw {
        let _ = io::stdout().write_all(data);
        let _ = io::stdout().flush();
        return;
    }
    let mut out = io::stdout().lock();
    for l in view::render(data, &wall) {
        let _ = writeln!(out, "{l}");
    }
}

/// `log-render <file.vsf>`: a saved answer or a converted old log, as lines.
pub fn cmd_render(path: &str) {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    show(&data, false);
}

/// One `diag` on an open link: its answer.
pub(crate) async fn diag(link: &crate::usb::UsbLink, params: &str) -> Vec<u8> {
    let cmd = crate::build_cmd(ferros_pt::command::caps::DIAG, ferros_pt::Op::Read, params.as_bytes());
    match crate::pt_send(link, &cmd).await {
        Ok(c) if c.success => {}
        Ok(_) => {
            eprintln!("diag {params}: the device reported a failure");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("diag {params}: {e}");
            std::process::exit(1);
        }
    }
    crate::pt_recv(link).await.unwrap_or_else(|e| {
        eprintln!("diag {params}: {e}");
        std::process::exit(1);
    })
}

/// One line of the old log: its boot, its counter milliseconds, its words.
#[derive(Debug, PartialEq)]
struct OldLine {
    boot: u64,
    ms: u64,
    words: String,
}

/// The old log's text as lines.
/// A line was written as `b<boot> t<ms> <words>\n`; a line that does not start that way is the rest of the words before it (words with a newline in them).
fn old_lines(text: &str) -> Vec<OldLine> {
    let mut out: Vec<OldLine> = Vec::new();
    for l in text.split('\n') {
        let head = (|| {
            let rest = l.strip_prefix('b')?;
            let (boot, rest) = rest.split_once(' ')?;
            let rest = rest.strip_prefix('t')?;
            let (ms, words) = rest.split_once(' ').unwrap_or((rest, ""));
            Some((boot.parse::<u64>().ok()?, ms.parse::<u64>().ok()?, words))
        })();
        match (head, out.last_mut()) {
            (Some((boot, ms, words)), _) => out.push(OldLine { boot, ms, words: words.to_string() }),
            (None, Some(last)) if !l.is_empty() => {
                last.words.push('\n');
                last.words.push_str(l);
            }
            _ => {}
        }
    }
    out
}

/// The old lines as chained `ledger_legacy` documents, the conversion's time on each.
fn old_docs(lines: &[OldLine], now: i64) -> Vec<u8> {
    let mut run = Vec::new();
    let mut prev: Hash = entry::genesis();
    for (n, l) in lines.iter().enumerate() {
        let d = view::legacy(n as u64, &prev, l.boot, l.ms, &l.words, Some(now)).unwrap_or_else(|e| {
            eprintln!("old line A#{n}: {e}");
            std::process::exit(1);
        });
        prev = view::hash_of(&d).expect("a document just built verifies");
        run.extend_from_slice(&d);
    }
    run
}

/// `log-legacy <out.vsf> [--erase]`.
pub async fn cmd_legacy(args: &[String]) {
    let Some(out) = args.first() else {
        eprintln!("Usage: ferros-bridge log-legacy <out.vsf> [--erase]  (the old text log as ledger_legacy documents; --erase zeroes it on the device once the file checks)");
        std::process::exit(1);
    };
    let erase = args.iter().any(|a| a == "--erase");
    let link = crate::usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let head = diag(&link, "log legacy 0 1").await;
    if head.len() < OLD_BLOCK || &head[..4] != b"FLOG" {
        eprintln!("no old log on this device (its first block is not the old header): nothing to convert");
        return;
    }
    // The old header, read here once: {"FLOG", u32 next block, u32 bytes used in it, u32 boot count}, little-endian.
    let word = |o: usize| u32::from_le_bytes([head[o], head[o + 1], head[o + 2], head[o + 3]]);
    let (last, used) = (word(4), word(8) as usize);
    // WHY the bounds: the header is the flash's; a damaged one must not send the reads past the old log or cut past a block.
    if last == 0 || last >= OLD_BLOCKS || used > OLD_BLOCK {
        eprintln!("the old header does not read (block A#{last}, A#{used} bytes): not converted");
        std::process::exit(1);
    }
    let mut text = Vec::with_capacity(last as usize * OLD_BLOCK);
    let mut first = 1u32;
    while first <= last {
        let got = diag(&link, &format!("log legacy {first} {STEP}")).await;
        if got.is_empty() || got.len() % OLD_BLOCK != 0 {
            eprintln!("block A#{first}: the device gave A#{} bytes", got.len());
            std::process::exit(1);
        }
        text.extend_from_slice(&got);
        first += (got.len() / OLD_BLOCK) as u32;
    }
    // Blocks 1 to `last`: every block before the last whole, the last up to `used`.
    text.truncate((last as usize - 1) * OLD_BLOCK + used);
    let lines = old_lines(&String::from_utf8_lossy(&text));
    let run = old_docs(&lines, vsf::eagle_time_oscillations());
    std::fs::write(out, &run).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    });
    let back = std::fs::read(out).unwrap_or_default();
    match view::check_legacy(&back) {
        Ok(n) if n == lines.len() as u64 => println!("{out}: A#{n} lines of the old log, A#{} bytes, each verified and chained", back.len()),
        r => {
            eprintln!("{out}: the file does not check ({r:?}); the old log is left as it is");
            std::process::exit(1);
        }
    }
    if !erase {
        println!("the old log is still on the device: `ferros-bridge log-legacy {out} --erase` zeroes it once you have checked the file (`ferros-bridge log-render {out}`)");
        return;
    }
    let mut first = 0u32;
    while first <= last {
        let r = diag(&link, &format!("log legacy erase {first} {STEP}")).await;
        let r = String::from_utf8_lossy(&r);
        eprint!("{r}");
        if !r.contains(&format!("A#{STEP} blocks")) && first + STEP <= last {
            eprintln!("erase stopped at block A#{first}");
            std::process::exit(1);
        }
        first += STEP;
    }
    println!("the old log's blocks A#0 to A#{last} are zeroed and read back");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_lines_and_their_continuations() {
        let text = "b930 t812 boot cold, flash log init true\nb930 t900 camera: on\nsecond line of it\nb931 t5 \n";
        let l = old_lines(text);
        assert_eq!(l, vec![
            OldLine { boot: 930, ms: 812, words: "boot cold, flash log init true".into() },
            OldLine { boot: 930, ms: 900, words: "camera: on\nsecond line of it".into() },
            OldLine { boot: 931, ms: 5, words: "".into() },
        ]);
        let run = old_docs(&l, 1);
        assert_eq!(view::check_legacy(&run), Ok(3));
        let shown = view::render(&run, &wall);
        assert_eq!(shown[1], "b930 (old log, A#900 ms after the counter started) camera: on\nsecond line of it");
    }

    #[test]
    fn which_diag_asks_for_entries() {
        assert!(is_entries(b"log"));
        assert!(is_entries(b"log 64"));
        assert!(!is_entries(b"log legacy 0 1"));
        assert!(!is_entries(b"log bench"));
        assert!(!is_entries(b"logs"));
    }
}
