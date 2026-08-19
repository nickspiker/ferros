fn main() {
    // FERROS_SHIM_HANDOFF=1 links ferros at the 0x92400000 handoff carveout (see aarch64-shim.ld) so a running Linux shim can chainload it via cpu_soft_restart and hand off a live UFS link.
    // Unset = normal cold-boot image at 0x80000.
    // The shim_handoff cfg lets ferros pick the post-handoff boot path (skip UFS link startup, adopt the live link, results to scratch).
    let shim = std::env::var("FERROS_SHIM_HANDOFF").is_ok();
    let ld = if shim {
        "ferros_kernel/aarch64-shim.ld"
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
    println!("cargo:rerun-if-env-changed=FERROS_SHIM_HANDOFF");
}
