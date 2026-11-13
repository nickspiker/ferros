fn main() {
    // A 512 MiB heap ceiling (it grows on demand, docs/ARCS.md step 2): the probe's 96 MiB values are held about three times over in its own heap (the value, kete's sealed copy, the read back and its opening); at Photon's 128 MiB the first 96 MiB write ran the probe out (2026-11-13, the 8a: "memory allocation of 100663312 bytes failed").
    println!("cargo:rustc-link-arg=--defsym=__ferros_heap=0x20000000");
}
