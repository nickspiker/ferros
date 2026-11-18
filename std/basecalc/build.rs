fn main() {
    // The heap's ceiling (ferros_rt/ferros.ld's __ferros_heap: a ceiling the arc grows to on demand, never a reservation).
    // 2^26: fluor's scratch frame for the whole glass at 16 bits (1344 x 2992 x 8 B, 32 MiB on the Pro), the fonts and their glyph caches, and GMP's numbers (a thousand digits is a few KiB).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ferros") {
        println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 26);
    }
}
