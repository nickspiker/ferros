fn main() {
    ready_text();
    println!("cargo:rustc-link-arg=-Tferros_hv/hv.ld");
    println!("cargo:rerun-if-changed=ferros_hv/hv.ld");
    println!("cargo:rerun-if-changed=ferros_hv/src/header.s");
    // Bisection knobs (HV.md): FERROS_HV_NO_WINDOW=1 forwards every SMC from EL2 (no native re-execution window); FERROS_HV_NO_ATA=1 leaves HCR_EL2.ATA clear; FERROS_HV_ACTLR_KEEP=1 leaves ACTLR_EL2 as found; FERROS_HV_LINUX_AT_EL2=1 hands Linux the machine at EL2 after the relocation and DTB work (the hv vanishes: tests the memory map alone); FERROS_HV_TSC_OFF_AFTER_BOOT=1 stops trapping SMCs on each core once eight CPU_ONs have passed (Linux at EL1 with an inert hv; needs deep idle disabled from Android or power-down wakes land in Linux at EL2).
    // A build tag in the banner: the knobs set, so the pstore log names the variant that wrote it (2026-10-17: a log was read as a stats build's when the flash had not happened).
    let mut tag = String::new();
    for (env, cfg) in [("FERROS_HV_NO_ATA", "hv_no_ata"), ("FERROS_HV_ACTLR_KEEP", "hv_actlr_keep"), ("FERROS_HV_LINUX_AT_EL2", "hv_linux_at_el2"), ("FERROS_HV_TSC_OFF_AFTER_BOOT", "hv_tsc_off_after_boot"), ("FERROS_HV_NO_CHORD_POLL", "hv_no_chord_poll"), ("FERROS_HV_NO_REPORT", "hv_no_report"), ("FERROS_HV_LEAN_RESUME", "hv_lean_resume"), ("FERROS_HV_STAGE2", "hv_stage2"), ("FERROS_HV_NO_PMU_READ", "hv_no_pmu_read"), ("FERROS_HV_ORACLE", "hv_oracle"), ("FERROS_HV_PDMA_DUMP", "hv_pdma_dump"), ("FERROS_HV_ORACLE_SMC", "hv_oracle_smc"), ("FERROS_HV_MODEM_RING", "hv_modem_ring"), ("FERROS_HV_ORACLE_STEP", "hv_oracle_step"), ("FERROS_HV_SNAPSHOT", "hv_snapshot"), ("FERROS_HV_XPLANT", "hv_xplant")] {
        println!("cargo:rustc-check-cfg=cfg({cfg})");
        println!("cargo:rerun-if-env-changed={env}");
        if std::env::var(env).map(|v| v == "1").unwrap_or(false) {
            println!("cargo:rustc-cfg={cfg}");
            tag.push_str(&cfg[3..]);
            tag.push(' ');
        }
    }
    // FERROS_HV_ORACLE_PAGES: the pages the oracle unmaps, as a comma-separated list of hex addresses ("G#" prefix or bare hex, underscores allowed); a range "start-end" takes every 4 KiB page in it. Defaults to the touch SPI controller. Generated into OUT_DIR/oracle_targets.rs.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_PAGES");
    let spec = std::env::var("FERROS_HV_ORACLE_PAGES").unwrap_or_else(|_| "G#111D_0000".to_string());
    let parse = |t: &str| -> u64 {
        let t = t.trim().trim_start_matches("G#").trim_start_matches("g#").replace('_', "");
        u64::from_str_radix(&t, 16).unwrap_or_else(|_| panic!("FERROS_HV_ORACLE_PAGES: bad address {t:?}"))
    };
    let mut pages: Vec<u64> = Vec::new();
    for item in spec.split(',').filter(|i| !i.trim().is_empty()) {
        if let Some((a, b)) = item.split_once('-') {
            let (a, b) = (parse(a) & !0xFFF, parse(b));
            let mut p = a;
            while p < b {
                pages.push(p);
                p += 0x1000;
            }
        } else {
            pages.push(parse(item) & !0xFFF);
        }
    }
    pages.sort();
    pages.dedup();
    let list: Vec<String> = pages.iter().map(|p| format!("{p:#x}")).collect();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("oracle_targets.rs");
    // FERROS_HV_ORACLE_CAP: how many trapped accesses get a log line (default 2048); later ones are still emulated, only not logged. The console ring is ~2 MiB and a line ~52 bytes, so ~16k lines is the useful ceiling.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_CAP");
    let cap: u64 = std::env::var("FERROS_HV_ORACLE_CAP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(2048);
    // FERROS_HV_ORACLE_ARM_GAP: seconds the trapped pages must sit idle before logging starts (default 0 = log from the first access). With a gap, a boot-time burst (the camera service reading calibration at ~30 s) goes unlogged and the next burst after the quiet is logged from its first access.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_ARM_GAP");
    let gap: u64 = std::env::var("FERROS_HV_ORACLE_ARM_GAP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    // FERROS_HV_ORACLE_QUIET: exact register addresses whose accesses are emulated but not logged — a known controller's setup and status registers, so only the content registers (target, start, data) fill the log. Same syntax as the page list, no ranges.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_QUIET");
    let mut quiet: Vec<u64> = std::env::var("FERROS_HV_ORACLE_QUIET").unwrap_or_default().split(',').filter(|i| !i.trim().is_empty()).map(|i| parse(i)).collect();
    quiet.sort();
    quiet.dedup();
    let qlist: Vec<String> = quiet.iter().map(|p| format!("{p:#x}")).collect();
    // FERROS_HV_ORACLE_ARM_AFTER_MS: no logging before this counter time in ms (the hv's counter runs close to the kernel's printk clock, measured 2026-10-19), so boot-time bursts are skipped and logging starts at the first access after it.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_ARM_AFTER_MS");
    let after: u64 = std::env::var("FERROS_HV_ORACLE_ARM_AFTER_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    // FERROS_HV_ORACLE_CENSUS=1: log only the first access to each target page — which pages a subsystem uses, before a full capture of just those.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_CENSUS");
    let census = std::env::var("FERROS_HV_ORACLE_CENSUS").map(|v| v == "1").unwrap_or(false);
    // FERROS_HV_ORACLE_SMC=1: also log every forwarded SMC that is not PSCI (function ID, x1-x3, and x0-x1 on return), after ORACLE_ARM_AFTER_MS and up to FERROS_HV_ORACLE_SMC_CAP lines (default 4096): power domains, PMU and other EL3-owned steps of a bring-up show up there and nowhere else.
    println!("cargo:rerun-if-env-changed=FERROS_HV_ORACLE_SMC_CAP");
    let smc_cap: u64 = std::env::var("FERROS_HV_ORACLE_SMC_CAP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(4096);
    std::fs::write(&out, format!("pub const ORACLE_TARGETS: [u64; {}] = [{}];\npub const ORACLE_CAP: u64 = {};\npub const ORACLE_ARM_GAP_S: u64 = {};\npub const ORACLE_QUIET: [u64; {}] = [{}];\npub const ORACLE_ARM_AFTER_MS: u64 = {};\npub const ORACLE_CENSUS: bool = {};\npub const ORACLE_SMC_CAP: u64 = {};\n", pages.len(), list.join(", "), cap, gap, quiet.len(), qlist.join(", "), after, census, smc_cap)).unwrap();
    // FERROS_HV_SNAP_LIST: a file of register addresses, one hex address per line (tools/camera-snap-list.py writes one), that a FERROS_HV_SNAPSHOT=1 build reads and logs mid-stream: Linux's live register state, for a diff against a cold ferros reading the same list. Generated into OUT_DIR/snap_list.rs.
    println!("cargo:rerun-if-env-changed=FERROS_HV_SNAP_LIST");
    let snap: Vec<String> = match std::env::var("FERROS_HV_SNAP_LIST") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("FERROS_HV_SNAP_LIST: cannot read {path}")).lines().filter(|l| !l.trim().is_empty()).map(|l| format!("{:#x}", parse(l))).collect()
        }
        Err(_) => Vec::new(),
    };
    std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("snap_list.rs"), format!("pub const SNAP_LIST: [u32; {}] = [{}];\n", snap.len(), snap.join(", "))).unwrap();
    // FERROS_HV_XPLANT_LIST: a file of writes ("<addr> <value>" in hex, or "delay <us>") that a FERROS_HV_XPLANT=1 build applies on top of Linux's running camera at the same trigger as the snapshot: ferros's own sequences transplanted into a working pipeline, to see which of them stops it. Generated into OUT_DIR/xplant_list.rs as (address, value) pairs, address 0 meaning a delay of `value` microseconds.
    println!("cargo:rerun-if-env-changed=FERROS_HV_XPLANT_LIST");
    let xp: Vec<String> = match std::env::var("FERROS_HV_XPLANT_LIST") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("FERROS_HV_XPLANT_LIST: cannot read {path}")).lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).map(|l| {
                let mut it = l.split_whitespace();
                let (a, v) = (it.next().unwrap(), it.next().unwrap_or_else(|| panic!("FERROS_HV_XPLANT_LIST: {l:?}")));
                if a == "delay" { format!("(0, {})", v.parse::<u64>().unwrap()) } else { format!("({:#x}, {:#x})", parse(a), parse(v)) }
            }).collect()
        }
        Err(_) => Vec::new(),
    };
    std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("xplant_list.rs"), format!("pub const XPLANT_LIST: [(u64, u64); {}] = [{}];\n", xp.len(), xp.join(", "))).unwrap();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=HV_BUILD_TAG={} {}", stamp % 100000, if tag.is_empty() { "base".to_string() } else { tag.trim_end().to_string() });
}

/// The boot gate's "Ready?" (gate.rs), rasterised on the host exactly as ferros_kernel's build.rs does: Oxanium ExtraLight at 200 px into OUT_DIR/ready.bin = [width u32 LE][height u32 LE][one alpha byte per pixel].
fn ready_text() {
    let font_path = format!("{}/../assets/fonts/Oxanium-ExtraLight.ttf", std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed={font_path}");
    let bytes = std::fs::read(&font_path).expect("Oxanium-ExtraLight.ttf");
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).expect("font");
    let px = 200.0;
    let glyphs: Vec<_> = "Ready?".chars().map(|c| font.rasterize(c, px)).collect();
    let line = font.horizontal_line_metrics(px).expect("line metrics");
    let ascent = line.ascent.ceil() as i32;
    let height = (line.ascent - line.descent).ceil() as usize;
    let width: usize = glyphs.iter().map(|(m, _)| m.advance_width.round() as usize).sum::<usize>() + 4;
    let mut img = vec![0u8; width * height];
    let mut pen = 0i32;
    for (m, bmp) in &glyphs {
        let x0 = pen + m.xmin;
        let y0 = ascent - m.height as i32 - m.ymin;
        for y in 0..m.height {
            for x in 0..m.width {
                let (ix, iy) = (x0 + x as i32, y0 + y as i32);
                if ix >= 0 && iy >= 0 && (ix as usize) < width && (iy as usize) < height {
                    let d = &mut img[iy as usize * width + ix as usize];
                    *d = (*d).max(bmp[y * m.width + x]);
                }
            }
        }
        pen += m.advance_width.round() as i32;
    }
    let mut out = Vec::with_capacity(8 + img.len());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&img);
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("ready.bin"), out).unwrap();
}
