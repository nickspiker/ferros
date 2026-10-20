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
    // FERROS_CAM_FRAME=<file>: embed a `cam.frame` bundle ('CFRM' + init length + sensor init + front-end program) so `diag cam frame` works without the vault. Dev only: the bundle carries the sensor vendor's settings, so it comes from a local file (never the repo) and only into a locally built image.
    println!("cargo:rustc-check-cfg=cfg(cam_frame_blob)");
    println!("cargo:rerun-if-env-changed=FERROS_CAM_FRAME");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("cam_frame.bin");
    match std::env::var("FERROS_CAM_FRAME") {
        Ok(path) => {
            println!("cargo:rerun-if-changed={path}");
            std::fs::copy(&path, &out).unwrap_or_else(|e| panic!("FERROS_CAM_FRAME {path}: {e}"));
            println!("cargo:rustc-cfg=cam_frame_blob");
        }
        Err(_) => std::fs::write(&out, b"").unwrap(),
    }
    // FERROS_PRODUCTION: the "Ready?" gate waits for the power key (dev builds auto-press after two seconds).
    if std::env::var("FERROS_PRODUCTION").is_ok() {
        println!("cargo:rustc-cfg=ferros_production");
    }
    println!("cargo:rustc-check-cfg=cfg(ferros_production)");
    // FERROS_NO_UFS_WP=1: a control build that does not arm the UFS power-on write protect at boot (bisecting the gate's Android path, 2026-10-17).
    println!("cargo:rustc-check-cfg=cfg(ferros_no_ufs_wp)");
    println!("cargo:rerun-if-env-changed=FERROS_NO_UFS_WP");
    if std::env::var("FERROS_NO_UFS_WP").map(|v| v == "1").unwrap_or(false) {
        println!("cargo:rustc-cfg=ferros_no_ufs_wp");
    }
    println!("cargo:rerun-if-env-changed=FERROS_PRODUCTION");
    println!("cargo:rerun-if-changed=ferros_kernel/aarch64.ld");
    println!("cargo:rerun-if-changed=ferros_kernel/aarch64-shim.ld");
    println!("cargo:rerun-if-changed=ferros_kernel/aarch64-pixel8.ld");
    println!("cargo:rerun-if-env-changed=FERROS_SHIM_HANDOFF");
    println!("cargo:rerun-if-env-changed=FERROS_SHIM_ENTRY");
    ready_text();
}

/// The "Ready?" gate text (BOOT.md), rasterised once on the host: Oxanium ExtraLight at 200 px into OUT_DIR/ready.bin = [width u32 LE][height u32 LE][one alpha byte per pixel, row-major]. The kernel only blits it.
fn ready_text() {
    let font_path = format!("{}/../assets/fonts/Oxanium-ExtraLight.ttf", std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed={font_path}");
    let bytes = std::fs::read(&font_path).expect("Oxanium-ExtraLight.ttf");
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).expect("font");
    let px = 200.0;
    let text = "Ready?";
    let glyphs: Vec<_> = text.chars().map(|c| font.rasterize(c, px)).collect();
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
                    let a = bmp[y * m.width + x];
                    let d = &mut img[iy as usize * width + ix as usize];
                    *d = (*d).max(a);
                }
            }
        }
        pen += m.advance_width.round() as i32;
    }
    let mut out = Vec::with_capacity(8 + img.len());
    out.extend_from_slice(&(width as u32).to_le_bytes());
    out.extend_from_slice(&(height as u32).to_le_bytes());
    out.extend_from_slice(&img);
    let dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(std::path::Path::new(&dir).join("ready.bin"), out).unwrap();
}
