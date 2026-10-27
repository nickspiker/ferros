fn main() {
    // A 32 MiB heap (ferros_rt/ferros.ld's __ferros_heap): the fonts are copied into the font system, the scratch frame and the glyph caches live here; the arc lands in the kernel's big pool.
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap=0x2000000");
}
