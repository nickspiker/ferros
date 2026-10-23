//! Bundle the map's app icons for the display server (SHELL.md: an app is its icon), in VSF RGB.
//!
//! Each app's icon comes from its own codebase in whatever format that codebase keeps it: PNG, Android WebP, or VSF. Everything is brought into linear VSF RGB first and every step after that happens there, once, on the host, so the display server only ever blits one raw format and decodes nothing.
//!
//! - PNG and WebP are taken as sRGB, which is what an app asset without a profile means, and enter VSF through vsf's `SRGB2VSF_RGB`: colorimetric, with no adaptation, so sRGB's white arrives as about VSF 0.91 / 1.03 / 1.09, not on the R = G = B axis (vsf's doctrine: neutralising a white is white balance, a Creative op).
//! - VSF icons (Opsin's orb) are VSF RGB gamma 2 already; fluor decodes them and the codes are squared back to light.
//! - Scaling to the bundle's size averages areas in linear light with premultiplied alpha, and the circle is cut with its exact coverage per pixel.
//! - The grey, for the map at rest (colour there means only "changed"), is photopic luminance: VSF RGB to cone space by vsf's `VSF_RGB2LMS` (Stockman–Sharpe 10°), then CIE 170-2's V*₁₀ weights, vsf's `LMS2PHOTOPIC`. That makes VSF RGB's weights R 0.258, G 0.635, B 0.107, and E white exactly 1.
//! - Stored as VSF RGB gamma 2 codes, white at 255 — untagged VSF RGB, the panel pipeline's plain path (DISPLAY-DESIGN.md §6). Light above white clips per channel; it is sRGB's white in blue, a few percent.
//!
//! Output: target/icons/icons.bin = b"ICN2", count (u8), size (u16 LE), then per icon size x size RGBA (straight alpha) followed by size x size grey, one byte each. The order is the map's (payloads/map): icon n is the n-th app, and 0 means no icon. Also target/icons/sheet.png, the colours over the greys, as the codes the panel is sent.
//!
//! Usage: app-icons <out dir>
use std::{env, fs, path::Path, process};
use vsf::colour::convert::apply_matrix_3x3_f32;
use vsf::colour::legacy::constants::SRGB2VSF_RGB;
// sRGB is deprecated in vsf as a space to work in; here it is only the space app assets arrive in, decoded once at the door.
#[allow(deprecated)]
use vsf::colour::legacy::transfer::srgb_eotf;
use vsf::colour::spectral::constants::{LMS2PHOTOPIC, VSF_RGB2LMS};

const SIZE: usize = 128;
const CODE: &str = "/mnt/Harbor/Code";

/// The map's apps, in its order, and where each keeps its icon.
const APPS: [(&str, &str); 6] = [
    ("Photon", "photon/assets/icon-128.png"),
    ("Mahere", "mahere/installers/icon-256.png"),
    ("Lumis", "lumis/app/src/main/res/mipmap-xxxhdpi/ic_launcher_round.webp"),
    ("Opsin", "opsin/assets/opsin_orb.vsf"),
    // The device's own circle (SHELL.md): ferros's logo, the phase-diagram slices of iron.
    ("ferros", "ferros/logo.svg"),
    // The present stage's test screen (payloads/ramps): its bands, as an icon.
    ("Ramps", "ferros/assets/ramps-icon.png"),
];

/// An image in linear VSF RGB with premultiplied alpha: [r, g, b, a] per pixel.
struct Linear {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

fn fail(msg: String) -> ! {
    eprintln!("app-icons: {msg}");
    process::exit(1);
}

fn load(path: &Path) -> Linear {
    // SVG is drawn to a PNG by rsvg-convert first (it carries no profile, so it is sRGB like any other app asset), large enough that the area average does the scaling.
    let bytes = if path.extension().is_some_and(|e| e == "svg") {
        let out = process::Command::new("rsvg-convert").args(["-w", "1024", "-h", "1024"]).arg(path).output().unwrap_or_else(|e| fail(format!("rsvg-convert: {e}")));
        if !out.status.success() {
            fail(format!("{}: rsvg-convert failed", path.display()));
        }
        out.stdout
    } else {
        fs::read(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())))
    };
    if path.extension().is_some_and(|e| e == "vsf") {
        let icon = fluor::host::icon::Icon::from_vsf_bytes(&bytes).unwrap_or_else(|e| fail(format!("{}: not a VSF image ({e:?})", path.display())));
        // fluor packs a pixel as alpha over darkness, the RGB bytes being 255 minus the code, so one XOR gives back the VSF RGB gamma 2 codes; squaring a code gives its light.
        let px = icon
            .pixels
            .iter()
            .map(|&p| {
                let v = p ^ 0x00FF_FFFF;
                let l = |s: u32| ((v >> s) & 255) as f32 / 255.0;
                [l(16) * l(16), l(8) * l(8), l(0) * l(0), 1.0]
            })
            .collect();
        return Linear { w: icon.width as usize, h: icon.height as usize, px };
    }
    let img = image::load_from_memory(&bytes).unwrap_or_else(|e| fail(format!("{}: {e}", path.display()))).to_rgba8();
    let px = img
        .pixels()
        .map(|p| {
            #[allow(deprecated)]
            let s = [srgb_eotf(p[0] as f32 / 255.0), srgb_eotf(p[1] as f32 / 255.0), srgb_eotf(p[2] as f32 / 255.0)];
            let v = apply_matrix_3x3_f32(&SRGB2VSF_RGB, &s);
            let a = p[3] as f32 / 255.0;
            [v[0] * a, v[1] * a, v[2] * a, a]
        })
        .collect();
    Linear { w: img.width() as usize, h: img.height() as usize, px }
}

/// The centred square, averaged down (or up) to SIZE: each output pixel is the exact area-weighted mean of the source it covers.
/// The icon's own art, cropped square: the box of every pixel at least half opaque, made square about its centre. App icons often sit inset on a transparent canvas (Mahere's round map fills the middle 80 %, Lumis's has a margin and a soft shadow); scaled as they come, that margin showed as a ring of the circle's fill around the art (owner, 2026-10-23: "a baked circle in the image data"). Cropped, every icon reaches the circle's edge.
fn content(src: &Linear) -> Linear {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..src.h {
        for x in 0..src.w {
            if src.px[y * src.w + x][3] >= 0.5 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return Linear { w: src.w, h: src.h, px: src.px.clone() };
    }
    let side = (x1 - x0).max(y1 - y0);
    let (cx2, cy2) = (x0 + x1, y0 + y1);
    let mut px = vec![[0f32; 4]; side * side];
    for y in 0..side {
        for x in 0..side {
            let (sx, sy) = ((cx2 as isize - side as isize) / 2 + x as isize, (cy2 as isize - side as isize) / 2 + y as isize);
            if sx >= 0 && sy >= 0 && (sx as usize) < src.w && (sy as usize) < src.h {
                px[y * side + x] = src.px[sy as usize * src.w + sx as usize];
            }
        }
    }
    Linear { w: side, h: side, px }
}

fn square(src: &Linear) -> Linear {
    let s = src.w.min(src.h);
    let (ox, oy) = ((src.w - s) / 2, (src.h - s) / 2);
    let scale = s as f32 / SIZE as f32;
    // How much of source cell `i` lies in [a, b).
    let overlap = |i: usize, a: f32, b: f32| ((i + 1) as f32).min(b) - (i as f32).max(a);
    let mut px = vec![[0f32; 4]; SIZE * SIZE];
    for y in 0..SIZE {
        let (y0, y1) = (y as f32 * scale, (y + 1) as f32 * scale);
        for x in 0..SIZE {
            let (x0, x1) = (x as f32 * scale, (x + 1) as f32 * scale);
            let mut acc = [0f32; 4];
            for sy in y0 as usize..(y1.ceil() as usize).min(s) {
                let wy = overlap(sy, y0, y1);
                for sx in x0 as usize..(x1.ceil() as usize).min(s) {
                    let wt = wy * overlap(sx, x0, x1);
                    let p = src.px[(oy + sy) * src.w + ox + sx];
                    for c in 0..4 {
                        acc[c] += p[c] * wt;
                    }
                }
            }
            let area = scale * scale;
            px[y * SIZE + x] = acc.map(|v| v / area);
        }
    }
    Linear { w: SIZE, h: SIZE, px }
}

/// Cut the inscribed circle: every pixel's alpha times the share of it inside the circle, measured on a 16 x 16 grid. An icon that is already round keeps its own edge.
fn circle(img: &mut Linear) {
    let r = SIZE as f32 / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut inside = 0;
            for j in 0..16 {
                for i in 0..16 {
                    let (dx, dy) = (x as f32 + (i as f32 + 0.5) / 16.0 - r, y as f32 + (j as f32 + 0.5) / 16.0 - r);
                    inside += (dx * dx + dy * dy <= r * r) as u32;
                }
            }
            let k = inside as f32 / 256.0;
            img.px[y * SIZE + x] = img.px[y * SIZE + x].map(|v| v * k);
        }
    }
}

/// Photopic luminance of linear VSF RGB, E white = 1: cone space by vsf's matrix, applied as vsf lays it out (column-major, a column per primary), then the V*₁₀ weights. vsf's own `vsf_rgb_to_photopic_f32` reads the same matrix by rows and weights red 0.61, blue 0.008 (2026-10-21, reported, not yet fixed there).
fn photopic(rgb: [f32; 3]) -> f32 {
    let lms = apply_matrix_3x3_f32(&VSF_RGB2LMS, &rgb);
    let white = apply_matrix_3x3_f32(&VSF_RGB2LMS, &[1.0; 3]);
    let y = |v: [f32; 3]| LMS2PHOTOPIC[0] * v[0] + LMS2PHOTOPIC[1] * v[1] + LMS2PHOTOPIC[2] * v[2];
    y(lms) / y(white)
}

/// Linear light to a VSF RGB gamma 2 code, clipped at white.
fn code(l: f32) -> u8 {
    (l.clamp(0.0, 1.0).sqrt() * 255.0).round() as u8
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: app-icons <out dir>");
        process::exit(2);
    }
    let out = Path::new(&args[1]);
    fs::create_dir_all(out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let mut blob = b"ICN2".to_vec();
    blob.push(APPS.len() as u8);
    blob.extend_from_slice(&(SIZE as u16).to_le_bytes());
    let mut sheet = image::RgbaImage::from_pixel((SIZE * APPS.len() + 8 * (APPS.len() + 1)) as u32, (2 * SIZE + 24) as u32, image::Rgba([0, 0, 0, 255]));
    for (k, (name, rel)) in APPS.iter().enumerate() {
        let path = Path::new(CODE).join(rel);
        let mut img = square(&content(&load(&path)));
        circle(&mut img);
        let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
        let mut grey = Vec::with_capacity(SIZE * SIZE);
        for (i, p) in img.px.iter().enumerate() {
            let a = p[3];
            let lin = if a > 0.0 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
            let (c, g) = ([code(lin[0]), code(lin[1]), code(lin[2])], code(photopic(lin)));
            let a8 = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            rgba.extend_from_slice(&[c[0], c[1], c[2], a8]);
            grey.push(g);
            // The sheet: each icon over black, as the codes the panel receives.
            let (x, y) = ((8 + k * (SIZE + 8) + i % SIZE) as u32, (8 + i / SIZE) as u32);
            let over = |v: u8| ((v as f32) * a).round() as u8;
            sheet.put_pixel(x, y, image::Rgba([over(c[0]), over(c[1]), over(c[2]), 255]));
            sheet.put_pixel(x, y + (SIZE + 8) as u32, image::Rgba([over(g), over(g), over(g), 255]));
        }
        blob.extend_from_slice(&rgba);
        blob.extend_from_slice(&grey);
        println!("  {name}: {} -> {SIZE} x {SIZE}", path.display());
    }
    fs::write(out.join("icons.bin"), &blob).unwrap_or_else(|e| fail(format!("icons.bin: {e}")));
    sheet.save(out.join("sheet.png")).unwrap_or_else(|e| fail(format!("sheet.png: {e}")));
    println!("app-icons: {} icons, {} bytes -> {}", APPS.len(), blob.len(), out.join("icons.bin").display());
}
