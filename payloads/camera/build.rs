//! The camera driver's link settings (position-independent, as every payload with formatted console lines links: payloads/torch).
//! Its list is built at LIST from each camera's IDT in the vault (ferros_cam list), not here: the IDT is the device's calibration, kept beside the sensor's init.

fn main() {
    let ld = std::path::Path::new("payloads/camera/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    for a in ["-pie", "--no-dynamic-linker", "-z", "norelro", "-z", "notext"] {
        println!("cargo:rustc-link-arg={a}");
    }
    println!("cargo:rerun-if-changed={}", ld.display());
}
