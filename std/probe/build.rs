fn main() {
    // A 128 MiB heap, Photon's size: the probe through the streamed spawn exercises the big arc and the big zeroed tail before Photon does.
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap=0x8000000");
}
