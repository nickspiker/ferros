//! The heap ceiling: two 2^24-byte buffers and four thread stacks are past the default 2^25 (ferros_rt/ferros.ld `__ferros_heap`).
fn main() {
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 26);
}
