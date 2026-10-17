fn main() {
    let ld = std::path::Path::new("payloads/shelltest/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent link: absolute addresses in data become .rela.dyn entries that ferros_sys::relocate() applies at entry (the vtables of `dyn` traits, for one — fontdue's outline builder faulted at an unrelocated one on 2026-10-16).
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=norelro");
    // The absolute words live in .rodata; lld only leaves relocations in a read-only section with `-z notext`.
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
    println!("cargo:rerun-if-changed={}", ld.display());
}
