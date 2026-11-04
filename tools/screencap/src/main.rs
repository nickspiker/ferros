//! screencap — what ferros's display server has composited, as a VSF visual: its 16-bit canvas (G#C800_0000, the back buffer before the panel's table and dither) read over the bridge and written as one Display plane, VSF RGB, gamma2tanh at 16 bits, so the highlights above white keep their headroom.
//! The canvas word is R, G, B, A at 16 bits each (payloads/display draw.rs `Px`), a gamma-2 working value with white at Q14: light = (v / 16384)². Each becomes a g2t code through vsf's own encoder, the exact inverse of how a reader turns the plane back into light.
//! The kernel's memory read hands back up to 4 MiB of DRAM a call (service.rs), so a WQHD canvas is 8 reads; the tool advances by what each read returns, so an older kernel (64 KiB) still works, slowly.
//! Usage: screencap [out.vsf] [width height]   (default target/screen/<time>.vsf, 1344 2992; a jump from Android's scaled mode runs 1008 2244)

use std::process::Command;
use vsf::colour::convert::delinearize_gamma2tanh_bits_f32;
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

const CANVAS: usize = 0xC800_0000;
const CHUNK: usize = 4 << 20;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bridge = repo.join("target/debug/ferros-bridge");
    let out = a.get(1).cloned().unwrap_or_else(|| {
        let dir = repo.join("target/screen");
        std::fs::create_dir_all(&dir).expect("target/screen");
        // Named by Eagle Time, in hex.
        let t = vsf::eagle_time_oscillations();
        dir.join(format!("{t:x}.vsf")).to_string_lossy().into_owned()
    });
    let w: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1344);
    let h: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2992);
    let bytes = w * h * 8;
    let tmp = std::env::temp_dir().join("screencap.part");
    let t0 = std::time::Instant::now();
    let mut raw = Vec::with_capacity(bytes);
    let mut reads = 0;
    while raw.len() < bytes {
        let want = (bytes - raw.len()).min(CHUNK);
        let st = Command::new(&bridge).args(["read", &format!("{:X}", CANVAS + raw.len()), &format!("{want:X}"), tmp.to_str().unwrap()]).output().expect("ferros-bridge");
        let got = std::fs::read(&tmp).unwrap_or_default();
        if !st.status.success() || got.is_empty() {
            eprintln!("read failed at +{:#x}: {}", raw.len(), String::from_utf8_lossy(&st.stderr).lines().last().unwrap_or(""));
            std::process::exit(1);
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
    let mut plane = Plane::encoded("screen", "still", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "canvas".into();
    let v = Visual { title: "ferros screen".into(), text: "The display server's 16-bit canvas, before the panel's table and dither.".into(), planes: vec![plane], ..Default::default() };
    let file = v.write().expect("visual");
    std::fs::write(&out, &file).expect("write");
    println!("{out}: {w}x{h}, {} reads in {read_ms} ms, {} bytes", reads, file.len());
}
