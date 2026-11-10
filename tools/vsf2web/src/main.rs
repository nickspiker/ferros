//! vsf2web: screen captures (screencap's VSF visuals: one Display plane, VSF RGB, gamma2tanh, 16 bits) as 8-bit sRGB AV1 images for the website (owner, 2026-11-10: "convert to sRGB from VSF RGB and do the 8 bit web").
//! Each pixel: its codes back to light through vsf's own gamma2tanh decoder (white at 1), VSF RGB to linear sRGB by vsf's matrix (colorimetric, E white kept as E light), out-of-gamut colours scaled in by vsf's hue-keeping `scale_to_gamut_f32`, then the sRGB transfer and 8 bits.
//! With `--mask` (screencap --mask: the glass's mask alone over white; owner, 2026-11-10: "pull that screen mask for the edges/cam, de premultiply it, apply alpha"): the device composites in gamma-2 working values, so a capture is its content's working values times the share of the glass that shows (the mask is black on the release look), and that share is the mask's working value, the root of its light. Each pixel's colour is then its working values divided by the share, squared back to light, and the share is its alpha: outside the glass transparent, the corners' and the camera hole's antialiasing as the panel gets it. The half-size copy averages working values and shares 2 x 2 (premultiplied, in the space the device composited in). Without a mask the image is opaque and the half size averages light.
//! The output is AV1 in an AVIF file (ravif, on rav1e): the house image codec, and an image format every current browser draws.
//! Usage: vsf2web [--mask mask.vsf] <out dir> <capture.vsf>...   → <out dir>/<name>.avif (full size) and <name>-half.avif

use vsf::colour::convert::{linearize_gamma2tanh_bits_f32, scale_to_gamut_f32};
// sRGB on purpose: the web's encoding, for the website only (vsf marks its xy-based spaces deprecated for everything else).
#[allow(deprecated)]
use vsf::colour::{srgb_oetf, VSF_RGB2SRGB};
use vsf::visual::{Samples, Visual};

/// A capture's light, VSF RGB, white at 1: (width, height, pixels).
fn light(path: &str) -> (usize, usize, Vec<[f32; 3]>) {
    let v = Visual::read_file(std::path::Path::new(path)).unwrap_or_else(|e| panic!("{path}: {e:?}"));
    let plane = &v.planes[0];
    let (w, h) = (plane.width, plane.height);
    let Samples::Packed(t) = &plane.samples else { panic!("{path}: samples not packed") };
    let codes = t.unpack_u16();
    let bits = plane.depth as u32;
    let px = (0..w * h).map(|i| [0, 1, 2].map(|c| linearize_gamma2tanh_bits_f32(codes[c * w * h + i] as u32, bits))).collect();
    (w, h, px)
}

fn main() {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let mask = if a.first().map(String::as_str) == Some("--mask") && a.len() > 1 {
        let m = light(&a[1]);
        a.drain(..2);
        Some(m)
    } else {
        None
    };
    if a.len() < 2 {
        eprintln!("usage: vsf2web [--mask mask.vsf] <out dir> <capture.vsf>...");
        std::process::exit(2);
    }
    let out = std::path::Path::new(&a[0]);
    std::fs::create_dir_all(out).expect("out dir");
    for path in &a[1..] {
        let (w, h, lin) = light(path);
        let name = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().into_owned();
        let (hw, hh) = (w / 2, h / 2);
        match &mask {
            Some((mw, mh, m)) => {
                assert!((*mw, *mh) == (w, h), "{path}: {w}x{h}, the mask {mw}x{mh}");
                // Working values (the root of light) and the glass's share (the mask's, its channels equal: green taken).
                let wv: Vec<[f32; 4]> = lin.iter().zip(m.iter()).map(|(p, q)| [p[0].sqrt(), p[1].sqrt(), p[2].sqrt(), q[1].sqrt()]).collect();
                write_rgba(&out.join(format!("{name}.avif")), &wv, w, h);
                let mut half = vec![[0f32; 4]; hw * hh];
                for y in 0..hh {
                    for x in 0..hw {
                        let s = [wv[2 * y * w + 2 * x], wv[2 * y * w + 2 * x + 1], wv[(2 * y + 1) * w + 2 * x], wv[(2 * y + 1) * w + 2 * x + 1]];
                        half[y * hw + x] = [0, 1, 2, 3].map(|c| (s[0][c] + s[1][c] + s[2][c] + s[3][c]) * 0.25);
                    }
                }
                write_rgba(&out.join(format!("{name}-half.avif")), &half, hw, hh);
            }
            None => {
                write_rgb(&out.join(format!("{name}.avif")), &lin, w, h);
                // Half size: each output pixel the mean light of its 2 x 2 (the captures' sides are even).
                let mut half = vec![[0f32; 3]; hw * hh];
                for y in 0..hh {
                    for x in 0..hw {
                        let s = [lin[2 * y * w + 2 * x], lin[2 * y * w + 2 * x + 1], lin[(2 * y + 1) * w + 2 * x], lin[(2 * y + 1) * w + 2 * x + 1]];
                        half[y * hw + x] = [0, 1, 2].map(|c| (s[0][c] + s[1][c] + s[2][c] + s[3][c]) * 0.25);
                    }
                }
                write_rgb(&out.join(format!("{name}-half.avif")), &half, hw, hh);
            }
        }
        println!("{path} -> {name}.avif ({w}x{h}), {name}-half.avif ({hw}x{hh}){}", if mask.is_some() { ", alpha from the glass's mask" } else { "" });
    }
}

/// VSF RGB light to 8-bit sRGB: vsf's matrix (column-major: entries 0-2 are what VSF red gives sRGB red, green and blue, 3-5 VSF green's, 6-8 VSF blue's; read as rows, white came out magenta), a colour past the gamut or above white brought in by vsf's hue-keeping scale, the sRGB transfer; the cast to u8 is then exact range.
#[allow(deprecated)]
fn srgb8(rgb: [f32; 3]) -> [u8; 3] {
    let m = &VSF_RGB2SRGB;
    let [r, g, b] = [0, 1, 2].map(|o| m[o] * rgb[0] + m[3 + o] * rgb[1] + m[6 + o] * rgb[2]);
    let (r, g, b) = scale_to_gamut_f32(r, g, b);
    [r, g, b].map(|v| (srgb_oetf(v) * 255. + 0.5) as u8)
}

fn encoder() -> ravif::Encoder {
    ravif::Encoder::new().with_quality(90.).with_alpha_quality(95.).with_speed(4).with_internal_color_model(ravif::ColorModel::RGB)
}

fn write_rgb(path: &std::path::Path, lin: &[[f32; 3]], w: usize, h: usize) {
    let px: Vec<rgb::RGB8> = lin.iter().map(|&p| { let [r, g, b] = srgb8(p); rgb::RGB8::new(r, g, b) }).collect();
    let res = encoder().encode_rgb(imgref::Img::new(px.as_slice(), w, h)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::fs::write(path, &res.avif_file).expect("write");
}

/// Premultiplied working values and the share `[r, g, b, a]` to straight 8-bit sRGB with alpha. Where nothing of the glass shows the pixel is transparent black.
fn write_rgba(path: &std::path::Path, wv: &[[f32; 4]], w: usize, h: usize) {
    let px: Vec<rgb::RGBA8> = wv
        .iter()
        .map(|&[r, g, b, a]| {
            if a <= 0. {
                return rgb::RGBA8::new(0, 0, 0, 0);
            }
            let [r, g, b] = srgb8([r, g, b].map(|v| (v / a) * (v / a)));
            rgb::RGBA8::new(r, g, b, (a * 255. + 0.5) as u8)
        })
        .collect();
    let res = encoder().encode_rgba(imgref::Img::new(px.as_slice(), w, h)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::fs::write(path, &res.avif_file).expect("write");
}
