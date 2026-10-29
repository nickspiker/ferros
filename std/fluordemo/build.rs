fn main() {
    // A 32 MiB heap (ferros_rt/ferros.ld's __ferros_heap): the fonts are copied into the font system, the scratch frame and the glyph caches live here; the arc lands in the kernel's big pool.
    // At G2t16 the full-screen frame alone is 32 MiB (1344 × 2992 × 8 B); the heap really has ≈ 60 MiB, since the arc rounds the 3 MiB image plus this up to 64 MiB and ferros_rt's allocator takes the slack.
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap=0x2000000");
}
