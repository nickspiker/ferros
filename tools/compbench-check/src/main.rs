//! The compose bench's kernels (payloads/compbench/src/kernel.rs) on the host, in valhall's emulator of the words, against fluor's compose of the same frame (fluor's `under` and `noise_under`, the CPU path Photon runs): the noise and compose kernel's G2t16 pixels bit for bit, the baked codes and the present's words against the same bake on the CPU (tables.rs `bake_px`), their largest difference said.
//! Usage: compbench-check [rows to check, spread over the frame; default 2^6]

#[path = "../../../payloads/compbench/src/layout.rs"]
#[allow(dead_code)]
mod layout;
#[path = "../../../payloads/compbench/src/noise.rs"]
#[allow(dead_code)]
mod noise;
#[path = "../../../payloads/compbench/src/kernel.rs"]
#[allow(dead_code)]
mod kernel;
#[path = "../../../payloads/compbench/src/tables.rs"]
#[allow(dead_code)]
mod tables;

use fluor::canvas::{Canvas, Damage};
use fluor::paint::Clip;
use fluor::pixel::{Pixel, G2t16};
use kernel::{Out, Shape};
use layout::*;
use valhall::sim::{run, Memory};

const VA: u64 = 0x8000_0000;
const SCROLL: u32 = 37;
const OFFSET: u32 = 517;

fn put64(m: &mut Memory, at: usize, v: u64) {
    m.bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
fn get64(m: &Memory, at: usize) -> u64 {
    u64::from_le_bytes(m.bytes[at..at + 8].try_into().unwrap())
}

/// fluor's frame: the chrome, the pane under it at its offset, the noise under both (noise_under, mirrored about the axis, scrolled).
fn fluor_frame(clip: Option<(usize, usize, usize, usize)>) -> Vec<u64> {
    let mut px = vec![G2t16::EMPTY; W * H];
    for y in 0..H {
        let row = &mut px[y * W..(y + 1) * W];
        if y < CHROME_ROWS {
            for x in 0..W {
                row[x] = chrome_px(x, y);
            }
        }
        let pane: Vec<u64> = (0..W).map(|x| pane_px(x, y + OFFSET as usize)).collect();
        G2t16::under_chunk_normal(row, &pane);
    }
    let mut d = Damage::new();
    let c = clip.map(|(x0, y0, x1, y1)| Clip::new(x0, y0, x1, y1));
    fluor::paint::noise_under(&mut Canvas::new(&mut px, W, H, &mut d), AXIS, SCROLL as isize, true, c, None);
    px
}

fn main() {
    let check_rows: usize = std::env::args().nth(1).map_or(1 << 6, |s| s.parse().expect("a row count"));
    let walk = (widen_mask(fluor::theme::BG_MASK), widen_mask(fluor::theme::BG_SPECKLE), widen_base(fluor::theme::BG_BASE));
    let odt = tables::odt_scaled(false);
    let k = tables::gain(0.25);
    let light: Vec<u16> = (0..1 << 16).map(|c| tables::light_entry(k, c, valhall::f32_to_f16)).collect();
    let code: Vec<f32> = (0..CODE_STEPS + 2).map(|i| tables::code_entry(i, false)).collect();
    let mut mem = Memory { base: VA, bytes: vec![0; PHASES_AT + 4 * PHASE_BYTES] };
    for t in 0..H {
        put64(&mut mem, SEED_AT + 8 * t, seed(t, H));
    }
    for (c, &l) in light.iter().enumerate() {
        mem.bytes[LIGHT_AT + 2 * c..LIGHT_AT + 2 * c + 2].copy_from_slice(&l.to_le_bytes());
    }
    for (i, &c) in code.iter().enumerate() {
        mem.bytes[CODE_AT + 4 * i..CODE_AT + 4 * i + 4].copy_from_slice(&c.to_bits().to_le_bytes());
    }
    for y in 0..16 {
        for x in 0..16 {
            mem.bytes[BLUE_AT + 16 * y + x] = tables::BLUE[y][x];
        }
    }
    for y in 0..CHROME_ROWS {
        for x in 0..W {
            put64(&mut mem, CHROME_AT + 8 * (y * W + x), chrome_px(x, y));
        }
    }
    for r in 0..PANE_ROWS {
        for x in 0..W {
            put64(&mut mem, PANE_AT + 8 * (r * W + x), pane_px(x, r));
        }
    }
    let frame = fluor_frame(None);
    // The noise alone: fluor's noise_under into an empty frame.
    let mut bare = vec![G2t16::EMPTY; W * H];
    let mut d = Damage::new();
    fluor::paint::noise_under(&mut Canvas::new(&mut bare, W, H, &mut d), AXIS, SCROLL as isize, true, None, None);
    let f16 = |h: u16| valhall::f16_to_f32(h);
    // The rows checked: spread over the frame, the chrome's edge and its shadow among them.
    let rows: Vec<usize> = (0..check_rows).map(|i| (i * H / check_rows + i * 7) % H).chain([0, CHROME_ROWS - 1, CHROME_ROWS, H - 1]).collect();
    let par_only = std::env::args().nth(2).is_some_and(|a| a == "par");
    if par_only {
        par(&mut mem, &rows, walk, &odt, &light, &code, &bare);
        return;
    }
    for (name, shape) in [
        ("noise", Shape { out: Out::G2t, layers: false, clip: false }),
        ("compose", Shape { out: Out::G2t, layers: true, clip: false }),
        ("baked", Shape { out: Out::Baked, layers: true, clip: false }),
        ("phases", Shape { out: Out::Phases, layers: true, clip: false }),
    ] {
        let k = kernel::record(shape);
        let mut u = [0u64; 64];
        let n = tables::uniforms(&mut u, &tables::Run::frame(), VA, OUT_AT, SCROLL, OFFSET, walk, &odt, &k.consts);
        let t0 = std::time::Instant::now();
        let mut steps = 0u64;
        for &y in &rows {
            for side in 0..2u32 {
                steps += run(&k.words, &u[..n], [side * H as u32 + y as u32, 0], &mut mem);
            }
        }
        let (mut wrong, mut most, mut first) = (0usize, 0u32, None);
        for &y in &rows {
            for x in 0..W {
                let i = y * W + x;
                let (got, want) = match shape.out {
                    Out::G2t => (get64(&mem, OUT_AT + 8 * i), if shape.layers { frame[i] } else { bare[i] }),
                    Out::Baked => (get64(&mem, OUT_AT + 8 * i), tables::baked(tables::bake_px(frame[i], &light, &code, &odt, f16))),
                    Out::Phases => {
                        let c = tables::bake_px(frame[i], &light, &code, &odt, f16);
                        let mut far = 0;
                        for p in 0..4 {
                            let at = PHASES_AT + p * PHASE_BYTES + 4 * i;
                            let got = u32::from_le_bytes(mem.bytes[at..at + 4].try_into().unwrap());
                            let want = tables::word(c, (tables::BLUE[y & 15][x & 15] ^ tables::TURN[p]) as u32);
                            for (sh, bits) in [(4u32, 8u32), (14, 8), (24, 8)] {
                                let (g, w) = ((got >> sh) & ((1 << bits) - 1), (want >> sh) & ((1 << bits) - 1));
                                far = far.max(g.abs_diff(w));
                            }
                            if got & 3 != 3 {
                                far = far.max(1 << 8);
                            }
                        }
                        (far as u64, 0)
                    }
                };
                let diff = match shape.out {
                    Out::G2t => (got != want) as u32,
                    Out::Baked => (0..3).map(|c| ((got >> (16 * c)) & 0xFFFF).abs_diff((want >> (16 * c)) & 0xFFFF) as u32).max().unwrap().max(((got >> 48) != (want >> 48)) as u32 * 65536),
                    Out::Phases => got as u32,
                };
                if diff != 0 {
                    wrong += 1;
                    most = most.max(diff);
                    first.get_or_insert((x, y, got, want));
                }
            }
        }
        println!(
            "{name}: {} words ({} a pass), {} rows emulated ({} instructions, {:.1} s): {wrong} of {} pixels differ, the most by {most}{}",
            k.words.len(),
            k.loop_words,
            rows.len(),
            steps,
            t0.elapsed().as_secs_f32(),
            rows.len() * W,
            first.map_or(String::new(), |(x, y, g, w)| format!(" (the first at {x},{y}: {g:016X} against {w:016X})"))
        );
    }
    // A damage rect thru the clip kernel: only its pixels written, each the whole frame's there (the noise's texture does not depend on the clip).
    let k = kernel::record(Shape { out: Out::Baked, layers: true, clip: true });
    for (x0, y0, x1, y1) in [(1001usize, 1500usize, 1005usize, 1564usize), (300, 200, 1000, 213), (20, 2900, 600, 2992)] {
        for i in 0..W * H {
            put64(&mut mem, OUT_AT + 8 * i, 0x5555_5555_5555_5555);
        }
        let r = tables::Run::rect(x0, y0, x1, y1);
        let mut u = [0u64; 64];
        let n = tables::uniforms(&mut u, &r, VA, OUT_AT, SCROLL, OFFSET, walk, &odt, &k.consts);
        for g in 0..r.groups() * 16 {
            run(&k.words, &u[..n], [g, 0], &mut mem);
        }
        let (mut inside_off, mut outside_touched, mut most) = (0usize, 0usize, 0u32);
        for y in 0..H {
            for x in 0..W {
                let i = y * W + x;
                let got = get64(&mem, OUT_AT + 8 * i);
                if x >= x0 && x < x1 && y >= y0 && y < y1 {
                    let want = tables::baked(tables::bake_px(frame[i], &light, &code, &odt, f16));
                    let d = (0..3).map(|c| ((got >> (16 * c)) & 0xFFFF).abs_diff((want >> (16 * c)) & 0xFFFF) as u32).max().unwrap();
                    if d != 0 || got >> 48 != 2 {
                        inside_off += 1;
                        most = most.max(d);
                    }
                } else if got != 0x5555_5555_5555_5555 {
                    outside_touched += 1;
                }
            }
        }
        println!("clip {x0},{y0} to {x1},{y1} ({} groups): {inside_off} of {} inside off the CPU's bake (the most by {most}), {outside_touched} outside written", r.groups(), (x1 - x0) * (y1 - y0));
    }
}

/// The parallel noise (noise.rs) as the CPU makes it, the whole frame: each half-row by `reference_row`, mirrored about the axis.
fn par_noise_frame(walk: (u64, u64, u64)) -> Vec<u64> {
    let mut px = vec![0u64; W * H];
    let mut half = vec![0u64; AXIS];
    for y in 0..H {
        let t = ((y as isize - SCROLL as isize).rem_euclid(H as isize)) as u32;
        noise::reference_row(t, false, W - AXIS, walk.0, walk.1, walk.2, 0, &mut half);
        px[y * W + AXIS..(y + 1) * W].copy_from_slice(&half[..W - AXIS]);
        noise::reference_row(t, true, AXIS, walk.0, walk.1, walk.2, 0, &mut half);
        for k in 0..AXIS {
            px[y * W + AXIS - 1 - k] = half[k];
        }
    }
    px
}

/// Each channel's visible 16-bit value of an opaque G2t16 (R, G, B).
fn vis(p: u64) -> [i64; 3] {
    [32u32, 16, 0].map(|s| (0xFFFF - ((p >> s) & 0xFFFF)) as i64)
}

/// The Photon session's measures of a noise frame (its cpu_bench.rs `stats`): each channel's mean and spread, the mean step |v(x + d) − v(x)| on one side of the axis at d = 1, 16, 64, 256, the vertical neighbours' correlation (R) and the channels' (R·G, G·B).
fn stats(px: &[u64]) -> Vec<f64> {
    let n = px.len() as f64;
    let (mut sum, mut sq) = ([0f64; 3], [0f64; 3]);
    for &p in px {
        let v = vis(p);
        for c in 0..3 {
            sum[c] += v[c] as f64;
            sq[c] += (v[c] * v[c]) as f64;
        }
    }
    let mean = sum.map(|s| s / n);
    let mut out: Vec<f64> = mean.to_vec();
    out.extend((0..3).map(|c| (sq[c] / n - mean[c] * mean[c]).sqrt()));
    for d in [1usize, 16, 64, 256] {
        let (mut acc, mut cnt) = ([0f64; 3], 0f64);
        for y in 0..H {
            let row = &px[y * W..(y + 1) * W];
            for (lo, hi) in [(0, AXIS), (AXIS, W)] {
                for x in lo..hi - d {
                    let (a, b) = (vis(row[x]), vis(row[x + d]));
                    for c in 0..3 {
                        acc[c] += (a[c] - b[c]).abs() as f64;
                    }
                    cnt += 1.;
                }
            }
        }
        out.extend(acc.map(|a| a / cnt));
    }
    let corr = |f: &dyn Fn(usize) -> (f64, f64), m: usize| {
        let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0., 0., 0., 0., 0.);
        for i in 0..m {
            let (x, y) = f(i);
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
        }
        let m = m as f64;
        (sxy / m - sx / m * sy / m) / ((sxx / m - (sx / m).powi(2)).sqrt() * (syy / m - (sy / m).powi(2)).sqrt())
    };
    out.push(corr(&|i| (vis(px[i])[0] as f64, vis(px[i + W])[0] as f64), px.len() - W));
    out.push(corr(&|i| (vis(px[i])[0] as f64, vis(px[i])[1] as f64), px.len()));
    out.push(corr(&|i| (vis(px[i])[1] as f64, vis(px[i])[2] as f64), px.len()));
    out
}

/// The parallel kernels (kernel.rs `record_par`) in the emulator, a workgroup a half-row with its shared storage and barriers: the noise against `noise::reference_row` bit for bit, the compose against fluor's `under` over the same noise, the bake and the phases as the serial kernels' checks; then the statistics of the parallel noise against fluor's `noise_under` over the whole frame.
#[allow(clippy::too_many_arguments)]
fn par(mem: &mut Memory, rows: &[usize], walk: (u64, u64, u64), odt: &[f32; 9], light: &[u16], code: &[f32], bare: &[u64]) {
    use fluor::pixel::{Blend, BlendMode};
    use valhall::sim::run_group;
    let f16 = |h: u16| valhall::f16_to_f32(h);
    let noise_ref = par_noise_frame(walk);
    // fluor's layers over the parallel noise: the chrome, the pane under it, the noise under both (an empty pixel takes the noise as it is, noise.rs `put`).
    let mut comp = vec![0u64; W * H];
    for y in 0..H {
        let row = &mut comp[y * W..(y + 1) * W];
        if y < CHROME_ROWS {
            for x in 0..W {
                row[x] = chrome_px(x, y);
            }
        }
        let pane: Vec<u64> = (0..W).map(|x| pane_px(x, y + OFFSET as usize)).collect();
        G2t16::under_chunk_normal(row, &pane);
        for x in 0..W {
            let nz = noise_ref[y * W + x];
            row[x] = if row[x] == G2t16::EMPTY { nz } else { row[x].under(nz, BlendMode::Normal) };
        }
    }
    let run_rows = |k: &valhall::ssa::Compiled, run: &tables::Run, rows: &[usize], mem: &mut Memory| -> u64 {
        let mut u = [0u64; 64];
        let n = tables::uniforms(&mut u, &run.par(), VA, OUT_AT, SCROLL, OFFSET, walk, odt, &k.consts);
        let mut steps = 0;
        for &y in rows {
            for side in 0..run.sides {
                let g = side * run.rows + (y as u32 - run.y0);
                steps += run_group(&k.words, &u[..n], g << k.wg_log2, 0, 1 << k.wg_log2, k.shared_bytes as usize, mem);
            }
        }
        steps
    };
    let shapes: [(&str, Shape, u32); 7] = [
        ("noise 2^6", Shape { out: Out::G2t, layers: false, clip: false }, 6),
        ("noise 2^7", Shape { out: Out::G2t, layers: false, clip: false }, 7),
        ("noise 2^8", Shape { out: Out::G2t, layers: false, clip: false }, 8),
        ("compose", Shape { out: Out::G2t, layers: true, clip: false }, kernel::PAR),
        ("baked", Shape { out: Out::Baked, layers: true, clip: false }, kernel::PAR),
        ("phases", Shape { out: Out::Phases, layers: true, clip: false }, kernel::PAR),
        ("clip", Shape { out: Out::Baked, layers: true, clip: true }, kernel::PAR),
    ];
    for (name, shape, wg) in shapes {
        let k = kernel::record_par(shape, wg);
        let (run, check): (tables::Run, Vec<usize>) = if shape.clip { (tables::Run::rect(300, 200, 1000, 213), (200..213).collect()) } else { (tables::Run::frame(), rows.to_vec()) };
        if shape.clip {
            for i in 0..W * H {
                put64(mem, OUT_AT + 8 * i, 0x5555_5555_5555_5555);
            }
        }
        let t0 = std::time::Instant::now();
        let steps = run_rows(&k, &run, &check, mem);
        let (mut wrong, mut most, mut first, mut outside) = (0usize, 0u32, None, 0usize);
        for &y in &check {
            for x in 0..W {
                let i = y * W + x;
                let inside = !shape.clip || (x >= run.x0 as usize && x < run.x1 as usize);
                if !inside {
                    outside += (get64(mem, OUT_AT + 8 * i) != 0x5555_5555_5555_5555) as usize;
                    continue;
                }
                let want_px = if shape.layers { comp[i] } else { noise_ref[i] };
                let diff = match shape.out {
                    Out::G2t => (get64(mem, OUT_AT + 8 * i) != want_px) as u32,
                    Out::Baked => {
                        let (got, want) = (get64(mem, OUT_AT + 8 * i), tables::baked(tables::bake_px(want_px, light, code, odt, f16)));
                        (0..3).map(|c| ((got >> (16 * c)) & 0xFFFF).abs_diff((want >> (16 * c)) & 0xFFFF) as u32).max().unwrap().max(((got >> 48) != (want >> 48)) as u32 * 65536)
                    }
                    Out::Phases => {
                        let c = tables::bake_px(want_px, light, code, odt, f16);
                        let mut far = 0;
                        for p in 0..4 {
                            let at = PHASES_AT + p * PHASE_BYTES + 4 * i;
                            let got = u32::from_le_bytes(mem.bytes[at..at + 4].try_into().unwrap());
                            let want = tables::word(c, (tables::BLUE[y & 15][x & 15] ^ tables::TURN[p]) as u32);
                            for sh in [4u32, 14, 24] {
                                far = far.max(((got >> sh) & 0xFF).abs_diff((want >> sh) & 0xFF));
                            }
                        }
                        far
                    }
                };
                if diff != 0 {
                    wrong += 1;
                    most = most.max(diff);
                    first.get_or_insert((x, y));
                }
            }
        }
        println!(
            "par {name}: workgroup 2^{}, {} words, {} B shared, {} rows emulated ({} instructions, {:.1} s): {wrong} of {} pixels differ, the most by {most}{}{}",
            k.wg_log2,
            k.words.len(),
            k.shared_bytes,
            check.len(),
            steps,
            t0.elapsed().as_secs_f32(),
            check.len() * if shape.clip { (run.x1 - run.x0) as usize } else { W },
            first.map_or(String::new(), |(x, y)| format!(" (the first at {x},{y})")),
            if shape.clip { format!(", {outside} outside written") } else { String::new() }
        );
    }
    // How it looks against fluor's: the same measures over the whole frame, and how far apart (relative, the correlations as a difference).
    let (a, b) = (stats(bare), stats(&noise_ref));
    let names = ["mean R", "mean G", "mean B", "sd R", "sd G", "sd B", "|d1| R", "|d1| G", "|d1| B", "|d16| R", "|d16| G", "|d16| B", "|d64| R", "|d64| G", "|d64| B", "|d256| R", "|d256| G", "|d256| B", "corr vert R", "corr R.G", "corr G.B"];
    println!("statistics of the visible 16-bit value, fluor's noise_under against the parallel noise (the GPU's, bit for bit on the rows above):");
    for (j, name) in names.iter().enumerate() {
        let rel = if j < 18 { format!("{:+.2} %", (b[j] - a[j]) / a[j] * 100.) } else { format!("{:+.4}", b[j] - a[j]) };
        println!("  {name:>12}: {:10.4} {:10.4}  {rel}", a[j], b[j]);
    }
}
