//! handle-guard — the fail-closed scanner that refuses any content carrying a Photon handle (the LEFT column of the pseudonym map in keys/: a handle derives an identity seed and must never enter a repo). This is the Rust twin of keys/handle-guard.sh, line for line in behaviour, written because the bash loop took an hour over a 58 MB first commit (2026-10-27); this reads 58 MB in well under a second.
//!
//! Reads stdin: a diff (`git diff`, `git log -p`) or plain text. Follows `diff --git` / `+++ b/<path>` / `@@ -a,b +c,d @@` so a hit is reported as `path:line`, skips `-` lines (a commit that removes a handle shows it there, and blocking those would make leaks uncleanable). Exit 0 clean; 1 a handle found (BLOCK); 2 the map cannot be read (BLOCK, fail-closed).
//!
//! Files beside the map, all in keys/ and never in a repo: `handle-guard-exclusions.txt` (`petname | repo | path glob`: that person's handle may stand in those files of that repo — a handle can be a real word) and `handle-guard-allow.txt` (one regex per line, each anchoring a whole line: the known-benign lines, with the record of why). Environment: FERROS_HANDLE_MAP, FERROS_HANDLE_EXCLUSIONS, FERROS_HANDLE_ALLOW, HANDLE_GUARD_REPO (else the git toplevel's name), FERROS_HANDLE_AUDIT=1 to name ungateable handles.

use std::io::Read;
use std::process::{Command, exit};

struct Handle {
    handle: String,
    petname: String,
    multiword: bool,
}

fn trimmed(s: &str) -> &str {
    s.trim()
}

fn main() {
    let keys = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_string());
    let map_path = keys("FERROS_HANDLE_MAP", "/mnt/Harbor/Code/keys/claude-pseudonym-map.txt");
    let map = match std::fs::read_to_string(&map_path) {
        Ok(m) => m,
        Err(_) => {
            eprintln!("handle-guard: CANNOT READ handle map ({map_path}) — refusing (fail-closed).");
            eprintln!("  set FERROS_HANDLE_MAP to keys/claude-pseudonym-map.txt on this machine.");
            exit(2);
        }
    };
    let beside = |file: &str| format!("{}/{}", std::path::Path::new(&map_path).parent().map_or(".".to_string(), |p| p.display().to_string()), file);
    let audit = std::env::var("FERROS_HANDLE_AUDIT").map_or(false, |v| v == "1");
    let mut handles: Vec<Handle> = Vec::new();
    for row in map.lines().filter(|l| !l.trim_start().starts_with('#') && l.contains('=')) {
        let (h, p) = row.split_once('=').unwrap();
        let (h, p) = (trimmed(h), trimmed(p));
        if h.is_empty() {
            continue;
        }
        if h.chars().count() < 3 || h.bytes().all(|b| b.is_ascii_digit()) {
            // Ungateable (1-2 chars or purely numeric): matches everything, cannot gate — the one hole; the person must rotate to a distinctive handle.
            if audit {
                eprintln!("handle-guard: UNGATEABLE handle '{h}' — rotate to a distinctive one.");
            }
            continue;
        }
        handles.push(Handle { handle: h.to_string(), petname: if p.is_empty() { "<petname>".to_string() } else { p.to_string() }, multiword: h.contains(' ') });
    }

    let mut content = String::new();
    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    content.push_str(&String::from_utf8_lossy(&raw));
    if content.is_empty() {
        exit(0);
    }

    // Allowlisted whole lines carry nothing.
    let allow: Vec<regex::Regex> = std::fs::read_to_string(keys("FERROS_HANDLE_ALLOW", &beside("handle-guard-allow.txt")))
        .map(|s| s.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).filter_map(|l| regex::Regex::new(l).ok()).collect())
        .unwrap_or_default();

    // Exclusions: (petname, path glob) rows for this repo.
    let repo = std::env::var("HANDLE_GUARD_REPO").ok().or_else(|| {
        Command::new("git").args(["rev-parse", "--show-toplevel"]).output().ok().filter(|o| o.status.success()).and_then(|o| {
            let top = String::from_utf8_lossy(&o.stdout).trim().to_string();
            std::path::Path::new(&top).file_name().map(|n| n.to_string_lossy().to_string())
        })
    });
    let mut exclusions: Vec<(String, glob::Pattern)> = Vec::new();
    if let (Ok(text), Some(repo)) = (std::fs::read_to_string(keys("FERROS_HANDLE_EXCLUSIONS", &beside("handle-guard-exclusions.txt"))), repo.as_ref()) {
        for l in text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')) {
            let mut it = l.splitn(3, '|');
            let (pet, r, g) = (it.next().map(trimmed).unwrap_or(""), it.next().map(trimmed).unwrap_or(""), it.next().unwrap_or(""));
            let g = g.split('#').next().unwrap_or("").trim();
            if pet.is_empty() || g.is_empty() || r != repo {
                continue;
            }
            if let Ok(p) = glob::Pattern::new(g) {
                exclusions.push((pet.to_string(), p));
            }
        }
    }
    let excluded = |pet: &str, path: &str| exclusions.iter().any(|(p, g)| p == pet && g.matches(path));

    // Mask every handle in a string with its petname, longest handle first so one handle inside another is never half-replaced.
    let mut by_len: Vec<&Handle> = handles.iter().collect();
    by_len.sort_by(|a, b| b.handle.len().cmp(&a.handle.len()));
    let mask = |s: &str| by_len.iter().fold(s.to_string(), |acc, h| acc.replace(h.handle.as_str(), &h.petname));

    let hunk = regex::Regex::new(r"^@@ -[0-9]+(,[0-9]+)? \+([0-9]+)(,[0-9]+)? @@").unwrap();
    let commit = regex::Regex::new(r"^commit [0-9a-f]{40}").unwrap();
    let (mut file, mut line, mut in_diff, mut in_hdr) = ("stdin".to_string(), 0usize, false, false);
    let (mut hit, mut scanned) = (false, 0usize);
    for l in content.lines() {
        scanned += 1;
        // Diff bookkeeping, as a state machine so no content line can pose as a header.
        if commit.is_match(l) {
            in_diff = false;
            in_hdr = false;
            file = "stdin".to_string();
        } else if let Some(_) = l.strip_prefix("diff --git ") {
            in_diff = true;
            in_hdr = true;
            file = "?".to_string();
            continue;
        } else if in_diff && hunk.is_match(l) {
            let c = hunk.captures(l).unwrap();
            in_hdr = false;
            line = c[2].parse::<usize>().unwrap_or(1).saturating_sub(1);
            continue;
        } else if in_hdr {
            if let Some(f) = l.strip_prefix("+++ ") {
                file = f.strip_prefix("b/").unwrap_or(f).to_string();
            }
            continue;
        } else if in_diff && (l.starts_with('-') || l.starts_with('\\')) {
            continue;
        }
        if !in_hdr {
            line += 1;
        }
        if allow.iter().any(|re| re.is_match(l)) {
            continue;
        }
        for h in &handles {
            let found = if h.multiword { l.contains(h.handle.as_str()) } else { whole_word(l, &h.handle) };
            if !found {
                continue;
            }
            if in_diff && !in_hdr && excluded(&h.petname, &file) {
                continue;
            }
            // Every handle on the line is masked, not only the one this report is about: a line holding several handles printed the others raw (2026-10-27, three handles reached a transcript that way).
            let shown = mask(l);
            let where_ = mask(&if in_diff { format!("{file}:{line}") } else { format!("stdin:{line}") });
            eprintln!("handle-guard: BLOCKED — handle of '{}' at {where_}", h.petname);
            eprintln!("    {shown}");
            eprintln!("    (shown with the handle already replaced by the petname — that is the edit)");
            hit = true;
        }
    }
    if hit {
        eprintln!("handle-guard: COMMIT/PUSH REFUSED ({scanned} lines scanned). A handle is a key leak — replace it with the person's PETNAME (right column of the map) at the locations above. --no-verify is NOT the answer.");
        exit(1);
    }
    exit(0);
}

/// `grep -w` for a single-word handle: the match bounded by non-word bytes (letters, digits and `_` are word bytes), exact case.
fn whole_word(line: &str, word: &str) -> bool {
    let (lb, wb) = (line.as_bytes(), word.as_bytes());
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(i) = line[from..].find(word) {
        let s = from + i;
        let e = s + wb.len();
        let before_ok = s == 0 || !is_word(lb[s - 1]);
        let after_ok = e >= lb.len() || !is_word(lb[e]);
        if before_ok && after_ok {
            return true;
        }
        from = s + 1;
        if from >= lb.len() {
            break;
        }
    }
    false
}
