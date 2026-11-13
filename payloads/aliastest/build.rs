fn main() {
    let ld = std::path::Path::new("payloads/aliastest/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent, as every payload links: absolute words in data are left as .rela.dyn entries that ferros_sys::relocate() applies at entry.
    for a in ["-pie", "--no-dynamic-linker", "-z", "norelro", "-z", "notext"] {
        println!("cargo:rustc-link-arg={a}");
    }
    println!("cargo:rerun-if-changed={}", ld.display());
}
