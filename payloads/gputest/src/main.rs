//! A client of the GPU service (payloads/gpu, GPU.md "Layers"): the Mandelbrot kernel built through the valhall builder, its words put in the layer arena, run through `gpu`, and every pixel compared with the CPU's own iteration.
//! Spawn: ferros-bridge spawn gputest.bin --name gputest --use gpu --dma layers
//! Grant slots: 0 console, 1 gpu (use), 2 the layer arena.
#![no_std]
#![no_main]

extern crate alloc;
use ferros_sys::{call, cap, console, exit};
use valhall::*;

const S_GPU: u64 = 1;
const S_ARENA: u64 = 2;
/// The arena at the GPU's VA (payloads/gpu ARENA_VA): the code at offset 0, the output 1 MiB in.
const ARENA_VA: u64 = 0x8000_0000;
const OUT: usize = 1 << 20;
const SIDE_SHIFT: u32 = 6;
const PIXELS: usize = 1 << (2 * SIDE_SHIFT);
const MAX_ITER: u32 = 64;

struct Line([u8; 160], usize);
impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.1 < self.0.len() {
                self.0[self.1] = b;
                self.1 += 1;
            }
        }
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($a:tt)*) => {{
        let mut l = Line([0; 160], 0);
        let _ = core::fmt::write(&mut l, format_args!($($a)*));
        console($g, &l.0[..l.1]);
    }};
}

/// The Mandelbrot kernel (valhall/tests/kernels.rs `mandelbrot`, verified word for word): per invocation one pixel of a 2^shift-wide grid, its escape count to the output.
fn mandelbrot(k: &mut Kernel<32>) {
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
}

/// The same pixel on the CPU, in the kernel's order of operations.
fn reference(gid: u32, x0: f32, y0: f32, step: f32) -> u32 {
    let py = gid >> SIDE_SHIFT;
    let px = gid - (py << SIDE_SHIFT);
    let (cr, ci) = (px as f32 * step + x0, py as f32 * step + y0);
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


/// The ops kernel (2026-11-05, the builder's new instructions checked on the device): per invocation, x and y from the input pairs, nine results to nine rows of the output.
const CASES: usize = 64;
const OPS: usize = 9;
fn ops(k: &mut Kernel<32>) {
    k.shaddx_u64(pair(2), u(0), pair(60), 3);
    k.load_i32(r(4), pair(2), 0, Slot::S0);
    k.load_i32(r(5), pair(2), 4, Slot::S0).wait(Slot::S0);
    k.shaddx_u64(pair(6), u(1), pair(60), 2);
    k.fmin_f32(r(8), r(4), r(5));
    k.fmax_f32(r(9), r(4), r(5));
    k.frsq_f32(r(10), r(4));
    k.f32_to_s32(r(11), r(4), Round::Rtn);
    k.f32_to_u32(r(12), r(4), Round::Rtz);
    k.fround_f32(r(13), r(4), Round::Rtn);
    k.s32_to_f32(r(14), r(5));
    k.lshift_xor_i32(r(15), r(4), Lut::ZERO, Byte::B0, r(5));
    k.fcmp_or_f32(r(16), Cond::Lt, CmpResult::M1, r(4), r(5), Lut::ZERO);
    k.mux_i32(r(17), Mux::Bit, r(4), r(5), r(16));
    for (n, reg) in [8u8, 9, 10, 11, 12, 13, 14, 15, 17].iter().enumerate() {
        let p = k.store_i32(r(*reg), pair(6), (n * CASES * 4) as i16, Slot::S1);
        if n == OPS - 1 {
            p.end();
        }
    }
}

/// The inputs: edges (±0, halves, negatives, large, tiny) and a spread, as (x, y) pairs.
fn case(i: usize) -> (f32, f32) {
    const X: [f32; 16] = [0.0, -0.0, 0.5, -0.5, 1.5, -1.5, 2.5, -2.5, 1e-38, 3.0e9, -7.75, 123456.7, 0.25, 4.0, 1.0e-3, 65536.5];
    let x = if i < 16 { X[i] } else { (i as f32 - 40.) * 1.37 };
    let y = if i < 16 { X[15 - i] } else { (i as f32) * -0.61 + 3. };
    (x, y)
}

/// The CPU's answer for op `n` (frsq is approximate on the GPU: checked by ulps, not bits).
fn expect(n: usize, x: f32, y: f32) -> u32 {
    match n {
        0 => (if x < y { x } else if y < x { y } else if x.to_bits() >> 31 == 1 { x } else { y }).to_bits(),
        1 => (if x > y { x } else if y > x { y } else if x.to_bits() >> 31 == 0 { x } else { y }).to_bits(),
        2 => (1. / libm_sqrt(x)).to_bits(),
        3 => floor(x) as i32 as u32,
        4 => if x <= 0. { 0 } else { x as u32 },
        // floor(-0) is -0 in IEEE; the integer round trip below would make it +0.
        5 => if x == 0. { x.to_bits() } else { floor(x).to_bits() },
        6 => (y.to_bits() as i32 as f32).to_bits(),
        7 => x.to_bits() ^ y.to_bits(),
        _ => (if x < y { x } else { y }).to_bits(),
    }
}
fn floor(x: f32) -> f32 {
    let t = x as i64 as f32;
    if t > x { t - 1. } else { t }
}
fn libm_sqrt(x: f32) -> f32 {
    // Newton on the CPU from a bit guess: exact enough to count the GPU's ulps against.
    if x <= 0. {
        return if x == 0. { 0. } else { f32::NAN };
    }
    let mut g = f32::from_bits((x.to_bits() >> 1) + 0x1FBD_1DF5);
    for _ in 0..6 {
        g = 0.5 * (g + x / g);
    }
    g
}

fn ops_test(grants: u64, arena: usize) -> ! {
    let mut k = Kernel::<32>::new();
    ops(&mut k);
    let words = k.finish();
    for (i, &w) in words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    let (inp, outp) = (2 << 20, 3 << 20);
    for i in 0..CASES {
        let (x, y) = case(i);
        unsafe {
            core::ptr::write_volatile((arena + inp + 8 * i) as *mut f32, x);
            core::ptr::write_volatile((arena + inp + 8 * i + 4) as *mut f32, y);
        }
    }
    for i in 0..CASES * OPS {
        unsafe { core::ptr::write_volatile((arena + outp + 4 * i) as *mut u32, 0xDEAD_BEEF) };
    }
    let uniforms = [ARENA_VA + inp as u64, ARENA_VA + outp as u64];
    let reply = run(grants, words.len(), (CASES / 16) as u32, &uniforms);
    const NAMES: [&str; OPS] = ["fmin", "fmax", "frsq", "f32_to_s32.rtn", "f32_to_u32.rtz", "fround.rtn", "s32_to_f32", "lshift_xor", "mux.bit"];
    for n in 0..OPS {
        let (mut wrong, mut worst_ulp, mut first) = (0u32, 0u32, None);
        for i in 0..CASES {
            let (x, y) = case(i);
            let got = unsafe { core::ptr::read_volatile((arena + outp + 4 * (n * CASES + i)) as *const u32) };
            let want = expect(n, x, y);
            if n == 2 {
                // Normal positive inputs only: the GPU flushes subnormals to zero (1e-38 is one).
                if x >= f32::MIN_POSITIVE {
                    let ulp = (got as i64 - want as i64).unsigned_abs() as u32;
                    if ulp > worst_ulp {
                        worst_ulp = ulp;
                        first = Some((i, got, want));
                    }
                }
            } else if got != want {
                wrong += 1;
                first.get_or_insert((i, got, want));
            }
        }
        if n == 2 {
            let (i, g, w) = first.unwrap_or((0, 0, 0));
            say!(grants, "gputest: {}: worst {worst_ulp} ulp over the normal positive inputs (case {i}: G#{g:08X}, the CPU G#{w:08X})\n", NAMES[n]);
        } else if let Some((i, g, w)) = first {
            say!(grants, "gputest: {}: {wrong} of {CASES} wrong, the first case {i}: G#{g:08X}, the CPU G#{w:08X}\n", NAMES[n]);
        } else {
            say!(grants, "gputest: {}: all {CASES} as the CPU\n", NAMES[n]);
        }
    }
    say!(grants, "gputest: ops job status {}, {} us\n", reply.0, reply.1);
    exit(0);
}


/// The SSA recorder's working memory (valhall::ssa: the graph and the compiled words): a bump allocator over a zeroed static whose last allocation can be given back, as ahau's.
#[repr(C, align(64))]
struct Heap([u8; 4 << 20]);
static mut HEAP: Heap = Heap([0; 4 << 20]);
static mut HEAP_NEXT: usize = 0;
struct Bump;
unsafe impl core::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: core::alloc::Layout) -> *mut u8 {
        unsafe {
            let base = (&raw mut HEAP) as usize;
            let start = (base + HEAP_NEXT + l.align() - 1) & !(l.align() - 1);
            if start + l.size() > base + (4 << 20) {
                return core::ptr::null_mut();
            }
            HEAP_NEXT = start + l.size() - base;
            start as *mut u8
        }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: core::alloc::Layout) {
        unsafe {
            let base = (&raw mut HEAP) as usize;
            if p as usize + l.size() == base + HEAP_NEXT {
                HEAP_NEXT = p as usize - base;
            }
        }
    }
}
#[global_allocator]
static ALLOC: Bump = Bump;

/// The Mandelbrot recorded through the SSA layer (valhall::ssa) instead of written register by register: the loop through `iterate`, as tapuwae's orbit will run.
fn ssa_test(grants: u64, arena: usize) -> ! {
    use valhall::ssa::{self, F, U};
    let (x0, y0, step) = (-2.0f32, -1.25f32, 0.0390625f32);
    ssa::begin();
    let g = ssa::gid();
    let py = g >> SIDE_SHIFT;
    let px = g & U::splat((1 << SIDE_SHIFT) - 1);
    let cr = F::from_u32(px) * step + x0;
    let ci = F::from_u32(py) * step + y0;
    let z = F::splat(0.);
    // The tail freezes a stopped invocation's state (valhall::ssa::iterate: the stopping pass runs the tail too).
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
    let compiled = ssa::compile();
    for (i, &w) in compiled.words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    for i in 0..PIXELS {
        unsafe { core::ptr::write_volatile((arena + OUT + 4 * i) as *mut u32, u32::MAX) };
    }
    let preload = 0x1000u16 | if compiled.regs64 { 0x8000 } else { 0 };
    let mut uniforms = alloc::vec![ARENA_VA + OUT as u64];
    uniforms.extend_from_slice(&compiled.consts);
    let reply = run_with(grants, compiled.words.len(), (PIXELS / 16) as u32, preload, &uniforms);
    let mut wrong = 0u32;
    let mut first = None;
    for gid in 0..PIXELS as u32 {
        let got = unsafe { core::ptr::read_volatile((arena + OUT + 4 * gid as usize) as *const u32) };
        let want = reference(gid, x0, y0, step);
        if got != want {
            wrong += 1;
            first.get_or_insert((gid, got, want));
        }
    }
    let (fg, fgot, fwant) = first.unwrap_or((0, 0, 0));
    say!(grants, "gputest: ssa mandelbrot: {} words, 64 registers {}, status {}, job {} us; {wrong} of {PIXELS} differ (first gid {fg}: {fgot} vs {fwant})\n", compiled.words.len(), compiled.regs64, reply.0, reply.1);
    exit(0);
}

/// tapuwae's attest button, quick pass, on the GPU (tapuwae::gpu::button_quick_kernel, its own shading recorded through valhall::ssa) against the CPU's render_button_pack16 (k = 1), every pixel of the 900-pixel square (r = 448, as ahau draws it).
fn ball_test(grants: u64, arena: usize) -> ! {
    use tapuwae::{Look, Params};
    let salt = *b"ferros gputest: the attest ball.";
    let p = Params::new_placed(&salt, "handle", true);
    let look = Look::attest();
    let r = 448f32;
    let side = 2 * r as usize + 4;
    let px = side * side;
    let t0 = ferros_sys::now();
    let compiled = tapuwae::gpu::button_quick_kernel(r, &p, &look);
    let compile_us = ferros_sys::us_since(t0);
    for (i, &w) in compiled.words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    let (gpu_out, cpu_out) = (4 << 20, 16 << 20);
    for i in 0..px {
        unsafe { core::ptr::write_volatile((arena + gpu_out + 8 * i) as *mut u64, u64::MAX) };
    }
    let preload = 0x1000u16 | if compiled.regs64 { 0x8000 } else { 0 };
    let groups = px.div_ceil(16) as u32;
    // Slot 0 the output, then the kernel's constants (valhall::ssa puts its most-used constants in uniforms 1 and up).
    let mut uniforms = alloc::vec![ARENA_VA + gpu_out as u64];
    uniforms.extend_from_slice(&compiled.consts);
    let reply = run_with(grants, compiled.words.len(), groups, preload, &uniforms);
    let t1 = ferros_sys::now();
    let cpu = unsafe { core::slice::from_raw_parts_mut((arena + cpu_out) as *mut u64, px) };
    // The CPU writes only the disc's bounding square (from row and column 1): the rest must start as the GPU leaves it, 0.
    cpu.fill(0);
    let c = side as f32 * 0.5;
    tapuwae::render_button_pack16(cpu, side, c, c, r, &p, &look, 1, (0, 1), &|| false);
    let cpu_us = ferros_sys::us_since(t1);
    let (mut same, mut near, mut far, mut worst) = (0u32, 0u32, 0u32, 0u32);
    let mut first_far = None;
    for i in 0..px {
        let g = unsafe { core::ptr::read_volatile((arena + gpu_out + 8 * i) as *const u64) };
        let w = cpu[i];
        if g == w {
            same += 1;
            continue;
        }
        let d = (0..4).map(|ch| ((g >> (16 * ch)) as u16 as i32 - (w >> (16 * ch)) as u16 as i32).unsigned_abs()).max().unwrap_or(0);
        worst = worst.max(d);
        if d <= 64 {
            near += 1;
        } else {
            far += 1;
            first_far.get_or_insert(i);
            if far <= 4 {
                say!(grants, "gputest: ball: far at ({}, {}): GPU G#{g:016X}, CPU G#{w:016X}\n", i % side, i / side);
            }
        }
    }
    say!(grants, "gputest: ball: {} words, 64 registers {}, {} nodes ({} kept, peak {} registers), compiled in {compile_us} us; GPU job status {} in {} us, CPU {cpu_us} us\n", compiled.words.len(), compiled.regs64, compiled.nodes, compiled.kept, compiled.peak, reply.0, reply.1);
    say!(grants, "gputest: ball: of {px} pixels {same} identical, {near} within 64 of 16384 a channel, {far} further (worst {worst}, the first further at {})\n", first_far.map_or(-1, |i| i as i64));
    exit(0);
}

/// tapuwae's button, its 4 x 4 pass as 16 distinct GPU passes over the circle's staged coordinates (tapuwae::gpu::button_fine_kernel), against the CPU's render_button_pack16 with k = 4 at a small size (r = 96, a single-core CPU reference in reasonable time); then the same 16 passes at ahau's size (r = 448), timed only. Arena: the frame from 4 MiB, the stage from 11, the sums from 14, the CPU's frame from 25 (ahau's region begins at 32).
fn fine_test(grants: u64, arena: usize) -> ! {
    use tapuwae::{Look, Params};
    let salt = *b"ferros gputest: the attest ball.";
    let p = Params::new_placed(&salt, "handle", true);
    let look = Look::attest();
    let (frame, stage, sums, cpu_at) = (4usize << 20, 11usize << 20, 14usize << 20, 25usize << 20);
    for &(r, compare) in &[(96f32, true), (448f32, false)] {
        let side = 2 * r as usize + 4;
        let px = side * side;
        // The quick pass leaves every pixel outside the circle at 0, and the passes never touch them.
        for i in 0..px {
            unsafe { core::ptr::write_volatile((arena + frame + 8 * i) as *mut u64, 0) };
        }
        let mut n = 0usize;
        tapuwae::gpu::button_stage(r, |i| {
            unsafe { core::ptr::write_volatile((arena + stage + 4 * n) as *mut u32, i) };
            n += 1;
        });
        let (mut total_us, mut compile_us, mut bad) = (0u32, 0u64, 0u8);
        for m in 0..16 {
            // The heap as it stands, and back to it after the pass: each compile's graph and words are dropped by then (a bump heap; 32 compiles ran its 4 MiB out).
            let mark = unsafe { HEAP_NEXT };
            let t0 = ferros_sys::now();
            let k = tapuwae::gpu::button_fine_kernel(r, &p, &look, m);
            compile_us += ferros_sys::us_since(t0);
            for (i, &w) in k.words.iter().enumerate() {
                unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
            }
            let mut uniforms = alloc::vec![ARENA_VA + frame as u64, ARENA_VA + stage as u64, ARENA_VA + sums as u64];
            uniforms.extend_from_slice(&k.consts);
            let preload = 0x1000u16 | if k.regs64 { 0x8000 } else { 0 };
            let reply = run_with(grants, k.words.len(), n.div_ceil(16) as u32, preload, &uniforms);
            total_us += reply.1;
            bad |= reply.0;
            if reply.0 != 0 {
                say!(grants, "gputest: fine r = {r}: pass {m} failed (status {}), {} words, {} uniforms\n", reply.0, k.words.len(), uniforms.len());
                exit(1);
            }
            drop(uniforms);
            drop(k);
            unsafe { HEAP_NEXT = mark };
        }
        say!(grants, "gputest: fine r = {r}: {n} staged pixels, 16 passes {total_us} us on the GPU (status {bad}), compiled in {compile_us} us\n");
        if compare {
            let cpu = unsafe { core::slice::from_raw_parts_mut((arena + cpu_at) as *mut u64, px) };
            cpu.fill(0);
            let c = side as f32 * 0.5;
            let t1 = ferros_sys::now();
            tapuwae::render_button_pack16(cpu, side, c, c, r, &p, &look, 4, (0, 1), &|| false);
            let cpu_us = ferros_sys::us_since(t1);
            let (mut same, mut near, mut far, mut worst) = (0u32, 0u32, 0u32, 0u32);
            for i in 0..px {
                let g = unsafe { core::ptr::read_volatile((arena + frame + 8 * i) as *const u64) };
                let w = cpu[i];
                if g == w {
                    same += 1;
                    continue;
                }
                let d = (0..4).map(|ch| ((g >> (16 * ch)) as u16 as i32 - (w >> (16 * ch)) as u16 as i32).unsigned_abs()).max().unwrap_or(0);
                worst = worst.max(d);
                if d <= 64 {
                    near += 1;
                } else {
                    far += 1;
                    if far <= 3 {
                        say!(grants, "gputest: fine far at ({}, {}): GPU G#{g:016X}, CPU G#{w:016X}\n", i % side, i / side);
                    }
                }
            }
            say!(grants, "gputest: fine r = {r}: of {px} pixels {same} identical, {near} within 64 of 16384 a channel, {far} further (worst {worst}); the CPU's 4 x 4 {cpu_us} us\n");
        }
    }
    exit(0);
}

/// The SSA's loads and indexed stores alone (2026-11-05: the fine passes wedged the queue): invocation g loads idx = list[g] (shift 2) and four sums (shift 4), writes idx ^ G#5A5A and g + the sums' first word at frame[idx] (shift 3).
fn ldst_test(grants: u64, arena: usize) -> ! {
    use valhall::ssa::{self, U};
    let (frame, list, sums) = (4usize << 20, 11usize << 20, 14usize << 20);
    let n = 256usize;
    for g in 0..n {
        unsafe {
            core::ptr::write_volatile((arena + list + 4 * g) as *mut u32, (2 * g + 1) as u32);
            core::ptr::write_volatile((arena + sums + 16 * g) as *mut u32, 1000 + g as u32);
        }
    }
    for i in 0..2 * n + 2 {
        unsafe { core::ptr::write_volatile((arena + frame + 8 * i) as *mut u64, 0) };
    }
    ssa::begin();
    ssa::reserve(3);
    let g = ssa::gid();
    let idx = ssa::load(1, g, 2, 0);
    let s0 = ssa::load(2, g, 4, 0);
    ssa::store_at(0, idx, 3, 0, idx ^ U::splat(0x5A5A));
    ssa::store_at(0, idx, 3, 4, s0);
    let k = ssa::compile();
    for (i, &w) in k.words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    let mut uniforms = alloc::vec![ARENA_VA + frame as u64, ARENA_VA + list as u64, ARENA_VA + sums as u64];
    uniforms.extend_from_slice(&k.consts);
    let reply = run_with(grants, k.words.len(), (n / 16) as u32, 0x1000 | if k.regs64 { 0x8000 } else { 0 }, &uniforms);
    let mut wrong = 0;
    for gi in 0..n {
        let idx = 2 * gi + 1;
        let lo = unsafe { core::ptr::read_volatile((arena + frame + 8 * idx) as *const u32) };
        let hi = unsafe { core::ptr::read_volatile((arena + frame + 8 * idx + 4) as *const u32) };
        if lo != (idx as u32 ^ 0x5A5A) || hi != 1000 + gi as u32 {
            wrong += 1;
        }
    }
    say!(grants, "gputest: ldst: {} words, status {}, {} us; {wrong} of {n} wrong\n", k.words.len(), reply.0, reply.1);
    exit(0);
}

/// tapuwae's jitter hash (its sequence: IMUL, XOR, shifts, u32 to f32) alone through the SSA (2026-11-05: the fine passes, the first kernels with an integer multiply, wedged the queue): invocation g hashes (g, 3g, 5) and writes both offsets' bits.
fn jitter_test(grants: u64, arena: usize) -> ! {
    use valhall::ssa::{self, F, U};
    let frame = 4usize << 20;
    let n = 256usize;
    fn hash_cpu(px: u32, py: u32, m: u32) -> (f32, f32) {
        let mut h = px.wrapping_mul(0x9E37_79B1) ^ py.wrapping_mul(0x85EB_CA77) ^ m.wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 16;
        h = h.wrapping_mul(0x7FEB_352D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x846C_A68B);
        h ^= h >> 16;
        let s = 1. / (1u32 << 16) as f32;
        ((h & 0xFFFF) as f32 * s, (h >> 16) as f32 * s)
    }
    ssa::begin();
    let g = ssa::gid();
    let k = |c: u32| U::splat(c);
    let py = g.wrapping_mul(k(3));
    let mut h = g.wrapping_mul(k(0x9E37_79B1)) ^ py.wrapping_mul(k(0x85EB_CA77)) ^ k(5).wrapping_mul(k(0xC2B2_AE3D));
    h = h ^ (h >> 16);
    h = h.wrapping_mul(k(0x7FEB_352D));
    h = h ^ (h >> 15);
    h = h.wrapping_mul(k(0x846C_A68B));
    h = h ^ (h >> 16);
    let s = 1. / (1u32 << 16) as f32;
    let (jx, jy) = (F::from_u32(h & k(0xFFFF)) * s, F::from_u32(h >> 16) * s);
    ssa::store(0, 3, 0, jx.to_bits());
    ssa::store(0, 3, 4, jy.to_bits());
    let c = ssa::compile();
    for (i, &w) in c.words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    let mut uniforms = alloc::vec![ARENA_VA + frame as u64];
    uniforms.extend_from_slice(&c.consts);
    let reply = run_with(grants, c.words.len(), (n / 16) as u32, 0x1000 | if c.regs64 { 0x8000 } else { 0 }, &uniforms);
    let mut wrong = 0;
    for gi in 0..n {
        let (x, y) = hash_cpu(gi as u32, 3 * gi as u32, 5);
        let gx = unsafe { core::ptr::read_volatile((arena + frame + 8 * gi) as *const u32) };
        let gy = unsafe { core::ptr::read_volatile((arena + frame + 8 * gi + 4) as *const u32) };
        if gx != x.to_bits() || gy != y.to_bits() {
            wrong += 1;
        }
    }
    say!(grants, "gputest: jitter: {} words, status {}, {} us; {wrong} of {n} wrong\n", c.words.len(), reply.0, reply.1);
    exit(0);
}

/// One kernel, one job, status only (2026-11-05: which of the fine pass's differences wedges the queue): input byte 7 then which (0 the fine pass m = 0 at r = 448, 1 the same at r = 96, 2 the quick pass at r = 96).
fn one_test(grants: u64, arena: usize, which: u8) -> ! {
    use tapuwae::{Look, Params};
    let salt = *b"ferros gputest: the attest ball.";
    let p = Params::new_placed(&salt, "handle", true);
    let look = Look::attest();
    let (frame, stage, sums) = (4usize << 20, 11usize << 20, 14usize << 20);
    let r = if which == 0 { 448f32 } else { 96f32 };
    let side = 2 * r as usize + 4;
    let mut n = 0usize;
    tapuwae::gpu::button_stage(r, |i| {
        unsafe { core::ptr::write_volatile((arena + stage + 4 * n) as *mut u32, i) };
        n += 1;
    });
    let (k, groups, u) = if which == 2 {
        (tapuwae::gpu::button_quick_kernel(r, &p, &look), (side * side).div_ceil(16), alloc::vec![ARENA_VA + frame as u64])
    } else {
        (tapuwae::gpu::button_fine_kernel(r, &p, &look, 0), n.div_ceil(16), alloc::vec![ARENA_VA + frame as u64, ARENA_VA + stage as u64, ARENA_VA + sums as u64])
    };
    for (i, &w) in k.words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    let mut uniforms = u;
    uniforms.extend_from_slice(&k.consts);
    let reply = run_with(grants, k.words.len(), groups as u32, 0x1000 | if k.regs64 { 0x8000 } else { 0 }, &uniforms);
    say!(grants, "gputest: one {which}: r = {r}, {} words, {} uniforms, {} groups, peak {}; status {}, {} us\n", k.words.len(), uniforms.len(), groups, k.peak, reply.0, reply.1);
    exit(0);
}
/// One run through the service: the code at arena offset 0, `groups` workgroups of 16, preload r60 (the global id X).
fn run(grants: u64, words: usize, groups: u32, uniforms: &[u64]) -> (u8, u32) {
    run_with(grants, words, groups, 0x1000, uniforms)
}

/// As `run` with the preload word given (bit 15: 64 registers a thread).
fn run_with(grants: u64, words: usize, groups: u32, preload: u16, uniforms: &[u64]) -> (u8, u32) {
    let mut msg = [0u8; 18 + 8 * 16];
    msg[0] = 1;
    msg[1..3].copy_from_slice(&preload.to_le_bytes());
    msg[3..7].copy_from_slice(&0u32.to_le_bytes());
    msg[7..9].copy_from_slice(&(words as u16).to_le_bytes());
    msg[9..13].copy_from_slice(&groups.to_le_bytes());
    msg[13..17].copy_from_slice(&1u32.to_le_bytes());
    msg[17] = uniforms.len() as u8;
    for (i, u) in uniforms.iter().enumerate() {
        msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&u.to_le_bytes());
    }
    let mut reply = [0u8; 5];
    call(cap(grants, S_GPU), &msg[..18 + 8 * uniforms.len()], &mut reply);
    (reply[0], u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]]))
}
// The entry with the image header (as wintest and ahau carry it): the kernel grows the arc by the .bss the header names, so the zeroed statics below (the recorder's heap) are inside it.
core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym gputest_main
);

extern "C" fn gputest_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let arena = ferros_sys::dma(cap(grants, S_ARENA)) as usize;
    if arena == 0 || arena == usize::MAX {
        say!(grants, "gputest: no layer arena\n");
        exit(1);
    }
    // Input byte 1: the ops test (the builder's new instructions); none: the Mandelbrot.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 1 {
        ops_test(grants, arena);
    }
    // Input byte 7 then a selector: one kernel, one job, status only.
    if input_len > 1 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 7 {
        one_test(grants, arena, unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + 1) as *const u8) });
    }
    // Input byte 6: tapuwae's jitter hash alone.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 6 {
        jitter_test(grants, arena);
    }
    // Input byte 5: the SSA's loads and indexed stores alone.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 5 {
        ldst_test(grants, arena);
    }
    // Input byte 4: the 4 x 4 pass as 16 distinct GPU passes against the CPU.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 4 {
        fine_test(grants, arena);
    }
    // Input byte 3: tapuwae's attest ball (quick pass) on the GPU against the CPU.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 3 {
        ball_test(grants, arena);
    }
    // Input byte 2: the Mandelbrot recorded through valhall::ssa.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 2 {
        ssa_test(grants, arena);
    }
    let mut k = Kernel::<32>::new();
    mandelbrot(&mut k);
    let words = k.finish();
    for (i, &w) in words.iter().enumerate() {
        unsafe { core::ptr::write_volatile((arena + 8 * i) as *mut u64, w) };
    }
    for i in 0..PIXELS {
        unsafe { core::ptr::write_volatile((arena + OUT + 4 * i) as *mut u32, u32::MAX) };
    }
    let (x0, y0, step) = (-2.0f32, -1.25f32, 0.0390625f32);
    let uniforms = [ARENA_VA + OUT as u64, SIDE_SHIFT as u64 | (MAX_ITER as u64) << 32, x0.to_bits() as u64 | (y0.to_bits() as u64) << 32, step.to_bits() as u64];
    // [1][preload u16][code offset u32][code words u16][groups x u32][groups y u32][n u8][n × u64]
    let mut msg = [0u8; 18 + 8 * 4];
    msg[0] = 1;
    msg[1..3].copy_from_slice(&0x1000u16.to_le_bytes());
    msg[3..7].copy_from_slice(&0u32.to_le_bytes());
    msg[7..9].copy_from_slice(&(words.len() as u16).to_le_bytes());
    msg[9..13].copy_from_slice(&((PIXELS / 16) as u32).to_le_bytes());
    msg[13..17].copy_from_slice(&1u32.to_le_bytes());
    msg[17] = uniforms.len() as u8;
    for (i, u) in uniforms.iter().enumerate() {
        msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&u.to_le_bytes());
    }
    let mut reply = [0u8; 5];
    let t0 = ferros_sys::now();
    let n = call(cap(grants, S_GPU), &msg, &mut reply);
    let call_us = ferros_sys::us_since(t0);
    let job_us = u32::from_le_bytes([reply[1], reply[2], reply[3], reply[4]]);
    let mut wrong = 0u32;
    let mut first = None;
    for gid in 0..PIXELS as u32 {
        let got = unsafe { core::ptr::read_volatile((arena + OUT + 4 * gid as usize) as *const u32) };
        if got != reference(gid, x0, y0, step) {
            wrong += 1;
            first.get_or_insert(gid);
        }
    }
    say!(grants, "gputest: {} words; reply {n} bytes, status {}, job {job_us} us, call {call_us} us; {wrong} of {PIXELS} pixels differ from the CPU (the first at gid {})\n", words.len(), reply[0], first.map_or(-1, |g| g as i64));
    exit(if reply[0] == 0 && wrong == 0 { 0 } else { 1 });
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Where and why, on the console: grants are slot 0 at arc offset 0 (the console token), as every payload's.
    let mut l = Line([0; 160], 0);
    let _ = core::fmt::write(&mut l, format_args!("gputest: panic: {}\n", info));
    console(0, &l.0[..l.1]);
    exit(0xDEAD);
}
