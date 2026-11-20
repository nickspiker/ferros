//! camera-pd-layout: the Pro main camera's phase-detect rows measured against the image they were read with (a SNAP pair, tools/camera-pd-pair.sh), and VSF visuals of what they hold (docs/CAMERA.md "Phase detect as data").
//!
//! Input: `<prefix>.image.raw10` (the binned frame, 4080 x 3072 at G#1400 a line) and `<prefix>.pd.raw10` (the PD path's 768 lines at the same stride), both MIPI RAW10 as the front end wrote them.
//! Printed: how a PD line is laid out, which samples are the two pupil halves, where each PD sample sits in the image (scale and offset found by correlation), and the signal (the halves' disparity across the frame, their brightness against the image's, the noise).
//! Written (next to the prefix, or in the directory given): four display visuals on the image's Bayer-cell grid (2040 x 1536, the image's own colour resolution, one sample per 2 x 2 cell), VSF RGB, gamma2tanh at 16 bits, orientation 6 as the sensor plane's:
//! - `<name>.pd-a.vsf`, `<name>.pd-b.vsf`: each pupil half;
//! - `<name>.pd-sum.vsf`: their mean;
//! - `<name>.pd-phase.vsf`: the owner's rendering, brightness the brighter of each pair (the brightest white), colour the pair's difference on one signed blue-yellow axis.
//! All four share one scale: the brightest half sample is white, so they compare by flipping between them.
//!
//! Usage: camera-pd-layout <prefix> [out dir]

use vsf::BitPackedTensor;
use vsf::colour::convert::delinearize_gamma2tanh_bits_f32;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

/// The main's binned frame and its PD rows: 4080 samples a line at G#1400 bytes, 3072 image lines, 768 PD lines.
const W: usize = 4080;
const H: usize = 3072;
const STRIDE: usize = 5120;
const PD_H: usize = 768;
/// A PD line's pairs and the image's Bayer cells across.
const PW: usize = W / 2;
/// The image's black (camera-pd-pair, the sensor plane's level); the PD rows' is measured against it below.
const BLACK: f32 = 64.;

/// RAW10 lines into 10-bit samples: per line of `stride` bytes, `w` samples, each group of four the high eight bits of each and then their low two bits in one byte (sample k at bits 2k).
fn unpack(raw: &[u8], w: usize, h: usize, stride: usize) -> Vec<u16> {
    assert!(raw.len() >= h * stride, "short: {} bytes for {h} lines of {stride}", raw.len());
    let mut px = vec![0u16; w * h];
    for y in 0..h {
        let line = &raw[y * stride..y * stride + w * 5 / 4];
        for (g, b) in line.chunks_exact(5).enumerate() {
            for k in 0..4 {
                px[y * w + g * 4 + k] = (b[k] as u16) << 2 | (b[4] as u16 >> (2 * k)) & 3;
            }
        }
    }
    px
}

/// A plane of f32 samples.
#[derive(Clone)]
struct Grid {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Grid {
    fn new(w: usize, h: usize) -> Grid {
        Grid { w, h, v: vec![0.; w * h] }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }

    /// Bilinear sample at a real position.
    /// WHY/PROOF: the matcher asks for positions shifted by every candidate offset and scale, so near the edges they fall outside the grid by construction; None drops that point from the sum instead of reading past the plane.
    fn sample(&self, x: f32, y: f32) -> Option<f32> {
        if x < 0. || y < 0. || x > (self.w - 1) as f32 || y > (self.h - 1) as f32 {
            return None;
        }
        let (x0, y0) = (x as usize, y as usize);
        // WHY/PROOF: x0 = w - 1 exactly on the last column (x == w - 1 passes the test above); its right neighbour would be one past the row, and its weight fx is 0 there, so the last column stands in for it.
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let t = self.at(x0, y0) * (1. - fx) + self.at(x1, y0) * fx;
        let b = self.at(x0, y1) * (1. - fx) + self.at(x1, y1) * fx;
        Some(t * (1. - fy) + b * fy)
    }

    /// The mean over a (2rx + 1) x (2ry + 1) box at every sample, by an integral image.
    /// WHY/PROOF: at the edges the box would reach past the plane; it is cut to the part inside (saturating_sub and min) and divided by its own area, so an edge sample's mean is of real samples only.
    fn boxmean(&self, rx: usize, ry: usize) -> Grid {
        let (w, h) = (self.w, self.h);
        let mut ii = vec![0f64; (w + 1) * (h + 1)];
        for y in 0..h {
            for x in 0..w {
                ii[(y + 1) * (w + 1) + x + 1] = self.at(x, y) as f64 + ii[y * (w + 1) + x + 1] + ii[(y + 1) * (w + 1) + x] - ii[y * (w + 1) + x];
            }
        }
        let mut o = Grid::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (xa, ya) = (x.saturating_sub(rx), y.saturating_sub(ry));
                let (xb, yb) = ((x + rx + 1).min(w), (y + ry + 1).min(h));
                let s = ii[yb * (w + 1) + xb] - ii[ya * (w + 1) + xb] - ii[yb * (w + 1) + xa] + ii[ya * (w + 1) + xa];
                o.v[y * w + x] = (s / ((xb - xa) * (yb - ya)) as f64) as f32;
            }
        }
        o
    }

    /// A band-pass: the 3 x 3 mean less the (2r + 1)^2 mean, so the noise and the shading both fall away and the edges stay.
    fn bandpass(&self, r: usize) -> Grid {
        let fine = self.boxmean(1, 1);
        let coarse = self.boxmean(r, r);
        Grid { w: self.w, h: self.h, v: fine.v.iter().zip(&coarse.v).map(|(a, b)| a - b).collect() }
    }
}

/// Normalised cross-correlation of `a` at its points (x, y) against `b` sampled at (sx * x + ox, sy * y + oy).
fn ncc(a: &Grid, b: &Grid, xs: &[usize], ys: &[usize], sx: f32, ox: f32, sy: f32, oy: f32) -> f32 {
    let (mut sab, mut saa, mut sbb, mut sa, mut sb, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for &y in ys {
        for &x in xs {
            if let Some(bv) = b.sample(sx * x as f32 + ox, sy * y as f32 + oy) {
                let (av, bv) = (a.at(x, y) as f64, bv as f64);
                sab += av * bv;
                saa += av * av;
                sbb += bv * bv;
                sa += av;
                sb += bv;
                n += 1.;
            }
        }
    }
    let cov = sab - sa * sb / n;
    let va = saa - sa * sa / n;
    let vb = sbb - sb * sb / n;
    (cov / (va * vb).sqrt()) as f32
}

/// A block's best match: its correlation, the offset found and how much texture it had (the band-passed variance of `a`).
#[derive(Clone, Copy, Debug)]
struct Match {
    ncc: f32,
    dx: f32,
    dy: f32,
    texture: f32,
}

/// The offset of `b` against `a` over a block: integers across +-rx, +-ry around (ox, oy), then quarters within one of the best, then a parabola thru the best quarter and its neighbours on each axis.
fn best_shift(a: &Grid, b: &Grid, xs: &[usize], ys: &[usize], sx: f32, sy: f32, ox: f32, oy: f32, rx: i32, ry: i32) -> Match {
    let mut best = (f32::MIN, 0f32, 0f32);
    for j in -ry..=ry {
        for i in -rx..=rx {
            let c = ncc(a, b, xs, ys, sx, ox + i as f32, sy, oy + j as f32);
            if c > best.0 {
                best = (c, ox + i as f32, oy + j as f32);
            }
        }
    }
    let mut q = [[f32::MIN; 9]; 9];
    let mut bq = (f32::MIN, 4usize, 4usize);
    for j in 0..9 {
        for i in 0..9 {
            let c = ncc(a, b, xs, ys, sx, best.1 + (i as f32 - 4.) / 4., sy, best.2 + (j as f32 - 4.) / 4.);
            q[j][i] = c;
            if c > bq.0 {
                bq = (c, i, j);
            }
        }
    }
    let (i, j) = (bq.1, bq.2);
    // A parabola thru three correlations a quarter apart: the vertex's offset from the middle, in quarters. On the search's edge (no neighbour outside) the quarter itself stands.
    let vertex = |m: f32, c: f32, p: f32| {
        let d = m - 2. * c + p;
        if d < 0. { 0.5 * (m - p) / d } else { 0. }
    };
    let fx = if i > 0 && i < 8 { vertex(q[j][i - 1], q[j][i], q[j][i + 1]) } else { 0. };
    let fy = if j > 0 && j < 8 { vertex(q[j - 1][i], q[j][i], q[j + 1][i]) } else { 0. };
    let texture = ys.iter().flat_map(|&y| xs.iter().map(move |&x| (x, y))).map(|(x, y)| a.at(x, y).powi(2)).sum::<f32>() / (xs.len() * ys.len()) as f32;
    Match { ncc: bq.0, dx: best.1 + (i as f32 - 4. + fx) / 4., dy: best.2 + (j as f32 - 4. + fy) / 4., texture }
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Noise by the median of second differences along a row at `step` samples, robust to edges: sigma = median|v[x - s] - 2 v[x] + v[x + s]| / (0.6745 sqrt 6) for white noise.
/// The samples are whole counts, so the differences are too: the median is read from their histogram, interpolated within its count (the grouped median), or it would step by whole counts.
fn noise(g: &Grid, step: usize) -> f32 {
    let mut hist = vec![0u64; 1 << 12];
    let mut n = 0u64;
    for y in 0..g.h {
        for x in step..g.w - step {
            hist[(g.at(x - step, y) - 2. * g.at(x, y) + g.at(x + step, y)).abs().round() as usize] += 1;
            n += 1;
        }
    }
    let mut below = 0u64;
    let mut k = 0usize;
    while below + hist[k] < n / 2 {
        below += hist[k];
        k += 1;
    }
    // The bin of k holds |d| from k - 1/2 to k + 1/2 (0 holds 0 to 1/2).
    let lo = if k == 0 { 0. } else { k as f64 - 0.5 };
    let width = if k == 0 { 0.5 } else { 1. };
    let med = lo + width * (n / 2 - below) as f64 / hist[k] as f64;
    (med / (0.6745 * 6f64.sqrt())) as f32
}

/// Least squares y = c + k x.
fn line_fit(pts: &[(f32, f32)]) -> (f64, f64) {
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0 as f64).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1 as f64).sum::<f64>() / n;
    let sxy: f64 = pts.iter().map(|p| (p.0 as f64 - mx) * (p.1 as f64 - my)).sum();
    let sxx: f64 = pts.iter().map(|p| (p.0 as f64 - mx).powi(2)).sum();
    let k = sxy / sxx;
    (my - k * mx, k)
}

/// The halves and the image, read and split.
struct Pair {
    /// Even samples of each PD line (pair k's first), less black.
    a: Grid,
    /// Odd samples (pair k's second), less black.
    b: Grid,
    /// The image's two greens per Bayer cell (G B / R G), averaged, less black: 2040 x 1536.
    green: Grid,
    /// The raw lines, for the layout's checks.
    pd_raw: Vec<u8>,
    pd: Vec<u16>,
    img: Vec<u16>,
}

fn read_pair(prefix: &str) -> Pair {
    let rd = |s: &str| std::fs::read(format!("{prefix}.{s}.raw10")).unwrap_or_else(|e| panic!("{prefix}.{s}.raw10: {e}"));
    let pd_raw = rd("pd");
    let img = unpack(&rd("image"), W, H, STRIDE);
    let pd = unpack(&pd_raw, W, PD_H, STRIDE);
    let (mut a, mut b) = (Grid::new(PW, PD_H), Grid::new(PW, PD_H));
    for y in 0..PD_H {
        for k in 0..PW {
            a.v[y * PW + k] = pd[y * W + 2 * k] as f32 - BLACK;
            b.v[y * PW + k] = pd[y * W + 2 * k + 1] as f32 - BLACK;
        }
    }
    let mut green = Grid::new(PW, H / 2);
    for cy in 0..H / 2 {
        for cx in 0..PW {
            green.v[cy * PW + cx] = (img[2 * cy * W + 2 * cx] as f32 + img[(2 * cy + 1) * W + 2 * cx + 1] as f32) * 0.5 - BLACK;
        }
    }
    Pair { a, b, green, pd_raw, pd, img }
}

/// What the measurements found, for the visuals.
struct Geometry {
    /// Cell x = sx * k + ox, cell y = sy * row + oy.
    sx: f64,
    ox: f64,
    sy: f64,
    oy: f64,
}

/// 0. The line: how many bytes carry samples, the padding, the samples' parity.
fn layout(p: &Pair) {
    println!("== the PD line");
    let used = (0..PD_H).map(|y| p.pd_raw[y * STRIDE..y * STRIDE + STRIDE].iter().rposition(|&v| v != 0).map_or(0, |i| i + 1)).max().unwrap();
    let pad_nonzero = (0..PD_H).filter(|&y| p.pd_raw[y * STRIDE + W * 5 / 4..(y + 1) * STRIDE].iter().any(|&v| v != 0)).count();
    let zero_lines = (0..PD_H).filter(|&y| p.pd_raw[y * STRIDE..y * STRIDE + W * 5 / 4].iter().all(|&v| v == 0)).count();
    let (pmin, pmax) = (p.pd.iter().min().unwrap(), p.pd.iter().max().unwrap());
    println!("bytes carrying samples: A#{used} of A#{STRIDE} a line (A#{} samples of RAW10), padding nonzero in A#{pad_nonzero} lines, all-zero lines A#{zero_lines}; samples A#{pmin} to A#{pmax}", used * 4 / 5);
    // The coarse profile across the line: PD against the image's green, 16 bands, and each parity's.
    let band = |g: &dyn Fn(usize, usize) -> f32, h: usize, b: usize| (0..h).map(|y| (b * PW / 16..(b + 1) * PW / 16).map(|x| g(x, y) as f64).sum::<f64>()).sum::<f64>() / (h * PW / 16) as f64;
    print!("band means across the line (16 bands), even samples:");
    for b in 0..16 {
        print!(" {:.1}", band(&|x, y| p.a.at(x, y), PD_H, b));
    }
    print!("\n                                        odd samples: ");
    for b in 0..16 {
        print!(" {:.1}", band(&|x, y| p.b.at(x, y), PD_H, b));
    }
    print!("\n                               the image's green:    ");
    for b in 0..16 {
        print!(" {:.1}", band(&|x, y| p.green.at(x, y), H / 2, b));
    }
    println!();
    // Rows: a top/bottom pair would alternate by row (or by row pair) and shade the opposite way down the frame, as the even and odd samples shade across it. The ratio of even rows to odd rows, and of row pairs, by 12 bands down.
    for (what, period) in [("even / odd rows", 2usize), ("rows 4r, 4r+1 / 4r+2, 4r+3", 4)] {
        print!("{what} of the pair sum by 12 bands down:");
        for b in 0..12 {
            let (mut e, mut o) = (0f64, 0f64);
            for y in b * 64..(b + 1) * 64 {
                let s: f64 = (0..PW).map(|x| (p.a.at(x, y) + p.b.at(x, y)) as f64).sum();
                if y % period < period / 2 {
                    e += s;
                } else {
                    o += s;
                }
            }
            print!(" {:.3}", e / o);
        }
        println!();
    }
}

/// 2. Where a PD sample sits in the image: scale and offset by correlation of the pair's sum with the image's green.
fn geometry(p: &Pair) -> Geometry {
    println!("== the PD samples in the image (cells: the image's 2 x 2 Bayer cells, 2040 x 1536)");
    let mut s = Grid::new(PW, PD_H);
    for i in 0..PW * PD_H {
        s.v[i] = p.a.v[i] + p.b.v[i];
    }
    let (sbp, gbp) = (s.bandpass(4), p.green.bandpass(4));
    let xs: Vec<usize> = (16..PW - 16).step_by(4).collect();
    let ys: Vec<usize> = (16..PD_H - 16).step_by(4).collect();
    let mut best = (f32::MIN, 0f32, 0f32, 0f32, 0f32);
    for &sx in &[0.5f32, 1., 2.] {
        for &sy in &[0.5f32, 1., 2., 4.] {
            let mut bb = (f32::MIN, 0f32, 0f32);
            for oy in -8..=8 {
                for ox in -8..=8 {
                    let c = ncc(&sbp, &gbp, &xs, &ys, sx, ox as f32, sy, oy as f32);
                    if c > bb.0 {
                        bb = (c, ox as f32, oy as f32);
                    }
                }
            }
            println!("  cells per PD step x {sx} y {sy}: best correlation {:.3} at whole-cell offset {:+} {:+}", bb.0, bb.1, bb.2);
            if bb.0 > best.0 {
                best = (bb.0, sx, bb.1, sy, bb.2);
            }
        }
    }
    let (sx, sy) = (best.1, best.3);
    // Blocks, 8 x 6, each offset to a quarter cell and a parabola; then a line thru them per axis gives the scale's correction.
    let mut fx = Vec::new();
    let mut fy = Vec::new();
    let mut cs = Vec::new();
    for by in 0..6 {
        for bx in 0..8 {
            let bxs: Vec<usize> = (bx * 255 + 8..bx * 255 + 247).step_by(2).collect();
            let bys: Vec<usize> = (by * 128 + 8..by * 128 + 120).step_by(2).collect();
            let m = best_shift(&sbp, &gbp, &bxs, &bys, sx, sy, best.2, best.4, 1, 1);
            if m.ncc > 0.5 {
                fx.push(((bx * 255 + 127) as f32, m.dx));
                fy.push(((by * 128 + 64) as f32, m.dy));
                cs.push(m.ncc);
            }
        }
    }
    let (ox, kx) = line_fit(&fx);
    let (oy, ky) = line_fit(&fy);
    let rms = |pts: &[(f32, f32)], c: f64, k: f64| (pts.iter().map(|p| (p.1 as f64 - c - k * p.0 as f64).powi(2)).sum::<f64>() / pts.len() as f64).sqrt();
    let g = Geometry { sx: sx as f64 + kx, ox, sy: sy as f64 + ky, oy };
    println!(
        "  fit over A#{} blocks (correlation median {:.3}): cell x = {:.5} k {:+.3}, cell y = {:.5} row {:+.3} (residuals {:.3} and {:.3} cells rms)",
        fx.len(),
        median(cs),
        g.sx,
        g.ox,
        g.sy,
        g.oy,
        rms(&fx, ox, kx),
        rms(&fy, oy, ky)
    );
    // Which of the cell's colours the PD samples follow: each Bayer plane band-passed, against the pair's sum at the fitted place.
    let plane = |dy: usize, dx: usize| {
        let mut c = Grid::new(PW, H / 2);
        for cy in 0..H / 2 {
            for cx in 0..PW {
                c.v[cy * PW + cx] = p.img[(2 * cy + dy) * W + 2 * cx + dx] as f32;
            }
        }
        c.bandpass(4)
    };
    let fine: Vec<usize> = (16..PW - 16).step_by(2).collect();
    let fine_y: Vec<usize> = (16..PD_H - 16).step_by(2).collect();
    print!("  the pair's sum against each colour of the cell at that place:");
    for (name, dy, dx) in [("G (top left)", 0, 0), ("B", 0, 1), ("R", 1, 0), ("G (bottom right)", 1, 1)] {
        print!(" {name} {:.3}", ncc(&sbp, &plane(dy, dx), &fine, &fine_y, g.sx as f32, g.ox as f32, g.sy as f32, g.oy as f32));
    }
    println!();
    g
}

/// 1 and 3. Each half against the image, and the halves against each other: the disparity across the frame, and whether any of it is vertical.
fn disparity(p: &Pair, g: &Geometry) {
    println!("== the halves");
    let (abp, bbp, gbp) = (p.a.bandpass(4), p.b.bandpass(4), p.green.bandpass(4));
    // Each half against the image, globally: its own offset.
    let xs: Vec<usize> = (16..PW - 16).step_by(4).collect();
    let ys: Vec<usize> = (16..PD_H - 16).step_by(4).collect();
    let ma = best_shift(&abp, &gbp, &xs, &ys, g.sx as f32, g.sy as f32, g.ox.round() as f32, g.oy.round() as f32, 1, 1);
    let mb = best_shift(&bbp, &gbp, &xs, &ys, g.sx as f32, g.sy as f32, g.ox.round() as f32, g.oy.round() as f32, 1, 1);
    println!("  even half against the image: correlation {:.3}, offset {:+.3} {:+.3} cells; odd half: {:.3}, {:+.3} {:+.3}", ma.ncc, ma.dx, ma.dy, mb.ncc, mb.dx, mb.dy);
    println!("  so the even half sits {:+.3} cells right and {:+.3} cells down of the odd one, on the frame's whole", mb.dx - ma.dx, mb.dy - ma.dy);
    // The halves against each other, by block: 16 x 12 blocks of 120 x 56 pairs; the odd half's offset that best matches the even half's block (in PD steps: one = a cell across, two cells down).
    println!("  disparity, the odd half's offset against the even (x, then y, in PD steps; '.' a block without texture or match):");
    let mut both = Vec::new();
    for by in 0..12 {
        let mut line_x = String::new();
        let mut line_y = String::new();
        for bx in 0..16 {
            let x0 = bx * PW / 16 + 4;
            let bxs: Vec<usize> = (x0..x0 + 120).collect();
            let bys: Vec<usize> = (by * 64 + 4..by * 64 + 60).collect();
            let m = best_shift(&abp, &bbp, &bxs, &bys, 1., 1., 0., 0., 6, 2);
            if m.ncc > 0.5 && m.texture > 4. {
                line_x += &format!(" {:+5.2}", m.dx);
                line_y += &format!(" {:+5.2}", m.dy);
                both.push(m);
            } else {
                line_x += "     .";
                line_y += "     .";
            }
        }
        println!("  x{line_x}   y{line_y}");
    }
    if both.len() < 8 {
        println!("  too few textured blocks to say more (A#{})", both.len());
        return;
    }
    let dxs: Vec<f32> = both.iter().map(|m| m.dx).collect();
    let dys: Vec<f32> = both.iter().map(|m| m.dy).collect();
    let (c, k) = line_fit(&both.iter().map(|m| (m.dx, m.dy)).collect::<Vec<_>>());
    let spread = |v: &[f32]| {
        let m = median(v.to_vec());
        median(v.iter().map(|x| (x - m).abs()).collect()) * 1.4826
    };
    println!(
        "  A#{} matched blocks: x from {:+.2} to {:+.2} (median {:+.2}, spread {:.2}); y median {:+.3}, spread {:.3}; y against x: y = {:+.3} {:+.3} x",
        both.len(),
        dxs.iter().cloned().fold(f32::MAX, f32::min),
        dxs.iter().cloned().fold(f32::MIN, f32::max),
        median(dxs.clone()),
        spread(&dxs),
        median(dys.clone()),
        spread(&dys),
        c,
        k
    );
    // The same question by the difference's shape: a disparity (dx, dy) between the halves makes their difference D = A - B about dx Sx + dy Sy of the sum's gradients (A = S(x + d/2) / 2, B = S(x - d/2) / 2), so a least-squares fit per block of D = c0 + c1 S + (dx / 2) Sx + (dy / 2) Sy says how much of the difference is vertical, wherever the block has both gradients. The gradients are of the 3 x 3 mean of S, the noise's pull toward 0 the same for both.
    let mut s = Grid::new(PW, PD_H);
    for i in 0..PW * PD_H {
        s.v[i] = p.a.v[i] + p.b.v[i];
    }
    let ss = s.boxmean(1, 1);
    let mut fits = Vec::new();
    for by in 0..12 {
        for bx in 0..16 {
            let mut m = [[0f64; 4]; 4];
            let mut r = [0f64; 4];
            let (mut ex, mut ey) = (0f64, 0f64);
            for y in by * 64 + 2..by * 64 + 62 {
                for x in bx * PW / 16 + 2..(bx + 1) * PW / 16 - 2 {
                    let gx = (ss.at(x + 1, y) - ss.at(x - 1, y)) as f64 * 0.5;
                    let gy = (ss.at(x, y + 1) - ss.at(x, y - 1)) as f64 * 0.5;
                    let f = [1., ss.at(x, y) as f64, gx, gy];
                    let d = (p.a.at(x, y) - p.b.at(x, y)) as f64;
                    for u in 0..4 {
                        for v in 0..4 {
                            m[u][v] += f[u] * f[v];
                        }
                        r[u] += f[u] * d;
                    }
                    ex += gx * gx;
                    ey += gy * gy;
                }
            }
            let n = (60 * (PW / 16 - 4)) as f64;
            // Both gradients' rms above 2 counts a PD step (the sum's noise about 1.5 after the 3 x 3 mean): a block with texture both ways.
            if (ex / n).sqrt() > 2. && (ey / n).sqrt() > 2. {
                let sol = solve4(m, r);
                fits.push((2. * sol[2]) as f32);
                fits.push((2. * sol[3]) as f32);
            }
        }
    }
    let dx: Vec<f32> = fits.iter().step_by(2).cloned().collect();
    let dy: Vec<f32> = fits.iter().skip(1).step_by(2).cloned().collect();
    if dx.len() >= 8 {
        let (c, k) = line_fit(&dx.iter().cloned().zip(dy.iter().cloned()).collect::<Vec<_>>());
        println!(
            "  by the difference's shape, A#{} blocks with texture both ways: dx median {:+.2} (|dx| {:.2}), dy median {:+.3} (|dy| {:.3}); dy against dx: dy = {:+.3} {:+.3} dx",
            dx.len(),
            median(dx.clone()),
            median(dx.iter().map(|v| v.abs()).collect()),
            median(dy.clone()),
            median(dy.iter().map(|v| v.abs()).collect()),
            c,
            k
        );
    }
}

/// A 4 x 4 linear system by Gaussian elimination with partial pivoting.
fn solve4(mut m: [[f64; 4]; 4], mut r: [f64; 4]) -> [f64; 4] {
    for c in 0..4 {
        let piv = (c..4).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs())).unwrap();
        m.swap(c, piv);
        r.swap(c, piv);
        for i in c + 1..4 {
            let f = m[i][c] / m[c][c];
            for j in c..4 {
                m[i][j] -= f * m[c][j];
            }
            r[i] -= f * r[c];
        }
    }
    let mut x = [0f64; 4];
    for c in (0..4).rev() {
        x[c] = (r[c] - (c + 1..4).map(|j| m[c][j] * x[j]).sum::<f64>()) / m[c][c];
    }
    x
}

/// 4. Brightness against the image, the shading between the halves, and the noise. Returns each half's black as measured, as its offset from A#64 (even, odd).
fn signal(p: &Pair, g: &Geometry) -> (f32, f32) {
    println!("== the signal");
    // Block means of each half against the image's green at the same place: slope and intercept say gain and black.
    let (mut pa, mut pb, mut ps) = (Vec::new(), Vec::new(), Vec::new());
    for by in 0..12 {
        for bx in 0..16 {
            let (x0, y0) = (bx * PW / 16, by * 64);
            let (mut sa, mut sb, mut sg, mut n) = (0f64, 0f64, 0f64, 0f64);
            for y in y0..y0 + 64 {
                for x in x0..x0 + PW / 16 {
                    // The last pair maps a hair past the last cell (cell x = 1.00002 k): that point is left out.
                    if let Some(gv) = p.green.sample((g.sx * x as f64 + g.ox) as f32, (g.sy * y as f64 + g.oy) as f32) {
                        sa += p.a.at(x, y) as f64;
                        sb += p.b.at(x, y) as f64;
                        sg += gv as f64;
                        n += 1.;
                    }
                }
            }
            let gm = (sg / n) as f32;
            pa.push((gm, (sa / n) as f32));
            pb.push((gm, (sb / n) as f32));
            ps.push((gm, ((sa + sb) / (2. * n)) as f32));
        }
    }
    let (ca, ka) = line_fit(&pa);
    let (cb, kb) = line_fit(&pb);
    let (cs, ks) = line_fit(&ps);
    println!("  against the image's green, 192 blocks, all less A#64: even half = {ka:.4} green {ca:+.2}, odd half = {kb:.4} green {cb:+.2}, their mean = {ks:.4} green {cs:+.2}");
    println!("  so the halves' blacks are about A#{:.1} and A#{:.1} (the image's A#64), and a half carries {:.2} of a green sample's light", 64. + ca, 64. + cb, ks);
    let (ca, cb) = (ca as f32, cb as f32);
    // Each half against the other by band, each less its own black: the shading of the pupil halves.
    let ratio = |xr: std::ops::Range<usize>, yr: std::ops::Range<usize>| {
        let (mut sa, mut sb) = (0f64, 0f64);
        for y in yr {
            for x in xr.clone() {
                sa += (p.a.at(x, y) - ca) as f64;
                sb += (p.b.at(x, y) - cb) as f64;
            }
        }
        sa / sb
    };
    print!("  even / odd half (each less its black) by 16 bands across:");
    for b in 0..16 {
        print!(" {:.3}", ratio(b * PW / 16..(b + 1) * PW / 16, 0..PD_H));
    }
    println!();
    print!("  even / odd half (each less its black) by 12 bands down:   ");
    for b in 0..12 {
        print!(" {:.3}", ratio(0..PW, b * 64..(b + 1) * 64));
    }
    println!();
    // Noise: second differences along a row; the image's green at a cell's step (one green a cell in each row).
    let mut img_g0 = Grid::new(PW, H / 2);
    for cy in 0..H / 2 {
        for cx in 0..PW {
            img_g0.v[cy * PW + cx] = p.img[2 * cy * W + 2 * cx] as f32 - BLACK;
        }
    }
    let mut d = Grid::new(PW, PD_H);
    for i in 0..PW * PD_H {
        d.v[i] = p.a.v[i] - p.b.v[i];
    }
    let mean = |g: &Grid| g.v.iter().map(|&v| v as f64).sum::<f64>() / g.v.len() as f64;
    let (na, nb, nd, ng) = (noise(&p.a, 1), noise(&p.b, 1), noise(&d, 1), noise(&img_g0, 1));
    println!(
        "  noise (counts, sigma): even half {na:.2} at mean {:.1}, odd half {nb:.2} at {:.1}, their difference {nd:.2}; the image's green {ng:.2} at {:.1} (one green a cell, the same step)",
        mean(&p.a),
        mean(&p.b),
        mean(&img_g0)
    );
    (ca, cb)
}

/// The smooth gain between the halves (even / odd over a 129 x 65 box), so the colour shows disparity and not the pupils' shading.
fn shading(a: &Grid, b: &Grid) -> Grid {
    let (ma, mb) = (a.boxmean(64, 32), b.boxmean(64, 32));
    Grid { w: a.w, h: a.h, v: ma.v.iter().zip(&mb.v).map(|(x, y)| x / y).collect() }
}

/// A PD-grid plane brought onto the image's cell grid (2040 x 1536) by the measured mapping, bilinear.
fn to_cells(src: &Grid, g: &Geometry) -> Grid {
    let mut o = Grid::new(PW, H / 2);
    for cy in 0..H / 2 {
        // WHY/PROOF: the mapping puts a PD row's centre half a cell below its first cell row, so the cell grid's first and last rows map a fraction outside the PD rows; the position is held to the PD grid's edge there, the edge row standing in.
        let y = ((cy as f64 - g.oy) / g.sy).clamp(0., (PD_H - 1) as f64) as f32;
        for cx in 0..PW {
            let x = ((cx as f64 - g.ox) / g.sx).clamp(0., (PW - 1) as f64) as f32;
            o.v[cy * PW + cx] = src.sample(x, y).unwrap();
        }
    }
    o
}

/// One display plane: linear VSF RGB, three planar channels, as gamma2tanh at 16 bits.
fn write_visual(path: &std::path::Path, rgb: [&Grid; 3], title: &str, text: &str) {
    let (w, h) = (rgb[0].w, rgb[0].h);
    let mut samples = vec![0u16; 3 * w * h];
    for (c, g) in rgb.iter().enumerate() {
        for i in 0..w * h {
            samples[c * w * h + i] = delinearize_gamma2tanh_bits_f32(g.v[i], 16) as u16;
        }
    }
    let mut plane = Plane::encoded("display", "pd", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "main".into();
    // The image's own rotation (camera-pd-pair's sensor plane): a quarter turn clockwise stands it upright.
    plane.orientation = 6;
    plane.prior = None;
    let v = Visual { title: title.into(), text: text.into(), planes: vec![plane], ..Default::default() };
    let bytes = v.write().unwrap_or_else(|e| panic!("{}: the VSF would not build: {e}", path.display()));
    std::fs::write(path, &bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // Read back as a reader would: one display plane of the size written, its samples the ones packed.
    let back = Visual::read_file(path).unwrap_or_else(|e| panic!("{}: reads back badly: {e:?}", path.display()));
    let bp = back.primary_plane().unwrap_or_else(|| panic!("{}: no plane read back", path.display()));
    assert_eq!((bp.width, bp.height, bp.encoding, bp.orientation), (w, h, Encoding::Gamma2Tanh, 6), "{}", path.display());
    assert_eq!(bp.packed_samples().unwrap().unpack_u16(), samples, "{}", path.display());
    println!("  {}: {} bytes, read back whole", path.display(), bytes.len());
}

/// The phase rendering's colour: sigma in (-1, 1) along one signed axis, blue (negative) to yellow (positive), luminance kept: the VSF YCbCr Y = (R + 2G + B) / 4 of (1, 1, -3) is 0.
fn phase_rgb(v: f32, sigma: f32) -> [f32; 3] {
    let k = sigma * (1. / 3.);
    [v * (1. + k), v * (1. + k), v * (1. - 3. * k)]
}

fn visuals(p: &Pair, g: &Geometry, blacks: (f32, f32), dir: &std::path::Path, name: &str) {
    println!("== the visuals");
    // Each half less its own measured black.
    let ha = Grid { w: PW, h: PD_H, v: p.a.v.iter().map(|&v| v - blacks.0).collect() };
    let hb = Grid { w: PW, h: PD_H, v: p.b.v.iter().map(|&v| v - blacks.1).collect() };
    // One scale for all four: the brightest half sample is white.
    let peak = ha.v.iter().chain(&hb.v).cloned().fold(f32::MIN, f32::max);
    let mut sorted: Vec<f32> = ha.v.iter().zip(&hb.v).map(|(a, b)| a.max(*b)).collect();
    sorted.sort_by(|x, y| x.total_cmp(y));
    println!("  the scale: the brightest half sample, A#{peak:.0} over its black, is white (the 1 - 2^-10 quantile of the brighter half is A#{:.0})", sorted[sorted.len() - (sorted.len() >> 10)]);
    let scaled = |src: &Grid| Grid { w: src.w, h: src.h, v: src.v.iter().map(|&x| x / peak).collect() };
    let gain = shading(&ha, &hb);
    let mut mean = Grid::new(PW, PD_H);
    let mut bright = Grid::new(PW, PD_H);
    let mut t = Grid::new(PW, PD_H);
    for i in 0..PW * PD_H {
        let (a, b) = (ha.v[i], hb.v[i] * gain.v[i]);
        mean.v[i] = (ha.v[i] + hb.v[i]) * 0.5;
        bright.v[i] = ha.v[i].max(hb.v[i]);
        // At or under black (a + b at or under 0) t is noise or not a number; the brightness there is about 0, so its colour is about black whatever t is, and a NaN encodes as 0.
        t.v[i] = (a - b) / (a + b);
    }
    // The colour's scale: the median |t| at the textured places (the 2^-3 of pairs with the largest band-passed sum), so the edges' colour is strong and flat ground stays near grey.
    let sum_bp = Grid { w: PW, h: PD_H, v: ha.v.iter().zip(&hb.v).map(|(a, b)| a + b).collect() }.bandpass(4);
    let mut e: Vec<(f32, f32)> = sum_bp.v.iter().zip(&t.v).map(|(s, t)| (s.abs(), t.abs())).collect();
    e.sort_by(|x, y| y.0.total_cmp(&x.0));
    let t0 = median(e[..e.len() >> 3].iter().map(|x| x.1).collect());
    println!("  the colour's scale: |difference / sum| A#{t0:.4} (the median at the eighth with the most texture) is tanh 1 of full colour; the halves' shading divided out first (even / odd over a 129 x 65 box)");
    let a = to_cells(&scaled(&ha), g);
    let b = to_cells(&scaled(&hb), g);
    let m = to_cells(&scaled(&mean), g);
    let v = to_cells(&scaled(&bright), g);
    let tc = to_cells(&t, g);
    let (mut r, mut gg, mut bb) = (Grid::new(PW, H / 2), Grid::new(PW, H / 2), Grid::new(PW, H / 2));
    for i in 0..PW * (H / 2) {
        let c = phase_rgb(v.v[i], (tc.v[i] / t0).tanh());
        r.v[i] = c[0];
        gg.v[i] = c[1];
        bb.v[i] = c[2];
    }
    let about = "ferros payloads/camera SNAP, the main's phase-detect rows (tools/camera-pd-layout, docs/CAMERA.md \"Phase detect as data\"), on the image's Bayer-cell grid";
    write_visual(&dir.join(format!("{name}.pd-a.vsf")), [&a, &a, &a], "PD even half", &format!("{about}: the even samples of each PD line (one pupil half)"));
    write_visual(&dir.join(format!("{name}.pd-b.vsf")), [&b, &b, &b], "PD odd half", &format!("{about}: the odd samples of each PD line (the other pupil half)"));
    write_visual(&dir.join(format!("{name}.pd-sum.vsf")), [&m, &m, &m], "PD sum", &format!("{about}: the halves' mean (the whole pupil)"));
    write_visual(&dir.join(format!("{name}.pd-phase.vsf")), [&r, &gg, &bb], "PD phase", &format!("{about}: brightness the brighter half (the brightest white), colour the halves' difference over their sum (shading divided out), yellow where the even half is brighter, blue the odd"));
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        eprintln!("usage: camera-pd-layout <prefix> [out dir]   (reads <prefix>.image.raw10 and <prefix>.pd.raw10)");
        std::process::exit(2);
    }
    let prefix = &a[1];
    let path = std::path::Path::new(prefix);
    let dir = a.get(2).map(std::path::PathBuf::from).unwrap_or_else(|| path.parent().unwrap().to_path_buf());
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let p = read_pair(prefix);
    layout(&p);
    let g = geometry(&p);
    disparity(&p, &g);
    let blacks = signal(&p, &g);
    visuals(&p, &g, blacks, &dir, &name);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One RAW10 group of four: the high bytes, then the low bits of each at bits 2k.
    #[test]
    fn a_group_unpacks() {
        let mut raw = vec![0u8; 16];
        raw[..5].copy_from_slice(&[0xFF, 0x00, 0x80, 0x01, 0b11_10_01_00]);
        assert_eq!(unpack(&raw, 4, 1, 16), vec![1020, 1, 514, 7]);
    }

    /// A textured plane and a copy shifted by a known amount: the matcher finds the shift to a tenth of a step.
    #[test]
    fn a_shift_is_found() {
        let (w, h) = (160usize, 96usize);
        let f = |x: f32, y: f32| (x * 0.37).sin() * (y * 0.23).cos() * 40. + (x * 0.11 + y * 0.07).sin() * 25.;
        let mut a = Grid::new(w, h);
        let mut b = Grid::new(w, h);
        for y in 0..h {
            for x in 0..w {
                a.v[y * w + x] = f(x as f32, y as f32);
                b.v[y * w + x] = f(x as f32 + 1.6, y as f32 - 0.3);
            }
        }
        let xs: Vec<usize> = (16..w - 16).collect();
        let ys: Vec<usize> = (16..h - 16).collect();
        let m = best_shift(&a, &b, &xs, &ys, 1., 1., 0., 0., 4, 2);
        assert!((m.dx + 1.6).abs() < 0.1 && (m.dy - 0.3).abs() < 0.1, "{m:?}");
    }

    /// The phase colour keeps the luminance and runs blue to yellow.
    #[test]
    fn the_phase_axis() {
        let y = |c: [f32; 3]| (c[0] + 2. * c[1] + c[2]) / 4.;
        for s in [-1f32, -0.5, 0., 0.5, 1.] {
            assert!((y(phase_rgb(0.5, s)) - 0.5).abs() < 1e-6);
        }
        let (blue, yellow) = (phase_rgb(1., -1.), phase_rgb(1., 1.));
        assert!(blue[2] > blue[0] && yellow[0] > yellow[2] && yellow[2].abs() < 1e-6);
    }
}
