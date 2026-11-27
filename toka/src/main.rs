//! toka: compile a WebAssembly module (.wasm, or .wat text) into a masked ferros image the sandbox checker has passed (docs/TOKA.md).
//!
//! Usage: toka <in.wasm|in.wat> [-o <out.bin>] [--fixed K | --exact] [--bti] [--reserve LOG2]

use std::process::exit;
use toka::{Mask, Options};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = Options::default();
    let mut input = None;
    let mut out = None;
    let mut k = 0;
    while k < args.len() {
        match args[k].as_str() {
            "-o" => {
                k += 1;
                out = args.get(k).cloned();
            }
            "--fixed" => {
                k += 1;
                let v: u32 = args.get(k).and_then(|v| v.parse().ok()).filter(|v| (16..=40).contains(v)).unwrap_or_else(|| usage("--fixed needs K, 16 to 40"));
                opts.mask = Mask::Fixed(v);
            }
            "--exact" => opts.mask = Mask::Exact,
            "--bti" => opts.bti = true,
            "--reserve" => {
                k += 1;
                opts.memory_reserve_log2 = args.get(k).and_then(|v| v.parse().ok()).unwrap_or_else(|| usage("--reserve needs a power of two"));
            }
            a if input.is_none() => input = Some(a.to_string()),
            a => usage(&format!("unexpected argument {a}")),
        }
        k += 1;
    }
    if opts.mask == Mask::Exact && !opts.bti {
        usage("--exact needs --bti");
    }
    let input = input.unwrap_or_else(|| usage("no input"));
    let raw = std::fs::read(&input).unwrap_or_else(|e| {
        eprintln!("toka: {input}: {e}");
        exit(2)
    });
    let bytes = if input.ends_with(".wat") {
        wat_to_wasm(&input)
    } else {
        raw
    };
    match toka::compile(&bytes, &opts) {
        Ok(image) => {
            if let Some(out) = out {
                std::fs::write(&out, &image.bytes).unwrap_or_else(|e| {
                    eprintln!("toka: {out}: {e}");
                    exit(2)
                });
            }
            eprintln!(
                "{input}: {} functions, {} bytes of code, {} instructions, 0 refused; image {} bytes",
                image.functions.len(),
                image.text_len,
                image.checked,
                image.bytes.len()
            );
        }
        Err(e) => {
            eprintln!("toka: {input}: {e}");
            exit(1)
        }
    }
}

/// .wat through wasm-tools (a developer's tool; the device takes binary modules only).
fn wat_to_wasm(path: &str) -> Vec<u8> {
    let out = std::process::Command::new("wasm-tools").args(["parse", path]).output().unwrap_or_else(|e| {
        eprintln!("toka: wasm-tools: {e}");
        exit(2)
    });
    if !out.status.success() {
        eprintln!("toka: {}", String::from_utf8_lossy(&out.stderr));
        exit(1)
    }
    out.stdout
}

fn usage(why: &str) -> ! {
    eprintln!("toka: {why}\nusage: toka <in.wasm|in.wat> [-o <out.bin>] [--fixed K | --exact] [--bti] [--reserve LOG2]");
    exit(2)
}
