fn main() {
    let ld = std::path::Path::new("payloads/touch/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rerun-if-changed={}", ld.display());
}
