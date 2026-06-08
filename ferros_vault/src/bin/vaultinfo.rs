//! Vault inspector CLI — companion to `vsfinfo`. `vsfinfo` decodes the outer VSF wrapper; `vaultinfo` decodes the inner ferros_vault structure (anchors, object envelopes, root_commit dict, optional decrypt).
//!
//! Arg sniffing: any arg that is exactly 64 hex characters is treated as a key; everything else is the file path. Order doesn't matter — `vaultinfo file.vsf HEX`, `vaultinfo HEX file.vsf`, `vaultinfo HEX file.vsf HEX` all work.
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
    let mut hex_keys: Vec<&str> = Vec::new();
    for arg in &argv {
        if is_hex_key(arg) {
            hex_keys.push(arg);
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

    match inspect_vault(&bytes, &hex_keys) {
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

/// A 64-char string of `[0-9a-fA-F]` is unambiguously a key — no path on a sane filesystem looks like that.
fn is_hex_key(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn print_usage() {
    eprintln!("usage: vaultinfo [HEX...] FILE [HEX...]");
    eprintln!();
    eprintln!("  HEX  one or two 64-char hex keys (any position)");
    eprintln!("       0 keys: layout view");
    eprintln!("       1 key:  treated as anchor_key (HMAC verify)");
    eprintln!("       2 keys: identity_seed + device_secret, any order — derives anchor_key");
    eprintln!("  FILE path to a vault .vsf file");
    eprintln!();
    eprintln!("examples:");
    eprintln!("  vaultinfo ~/.config/photon.vsf");
    eprintln!("  vaultinfo ~/.config/photon.vsf DEADBEEF...64chars");
    eprintln!("  vaultinfo HEX1 HEX2 ~/.config/photon.vsf");
}
