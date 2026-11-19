//! The heap ceiling: a 1:1 frame (63 MB) given as the spawn's input, its copies and the codec's output buffers are past the default 2^25 (ferros_rt/ferros.ld `__ferros_heap`).
fn main() {
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 29);
}
