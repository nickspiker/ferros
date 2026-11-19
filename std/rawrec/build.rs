//! The heap ceiling: four frame copies kept for the check after the clip (16 MiB each), the coded frames waiting for the flash and the workers' buffers are past the default 2^25 (ferros_rt/ferros.ld `__ferros_heap`).
fn main() {
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 29);
}
