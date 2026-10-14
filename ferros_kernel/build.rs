fn main() {
    // FERROS_SHIM_HANDOFF=1 links ferros at the 0x92400000 handoff carveout (see aarch64-shim.ld) so a running Linux shim can chainload it via cpu_soft_restart and hand off a live UFS link.
    // Unset = cold-boot image: Pixel at 0x8008_0000 (aarch64-pixel8.ld), m1 at 0x80000 (aarch64.ld).
    // The shim_handoff cfg lets ferros pick the post-handoff boot path (skip UFS link startup, adopt the live link, results to scratch).
    let shim = std::env::var("FERROS_SHIM_HANDOFF").is_ok();
    // Pixel cold boot links at ABL's real load address (aarch64-pixel8.ld); m1 keeps aarch64.ld.
    let pixel8 = std::env::var("CARGO_FEATURE_PIXEL8").is_ok();
    let ld = if shim {
        "ferros_kernel/aarch64-shim.ld"
    } else if pixel8 {
        "ferros_kernel/aarch64-pixel8.ld"
    } else {
        "ferros_kernel/aarch64.ld"
    };
    println!("cargo:rustc-link-arg=-T{}", ld);
    if shim {
        println!("cargo:rustc-cfg=shim_handoff");
    }
    println!("cargo:rustc-check-cfg=cfg(shim_handoff)");
    println!("cargo:rerun-if-changed=ferros_kernel/aarch64.ld");
    println!("cargo:rerun-if-changed=ferros_kernel/aarch64-shim.ld");
    println!("cargo:rerun-if-changed=ferros_kernel/aarch64-pixel8.ld");
    println!("cargo:rerun-if-env-changed=FERROS_SHIM_HANDOFF");
    println!("cargo:rerun-if-env-changed=FERROS_SHIM_ENTRY");
}
