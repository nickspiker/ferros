fn main() {
    // The heap's ceiling (ferros_rt/ferros.ld's __ferros_heap: a ceiling the arc grows to on demand, never a reservation).
    // 2^31: the integrator's memory for the main camera's whole frames (4080 x 3072 samples, docs/CAMERA.md "The view": its four-slot buffer at 16 bytes a sample, 200 MB, the slitscan ring, its accumulators, 125 MB), the frame unpacked out of the view (25 MB), the UI's own (the frame at 3 bytes a pixel, 12 MB on the Pro, its histogram and arrow buffers), the host's G2t16 copy and fluor's scratch (32 MB each), the menu's two button-state frames, the fonts, and a save's developed plane, its AV1 encoder and its VSF at 12 MP. 2^30 held it at bin 2.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ferros") {
        println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 31);
    }
}
