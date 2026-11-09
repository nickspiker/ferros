//! A panel's ODT (VSF RGB → panel linear RGB) and its equal-energy (E) white, from the i1 Pro's measured spectra, in VSF RGB under VSF's observer (Stockman & Sharpe 2000 10°): the Rust of tools/panel-white-e.py (owner rule: no Python), for any profile written by tools/display-profile.sh or tools/akita-profile.sh.
//! Each primary's spectrum (spotread, 10 nm steps, its `colours` reading at the DBV given) integrated against VSF RGB's own colour-matching functions (vsf's cone fundamentals through LMS2VSF_RGB, 1 nm) gives the panel's primaries in VSF RGB. From them: the gains (largest 1) that make R + G + B equal E in linear light and their code multipliers g^(1/γ) in Q15; how much of native white's light E keeps (photopic, vsf's LMS2PHOTOPIC through VSF_RGB2LMS); and the ODT, the primaries' matrix inverted and scaled so its largest row sums to 1 (its row sums are the gains).
//!
//! Usage: panel-white <profile dir> <dbv label as logged, e.g. 3175 or hbm> <panel gamma>
use vsf::colour::spectral::{LMS2PHOTOPIC, LMS2VSF_RGB, LMS_2000_10DEG_1NM, VSF_RGB2LMS};

/// A column-major vsf matrix applied to a vector.
fn mul(m: &[f32; 9], v: [f64; 3]) -> [f64; 3] {
    core::array::from_fn(|r| (0..3).map(|c| m[3 * c + r] as f64 * v[c]).sum())
}

fn det(a: [[f64; 3]; 3]) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

/// m x = rhs, Cramer's rule; m[row][col].
fn solve(m: [[f64; 3]; 3], rhs: [f64; 3]) -> [f64; 3] {
    let d = det(m);
    core::array::from_fn(|c| {
        let mut a = m;
        for r in 0..3 {
            a[r][c] = rhs[r];
        }
        det(a) / d
    })
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (dir, dbv, gamma) = (&a[1], &a[2], a[3].parse::<f64>().unwrap());
    let log = std::fs::read_to_string(format!("{dir}/spotread.log")).unwrap();
    // A reading's spectrum: its block's "Spectrum from lo to hi nm in n steps" and the line after.
    let spectrum = |label: &str| -> (f64, f64, Vec<f64>) {
        let head = format!("=== colours {label} dbv={dbv} ");
        let at = log.find(&head).unwrap_or_else(|| panic!("no colours {label} at dbv {dbv}"));
        let block = &log[at..];
        let s = block.find("Spectrum from ").unwrap();
        let line = block[s..].lines().next().unwrap();
        let w: Vec<&str> = line.split_whitespace().collect();
        let (lo, hi, n) = (w[2].parse::<f64>().unwrap(), w[4].parse::<f64>().unwrap(), w[7].parse::<f64>().unwrap());
        let vals: Vec<f64> = block[s..].lines().nth(1).unwrap().split(',').map(|v| v.trim().parse().unwrap()).collect();
        (lo, (hi - lo) / (n - 1.), vals)
    };
    let lms = LMS_2000_10DEG_1NM.data;
    let vsf_rgb = |label: &str| -> [f64; 3] {
        let (lo, step, vals) = spectrum(label);
        let mut acc = [0f64; 3];
        for nm in 390..=730usize {
            // The 10 nm reading linearly interpolated to 1 nm.
            let t = (nm as f64 - lo) / step;
            let i = (t as usize).min(vals.len() - 2);
            let v = vals[i] + (vals[i + 1] - vals[i]) * (t - i as f64);
            let k = 3 * (nm - 390);
            let c = mul(&LMS2VSF_RGB, [lms[k] as f64, lms[k + 1] as f64, lms[k + 2] as f64]);
            for j in 0..3 {
                acc[j] += v * c[j];
            }
        }
        acc
    };
    let p = [vsf_rgb("red"), vsf_rgb("green"), vsf_rgb("blue")];
    let m: [[f64; 3]; 3] = core::array::from_fn(|r| core::array::from_fn(|c| p[c][r]));
    let g = solve(m, [1.; 3]);
    let top = g.iter().cloned().fold(f64::MIN, f64::max);
    let g = g.map(|x| x / top);
    // Photopic weights on VSF RGB: LMS2PHOTOPIC · VSF_RGB2LMS.
    let photopic: [f64; 3] = core::array::from_fn(|c| (0..3).map(|k| LMS2PHOTOPIC[k] as f64 * VSF_RGB2LMS[3 * c + k] as f64).sum());
    let lum = |v: [f64; 3]| (0..3).map(|c| photopic[c] * v[c]).sum::<f64>();
    let native = lum(core::array::from_fn(|r| (0..3).map(|c| p[c][r]).sum()));
    let e = lum(core::array::from_fn(|r| (0..3).map(|c| g[c] * p[c][r]).sum()));
    println!("E white under Stockman-Sharpe 10° (dbv {dbv}): gains R {:.4} G {:.4} B {:.4}", g[0], g[1], g[2]);
    let q: Vec<String> = g.iter().map(|x| format!("{:.4} (Q15 {})", x.powf(1. / gamma), (x.powf(1. / gamma) * 32768.).round())).collect();
    println!("  code multipliers g^(1/{gamma}): {}", q.join(" "));
    println!("  E white keeps {:.1} % of native white's light (photopic)", 100. * e / native);
    // The ODT: the primaries' matrix inverted, scaled so its largest row sums to 1.
    let cols: [[f64; 3]; 3] = core::array::from_fn(|j| solve(m, core::array::from_fn(|i| if i == j { 1. } else { 0. })));
    let o: [[f64; 3]; 3] = core::array::from_fn(|i| core::array::from_fn(|j| cols[j][i]));
    let top = o.iter().map(|r| r.iter().sum::<f64>()).fold(f64::MIN, f64::max);
    println!("ODT, VSF RGB -> panel linear RGB (rows: panel R, G, B; columns: VSF R, G, B; row sums = E's gains):");
    for r in o {
        let r = r.map(|x| x / top);
        println!("  {:+.4}  {:+.4}  {:+.4}   sum {:.4}", r[0], r[1], r[2], r.iter().sum::<f64>());
    }
}
