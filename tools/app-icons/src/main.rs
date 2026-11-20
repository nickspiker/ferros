//! Kist icons in the one icon format, and the display's bundle made from them.
//!
//! **The format** (owner, 2026-11-20: "I'd just use the exact same format and process as the Photon avatars so we have a consistent 256x256 g2t ... icon everywhere. friends, kists, except for pucks of course"): a Photon avatar's picture, tagged `g2t`.
//! - A complete VSF document, one section `image`: `pixels` (`v'a'`, an AV1 key frame, 256 x 256, 8 bits, VSF YCbCr 4:2:0 as Photon's avatars and vsf::image have it) and `encoding` (`a`, `g2t`): vsf's flat image section (`builders::compressed_image_encoded`), the form Photon keeps an avatar's picture in.
//! - Made by Photon's own avatar process, called as a library (photon_messenger::ui::avatar), never a second encoder here: the source decoded with its ICC profile (sRGB without one), the square taken, Lanczos-3 resized in linear VSF RGB to 256, kept out to the circle grown by 3 pixels and black past it (the circle is cut where it is drawn), gamma 2, then rav1e at Photon's settings (quantizer 32, speed 6).
//! - The square is the source's art ([`AvatarFrame::Art`]): the box of every pixel at least half opaque, made square about its centre, colour onto black by its opacity; for a picture with no alpha (a photo, Kūaha's lintel) that is the centred square, as a picked avatar.
//! - **Diffuse white** (owner, 2026-11-20: "maybe we just convert from sRGB to VSF RGB g2, then assign it g2t and see what it looks like"), two ways, chosen per icon:
//!   - `place` (the default): the light converted, `tanh(√L)`, so the source's white lands on diffuse white (code 194) and nothing is above it; on any display it looks as the gamma 2 avatar does;
//!   - `assign`: Photon's gamma 2 codes kept and tagged `g2t`, so a code c means atanh(c)² of white: the source's white becomes 3.28 stops above diffuse white, diffuse white sits where the source had 58 % of its white, and everything brighter reads as highlights.
//!   - `assign-inside`: assigned inside a polygon of the source's pixels and placed outside it, the two mixed across a feathered edge (Kūaha's lintel, owner 2026-11-20: the cosmos in the doorway assigned, the frame, wall and floor placed).
//!
//! **The bundle** (the display's `icons.bin`, ICN3): each app's icon made as above (or taken as it is when its source already is one), decoded by Photon's decoder (the one Photon on ferros draws avatars with), then on the host, once, what the display draws: its codes as `g2t`, the inscribed circle cut with its exact coverage per pixel, and the photopic grey. So the display decodes nothing and draws exactly the pixels the AV1 icon holds.
//! - ICN3: b"ICN3", count (u8), size (u16 LE), then per icon size x size RGBA (straight alpha) followed by size x size grey, one byte each, the colours and the grey VSF RGB gamma2tanh codes (tanh(√L), 8 bits). The order is the map's (payloads/map `BUNDLED`): icon n is the n-th app, 0 none.
//! - The grey (for the map at rest, where colour means only "changed") is photopic luminance: VSF RGB to cone space by vsf's `VSF_RGB2LMS` (Stockman–Sharpe 10°), then CIE 170-2's V*₁₀ weights (`LMS2PHOTOPIC`): R 0.258, G 0.635, B 0.107, E white exactly 1.
//! - Beside it `<name>.vsf` per app (its icon) and `sheet.vsf`, the colours over the greys on black as one VSF visual (8-bit `g2t` plane) Opsin opens.
//!
//! Usage:
//! - `app-icons icon <source> <out.vsf> [--assign | --assign-inside x,y,x,y,...]`: one icon (JPEG, PNG, WebP, TIFF; or a 256 x 256 VSF RGB gamma 2 image such as Opsin's orb, which enters at the stage Photon's resize leaves).
//! - `app-icons compare <out.vsf> <icon.vsf>...`: the icons side by side, each cut to its circle on black, as one VSF visual (8-bit `g2t`) for Opsin.
//! - `app-icons bundle <out dir>`: every app's icon and the bundle.
use photon_messenger::ui::avatar::{decode_avatar, encode_avatar_rgb_f32, image_to_avatar_rgb_f32_framed, AvatarFrame, AVATAR_SIZE};
use std::{env, fs, path::Path, process};
use vsf::colour::convert::{apply_matrix_3x3_f32, delinearize_gamma2tanh_f32, delinearize_gamma2tanh_u8_f32, linearize_gamma2tanh_u8_f32};
use vsf::colour::spectral::constants::{LMS2PHOTOPIC, VSF_RGB2LMS};
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::visual::{Encoding, Plane, Primaries, Visual};
use vsf::{BitPackedTensor, VsfType};

const SIZE: usize = AVATAR_SIZE;
const CODE: &str = "/mnt/Harbor/Code";

/// The map's apps, in its order (payloads/map `BUNDLED`), and where each keeps its icon's source: an image made into an icon here, or an icon already (Kūaha's, final, kept in its own repo).
const APPS: [(&str, &str); 6] = [
    ("Photon", "photon/assets/icon-1024.png"),
    ("Mahere", "mahere/installers/icon-1024.png"),
    ("Lumis", "lumis/app/src/main/res/mipmap-xxxhdpi/ic_launcher_round.webp"),
    ("Opsin", "opsin/assets/opsin_orb.vsf"),
    // The present stage's test screen (payloads/ramps): its bands, as an icon.
    ("Ramps", "ferros/assets/ramps-icon.png"),
    // The device's own circle (SHELL.md), ferros's mark, is not here: the display draws it from logo.vsf's geometry at any size (draw::logo), as the number one past the bundle, 7.
    ("Kuaha", "kuaha/assets/icon.vsf"),
];

fn fail(msg: String) -> ! {
    eprintln!("app-icons: {msg}");
    process::exit(1);
}

/// Where the source's white lands in the `g2t` icon (module docs).
#[derive(Clone, PartialEq)]
enum White {
    Place,
    Assign,
    /// Assigned inside a polygon of the source's pixels, placed outside it, with a feathered edge between ([`assign_mask`]).
    AssignInside(Vec<(f32, f32)>),
}

/// The feather's width, source pixels: the Gaussian's σ the polygon's inside is blurred by, so its edge is erf(d / (σ√2)) of the distance d across it, about 2.6 σ from 10 to 90 % (5 pixels at σ = 2).
const FEATHER: f32 = (1 << 1) as f32;

/// Whether (x, y) is inside the polygon (even-odd crossings of a ray toward +x).
fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut odd = false;
    for k in 0..poly.len() {
        let ((x0, y0), (x1, y1)) = (poly[k], poly[(k + 1) % poly.len()]);
        if (y0 > y) != (y1 > y) && x < x0 + (y - y0) * (x1 - x0) / (y1 - y0) {
            odd = !odd;
        }
    }
    odd
}

/// How much of the assigned mapping each of the icon's pixels takes (0 placed, 1 assigned), from a polygon in the source's pixels (owner, 2026-11-20, Kūaha's lintel: the cosmos in the doorway assigned, its highlights unrolled; the frame, wall and floor placed; "a mask over a brightness blend").
/// The polygon's inside, sampled at every source pixel of Photon's square, blurred by a Gaussian of σ [`FEATHER`], then averaged over each icon pixel's block. The edge is a C∞ sigmoid of the distance across it (erf, the Gaussian's integral), with no clamp and no step; unlike a sigmoid of the distance to the nearest edge, it has no kink where two edges' nearest regions meet, at the corners.
/// The square is Photon's for a source without alpha, the centred one; a source with alpha (whose square is its art's) is refused, as its square is not known here.
fn assign_mask(path: &Path, poly: &[(f32, f32)]) -> Vec<f32> {
    use image::ImageDecoder;
    let decoder = image::ImageReader::open(path).and_then(|r| r.with_guessed_format()).map_err(|e| e.to_string()).and_then(|r| r.into_decoder().map_err(|e| e.to_string())).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    if decoder.color_type().has_alpha() {
        fail(format!("{}: a source with alpha frames by its art, so a polygon in its pixels has no known place in the icon", path.display()));
    }
    let (w, h) = decoder.dimensions();
    let side = w.min(h) as usize;
    let (ox, oy) = (((w as usize - side) / 2) as f32, ((h as usize - side) / 2) as f32);
    // The kernel, out to 4 σ each side, and the inside sampled that far past the square, so the blur needs no edge rule.
    let r = (4. * FEATHER).ceil() as usize;
    let kernel: Vec<f32> = (0..2 * r + 1).map(|i| (-((i as f32 - r as f32).powi(2)) / (2. * FEATHER * FEATHER)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|k| k / sum).collect();
    let n = side + 2 * r;
    let mut ind = vec![0f32; n * n];
    for j in 0..n {
        for i in 0..n {
            let (x, y) = (ox + i as f32 - r as f32 + 0.5, oy + j as f32 - r as f32 + 0.5);
            ind[j * n + i] = inside(poly, x, y) as u32 as f32;
        }
    }
    // Separable: rows over the padded grid, then columns into the square.
    let mut rows = vec![0f32; n * side];
    for j in 0..n {
        for i in 0..side {
            rows[j * side + i] = (0..2 * r + 1).map(|k| kernel[k] * ind[j * n + i + k]).sum();
        }
    }
    let mut blurred = vec![0f32; side * side];
    for j in 0..side {
        for i in 0..side {
            blurred[j * side + i] = (0..2 * r + 1).map(|k| kernel[k] * rows[(j + k) * side + i]).sum();
        }
    }
    // Each icon pixel: the mean over the source pixels it covers.
    let mut m = vec![0f32; SIZE * SIZE];
    for y in 0..SIZE {
        let (y0, y1) = (y * side / SIZE, (y + 1) * side / SIZE);
        for x in 0..SIZE {
            let (x0, x1) = (x * side / SIZE, (x + 1) * side / SIZE);
            let mut acc = 0.;
            for j in y0..y1 {
                for i in x0..x1 {
                    acc += blurred[j * side + i];
                }
            }
            m[y * SIZE + x] = acc / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    m
}

/// A polygon from the command line: `x,y,x,y,...`, at least three points, in the source's pixels.
fn parse_polygon(s: &str) -> Vec<(f32, f32)> {
    let v: Vec<f32> = s.split(',').map(|t| t.trim().parse().unwrap_or_else(|_| fail(format!("--assign-inside: {t:?} is not a number")))).collect();
    if v.len() < 6 || v.len() % 2 != 0 {
        fail("--assign-inside takes x,y pairs, at least three".to_string());
    }
    v.chunks(2).map(|p| (p[0], p[1])).collect()
}

/// The `image` section of a VSF image document, by its table of contents.
fn image_section(doc: &[u8]) -> Result<VsfSection, String> {
    let (header, _) = VsfHeader::decode(doc)?;
    let entry = header.fields.iter().find(|f| f.name == "image").ok_or("no `image` section")?;
    let mut at = entry.offset_bytes;
    VsfSection::parse(doc, &mut at)
}

/// What a VSF image source holds: an icon's AV1 and its encoding, or plain gamma 2 codes (a `t_u3` tensor [h, w, 3], Opsin's orb).
enum VsfSource {
    Icon(Vec<u8>, Encoding),
    Codes(usize, usize, Vec<u8>),
}

fn read_vsf_source(doc: &[u8]) -> Result<VsfSource, String> {
    let s = image_section(doc)?;
    let mut encoding = Encoding::Gamma2;
    let mut found = None;
    for f in &s.fields {
        for v in &f.values {
            match v {
                VsfType::a(t) if f.name == "encoding" => encoding = Encoding::parse(t).map_err(|e| format!("encoding: {e}"))?,
                VsfType::v(b'a', av1) if found.is_none() => found = Some(VsfSource::Icon(av1.clone(), Encoding::Gamma2)),
                VsfType::t_u3(t) if found.is_none() && t.shape.len() == 3 && t.shape[2] == 3 => found = Some(VsfSource::Codes(t.shape[1], t.shape[0], t.data.clone())),
                _ => {}
            }
        }
    }
    match found.ok_or("no AV1 or [h, w, 3] tensor in the `image` section")? {
        VsfSource::Icon(av1, _) => {
            if !encoding.is_vsf() {
                return Err(format!("encoding `{}`: an icon is VSF RGB", encoding.as_str()));
            }
            Ok(VsfSource::Icon(av1, encoding))
        }
        VsfSource::Codes(w, h, px) if encoding == Encoding::Gamma2 => Ok(VsfSource::Codes(w, h, px)),
        VsfSource::Codes(..) => Err(format!("a `{}` tensor: only gamma 2 codes enter as a prepared picture", encoding.as_str())),
    }
}

/// Photon's prepared picture (256 x 256 gamma 2 VSF RGB, f32, black past the circle grown by 3 pixels) from a source.
fn prepare(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    if !bytes.starts_with("RÅ<".as_bytes()) {
        return image_to_avatar_rgb_f32_framed(&bytes, AvatarFrame::Art).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    }
    // A VSF RGB gamma 2 picture already 256 square is what Photon's resize leaves; it enters there, with the same black past the grown circle.
    let (w, h, codes) = match read_vsf_source(&bytes).unwrap_or_else(|e| fail(format!("{}: {e}", path.display()))) {
        VsfSource::Codes(w, h, c) => (w, h, c),
        VsfSource::Icon(..) => fail(format!("{}: already an icon", path.display())),
    };
    if (w, h) != (SIZE, SIZE) {
        fail(format!("{}: a VSF source is {w} x {h}; only {SIZE} square enters without Photon's resize", path.display()));
    }
    let half = (SIZE / 2) as f32;
    let keep2 = (half + 3.) * (half + 3.);
    let mut out = vec![0f32; SIZE * SIZE * 3];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (x as f32 + 0.5 - half, y as f32 + 0.5 - half);
            if dx * dx + dy * dy <= keep2 {
                let at = (y * SIZE + x) * 3;
                for c in 0..3 {
                    // Photon's prepared values are codes over 255 (its encoder writes v × 255), so a code comes back as itself.
                    out[at + c] = codes[at + c] as f32 / 255.;
                }
            }
        }
    }
    out
}

/// One icon from its source: Photon's prepared picture, its white placed or assigned (module docs), Photon's AV1 encoder, vsf's flat image section tagged `g2t`.
fn make_icon(path: &Path, white: &White) -> Vec<u8> {
    let mut px = prepare(path);
    // How much of the assigned mapping each pixel takes: none placed, all assigned, the mask's share inside a polygon.
    let mask = match white {
        White::Place => vec![0f32; SIZE * SIZE],
        White::Assign => vec![1f32; SIZE * SIZE],
        White::AssignInside(poly) => assign_mask(path, poly),
    };
    for (i, v) in px.iter_mut().enumerate() {
        // Photon's values are √L exactly (gamma 2 of linear light), so L = v²; tanh(√L) puts the source's white (L = 1) on diffuse white. Assigned, the value stays v, read as g2t.
        let k = mask[i / 3];
        *v = k * *v + (1. - k) * delinearize_gamma2tanh_f32(*v * *v);
    }
    let av1 = encode_avatar_rgb_f32(&px).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    vsf::builders::compressed_image_encoded(av1, Encoding::Gamma2Tanh).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())))
}

/// An icon document decoded by Photon's decoder to `g2t` codes, RGB, 256 square: an untagged (gamma 2) icon's codes are taken to `g2t` (bucket bottom to light, then the 8-bit `g2t` code).
fn decode_icon(doc: &[u8], name: &str) -> Vec<u8> {
    let (av1, encoding) = match read_vsf_source(doc).unwrap_or_else(|e| fail(format!("{name}: {e}"))) {
        VsfSource::Icon(a, e) => (a, e),
        VsfSource::Codes(..) => fail(format!("{name}: not an icon (no AV1)")),
    };
    let (w, h, rgb) = decode_avatar(&av1).unwrap_or_else(|e| fail(format!("{name}: {e}")));
    if (w, h) != (SIZE, SIZE) {
        fail(format!("{name}: an icon is {SIZE} square, this one {w} x {h}"));
    }
    match encoding {
        Encoding::Gamma2Tanh => rgb,
        Encoding::Gamma2 => rgb.iter().map(|&c| delinearize_gamma2tanh_u8_f32((c as f32 / 256.) * (c as f32 / 256.))).collect(),
        e => fail(format!("{name}: a `{}` icon", e.as_str())),
    }
}

/// The share of each pixel inside the inscribed circle, measured on a 16 x 16 grid.
fn coverage() -> Vec<f32> {
    let r = SIZE as f32 / 2.;
    let mut k = vec![0f32; SIZE * SIZE];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut inside = 0;
            for j in 0..16 {
                for i in 0..16 {
                    let (dx, dy) = (x as f32 + (i as f32 + 0.5) / 16. - r, y as f32 + (j as f32 + 0.5) / 16. - r);
                    inside += (dx * dx + dy * dy <= r * r) as u32;
                }
            }
            k[y * SIZE + x] = inside as f32 / 256.;
        }
    }
    k
}

/// Photopic luminance of linear VSF RGB, E white = 1: cone space by vsf's matrix, applied as vsf lays it out (column-major, a column per primary), then the V*₁₀ weights. vsf's own `vsf_rgb_to_photopic_f32` reads the same matrix by rows and weights red 0.61, blue 0.008 (2026-10-21, reported, not yet fixed there).
fn photopic(rgb: [f32; 3]) -> f32 {
    let lms = apply_matrix_3x3_f32(&VSF_RGB2LMS, &rgb);
    let white = apply_matrix_3x3_f32(&VSF_RGB2LMS, &[1.; 3]);
    let y = |v: [f32; 3]| LMS2PHOTOPIC[0] * v[0] + LMS2PHOTOPIC[1] * v[1] + LMS2PHOTOPIC[2] * v[2];
    y(lms) / y(white)
}

/// `g2t` codes over black at coverage `k`: the light times k, encoded again.
fn over_black(code: u8, k: f32) -> u8 {
    delinearize_gamma2tanh_u8_f32(linearize_gamma2tanh_u8_f32(code) * k)
}

/// A picture of `w` x `h` `g2t` codes (RGB interleaved) as a VSF visual: one still, an 8-bit planar `g2t` plane in VSF RGB, which Opsin opens.
fn write_visual(out: &Path, w: usize, h: usize, rgb: &[u8]) {
    let mut planar = vec![0u8; w * h * 3];
    for i in 0..w * h {
        for c in 0..3 {
            planar[c * w * h + i] = rgb[i * 3 + c];
        }
    }
    let plane = Plane::encoded("b0", "s0", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack_u8(8, vec![3, h, w], &planar));
    let doc = Visual::still(plane, None, None, &[]).write().unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    fs::write(out, doc).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
}

fn icon_cmd(args: &[String]) {
    let mut white = White::Place;
    let mut rest: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--assign" => white = White::Assign,
            "--assign-inside" => white = White::AssignInside(parse_polygon(it.next().unwrap_or_else(|| usage()))),
            _ => rest.push(a),
        }
    }
    let [src, out] = rest[..] else { usage() };
    let doc = make_icon(Path::new(src), &white);
    fs::write(out, &doc).unwrap_or_else(|e| fail(format!("{out}: {e}")));
    let how = match white {
        White::Place => "placed",
        White::Assign => "assigned",
        White::AssignInside(_) => "assigned inside the polygon, placed outside",
    };
    println!("app-icons: {src} -> {out} ({} bytes, {SIZE} x {SIZE} AV1, g2t, white {how})", doc.len());
}

fn compare_cmd(args: &[String]) {
    let [out, icons @ ..] = args else { usage() };
    if icons.is_empty() {
        usage();
    }
    let gap = SIZE >> 5;
    let (w, h) = (icons.len() * (SIZE + gap) + gap, SIZE + 2 * gap);
    let mut rgb = vec![0u8; w * h * 3];
    let k = coverage();
    for (n, path) in icons.iter().enumerate() {
        let doc = fs::read(path).unwrap_or_else(|e| fail(format!("{path}: {e}")));
        let codes = decode_icon(&doc, path);
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (i, at) = (y * SIZE + x, ((gap + y) * w + gap + n * (SIZE + gap) + x) * 3);
                for c in 0..3 {
                    rgb[at + c] = over_black(codes[i * 3 + c], k[i]);
                }
            }
        }
    }
    write_visual(Path::new(out), w, h, &rgb);
    println!("app-icons: {} icons side by side -> {out}", icons.len());
}

fn bundle_cmd(args: &[String]) {
    let [out] = args else { usage() };
    let out = Path::new(out);
    fs::create_dir_all(out).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    let mut blob = b"ICN3".to_vec();
    blob.push(APPS.len() as u8);
    blob.extend_from_slice(&(SIZE as u16).to_le_bytes());
    let k = coverage();
    let gap = SIZE >> 5;
    let (sw, sh) = (APPS.len() * (SIZE + gap) + gap, 2 * SIZE + 3 * gap);
    let mut sheet = vec![0u8; sw * sh * 3];
    for (n, (name, rel)) in APPS.iter().enumerate() {
        let path = Path::new(CODE).join(rel);
        let bytes = fs::read(&path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
        // A source that is already an icon is taken as it is; anything else is made into one by Photon's process, its white placed.
        let doc = match bytes.starts_with("RÅ<".as_bytes()).then(|| read_vsf_source(&bytes)) {
            Some(Ok(VsfSource::Icon(..))) => bytes,
            _ => make_icon(&path, &White::Place),
        };
        fs::write(out.join(format!("{}.vsf", name.to_ascii_lowercase())), &doc).unwrap_or_else(|e| fail(format!("{name}: {e}")));
        let codes = decode_icon(&doc, name);
        let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
        let mut grey = Vec::with_capacity(SIZE * SIZE);
        for i in 0..SIZE * SIZE {
            let c = [codes[i * 3], codes[i * 3 + 1], codes[i * 3 + 2]];
            let g = delinearize_gamma2tanh_u8_f32(photopic(c.map(linearize_gamma2tanh_u8_f32)));
            rgba.extend_from_slice(&[c[0], c[1], c[2], (k[i] * 255.).round() as u8]);
            grey.push(g);
            let (x, y) = (gap + n * (SIZE + gap) + i % SIZE, gap + i / SIZE);
            for (ch, v) in c.iter().enumerate() {
                sheet[(y * sw + x) * 3 + ch] = over_black(*v, k[i]);
                sheet[((y + SIZE + gap) * sw + x) * 3 + ch] = over_black(g, k[i]);
            }
        }
        blob.extend_from_slice(&rgba);
        blob.extend_from_slice(&grey);
        println!("  {name}: {} -> {SIZE} x {SIZE}, {} bytes as an icon", path.display(), doc.len());
    }
    fs::write(out.join("icons.bin"), &blob).unwrap_or_else(|e| fail(format!("icons.bin: {e}")));
    write_visual(&out.join("sheet.vsf"), sw, sh, &sheet);
    println!("app-icons: {} icons, {} bytes -> {}", APPS.len(), blob.len(), out.join("icons.bin").display());
}

fn usage() -> ! {
    eprintln!("usage:\n  app-icons icon <source> <out.vsf> [--assign | --assign-inside x,y,x,y,...]\n  app-icons compare <out.vsf> <icon.vsf>...\n  app-icons bundle <out dir>");
    process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("icon") => icon_cmd(&args[1..]),
        Some("compare") => compare_cmd(&args[1..]),
        Some("bundle") => bundle_cmd(&args[1..]),
        _ => usage(),
    }
}
