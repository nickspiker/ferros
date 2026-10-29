fn main() {
    // A 128 MiB heap (ferros_rt/ferros.ld's __ferros_heap): the fonts, the frame, the chains and the vault cache; the arc lands in the kernel's big pool.
    // At G2t16 the frames take ≈ 100 MB; the heap really has ≈ 156 MiB, since the arc rounds the ≈ 100 MiB development image plus this up to 256 MiB and ferros_rt's allocator takes the slack. Declaring more buys nothing inside that arc and only spends the image's room to grow before the arc doubles to 512 MiB (the whole big pool, cachemap POOL_BIG).
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap=0x8000000");
}
