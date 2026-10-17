fn main() {
    println!("cargo:rustc-link-arg=-Tferros_hv/hv.ld");
    println!("cargo:rerun-if-changed=ferros_hv/hv.ld");
    println!("cargo:rerun-if-changed=ferros_hv/src/header.s");
}
