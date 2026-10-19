fn main() {
    println!("cargo:rustc-link-arg=-Tferros_hv/hv.ld");
    println!("cargo:rerun-if-changed=ferros_hv/hv.ld");
    println!("cargo:rerun-if-changed=ferros_hv/src/header.s");
    // Bisection knobs (HV.md): FERROS_HV_NO_WINDOW=1 forwards every SMC from EL2 (no native re-execution window); FERROS_HV_NO_ATA=1 leaves HCR_EL2.ATA clear; FERROS_HV_ACTLR_KEEP=1 leaves ACTLR_EL2 as found; FERROS_HV_LINUX_AT_EL2=1 hands Linux the machine at EL2 after the relocation and DTB work (the hv vanishes: tests the memory map alone); FERROS_HV_TSC_OFF_AFTER_BOOT=1 stops trapping SMCs on each core once eight CPU_ONs have passed (Linux at EL1 with an inert hv; needs deep idle disabled from Android or power-down wakes land in Linux at EL2).
    // A build tag in the banner: the knobs set, so the pstore log names the variant that wrote it (2026-10-17: a log was read as a stats build's when the flash had not happened).
    let mut tag = String::new();
    for (env, cfg) in [("FERROS_HV_NO_ATA", "hv_no_ata"), ("FERROS_HV_ACTLR_KEEP", "hv_actlr_keep"), ("FERROS_HV_LINUX_AT_EL2", "hv_linux_at_el2"), ("FERROS_HV_TSC_OFF_AFTER_BOOT", "hv_tsc_off_after_boot"), ("FERROS_HV_NO_CHORD_POLL", "hv_no_chord_poll"), ("FERROS_HV_NO_REPORT", "hv_no_report"), ("FERROS_HV_LEAN_RESUME", "hv_lean_resume"), ("FERROS_HV_STAGE2", "hv_stage2"), ("FERROS_HV_NO_PMU_READ", "hv_no_pmu_read")] {
        println!("cargo:rustc-check-cfg=cfg({cfg})");
        println!("cargo:rerun-if-env-changed={env}");
        if std::env::var(env).map(|v| v == "1").unwrap_or(false) {
            println!("cargo:rustc-cfg={cfg}");
            tag.push_str(&cfg[3..]);
            tag.push(' ');
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=HV_BUILD_TAG={} {}", stamp % 100000, if tag.is_empty() { "base".to_string() } else { tag.trim_end().to_string() });
}
