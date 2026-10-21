fn main() {
    let ld = std::path::Path::new("payloads/display/payload.ld");
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
    // The map's app icons (SHELL.md: an app is its icon), bundled on the host by tools/app-icons.py from each app's own codebase. Embedded rather than read at run time; without the bundle an empty one goes in, and the map draws plain circles.
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../target/icons/icons.bin");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("icons.bin");
    match std::fs::read(&src) {
        Ok(b) => std::fs::write(&out, b).unwrap(),
        Err(_) => std::fs::write(&out, b"").unwrap(),
    }
    println!("cargo:rerun-if-changed={}", src.display());
}
