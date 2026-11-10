//! vsf2web: screen captures (screencap's VSF visuals: one Display plane, VSF RGB, gamma2tanh, 16 bits) as 8-bit sRGB AV1 images for the website (owner, 2026-11-10: "convert to sRGB from VSF RGB and do the 8 bit web").
//! Each pixel: its codes back to light through vsf's own gamma2tanh decoder (white at 1), VSF RGB to linear sRGB by vsf's matrix (E white to D65), out-of-gamut colours scaled in by vsf's hue-keeping `scale_to_gamut_f32`, then the sRGB transfer and 8 bits. A half-size copy is averaged 2 x 2 in linear light (the only place averaging means light), for screens that do not need every pixel.
//! The output is AV1 in an AVIF file (ravif, on rav1e): the house image codec, and an image format every current browser draws. 4:4:4, so the text's colour edges stay sharp.
//! Usage: vsf2web <out dir> <capture.vsf>...   → <out dir>/<name>.avif (full size) and <name>-half.avif

use vsf::colour::convert::{linearize_gamma2tanh_bits_f32, scale_to_gamut_f32};
use vsf::colour::{srgb_oetf, VSF_RGB2SRGB};
use vsf::visual::{Samples, Visual};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: vsf2web <out dir> <capture.vsf>...");
        std::process::exit(2);
    }
    let out = std::path::Path::new(&a[1]);
    std::fs::create_dir_all(out).expect("out dir");
    for path in &a[2..] {
        let p = std::path::Path::new(path);
        let v = Visual::read_file(p).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        let plane = &v.planes[0];
        let (w, h) = (plane.width, plane.height);
        let Samples::Packed(t) = &plane.samples else { panic!("{path}: samples not packed") };
        let codes = t.unpack_u16();
        let bits = plane.depth as u32;
        // Linear sRGB light, planar [3, h, w] in, interleaved out.
        let mut lin = vec![[0f32; 3]; w * h];
        for (i, px) in lin.iter_mut().enumerate() {
            let rgb = [0, 1, 2].map(|c| linearize_gamma2tanh_bits_f32(codes[c * w * h + i] as u32, bits));
            // Column-major (vsf constants.rs): entries 0-2 are what VSF red gives sRGB red, green and blue, 3-5 VSF green's, 6-8 VSF blue's. Read as rows, VSF white came out magenta (1.8, 0.32, 0.94).
            let m = &VSF_RGB2SRGB;
            *px = [0, 1, 2].map(|o| m[o] * rgb[0] + m[3 + o] * rgb[1] + m[6 + o] * rgb[2]);
        }
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        write(&out.join(format!("{name}.avif")), &lin, w, h);
        // Half size: each output pixel the mean light of its 2 x 2 (the captures' sides are even).
        let (hw, hh) = (w / 2, h / 2);
        let mut half = vec![[0f32; 3]; hw * hh];
        for y in 0..hh {
            for x in 0..hw {
                let s = [lin[2 * y * w + 2 * x], lin[2 * y * w + 2 * x + 1], lin[(2 * y + 1) * w + 2 * x], lin[(2 * y + 1) * w + 2 * x + 1]];
                half[y * hw + x] = [0, 1, 2].map(|c| (s[0][c] + s[1][c] + s[2][c] + s[3][c]) * 0.25);
            }
        }
        write(&out.join(format!("{name}-half.avif")), &half, hw, hh);
        println!("{path} -> {name}.avif ({w}x{h}), {name}-half.avif ({hw}x{hh})");
    }
}

/// Linear sRGB light to 8-bit sRGB, encoded as AVIF. A colour past the sRGB gamut or above white is brought in by vsf's hue-keeping scale; the cast to u8 is then exact range.
fn write(path: &std::path::Path, lin: &[[f32; 3]], w: usize, h: usize) {
    let px: Vec<rgb::RGB8> = lin
        .iter()
        .map(|&[r, g, b]| {
            let (r, g, b) = scale_to_gamut_f32(r, g, b);
            let q = |v: f32| (srgb_oetf(v) * 255. + 0.5) as u8;
            rgb::RGB8::new(q(r), q(g), q(b))
        })
        .collect();
    let img = imgref::Img::new(px.as_slice(), w, h);
    let enc = ravif::Encoder::new().with_quality(90.).with_speed(4).with_internal_color_model(ravif::ColorModel::RGB);
    let res = enc.encode_rgb(img).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::fs::write(path, &res.avif_file).expect("write");
}
