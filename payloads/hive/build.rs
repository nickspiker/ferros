fn main() {
    let ld = std::path::Path::new("payloads/hive/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent link: absolute addresses in data become .rela.dyn entries that ferros_sys::relocate() applies at entry.
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=norelro");
    // The absolute words live in .rodata; lld only leaves relocations in a read-only section with `-z notext`.
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
    println!("cargo:rerun-if-changed={}", ld.display());
    // The shape-writing lexicon (honeycomb::Lexicon): rhe's English frequency list, most common first, the first 32768 words made only of the letters a to z (no apostrophes: a trace has no key for them), each a length byte then its letters. Built here, never committed: the list is rhe's. Without rhe checked out the lexicon is empty and tracing types nothing.
    let src = "/mnt/Harbor/Code/rhe/data/en_freq.txt";
    println!("cargo:rerun-if-changed={src}");
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    match std::fs::read_to_string(src) {
        Ok(text) => {
            for line in text.lines() {
                let w = line.split_whitespace().next().unwrap_or("");
                if w.is_empty() || w.len() > 24 || !w.bytes().all(|b| b.is_ascii_lowercase()) || !seen.insert(w.to_string()) {
                    continue;
                }
                out.push(w.len() as u8);
                out.extend_from_slice(w.as_bytes());
                if seen.len() == 1 << 15 {
                    break;
                }
            }
        }
        Err(_) => println!("cargo:warning=no {src}: hive's lexicon is empty, tracing types nothing"),
    }
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("lexicon.bin"), out).unwrap();
}
