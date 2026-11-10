//! screencap — what ferros's display server has composited, as a VSF visual: the finished frame (windows, the corners, the glass, composed for the panel by `display capture` into a free surface; the canvas when no window is up), before the panel's table and dither, read over the bridge and written as one Display plane, VSF RGB, gamma2tanh at 16 bits, so the highlights above white keep their headroom.
//! The canvas word is R, G, B, A at 16 bits each (payloads/display draw.rs `Px`), a gamma-2 working value with white at Q14: light = (v / 16384)². Each becomes a g2t code through vsf's own encoder, the exact inverse of how a reader turns the plane back into light.
//! The kernel's memory read hands back up to 4 MiB of DRAM a call (service.rs), so a WQHD canvas is 8 reads; the tool advances by what each read returns, so an older kernel (64 KiB) still works, slowly.
//! Usage: screencap [out.vsf] [width height]   (default target/screen/<time>.vsf, 1344 2992; a jump from Android's scaled mode runs 1008 2244)
//!        screencap --enter [width height]      (a capture each time Enter is pressed, into the folder it was started from as <Eagle time in hex>.vsf; `q` then Enter ends it; tools/snap.sh runs it from anywhere)
//!        screencap --mask [out.vsf] [width height]   (the glass's mask alone, white where the glass shows and black where the mask covers, its edges antialiased as the panel gets them; default mask.vsf here: tools/vsf2web's --mask)

use std::process::Command;
use vsf::colour::convert::delinearize_gamma2tanh_bits_f32;
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

const CANVAS: usize = 0xC800_0000;

/// Where the finished frame is: the display server composes it into a free surface on op 30 (the windows, the corners, the glass, in working values) and says where; the `capture` process (payloads/capture, a display client: the kernel has no door to the display server) asks it and prints the answer. With no window up the canvas is the screen.
/// The capture process built and made flat once, so a run of captures (`--enter`) pays for it only at the start.
fn build_capture() -> std::path::PathBuf {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let elf = repo.join("target/el0/aarch64-unknown-none/release/capture");
    let bin = repo.join("target/el0/aarch64-unknown-none/release/capture.bin");
    let _ = Command::new("cargo").current_dir(&repo).args(["el0", "-q", "-p", "ferros-payload-capture"]).status();
    let _ = Command::new("aarch64-linux-gnu-objcopy").args(["-O", "binary"]).arg(&elf).arg(&bin).status();
    bin
}

fn source(bridge: &std::path::Path, bin: &std::path::Path, mask: bool) -> usize {
    // The capture process's input `m` asks for the mask alone (display op 30 with 1).
    let mut cmd = Command::new(bridge);
    cmd.arg("spawn").arg(bin);
    if mask {
        cmd.arg("6d");
    }
    let out = cmd.args(["--use", "display"]).output().expect("ferros-bridge");
    // The capture's reply, `proc: G#…`, anywhere in a line: other processes' console lines run into it when they have no newline of their own yet (2026-11-03: four captures read the canvas while the modem's log was streaming).
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    // The capture waits on the display server, so the spawn can come back before its answer does (2026-11-07: parked, its `proc:` line on the console a moment later): then the console is asked, a few times a second apart, and its last answer taken.
    for _ in 0..4 {
        if text.contains("proc: G#") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Ok(o) = Command::new(bridge).args(["diag", "console"]).output() {
            let console = String::from_utf8_lossy(&o.stdout).into_owned();
            if let Some(last) = console.rsplit("proc: G#").next().filter(|_| console.contains("proc: G#")) {
                text = format!("proc: G#{last}");
            }
        }
    }
    let hex = text.split("proc: G#").nth(1).map(|s| s.chars().take_while(|c| c.is_ascii_hexdigit()).collect::<String>());
    match hex.filter(|h| !h.is_empty()) {
        Some(h) => usize::from_str_radix(&h, 16).expect("capture address"),
        None => CANVAS,
    }
}
const CHUNK: usize = 4 << 20;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bridge = repo.join("target/debug/ferros-bridge");
    if a.get(1).map(String::as_str) == Some("--mask") {
        let out = a.get(2).cloned().unwrap_or_else(|| "mask.vsf".into());
        let w: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1344);
        let h: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(2992);
        let bin = build_capture();
        capture(&bridge, &bin, &out, w, h, true);
        return;
    }
    if a.get(1).map(String::as_str) == Some("--enter") {
        let w: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1344);
        let h: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2992);
        let bin = build_capture();
        let here = std::env::current_dir().expect("the current folder");
        eprintln!("Enter: capture into {}   q then Enter: done", here.display());
        let mut line = String::new();
        loop {
            line.clear();
            if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 || line.trim() == "q" {
                break;
            }
            // Named by Eagle Time, in hex.
            let out = here.join(format!("{:x}.vsf", vsf::eagle_time_oscillations()));
            capture(&bridge, &bin, &out.to_string_lossy(), w, h, false);
        }
        return;
    }
    let out = a.get(1).cloned().unwrap_or_else(|| {
        let dir = repo.join("target/screen");
        std::fs::create_dir_all(&dir).expect("target/screen");
        // Named by Eagle Time, in hex.
        let t = vsf::eagle_time_oscillations();
        dir.join(format!("{t:x}.vsf")).to_string_lossy().into_owned()
    });
    let w: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1344);
    let h: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2992);
    let bin = build_capture();
    capture(&bridge, &bin, &out, w, h, false);
}

/// One capture: the finished frame's address from the capture process, the frame read over the bridge, written to `out` as a VSF visual.
fn capture(bridge: &std::path::Path, bin: &std::path::Path, out: &str, w: usize, h: usize, mask: bool) {
    let bytes = w * h * 8;
    let tmp = std::env::temp_dir().join("screencap.part");
    let t0 = std::time::Instant::now();
    let base = source(bridge, bin, mask);
    eprintln!("reading G#{base:X}{}", if base == CANVAS { " (the canvas: the capture named no surface)" } else { "" });
    let mut raw = Vec::with_capacity(bytes);
    let mut reads = 0;
    while raw.len() < bytes {
        let want = (bytes - raw.len()).min(CHUNK);
        let st = Command::new(bridge).args(["read", &format!("{:X}", base + raw.len()), &format!("{want:X}"), tmp.to_str().unwrap()]).output().expect("ferros-bridge");
        let got = std::fs::read(&tmp).unwrap_or_default();
        if !st.status.success() || got.is_empty() {
            eprintln!("read failed at +G#{:X}: {}", raw.len(), String::from_utf8_lossy(&st.stderr).lines().last().unwrap_or(""));
            return;
        }
        raw.extend_from_slice(&got[..got.len().min(bytes - raw.len())]);
        reads += 1;
    }
    let _ = std::fs::remove_file(&tmp);
    let read_ms = t0.elapsed().as_millis();
    // Planar [3, h, w]: R plane, G plane, B plane.
    let mut samples = vec![0u16; 3 * w * h];
    for i in 0..w * h {
        let p = u64::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap());
        for c in 0..3 {
            let v = (p >> (16 * c)) as u16 as f32 / 16384.;
            samples[c * w * h + i] = delinearize_gamma2tanh_bits_f32(v * v, 16) as u16;
        }
    }
    let mut plane = Plane::encoded("display", "screen", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "canvas".into();
    let v = Visual { title: "ferros screen".into(), text: "The display server's 16-bit canvas, before the panel's table and dither.".into(), planes: vec![plane], ..Default::default() };
    let file = v.write().expect("visual");
    std::fs::write(out, &file).expect("write");
    println!("{out}: {w}x{h}, {} reads in {read_ms} ms, {} bytes", reads, file.len());
}
