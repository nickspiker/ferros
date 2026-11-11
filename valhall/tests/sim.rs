//! The emulator (valhall::sim) checked against what the device ran: the Mandelbrot kernel's words, which on the device gave every pixel the CPU's escape count (GPU.md, sum 83000, 1038 pixels at 64), and the same kernel recorded through the SSA layer (gputest's `ssa_test`, 0 of 4096 different on the device).
//! Then what the device has not run yet, checked here first: two loops one after the other in one kernel (the wallpaper's far and near orbits), and a uniform word and an integer add read as values.
#![cfg(all(feature = "sim", feature = "ssa"))]

use valhall::sim::{run, Memory};
use valhall::*;

const SIDE_SHIFT: u32 = 6;
const PIXELS: usize = 1 << (2 * SIDE_SHIFT);
const MAX_ITER: u32 = 64;
const OUT: u64 = 0x8000_0000;
const X0: f32 = -2.0;
const Y0: f32 = -1.25;
const STEP: f32 = 0.0390625;

/// gputest's `reference`: the pixel on the CPU in the kernel's order of operations.
fn reference(gid: u32) -> u32 {
    let py = gid >> SIDE_SHIFT;
    let px = gid - (py << SIDE_SHIFT);
    let (cr, ci) = (px as f32 * STEP + X0, py as f32 * STEP + Y0);
    let (mut zx, mut zy, mut k) = (0f32, 0f32, 0u32);
    loop {
        let (x2, y2) = (zx * zx, zy * zy);
        if k == MAX_ITER || !(x2 + y2 <= 4.0) {
            return k;
        }
        zy = (zx + zx) * zy + ci;
        zx = (x2 - y2) + cr;
        k += 1;
    }
}

/// Every pixel through the emulator; the counts read back.
fn counts(words: &[u64], uniforms: &[u64]) -> Vec<u32> {
    let mut mem = Memory { base: OUT, bytes: vec![0xFF; 4 * PIXELS] };
    for g in 0..PIXELS as u32 {
        run(words, uniforms, [g, 0], &mut mem);
    }
    (0..PIXELS).map(|i| mem.read32(OUT + 4 * i as u64)).collect()
}

#[test]
fn the_devices_mandelbrot_words() {
    let mut k = Kernel::<32>::new();
    k.rshift_or_i32(r(13), r(60), u(1).w0(), Byte::B0, Lut::ZERO);
    k.lshift_or_i32(r(12), r(13), u(1).w0(), Byte::B0, Lut::ZERO);
    k.isub_s32(r(12), r(60), r(12));
    k.u32_to_f32(r(12), r(12));
    k.u32_to_f32(r(13), r(13));
    k.mov_i32(r(10), u(3).w0());
    k.fma_f32(r(6), r(12), r(10), u(2).w0());
    k.fma_f32(r(7), r(13), r(10), u(2).w1());
    k.iadd_imm_i32(r(4), Lut::ZERO, 4f32.to_bits());
    k.iadd_imm_i32(r(0), Lut::ZERO, 0);
    k.iadd_imm_i32(r(1), Lut::ZERO, 0);
    k.iadd_imm_i32(r(5), Lut::ZERO, 0).reconverge();
    let top = k.here();
    k.fma_f32(r(8), r(1), r(1), Lut::ZERO.neg());
    k.fma_f32(r(9), r(5), r(5), Lut::ZERO.neg());
    k.fadd_f32(r(10), r(8), r(9));
    k.icmp_or_u32(r(11), Cond::Ne, CmpResult::M1, r(0), u(1).w1(), Lut::ZERO);
    k.fcmp_or_f32(r(12), Cond::Le, CmpResult::M1, r(10), r(4), Lut::ZERO);
    k.lshift_and_i32(r(11), r(11), Lut::ZERO, Byte::B0, r(12));
    k.fadd_f32(r(10), r(1), r(1));
    k.fma_f32(r(5), r(10), r(5), r(7));
    k.fadd_f32(r(10), r(8), r(9).neg());
    k.fadd_f32(r(1), r(10), r(6));
    k.isub_s32(r(0), r(0), r(11));
    k.branchz(r(11), top).reconverge();
    k.shaddx_u64(pair(2), u(0), pair(60), 2);
    k.store_i32(r(0), pair(2), 0, Slot::S0).end();
    let uniforms = [OUT, SIDE_SHIFT as u64 | (MAX_ITER as u64) << 32, X0.to_bits() as u64 | (Y0.to_bits() as u64) << 32, STEP.to_bits() as u64];
    let got = counts(k.finish(), &uniforms);
    // What the device measured (GPU.md "A real kernel"): the sum and the pixels at the cap.
    assert_eq!(got.iter().map(|&c| c as u64).sum::<u64>(), 83000);
    assert_eq!(got.iter().filter(|&&c| c == MAX_ITER).count(), 1038);
    for g in 0..PIXELS as u32 {
        assert_eq!(got[g as usize], reference(g), "pixel {g}");
    }
}

/// The SSA lock: one recording at a time (the recorder is one global).
static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn the_ssa_mandelbrot() {
    use valhall::ssa::{self, F, U};
    let _turn = RECORDING.lock().unwrap();
    ssa::begin();
    let g = ssa::gid();
    let py = g >> SIDE_SHIFT;
    let px = g & U::splat((1 << SIDE_SHIFT) - 1);
    let cr = F::from_u32(px) * STEP + X0;
    let ci = F::from_u32(py) * STEP + Y0;
    let z = F::splat(0.);
    let (_, _, kf, _, _, _) = ssa::iterate(
        MAX_ITER,
        (z, z, F::splat(0.), z, z, ssa::M::splat(true)),
        |_, (zx, zy, kf, _, _, _)| {
            let (x2, y2) = (zx * zx, zy * zy);
            let running = !(x2 + y2).gt(F::splat(4.));
            ((zx, zy, kf, x2, y2, running), running)
        },
        |_, (zx, zy, kf, x2, y2, running)| {
            let nzy = (zx + zx) * zy + ci;
            let nzx = (x2 - y2) + cr;
            (F::select(running, nzx, zx), F::select(running, nzy, zy), F::select(running, kf + 1., kf), x2, y2, running)
        },
    );
    ssa::store(0, 2, 0, kf.to_u32());
    let c = ssa::compile();
    let mut uniforms = vec![OUT];
    uniforms.extend_from_slice(&c.consts);
    let got = counts(&c.words, &uniforms);
    // Fast (contracted) as the device ran it: every pixel the CPU's on the device too.
    let wrong = (0..PIXELS as u32).filter(|&g| got[g as usize] != reference(g)).count();
    assert_eq!(wrong, 0);
}

/// Two loops in turn, values carried from the first into the second and out of both, a uniform word and an integer add: per invocation g, the escape count of z² + c from 0 at c = (g/64 − 1.5, b/64 − 0.5) with b uniform 1's low word, and then of the same orbit from that count's z with c halved.
#[test]
fn two_loops_a_uniform_and_an_add() {
    use valhall::ssa::{self, F, M};
    let _turn = RECORDING.lock().unwrap();
    let escape = |c: (F, F), z0: (F, F)| {
        let (_, _, k, x, y) = ssa::iterate(
            48,
            (z0.0, z0.1, F::splat(0.), z0.0, z0.1),
            |n, (x, y, _, _, _)| {
                let running: M = F::splat(4.).ge(x * x + y * y);
                ((x, y, n, x, y), running)
            },
            |_, (x, y, k, _, _)| ((x * x - y * y) + c.0, (x * y) * 2. + c.1, k, x, y),
        );
        (k, x, y)
    };
    ssa::begin();
    ssa::reserve(2);
    let g = ssa::gid();
    let b = ssa::uniform(1, 0);
    let gb = g + b;
    let c = (F::from_u32(g) * (1. / 64.) - 1.5, F::from_u32(b) * (1. / 64.) - 0.5);
    let (k0, x0, y0) = escape(c, (F::splat(0.), F::splat(0.)));
    let (k1, _, _) = escape((c.0 * 0.5, c.1 * 0.5), (x0 * 0.5, y0 * 0.5));
    ssa::store(0, 4, 0, k0.to_u32());
    ssa::store(0, 4, 4, k1.to_u32());
    ssa::store(0, 4, 8, gb);
    ssa::store(0, 4, 12, (k0 + k1).to_bits());
    let comp = ssa::compile();
    let bv = 19u32;
    let mut uniforms = vec![OUT, bv as u64 | 0xDEAD_0000_0000_0000];
    uniforms.extend_from_slice(&comp.consts);
    let n = 128u32;
    let mut mem = Memory { base: OUT, bytes: vec![0; 16 * n as usize] };
    for g in 0..n {
        run(&comp.words, &uniforms, [g, 0], &mut mem);
    }
    // The CPU, the same steps (no contraction: the counts are what is compared, and a contracted step moves few if any).
    // The kernel leaves with the head's z: at the escape, or at the last pass (47) before its update.
    let esc = |c: (f32, f32), mut z: (f32, f32)| {
        for k in 0..48u32 {
            if !(4. >= z.0 * z.0 + z.1 * z.1) || k == 47 {
                return (k, z);
            }
            z = ((z.0 * z.0 - z.1 * z.1) + c.0, (z.0 * z.1) * 2. + c.1);
        }
        unreachable!()
    };
    let mut off = 0;
    for g in 0..n {
        let c = (g as f32 * (1. / 64.) - 1.5, bv as f32 * (1. / 64.) - 0.5);
        let (k0, z) = esc(c, (0., 0.));
        let (k1, _) = esc((c.0 * 0.5, c.1 * 0.5), (z.0 * 0.5, z.1 * 0.5));
        let at = OUT + 16 * g as u64;
        let (g0, g1) = (mem.read32(at), mem.read32(at + 4));
        off += (g0 != k0) as u32 + (g1 != k1) as u32;
        assert_eq!(mem.read32(at + 8), g + bv, "g + b at {g}");
        assert_eq!(f32::from_bits(mem.read32(at + 12)), g0 as f32 + g1 as f32, "the sum after both loops at {g}");
    }
    // Contraction may move an escape by a step at the set's edge; most must agree.
    assert!(off <= 4, "{off} of {} counts differ", 2 * n);
}
