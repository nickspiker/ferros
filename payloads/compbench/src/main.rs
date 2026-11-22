//! The compose bench (owner, 2026-11-21: "if it's stupid fast to keep it all on the GPU, add useful features and not sacrifice security, then I'm pretty sure I'm down"): one full 1344 × 2992 frame of three layers (layout.rs) composed front to back with fluor's `under`, the background noise generated under what is not opaque, baked and presented, on NEON and on the GPU, and small damage rects patched by each.
//!
//! NEON: fluor's own noise (`noise_under`), fluor's `under` (the same kernel in NEON here: fluor's SIMD needs std), the display's own bake (bake.rs `g2t_row`) and present (neon.rs `present_app16`, four phases), on worker threads pinned to the A715s and the X3.
//! GPU: the kernels of kernel.rs (recorded on the host by build.rs), thru a queue in the bench's own block, one invocation a half-row.
//!
//! Spawn: `ferros-bridge spawn compbench.bin [mode [seconds]] --name compbench --use gpu --dma layers-32`
//! Grant slots: 0 console, 1 gpu (use), 2 its GPU block (grown to 2^28 bytes).
//! Since 2026-11-21 also the noise in its parallel form (noise.rs, kernel.rs `record_par`: a workgroup a half-row, its scan in shared storage), measured and checked on the device in mode 0 beside the serial kernels, and the memory floor (a pane copied).
//! Modes: none or 0 every measure (the console has the lines); 1 the frame as fast as it goes up to 120 a second on NEON for the seconds given (default 2^4); 2 the same with the GPU making the baked frame and NEON presenting it; 3 the GPU making the phases' words itself; 4 nothing at all for the seconds (the power's floor); 5 and 6 to 9 the clip kernel checked on the device; 10 the queue's uniforms against calls over 2^10 jobs, 11 a NEON patch ending mid-line then a GPU job writing the rest of its lines, 2^6 times each way, 12 where any byte changed under such a patch would come from, 13 data the CPU writes between jobs (a pane, the same uniforms every job) over 2^10 jobs each way (GPU-KISTS.md section 3, "The queue's uniforms read stale"); 14 as 2 and 15 as 3 with the parallel kernels. The rects and patches of mode 0 run thru calls, chosen while the queue read stale uniforms (fixed 2026-11-21 by `ferros_gpuq::END_FLUSH`).
#![no_std]
#![no_main]

extern crate alloc;

#[allow(dead_code)]
mod layout;
#[allow(dead_code)]
mod tables;
#[allow(dead_code)]
mod noise;
#[path = "../../display/src/neon.rs"]
#[allow(dead_code)]
mod neon;
#[path = "../../display/src/odt.rs"]
#[allow(dead_code)]
mod odt;
#[path = "../../display/src/bake.rs"]
#[allow(dead_code)]
mod bake;
mod kernels {
    /// A kernel recorded by build.rs: its words, its register mode, its loop's length, its constants (the uniforms after the caller's).
    pub struct Kernel {
        pub regs64: bool,
        pub loop_words: usize,
        /// Its workgroup, 2^wg_log2 invocations (4: a warp, the serial kernels), and its shared storage a workgroup (0: none).
        pub wg_log2: u32,
        pub shared_bytes: u32,
        pub words: &'static [u64],
        pub consts: &'static [u64],
    }
    include!(concat!(env!("OUT_DIR"), "/kernels.rs"));
}

use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::gpu::queue::{Job, Queue, S_DONE};
use ferros_sys::{cap, console, exit};
use fluor::canvas::{Canvas, Damage};
use fluor::paint::Clip;
use fluor::pixel::{Blend, BlendMode};
use layout::*;

const S_GPU: u64 = 1;
const S_BLOCK: u64 = 2;

struct Line([u8; 384], usize);
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
    ($($a:tt)*) => {{
        let mut l = Line([0; 384], 0);
        let _ = core::fmt::write(&mut l, format_args!($($a)*));
        console(0, &l.0[..l.1]);
    }};
}

/// The bake's tables and the NEON thread's scratch: a bump allocator over a zeroed static (bake.rs allocates its tables once).
#[repr(C, align(64))]
struct Heap([u8; 2 << 20]);
static mut HEAP: Heap = Heap([0; 2 << 20]);
static mut HEAP_NEXT: usize = 0;
struct Bump;
unsafe impl core::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, l: core::alloc::Layout) -> *mut u8 {
        unsafe {
            let base = (&raw mut HEAP) as usize;
            let start = (base + HEAP_NEXT + l.align() - 1) & !(l.align() - 1);
            if start + l.size() > base + (2 << 20) {
                return core::ptr::null_mut();
            }
            HEAP_NEXT = start + l.size() - base;
            start as *mut u8
        }
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static ALLOC: Bump = Bump;

/// The block's base in this process's view.
static mut BLOCK: usize = 0;
fn at<T>(off: usize) -> *mut T {
    unsafe { (BLOCK + off) as *mut T }
}
fn px(off: usize, n: usize) -> &'static mut [u64] {
    unsafe { core::slice::from_raw_parts_mut(at::<u64>(off), n) }
}

/// The frame's scroll: the noise's texture rows and the pane's row at the screen's top.
const SCROLL: u32 = 37;
const OFFSET: u32 = 517;

// ---- NEON ----

/// fluor's `under` (pixel.rs `under_x8_normal_g2t16`, the same arithmetic) four G2t16 pixels at a time: `d` the composite so far, `s` the layer under it. vld2 splits each pixel's words: the low (G << 16 | B) and the high (α << 16 | R).
#[inline(always)]
unsafe fn under4(d: *mut u64, s: *const u64) {
    use core::arch::aarch64::*;
    unsafe {
        let dv = vld2q_u32(d as *const u32);
        let sv = vld2q_u32(s as *const u32);
        let f = vdupq_n_u32(0xFFFF);
        let da = vshrq_n_u32::<16>(dv.1);
        let cons = vshrq_n_u32::<16>(vmulq_u32(vsubq_u32(vdupq_n_u32(1 << 16), da), vshrq_n_u32::<16>(sv.1)));
        let nr = vaddq_u32(vandq_u32(dv.1, f), vshrq_n_u32::<16>(vmulq_u32(vandq_u32(sv.1, f), cons)));
        let ng = vaddq_u32(vshrq_n_u32::<16>(dv.0), vshrq_n_u32::<16>(vmulq_u32(vshrq_n_u32::<16>(sv.0), cons)));
        let nb = vaddq_u32(vandq_u32(dv.0, f), vshrq_n_u32::<16>(vmulq_u32(vandq_u32(sv.0, f), cons)));
        let na = vaddq_u32(da, cons);
        let hi = vorrq_u32(vshlq_n_u32::<16>(na), nr);
        let lo = vorrq_u32(vshlq_n_u32::<16>(ng), nb);
        let op = vceqq_u32(da, f);
        vst2q_u32(d as *mut u32, uint32x4x2_t(vbslq_u32(op, dv.0, lo), vbslq_u32(op, dv.1, hi)));
    }
}

fn under_row(d: &mut [u64], s: &[u64]) {
    let n = d.len();
    let mut i = 0;
    while i + 4 <= n {
        // SAFETY: i + 4 ≤ n, both rows n long.
        unsafe { under4(d.as_mut_ptr().add(i), s.as_ptr().add(i)) };
        i += 4;
    }
    while i < n {
        d[i] = d[i].under(s[i], BlendMode::Normal);
        i += 1;
    }
}

/// The stages a NEON task runs over its rows, each over columns [x0, x1).
const LAYERS: u8 = 1;
const NOISE: u8 = 2;
const BAKE: u8 = 4;
const PRESENT: u8 = 8;

#[derive(Clone, Copy)]
struct Task {
    /// Which threads take part (bit j thread j).
    who: u8,
    stages: u8,
    y0: usize,
    rows: usize,
    x0: usize,
    x1: usize,
    /// Where the bake writes (the NEON surface, or the GPU's output for a patch).
    dest: usize,
}

/// Rows a thread claims at a time.
const CHUNK: usize = 1 << 4;

/// The stages of `t` over rows [ya, yb).
fn run_rows(t: &Task, ya: usize, yb: usize) {
    let (x0, x1) = (t.x0, t.x1);
    let canvas = px(CANVAS_AT, W * H);
    if t.stages & LAYERS != 0 {
        let pane = px(PANE_AT, W * PANE_ROWS);
        let chrome = px(CHROME_AT, W * CHROME_ROWS);
        for y in ya..yb {
            let row = &mut canvas[y * W + x0..y * W + x1];
            if y < CHROME_ROWS {
                row.copy_from_slice(&chrome[y * W + x0..y * W + x1]);
            } else {
                row.fill(0);
            }
            let p = (y + OFFSET as usize) * W;
            under_row(row, &pane[p + x0..p + x1]);
        }
    }
    if t.stages & NOISE != 0 {
        let mut d = Damage::new();
        // SAFETY of the sharing: each thread's noise writes only its clip's rows (noise.rs `lay`: `canvas.pixels[ys * w..ye * w]`), and the threads' rows never overlap (`CLAIM`).
        let all = unsafe { core::slice::from_raw_parts_mut(at::<u64>(CANVAS_AT), W * H) };
        fluor::paint::noise_under(&mut Canvas::new(all, W, H, &mut d), AXIS, SCROLL as isize, true, Some(Clip::new(x0, ya, x1, yb)), None);
    }
    if t.stages & BAKE != 0 {
        let dest = px(t.dest, W * H);
        let b = bake::now();
        for y in ya..yb {
            b.g2t_row(&canvas[y * W + x0..y * W + x1], &mut dest[y * W + x0..y * W + x1]);
        }
    }
    if t.stages & PRESENT != 0 {
        let src = px(t.dest, W * H);
        let cm = neon::Chans::new(7);
        // The display presents whole blocks of 16 (a row's tail padded): the rect's columns out to them.
        let (xs, xe) = (x0 & !15, (x1 + 15) & !15);
        for y in ya..yb {
            for k in 0..4 {
                let (t0, t1) = neon::row_thresholds_at(y, k, xs);
                let dst = at::<u32>(PHASES_AT + k * PHASE_BYTES);
                let mut x = xs;
                while x < xe {
                    // SAFETY: x + 16 ≤ xe ≤ W in row y < H of a full-screen surface and phase.
                    unsafe { neon::present_app16(src.as_ptr().add(y * W + x), dst.add(y * W + x), t0, t1, cm) };
                    x += 16;
                }
            }
        }
    }
}

/// The workers: four on the A715s (cores 4-7), one on the X3 (core 8).
const THREADS: usize = 5;
const CORES: [u64; THREADS] = [4, 5, 6, 7, 8];
const A715_1: u8 = 1;
const A715_4: u8 = 15;
const X3: u8 = 16;
const X3_A715_3: u8 = 16 | 7;
static GO: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static CLAIM: AtomicU64 = AtomicU64::new(0);
static mut TASK: Task = Task { who: 0, stages: 0, y0: 0, rows: 0, x0: 0, x1: 0, dest: 0 };
/// Each thread's cycles and counter ticks in its last task.
static CYC: [AtomicU64; THREADS] = [const { AtomicU64::new(0) }; THREADS];
static TICKS: [AtomicU64; THREADS] = [const { AtomicU64::new(0) }; THREADS];
static mut STACKS: [[u8; 1 << 17]; THREADS] = [[0; 1 << 17]; THREADS];

fn pin_here(core: u64) {
    ferros_sys::pin(1 << core);
    // The pin takes at the next reschedule (pmu_timing_pin_x3).
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
}

extern "C" fn worker(j: u64) -> ! {
    pin_here(CORES[j as usize]);
    let mut seen = 0;
    loop {
        let g = GO.load(Ordering::Acquire);
        if g == seen {
            ferros_sys::wait(&GO, g, 0);
            continue;
        }
        seen = g;
        let t = unsafe { TASK };
        if t.who >> j & 1 == 0 {
            continue;
        }
        let (c0, t0) = (ferros_sys::pmu::cycles(), ferros_sys::now());
        loop {
            let r = CLAIM.fetch_add(CHUNK as u64, Ordering::AcqRel) as usize;
            if r >= t.rows {
                break;
            }
            // WHY the min: the last chunk is what is left of the task's rows.
            run_rows(&t, t.y0 + r, t.y0 + (r + CHUNK).min(t.rows));
        }
        CYC[j as usize].store(ferros_sys::pmu::cycles() - c0, Ordering::Relaxed);
        TICKS[j as usize].store(ferros_sys::now() - t0, Ordering::Relaxed);
        DONE.fetch_add(1, Ordering::AcqRel);
        ferros_sys::wake(&DONE, 1);
    }
}

/// `t` run by its threads; the wall time from the dispatch to the last one done, ticks.
fn dispatch(t: Task) -> u64 {
    let n = t.who.count_ones() as u64;
    let t0 = ferros_sys::now();
    unsafe { TASK = t };
    CLAIM.store(0, Ordering::Release);
    DONE.store(0, Ordering::Release);
    GO.fetch_add(1, Ordering::AcqRel);
    ferros_sys::wake(&GO, THREADS as u64);
    loop {
        let d = DONE.load(Ordering::Acquire);
        if d >= n {
            break;
        }
        ferros_sys::wait(&DONE, d, 0);
    }
    ferros_sys::now() - t0
}

/// The busiest participant's own time in its last task (its compute, no wake), ticks; and its clock, MHz (cycles over time: which core it ran on).
fn inner(who: u8) -> (u64, u64) {
    let mut best = (0, 0);
    for j in 0..THREADS {
        if who >> j & 1 == 1 {
            let t = TICKS[j].load(Ordering::Relaxed);
            if t > best.0 {
                best = (t, CYC[j].load(Ordering::Relaxed) * ferros_sys::freq() / t.max(1) / 1_000_000);
            }
        }
    }
    best
}

fn us(t: u64) -> u64 {
    t * 1_000_000 / ferros_sys::freq()
}

/// Least, mean and most of `v`, µs.
fn spread(v: &[u64]) -> (u64, u64, u64) {
    let (mut lo, mut hi, mut sum) = (u64::MAX, 0, 0);
    for &x in v {
        lo = lo.min(x);
        hi = hi.max(x);
        sum += x;
    }
    (us(lo), us(sum / v.len().max(1) as u64), us(hi))
}

const FRAME: Task = Task { who: 0, stages: 0, y0: 0, rows: H, x0: 0, x1: W, dest: BAKED_AT };

/// A NEON frame task `n` times: the wall (dispatch to done) and the busiest thread's own time, least/mean/most µs, and its clock.
fn neon_runs(t: Task, n: usize) -> ((u64, u64, u64), (u64, u64, u64), u64) {
    let (mut wall, mut own) = ([0u64; 16], [0u64; 16]);
    let mut mhz = 0;
    dispatch(t);
    for i in 0..n {
        wall[i] = dispatch(t);
        let (o, m) = inner(t.who);
        own[i] = o;
        mhz = m;
    }
    (spread(&wall[..n]), spread(&own[..n]), mhz)
}

// ---- GPU ----

struct Gpu {
    q: Queue,
    handles: [u8; KERNELS],
}
const K_NOISE: usize = 0;
const K_G2T: usize = 1;
const K_BAKED: usize = 2;
const K_PHASES: usize = 3;
const K_CLIP: usize = 4;
/// The parallel noise's kernels (kernel.rs `record_par`, 2026-11-21): the noise alone at workgroups of 2^6, 2^7 and 2^8, the compose to G2t16, baked and the phases, the clip; and the memory floor (a pane copied).
const K_NOISE_P6: usize = 5;
const K_NOISE_P8: usize = 7;
const K_G2T_P: usize = 8;
const K_BAKED_P: usize = 9;
const K_PHASES_P: usize = 10;
const K_CLIP_P: usize = 11;
const K_COPY: usize = 12;
const KERNELS: usize = 13;
fn kernel(i: usize) -> &'static kernels::Kernel {
    [&kernels::NOISE, &kernels::G2T, &kernels::BAKED, &kernels::PHASES, &kernels::CLIP, &kernels::NOISE_P6, &kernels::NOISE_P7, &kernels::NOISE_P8, &kernels::G2T_P, &kernels::BAKED_P, &kernels::PHASES_P, &kernels::CLIP_P, &kernels::COPY][i]
}

fn walk() -> (u64, u64, u64) {
    (widen_mask(fluor::theme::BG_MASK), widen_mask(fluor::theme::BG_SPECKLE), widen_base(fluor::theme::BG_BASE))
}

/// Kernel `k`'s run and workgroups for `run`: the serial kernels' as the run says, the parallel ones a workgroup a row a side (`Run::par`), the copy a warp a 16 pixels of the frame.
fn grid(k: usize, run: &tables::Run) -> (tables::Run, u32) {
    if k == K_COPY {
        (*run, (W * H / 16) as u32)
    } else if kernel(k).wg_log2 > 4 {
        (run.par(), run.par_groups())
    } else {
        (*run, run.groups())
    }
}

impl Gpu {
    /// Kernel `k` over `run`, its output at `out_at`: submitted, waited for; the caller's submit to its wake and the GPU's own time, ticks, and the status.
    fn job(&mut self, k: usize, run: &tables::Run, out_at: usize) -> (u64, u64, u64) {
        let (run, groups) = grid(k, run);
        let mut u = [0u64; 64];
        let n = tables::uniforms(&mut u, &run,ferros_sys::gpu::ARENA_VA, out_at, unsafe { JOB_SCROLL }, OFFSET, walk(), &tables::odt_scaled(ferros_sys::akita()), kernel(k).consts);
        let t0 = ferros_sys::now();
        let Some(j) = self.q.submit(&Job { kernel: self.handles[k], groups: (groups, 1), uniforms: &u[..n] }) else {
            say!("compbench: a submit refused (the queue ended A#{})\n", self.q.ended());
            exit(1);
        };
        let e = self.q.wait(j);
        let wall = ferros_sys::now() - t0;
        if e.status != S_DONE {
            say!("compbench: kernel A#{k} over A#{} groups: status A#{}, GPU address G#{:X}, AS_FAULTSTATUS G#{:X}; the queue ended A#{}, opened again {}\n", groups, e.status, e.fault_address, e.fault_status, self.q.ended(), self.q.reopen());
        }
        (wall, e.gpu_ticks().unwrap_or(0), e.status)
    }

    fn runs(&mut self, k: usize, run: &tables::Run, out_at: usize, n: usize, gap_ms: u64) -> ((u64, u64, u64), (u64, u64, u64)) {
        let (mut wall, mut gpu) = ([0u64; 16], [0u64; 16]);
        if gap_ms == 0 {
            self.job(k, run, out_at);
        }
        for i in 0..n {
            if gap_ms != 0 {
                ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000 * gap_ms);
            }
            let (w, g, s) = self.job(k, run, out_at);
            if s != S_DONE {
                say!("compbench: kernel A#{k} status A#{s}\n");
                exit(1);
            }
            wall[i] = w;
            gpu[i] = g;
        }
        (spread(&wall[..n]), spread(&gpu[..n]))
    }

    /// The service's op 6: the GPU's clock held up (true) or left to the service.
    fn hold(&self, on: bool) -> bool {
        let mut r = [1u8; 1];
        ferros_sys::call(unsafe { GRANTS_GPU }, &[6, on as u8], &mut r);
        r[0] == 0
    }
}
static mut GRANTS_GPU: u64 = 0;

// ---- checks ----

/// The largest difference of any channel (lanes 0-2) between two runs of u64 pixels, and how many pixels differ at all.
fn diff64(a: &[u64], b: &[u64]) -> (u64, usize) {
    let (mut most, mut n) = (0u64, 0usize);
    for (&x, &y) in a.iter().zip(b) {
        if x != y {
            n += 1;
            for c in 0..4 {
                most = most.max(((x >> (16 * c)) & 0xFFFF).abs_diff((y >> (16 * c)) & 0xFFFF));
            }
        }
    }
    (most, n)
}

/// The phases' words against the present of the NEON surface, row by row: the largest code difference and how many words differ.
fn diff_phases() -> (u32, usize) {
    let src = px(BAKED_AT, W * H);
    let cm = neon::Chans::new(7);
    let (mut most, mut n) = (0u32, 0usize);
    let mut row = [0u32; W];
    for y in 0..H {
        for k in 0..4 {
            let (t0, t1) = neon::row_thresholds_at(y, k, 0);
            for x in (0..W).step_by(16) {
                unsafe { neon::present_app16(src.as_ptr().add(y * W + x), row.as_mut_ptr().add(x), t0, t1, cm) };
            }
            let got = unsafe { core::slice::from_raw_parts(at::<u32>(PHASES_AT + k * PHASE_BYTES + 4 * y * W), W) };
            for (&g, &w) in got.iter().zip(&row) {
                if g != w {
                    n += 1;
                    for sh in [4u32, 14, 24] {
                        most = most.max(((g >> sh) & 0xFF).abs_diff((w >> sh) & 0xFF));
                    }
                }
            }
        }
    }
    (most, n)
}

// ---- entry ----

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
    main = sym compbench_main
);

fn byte(input: u64, len: u64, i: u64) -> Option<u8> {
    (i < len).then(|| unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + i) as *const u8) })
}

extern "C" fn compbench_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    ferros_sys::panic_grants(grants);
    let mode = byte(input, input_len, 0).unwrap_or(0);
    let seconds = byte(input, input_len, 1).map_or(1 << 4, |s| s as u64);
    unsafe { GRANTS_GPU = cap(grants, S_GPU) };
    if ferros_sys::gpu_block_ask(cap(grants, S_BLOCK), BLOCK_LOG2).is_none() {
        say!("compbench: the GPU block did not grow to A#{} MiB\n", 1u64 << (BLOCK_LOG2 - 20));
        exit(1);
    }
    let base = ferros_sys::dma(cap(grants, S_BLOCK));
    if base == ferros_sys::ERR || base == 0 {
        say!("compbench: no GPU block\n");
        exit(1);
    }
    unsafe { BLOCK = base as usize };
    // The bake: the display's, white two stops under the E peak.
    bake::set(0.25, 1.);
    let akita = ferros_sys::akita();
    let k = tables::gain(0.25);
    let t0 = ferros_sys::now();
    let light = unsafe { core::slice::from_raw_parts_mut(at::<u16>(LIGHT_AT), 1 << 16) };
    for (c, e) in light.iter_mut().enumerate() {
        *e = tables::light_entry(k, c, odt::f32_to_f16);
    }
    let code = unsafe { core::slice::from_raw_parts_mut(at::<f32>(CODE_AT), CODE_STEPS + 2) };
    for (i, e) in code.iter_mut().enumerate() {
        *e = tables::code_entry(i, akita);
    }
    for y in 0..16 {
        for x in 0..16 {
            unsafe { *at::<u8>(BLUE_AT + 16 * y + x) = tables::BLUE[y][x] };
        }
    }
    let seeds = px(SEED_AT, H);
    for (t, s) in seeds.iter_mut().enumerate() {
        *s = seed(t, H);
    }
    let chrome = px(CHROME_AT, W * CHROME_ROWS);
    for y in 0..CHROME_ROWS {
        for x in 0..W {
            chrome[y * W + x] = chrome_px(x, y);
        }
    }
    let pane = px(PANE_AT, W * PANE_ROWS);
    for r in 0..PANE_ROWS {
        for x in 0..W {
            pane[r * W + x] = pane_px(x, r);
        }
    }
    say!("compbench: block A#{} MiB at G#{:X}, the frame's inputs made in A#{} ms\n", 1u64 << (BLOCK_LOG2 - 20), base, us(ferros_sys::now() - t0) / 1000);
    let q = match Queue::open(cap(grants, S_GPU), cap(grants, S_BLOCK), Q_AT, 2) {
        Ok(q) => q,
        Err(e) => {
            say!("compbench: the queue did not open (G#{e:X})\n");
            exit(1);
        }
    };
    let mut handles = [0u8; KERNELS];
    for (i, h) in handles.iter_mut().enumerate() {
        let kk = kernel(i);
        let w = px(K_AT + i * K_ROOM, kk.words.len());
        w.copy_from_slice(kk.words);
        let preload = ferros_sys::gpu::PRELOAD_X | if kk.regs64 { ferros_sys::gpu::PRELOAD_REGS64 } else { 0 };
        let loaded = if kk.wg_log2 > 4 {
            // A parallel kernel: its workgroup and its shared storage, which the GPU keeps in this block at SHARED_AT (layout.rs).
            let r = q.kernel_group(K_AT + i * K_ROOM, kk.words.len(), preload, kk.wg_log2 as u8, kk.shared_bytes.trailing_zeros() as u8, SHARED_AT, SHARED_ROOM_LOG2);
            if let Some((_, inst)) = r {
                say!("compbench: kernel A#{i} (workgroup 2^A#{}, A#{} B shared) loaded, A#{} workgroups a core\n", kk.wg_log2, kk.shared_bytes, 1u32 << inst);
            }
            r.map(|x| x.0)
        } else {
            q.kernel(K_AT + i * K_ROOM, kk.words.len(), preload)
        };
        match loaded {
            Some(x) => *h = x,
            None => {
                say!("compbench: kernel A#{i} not loaded\n");
                exit(1);
            }
        }
    }
    let mut gpu = Gpu { q, handles };
    for j in 0..THREADS {
        let stack = unsafe { &mut *(&raw mut STACKS[j]) };
        ferros_sys::thread_spawn(worker, stack, j as u64);
    }
    // This thread on the X3 (the display's render thread's core): the small patches run here inline.
    pin_here(8);
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 100);
    match mode {
        1..=4 | 14 | 15 => sustained(&mut gpu, mode, seconds),
        5 => clip_probe(&mut gpu),
        6..=9 => clip_sequence(&mut gpu, mode - 6),
        10 => uniforms_probe(&mut gpu),
        11 => patch_probe(&mut gpu),
        12 => patch_sources(&mut gpu),
        13 => data_probe(&mut gpu),
        _ => everything(&mut gpu),
    }
}

/// The frame at 120 a second for `seconds` (the power rig reads the draw meanwhile): 1 NEON (four A715s make and bake it, the X3 and three A715s present it), 2 the GPU makes and bakes it and NEON presents it, 3 the GPU makes its phases' words, 4 nothing.
fn sustained(gpu: &mut Gpu, mode: u8, seconds: u64) -> ! {
    let f = ferros_sys::freq();
    let period = f / 120;
    let mut frames = 0u64;
    let (mut late, mut busy) = (0u64, 0u64);
    let start = ferros_sys::now();
    let mut next = start;
    while ferros_sys::now() < start + seconds * f {
        frames += 1;
        let t0 = ferros_sys::now();
        match mode {
            1 => {
                dispatch(Task { who: A715_4, stages: LAYERS | NOISE | BAKE, ..FRAME });
                dispatch(Task { who: X3_A715_3, stages: PRESENT, ..FRAME });
            }
            2 => {
                gpu.job(K_BAKED, &tables::Run::frame(), BAKED_AT);
                dispatch(Task { who: X3_A715_3, stages: PRESENT, ..FRAME });
            }
            3 => {
                gpu.job(K_PHASES, &tables::Run::frame(), BAKED_AT);
            }
            // The parallel noise's kernels (2026-11-21): 14 as 2, 15 as 3.
            14 => {
                gpu.job(K_BAKED_P, &tables::Run::frame(), BAKED_AT);
                dispatch(Task { who: X3_A715_3, stages: PRESENT, ..FRAME });
            }
            15 => {
                gpu.job(K_PHASES_P, &tables::Run::frame(), BAKED_AT);
            }
            _ => {}
        }
        let t1 = ferros_sys::now();
        busy += t1 - t0;
        next += period;
        if t1 > next {
            late += 1;
            next = t1;
        } else {
            ferros_sys::sleep_until(next);
        }
    }
    let wall = ferros_sys::now() - start;
    say!("compbench sustained mode A#{mode}: A#{frames} frames in A#{} ms, A#{} us a frame busy, A#{late} past their slot\n", us(wall) / 1000, us(busy) / frames.max(1));
    exit(0);
}

fn everything(gpu: &mut Gpu) -> ! {
    let n = 16;
    // ---- NEON, the whole frame ----
    let stages: [(&str, u8, u8); 9] = [
        ("layers (chrome copy, pane under), 4 A715", LAYERS, A715_4),
        ("noise, 4 A715", NOISE, A715_4),
        ("bake, 4 A715", BAKE, A715_4),
        ("present 4 phases, X3 + 3 A715", PRESENT, X3_A715_3),
        ("present 4 phases, X3", PRESENT, X3),
        ("layers+noise+bake fused, 4 A715", LAYERS | NOISE | BAKE, A715_4),
        ("layers+noise+bake fused, 1 A715", LAYERS | NOISE | BAKE, A715_1),
        ("layers+noise+bake fused, X3", LAYERS | NOISE | BAKE, X3),
        ("noise, 1 A715", NOISE, A715_1),
    ];
    for (name, s, who) in stages {
        let reps = if who.count_ones() == 1 && s != PRESENT { 4 } else { n };
        let (w, o, mhz) = neon_runs(Task { who, stages: s, ..FRAME }, reps);
        say!("compbench NEON {name}: wall A#{}/A#{}/A#{} us (least/mean/most of A#{reps}), busiest thread A#{}/A#{}/A#{} us at A#{mhz} MHz\n", w.0, w.1, w.2, o.0, o.1, o.2);
    }
    // The NEON frame as the reference for the GPU's: the canvas (G2t16), its bake, its present.
    dispatch(Task { who: A715_4, stages: LAYERS | NOISE | BAKE, ..FRAME });
    dispatch(Task { who: X3_A715_3, stages: PRESENT, ..FRAME });
    // ---- GPU, the whole frame ----
    let frame = tables::Run::frame();
    for held in [false, true] {
        if held {
            say!("compbench GPU: the clock held up (op 6): {}\n", gpu.hold(true));
        }
        for (name, k) in [("noise alone, G2t16 out", K_NOISE), ("compose, G2t16 out", K_G2T), ("compose + bake, baked out", K_BAKED), ("compose + bake + dither + pack, 4 phases out", K_PHASES)] {
            let (w, g) = gpu.runs(k, &frame, OUT_AT, n, 0);
            say!("compbench GPU {name} ({} words, {} a pass{}): submit to wake A#{}/A#{}/A#{} us, the GPU's own A#{}/A#{}/A#{} us\n", kernel(k).words.len(), kernel(k).loop_words, if kernel(k).regs64 { ", 64 registers" } else { "" }, w.0, w.1, w.2, g.0, g.1, g.2);
        }
        if held {
            gpu.hold(false);
        }
    }
    // ---- GPU, the parallel noise (2026-11-21): a workgroup a half-row, its scan in shared storage; and the memory floor ----
    for held in [false, true] {
        if held {
            say!("compbench GPU parallel: the clock held up (op 6): {}\n", gpu.hold(true));
        }
        for (name, k) in [("par noise alone, workgroup 2^6", K_NOISE_P6), ("par noise alone, workgroup 2^7", K_NOISE_P6 + 1), ("par noise alone, workgroup 2^8", K_NOISE_P8), ("par compose, G2t16 out", K_G2T_P), ("par compose + bake, baked out", K_BAKED_P), ("par compose + bake + dither + pack, 4 phases out", K_PHASES_P), ("the floor: the pane copied, 8 B read and 8 B written a pixel", K_COPY)] {
            let (w, g) = gpu.runs(k, &frame, OUT_AT, n, 0);
            say!("compbench GPU {name} ({} words, workgroup 2^{}, {} B shared): submit to wake A#{}/A#{}/A#{} us, the GPU's own A#{}/A#{}/A#{} us\n", kernel(k).words.len(), kernel(k).wg_log2, kernel(k).shared_bytes, w.0, w.1, w.2, g.0, g.1, g.2);
        }
        if held {
            gpu.hold(false);
        }
    }
    par_check(gpu);
    // The GPU's pixels against NEON's: the compose bit for bit, the bake and the phases by their largest difference.
    gpu.job(K_G2T, &frame, OUT_AT);
    let (m0, n0) = diff64(px(OUT_AT, W * H), px(CANVAS_AT, W * H));
    gpu.job(K_BAKED, &frame, OUT_AT);
    let (m1, n1) = diff64(px(OUT_AT, W * H), px(BAKED_AT, W * H));
    gpu.job(K_PHASES, &frame, OUT_AT);
    let (m2, n2) = diff_phases();
    say!("compbench GPU against NEON: compose A#{n0} of A#{} pixels differ (most A#{m0}); baked A#{n1} differ, the most by A#{m1} of 65535; phases A#{n2} words of A#{} differ, the most by A#{m2} codes\n", W * H, 4 * W * H);
    // ---- small damage: NEON patches against GPU jobs ----
    // The GPU's whole frame in its output, NEON's patches baked into the same buffer (the hybrid: memory is shared).
    gpu.job(K_BAKED, &frame, OUT_AT);
    let patch = |x0: usize, y0: usize, x1: usize, y1: usize, who: u8| -> u64 {
        let t = Task { who, stages: LAYERS | NOISE | BAKE, y0, rows: y1 - y0, x0, x1, dest: OUT_AT };
        if who == 0 {
            let t0 = ferros_sys::now();
            run_rows(&t, y0, y1);
            ferros_sys::now() - t0
        } else {
            dispatch(t);
            inner(who).0
        }
    };
    say!("compbench rects: log2 pixels, w x h, where; NEON on the X3 (inline) and one A715 (its own time): least/mean us; the GPU thru calls back to back and after 2^6 ms idle: call to reply mean us (its own); NEON's present of the rect on the X3 after either\n");
    for lg in 4..=20u32 {
        let (w, h) = (1usize << lg.div_ceil(2), 1usize << (lg / 2));
        for far in [false, true] {
            let (x0, y0) = if far { (W - 16 - w, 1000) } else { (AXIS - w / 2, 1000) };
            let (x1, y1) = (x0 + w, y0 + h);
            let reps = if lg <= 14 { 8 } else { 4 };
            let (mut a, mut b) = ([0u64; 8], [0u64; 8]);
            for i in 0..reps {
                a[i] = patch(x0, y0, x1, y1, 0);
                b[i] = patch(x0, y0, x1, y1, A715_1);
            }
            let (na, nb) = (spread(&a[..reps]), spread(&b[..reps]));
            let run = tables::Run::rect(x0, y0, x1, y1);
            for y in y0..y1 {
                px(OUT_AT, W * H)[y * W + x0..y * W + x1].fill(0x5555_5555_5555_5555);
            }
            let (gw, gg) = call_runs(K_CLIP, &run, reps, 0);
            let mut left = 0usize;
            for y in y0..y1 {
                left += px(OUT_AT, W * H)[y * W + x0..y * W + x1].iter().filter(|&&p| p == 0x5555_5555_5555_5555).count();
            }
            if left != 0 {
                say!("compbench rect 2^A#{lg}: A#{left} of its pixels not written by the GPU\n");
            }
            let (iw, ig) = call_runs(K_CLIP, &run, 4, 1 << 6);
            // The parallel clip thru the queue, warm (2026-11-21).
            let (pw, pg) = gpu.runs(K_CLIP_P, &run, OUT_AT, reps, 0);
            let t0 = ferros_sys::now();
            run_rows(&Task { who: 0, stages: PRESENT, y0, rows: h, x0, x1, dest: OUT_AT }, y0, y1);
            let pres = us(ferros_sys::now() - t0);
            say!("compbench rect 2^A#{lg} ({w}x{h} {}): NEON X3 A#{}/A#{} us, A715 A#{}/A#{} us; GPU warm A#{} us (A#{}), idle A#{} us (A#{}); GPU par warm A#{} us (A#{}); present A#{pres} us\n", if far { "far" } else { "near" }, na.0, na.1, nb.0, nb.1, gw.1, gg.1, iw.1, ig.1, pw.1, pg.1);
        }
    }
    // A cursor blink (4 × 64, at the text's place) and a hover (a 256 × 96 button), each both ways.
    for (name, x0, y0, w, h) in [("cursor blink 4x64", 1100usize, 1500usize, 4usize, 64usize), ("hover 256x96", 200, 800, 256, 96)] {
        let (x1, y1) = (x0 + w, y0 + h);
        let (mut a, mut b) = ([0u64; 8], [0u64; 8]);
        for i in 0..8 {
            a[i] = patch(x0, y0, x1, y1, 0);
            b[i] = patch(x0, y0, x1, y1, A715_1);
        }
        let (na, nb) = (spread(&a), spread(&b));
        let run = tables::Run::rect(x0, y0, x1, y1);
        let (gw, gg) = call_runs(K_CLIP, &run, 8, 0);
        let (iw, ig) = call_runs(K_CLIP, &run, 4, 1 << 6);
        say!("compbench {name}: NEON X3 A#{}/A#{}/A#{} us, A715 A#{}/A#{}/A#{} us; GPU warm A#{} us (own A#{}), idle A#{} us (own A#{})\n", na.0, na.1, na.2, nb.0, nb.1, nb.2, gw.1, gg.1, iw.1, ig.1);
    }
    // ---- the handoff ----
    // NEON's patch and the GPU's job of the same pixels: how far apart.
    let (x0, y0, x1, y1) = (200usize, 800usize, 456usize, 896usize);
    // A mark first, so a GPU job that wrote nothing shows as every pixel differing.
    for y in y0..y1 {
        px(OUT_AT, W * H)[y * W + x0..y * W + x1].fill(0x5555_5555_5555_5555);
    }
    call_job(K_CLIP, &tables::Run::rect(x0, y0, x1, y1), OUT_AT);
    let keep = px(KEEP_AT, (x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        keep[(y - y0) * (x1 - x0)..(y - y0 + 1) * (x1 - x0)].copy_from_slice(&px(OUT_AT, W * H)[y * W + x0..y * W + x1]);
    }
    patch(x0, y0, x1, y1, 0);
    let mut most = 0u64;
    let mut nd = 0usize;
    for y in y0..y1 {
        let (m, c) = diff64(&px(OUT_AT, W * H)[y * W + x0..y * W + x1], &keep[(y - y0) * (x1 - x0)..(y - y0 + 1) * (x1 - x0)]);
        most = most.max(m);
        nd += c;
    }
    say!("compbench handoff: NEON's patch against the GPU's job of the same A#{} pixels: A#{nd} differ, the most by A#{most} of 65535\n", (x1 - x0) * (y1 - y0));
    // A GPU job over pixels that share cache lines with a NEON patch made after the GPU's last job: whether the GPU writes back stale bytes over the patch. The patch's columns end mid-line (a 64-byte line holds 8 pixels): NEON patches [1001, 1005) after a GPU frame, then the GPU writes [1005, 1013), the same lines, with a different scroll so the two differ wherever they overlap.
    gpu.job(K_BAKED, &frame, OUT_AT);
    let (px0, px1, py0, py1) = (1001usize, 1005usize, 1500usize, 1564usize);
    for pass in 0..2 {
        let sc = 1 + pass as u32 * 2;
        // NEON's patch at another scroll: its pixels differ from the GPU's frame's.
        unsafe { SCROLL_PATCH = sc };
        patch_scrolled(px0, py0, px1, py1);
        let keep = px(KEEP_AT, (px1 - px0) * (py1 - py0));
        for y in py0..py1 {
            keep[(y - py0) * 4..(y - py0 + 1) * 4].copy_from_slice(&px(OUT_AT, W * H)[y * W + px0..y * W + px1]);
        }
        let (w, _, s) = call_job(K_CLIP, &tables::Run::rect(1005, py0, 1013, py1), OUT_AT);
        let mut clobbered = 0;
        for y in py0..py1 {
            for x in px0..px1 {
                if px(OUT_AT, W * H)[y * W + x] != keep[(y - py0) * 4 + x - px0] {
                    clobbered += 1;
                }
            }
        }
        say!("compbench handoff pass A#{pass}: NEON patched A#{} pixels mid-line after the GPU's frame, then a GPU job (status A#{s}, A#{} us) wrote the rest of their lines: A#{clobbered} of the patch's pixels changed under it\n", (px1 - px0) * (py1 - py0), us(w));
    }
    say!("compbench: done\n");
    exit(0);
}

/// Mode 5: what the clip kernel writes on the device (its GPU time was 15 us for every rect: too little for the walk). Each rect marked, the kernel run, then the rows and columns it wrote and how its pixels compare with the whole-frame kernel's; then the clip kernel over the whole frame against the whole-frame kernel.
fn clip_probe(gpu: &mut Gpu) -> ! {
    let frame = tables::Run::frame();
    gpu.job(K_BAKED, &frame, BAKED_AT);
    let reference = px(BAKED_AT, W * H);
    for (x0, y0, x1, y1) in [(1072usize, 1000usize, 1328usize, 1256usize), (200, 800, 456, 896), (0, 0, W, H), (672, 1000, 688, 1016)] {
        let out = px(OUT_AT, W * H);
        for y in y0..y1 {
            out[y * W + x0..y * W + x1].fill(0x5555_5555_5555_5555);
        }
        let run = tables::Run::rect(x0, y0, x1, y1);
        let (w, g, s) = gpu.job(K_CLIP, &run, OUT_AT);
        let (mut written, mut rows, mut far, mut near, mut wrong) = (0usize, 0usize, 0usize, W, 0usize);
        for y in y0..y1 {
            let mut any = false;
            for x in x0..x1 {
                let p = out[y * W + x];
                if p != 0x5555_5555_5555_5555 {
                    written += 1;
                    any = true;
                    far = far.max(x);
                    near = near.min(x);
                    if p != reference[y * W + x] {
                        wrong += 1;
                    }
                }
            }
            rows += any as usize;
        }
        say!("compbench clip probe {x0},{y0} to {x1},{y1}: A#{} groups, status A#{s}, A#{} us (GPU A#{} us): A#{written} of A#{} written in A#{rows} rows, columns A#{near} to A#{far}; A#{wrong} written differ from the whole-frame kernel\n", run.groups(), us(w), us(g), (x1 - x0) * (y1 - y0));
    }
    exit(0);
}

/// Mode 6: the clip kernel wrote everything in mode 5 and nothing inside mode 0's run. Here it runs 2^6 times over the hover's rect, each time after one of the other kernels' whole frames (none, noise, compose, baked, phases in turn), each run's pixels counted: which neighbour, or which count, leaves it writing nothing.
fn clip_sequence(gpu: &mut Gpu, how: u8) -> ! {
    // how: bit 0 the clock held and let go first (op 6, as mode 0 does before its rects), bit 1 a NEON patch of the rect on this thread and one A715 before each GPU job (as mode 0 times them).
    if how & 1 != 0 {
        say!("compbench clip sequence: held {}, let go {}\n", gpu.hold(true), gpu.hold(false));
    }
    let (x0, y0, x1, y1) = (200usize, 800usize, 456usize, 896usize);
    let run = tables::Run::rect(x0, y0, x1, y1);
    let frame = tables::Run::frame();
    let mut line = [0u8; 64];
    for i in 0..64 {
        let before = [usize::MAX, K_NOISE, K_G2T, K_BAKED, K_PHASES][i % 5];
        if before != usize::MAX {
            gpu.job(before, &frame, BAKED_AT);
        }
        if how & 2 != 0 {
            let t = Task { who: 0, stages: LAYERS | NOISE | BAKE, y0, rows: y1 - y0, x0, x1, dest: OUT_AT };
            run_rows(&t, y0, y1);
            dispatch(Task { who: A715_1, ..t });
        }
        let out = px(OUT_AT, W * H);
        for y in y0..y1 {
            out[y * W + x0..y * W + x1].fill(0x5555_5555_5555_5555);
        }
        let (_, g, s) = gpu.job(K_CLIP, &run, OUT_AT);
        let mut written = 0usize;
        for y in y0..y1 {
            written += out[y * W + x0..y * W + x1].iter().filter(|&&p| p != 0x5555_5555_5555_5555).count();
        }
        line[i] = if s != S_DONE { b'F' } else if written == 0 { b'0' } else if written == (x1 - x0) * (y1 - y0) { b'W' } else { b'p' };
        if i < 10 || written != (x1 - x0) * (y1 - y0) {
            say!("compbench clip sequence A#{i} (after kernel A#{}): status A#{s}, GPU A#{} us, A#{written} written\n", before as i64, us(g));
        }
    }
    say!("compbench clip sequence: {}\n", core::str::from_utf8(&line).unwrap_or("?"));
    exit(0);
}

/// Kernel `k` over `run` thru a call (op 1: its uniforms copied by the service into its own descriptor page, which the GPU maps non-cacheable), not the queue: the caller's call to its reply and the GPU's own time, ticks, and the status.
fn call_job(k: usize, run: &tables::Run, out_at: usize) -> (u64, u64, u64) {
    let kk = kernel(k);
    let mut u = [0u64; 64];
    let n = tables::uniforms(&mut u, run, ferros_sys::gpu::ARENA_VA, out_at, unsafe { JOB_SCROLL }, OFFSET, walk(), &tables::odt_scaled(ferros_sys::akita()), kk.consts);
    let preload = ferros_sys::gpu::PRELOAD_X | if kk.regs64 { ferros_sys::gpu::PRELOAD_REGS64 } else { 0 };
    let mut msg = [0u8; 18 + 8 * 64];
    msg[0] = 1;
    msg[1..3].copy_from_slice(&preload.to_le_bytes());
    msg[3..7].copy_from_slice(&((K_AT + k * K_ROOM) as u32).to_le_bytes());
    msg[7..9].copy_from_slice(&(kk.words.len() as u16).to_le_bytes());
    msg[9..13].copy_from_slice(&run.groups().to_le_bytes());
    msg[13..17].copy_from_slice(&1u32.to_le_bytes());
    msg[17] = n as u8;
    for i in 0..n {
        msg[18 + 8 * i..26 + 8 * i].copy_from_slice(&u[i].to_le_bytes());
    }
    let mut reply = [0u8; 17];
    let t0 = ferros_sys::now();
    let r = ferros_sys::call(unsafe { GRANTS_GPU }, &msg[..18 + 8 * n], &mut reply);
    let wall = ferros_sys::now() - t0;
    let gpu = if r >= 9 && reply[0] == 0 { u32::from_le_bytes([reply[5], reply[6], reply[7], reply[8]]) as u64 * ferros_sys::freq() / 1_000_000 } else { 0 };
    (wall, gpu, reply[0] as u64)
}

/// Mode 10: two rects in the pattern A, A, B (period 3 against the queue's 4 records, so a record is reused with other uniforms), small jobs only (nothing evicting the GPU's L2 between them), each run's rect counted as written whole (W), partly (p) or not (0); 2^10 jobs thru the queue, then thru calls: the first 48 as letters, the counts, and the submit (or call) to wake, least/mean/most µs.
fn uniforms_probe(gpu: &mut Gpu) -> ! {
    let rects = [(200usize, 800usize, 456usize, 896usize), (1072, 1000, 1328, 1064)];
    for thru_calls in [false, true] {
        let mut line = [0u8; 48];
        let mut counts = [0usize; 4];
        let (mut least, mut sum, mut most) = (u64::MAX, 0u64, 0u64);
        for i in 0..1usize << 10 {
            let (x0, y0, x1, y1) = rects[(i % 3 == 2) as usize];
            let out = px(OUT_AT, W * H);
            for y in y0..y1 {
                out[y * W + x0..y * W + x1].fill(0x5555_5555_5555_5555);
            }
            let run = tables::Run::rect(x0, y0, x1, y1);
            let (w, _, s) = if thru_calls { call_job(K_CLIP, &run, OUT_AT) } else { gpu.job(K_CLIP, &run, OUT_AT) };
            let mut written = 0usize;
            for y in y0..y1 {
                written += out[y * W + x0..y * W + x1].iter().filter(|&&p| p != 0x5555_5555_5555_5555).count();
            }
            let c = if s != 0 { 3 } else if written == 0 { 1 } else if written == (x1 - x0) * (y1 - y0) { 0 } else { 2 };
            counts[c] += 1;
            if i < line.len() {
                line[i] = b"W0pF"[c];
            }
            (least, sum, most) = (least.min(w), sum + w, most.max(w));
        }
        say!("compbench uniforms probe thru {}: {}...; of A#1024: A#{} W, A#{} nothing, A#{} part, A#{} failed; submit to wake A#{}/A#{}/A#{} us\n", if thru_calls { "calls" } else { "the queue" }, core::str::from_utf8(&line).unwrap_or("?"), counts[0], counts[1], counts[2], counts[3], us(least), us(sum >> 10), us(most));
    }
    exit(0);
}

/// Mode 11: the handoff of mode 0's end, 2^6 times thru calls and 2^6 thru the queue: the GPU's whole baked frame, then NEON's patch of columns [1001, 1005) at another scroll (ending mid-line: a 64-byte line holds 8 pixels), then a GPU clip job over [1005, 1013) of the same rows, the rest of those lines; the patch's pixels that changed under it counted (64 of 256 each pass when first run, 2026-11-21: the test's own copy of the patch overwritten by the clip's sink, layout.rs `KEEP_AT`; 0 since).
fn patch_probe(gpu: &mut Gpu) -> ! {
    let frame = tables::Run::frame();
    let (px0, px1, py0, py1) = (1001usize, 1005usize, 1500usize, 1564usize);
    let clip = tables::Run::rect(1005, py0, 1013, py1);
    for thru_calls in [true, false] {
        let (mut passes, mut hit, mut total, mut most) = (0usize, 0usize, 0usize, 0usize);
        for pass in 0..1usize << 6 {
            gpu.job(K_BAKED, &frame, OUT_AT);
            unsafe { SCROLL_PATCH = 1 + 2 * (pass as u32 & 1) };
            patch_scrolled(px0, py0, px1, py1);
            let keep = px(KEEP_AT, (px1 - px0) * (py1 - py0));
            for y in py0..py1 {
                keep[(y - py0) * 4..(y - py0 + 1) * 4].copy_from_slice(&px(OUT_AT, W * H)[y * W + px0..y * W + px1]);
            }
            let (_, _, s) = if thru_calls { call_job(K_CLIP, &clip, OUT_AT) } else { gpu.job(K_CLIP, &clip, OUT_AT) };
            let mut clobbered = 0usize;
            for y in py0..py1 {
                for x in px0..px1 {
                    clobbered += (px(OUT_AT, W * H)[y * W + x] != keep[(y - py0) * 4 + x - px0]) as usize;
                }
            }
            passes += 1;
            hit += (clobbered != 0 || s != 0) as usize;
            total += clobbered;
            most = most.max(clobbered);
            if s != 0 || pass < 2 {
                say!("compbench patch probe thru {} pass A#{pass}: status A#{s}, A#{clobbered} of the patch's A#256 changed under it\n", if thru_calls { "calls" } else { "the queue" });
            }
        }
        say!("compbench patch probe thru {}: A#{hit} of A#{passes} passes with a patch pixel changed, A#{total} pixels in all, the most in a pass A#{most}\n", if thru_calls { "calls" } else { "the queue" });
    }
    exit(0);
}

static mut SCROLL_PATCH: u32 = 0;
/// The noise scroll a GPU job is given (mode 12 gives the clip another, so its pixels differ from the frame's).
static mut JOB_SCROLL: u32 = SCROLL;

/// The patch at `SCROLL_PATCH`'s noise scroll (its own texture rows), on this thread: a patch that differs from the GPU's frame wherever both write.
fn patch_scrolled(x0: usize, y0: usize, x1: usize, y1: usize) {
    let canvas = px(CANVAS_AT, W * H);
    let pane = px(PANE_AT, W * PANE_ROWS);
    for y in y0..y1 {
        let row = &mut canvas[y * W + x0..y * W + x1];
        row.fill(0);
        let p = (y + OFFSET as usize) * W;
        under_row(row, &pane[p + x0..p + x1]);
    }
    let mut d = Damage::new();
    let all = px(CANVAS_AT, W * H);
    fluor::paint::noise_under(&mut Canvas::new(all, W, H, &mut d), AXIS, unsafe { SCROLL_PATCH } as isize, true, Some(Clip::new(x0, y0, x1, y1)), None);
    let dest = px(OUT_AT, W * H);
    for y in y0..y1 {
        bake::now().g2t_row(&px(CANVAS_AT, W * H)[y * W + x0..y * W + x1], &mut dest[y * W + x0..y * W + x1]);
    }
}

/// Mode 12: where the bytes a GPU job writes back over a NEON patch come from (mode 11 still saw 64 of 256 after the end flush invalidated the L2, before its copy moved to `KEEP_AT`). Each pass: the lines of columns [1000, 1016) of 64 rows made (`how` below), kept as they were, NEON's patch of [1001, 1005) at its own scroll, then the clip over [1005, 1013) at a third scroll thru calls; every pixel of [1000, 1016) after it classed as unchanged (u), the patch's (P), the value before the patch (b), or other (x), as a row's 16 letters (row 0), and counts over the 16 passes.
/// how 0: the GPU's whole frame wrote the lines (mode 11's case); 1: NEON wrote them (a mark), no GPU job since the last pass's at other rows; 2: the GPU's frame wrote them, then a GPU frame into another buffer (BAKED_AT, 25 MB: the L2 evicted); 3: the GPU's frame, then the CPU rewrote every pixel of the lines with the frame's own value (the CPU's lines all dirty).
fn patch_sources(gpu: &mut Gpu) -> ! {
    let frame = tables::Run::frame();
    let (c0, c1) = (1000usize, 1016usize);
    let (px0, px1) = (1001usize, 1005usize);
    for how in 0..4u32 {
        let mut counts = [[0usize; 4]; 16];
        let mut first = [0u8; 16];
        for pass in 0..16usize {
            let (py0, py1) = (1024 + 64 * pass + 512 * (how as usize & 1), 1088 + 64 * pass + 512 * (how as usize & 1));
            unsafe { JOB_SCROLL = SCROLL };
            match how {
                1 => {
                    for y in py0..py1 {
                        for x in c0..c1 {
                            px(OUT_AT, W * H)[y * W + x] = 0x1234_0000_0000_0000 | (y * W + x) as u64;
                        }
                    }
                }
                _ => {
                    gpu.job(K_BAKED, &frame, OUT_AT);
                }
            }
            if how == 2 {
                gpu.job(K_BAKED, &frame, BAKED_AT);
            }
            if how == 3 {
                for y in py0..py1 {
                    for x in c0..c1 {
                        let v = unsafe { core::ptr::read_volatile(&px(OUT_AT, W * H)[y * W + x]) };
                        unsafe { core::ptr::write_volatile(&mut px(OUT_AT, W * H)[y * W + x], v) };
                    }
                }
            }
            let before = px(KEEP_AT, 2 * 16 * 64);
            for y in py0..py1 {
                before[(y - py0) * 16..(y - py0 + 1) * 16].copy_from_slice(&px(OUT_AT, W * H)[y * W + c0..y * W + c1]);
            }
            unsafe { SCROLL_PATCH = 1 + 2 * (pass as u32 & 1) };
            patch_scrolled(px0, py0, px1, py1);
            for y in py0..py1 {
                let (b, o) = (16 * 64 + (y - py0) * 16, y * W + c0);
                for i in 0..16 {
                    before[b + i] = px(OUT_AT, W * H)[o + i];
                }
            }
            unsafe { JOB_SCROLL = 101 };
            let (_, _, s) = call_job(K_CLIP, &tables::Run::rect(1005, py0, 1013, py1), OUT_AT);
            unsafe { JOB_SCROLL = SCROLL };
            if s != 0 {
                say!("compbench patch sources how A#{how} pass A#{pass}: status A#{s}\n");
            }
            for y in py0..py1 {
                for i in 0..16 {
                    let now = px(OUT_AT, W * H)[y * W + c0 + i];
                    let (pre, patched) = (before[(y - py0) * 16 + i], before[16 * 64 + (y - py0) * 16 + i]);
                    let c = if now == patched { if patched == pre { 0 } else { 1 } } else if now == pre { 2 } else { 3 };
                    counts[i][c] += 1;
                    if pass == 0 && y == py0 {
                        first[i] = b"uPbx"[c];
                    }
                }
            }
        }
        say!("compbench patch sources how A#{how}: row 0 of pass 0 {} (columns 1000-1015)\n", core::str::from_utf8(&first).unwrap_or("?"));
        for (i, c) in counts.iter().enumerate() {
            say!("compbench patch sources how A#{how} column A#{}: u A#{} P A#{} b A#{} x A#{}\n", c0 + i, c[0], c[1], c[2], c[3]);
        }
    }
    exit(0);
}

/// Mode 13: data the CPU writes between GPU jobs, read by the next (mode 10 is the uniforms' case). The clip over one rect, the same uniforms every job, its pane rows written by the CPU before each job as content A (the pane as made) or B (each darkness lane XOR G#1555), in the pattern A, A, B, small jobs only; each job's rect against the references (each content's clip run after a whole frame into another buffer, 32 MiB, which evicts the GPU's L2): its own (R), the other content's (s, stale), or neither (x). 2^10 jobs thru the queue, then thru calls.
fn data_probe(gpu: &mut Gpu) -> ! {
    let (x0, y0, x1, y1) = (1072usize, 1000usize, 1328usize, 1064usize);
    let (w, h) = (x1 - x0, y1 - y0);
    let run = tables::Run::rect(x0, y0, x1, y1);
    let frame = tables::Run::frame();
    let content = |b: bool| {
        let pane = px(PANE_AT, W * PANE_ROWS);
        for y in y0..y1 {
            let r = y + OFFSET as usize;
            for x in x0..x1 {
                let p = pane_px(x, r);
                pane[r * W + x] = if b && p != 0 { p ^ 0x0000_1555_1555_1555 } else { p };
            }
        }
    };
    let refs = px(KEEP_AT, 2 * w * h);
    for b in [false, true] {
        content(b);
        gpu.job(K_BAKED, &frame, BAKED_AT);
        call_job(K_CLIP, &run, OUT_AT);
        for y in y0..y1 {
            refs[b as usize * w * h + (y - y0) * w..b as usize * w * h + (y - y0 + 1) * w].copy_from_slice(&px(OUT_AT, W * H)[y * W + x0..y * W + x1]);
        }
    }
    let differ = (0..w * h).filter(|&i| refs[i] != refs[w * h + i]).count();
    say!("compbench data probe: the two contents' references differ in A#{differ} of A#{} pixels\n", w * h);
    for thru_calls in [false, true] {
        let mut line = [0u8; 48];
        let mut counts = [0usize; 3];
        for i in 0..1usize << 10 {
            let b = i % 3 == 2;
            content(b);
            let (_, _, s) = if thru_calls { call_job(K_CLIP, &run, OUT_AT) } else { gpu.job(K_CLIP, &run, OUT_AT) };
            let (mut own, mut other) = (0usize, 0usize);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (k, v) = ((y - y0) * w + x - x0, px(OUT_AT, W * H)[y * W + x]);
                    own += (v == refs[b as usize * w * h + k]) as usize;
                    other += (v == refs[(!b) as usize * w * h + k]) as usize;
                }
            }
            let c = if s == 0 && own == w * h { 0 } else if s == 0 && other == w * h { 1 } else { 2 };
            counts[c] += 1;
            if i < line.len() {
                line[i] = b"Rsx"[c];
            }
        }
        say!("compbench data probe thru {}: {}...; of A#1024: A#{} right, A#{} the other content's (stale), A#{} neither\n", if thru_calls { "calls" } else { "the queue" }, core::str::from_utf8(&line).unwrap_or("?"), counts[0], counts[1], counts[2]);
    }
    content(false);
    exit(0);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("compbench: panic: {}\n", info);
    exit(0xDEAD);
}

/// `call_job` `n` times (after one unmeasured when back to back, or each after `gap_ms` idle): the call to its reply and the GPU's own time, least/mean/most µs. Calls, not the queue: chosen while the queue's records reused with other uniforms were read stale by the GPU (mode 10; fixed 2026-11-21).
fn call_runs(k: usize, run: &tables::Run, n: usize, gap_ms: u64) -> ((u64, u64, u64), (u64, u64, u64)) {
    let (mut wall, mut gpu) = ([0u64; 16], [0u64; 16]);
    if gap_ms == 0 {
        call_job(k, run, OUT_AT);
    }
    for i in 0..n {
        if gap_ms != 0 {
            ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000 * gap_ms);
        }
        let (w, g, s) = call_job(k, run, OUT_AT);
        if s != 0 {
            say!("compbench: call of kernel A#{k} status A#{s}\n");
            exit(1);
        }
        wall[i] = w;
        gpu[i] = g;
    }
    (spread(&wall[..n]), spread(&gpu[..n]))
}

/// The parallel kernels' pixels on the device against the CPU's (2026-11-21): 2^6 rows spread over the frame, both sides; the noise of each workgroup size bit for bit against noise.rs `reference_row`, the compose bit for bit against the layers (fluor's `under`, as NEON's) over that noise, the bake against the display's own bake of it (bake.rs `g2t_row`) by its largest difference. A barrier or the shared storage wrong on the device shows here as rows that differ.
fn par_check(gpu: &mut Gpu) {
    let frame = tables::Run::frame();
    let (mask, speckle, base) = walk();
    let rows = |i: usize| (i * H / 64 + i * 7) % H;
    let mut half = [0u64; AXIS];
    let mut want = [0u64; W];
    // Row y's parallel noise, the whole row (mirrored about the axis), into `want`.
    let mut noise_row = |y: usize, want: &mut [u64; W]| {
        let t = ((y as isize - SCROLL as isize).rem_euclid(H as isize)) as u32;
        noise::reference_row(t, false, W - AXIS, mask, speckle, base, 0, &mut half);
        want[AXIS..].copy_from_slice(&half[..W - AXIS]);
        noise::reference_row(t, true, AXIS, mask, speckle, base, 0, &mut half);
        for k in 0..AXIS {
            want[AXIS - 1 - k] = half[k];
        }
    };
    for k in [K_NOISE_P6, K_NOISE_P6 + 1, K_NOISE_P8] {
        gpu.job(k, &frame, OUT_AT);
        let (mut wrong, mut rows_off) = (0usize, 0usize);
        for i in 0..64 {
            let y = rows(i);
            noise_row(y, &mut want);
            let got = &px(OUT_AT, W * H)[y * W..(y + 1) * W];
            let d = got.iter().zip(want.iter()).filter(|(a, b)| a != b).count();
            wrong += d;
            rows_off += (d != 0) as usize;
        }
        say!("compbench par check: noise at workgroup 2^A#{}: A#{wrong} of A#{} pixels off the CPU's, in A#{rows_off} of 64 rows\n", kernel(k).wg_log2, 64 * W);
    }
    // The compose: the layers as NEON makes them, the noise under them.
    let mut comp = [0u64; W];
    let mut compose_row = |y: usize, comp: &mut [u64; W]| {
        noise_row(y, &mut want);
        if y < CHROME_ROWS {
            comp.copy_from_slice(&px(CHROME_AT, W * CHROME_ROWS)[y * W..(y + 1) * W]);
        } else {
            comp.fill(0);
        }
        let p = (y + OFFSET as usize) * W;
        under_row(comp, &px(PANE_AT, W * PANE_ROWS)[p..p + W]);
        for x in 0..W {
            comp[x] = if comp[x] == 0 { want[x] } else { comp[x].under(want[x], BlendMode::Normal) };
        }
    };
    gpu.job(K_G2T_P, &frame, OUT_AT);
    let mut wrong = 0usize;
    for i in 0..64 {
        let y = rows(i);
        compose_row(y, &mut comp);
        wrong += px(OUT_AT, W * H)[y * W..(y + 1) * W].iter().zip(comp.iter()).filter(|(a, b)| a != b).count();
    }
    gpu.job(K_BAKED_P, &frame, OUT_AT);
    let (mut most, mut baked_off) = (0u64, 0usize);
    let mut baked = [0u64; W];
    for i in 0..64 {
        let y = rows(i);
        compose_row(y, &mut comp);
        bake::now().g2t_row(&comp, &mut baked);
        let (m, c) = diff64(&px(OUT_AT, W * H)[y * W..(y + 1) * W], &baked);
        most = most.max(m);
        baked_off += c;
    }
    say!("compbench par check: compose A#{wrong} of A#{} pixels off the CPU's; baked A#{baked_off} differ, the most by A#{most} of 65535\n", 64 * W);
}
