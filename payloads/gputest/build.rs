//! The link (the payload's script, position-independent), and the queue test's band coder (input 14): ferros_clip's GPU kernel recorded on the host for the binned main's geometry, written as words for the test to load (RAW-VIDEO.md "On the GPU": the device never runs that compiler).

#[path = "../../crates/clip/src/gpu_kernel.rs"]
#[allow(dead_code)]
mod gpu_kernel;

use std::fmt::Write;

fn main() {
    let ld = std::path::Path::new("payloads/gputest/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent link: absolute addresses in data become .rela.dyn entries that ferros_sys::relocate() applies at entry (the vtables of `dyn` traits, for one — fontdue's outline builder faulted at an unrelocated one on 2026-10-16).
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=norelro");
    // The absolute words live in .rodata; lld only leaves relocations in a read-only section with `-z notext`.
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
    println!("cargo:rerun-if-changed={}", ld.display());
    println!("cargo:rerun-if-changed=../../crates/clip/src/gpu_kernel.rs");
    let k = gpu_kernel::kernel(4080, 0x1400);
    let mut s = String::new();
    let _ = write!(s, "/// ferros_clip's band coder for 4080 samples on G#1400-byte rows (build.rs).\npub const BAND_REGS64: bool = {};\npub static BAND_WORDS: &[u64] = &[", k.regs64);
    for x in &k.words {
        let _ = write!(s, "{x},");
    }
    s.push_str("];\npub static BAND_CONSTS: &[u64] = &[");
    for x in &k.consts {
        let _ = write!(s, "{x},");
    }
    s.push_str("];\n");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("band.rs");
    std::fs::write(out, s).expect("OUT_DIR is writable");
}
