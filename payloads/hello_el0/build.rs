fn main() {
    let ld = std::path::Path::new("payloads/hello_el0/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rerun-if-changed={}", ld.display());
}
