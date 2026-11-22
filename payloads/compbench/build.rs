//! The link (the payload's script, position-independent), and the frame's GPU kernels recorded on the host (src/kernel.rs) into words for the bench to load: the device never runs that compiler.

#[path = "src/layout.rs"]
#[allow(dead_code)]
mod layout;
#[path = "src/noise.rs"]
#[allow(dead_code)]
mod noise;
#[path = "src/kernel.rs"]
#[allow(dead_code)]
mod kernel;

use kernel::{Out, Shape};
use std::fmt::Write;

fn main() {
    let ld = std::path::Path::new("payloads/compbench/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent link, as every payload's (gputest's build.rs has the why).
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=norelro");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
    println!("cargo:rerun-if-changed={}", ld.display());
    println!("cargo:rerun-if-changed=src/kernel.rs");
    println!("cargo:rerun-if-changed=src/layout.rs");
    println!("cargo:rerun-if-changed=src/noise.rs");
    let shape = |out, layers, clip| Shape { out, layers, clip };
    // The serial walk's kernels (one invocation a half-row), then the parallel noise's (a workgroup a half-row, `par` its size log 2): the noise alone at three workgroup sizes, the compose's three outputs and the clip at the size `kernel::PAR` names; the memory floor, a pane copied (`record_copy`, Some(0)); and the compose into one phase (GPU-COMPOSE.md step 0).
    let kernels: [(&str, Shape, Option<u32>); 14] = [
        ("NOISE", shape(Out::G2t, false, false), None),
        ("G2T", shape(Out::G2t, true, false), None),
        ("BAKED", shape(Out::Baked, true, false), None),
        ("PHASES", shape(Out::Phases, true, false), None),
        ("CLIP", shape(Out::Baked, true, true), None),
        ("NOISE_P6", shape(Out::G2t, false, false), Some(6)),
        ("NOISE_P7", shape(Out::G2t, false, false), Some(7)),
        ("NOISE_P8", shape(Out::G2t, false, false), Some(8)),
        ("G2T_P", shape(Out::G2t, true, false), Some(kernel::PAR)),
        ("BAKED_P", shape(Out::Baked, true, false), Some(kernel::PAR)),
        ("PHASES_P", shape(Out::Phases, true, false), Some(kernel::PAR)),
        ("CLIP_P", shape(Out::Baked, true, true), Some(kernel::PAR)),
        ("COPY", shape(Out::G2t, false, false), Some(0)),
        ("PHASE1_P", shape(Out::Phase1, true, false), Some(kernel::PAR)),
    ];
    let mut s = String::new();
    let mut total = 0;
    for (name, shape, par) in kernels {
        let k = match par {
            None => kernel::record(shape),
            Some(0) => kernel::record_copy(),
            Some(wg) => kernel::record_par(shape, wg),
        };
        total += k.words.len().next_multiple_of(16) + 16;
        assert!(k.words.len() * 8 <= layout::K_ROOM, "{name}: {} words past its room", k.words.len());
        let _ = writeln!(s, "/// {shape:?}, workgroup 2^{}: {} words, {} in the loop, {} nodes kept, peak {} registers, {} bytes of shared storage.", k.wg_log2, k.words.len(), k.loop_words, k.kept, k.peak, k.shared_bytes);
        let _ = write!(s, "pub static {name}: Kernel = Kernel {{ regs64: {}, loop_words: {}, wg_log2: {}, shared_bytes: {}, words: &[", k.regs64, k.loop_words, k.wg_log2, k.shared_bytes);
        for x in &k.words {
            let _ = write!(s, "{x},");
        }
        s.push_str("], consts: &[");
        for x in &k.consts {
            let _ = write!(s, "{x},");
        }
        s.push_str("] };\n");
    }
    // WHY: the GPU service keeps a client's loaded kernels in 32 pages (2^14 words, each kernel's words rounded to 16 and 16 zero words after); past them the last loads are refused on the device.
    assert!(total <= 1 << 14, "the kernels take {total} of the service's 2^14 words");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("kernels.rs");
    std::fs::write(out, s).expect("OUT_DIR is writable");
}
