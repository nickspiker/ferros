//! wallpaper — a 16-bit RGB TIFF in Rec.2020 primaries with a pure power-law transfer (Rec.1886's reference curve, linear = code^2.4) as a VSF visual: one Display plane, VSF RGB, gamma2tanh at 16 bits, written the way screencap writes the screen.
//! The TIFF's own ICC profile says what its codes mean, and the tool follows it: the TRC tags give each channel's exponent, and the colorants, taken back from the D50 connection space thru the profile's chad, must sit on Rec.2020's primaries and D65 white, or the tool stops (a different space would need its own matrix, not a guess).
//! A TIFF with no profile is read as Rec.2020 at gamma 2.4, and the tool says so.
//! Rec.2020's primaries are single wavelengths (630, 532 and 467 nm), so the 3 × 3 vsf derives from them under the Stockman & Sharpe 10° cones (`REC2020_2VSF_RGB`) is the spectral conversion itself, exact for every mix of the three lines, not a fit: there is no finer route to take.
//! It is colorimetric, with no chromatic adaptation, so the source's white (D65 at Illuminant E's photopic luminance) lands a little blue of VSF's R = G = B: that is what D65 is in VSF RGB.
//! Light above VSF white is kept: gamma2tanh rolls it off under the tanh instead of clipping (the top 16-bit code is 5.12 stops above diffuse white).
//! Rec.2020's three lines sit just outside VSF RGB's triangle (630 and 532 nm a hair past the chord from 523 to 703 nm, 467 nm past the one from 462 to 523 nm), so a colour saturated enough to approach one of them comes out of the matrix with a small negative channel, which gamma2tanh cannot hold (negative light encodes to code 0).
//! Clipping that channel to 0 would add light of that primary, moving both the hue and the luminance; instead the pixel is mixed toward the neutral of its own photopic luminance just far enough to bring its lowest channel to 0, which keeps its luminance and its hue line exactly and gives up only the saturation the plane cannot carry.
//! The tool reports how many pixels moved and how far.
//! After writing, the file is read back with vsf and every sample is checked to decode to the bucket that holds the light computed for it.
//! Usage: wallpaper <in.tif> [out.vsf]   (default target/wallpaper/<input stem>.vsf in the ferros repo)

use std::path::{Path, PathBuf};
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;
use vsf::colour::convert::{apply_matrix_3x3_f32, delinearize_gamma2tanh_bits_f32, linearize_gamma2tanh_bits_f32};
use vsf::colour::rec2020::REC2020_2VSF_RGB;
use vsf::colour::spectral::{LMS2PHOTOPIC, VSF_RGB2LMS};
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Samples, Visual};

/// Rec.2020's primaries and white (ITU-R BT.2020, table 3) as CIE 1931 xy: red, green, blue, D65.
const REC2020_XY: [[f64; 2]; 4] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046], [0.3127, 0.3290]];

/// How far a profile's xy may sit from Rec.2020's and still be it: the s15.16 colorants and chad round to about 1.5 × 10⁻⁵, so a 512th is loose enough for any honest writer and far tighter than the next space over (P3's green is 0.03 away).
const XY_TOLERANCE: f64 = 1. / (1 << 9) as f64;

/// The gamma2tanh plane's bits per sample.
const BITS: u32 = 16;

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("wallpaper: {msg}");
    std::process::exit(1)
}

fn be16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([b[o], b[o + 1]])
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// An ICC s15Fixed16Number.
fn s15f16(b: &[u8], o: usize) -> f64 {
    be32(b, o) as i32 as f64 / (1 << 16) as f64
}

/// What the TIFF's ICC profile says about its codes.
struct Profile {
    name: String,
    /// Each channel's exponent: linear = code^gamma.
    gamma: [f64; 3],
}

/// The ICC tag table as (signature, offset, size); every offset is checked against the profile's length, since the profile is file data.
/// WHY/PROOF: the count and each offset and size come from the file; unchecked, a short or crafted profile indexes past its bytes and panics in the slicing below, so each is held to the profile's length here and every later tag read stays inside its own slice.
fn tags(icc: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    if icc.len() < 132 {
        fail(format!("ICC profile of {} bytes is shorter than its header", icc.len()));
    }
    let count = be32(icc, 128) as usize;
    if 132 + count * 12 > icc.len() {
        fail(format!("ICC tag table of {count} entries runs past the profile's {} bytes", icc.len()));
    }
    (0..count)
        .map(|k| {
            let e = 132 + k * 12;
            let (off, size) = (be32(icc, e + 4) as usize, be32(icc, e + 8) as usize);
            if off + size > icc.len() || size < 8 {
                fail(format!("ICC tag {} at {off} + {size} runs past the profile's {} bytes", String::from_utf8_lossy(&icc[e..e + 4]), icc.len()));
            }
            (icc[e..e + 4].try_into().unwrap(), off, size)
        })
        .collect()
}

/// The data of tag `sig`, type signature first.
fn tag<'a>(icc: &'a [u8], table: &[([u8; 4], usize, usize)], sig: &[u8; 4]) -> Option<&'a [u8]> {
    table.iter().find(|t| &t.0 == sig).map(|&(_, off, size)| &icc[off..off + size])
}

/// A TRC tag as a pure power: `para` function type 0 (Y = X^g), or `curv` with no entries (identity) or one (a u8Fixed8 exponent).
/// Anything else — a sampled curve, a piecewise sRGB-style function — stops the tool: it would need applying as written, not reading as 2.4.
fn trc_gamma(t: &[u8], which: &str) -> f64 {
    match &t[0..4] {
        b"para" if t.len() >= 16 && be16(t, 8) == 0 => s15f16(t, 12),
        b"para" => fail(format!("{which}: parametric curve type {} is not a pure power", be16(t, 8))),
        b"curv" if t.len() >= 12 && be32(t, 8) == 0 => 1.,
        b"curv" if t.len() >= 14 && be32(t, 8) == 1 => be16(t, 12) as f64 / (1 << 8) as f64,
        b"curv" => fail(format!("{which}: a sampled curve of {} points, not a pure power", be32(t, 8))),
        other => fail(format!("{which}: TRC type '{}'", String::from_utf8_lossy(other))),
    }
}

/// An XYZ tag's one triple.
fn xyz(t: &[u8], which: &str) -> [f64; 3] {
    if &t[0..4] != b"XYZ " || t.len() < 20 {
        fail(format!("{which}: not an XYZ tag"));
    }
    [s15f16(t, 8), s15f16(t, 12), s15f16(t, 16)]
}

fn inverse3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c = |r: usize, k: usize| m[(r + 1) % 3][(k + 1) % 3] * m[(r + 2) % 3][(k + 2) % 3] - m[(r + 1) % 3][(k + 2) % 3] * m[(r + 2) % 3][(k + 1) % 3];
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    if det == 0. {
        fail("ICC chad matrix is singular");
    }
    let mut out = [[0.; 3]; 3];
    for r in 0..3 {
        for k in 0..3 {
            out[k][r] = c(r, k) / det;
        }
    }
    out
}

/// The profile's name: a v4 `mluc`'s first record (UTF-16BE) or a v2 `desc`'s ASCII.
fn description(t: &[u8]) -> String {
    match &t[0..4] {
        b"mluc" if t.len() >= 28 => {
            let (len, off) = (be32(t, 20) as usize, be32(t, 24) as usize);
            // WHY/PROOF: the record's length and offset are file data; past the tag they would index out of its slice.
            if off + len > t.len() {
                return String::new();
            }
            let units: Vec<u16> = (0..len / 2).map(|k| be16(t, off + 2 * k)).collect();
            String::from_utf16_lossy(&units)
        }
        b"desc" if t.len() >= 12 => {
            let len = be32(t, 8) as usize;
            // WHY/PROOF: the count is file data; past the tag it would index out of its slice.
            if 12 + len > t.len() {
                return String::new();
            }
            String::from_utf8_lossy(&t[12..12 + len]).trim_end_matches('\0').into()
        }
        _ => String::new(),
    }
}

/// Reads the profile and stops unless it is an RGB display space on Rec.2020's primaries and white with a pure-power TRC per channel.
fn read_profile(icc: &[u8]) -> Profile {
    if &icc[16..20] != b"RGB " || &icc[20..24] != b"XYZ " {
        fail(format!("ICC profile is {} → {}, not RGB → XYZ", String::from_utf8_lossy(&icc[16..20]), String::from_utf8_lossy(&icc[20..24])));
    }
    let table = tags(icc);
    let get = |sig: &[u8; 4]| tag(icc, &table, sig).unwrap_or_else(|| fail(format!("ICC profile has no {} tag", String::from_utf8_lossy(sig))));
    let gamma = [trc_gamma(get(b"rTRC"), "rTRC"), trc_gamma(get(b"gTRC"), "gTRC"), trc_gamma(get(b"bTRC"), "bTRC")];
    // The colorants are in the D50 connection space; chad (row-major) took the source white there, so its inverse takes them back to what the space itself is.
    let unadapt = match tag(icc, &table, b"chad") {
        Some(t) if &t[0..4] == b"sf32" && t.len() >= 44 => inverse3([[s15f16(t, 8), s15f16(t, 12), s15f16(t, 16)], [s15f16(t, 20), s15f16(t, 24), s15f16(t, 28)], [s15f16(t, 32), s15f16(t, 36), s15f16(t, 40)]]),
        Some(_) => fail("ICC chad tag is not an sf32 3 × 3"),
        None => [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
    };
    let colorants = [xyz(get(b"rXYZ"), "rXYZ"), xyz(get(b"gXYZ"), "gXYZ"), xyz(get(b"bXYZ"), "bXYZ")];
    let src: Vec<[f64; 3]> = colorants.iter().map(|c| [0, 1, 2].map(|r| unadapt[r][0] * c[0] + unadapt[r][1] * c[1] + unadapt[r][2] * c[2])).collect();
    let white = [0, 1, 2].map(|k| src[0][k] + src[1][k] + src[2][k]);
    let xy = |v: [f64; 3]| [v[0] / (v[0] + v[1] + v[2]), v[1] / (v[0] + v[1] + v[2])];
    let found = [xy(src[0]), xy(src[1]), xy(src[2]), xy(white)];
    for (k, (got, want)) in found.iter().zip(REC2020_XY.iter()).enumerate() {
        if (got[0] - want[0]).abs() > XY_TOLERANCE || (got[1] - want[1]).abs() > XY_TOLERANCE {
            let what = ["red", "green", "blue", "white"][k];
            fail(format!("ICC {what} is xy {:.4} {:.4}, Rec.2020's is {:.4} {:.4}: not a Rec.2020 profile", got[0], got[1], want[0], want[1]));
        }
    }
    let name = tag(icc, &table, b"desc").map(description).unwrap_or_default().trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    Profile { name, gamma }
}

/// Per-channel running statistics of linear light.
struct Stats {
    min: [f32; 3],
    max: [f32; 3],
    sum: [f64; 3],
}

impl Stats {
    fn new() -> Self {
        Stats { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3], sum: [0.; 3] }
    }
    fn add(&mut self, v: [f32; 3]) {
        for c in 0..3 {
            self.min[c] = self.min[c].min(v[c]);
            self.max[c] = self.max[c].max(v[c]);
            self.sum[c] += v[c] as f64;
        }
    }
    fn print(&self, label: &str, n: usize) {
        let ch = ["R", "G", "B"];
        let parts: Vec<String> = (0..3).map(|c| format!("{} {:.5}..{:.5} mean {:.5}", ch[c], self.min[c], self.max[c], self.sum[c] / n as f64)).collect();
        println!("  {label}: {}", parts.join(" | "));
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let Some(input) = a.get(1).map(PathBuf::from) else { fail("usage: wallpaper <in.tif> [out.vsf]") };
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|e| fail(format!("ferros repo: {e}")));
    let out = a.get(2).map(PathBuf::from).unwrap_or_else(|| {
        let dir = repo.join("target/wallpaper");
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| fail(format!("{}: {e}", dir.display())));
        let stem = input.file_stem().unwrap_or_else(|| fail(format!("{}: no file name", input.display())));
        dir.join(stem).with_extension("vsf")
    });

    let file = std::fs::File::open(&input).unwrap_or_else(|e| fail(format!("{}: {e}", input.display())));
    let mut dec = Decoder::new(std::io::BufReader::new(file)).unwrap_or_else(|e| fail(format!("{}: {e}", input.display())));
    let (w, h) = dec.dimensions().unwrap_or_else(|e| fail(e));
    let (w, h) = (w as usize, h as usize);
    match dec.colortype() {
        Ok(tiff::ColorType::RGB(16)) => {}
        Ok(other) => fail(format!("{}: {other:?}, this tool reads 16-bit RGB", input.display())),
        Err(e) => fail(e),
    }
    // Planar configuration 2 (one plane per channel) would decode as something other than R, G, B interleaved; this tool reads the chunky layout only.
    if dec.find_tag_unsigned::<u16>(Tag::PlanarConfiguration).unwrap_or_else(|e| fail(e)).unwrap_or(1) != 1 {
        fail("planar-configuration TIFF (one plane per channel): only chunky R, G, B is read");
    }
    let icc = dec.find_tag(Tag::IccProfile).unwrap_or_else(|e| fail(e)).map(|v| v.into_u8_vec().unwrap_or_else(|e| fail(format!("ICC profile tag: {e}"))));
    let profile = match &icc {
        Some(icc) => read_profile(icc),
        None => {
            println!("  no ICC profile: reading the codes as Rec.2020 primaries at gamma 2.4");
            Profile { name: String::new(), gamma: [2.4; 3] }
        }
    };
    let codes = match dec.read_image().unwrap_or_else(|e| fail(e)) {
        DecodingResult::U16(v) => v,
        _ => fail("decoder did not hand back 16-bit samples"),
    };
    if codes.len() != 3 * w * h {
        fail(format!("{} samples for 3 × {w} × {h}", codes.len()));
    }

    let n = w * h;
    // The photopic luminance of linear VSF RGB: the LMS of each primary weighted by the cones' shares of V*₁₀; VSF white [1, 1, 1] has luminance 1.
    let lum: [f32; 3] = [0, 1, 2].map(|k| LMS2PHOTOPIC[0] * VSF_RGB2LMS[3 * k] + LMS2PHOTOPIC[1] * VSF_RGB2LMS[3 * k + 1] + LMS2PHOTOPIC[2] * VSF_RGB2LMS[3 * k + 2]);
    let top = ((1u32 << 16) - 1) as f32;
    let (mut src_stats, mut vsf_stats) = (Stats::new(), Stats::new());
    let (mut outside, mut deepest, mut deepest_rel, mut moved) = (0usize, [0f32; 3], 0f32, 0f32);
    let mut light = vec![0f32; 3 * n];
    for i in 0..n {
        let rgb = [0, 1, 2].map(|c| (codes[3 * i + c] as f32 / top).powf(profile.gamma[c] as f32));
        src_stats.add(rgb);
        let mut v = apply_matrix_3x3_f32(&REC2020_2VSF_RGB, &rgb);
        vsf_stats.add(v);
        let low = v[0].min(v[1]).min(v[2]);
        if low < 0. {
            // Y > 0 here: every Rec.2020 primary has positive luminance in VSF RGB and the source's codes are non-negative, so a pixel with a negative channel has light in it and y − low > y > 0.
            let y = lum[0] * v[0] + lum[1] * v[1] + lum[2] * v[2];
            outside += 1;
            for c in 0..3 {
                deepest[c] = deepest[c].min(v[c]);
            }
            deepest_rel = deepest_rel.min(low / y);
            let t = y / (y - low);
            for c in 0..3 {
                let mapped = y + t * (v[c] - y);
                moved = moved.max((mapped - v[c]).abs());
                v[c] = mapped;
            }
        }
        for c in 0..3 {
            light[c * n + i] = v[c];
        }
    }
    let samples: Vec<u16> = light.iter().map(|&l| delinearize_gamma2tanh_bits_f32(l, BITS) as u16).collect();

    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut plane = Plane::encoded("display", "wallpaper", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "main".into();
    let text = format!("{} in Rec.2020 primaries ({}), taken to VSF RGB by vsf's Rec.2020 matrix; {outside} pixels past VSF RGB mixed toward the neutral of their own luminance.", input.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), if profile.name.is_empty() { "no ICC profile".into() } else { format!("ICC {}", profile.name) });
    let v = Visual { title: stem, text, planes: vec![plane], ..Default::default() };
    let bytes = v.write().unwrap_or_else(|e| fail(format!("visual: {e}")));
    std::fs::write(&out, &bytes).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));

    println!("{}: {w}x{h}, ICC '{}', gamma {:.5} {:.5} {:.5}", input.display(), profile.name, profile.gamma[0], profile.gamma[1], profile.gamma[2]);
    src_stats.print("Rec.2020 linear", n);
    vsf_stats.print("VSF RGB linear ", n);
    if outside == 0 {
        println!("  past VSF RGB: none of {n} pixels (no channel below 0 after the matrix; nothing mixed)");
    } else {
        println!("  past VSF RGB: {outside} of {n} pixels ({:.3} %), lowest R {:.5} G {:.5} B {:.5}, deepest {:.3} % of the pixel's luminance; mixing moved a channel at most {:.5}", 100. * outside as f64 / n as f64, deepest[0], deepest[1], deepest[2], -100. * deepest_rel, moved);
    }

    // Read the file back with vsf and check every sample decodes to the bucket holding its light.
    let back = Visual::read_file(&out).unwrap_or_else(|e| fail(format!("read back {}: {e:?}", out.display())));
    let p = &back.planes[0];
    let Samples::Packed(t) = &p.samples else { fail("read back: plane is not bit-packed") };
    let got = t.unpack_u16();
    if (p.width, p.height, p.primaries, p.encoding, got.len()) != (w, h, Primaries::Vsf, Encoding::Gamma2Tanh, 3 * n) {
        fail(format!("read back: {}x{} {} {} with {} samples", p.width, p.height, p.primaries.as_str(), p.encoding.as_str(), got.len()));
    }
    let top_code = (1u32 << BITS) - 1;
    let mut wrong = 0usize;
    for (k, (&code, &l)) in got.iter().zip(light.iter()).enumerate() {
        let c = code as u32;
        let ok = code == samples[k] && (linearize_gamma2tanh_bits_f32(c, BITS) <= l || c == 0) && (c == top_code || linearize_gamma2tanh_bits_f32(c + 1, BITS) > l);
        wrong += !ok as usize;
    }
    let brightest = (0..n).max_by(|&i, &j| (light[i] + light[n + i] + light[2 * n + i]).total_cmp(&(light[j] + light[n + j] + light[2 * n + j]))).unwrap();
    for (label, i) in [("top left", 0), ("centre", (h / 2) * w + w / 2), ("bottom right", n - 1), ("brightest", brightest)] {
        let dec = [0, 1, 2].map(|c| linearize_gamma2tanh_bits_f32(got[c * n + i] as u32, BITS));
        println!("  {label:>12} ({}, {}): computed {:.5} {:.5} {:.5} → codes {} {} {} → read back {:.5} {:.5} {:.5}", i % w, i / w, light[i], light[n + i], light[2 * n + i], got[i], got[n + i], got[2 * n + i], dec[0], dec[1], dec[2]);
    }
    println!("{}: {} bytes; read back {} samples, {wrong} outside their bucket", out.display(), bytes.len(), got.len());
    if wrong != 0 {
        std::process::exit(1);
    }
}
