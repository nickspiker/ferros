//! The recording, made ready at build time (on the host): CAMREPLAY_FRAME names a raw frame pulled from the Pro's main camera (tools/camera-pull.sh: 4080 x 3072 RAW10, stride G#1400, mosaic G B / R G, black 64), binned 2 x 2 per colour into a 2040 x 1536 mosaic of 16-bit samples; the main camera's Absolute IDT is read from the repository's calibration data. The frame is a local file and stays one: it lands in OUT_DIR, never in the tree.

fn main() {
    println!("cargo:rerun-if-env-changed=CAMREPLAY_FRAME");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let path = std::env::var("CAMREPLAY_FRAME").expect("CAMREPLAY_FRAME=<a 4080 x 3072 RAW10 frame from tools/camera-pull.sh>: the recording camreplay serves");
    println!("cargo:rerun-if-changed={path}");
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("CAMREPLAY_FRAME {path}: {e}"));
    let (w, h, stride) = (4080usize, 3072usize, 0x1400usize);
    assert_eq!(raw.len(), h * stride, "{path} is not a 4080 x 3072 RAW10 frame at stride G#1400");
    let px = |x: usize, y: usize| -> u32 {
        let g = y * stride + (x / 4) * 5;
        (raw[g + (x & 3)] as u32) << 2 | (raw[g + 4] as u32 >> (2 * (x & 3))) & 3
    };
    // Binning keeps the mosaic's phase: output sample (ox, oy) is the mean of the four same-colour samples of its 4 x 4 block.
    let (bw, bh) = (w / 2, h / 2);
    let mut bytes = Vec::with_capacity(bw * bh * 2);
    for oy in 0..bh {
        for ox in 0..bw {
            let (x0, y0) = (4 * (ox / 2) + (ox & 1), 4 * (oy / 2) + (oy & 1));
            let s = px(x0, y0) + px(x0 + 2, y0) + px(x0, y0 + 2) + px(x0 + 2, y0 + 2);
            bytes.extend_from_slice(&((s / 4) as u16).to_le_bytes());
        }
    }
    std::fs::write(out.join("frame.bin"), &bytes).unwrap();
    // The IDT, as camera-raw2vsf reads it: the characterization section's Absolute entry, its matrix row-major and its sensitivity.
    let idt = concat!(env!("CARGO_MANIFEST_DIR"), "/../../pixel 8 pro calibration data/derived/cam2-main.idt.vsf");
    println!("cargo:rerun-if-changed={idt}");
    let doc = vsf::container::Document::open(std::fs::File::open(idt).unwrap()).unwrap();
    let sec = doc.section(doc.entry("characterization").expect("the IDT has no characterization")).unwrap();
    let prof = vsf::spectral_image::profile_from_fields(&sec.fields).unwrap();
    let abs = prof.entries.iter().find(|e| e.class == vsf::spectral_image::IdtClass::Absolute).expect("no absolute entry");
    let sens = abs.scale.as_ref().expect("the absolute entry has no scale").sensitivity;
    std::fs::write(out.join("idt.rs"), format!("const IDT: [f32; 9] = {:?};\nconst SENSITIVITY: [f32; 3] = {:?};\n", abs.matrix, sens)).unwrap();
}
