//! Vault inspector CLI — companion to `vsfinfo`. `vsfinfo` decodes the outer VSF wrapper; `vaultinfo` decodes the inner ferros_vault structure (anchors, object envelopes, root_commit dict, optional decrypt).
//!
//! Arg sniffing: any arg that looks like a key (64 hex chars OR voca PascalCase word concatenation) is treated as a key; everything else is the file path. Order doesn't matter — `vaultinfo file.vsf KEY`, `vaultinfo KEY file.vsf`, `vaultinfo KEY1 file.vsf KEY2` all work. Hex and voca shapes can't collide (hex is all-lowercase digits, voca has inner capitals) so mixing both forms in one invocation is fine.
//!
//! - 0 keys: layout view (slot fields, object envelopes, root_commit dict)
//! - 1 key: treated as anchor_key; HMAC-verifies each slot
//! - 2 keys: tries each as anchor_key directly AND derives anchor_key via `derive_anchor_key` both orderings; the one that HMAC-validates slot 0 wins
//!
//! Object content is shown as `(encrypted, N bytes)` — Photon's per-key ChaCha20-Poly1305 lives in photon/storage/mod.rs; a higher-level inspector there could wrap this output to decrypt content. Not in scope for v1.

use ferros_vault::host_file::inspect_vault;
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();

    if argv.is_empty() || argv.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return ExitCode::from(if argv.is_empty() { 1 } else { 0 });
    }

    let mut file: Option<&str> = None;
    let mut keys: Vec<&str> = Vec::new();
    for arg in &argv {
        if looks_like_key(arg) {
            keys.push(arg);
        } else if file.is_some() {
            eprintln!("error: multiple file paths supplied ({} and {})", file.unwrap(), arg);
            return ExitCode::from(1);
        } else {
            file = Some(arg);
        }
    }

    let Some(path) = file else {
        eprintln!("error: no file path supplied");
        print_usage();
        return ExitCode::from(1);
    };

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error reading {}: {}", path, e);
            return ExitCode::from(1);
        }
    };

    match inspect_vault(&bytes, &keys) {
        Ok(s) => {
            print!("{}", s);
            ExitCode::from(0)
        }
        Err(e) => {
            eprintln!("inspect error: {}", e);
            ExitCode::from(1)
        }
    }
}

/// Classify an arg as key or path. Two shapes both count as keys:
///  - 64 hex chars `[0-9a-fA-F]` — legacy hex form
///  - PascalCase word concatenation with an inner uppercase letter — voca FULL form
///
/// A 32-byte voca-encoded key is ~22 words concatenated, so an inner uppercase is guaranteed. File paths on every sane filesystem either lack inner uppercase entirely or have path separators (`/`) the voca shape doesn't include — no collision in practice.
fn looks_like_key(s: &str) -> bool {
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return true;
    }
    if s.contains('/') || s.contains('\\') || s.contains('.') {
        return false;
    }
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else { return false };
    if !first.is_ascii_uppercase() {
        return false;
    }
    bytes.any(|b| b.is_ascii_uppercase())
}

fn print_usage() {
    eprintln!("usage: vaultinfo [KEY...] FILE [KEY...]");
    eprintln!();
    eprintln!("  KEY  one or two keys (any position). Each is either:");
    eprintln!("         - 64 hex chars (legacy form)");
    eprintln!("         - voca PascalCase word concatenation (e.g. BiasZippyMoment…)");
    eprintln!("       0 keys: layout view");
    eprintln!("       1 key:  treated as anchor_key (HMAC verify)");
    eprintln!("       2 keys: identity_seed + device_secret, any order — derives anchor_key");
    eprintln!("  FILE path to a vault .vsf file");
    eprintln!();
    eprintln!("examples:");
    eprintln!("  vaultinfo ~/.config/photon.vsf");
    eprintln!("  vaultinfo ~/.config/photon.vsf BiasZippyMomentRibbon…");
    eprintln!("  vaultinfo DEADBEEF…64chars ~/.config/photon.vsf");
    eprintln!("  vaultinfo KEY1 KEY2 ~/.config/photon.vsf");
}
