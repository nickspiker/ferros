//! The camera driver's link settings (position-independent, as every payload with formatted console lines links: payloads/torch) and its list, the `camera` endpoint's answer to LIST (docs/CAMERA.md "The endpoint"), made here on the host: a VSF document with one `camera` section, the main camera's readout, mosaic, levels and ranges, and its Absolute IDT read from the repository's calibration data (`pixel 8 pro calibration data/derived/cam2-main.idt.vsf`, the matrix row-major and the sensitivity, as std/camreplay reads it). The driver itself carries no VSF writer: the list is fixed for the camera it drives.

fn main() {
    let ld = std::path::Path::new("payloads/camera/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    for a in ["-pie", "--no-dynamic-linker", "-z", "norelro", "-z", "notext"] {
        println!("cargo:rustc-link-arg={a}");
    }
    println!("cargo:rerun-if-changed={}", ld.display());

    let idt = concat!(env!("CARGO_MANIFEST_DIR"), "/../../pixel 8 pro calibration data/derived/cam2-main.idt.vsf");
    println!("cargo:rerun-if-changed={idt}");
    let doc = vsf::container::Document::open(std::fs::File::open(idt).unwrap()).unwrap();
    let sec = doc.section(doc.entry("characterization").expect("the IDT has no characterization")).unwrap();
    let prof = vsf::spectral_image::profile_from_fields(&sec.fields).unwrap();
    let abs = prof.entries.iter().find(|e| e.class == vsf::spectral_image::IdtClass::Absolute).expect("no absolute entry");
    let sens = abs.scale.as_ref().expect("the absolute entry has no scale").sensitivity;

    use vsf::VsfType;
    let mut s = vsf::file_format::VsfSection::new("camera");
    let mut put = |name: &str, values: Vec<VsfType>| {
        let mut f = vsf::file_format::VsfField::new(name);
        f.values = values;
        s.fields.push(f);
    };
    let u = |v: u64| vec![VsfType::u(v as usize, false)];
    // The main camera (boitata), its captured mode: 4080 x 3072 RAW10, the mosaic G B / R G (two bits a sample, top left first, 0 R 1 G 2 B: G#49), black 64, white 1023, a quarter turn clockwise to stand upright (docs/oracle/CAMERA.md "First colour").
    put("id", u(0));
    put("name", vec![VsfType::a(vsf::Ascii::lit("main"))]);
    put("facing", u(0));
    put("width", u(4080));
    put("height", u(3072));
    put("cfa", u(1 | 2 << 2 | 0 << 4 | 1 << 6));
    put("black", u(64));
    put("white", u(1023));
    put("rotation", u(1));
    put("idt", abs.matrix.iter().map(|v| VsfType::f5(*v)).collect());
    put("sensitivity", sens.iter().map(|v| VsfType::f5(*v)).collect());
    // Exposure: one line (3.2237 µs measured) to the init's longest integration, 21342 lines (68.8 ms), the frame's length bounding it (ferros_cam MAIN_LINE_PS, MAIN_EXPOSURE_MAX_LINES).
    let line_ps = 3_223_700u64;
    put("exposure_min", u(line_ps / 1000));
    put("exposure_max", u(0x535E * line_ps / 1000));
    // Gain: the captured code alone (G#0400 at 1024, unity in the endpoint's units) until how the sensor's code maps to gain is measured; any other is answered outside the range.
    put("gain_min", u(1 << 10));
    put("gain_max", u(1 << 10));
    // Focus: the lens actuator's target, over the span it was measured tracking (G#0800 to G#7800 within 30 ms; G#3800 is about a metre).
    put("focus_min", u(0x0800));
    put("focus_max", u(0x7800));
    let bytes = vsf::VsfBuilder::new().add_section_direct(s).build().expect("the list builds");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("list.vsf"), &bytes).unwrap();
}
