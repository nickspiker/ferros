// raw-codec: the host side of the RAW codec prototype: round-trip tests, ratio and noise reports over real frames, and a timing bench that also runs on a device.
// Numbers print as A# (decimal) and G# (hex).
//
// Commands:
//   raw-codec selftest
//   raw-codec report <frames list> <results.txt>
//   raw-codec bench <file> <width> <height> <stride> <period 2|4> <runs> [variant] [band rows] [decode]
// A frames list has one frame a line: label path width height stride period (lines starting # are skipped).
// Variants: see variant() (med-block, avg4-block, adapt2-block, med-loco, ...); "default" is Params::DEFAULT.

use raw_codec::*;
use std::fmt::Write as _;
use std::time::Instant;

const BLACK: u32 = 1 << 6;
const DEFAULT_BAND: usize = 1 << 6;

struct Frame {
    label: String,
    w: usize,
    h: usize,
    stride: usize,
    cfa: Cfa,
    raw: Vec<u8>,
}

fn a(v: impl std::fmt::Display) -> String {
    format!("A#{v}")
}

fn af(v: f64, prec: usize) -> String {
    format!("A#{v:.prec$}")
}

fn variant(name: &str) -> Params {
    let mut p = Params::DEFAULT;
    p.predictor = Predictor::Med;
    match name {
        "med-block" => {}
        "default" => p = Params::DEFAULT,
        "avg-block" => p.predictor = Predictor::Avg,
        "left-block" => p.predictor = Predictor::Left,
        "med-block-best3" => p.kpick = KPick::Best3,
        "med-block-split" => p.split = true,
        "med-block-b4" => p.block_log2 = 4,
        "med-block-b6" => p.block_log2 = 6,
        "med-block-e3" => p.esc_log2 = 3,
        "avg4-block" => p.predictor = Predictor::Avg4,
        "grad-block" => p.predictor = Predictor::Grad,
        "up-block" => p.predictor = Predictor::Up,
        "med-block-1s" => p.two_streams = false,
        "avg4-block-1s" => {
            p.predictor = Predictor::Avg4;
            p.two_streams = false;
        }
        "adapt2-block" => p.predictor = Predictor::Adapt2,
        "adapt4-block" => p.predictor = Predictor::Adapt4,
        "adapt4-block-b4" => {
            p.predictor = Predictor::Adapt4;
            p.block_log2 = 4;
        }
        "adapt4-block-b6" => {
            p.predictor = Predictor::Adapt4;
            p.block_log2 = 6;
        }
        "med-loco" => p.coder = Coder::Loco,
        _ => panic!("unknown variant {name}"),
    }
    p
}

const VARIANTS: [&str; 17] = ["left-block", "up-block", "avg-block", "med-block", "avg4-block", "grad-block", "adapt2-block", "adapt4-block", "adapt4-block-b4", "adapt4-block-b6", "med-block-best3", "med-block-split", "med-block-b4", "med-block-b6", "med-block-e3", "med-block-1s", "med-loco"];

fn unpack_frame(f: &Frame) -> Vec<u16> {
    let mut px = vec![0u16; f.w * f.h];
    for y in 0..f.h {
        let row = &f.raw[y * f.stride..y * f.stride + f.w * 5 / 4];
        for (g, d) in row.chunks_exact(5).zip(px[y * f.w..(y + 1) * f.w].chunks_exact_mut(4)) {
            for k in 0..4 {
                d[k] = ((g[k] as u16) << 2) | ((g[4] as u16 >> (2 * k)) & 3);
            }
        }
    }
    px
}

/// Encode a whole frame band by band into `out` (sized by the caller), each band's bytes back to back; returns the bands' (offset, length).
fn encode_frame(f: &Frame, band: usize, p: &Params, lut: Option<&[u16; 1024]>, work: &mut Work, out: &mut [u8], bands: &mut Vec<(usize, usize)>) -> usize {
    bands.clear();
    let mut pos = 0;
    let mut y = 0;
    while y < f.h {
        let rows = band.min(f.h - y);
        let src = &f.raw[y * f.stride..];
        let n = encode_band(src, f.stride, f.w, rows, f.cfa, p, lut, work, &mut out[pos..]);
        bands.push((pos, n));
        pos += n;
        y += rows;
    }
    pos
}

fn decode_frame(f: &Frame, band: usize, p: &Params, work: &mut Work, out: &[u8], bands: &[(usize, usize)], dst: &mut [u16]) {
    let mut y = 0;
    for &(off, len) in bands {
        let rows = band.min(f.h - y);
        let used = decode_band(&out[off..off + len], f.w, rows, f.cfa, p, work, &mut dst[y * f.w..], f.w);
        assert_eq!(used, len, "band at row {y}: decoder used {used} of {len} bytes");
        y += rows;
    }
}

fn out_len(f: &Frame, band: usize) -> usize {
    let nb = f.h.div_ceil(band);
    nb * max_encoded_len(f.w, band)
}

/// Band index cost counted with the bands: 4 bytes a band (its length), so small bands pay for their own table.
const BAND_INDEX: usize = 4;

fn coded_size(bands: &[(usize, usize)]) -> usize {
    bands.iter().map(|b| b.1 + BAND_INDEX).sum()
}

#[cfg(target_arch = "x86_64")]
fn tsc() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86_64.
    unsafe { core::arch::x86_64::_rdtsc() }
}
#[cfg(not(target_arch = "x86_64"))]
fn tsc() -> u64 {
    0
}

fn load(label: &str, path: &str, w: usize, h: usize, stride: usize, period: usize) -> Frame {
    let raw = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(raw.len() >= (h - 1) * stride + w * 5 / 4, "{path}: {} bytes is short for {w} x {h} at stride {stride}", raw.len());
    let cfa = if period == 4 { Cfa::Quad } else { Cfa::Bayer };
    Frame { label: label.to_string(), w, h, stride, cfa, raw }
}

// ---------------------------------------------------------------- noise

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.unit(), self.unit());
        (-2. * u.ln()).sqrt() * (2. * std::f64::consts::PI * v).cos()
    }
}

const NB: usize = 1 << 3;

/// Noise variance of one 8 x 8 block of same-colour samples: horizontal and vertical second differences (2v - left - right), which cancel any linear ramp, their mean square over 6 (the variance of 2v - l - r for independent noise is 6 sigma^2).
fn block_var(v: &[[f64; NB]; NB]) -> f64 {
    let mut s = 0.;
    let mut n = 0.;
    for i in 0..NB {
        for j in 1..NB - 1 {
            let eh = 2. * v[i][j] - v[i][j - 1] - v[i][j + 1];
            let ev = 2. * v[j][i] - v[j - 1][i] - v[j + 1][i];
            s += eh * eh + ev * ev;
            n += 2.;
        }
    }
    s / n / 6.
}

/// The lower-quartile estimator's bias for pure noise, by simulation: the quartile of block_var over many blocks of unit Gaussian noise.
fn quartile_bias() -> f64 {
    let mut r = Rng(G_SEED);
    let mut vs: Vec<f64> = (0..1 << 14)
        .map(|_| {
            let mut v = [[0.; NB]; NB];
            for row in v.iter_mut() {
                for x in row.iter_mut() {
                    *x = r.gauss();
                }
            }
            block_var(&v)
        })
        .collect();
    vs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    vs[vs.len() >> 2]
}

const G_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

struct Noise {
    g: f64,
    r2: f64,
    /// The fit's own intercept before the floor.
    r2_fit: f64,
    bins_used: usize,
    /// (level over black, sigma^2 estimate, blocks) per level bin used.
    pts: Vec<(f64, f64, usize)>,
}

/// Photon transfer from one frame: per 8 x 8 block of each colour plane (16 x 16 pixels) its mean and noise variance (block_var); blocks binned by mean in 2^4 DN bins; each bin's lower quartile of the variances (texture only adds variance, so the flattest blocks are the noise), corrected by the simulated quartile bias; then sigma^2 = g (mean - black) + r^2 fitted by least squares weighted by blocks / sigma^4.
/// Blocks with any sample at or above 2^10 - 2^5 are left out (clipping flattens them).
fn noise_estimate(px: &[u16], w: usize, h: usize, bias: f64) -> Noise {
    let nbins = 1 << 6;
    let mut bins: Vec<Vec<f64>> = vec![Vec::new(); nbins];
    let sat = (1u16 << 10) - (1 << 5);
    for py in 0..2 {
        for pxo in 0..2 {
            let pw = w / 2;
            let ph = h / 2;
            for by in 0..ph / NB {
                for bx in 0..pw / NB {
                    let mut v = [[0.; NB]; NB];
                    let mut sum = 0.;
                    let mut clipped = false;
                    for i in 0..NB {
                        for j in 0..NB {
                            let y = py + 2 * (by * NB + i);
                            let x = pxo + 2 * (bx * NB + j);
                            let s = px[y * w + x];
                            clipped |= s >= sat;
                            v[i][j] = s as f64;
                            sum += s as f64;
                        }
                    }
                    if clipped {
                        continue;
                    }
                    let mean = sum / (NB * NB) as f64;
                    bins[(mean as usize) >> 4].push(block_var(&v));
                }
            }
        }
    }
    let mut pts = Vec::new();
    for (i, b) in bins.iter_mut().enumerate() {
        if b.len() < 1 << 6 {
            continue;
        }
        b.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = b[b.len() >> 2] / bias;
        let level = ((i << 4) + (1 << 3)) as f64 - BLACK as f64;
        pts.push((level, q, b.len()));
    }
    // Weighted least squares of var = g L + r2.
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0., 0., 0., 0., 0.);
    for &(l, v, n) in &pts {
        let wt = n as f64 / (v * v);
        sw += wt;
        sx += wt * l;
        sy += wt * v;
        sxx += wt * l * l;
        sxy += wt * l * v;
    }
    let det = sw * sxx - sx * sx;
    let (g, r2) = if pts.len() >= 2 && det > 0. {
        let g = (sw * sxy - sx * sy) / det;
        (g, (sy - g * sx) / sw)
    } else {
        (0., if pts.is_empty() { 1. } else { pts[0].1 })
    };
    // The read noise: the fit's intercept, or where the darkest bin measured more noise than the line leaves at its level, that bin's own measure less its shot part (the line bends near black in these frames, the fit's intercept going negative while the darkest blocks measure 1.5 DN).
    // WHY the floors: every 10-bit sample carries at least the ADC's own quantisation variance, 1/12 DN^2, and a sigma of zero at black would leave the step and the err/sigma report undefined.
    let r2_fit = r2;
    let dark = pts.first().map(|&(l, v, _)| v - g * l.max(0.)).unwrap_or(0.);
    let r2 = r2.max(dark).max(1. / 12.);
    Noise { g, r2, r2_fit, bins_used: pts.len(), pts }
}

// ---------------------------------------------------------------- report

fn report(list: &str, out_path: &str) {
    let text = std::fs::read_to_string(list).unwrap();
    let bias = quartile_bias();
    let mut rep = String::new();
    let mut emit = |s: String| {
        println!("{s}");
        rep.push_str(&s);
        rep.push('\n');
    };
    emit(format!("raw-codec results (prototype; sizes count A#{BAND_INDEX} bytes a band for its length; ratio = packed RAW10 bytes / coded bytes; bpp = coded bits a pixel against 10 packed)"));
    emit(format!("noise estimator: lower-quartile bias for pure noise {} (divided out)", af(bias, 4)));
    let mut work = Box::new(Work::new());
    let mut bands = Vec::new();
    let (mut sum_lossless, mut sum_band, mut sum_noise, mut sum_shot) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.is_empty() || t[0].starts_with('#') {
            continue;
        }
        let (w, h, stride, period): (usize, usize, usize, usize) = (t[2].parse().unwrap(), t[3].parse().unwrap(), t[4].parse().unwrap(), t[5].parse().unwrap());
        let note = t[6..].join(" ");
        let f = load(t[0], t[1], w, h, stride, period);
        let px = unpack_frame(&f);
        let npx = (w * h) as f64;
        let packed = (w * h * 10 / 8) as f64;
        let mean = px.iter().map(|&v| v as f64).sum::<f64>() / npx;
        let sat = px.iter().filter(|&&v| v >= 1023).count() as f64 / npx;
        emit(String::new());
        emit(format!("== {} ({} x {}, stride {}, period {}) {}", f.label, a(w), a(h), a(stride), a(period), note));
        emit(format!("   mean {} DN ({} over black), at white {}%", af(mean, 1), af(mean - BLACK as f64, 1), af(sat * 100., 3)));
        // Fixed pattern: the spread of whole-column and whole-row means among same-colour columns (rows), which no temporal noise survives at these lengths.
        let mut colm = vec![0f64; w];
        let mut rowm = vec![0f64; h];
        for y in 0..h {
            for x in 0..w {
                colm[x] += px[y * w + x] as f64;
                rowm[y] += px[y * w + x] as f64;
            }
        }
        let spread = |v: &[f64], n: f64| -> f64 {
            let mut s = 0.;
            for ph in 0..2 {
                let c: Vec<f64> = v.iter().skip(ph).step_by(2).map(|&t| t / n).collect();
                let m = c.iter().sum::<f64>() / c.len() as f64;
                s += c.iter().map(|&t| (t - m) * (t - m)).sum::<f64>() / c.len() as f64;
            }
            (s / 2.).sqrt()
        };
        emit(format!("   column means spread {} DN, row means spread {} DN (same-colour, rms)", af(spread(&colm, h as f64), 2), af(spread(&rowm, w as f64), 2)));
        let mut out = vec![0u8; out_len(&f, h.max(DEFAULT_BAND))];
        let mut dst = vec![0u16; w * h];
        emit(format!("   band {} rows: variant, bpp, ratio, encode ms (unpinned, one run)", a(DEFAULT_BAND)));
        let mut vb: Vec<String> = Vec::new();
        for name in VARIANTS {
            let p = variant(name);
            let t0 = Instant::now();
            encode_frame(&f, DEFAULT_BAND, &p, None, &mut work, &mut out, &mut bands);
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            decode_frame(&f, DEFAULT_BAND, &p, &mut work, &out, &bands, &mut dst);
            let bad = px.iter().zip(dst.iter()).filter(|(a, b)| a != b).count();
            assert!(bad == 0, "{}: {name}: {bad} samples differ after the round trip", f.label);
            let sz = coded_size(&bands) as f64;
            emit(format!("     {:<16} {:>8} {:>8} {:>9}  round trip exact", name, af(sz * 8. / npx, 3), af(packed / sz, 3), af(ms, 1)));
            vb.push(af(sz * 8. / npx, 3));
        }
        let pick = [0usize, 1, 2, 3, 4, 5, 6, 7, 10, 16];
        let mut sl = format!("{:<16} {:>7}", f.label, af(mean - BLACK as f64, 1));
        for &i in &pick {
            let _ = write!(sl, " {:>7}", vb[i]);
        }
        sum_lossless.push(sl);
        emit("   band height sensitivity (bpp; med-block, avg4-block, adapt2-block, med-loco):".to_string());
        let heights: Vec<usize> = vec![1 << 2, 1 << 3, 1 << 4, 1 << 5, 1 << 6, 1 << 7, 1 << 8, 1 << 10, h];
        for &bh in &heights {
            let mut line = format!("     {:>8} rows", if bh == h { format!("{} (whole)", a(bh)) } else { a(bh) });
            for name in ["med-block", "avg4-block", "adapt2-block", "med-loco"] {
                let p = variant(name);
                let mut o2 = vec![0u8; out_len(&f, bh)];
                encode_frame(&f, bh, &p, None, &mut work, &mut o2, &mut bands);
                decode_frame(&f, bh, &p, &mut work, &o2, &bands, &mut dst);
                assert!(px == dst, "{}: {name} band {bh}: round trip differs", f.label);
                let sz = coded_size(&bands) as f64;
                let _ = write!(line, "  {} {}", name, af(sz * 8. / npx, 3));
            }
            emit(line.clone());
            if bh == 1 << 4 || bh == 1 << 6 || bh == h {
                sum_band.push(format!("{:<16} {:<12} {}", f.label, if bh == h { "whole".to_string() } else { a(bh) }, line.split("rows").nth(1).unwrap_or("").trim()));
            }
        }
        if period != 2 {
            continue;
        }
        let nz = noise_estimate(&px, w, h, bias);
        emit(format!("   noise (photon transfer, {} level bins): g {} DN/e, r {} DN (sigma^2 = g (x - black) + r^2; r^2 the largest of the fit's {}, the darkest bin's less its shot part, and 1/12)", a(nz.bins_used), af(nz.g, 4), af(nz.r2.sqrt(), 3), af(nz.r2_fit, 3)));
        sum_noise.push(format!("{:<16} {:>8} DN/e  r {:>6} DN  ({} level bins)", f.label, af(nz.g, 4), af(nz.r2.sqrt(), 3), a(nz.bins_used)));
        let mut pl = String::from("     bins (level over black: sigma DN):");
        for (i, &(l, v, _)) in nz.pts.iter().enumerate() {
            if i % 2 == 0 || i + 1 == nz.pts.len() {
                let _ = write!(pl, " {}:{}", af(l, 0), af(v.sqrt(), 2));
            }
        }
        emit(pl);
        if nz.g <= 0. {
            emit("   shot-noise mode skipped: no usable noise fit for this frame".to_string());
            continue;
        }
        let rows = shot_report(&f, &px, &nz, &mut work, &mut out, &mut dst, &mut bands, &mut emit);
        for r in rows {
            sum_shot.push(format!("{:<16} {:<6} {:>7} {:>7} {:>7} {:>7}   {:>7} {:>7} {:>7}", f.label, format!("1/{}", a(1 << r.0)), af(r.1, 3), af(r.2, 3), af(r.3, 3), af(r.4, 3), af(r.5, 3), af(r.6, 3), af(r.7, 3)));
        }
    }
    emit(String::new());
    emit("== SUMMARY".to_string());
    emit("lossless bpp at band 64 (10 = packed RAW10; ratio = 10 / bpp):".to_string());
    emit(format!("{:<16} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}", "frame", "over bk", "left", "up", "avg", "med", "avg4", "grad", "adapt2", "adapt4", "med-b3", "loco"));
    for l in &sum_lossless {
        emit(l.clone());
    }
    emit(String::new());
    emit("band height (bpp at 16 rows, 64 rows, the whole frame as one band):".to_string());
    for l in &sum_band {
        emit(l.clone());
    }
    emit(String::new());
    emit("noise (photon transfer per frame):".to_string());
    for l in &sum_noise {
        emit(l.clone());
    }
    emit(String::new());
    emit("shot-noise mode (adapt2-block ratio; worst over level bands of at least 2^10 samples, sigma measured): fixed inverse RMS err/sigma, |mean err|/sigma, max |err|/sigma; own centroids the same three".to_string());
    for l in &sum_shot {
        emit(l.clone());
    }
    std::fs::write(out_path, rep).unwrap();
}

/// Measured noise sigma at a level, from the photon transfer bins: variance interpolated linearly between bin centres, held at the end bins' values beyond them (no data there to say otherwise).
fn sigma_measured(nz: &Noise, l: f64) -> f64 {
    let p = &nz.pts;
    if l <= p[0].0 {
        return p[0].1.sqrt();
    }
    for w in p.windows(2) {
        if l <= w[1].0 {
            let t = (l - w[0].0) / (w[1].0 - w[0].0);
            return (w[0].1 + t * (w[1].1 - w[0].1)).sqrt();
        }
    }
    p[p.len() - 1].1.sqrt()
}

/// One frame's shot-noise line for the summary: s_log2, ratio, then over level bands with at least 2^10 samples the worst RMS err/sigma, |mean err|/sigma and max |err|/sigma with the fixed inverse, then the same three with the frame's own centroids.
type ShotRow = (u32, f64, f64, f64, f64, f64, f64, f64);

#[allow(clippy::too_many_arguments)]
fn shot_report(f: &Frame, px: &[u16], nz: &Noise, work: &mut Work, out: &mut [u8], dst: &mut [u16], bands: &mut Vec<(usize, usize)>, emit: &mut impl FnMut(String)) -> Vec<ShotRow> {
    let npx = (f.w * f.h) as f64;
    let packed = (f.w * f.h * 10 / 8) as f64;
    let g_q16 = (nz.g * 65536.).round() as u64;
    let r2_q16 = (nz.r2 * 65536.).round() as u64;
    emit("   shot-noise mode (adapt2-block, band 64): s, code values, bpp, ratio, avg4-block ratio, med-loco ratio".to_string());
    emit("     then per level band (x - black): samples; RMS err/sigma, mean err DN, mean err/sigma, max |err| DN, max |err|/sigma with the fixed inverse (each code's centroid under a flat spread); the same with the frame's own centroids (a table of at most 1024 values the encoder would send)".to_string());
    emit("     sigma is the noise measured in the frame at that level (the photon transfer bins), not the fitted model the quantiser used".to_string());
    let mut rows = Vec::new();
    let sig: Vec<f64> = (0..1024).map(|x| sigma_measured(nz, x as f64 - BLACK as f64)).collect();
    for s_log2 in [0u32, 1, 2] {
        let mut fwd = [0u16; 1024];
        let mut inv = [0u16; 1024];
        let nc = shot_tables(BLACK, g_q16, r2_q16, s_log2, &mut fwd, &mut inv);
        let codes: Vec<u16> = px.iter().map(|&x| fwd[x as usize]).collect();
        let mut sizes = Vec::new();
        for name in ["adapt2-block", "avg4-block", "med-loco"] {
            let p = variant(name);
            encode_frame(f, DEFAULT_BAND, &p, Some(&fwd), work, out, bands);
            decode_frame(f, DEFAULT_BAND, &p, work, out, bands, dst);
            assert!(codes == dst, "{}: shot mode {name}: code values differ after the round trip", f.label);
            sizes.push(coded_size(bands) as f64);
        }
        emit(format!("     s = 1/{}  {} codes  bpp {}  ratio {}  avg4-block ratio {}  med-loco ratio {}", a(1 << s_log2), a(nc), af(sizes[0] * 8. / npx, 3), af(packed / sizes[0], 3), af(packed / sizes[1], 3), af(packed / sizes[2], 3)));
        // The frame's own centroids: per code value the mean of the samples it holds, rounded half to even.
        let mut sum = [0u64; 1024];
        let mut cnt = [0u64; 1024];
        for &x in px {
            sum[fwd[x as usize] as usize] += x as u64;
            cnt[fwd[x as usize] as usize] += 1;
        }
        let mut own = inv;
        for c in 0..1024 {
            if cnt[c] > 0 {
                let (q, r) = (sum[c] / cnt[c], sum[c] % cnt[c]);
                own[c] = (q + ((2 * r > cnt[c]) | ((2 * r == cnt[c]) & (q & 1 == 1))) as u64) as u16;
            }
        }
        // Level bands: below black, then [0, 4), [4, 16), [16, 64), [64, 256), [256, 1024).
        let mut st = [[(0usize, 0f64, 0f64, 0f64, 0f64, 0f64); 6]; 2];
        for (&x, &c) in px.iter().zip(dst.iter()) {
            let l = x as i32 - BLACK as i32;
            let b = if l < 0 { 0 } else { (((32 - (l as u32 | 1).leading_zeros()) + 1) >> 1) as usize };
            let sg = sig[x as usize];
            for (k, table) in [&inv, &own].iter().enumerate() {
                let e = table[c as usize] as f64 - x as f64;
                let s = &mut st[k][b];
                s.0 += 1;
                s.1 += (e / sg) * (e / sg);
                s.2 += e;
                s.3 += e / sg;
                s.4 = s.4.max(e.abs());
                s.5 = s.5.max(e.abs() / sg);
            }
        }
        let names = ["<0", "0-4", "4-16", "16-64", "64-256", "256-960"];
        let mut worst = [0f64; 6];
        for i in 0..6 {
            let (s, o) = (st[0][i], st[1][i]);
            if s.0 == 0 {
                continue;
            }
            let n = s.0 as f64;
            emit(format!("        {:<8} {:>10}  fixed {:>7} {:>7} {:>7} {:>4} {:>6}  own {:>7} {:>7} {:>7} {:>4} {:>6}", names[i], a(s.0), af((s.1 / n).sqrt(), 3), af(s.2 / n, 3), af(s.3 / n, 3), af(s.4, 0), af(s.5, 3), af((o.1 / n).sqrt(), 3), af(o.2 / n, 3), af(o.3 / n, 3), af(o.4, 0), af(o.5, 3)));
            // The summary's worst cases over bands with at least 2^10 samples (a handful of samples in a band says nothing about bias).
            if s.0 >= 1 << 10 {
                worst[0] = worst[0].max((s.1 / n).sqrt());
                worst[1] = worst[1].max((s.3 / n).abs());
                worst[2] = worst[2].max(s.5);
                worst[3] = worst[3].max((o.1 / n).sqrt());
                worst[4] = worst[4].max((o.3 / n).abs());
                worst[5] = worst[5].max(o.5);
            }
        }
        rows.push((s_log2, packed / sizes[0], worst[0], worst[1], worst[2], worst[3], worst[4], worst[5]));
    }
    rows
}

// ---------------------------------------------------------------- selftest

fn synth(w: usize, h: usize, seed: u64) -> Vec<u8> {
    // A synthetic packed frame: a ramp with a per-phase offset, shot-like noise, a hard edge, a clipped patch and a patch of uniform noise (to hit the escapes).
    let mut r = Rng(seed);
    let mut px = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let base = 64. + (x as f64 / w as f64) * 600. + ((x & 1) * 40 + (y & 1) * 25) as f64;
            let mut v = base + r.gauss() * (base - 60.).sqrt();
            if x > w / 2 && y > h / 3 {
                v += 300.;
            }
            if x < w / 8 && y < h / 4 {
                v = 1023.;
            }
            if x > w - w / 8 && y > h - h / 4 {
                v = (r.next() & 1023) as f64;
            }
            // The synthetic signal plus noise can leave the sensor range; a real ADC clips at 0 and 1023, so the generator does too.
            px[y * w + x] = v.round().clamp(0., 1023.) as u16;
        }
    }
    let stride = w * 5 / 4 + (1 << 3);
    let mut raw = vec![0u8; stride * h];
    for y in 0..h {
        for g in 0..w / 4 {
            let mut lo = 0u8;
            for k in 0..4 {
                let v = px[y * w + g * 4 + k];
                raw[y * stride + g * 5 + k] = (v >> 2) as u8;
                lo |= ((v & 3) as u8) << (2 * k);
            }
            raw[y * stride + g * 5 + 4] = lo;
        }
    }
    raw
}

fn selftest() {
    let mut work = Box::new(Work::new());
    let mut bands = Vec::new();
    let mut n = 0;
    for (w, h) in [(200usize, 37usize), (4080, 24), (1 << 13, 12)] {
        for period in [2usize, 4] {
            let raw = synth(w, h, G_SEED ^ (w * h * period) as u64);
            let f = Frame { label: "synth".into(), w, h, stride: w * 5 / 4 + (1 << 3), cfa: if period == 4 { Cfa::Quad } else { Cfa::Bayer }, raw };
            let px = unpack_frame(&f);
            let mut dst = vec![0u16; w * h];
            for band in [1usize << 2, 1 << 3, 1 << 4, h] {
                for name in VARIANTS {
                    let p = variant(name);
                    let mut out = vec![0u8; out_len(&f, band)];
                    encode_frame(&f, band, &p, None, &mut work, &mut out, &mut bands);
                    decode_frame(&f, band, &p, &mut work, &out, &bands, &mut dst);
                    assert!(px == dst, "selftest {w} x {h} period {period} band {band} {name}: differs");
                    n += 1;
                }
            }
            let mut fwd = [0u16; 1024];
            let mut inv = [0u16; 1024];
            let nc = shot_tables(BLACK, 3 << 16, 9 << 16, 1, &mut fwd, &mut inv);
            assert!(nc <= 1024 && fwd.windows(2).all(|p| p[1] >= p[0] && p[1] - p[0] <= 1));
            let codes: Vec<u16> = px.iter().map(|&x| fwd[x as usize]).collect();
            let p = Params::DEFAULT;
            let mut out = vec![0u8; out_len(&f, 1 << 3)];
            encode_frame(&f, 1 << 3, &p, Some(&fwd), &mut work, &mut out, &mut bands);
            decode_frame(&f, 1 << 3, &p, &mut work, &out, &bands, &mut dst);
            assert!(codes == dst, "selftest shot mode {w} x {h} period {period}: differs");
            n += 1;
        }
    }
    println!("selftest: {} round trips exact (Bayer and quad, widths A#200 A#4080 A#8192, partial bands, every variant, the shot-noise mode)", a(n));
}

// ---------------------------------------------------------------- bench

fn bench(args: &[String]) {
    let (w, h, stride, period, runs): (usize, usize, usize, usize, usize) = (args[1].parse().unwrap(), args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap(), args[5].parse().unwrap());
    let vname = args.get(6).map(|s| s.as_str()).unwrap_or("default");
    let band: usize = args.get(7).map(|s| s.parse().unwrap()).unwrap_or(DEFAULT_BAND);
    let dec = args.get(8).map(|s| s == "decode").unwrap_or(false);
    let f = load("bench", &args[0], w, h, stride, period);
    let p = variant(vname);
    let mut work = Box::new(Work::new());
    let mut bands = Vec::new();
    let mut out = vec![0u8; out_len(&f, band)];
    let mut dst = vec![0u16; w * h];
    let npx = (w * h) as f64;
    // Touch the output once so page faults are not timed.
    encode_frame(&f, band, &p, None, &mut work, &mut out, &mut bands);
    let mut ts = Vec::new();
    let mut ticks = Vec::new();
    let t_all = Instant::now();
    let k_all = tsc();
    for _ in 0..runs {
        let t0 = Instant::now();
        let k0 = tsc();
        if dec {
            decode_frame(&f, band, &p, &mut work, &out, &bands, &mut dst);
        } else {
            encode_frame(&f, band, &p, None, &mut work, &mut out, &mut bands);
        }
        ticks.push(tsc() - k0);
        ts.push(t0.elapsed().as_secs_f64());
    }
    let tsc_hz = (tsc() - k_all) as f64 / t_all.elapsed().as_secs_f64();
    let mut sorted = ts.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = sorted[sorted.len() / 2];
    let best = sorted[0];
    let mut tk = ticks.clone();
    tk.sort();
    let mhz = cpu_mhz();
    println!(
        "{} {} band {}: {} x {} runs, best {} ms, median {} ms, {} Mpix/s; TSC {} GHz, {} TSC ticks a pixel (median); core clock now {} MHz; coded {} bytes ({} bpp)",
        if dec { "decode" } else { "encode" },
        vname,
        a(band),
        a(runs),
        a(w * h),
        af(best * 1e3, 2),
        af(med * 1e3, 2),
        af(npx / med / 1e6, 1),
        af(tsc_hz / 1e9, 3),
        af(tk[tk.len() / 2] as f64 / npx, 2),
        mhz,
        a(coded_size(&bands)),
        af(coded_size(&bands) as f64 * 8. / npx, 3)
    );
}

/// The clock of the core this thread runs on, from /proc/cpuinfo (Linux hosts; "?" elsewhere).
fn cpu_mhz() -> String {
    #[cfg(target_os = "linux")]
    {
        let cpu = unsafe { libc_sched_getcpu() };
        if let Ok(t) = std::fs::read_to_string("/proc/cpuinfo") {
            for blk in t.split("\n\n") {
                let mut id = None;
                let mut mhz = None;
                for l in blk.lines() {
                    if l.starts_with("processor") {
                        id = l.split(':').nth(1).and_then(|s| s.trim().parse::<i32>().ok());
                    }
                    if l.starts_with("cpu MHz") {
                        mhz = l.split(':').nth(1).map(|s| s.trim().to_string());
                    }
                }
                if id == Some(cpu) {
                    return format!("A#{}", mhz.unwrap_or_default());
                }
            }
        }
    }
    "?".into()
}

#[cfg(target_os = "linux")]
unsafe extern "C" {
    #[link_name = "sched_getcpu"]
    fn libc_sched_getcpu() -> i32;
}

// ---------------------------------------------------------------- temporal

/// Zero-order entropy (bits a sample) of zigzagged residuals wrapped to 10 bits: the floor a memoryless coder of them approaches.
fn entropy(hist: &[u64; 1024]) -> f64 {
    let n: u64 = hist.iter().sum();
    hist.iter().filter(|&&c| c > 0).map(|&c| {
        let p = c as f64 / n as f64;
        -p * p.log2()
    }).sum()
}

/// How much predicting from the previous frame would give: residual entropies of spatial, temporal and mixed predictors over two Bayer frames of one scene.
fn temporal(args: &[String]) {
    let (w, h, stride): (usize, usize, usize) = (args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap());
    let fp = load("prev", &args[0], w, h, stride, 2);
    let fc = load("cur", &args[1], w, h, stride, 2);
    let (pp, pc) = (unpack_frame(&fp), unpack_frame(&fc));
    let avg4 = |p: &[u16], i: usize| -> i32 { (p[i - 2] as i32 + p[i - 2 * w] as i32 + p[i - 2 * w - 2] as i32 + p[i - 2 * w + 2] as i32) >> 2 };
    let med = |p: &[u16], i: usize| -> i32 {
        let (a, b, c) = (p[i - 2] as i32, p[i - 2 * w] as i32, p[i - 2 * w - 2] as i32);
        (a + b - c).max(a.min(b)).min(a.max(b))
    };
    let zig = |x: i32, pr: i32| -> usize {
        let e = ((x - pr + 512) & 1023) - 512;
        ((e << 1) ^ (e >> 31)) as usize
    };
    let names = ["spatial avg4", "spatial med", "temporal (previous frame's sample)", "temporal + spatial change (prev + avg4(cur) - avg4(prev))", "mean of avg4 and previous"];
    let mut hs = [[0u64; 1024]; 5];
    for y in 4..h {
        for x in 4..w - 4 {
            let i = y * w + x;
            let c = pc[i] as i32;
            let (sa, sm, t) = (avg4(&pc, i), med(&pc, i), pp[i] as i32);
            let preds = [sa, sm, t, t + sa - avg4(&pp, i), (sa + t) >> 1];
            for (hh, pr) in hs.iter_mut().zip(preds) {
                hh[zig(c, pr)] += 1;
            }
        }
    }
    println!("temporal: {} after {} ({} x {}): zero-order entropy of each predictor's residual, bits a sample", args[1], args[0], a(w), a(h));
    for (n, hh) in names.iter().zip(hs.iter()) {
        println!("  {:<60} {}", n, af(entropy(hh), 3));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("selftest") => selftest(),
        Some("report") => report(&args[2], &args[3]),
        Some("bench") => bench(&args[2..]),
        Some("temporal") => temporal(&args[2..]),
        Some("tables") => {
            // raw-codec tables <g> <r^2> <s_log2> <from> <to>: the shot-noise transform's samples, code values and reconstructions.
            let (g, r2, sl): (f64, f64, u32) = (args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap());
            let (lo, hi): (usize, usize) = (args[5].parse().unwrap(), args[6].parse().unwrap());
            let mut fwd = [0u16; 1024];
            let mut inv = [0u16; 1024];
            let nc = shot_tables(BLACK, (g * 65536.).round() as u64, (r2 * 65536.).round() as u64, sl, &mut fwd, &mut inv);
            println!("{} codes", a(nc));
            for x in lo..=hi {
                println!("  x {}  code {}  back {}", a(x), a(fwd[x]), a(inv[fwd[x] as usize]));
            }
        }
        _ => eprintln!("usage: raw-codec selftest | report <frames list> <results.txt> | bench <file> <w> <h> <stride> <period> <runs> [variant] [band] [decode] | temporal <prev> <cur> <w> <h> <stride>"),
    }
}
