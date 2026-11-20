//! The GPU kernels, compiled here on the host (the 60/s work, 2026-11-19: "Mali kernels compiled on the HOST only, binaries shipped as data"): src/gpu_kernel.rs recorded by valhall's SSA for each camera geometry the recorder knows, written as the words and constants `gpu::PREBUILT` carries.
//! The device never runs the compiler; it uploads these words to its GPU block.

#[path = "src/gpu_kernel.rs"]
#[allow(dead_code)]
mod gpu_kernel;

use std::fmt::Write;

/// The geometries (width, stride) a kernel is built for: the binned main (4080 samples on a G#1400-byte line, payloads/camera `W_MOST`).
const GEOMETRIES: [(usize, usize); 1] = [(4080, 0x1400)];

fn main() {
    println!("cargo:rerun-if-changed=src/gpu_kernel.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let mut s = String::new();
    s.push_str("/// The kernels built on the host (build.rs), one a geometry.\npub static PREBUILT: &[Prebuilt] = &[\n");
    for (w, stride) in GEOMETRIES {
        let k = gpu_kernel::kernel(w, stride);
        let _ = write!(s, "    Prebuilt {{ width: {w}, stride: {stride}, regs64: {}, words: &[", k.regs64);
        for x in &k.words {
            let _ = write!(s, "{x},");
        }
        s.push_str("], consts: &[");
        for x in &k.consts {
            let _ = write!(s, "{x},");
        }
        s.push_str("] },\n");
    }
    s.push_str("];\n");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("kernels.rs");
    std::fs::write(out, s).expect("OUT_DIR is writable");
}
