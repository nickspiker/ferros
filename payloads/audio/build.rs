fn main() {
    let ld = std::path::Path::new("payloads/audio/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent, as the display server links: the console's `core::fmt` vtables are absolute words in data, left as .rela.dyn entries that ferros_sys::relocate() applies at entry.
    for a in ["-pie", "--no-dynamic-linker", "-z", "norelro", "-z", "notext"] {
        println!("cargo:rustc-link-arg={a}");
    }
    println!("cargo:rerun-if-changed={}", ld.display());
}
