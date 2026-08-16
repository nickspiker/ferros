fn main() {
    let ld = std::path::Path::new("payloads/slot1nop/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rerun-if-changed={}", ld.display());
}
