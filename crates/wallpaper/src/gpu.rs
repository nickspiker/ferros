//! The wallpaper on ferros's GPU (feature `gpu`; GPU-KISTS.md plan step 0; owner, 2026-11-11: "We should be using GPU path for the brot in all cases and never 'falling back' to the CPU").
//! The same picture code as the CPU's (`orbit`, `Frame::pixel`, `Frame::refined`, `colour`), run once with valhall's recording lane type: `Lane` for `valhall::ssa::F`, whose arithmetic records a graph that valhall compiles to Mali-G715 words (tapuwae's route for the attest mark).
//! One invocation is one pixel, or one preview pixel for the survey; the orbit's loop leaves for it alone.
//!
//! What the GPU does differently, and why it looks the same (owner, 2026-11-11: "if it looks roughly similar, that's totally fine ... Just needs to look the same"): divides and square roots are the special function unit's (within an ulp), log₂ its own four-instruction sequence and ln that times ln 2, multiplies fused into adds, sin 2πt a polynomial (libm's on the CPU).
//! The tests run the compiled words in valhall's emulator and measure the difference from the CPU's picture in a perceptual space.
//!
//! f32 throughout. Where f16 would do (tests `f16_where_it_breaks`): the colour stage, once the orbit has given its count, distance and directions, looks the same in f16 when the distance is taken in spans of the frame before it is narrowed; the orbit itself cannot (z must resolve the set's detail at the frame's step, 2^-11 of a unit at best in f16), nor the distance in plane units (2^-24 and below near the set: f16's smallest normal is 2^-14). The recorder has no f16 type yet; the 2× it would buy is on the paired (v2f16) colour stage, a few hundred instructions a sample against the orbit's tens a step times its steps.
//!
//! The kernels' uniforms: 0 the output's address (and each kernel's own), then the recorder's constants from the slot after them (`Compiled::consts`).

use crate::lane::{Lane, Mask};
use crate::{orbit, preview, Frame, Params, NEAR_VIEW};
use valhall::ssa::{self, F, M, U};
/// What a kernel compiles to (valhall's), for the caller that runs it.
pub use valhall::ssa::Compiled;

impl Mask for M {
    /// Recorded code runs every block `any()` guards: the blocks are select-guarded, so the values are the same.
    fn any(self) -> bool {
        true
    }
}

impl Lane for F {
    type M = M;
    fn splat(v: f32) -> F {
        F::splat(v)
    }
    fn sqrt(self) -> F {
        F::sqrt(self)
    }
    // The special function unit's log₂ (Mesa's sequence, within an ulp on the device: gputest 12).
    fn log2(self) -> F {
        F::log2_hw(self)
    }
    fn ln(self) -> F {
        F::log2_hw(self) * core::f32::consts::LN_2
    }
    /// sin 2πt: t less its nearest whole number (r in [−½, ½)), folded into [−¼, ¼] by sin(π − x) = sin x, then sin x by its Taylor series to x¹¹ on [−π/2, π/2] (within 6e-8 of sin x there, below f32's own rounding of most of its values).
    fn sin_turns(self) -> F {
        let zero = F::splat(0.);
        let r = self - (self + 0.5).floor();
        let edge = F::select(r.gt(zero), F::splat(0.5), F::splat(-0.5));
        let b = F::select(r.abs().gt(F::splat(0.25)), edge - r, r);
        let x = b * core::f32::consts::TAU;
        let x2 = x * x;
        // 1/3!, 1/5!, … 1/11! with their signs, Horner's rule from the highest.
        let mut p = F::splat(-1. / 39_916_800.);
        for c in [1. / 362_880., -1. / 5040., 1. / 120., -1. / 6.] {
            p = p * x2 + c;
        }
        x * (p * x2 + 1.)
    }
    fn gt(self, o: F) -> M {
        F::gt(self, o)
    }
    fn ge(self, o: F) -> M {
        F::ge(self, o)
    }
    fn lt(self, o: F) -> M {
        F::lt(self, o)
    }
    fn select(m: M, a: F, b: F) -> F {
        F::select(m, a, b)
    }
    fn iterate<S: Copy>(max: u32, init: S, head: impl FnMut(F, S) -> (S, M), tail: impl FnMut(F, S) -> S) -> S {
        ssa::iterate(max, init, head, tail)
    }
}

/// Column and row of pixel `i` of a `w`-wide frame, as floats (exact: every index is below 2^24). The row from (i + ½) / w by a reciprocal, which can land a hair either side of a whole number at a row's ends, then put right by the column it leaves: one row on or back where that column falls outside 0 to w − 1.
fn column_row(i: F, w: usize) -> (F, F) {
    let wf = w as f32;
    let y = ((i + 0.5) * (1. / wf)).floor();
    let x = i - y * wf;
    let one = F::splat(1.);
    let zero = F::splat(0.);
    let y = y + F::select(x.ge(F::splat(wf)), one, zero) - F::select(x.lt(zero), one, zero);
    (i - y * wf, y)
}

/// A pixel's working value, as `crate::pack` packs it: each code × 16384 toward zero, R | G << 16 to the low word, B | α G#FFFF << 16 to the high, at the frame in uniform 0, pixel `idx`.
fn store_pixel(idx: U, c: [F; 3]) {
    let q = |v: F| (v * 16384.).to_u32();
    ssa::store_at(0, idx, 3, 0, q(c[0]) | q(c[1]) << 16);
    ssa::store_at(0, idx, 3, 4, q(c[2]) | U::splat(0xFFFF << 16));
}

/// The survey of candidate `p` (`crate::survey_with`, on the GPU): invocation g is preview pixel g (`crate::preview`'s frame, row by row), its smooth count and distance as two f32 to uniform 0 + 8g, which the caller reads back and `tally`s.
/// Run it with ⌈preview pixels / 16⌉ workgroups of 16: those past the last pixel write up to 15 entries past the frame's, which the caller leaves room for.
pub fn survey_kernel(p: &Params) -> Compiled {
    let q = preview(p);
    let f = Frame::new(&q);
    ssa::begin();
    let (x, y) = column_row(F::from_u32(ssa::gid()), q.width);
    let (zx, zy) = f.z_at(NEAR_VIEW, x + 0.5 - f.half[0], y + 0.5 - f.half[1]);
    let o = orbit(zx, zy, p.c[0], p.c[1]);
    ssa::store(0, 3, 0, o.n.to_bits());
    ssa::store(0, 3, 4, o.d.to_bits());
    ssa::compile()
}

/// The first pass (`crate::render_rows`, on the GPU): invocation g draws pixel b + g, b the low word of uniform 1, one sample at its centre (`Frame::pixel`), its working value into the frame at uniform 0. One compile serves every band of the frame (b each band's first pixel), so no job runs long.
/// Run it with ⌈band / 16⌉ workgroups of 16; past the frame's last pixel up to 15 more are written, which the caller leaves room for.
pub fn render_kernel(p: &Params) -> Compiled {
    let f = Frame::new(p);
    ssa::begin();
    ssa::reserve(2);
    let idx = ssa::gid() + ssa::uniform(1, 0);
    let (x, y) = column_row(F::from_u32(idx), p.width);
    store_pixel(idx, f.pixel(x + 0.5 - f.half[0], y + 0.5 - f.half[1]));
    ssa::compile()
}

/// The refining pass (`crate::refine_rows`, on the GPU): invocation g refines the pixel whose index is word g of the list at uniform 1 (the marked pixels, `crate::mark_rows`), as the mean of its 4 × 4 samples (`Frame::refined`), into the frame at uniform 0 in place.
/// The marks are made before, from the first pass, so no invocation reads what another writes. Run it with the list's length / 16 workgroups of 16: the caller pads the list with its last index (drawn twice, the same value).
pub fn refine_kernel(p: &Params) -> Compiled {
    let f = Frame::new(p);
    ssa::begin();
    ssa::reserve(2);
    let idx = ssa::load(1, ssa::gid(), 2, 0);
    let (x, y) = column_row(F::from_u32(idx), p.width);
    store_pixel(idx, f.refined(x, y));
    ssa::compile()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mark_rows, mask_stride, params, refine_rows, render_rows, seed_of, tally, Knobs, PREVIEW};
    use valhall::sim::{run, Memory};

    /// The recorder is one global (the device compiles on one thread at a time), so the tests that record take turns.
    static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    /// Where the emulated memory sits (the layer arena's GPU address).
    const VA: u64 = 0x8000_0000;

    /// `n` invocations of `k` (uniform 0 the memory's start, then `extra`, then its constants) in the emulator, over memory of `bytes`, on every host core: each thread its own copy, its invocations' 8-byte entries (at `entry(g)`) taken back from it.
    fn emulate(k: &Compiled, extra: &[u64], n: usize, bytes: Vec<u8>, entry: impl Fn(usize) -> usize + Sync) -> Vec<u8> {
        let mut uniforms = vec![VA];
        uniforms.extend_from_slice(extra);
        uniforms.extend_from_slice(&k.consts);
        let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(4);
        let per = n.div_ceil(threads);
        let parts: Vec<(usize, Memory)> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..threads)
                .map(|t| {
                    let (words, uniforms, bytes) = (&k.words, &uniforms, bytes.clone());
                    sc.spawn(move || {
                        let mut mem = Memory { base: VA, bytes };
                        for g in t * per..((t + 1) * per).min(n) {
                            run(words, uniforms, [g as u32, 0], &mut mem);
                        }
                        (t, mem)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut out = bytes;
        for (t, mem) in parts {
            for g in t * per..((t + 1) * per).min(n) {
                let o = entry(g);
                out[o..o + 8].copy_from_slice(&mem.bytes[o..o + 8]);
            }
        }
        out
    }

    /// A working value's colour in a perceptual space: its light through vsf's cone matrix, the cones' square roots, then the crate's opponent axes (J the photopic lightness, IPT's red-green and yellow-blue), × 100, where equal steps look about equally far apart; ΔE is the distance there (about 1 at a just noticeable difference).
    fn perceive(v: u64) -> [f32; 3] {
        let c = |k: u32| ((v >> (16 * k)) & 0xFFFF) as f32 / 16384.;
        let light = [c(0) * c(0), c(1) * c(1), c(2) * c(2)];
        let lms = crate::cols(&vsf::colour::VSF_RGB2LMS, light);
        let o = crate::rows(&crate::OPP, [lms[0].max(0.).sqrt(), lms[1].max(0.).sqrt(), lms[2].max(0.).sqrt()]);
        [o[0] * 100., o[1] * 100., o[2] * 100.]
    }

    /// ΔE between two frames, pixel by pixel: the mean, the 99th percentile, the most, and the share above 1 and above 4.
    fn delta_e(a: &[u64], b: &[u64]) -> (f32, f32, f32, f32, f32) {
        let mut d: Vec<f32> = a
            .iter()
            .zip(b)
            .map(|(&x, &y)| {
                let (p, q) = (perceive(x), perceive(y));
                ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
            })
            .collect();
        let n = d.len() as f32;
        let mean = d.iter().sum::<f32>() / n;
        let over = |t: f32| d.iter().filter(|&&x| x > t).count() as f32 / n;
        let (o1, o4) = (over(1.), over(4.));
        d.sort_by(|x, y| x.partial_cmp(y).unwrap());
        (mean, d[(n * 0.99) as usize], d[d.len() - 1], o1, o4)
    }

    /// The frame the host tests draw: husky's divided by 8 (its aspect, its survey at a sixteenth of that).
    const W: usize = 1344 / 8;
    const H: usize = 2992 / 8;

    /// The survey on the GPU against the CPU's: for 8 seeds, each candidate the device would look at, the GPU's counts and distances tallied as the CPU's are; their fractions and percentiles close, and the same candidate kept.
    #[test]
    fn survey_twin() {
        let _turn = RECORDING.lock().unwrap();
        let (w, h) = (1344, 2992);
        let pw = (w / PREVIEW) * (h / PREVIEW);
        let k = Knobs::DEFAULT;
        let mut kept_same = 0;
        for s in 0..8 {
            let seed = seed_of(s);
            let mut worst = 0f32;
            let gpu = crate::params_by(
                &seed,
                w,
                h,
                &k,
                |p| {
                    let kern = survey_kernel(p);
                    let mem = emulate(&kern, &[], pw.next_multiple_of(16), vec![0; 8 * pw.next_multiple_of(16)], |g| 8 * g);
                    let q = preview(p);
                    let got = tally(p, &k, |emit| {
                        for i in 0..pw {
                            let f = |o: usize| f32::from_le_bytes(mem[8 * i + o..8 * i + o + 4].try_into().unwrap());
                            emit(i % q.width, i / q.width, f(0), f(4));
                        }
                    });
                    let want = crate::survey_with(p, &k);
                    for (a, b) in [(got.near, want.near), (got.near_min_third, want.near_min_third), (got.inside, want.inside), (got.p50 / 9., want.p50 / 9.), (got.p99 / 9., want.p99 / 9.)] {
                        worst = worst.max((a - b).abs());
                    }
                    got
                },
                |_| {},
            );
            let cpu = params(&seed, w, h);
            kept_same += (gpu.attempt == cpu.attempt) as u32;
            println!("SURVEY seed {s}: kept attempt {} (CPU {}), worst difference in a fraction or a percentile over log2(514) {worst:.5}", gpu.attempt, cpu.attempt);
            assert!(worst < 1. / 64., "seed {s}: the GPU's survey is {worst} from the CPU's");
        }
        assert!(kept_same >= 7, "the GPU kept the CPU's candidate for {kept_same} of 8 seeds");
    }

    /// The first pass and the refining pass on the GPU, compiled and run in the emulator, against the CPU's frame at an eighth of husky's size: ΔE in the perceptual space, printed, and held to "looks the same".
    #[test]
    fn picture_twin() {
        let _turn = RECORDING.lock().unwrap();
        for s in [0u64, 3, 41] {
            let p = params(&seed_of(s), W, H);
            let n = W * H;
            let mut cpu = vec![0u64; n];
            render_rows(&p, 0..H, &mut cpu);
            let t = std::time::Instant::now();
            let kern = render_kernel(&p);
            let compile = t.elapsed();
            // Two bands, to run the band's start (uniform 1) as wallgen does.
            let half = n.next_multiple_of(32) / 2;
            let mut mem = vec![0u8; 8 * (n.next_multiple_of(32) + 16)];
            for b in 0..2 {
                mem = emulate(&kern, &[(b * half) as u64], half, mem, |g| 8 * (b * half + g));
            }
            let gpu: Vec<u64> = (0..n).map(|i| u64::from_le_bytes(mem[8 * i..8 * i + 8].try_into().unwrap())).collect();
            let (mean, p99, most, o1, o4) = delta_e(&cpu, &gpu);
            println!("FIRST seed {s} {W}x{H}: {} words, {} kept nodes, 64 registers {}, peak {}, compiled in {:?}; ΔE mean {mean:.3}, 99% {p99:.2}, most {most:.1}; above 1: {:.2} %, above 4: {:.3} %", kern.words.len(), kern.kept, kern.regs64, kern.peak, compile, 100. * o1, 100. * o4);
            assert!(mean < 0.5 && o4 < 1. / 64., "seed {s}: the GPU's first pass does not look the CPU's");
            // The refine: the marks from the GPU's first pass, the marked pixels listed and refined on the GPU, against the CPU's refine of its own first pass.
            let stride = mask_stride(W);
            let mut mask = vec![0u8; H * stride];
            mark_rows(&p, &gpu, 0..H, &mut mask);
            let list: Vec<u32> = (0..n).filter(|&i| mask[i / W * stride + i % W / 8] >> (i % W % 8) & 1 == 1).map(|i| i as u32).collect();
            let mut padded = list.clone();
            padded.resize(list.len().next_multiple_of(16), *list.last().unwrap());
            let t = std::time::Instant::now();
            let rk = refine_kernel(&p);
            let compile = t.elapsed();
            // The list after the frame (uniform 1).
            let at = mem.len();
            let mut m2 = mem.clone();
            m2.extend(padded.iter().flat_map(|i| i.to_le_bytes()));
            let m2 = emulate(&rk, &[VA + at as u64], padded.len(), m2, |g| 8 * padded[g] as usize);
            let gpu_r: Vec<u64> = (0..n).map(|i| u64::from_le_bytes(m2[8 * i..8 * i + 8].try_into().unwrap())).collect();
            let mut cpu_mask = vec![0u8; H * stride];
            mark_rows(&p, &cpu, 0..H, &mut cpu_mask);
            let mut cpu_r = cpu.clone();
            refine_rows(&p, 0..H, &cpu_mask, &mut cpu_r);
            let (mean, p99, most, o1, o4) = delta_e(&cpu_r, &gpu_r);
            let cpu_marked: u32 = cpu_mask.iter().map(|b| b.count_ones()).sum();
            println!("REFINED seed {s}: {} words, {} kept nodes, peak {}, compiled in {:?}; {} marked (CPU {cpu_marked}); ΔE mean {mean:.3}, 99% {p99:.2}, most {most:.1}; above 1: {:.2} %, above 4: {:.3} %", rk.words.len(), rk.kept, rk.peak, compile, list.len(), 100. * o1, 100. * o4);
            assert!(mean < 0.5 && o4 < 1. / 64., "seed {s}: the GPU's refined frame does not look the CPU's");
            if s == 0 {
                write_pair(&cpu_r, &gpu_r);
            }
        }
    }

    /// With WALLPAPER_TWIN set to a directory, the CPU's and the GPU's refined frames there as VSF visuals (as examples/render.rs writes one) to compare by eye.
    fn write_pair(cpu: &[u64], gpu: &[u64]) {
        let Ok(dir) = std::env::var("WALLPAPER_TWIN") else { return };
        for (name, px) in [("cpu", cpu), ("gpu", gpu)] {
            let n = px.len();
            let mut samples = vec![0u16; 3 * n];
            for (i, &v) in px.iter().enumerate() {
                for c in 0..3 {
                    let code = ((v >> (16 * c)) & 0xFFFF) as f32 / 16384.;
                    samples[c * n + i] = vsf::colour::convert::delinearize_gamma2tanh_bits_f32(code * code, 16) as u16;
                }
            }
            let tensor = vsf::types::BitPackedTensor::pack(16, vec![3, H, W], &samples);
            let mut plane = vsf::visual::Plane::encoded("display", "wallpaper", W, H, vsf::visual::Primaries::Vsf, vsf::visual::Encoding::Gamma2Tanh, tensor);
            plane.name = "main".into();
            let v = vsf::visual::Visual { title: format!("wallpaper seed 0, {name}"), text: "The wallpaper crate's picture at an eighth of husky's frame: the CPU's, or the GPU kernels' run in valhall's emulator.".into(), planes: vec![plane], ..Default::default() };
            std::fs::write(format!("{dir}/wallpaper-{name}.vsf"), v.write().unwrap()).unwrap();
        }
    }

    /// f32 rounded to f16 (nearest, ties to even; f16's range: past 65504 infinite, below 2^-24 zero), kept in an f32.
    fn f16r(x: f32) -> f32 {
        if !x.is_finite() || x == 0. {
            return x;
        }
        let a = x.abs();
        if a >= 65520. {
            return x.signum() * f32::INFINITY;
        }
        // The spacing of f16 values at a: 2^(e − 10) for normals (e = ⌊log₂ a⌋, at least −14), so subnormals keep 2^-24.
        let e = (a.log2().floor() as i32).max(-14);
        let q = ((e - 10) as f32).exp2();
        (x / q).round_ties_even() * q
    }

    /// A lane whose every result is rounded to f16: the colour stage as the GPU's paired half-precision ops would compute it.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    struct Hf(f32);
    impl core::ops::Add for Hf {
        type Output = Hf;
        fn add(self, o: Hf) -> Hf {
            Hf(f16r(self.0 + o.0))
        }
    }
    impl core::ops::Sub for Hf {
        type Output = Hf;
        fn sub(self, o: Hf) -> Hf {
            Hf(f16r(self.0 - o.0))
        }
    }
    impl core::ops::Mul for Hf {
        type Output = Hf;
        fn mul(self, o: Hf) -> Hf {
            Hf(f16r(self.0 * o.0))
        }
    }
    impl core::ops::Div for Hf {
        type Output = Hf;
        fn div(self, o: Hf) -> Hf {
            Hf(f16r(self.0 / o.0))
        }
    }
    impl core::ops::Neg for Hf {
        type Output = Hf;
        fn neg(self) -> Hf {
            Hf(-self.0)
        }
    }
    impl core::ops::Add<f32> for Hf {
        type Output = Hf;
        fn add(self, o: f32) -> Hf {
            self + Hf(f16r(o))
        }
    }
    impl core::ops::Sub<f32> for Hf {
        type Output = Hf;
        fn sub(self, o: f32) -> Hf {
            self - Hf(f16r(o))
        }
    }
    impl core::ops::Mul<f32> for Hf {
        type Output = Hf;
        fn mul(self, o: f32) -> Hf {
            self * Hf(f16r(o))
        }
    }
    impl core::ops::Div<f32> for Hf {
        type Output = Hf;
        fn div(self, o: f32) -> Hf {
            self / Hf(f16r(o))
        }
    }
    impl Lane for Hf {
        type M = bool;
        fn splat(v: f32) -> Hf {
            Hf(f16r(v))
        }
        fn sqrt(self) -> Hf {
            Hf(f16r(self.0.sqrt()))
        }
        fn log2(self) -> Hf {
            Hf(f16r(self.0.log2()))
        }
        fn ln(self) -> Hf {
            Hf(f16r(self.0.ln()))
        }
        fn sin_turns(self) -> Hf {
            Hf(f16r((self.0 * core::f32::consts::TAU).sin()))
        }
        fn gt(self, o: Hf) -> bool {
            self.0 > o.0
        }
        fn ge(self, o: Hf) -> bool {
            self.0 >= o.0
        }
        fn lt(self, o: Hf) -> bool {
            self.0 < o.0
        }
        fn select(m: bool, a: Hf, b: Hf) -> Hf {
            if m { a } else { b }
        }
    }

    /// Where f16 breaks the picture (owner, 2026-11-11: "f16 where it suffices ... check where f16 breaks the picture and say"): the first pass at an eighth of husky's frame with (a) the colour stage in f16 from the orbit's own values (the distance in plane units), (b) the same with the distance taken in spans of the frame before it is narrowed, (c) the orbit itself in f16. ΔE against the f32 picture, printed.
    #[test]
    fn f16_where_it_breaks() {
        for s in [0u64, 3, 41] {
            let p = params(&seed_of(s), W, H);
            let f = Frame::new(&p);
            let mut cpu = vec![0u64; W * H];
            render_rows(&p, 0..H, &mut cpu);
            let to_h = |o: crate::Orbit<f32>, dscale: f32| crate::Orbit { n: Hf(f16r(o.n)), d: Hf(f16r(o.d * dscale)), s: Hf(f16r(o.s)), u: [Hf(f16r(o.u[0])), Hf(f16r(o.u[1]))], f: [Hf(f16r(o.f[0])), Hf(f16r(o.f[1]))] };
            let ds = f.views[NEAR_VIEW].2;
            let mut variants = [vec![0u64; W * H], vec![0u64; W * H], vec![0u64; W * H]];
            for y in 0..H {
                for x in 0..W {
                    let (fx, fy) = (x as f32 + 0.5 - f.half[0], y as f32 + 0.5 - f.half[1]);
                    let far = f.far(fx, fy);
                    let (zx, zy) = f.z_at(NEAR_VIEW, fx, fy);
                    let o = orbit(zx, zy, p.c[0], p.c[1]);
                    let un = |c: [Hf; 3]| crate::pack([c[0].0, c[1].0, c[2].0]);
                    // (a): d narrowed in plane units, the colour stage's own scale to spans in f16.
                    variants[0][y * W + x] = un(f.colour(Hf(f16r(fx)), Hf(f16r(fy)), to_h(o, 1.), Hf(f16r(far))));
                    // (b): d in spans before narrowing, the colour stage's scale then 1 (a frame whose near view's factor is 1 sees the same picture).
                    let mut g = Frame::new(&p);
                    g.views[NEAR_VIEW].2 = 1.;
                    g.span = f.span;
                    variants[1][y * W + x] = un(g.colour(Hf(f16r(fx)), Hf(f16r(fy)), to_h(o, ds), Hf(f16r(far))));
                    // (c): the orbit in f16 too, from the pixel's plane point narrowed.
                    let oh = orbit(Hf(f16r(zx)), Hf(f16r(zy)), p.c[0], p.c[1]);
                    variants[2][y * W + x] = un(f.colour(Hf(f16r(fx)), Hf(f16r(fy)), oh, Hf(f16r(far))));
                }
            }
            for (name, v) in ["colour f16, d in plane units", "colour f16, d in spans", "orbit f16 too"].iter().zip(&variants) {
                let (mean, p99, most, o1, o4) = delta_e(&cpu, v);
                println!("F16 seed {s} {name}: ΔE mean {mean:.3}, 99% {p99:.2}, most {most:.1}; above 1: {:.2} %, above 4: {:.3} %", 100. * o1, 100. * o4);
            }
            let (mean, ..) = delta_e(&cpu, &variants[1]);
            assert!(mean < 1., "seed {s}: the colour stage in f16 (d in spans) does not look the same: ΔE {mean}");
        }
    }
}
