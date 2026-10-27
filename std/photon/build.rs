fn main() {
    // A 128 MiB heap (ferros_rt/ferros.ld's __ferros_heap): the fonts, the frame, the chains and the vault cache; the arc lands in the kernel's big pool.
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap=0x8000000");
}
