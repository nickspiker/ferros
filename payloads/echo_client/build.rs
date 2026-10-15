fn main() {
    let ld = std::path::Path::new("payloads/echo_client/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rerun-if-changed={}", ld.display());
}
