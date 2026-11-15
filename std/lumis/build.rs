fn main() {
    // The heap's ceiling (ferros_rt/ferros.ld's __ferros_heap: a ceiling the arc grows to on demand, never a reservation).
    // 2^30: the integrator's memory for the main camera at bin 2 (2040 x 1536 samples: its four-slot buffer at 16 bytes a sample, 50 MB, the slitscan ring, 17 MB, its accumulators), the UI's own (the frame at 3 bytes a pixel, 12 MB on the Pro, its histogram and arrow buffers), the host's G2t16 copy and fluor's scratch (32 MB each), the menu's two button-state frames, the fonts, and a save's developed plane and its VSF.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ferros") {
        println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 30);
    }
}
