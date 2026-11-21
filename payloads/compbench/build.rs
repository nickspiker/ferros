//! The link (the payload's script, position-independent), and the frame's GPU kernels recorded on the host (src/kernel.rs) into words for the bench to load: the device never runs that compiler.

#[path = "src/layout.rs"]
#[allow(dead_code)]
mod layout;
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
    let shapes = [
        ("NOISE", Shape { out: Out::G2t, layers: false, clip: false }),
        ("G2T", Shape { out: Out::G2t, layers: true, clip: false }),
        ("BAKED", Shape { out: Out::Baked, layers: true, clip: false }),
        ("PHASES", Shape { out: Out::Phases, layers: true, clip: false }),
        ("CLIP", Shape { out: Out::Baked, layers: true, clip: true }),
    ];
    let mut s = String::new();
    for (name, shape) in shapes {
        let k = kernel::record(shape);
        assert!(k.words.len() * 8 <= layout::K_ROOM, "{name}: {} words past its room", k.words.len());
        let _ = writeln!(s, "/// {shape:?}: {} words, {} in the loop, {} nodes kept, peak {} registers.", k.words.len(), k.loop_words, k.kept, k.peak);
        let _ = write!(s, "pub static {name}: Kernel = Kernel {{ regs64: {}, loop_words: {}, words: &[", k.regs64, k.loop_words);
        for x in &k.words {
            let _ = write!(s, "{x},");
        }
        s.push_str("], consts: &[");
        for x in &k.consts {
            let _ = write!(s, "{x},");
        }
        s.push_str("] };\n");
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("kernels.rs");
    std::fs::write(out, s).expect("OUT_DIR is writable");
}
