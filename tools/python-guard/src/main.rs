//! python-guard: no Python in any commit of the owner's (his rule, 2026-11-21: not portable, not deterministic, not safe, and its code can't be copy-pasted unless the indentation matches exactly). Fail closed.
//!
//!   python-guard staged                  what pre-commit is about to commit (the index)
//!   python-guard push <tip> <rev-args…>  the files the pushed commits (`<rev-args>`, as `git log` takes them) add or change, as they stand at `<tip>`: pre-push's backstop for `--no-verify`
//!
//! A file is refused when it is added or changed (deleting Python is always allowed) and it is:
//! - a Python source by name: .py .pyw .pyi .pyx .pxd .ipynb
//! - any file whose first line is a python shebang
//! - a build script (shell, make, just, cmake, CI yaml, toml, PowerShell, batch) with a non-comment line naming python or pip
//! - a Rust source with a "python…" or "pip…" string literal (a Command that runs it)
//!
//! Exemptions are the owner's alone, one per line in `~/.config/git/python-allow` (or `$PYTHON_GUARD_ALLOW`): a repo name (its top folder) exempts the whole repo, `repo/path-glob` one path (`*` and `?`, `*` crossing `/`).
//! Every file's content comes through ONE `git cat-file --batch -Z`, so a commit of thousands of files costs one process, not thousands (the bash twin spent ~30 ms a file).

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, ExitCode, Stdio};

const PY_EXT: [&str; 6] = [".py", ".pyw", ".pyi", ".pyx", ".pxd", ".ipynb"];
const SCRIPT_EXT: [&str; 11] = [".sh", ".bash", ".zsh", ".mk", ".cmake", ".yml", ".yaml", ".toml", ".ps1", ".bat", ".cmd"];
const SCRIPT_NAMES: [&str; 6] = ["Makefile", "GNUmakefile", "makefile", "justfile", "Justfile", "CMakeLists.txt"];

fn git(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn trimmed(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii_whitespace())
}

/// `*` any run of characters (slashes included), `?` one character; everything else literal.
fn glob(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| glob(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && glob(rest, &s[1..]),
        Some((&c, rest)) => s.first() == Some(&c) && glob(rest, &s[1..]),
    }
}

fn word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Does `line` name python or pip as a word (`python`, `python3`, `python3.11`, `pip3`; not `pipe`, not `mypython`)?
fn names_python(line: &[u8]) -> bool {
    for stem in [&b"python"[..], &b"pip"[..]] {
        let mut from = 0;
        while let Some(at) = line[from..].windows(stem.len()).position(|w| w == stem).map(|p| p + from) {
            let before_ok = at == 0 || !word_char(line[at - 1]);
            let mut end = at + stem.len();
            while end < line.len() && (line[end].is_ascii_digit() || line[end] == b'.') {
                end += 1;
            }
            // A hyphen after it makes a longer name (python-guard, the gate's own crate), not a command.
            let after_ok = end == line.len() || !(word_char(line[end]) || line[end] == b'-');
            if before_ok && after_ok {
                return true;
            }
            from = at + 1;
        }
    }
    false
}

/// Is `lit` (a string literal's content) a python or pip command, bare or by path?
fn is_python_command(lit: &[u8]) -> bool {
    let name = lit.rsplit(|&b| b == b'/').next().unwrap_or(lit);
    for stem in [&b"python"[..], &b"pip"[..]] {
        if let Some(rest) = name.strip_prefix(stem) {
            if rest.iter().all(|&b| b.is_ascii_digit() || b == b'.') {
                return true;
            }
        }
    }
    false
}

fn comment_line(line: &[u8]) -> bool {
    let t = line.trim_ascii_start();
    t.starts_with(b"#") || t.starts_with(b"//") || t.starts_with(b";") || t.starts_with(b"rem ") || t.starts_with(b"REM ")
}

/// Why `path` (with `content`) is Python, or None.
fn verdict(path: &str, content: &[u8]) -> Option<String> {
    if PY_EXT.iter().any(|e| path.ends_with(e)) {
        return Some("a Python source".into());
    }
    let first = content.split(|&b| b == b'\n').next().unwrap_or(b"");
    if first.starts_with(b"#!") && first.windows(6).any(|w| w == b"python") {
        return Some("a python shebang".into());
    }
    // Binary content has no lines to read.
    if content[..content.len().min(8192)].contains(&0) {
        return None;
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    let script = SCRIPT_EXT.iter().any(|e| base.ends_with(e)) || SCRIPT_NAMES.contains(&base);
    let rust = base.ends_with(".rs");
    if !script && !rust {
        return None;
    }
    for (n, line) in content.split(|&b| b == b'\n').enumerate() {
        if comment_line(line) {
            continue;
        }
        let hit = if script {
            names_python(line)
        } else {
            // Only a line that builds a Command runs anything: a word list or a lookup table may hold the plain word "python" (the snake) or "pip" as data (voca's wordlist, vsf's world_coord, 2026-11-21). Each string literal on it: "…" between quote pairs, escapes not interpreted (a command name holds none).
            line.windows(7).any(|w| w == b"Command") && line.split(|&b| b == b'"').skip(1).step_by(2).any(is_python_command)
        };
        if hit {
            let shown = String::from_utf8_lossy(line);
            let shown = trimmed(&shown);
            let shown: String = shown.chars().take(120).collect();
            return Some(format!("runs Python: line {}: {shown}", n + 1));
        }
    }
    None
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(top) = git(&["rev-parse", "--show-toplevel"]) else { return ExitCode::SUCCESS };
    let top = String::from_utf8_lossy(&top).trim_end().to_string();
    let repo = top.rsplit('/').next().unwrap_or(&top).to_string();

    let allow_path = std::env::var_os("PYTHON_GUARD_ALLOW").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/git/python-allow"));
    let allow: Vec<String> = std::fs::read_to_string(&allow_path)
        .unwrap_or_default()
        .lines()
        .map(|l| trimmed(l.split('#').next().unwrap_or("")).to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if allow.iter().any(|l| *l == repo) {
        return ExitCode::SUCCESS;
    }

    let (names, tip) = match args.first().map(String::as_str) {
        Some("staged") | None => (git(&["diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"]), String::new()),
        Some("push") if args.len() >= 3 => {
            let mut a = vec!["log", "--name-only", "--diff-filter=ACMR", "--format=", "-z"];
            a.extend(args[2..].iter().map(String::as_str));
            (git(&a), args[1].clone())
        }
        _ => {
            eprintln!("python-guard: usage: python-guard staged | python-guard push <tip> <rev-args…>");
            return ExitCode::from(2);
        }
    };
    // A git that cannot list the files is a failure to check, and a failure to check refuses.
    let Some(names) = names else {
        eprintln!("python-guard: git could not list the files to check in {repo}: refusing (fail-closed).");
        return ExitCode::FAILURE;
    };
    let mut files: Vec<String> = names.split(|&b| b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).into_owned()).collect();
    files.sort();
    files.dedup();
    files.retain(|f| {
        let full = format!("{repo}/{f}");
        !allow.iter().any(|p| glob(p.as_bytes(), full.as_bytes()))
    });
    if files.is_empty() {
        return ExitCode::SUCCESS;
    }

    // One cat-file for every file: requests written on a thread while the answers are read here, so neither pipe can fill and stall the other.
    let Ok(mut child) = Command::new("git").args(["cat-file", "--batch", "-Z"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
        eprintln!("python-guard: could not run git cat-file: refusing (fail-closed).");
        return ExitCode::FAILURE;
    };
    let mut stdin = child.stdin.take().expect("piped stdin");
    let specs: Vec<String> = files.iter().map(|f| format!("{tip}:{f}")).collect();
    let writer = std::thread::spawn(move || {
        for s in &specs {
            if stdin.write_all(s.as_bytes()).and_then(|_| stdin.write_all(&[0])).is_err() {
                return;
            }
        }
    });
    let mut out = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut bad = Vec::new();
    for f in &files {
        let mut header = Vec::new();
        if out.read_until(0, &mut header).unwrap_or(0) == 0 {
            eprintln!("python-guard: git cat-file ended early: refusing (fail-closed).");
            return ExitCode::FAILURE;
        }
        header.pop();
        // "<spec> missing": not there at the tip (removed later in the push): nothing to check.
        if header.ends_with(b" missing") {
            continue;
        }
        let size: usize = String::from_utf8_lossy(&header).rsplit(' ').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut content = vec![0u8; size + 1];
        if out.read_exact(&mut content).is_err() {
            eprintln!("python-guard: git cat-file ended early: refusing (fail-closed).");
            return ExitCode::FAILURE;
        }
        content.pop();
        if let Some(why) = verdict(f, &content) {
            bad.push(format!("  {f}: {why}"));
        }
    }
    let _ = writer.join();
    let _ = child.wait();

    if bad.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!("python-guard: no Python in {repo}, by the owner's rule. Refused:");
    for b in &bad {
        eprintln!("{b}");
    }
    eprintln!("Port it to Rust (or a plain shell step), or delete it. A genuine exception goes in {} (owner only).", allow_path.display());
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_python_as_a_word_only() {
        assert!(names_python(b"python3 tool.py"));
        assert!(names_python(b"x=$(python -c 1)"));
        assert!(names_python(b"pip3 install x"));
        assert!(names_python(b"/usr/bin/python3.11 a"));
        assert!(!names_python(b"cat file | pipe_to"));
        assert!(!names_python(b"mypython x"));
        assert!(!names_python(b"echo pipeline"));
        assert!(!names_python(b"    \"tools/python-guard\","));
        assert!(names_python(b"apt install clang python3 git"));
    }

    #[test]
    fn rust_literals() {
        assert!(verdict("a.rs", b"Command::new(\"python3\").arg(\"x\")").is_some());
        assert!(verdict("a.rs", b"Command::new(\"/usr/bin/python\")").is_some());
        assert!(verdict("a.rs", b"// Command::new(\"python3\")").is_none());
        assert!(verdict("a.rs", b"let s = \"no python here\";").is_none());
        assert!(verdict("words.rs", b"b\"work\",b\"pip\",b\"python\",").is_none());
    }

    #[test]
    fn kinds() {
        assert!(verdict("x/a.py", b"").is_some());
        assert!(verdict("tool", b"#!/usr/bin/env python3\nprint(1)").is_some());
        assert!(verdict("b.sh", b"#!/bin/sh\npython3 x").is_some());
        assert!(verdict("b.sh", b"#!/bin/sh\n# python only in a comment\necho ok").is_none());
        assert!(verdict("README.md", b"we used to use python").is_none());
        assert!(verdict("img.png", b"\x89PNG\0python").is_none());
    }

    #[test]
    fn globs() {
        assert!(glob(b"photon/scripts/lib/comment-gate.sh", b"photon/scripts/lib/comment-gate.sh"));
        assert!(glob(b"repo/tools/*.py", b"repo/tools/a/b.py"));
        assert!(!glob(b"repo/tools/*.py", b"repo/src/a.py"));
    }
}
