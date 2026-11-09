fn main() {
    let ld = std::path::Path::new("payloads/pcie1/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent, as the display (payloads/display/build.rs): absolute addresses in data become .rela.dyn entries that ferros_sys::relocate() applies at entry.
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=norelro");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
    println!("cargo:rerun-if-changed={}", ld.display());
}
