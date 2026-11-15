//! The camera driver's link settings (position-independent, as every payload with formatted console lines links: payloads/torch) and its lists, the `camera` endpoint's answers to LIST (docs/CAMERA.md "The endpoint"), made here on the host.
//! One `camera` section per camera that can stream (ferros_cam cams: the main, the ultrawide, the front), each with its readout, mosaic, levels, rotation, ranges and its Absolute IDT read from the repository's calibration data (`pixel 8 pro calibration data/derived/`, the matrix row-major and the sensitivity, as std/camreplay reads it).
//! The driver answers LIST with the document of the cameras whose inputs are in the vault, so it carries one document per subset: 'LST8', eight lengths (u32), the eight documents, document k with the sections of the cameras whose bit is set in k (bit 0 the main, 1 the ultrawide, 2 the front). The driver itself carries no VSF writer.

use ferros_cam::cams::{self, Cam};
use vsf::VsfType;

/// Each camera's IDT file, by id (the calibration's module numbers: cam2 the main, cam3 the ultrawide, cam7 the front).
const IDT: [&str; 3] = ["cam2-main", "cam3-ultrawide", "cam7-front"];

fn section(c: &Cam) -> vsf::file_format::VsfSection {
    let idt = format!("{}/../../pixel 8 pro calibration data/derived/{}.idt.vsf", env!("CARGO_MANIFEST_DIR"), IDT[c.id as usize]);
    println!("cargo:rerun-if-changed={idt}");
    let doc = vsf::container::Document::open(std::fs::File::open(&idt).unwrap()).unwrap();
    let sec = doc.section(doc.entry("characterization").expect("the IDT has no characterization")).unwrap();
    let prof = vsf::spectral_image::profile_from_fields(&sec.fields).unwrap();
    let abs = prof.entries.iter().find(|e| e.class == vsf::spectral_image::IdtClass::Absolute).expect("no absolute entry");
    let sens = abs.scale.as_ref().expect("the absolute entry has no scale").sensitivity;

    let mut s = vsf::file_format::VsfSection::new("camera");
    let mut put = |name: &str, values: Vec<VsfType>| {
        let mut f = vsf::file_format::VsfField::new(name);
        f.values = values;
        s.fields.push(f);
    };
    let u = |v: u64| vec![VsfType::u(v as usize, false)];
    put("id", u(c.id as u64));
    put("name", vec![VsfType::a(vsf::Ascii::lit(c.name))]);
    put("facing", u(c.facing as u64));
    put("width", u(c.width as u64));
    put("height", u(c.height as u64));
    put("cfa", u(c.cfa as u64));
    put("black", u(c.black as u64));
    put("white", u(c.white as u64));
    put("rotation", u(c.rotation as u64));
    put("idt", abs.matrix.iter().map(|v| VsfType::f5(*v)).collect());
    put("sensitivity", sens.iter().map(|v| VsfType::f5(*v)).collect());
    // Exposure: one line to the longest offered (the main's the init's own 21342 lines, 68.8 ms; ferros_cam cams for each).
    put("exposure_min", u(c.line_ps / 1000));
    put("exposure_max", u(c.exposure_max_lines * c.line_ps / 1000));
    // Gain: the captured code alone (1024, unity in the endpoint's units) until how each sensor's code maps to gain is measured; any other is answered outside the range.
    put("gain_min", u(1 << 10));
    put("gain_max", u(1 << 10));
    // Focus: the lens actuator's targets over the span measured (the main's G#0800 to G#7800); 0 and 0 where focus is not offered.
    put("focus_min", u(c.focus.0));
    put("focus_max", u(c.focus.1));
    s
}

fn main() {
    let ld = std::path::Path::new("payloads/camera/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    for a in ["-pie", "--no-dynamic-linker", "-z", "norelro", "-z", "notext"] {
        println!("cargo:rustc-link-arg={a}");
    }
    println!("cargo:rerun-if-changed={}", ld.display());
    println!("cargo:rerun-if-changed=../../ferros_cam/src/cams.rs");

    let listed = [&cams::MAIN, &cams::ULTRAWIDE, &cams::FRONT];
    let mut docs = vec![];
    for mask in 0..1usize << listed.len() {
        let mut b = vsf::VsfBuilder::new();
        for (k, c) in listed.iter().enumerate() {
            if mask >> k & 1 == 1 {
                b = b.add_section_direct(section(c));
            }
        }
        docs.push(b.build().expect("the list builds"));
    }
    let mut out = b"LST8".to_vec();
    for d in &docs {
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
    }
    for d in &docs {
        out.extend_from_slice(d);
    }
    let dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(dir.join("lists.bin"), &out).unwrap();
}
