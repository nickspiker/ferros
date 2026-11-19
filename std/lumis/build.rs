fn main() {
    // The heap's ceiling (ferros_rt/ferros.ld's __ferros_heap: a ceiling the arc grows to on demand, never a reservation).
    // 2^32: the integrator's memory for the main camera's 1:1 readout (8160 x 6144 samples, ferros cam-full-res: its four-slot buffer at 16 bytes a sample, 802 MB, and the slitscan ring, 267 MB; its sums and the previous frame, 500 MB more, only for a stack past 64 frames, Difference, Motion or a calibration, lumis 892bb70), the UI's own (the frame at 3 bytes a pixel, 12 MB on the Pro, the slitscan wiper's layer as much again, its histogram and arrow buffers), fluor's scratch (32 MB), the menu's frames, the fonts, and a save's copy of the slot (100 MB at 50 MP), its developed plane, its AV1 encoder and its VSF. 2^31 held the binned readout (12.5 MP: 200 MB of slots, 67 MB of ring). A ceiling the arc grows to, never a reservation (docs/ARCS.md: the frames come from the one pool as the heap asks).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ferros") {
        println!("cargo:rustc-link-arg=--defsym=__ferros_heap={}", 1u64 << 32);
    }
}
