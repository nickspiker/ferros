// camera-raw2vsf — a ferros main-camera frame (MIPI RAW10 packed, 4080 x 3072, stride 5120) into a VSF visual, characterised by the camera's Absolute IDT, plus a preview PNG.
// The visual is the Lumis main-camera capture used as a template (same sensor, same 4080 x 3072 mosaic, black 64, white 1023): its plane's samples are replaced by ferros's, its characterization by the IDT's, and its text says where the frame came from.
// Preview: 2 x 2 cells to camera R, G, B (G = mean of the two greens), black-subtracted, scaled per channel by the IDT's sensitivity (so channel ratios are radiance ratios), through the absolute matrix to linear VSF RGB, then vsf's VSF RGB -> sRGB matrix and the sRGB curve for a screen. Exposure for the preview is a single scale (99.5th percentile of VSF G to 0.9); no white balance: an Absolute IDT keeps the room's cast.
// Usage: camera-raw2vsf <frame.raw10> <lumis main capture.vsf> <idt.vsf> <out.vsf> <out.png> [exposure_s]
use vsf::colour::VSF_RGB2SRGB;
use vsf::colour::convert::apply_matrix_3x3_f32;
use vsf::spectral_image::{IdtClass, PlaneLayout};
use vsf::types::BitPackedTensor;
use vsf::visual::{Samples, Visual};

const W: usize = 4080;
const H: usize = 3072;
const STRIDE: usize = 5120;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    assert!(raw.len() >= H * STRIDE, "short frame");
    let mut px = vec![0u16; W * H];
    for y in 0..H {
        let row = &raw[y * STRIDE..];
        for g in 0..W / 4 {
            let b = &row[g * 5..g * 5 + 5];
            for k in 0..4 {
                px[y * W + g * 4 + k] = ((b[k] as u16) << 2) | ((b[4] as u16 >> (2 * k)) & 3);
            }
        }
    }
    let mut v = Visual::read_file(std::path::Path::new(&a[2])).unwrap();
    let cfa = match &v.planes[0].layout { PlaneLayout::Mosaic { cfa } => cfa.clone(), _ => panic!("template plane is not a mosaic") };
    println!("template cfa {:?} (0 = R, 1 = G, 2 = B as vsf numbers them), {}x{}", cfa, v.planes[0].width, v.planes[0].height);
    // The IDT file is a plain VSF (absidt's sections: characterization, curves, emitter, check), not a visual: read its characterization section directly.
    let doc = vsf::container::Document::open(std::fs::File::open(&a[3]).unwrap()).unwrap();
    let sec = doc.section(doc.entry("characterization").expect("IDT has no characterization")).unwrap();
    let prof = vsf::spectral_image::profile_from_fields(&sec.fields).unwrap();
    println!("template channels {:?}", v.planes[0].channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
    let abs = prof.entries.iter().find(|e| e.class == IdtClass::Absolute).expect("no absolute entry");
    let sens = abs.scale.as_ref().expect("absolute entry has no scale").sensitivity;
    println!("absolute matrix {:?}\nsensitivity {:?}", abs.matrix, sens);
    // The VSF visual.
    {
        let p = &mut v.planes[0];
        assert!(p.width == W && p.height == H, "template is {}x{}", p.width, p.height);
        p.samples = Samples::Packed(BitPackedTensor::pack(10, vec![H, W], &px));
        p.black = vec![64.; 3];
        p.white = vec![1023.; 3];
    }
    v.characterization = Some(prof.clone());
    v.response = vsf::visual::SpectralResponse::from_section(&sec).ok().flatten();
    v.text = format!("ferros cold boot, main camera, front end via PDMA; RAW10 as written to G#A200_0000{}", a.get(6).map(|t| format!("; exposure {t} s estimated from frame timing")).unwrap_or_default());
    if let (Some(t), Some(c)) = (a.get(6), v.capture.as_mut()) {
        c.exposure_s = t.parse().ok();
    }
    std::fs::write(&a[4], v.write().unwrap()).unwrap();
    // Which 2x2 site is which channel, from the template's CFA.
    let site = |c: u8| -> Vec<(usize, usize)> { (0..4).filter(|&i| cfa.data[i] == c).map(|i| (i / 2, i % 2)).collect() };
    let (rs, gs, bs) = (site(0), site(1), site(2));
    println!("sites R {rs:?} G {gs:?} B {bs:?}");
    let (w2, h2) = (W / 2, H / 2);
    let mut lin = vec![[0f32; 3]; w2 * h2];
    for y in 0..h2 {
        for x in 0..w2 {
            let at = |(dy, dx): (usize, usize)| (px[(2 * y + dy) * W + 2 * x + dx] as f32 - 64.) / (1023. - 64.);
            let r = at(rs[0]);
            let g = gs.iter().map(|&s| at(s)).sum::<f32>() / gs.len() as f32;
            let b = at(bs[0]);
            // ProfileEntry.matrix is row-major camera -> VSF RGB (spectral_image.rs); apply_matrix_3x3_f32 reads column-major (vsf's own constants), so the IDT matrix is applied by rows here.
            let (m, c) = (&abs.matrix, [r * sens[0], g * sens[1], b * sens[2]]);
            lin[y * w2 + x] = [0, 1, 2].map(|i| m[i * 3] * c[0] + m[i * 3 + 1] * c[1] + m[i * 3 + 2] * c[2]);
        }
    }
    let mut gv: Vec<f32> = lin.iter().map(|p| p[1]).collect();
    gv.sort_by(|a, b| a.total_cmp(b));
    let scale = 0.9 / gv[(gv.len() as f32 * 0.995) as usize].max(1e-12);
    let oetf = |v: f32| { let v = v.clamp(0., 1.); if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1. / 2.4) - 0.055 } };
    let mut img = image::RgbImage::new(w2 as u32, h2 as u32);
    for y in 0..h2 {
        for x in 0..w2 {
            let p = lin[y * w2 + x];
            let s = apply_matrix_3x3_f32(&VSF_RGB2SRGB, &[p[0] * scale, p[1] * scale, p[2] * scale]);
            img.put_pixel(x as u32, y as u32, image::Rgb([0, 1, 2].map(|c| (oetf(s[c]) * 255. + 0.5) as u8)));
        }
    }
    img.save(&a[5]).unwrap();
    println!("wrote {} and {}", a[4], a[5]);
}
