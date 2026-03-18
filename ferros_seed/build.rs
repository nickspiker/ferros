fn main() {
    let ld = std::path::Path::new("ferros_seed/aarch64.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rerun-if-changed={}", ld.display());
}
