//! Kist icons in the one icon format, and the display's bundle made from them.
//!
//! **The format** (owner, 2026-11-20: "I'd just use the exact same format and process as the Photon avatars so we have a consistent 256x256 g2t ... icon everywhere. friends, kists, except for pucks of course"): a Photon avatar's picture, tagged `g2t`.
//! - A complete VSF document, one section `image`: `pixels` (`v'a'`, an AV1 key frame, 256 x 256, 8 bits, VSF YCbCr 4:2:0 as Photon's avatars and vsf::image have it) and `encoding` (`a`, `g2t`): vsf's flat image section (`builders::compressed_image_encoded`), the form Photon keeps an avatar's picture in.
//! - Made by Photon's own avatar process, called as a library (photon_messenger::ui::avatar), never a second encoder here: the source's codes taken as VSF RGB as they stand (no ICC profile, no sRGB, nothing converted; owner, 2026-11-20: "Assign it VSF RGB g2t instead of sRGB"), the square taken, Lanczos-3 resized in linear VSF RGB to 256, kept out to the circle grown by 3 pixels and black past it (the circle is cut where it is drawn), gamma 2, then rav1e at Photon's settings (quantizer 32, speed 6).
//! - The square is the source's art ([`AvatarFrame::Art`]): the box of every pixel at least half opaque, made square about its centre, colour onto black by its opacity; for a picture with no alpha (a photo, Kūaha's lintel) that is the centred square, as a picked avatar.
//! - **What the codes mean**, chosen per icon (owner, 2026-11-20: "Assign it VSF RGB g2t instead of sRGB when you do the icon VSF generation"):
//!   - assigned (the default): the codes are VSF RGB `g2t`, so a code c holds atanh(c/256)² of diffuse white: codes above 194 are light above white, up to 3.28 stops at 255. Photon's resize runs in that light, and the icon's codes are the g2t of the resized light, so a picture its own size keeps its codes;
//!   - `--place`: the codes are VSF RGB gamma 2, (c/256)² of white, so the source's white is diffuse white and nothing is above it; the icon holds the g2t of that light;
//!   - `--assign-inside x,y,...`: assigned inside a polygon of the source's pixels and placed outside it, the two mixed across a feathered edge (Kūaha's lintel, owner 2026-11-20: the cosmos in the doorway assigned, the frame, wall and floor placed, by a mask).
//! - Photon's encoder writes a value v as the truncation of v x 255; VSF's codes are the value x 256, bucket bottom. So every value goes in as (v + 1/512) x 256/255, and the encoder's codes are VSF's, rounded.
//!
//! **The bundle** (the display's `icons.bin`, ICN3): each app's icon made as above (or taken as it is when its source already is one), decoded by Photon's decoder (the one Photon on ferros draws avatars with), then on the host, once, what the display draws: its codes as `g2t`, the inscribed circle cut with its exact coverage per pixel, and the photopic grey. So the display decodes nothing and draws exactly the pixels the AV1 icon holds.
//! - ICN3: b"ICN3", count (u8), size (u16 LE), then per icon size x size RGBA (straight alpha) followed by size x size grey, one byte each, the colours and the grey VSF RGB gamma2tanh codes (tanh(√L), 8 bits). The order is the map's (payloads/map `BUNDLED`): icon n is the n-th app, 0 none.
//! - The grey (for the map at rest, where colour means only "changed") is photopic luminance: VSF RGB to cone space by vsf's `VSF_RGB2LMS` (Stockman–Sharpe 10°), then CIE 170-2's V*₁₀ weights (`LMS2PHOTOPIC`): R 0.258, G 0.635, B 0.107, E white exactly 1.
//! - Beside it `<name>.vsf` per app (its icon) and `sheet.vsf`, the colours over the greys on black as one VSF visual (8-bit `g2t` plane) Opsin opens.
//!
//! Usage:
//! - `app-icons icon <source> <out.vsf> [--place | --assign-inside x,y,x,y,...] [--inset N]`: one icon (JPEG, PNG, WebP, TIFF; or a 256 x 256 VSF picture of codes such as Opsin's orb, which enters at the stage Photon's resize leaves, its codes read as the icon's are).
//! - `app-icons compare <out.vsf> <icon.vsf>...`: the icons side by side, each cut to its circle on black, as one VSF visual (8-bit `g2t`) for Opsin.
//! - `app-icons bundle <out dir>`: every app's icon and the bundle.
use photon_messenger::ui::avatar::{decode_avatar, encode_avatar_rgb_f32, image_to_avatar_rgb_f32_framed, AvatarCodes, AvatarFrame, AVATAR_SIZE};
use std::{env, fs, path::Path, process};
use vsf::colour::convert::{apply_matrix_3x3_f32, delinearize_gamma2tanh_u8_f32, linearize_gamma2tanh_u8_f32};
use vsf::colour::spectral::constants::{LMS2PHOTOPIC, VSF_RGB2LMS};
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::visual::{Encoding, Plane, Primaries, Visual};
use vsf::{BitPackedTensor, VsfType};

const SIZE: usize = AVATAR_SIZE;
const CODE: &str = "/mnt/Harbor/Code";

/// The map's apps, in its order (payloads/map `BUNDLED`), and where each keeps its icon's source: an image made into an icon here, or an icon already (Kūaha's, final, kept in its own repo).
const APPS: [(&str, &str); 6] = [
    // Photon's logo (owner, 2026-11-20: "logo original.jpeg" is Photon's icon).
    ("Photon", "photon/assets/logo original.jpeg"),
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

/// What the source's codes mean (module docs).
#[derive(Clone, PartialEq)]
enum Mapping {
    /// VSF RGB `g2t`.
    Assign,
    /// VSF RGB gamma 2.
    Place,
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

/// Photon's prepared picture (256 x 256, the gamma 2 of the light in VSF RGB, f32, black past the circle grown by 3 pixels) from a source whose codes mean `codes` (G2t or Gamma2), its art's square cut by `inset` 1024ths of its side at each edge (0: none).
fn prepare(path: &Path, codes: AvatarCodes, inset: u16) -> Vec<f32> {
    let bytes = fs::read(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    if !bytes.starts_with("RÅ<".as_bytes()) {
        // The art's square, cut by `inset` 1024ths of its side at each edge when asked (`--inset`, Photon's `AvatarFrame::ArtInset`: before its one resize).
        let frame = if inset == 0 { AvatarFrame::Art } else { AvatarFrame::ArtInset(inset) };
        return image_to_avatar_rgb_f32_framed(&bytes, frame, codes).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    }
    // A VSF picture of codes already 256 square is what Photon's resize leaves; it enters there, its codes read as `codes` says, with the same black past the grown circle.
    let (w, h, px) = match read_vsf_source(&bytes).unwrap_or_else(|e| fail(format!("{}: {e}", path.display()))) {
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
                    // The gamma 2 of the code's light: the code over 256 itself for gamma 2, its atanh for g2t.
                    let s = px[at + c] as f32 / 256.;
                    out[at + c] = if codes == AvatarCodes::G2t { s.atanh() } else { s };
                }
            }
        }
    }
    out
}

/// One icon from its source: Photon's prepared picture with the codes read as the mapping says (module docs), the g2t of its light, Photon's AV1 encoder, vsf's flat image section tagged `g2t`.
fn make_icon(path: &Path, mapping: &Mapping, primaries: Option<Primaries>, inset: u16) -> Vec<u8> {
    // Photon's values are √L (the gamma 2 of the light), so the g2t of the light, tanh(√L), is tanh of the value.
    let g2t = |codes: AvatarCodes| -> Vec<f32> { prepare(path, codes, inset).into_iter().map(f32::tanh).collect() };
    let mut px = match mapping {
        Mapping::Assign => g2t(AvatarCodes::G2t),
        Mapping::Place => g2t(AvatarCodes::Gamma2),
        Mapping::AssignInside(poly) => {
            // The mask's share of the assigned picture, the rest of the placed one.
            let (a, p, mask) = (g2t(AvatarCodes::G2t), g2t(AvatarCodes::Gamma2), assign_mask(path, poly));
            (0..a.len()).map(|i| mask[i / 3] * a[i] + (1. - mask[i / 3]) * p[i]).collect()
        }
    };
    // Into Photon's encoder, which truncates v x 255, as VSF's codes (v x 256, rounded): module docs.
    for v in px.iter_mut() {
        *v = (*v + 1. / ((1 << 9) as f32)) * (256. / 255.);
    }
    let av1 = encode_avatar_rgb_f32(&px).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    icon_doc(av1, primaries).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())))
}

/// An icon document: vsf's flat image section (`pixels` the AV1, `encoding` `g2t`) and, when the icon says what its codes are, `primaries` (vsf::visual `Primaries`' word; owner, 2026-11-24: Kūaha's and Aemulus's say `vsf`, kept as they are; an icon naming none is taken as the source's sRGB numbers and converted on the device, ferros_kept::icon `Source`).
fn icon_doc(av1: Vec<u8>, primaries: Option<Primaries>) -> Result<Vec<u8>, String> {
    let mut fields = vec![("pixels".to_string(), VsfType::v(b'a', av1)), ("encoding".to_string(), VsfType::a(vsf::Ascii::new(Encoding::Gamma2Tanh.as_str()).map_err(|e| format!("encoding: {e}"))?))];
    if let Some(p) = primaries {
        fields.push(("primaries".to_string(), VsfType::a(vsf::Ascii::new(p.as_str()).map_err(|e| format!("primaries: {e}"))?)));
    }
    vsf::vsf_builder::VsfBuilder::new().provenance_only().add_section("image", fields).build()
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
    let mut mapping = Mapping::Assign;
    let mut primaries = None;
    let mut inset: u16 = 0;
    let mut rest: Vec<&String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--place" => mapping = Mapping::Place,
            // `--inset N`: the art's square cut by N 1024ths of its side at each edge (Lumis's, 2026-11-24, the owner: "crop 3% off the edges, the border is showing": 31).
            "--inset" => inset = it.next().and_then(|n| n.parse().ok()).unwrap_or_else(|| usage()),
            "--primaries" => primaries = Some(Primaries::parse(it.next().unwrap_or_else(|| usage())).unwrap_or_else(|e| fail(format!("--primaries: {e}")))),
            "--assign-inside" => mapping = Mapping::AssignInside(parse_polygon(it.next().unwrap_or_else(|| usage()))),
            _ => rest.push(a),
        }
    }
    let [src, out] = rest[..] else { usage() };
    let doc = make_icon(Path::new(src), &mapping, primaries, inset);
    fs::write(out, &doc).unwrap_or_else(|e| fail(format!("{out}: {e}")));
    let how = match mapping {
        Mapping::Assign => "codes as g2t",
        Mapping::Place => "codes as gamma 2, white at diffuse white",
        Mapping::AssignInside(_) => "codes as g2t inside the polygon, as gamma 2 outside",
    };
    println!("app-icons: {src} -> {out} ({} bytes, {SIZE} x {SIZE} AV1, g2t; {how})", doc.len());
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

/// `tag <in.vsf> <out.vsf> --primaries <p>`: an icon document again with `primaries` set, its AV1 and encoding as they were (Kūaha's, owner 2026-11-24: "I like that one the way it is").
fn tag_cmd(args: &[String]) {
    let [src, out, flag, p] = args else { usage() };
    if flag != "--primaries" {
        usage();
    }
    let p = Primaries::parse(p).unwrap_or_else(|e| fail(format!("--primaries: {e}")));
    let doc = fs::read(src).unwrap_or_else(|e| fail(format!("{src}: {e}")));
    let av1 = match read_vsf_source(&doc).unwrap_or_else(|e| fail(format!("{src}: {e}"))) {
        VsfSource::Icon(a, Encoding::Gamma2Tanh) => a,
        _ => fail(format!("{src}: not a g2t icon")),
    };
    let tagged = icon_doc(av1, Some(p)).unwrap_or_else(|e| fail(format!("{out}: {e}")));
    fs::write(out, &tagged).unwrap_or_else(|e| fail(format!("{out}: {e}")));
    println!("app-icons: {src} -> {out} (primaries {})", p.as_str());
}

/// `looks <out.vsf> <name>=<path>[:keep|:gamma2]...`: three rows, each icon cut to its circle on black, as one VSF visual (8-bit `g2t`): the gamma 2 build (7c76fd89: vsf's colorimetric `SRGB2VSF_RGB`, then gamma 2, each value cast as it came); the owner's highlight extension (ferros_kept::icon `from_srgb`: L = tan(x · π/2), the matrix, g2t); and the comparison extension (`extend_g2t_inverse`: L = atanh(√x)², a grey keeping its gamma 2 code).
/// `:keep` an icon kept as it is (Kūaha's, `primaries: vsf`); `:gamma2` a VSF RGB gamma 2 source picture (Aemulus's, already through the owner's steps 1 to 3 by its own tool): before, the `--place` icon made of it, after, its codes assigned as they stand (`icon <source> <out> --primaries vsf`), what an sRGB icon becomes on the device.
fn looks_cmd(args: &[String]) {
    let [out, icons @ ..] = args else { usage() };
    if icons.is_empty() {
        usage();
    }
    let gap = SIZE >> 5;
    let (w, h) = (icons.len() * (SIZE + gap) + gap, 3 * SIZE + 4 * gap);
    let mut rgb = vec![0u8; w * h * 3];
    let k = coverage();
    for (n, arg) in icons.iter().enumerate() {
        let (name, rest) = arg.split_once('=').unwrap_or_else(|| usage());
        let (path, how) = rest.split_once(':').unwrap_or((rest, ""));
        let doc = fs::read(path).unwrap_or_else(|e| fail(format!("{path}: {e}")));
        let (before, after, inverse): (Vec<u8>, Vec<u8>, Vec<u8>) = match how {
            "gamma2" => {
                let px = match read_vsf_source(&doc).unwrap_or_else(|e| fail(format!("{path}: {e}"))) {
                    VsfSource::Codes(..) => prepare(Path::new(path), AvatarCodes::Gamma2, 0),
                    VsfSource::Icon(..) => fail(format!("{path}: :gamma2 takes a gamma 2 source picture")),
                };
                let kept: Vec<u8> = px.iter().map(|&v| (v * 256.) as u8).collect();
                (kept.clone(), kept.clone(), kept)
            }
            "keep" => {
                let c = decode_icon(&doc, path);
                (c.clone(), c.clone(), c)
            }
            "" => {
                let c = decode_icon(&doc, path);
                let (mut b, mut a, mut v) = (Vec::with_capacity(c.len()), Vec::with_capacity(c.len()), Vec::with_capacity(c.len()));
                for p in c.chunks_exact(3) {
                    let t = [p[0], p[1], p[2]];
                    // The gamma 2 build on the Pro now, for the comparison only.
                    #[allow(deprecated)]
                    let s = t.map(vsf::colour::legacy::convert::linearize_srgb_u8);
                    let l = apply_matrix_3x3_f32(&vsf::colour::legacy::constants::SRGB2VSF_RGB, &s);
                    b.extend(l.map(|x| (x.sqrt() * 256.) as u8));
                    a.extend_from_slice(&ferros_kept::icon::from_srgb(t));
                    v.extend_from_slice(&ferros_kept::icon::from_srgb_extended(t, ferros_kept::icon::extend_g2t_inverse));
                }
                let white = |x: &[u8]| x.chunks_exact(3).zip(&k).filter(|(p, kk)| **kk >= 1. && p.iter().all(|&q| q == 255)).count();
                println!("  {name}: pixels at code 255 in all three: on the Pro now {}, tan {}, atanh {} (of {} inside)", white(&b), white(&a), white(&v), k.iter().filter(|&&kk| kk >= 1.).count());
                (b, a, v)
            }
            _ => usage(),
        };
        for (row, codes) in [&before, &after, &inverse].into_iter().enumerate() {
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let (i, at) = (y * SIZE + x, ((gap + row * (SIZE + gap) + y) * w + gap + n * (SIZE + gap) + x) * 3);
                    for c in 0..3 {
                        rgb[at + c] = over_black(codes[i * 3 + c], k[i]);
                    }
                }
            }
        }
        // How far the two rows are apart inside the circle, per channel: the mean and the most of |after − before| in codes, and the mean code of each.
        let mut stat = [(0f64, 0u8, 0f64, 0f64); 3];
        let mut n_in = 0f64;
        for i in 0..SIZE * SIZE {
            if k[i] < 1. {
                continue;
            }
            n_in += 1.;
            for c in 0..3 {
                let (b, a) = (before[i * 3 + c], after[i * 3 + c]);
                let d = b.abs_diff(a);
                stat[c].0 += d as f64;
                stat[c].1 = stat[c].1.max(d);
                stat[c].2 += b as f64;
                stat[c].3 += a as f64;
            }
        }
        let line: Vec<String> = ["R", "G", "B"].iter().zip(stat).map(|(ch, (sum, most, b, a))| format!("{ch} mean |diff| {:.1} most {most}, mean code {:.1} -> {:.1}", sum / n_in, b / n_in, a / n_in)).collect();
        println!("  {name}: {path}{}; {}", if how.is_empty() { String::new() } else { format!(" ({how})") }, line.join("; "));
    }
    write_visual(Path::new(out), w, h, &rgb);
    println!("app-icons: {} icons, before over after -> {out}", icons.len());
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
        // A source that is already a g2t icon is taken as it is (Kūaha's); an untagged one is from before 2026-11-20 and is refused, to be made again; anything else is made into one by Photon's process, its codes as g2t.
        let doc = match bytes.starts_with("RÅ<".as_bytes()).then(|| read_vsf_source(&bytes)) {
            Some(Ok(VsfSource::Icon(_, Encoding::Gamma2Tanh))) => bytes,
            Some(Ok(VsfSource::Icon(..))) => fail(format!("{}: an icon not tagged g2t (the form before 2026-11-20): make it again with app-icons icon", path.display())),
            _ => make_icon(&path, &Mapping::Assign, None, 0),
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
    eprintln!("usage:\n  app-icons icon <source> <out.vsf> [--place | --assign-inside x,y,x,y,...] [--primaries vsf] [--inset N]\n  app-icons tag <icon.vsf> <out.vsf> --primaries vsf\n  app-icons compare <out.vsf> <icon.vsf>...\n  app-icons looks <out.vsf> <name>=<path>[:keep|:gamma2]...\n  app-icons bundle <out dir>");
    process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("icon") => icon_cmd(&args[1..]),
        Some("compare") => compare_cmd(&args[1..]),
        Some("bundle") => bundle_cmd(&args[1..]),
        Some("tag") => tag_cmd(&args[1..]),
        Some("looks") => looks_cmd(&args[1..]),
        _ => usage(),
    }
}
