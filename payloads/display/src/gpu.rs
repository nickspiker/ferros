//! The compose on the GPU (docs/GPU-COMPOSE.md, step 2, 2026-11-21; the owner: "build the GPU path, whilst we are we can tweak and adjust"): whole frames of today's windows composed, presented and packed into a phase by a GPU job, behind the development switch op 86 ("GPU compose"), off at every start.
//!
//! The display is a GPU client like any other (GPU-COMPOSE.md 1.2): its own block (slot 28, grown to 64 MiB), its own queue there, its kernel recorded on the host (gpu_kernel.rs, build.rs). The kernel stamps its calls on `gpu` with its view (the phases read-write, the windows it names read-only), so its GPU space maps nothing of anyone's it does not compose, and nothing writable but its block and the phases.
//! Two threads: the render thread composes (`compose`: one job a phase, waited for, so the phases' rules are NEON's: a phase is written only while it is neither on the glass nor armed); the GPU thread (`gpu_thread`) makes the calls (the block, the queue, the kernel, the view), so a call never holds the render thread.
//! What step 2 leaves to NEON (`eligible`): formats 0 and 1, blend modes, windows that overlap (a partly covered pixel over another window is composed in working values, main.rs `partial_run`), more than `WINDOWS` windows, the screencap, the hit map's development view, a window whose surface the view does not name yet (named for the next frame), and anything while the GPU is not ready. Each of those frames is published as before, so the glass never waits on the GPU.

use crate::gpu_layout::*;
use crate::{bar_line, draw, edge_cols, edge_mask, front_order, hit, live_gen, q16, scale_q16, trans, under_pm, GlassRow, BAKED, CLEAR, MODE_MASK, OPAQUE, PIXELS, RINGS, SURFACES};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use ferros_sys::gpu::queue::{Job, Queue, S_DONE};

/// The display's grants past init's (the kernel's, process.rs `build_from`): 27 use gpu, 28 its GPU block. Empty slots when no GPU service ran at its start (after a jump): then nothing here ever starts.
const S_GPU: u64 = 27;
const S_BLOCK: u64 = 28;

mod kernel {
    // The compose kernel's words and constants (build.rs, gpu_kernel.rs).
    include!(concat!(env!("OUT_DIR"), "/compose.rs"));
}

/// The switch (op 86).
static ON: AtomicBool = AtomicBool::new(false);
/// Where the GPU side stands: `NONE` not started, `READY` serving, `FAILED` (no GPU service, or it refused): the switch then does nothing until the display starts again.
const NONE: u64 = 0;
const READY: u64 = 2;
const FAILED: u64 = 3;
static STATE: AtomicU64 = AtomicU64::new(NONE);
/// The GPU thread's doorbell.
static BELL: AtomicU64 = AtomicU64::new(0);
/// The windows' surfaces the view should name and does name: a nibble a surface slot, its generation (0 not named).
static VIEW_WANT: AtomicU64 = AtomicU64::new(0);
static VIEW_NAMED: AtomicU64 = AtomicU64::new(0);
/// The queue and the kernel's handle, made by the GPU thread before `STATE` says `READY` (Release), used by the render thread alone after it reads `READY` (Acquire).
struct Ctx {
    q: Queue,
    kernel: u8,
    block: usize,
}
static mut CTX: Option<Ctx> = None;
/// Frames composed here and their GPU µs, since the stats line last said them (`stats`).
static JOBS: AtomicU64 = AtomicU64::new(0);
static JOB_US: AtomicU64 = AtomicU64::new(0);
static ROUND_US: AtomicU64 = AtomicU64::new(0);
static NEON_FRAMES: AtomicU64 = AtomicU64::new(0);
static mut STACK: [u8; 1 << 14] = [0; 1 << 14];
static mut GRANTS: u64 = 0;

fn ring() {
    BELL.fetch_add(1, Ordering::Release);
    ferros_sys::wake(&BELL, 1);
}

/// The GPU thread started (the render thread's start, `main.rs`).
pub fn start(grants: u64) {
    unsafe { GRANTS = grants };
    ferros_sys::thread_spawn(gpu_thread, unsafe { &mut *(&raw mut STACK) }, grants);
}

/// Op 86 `gpu compose [what u8: 0 off, 1 on, 2 toggle, 3 ask]` → [0][on u8][state u8: 0 not started, 2 ready, 3 failed].
pub fn op(what: u8) -> [u8; 3] {
    let was = ON.load(Ordering::Acquire);
    let on = match what {
        0 => false,
        1 => true,
        2 => !was,
        _ => was,
    };
    if on != was {
        ON.store(on, Ordering::Release);
        if !on {
            // Off: the view named empty, so every window's surface may go back to the pool as its owner gives it.
            VIEW_WANT.store(0, Ordering::Release);
        }
        ring();
    }
    [0, on as u8, STATE.load(Ordering::Acquire) as u8]
}

/// The windows changed (main.rs `wins_mirror`): a surface the view names whose window is gone or whose owner ended is named no more, so the kernel can give its frames back.
pub fn windows_changed() {
    let named = VIEW_NAMED.load(Ordering::Acquire);
    let keep = live_of(named);
    if keep != VIEW_WANT.load(Ordering::Acquire) && keep != named {
        VIEW_WANT.store(keep, Ordering::Release);
        ring();
    }
}

/// The nibbles of `set` whose surface is still live under the same generation.
fn live_of(set: u64) -> u64 {
    let mut out = 0;
    for j in 0..SURFACES {
        let g = (set >> (4 * j)) & 15;
        if g != 0 && live_gen(j) as u64 == g {
            out |= g << (4 * j);
        }
    }
    out
}

extern "C" fn gpu_thread(grants: u64) -> ! {
    let mut seen = 0;
    loop {
        if ON.load(Ordering::Acquire) && STATE.load(Ordering::Acquire) == NONE {
            let ok = setup(grants);
            STATE.store(if ok { READY } else { FAILED }, Ordering::Release);
        }
        if STATE.load(Ordering::Acquire) == READY {
            let want = VIEW_WANT.load(Ordering::Acquire);
            if want != VIEW_NAMED.load(Ordering::Acquire) {
                let mut msg = [0u8; 3 + 2 * SURFACES];
                let mut n = 0;
                for j in 0..SURFACES {
                    let g = (want >> (4 * j)) & 15;
                    if g != 0 {
                        msg[3 + 2 * n] = j as u8;
                        msg[4 + 2 * n] = g as u8;
                        n += 1;
                    }
                }
                msg[..3].copy_from_slice(&[5, 1, n as u8]);
                let mut r = [1u8; 2];
                let got = ferros_sys::call(ferros_sys::cap(grants, S_GPU), &msg[..3 + 2 * n], &mut r);
                if got == 2 && r[0] == 0 {
                    VIEW_NAMED.store(want, Ordering::Release);
                } else {
                    let mut l = crate::Log { grants, buf: [0; 256], n: 0 };
                    l.s(b"display: the GPU service refused the view\n");
                    l.flush();
                }
                // Asked again at once if it changed meanwhile.
                continue;
            }
        }
        let b = BELL.load(Ordering::Acquire);
        if b == seen {
            ferros_sys::wait(&BELL, b, 0);
        }
        seen = b;
    }
}

/// The block grown to 64 MiB, the kernel's words and the thresholds written into it, the queue opened, the kernel loaded. False with no GPU service (empty slots) or a refusal, said on the console.
fn setup(grants: u64) -> bool {
    let (gcap, bcap) = (ferros_sys::cap(grants, S_GPU), ferros_sys::cap(grants, S_BLOCK));
    let mut l = crate::Log { grants, buf: [0; 256], n: 0 };
    let base = ferros_sys::dma(bcap);
    if base == ferros_sys::ERR || base == 0 || ferros_sys::gpu_block_ask(bcap, BLOCK_LOG2).is_none() {
        l.s(b"display: GPU compose: no GPU block (no GPU service at the display's start, or the pool too full)\n");
        l.flush();
        return false;
    }
    let base = ferros_sys::dma(bcap) as usize;
    // SAFETY: the block is this process's for its life, mapped at `base`, 2^BLOCK_LOG2 bytes (grown above); each buffer lies inside it (gpu_layout.rs).
    unsafe {
        let words = core::slice::from_raw_parts_mut((base + K_AT) as *mut u64, kernel::WORDS.len());
        words.copy_from_slice(kernel::WORDS);
        let blue = core::slice::from_raw_parts_mut((base + BLUE_AT) as *mut u8, 256);
        for y in 0..16 {
            blue[16 * y..16 * y + 16].copy_from_slice(&crate::neon::BLUE[y]);
        }
    }
    let q = match Queue::open(gcap, bcap, Q_AT, Q_DEPTH_LOG2) {
        Ok(q) => q,
        Err(e) => {
            l.s(b"display: GPU compose: the queue did not open (G#");
            l.hex(e);
            l.s(b")\n");
            l.flush();
            return false;
        }
    };
    let preload = ferros_sys::gpu::PRELOAD_X | if kernel::REGS64 { ferros_sys::gpu::PRELOAD_REGS64 } else { 0 };
    let Some(k) = q.kernel(K_AT, kernel::WORDS.len(), preload) else {
        l.s(b"display: GPU compose: the kernel was not loaded\n");
        l.flush();
        return false;
    };
    unsafe { *(&raw mut CTX) = Some(Ctx { q, kernel: k, block: base }) };
    l.s(b"display: GPU compose ready: block A#");
    l.dec(1 << (BLOCK_LOG2 - 20));
    l.s(b" MiB, kernel A#");
    l.dec(kernel::WORDS.len() as u64);
    l.s(b" words\n");
    l.flush();
    true
}

/// A window as a job reads it.
#[derive(Clone, Copy)]
struct Wn {
    slot: usize,
    rect: draw::Rect,
    ring: crate::Ring,
    opaque: bool,
}

/// The windows up, if step 2 composes them: at most `WINDOWS`, each `BAKED` in Normal, none overlapping another (so every pixel has one window or none, and no partly covered pixel stands over a window).
fn eligible(width: usize, height: usize) -> Option<([Wn; WINDOWS], usize)> {
    let (ord, n) = front_order();
    if n > WINDOWS {
        return None;
    }
    let mut out = [Wn { slot: 0, rect: draw::Rect::EMPTY, ring: crate::NO_RING, opaque: false }; WINDOWS];
    let mut i = 0;
    for &(k, w) in &ord[..n] {
        if w.format != BAKED || w.mode & MODE_MASK != 0 {
            return None;
        }
        let rect = w.rect.clip(width, height);
        // A window with no pixel on the screen composes nothing.
        if rect.is_empty() {
            continue;
        }
        if out[..i].iter().any(|o| !o.rect.intersect(rect).is_empty()) {
            return None;
        }
        out[i] = Wn { slot: unsafe { PIXELS[k] } as usize, rect, ring: unsafe { RINGS[k] }, opaque: w.mode & OPAQUE != 0 };
        i += 1;
    }
    Some((out, i))
}

/// Rect `r` (clipped to the screen) of the phases `mask` names composed on the GPU, one job a phase, waited for; the hit map's rows with it on the CPU when `hits`. False when step 2 does not compose this frame (`eligible`, the switch, the view, a failed job): the caller publishes it on NEON as before.
pub fn compose(r: draw::Rect, mask: u32, width: usize, height: usize, hits: bool) -> bool {
    if !ON.load(Ordering::Acquire) || STATE.load(Ordering::Acquire) != READY || unsafe { crate::CAPTURE } != 0 || hit::view() {
        return false;
    }
    let Some((wins, n)) = eligible(width, height) else {
        NEON_FRAMES.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    // Every window's surface named by the view under its generation; else asked for (with what is named and still live) and this frame on NEON.
    let named = VIEW_NAMED.load(Ordering::Acquire);
    let mut need = 0u64;
    for w in &wins[..n] {
        need |= (live_gen(w.slot) as u64) << (4 * w.slot);
    }
    if wins[..n].iter().any(|w| (named >> (4 * w.slot)) & 15 != (need >> (4 * w.slot)) & 15 || (need >> (4 * w.slot)) & 15 == 0) {
        let want = live_of(named) | need;
        if want != VIEW_WANT.load(Ordering::Acquire) {
            VIEW_WANT.store(want, Ordering::Release);
            ring();
        }
        NEON_FRAMES.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    // The crossover (step 3): the cheaper path by the display's own measured costs.
    let px = ((r.x1 - r.x0) * (r.y1 - r.y0)) as u64 * (mask & 15).count_ones() as u64;
    if !gpu_cheaper(px) {
        CROSSED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let t0 = ferros_sys::now();
    front_update(width, height);
    // SAFETY: `READY` was read above (Acquire), and only this thread uses the context after it.
    let Some(c) = (unsafe { (*(&raw mut CTX)).as_mut() }) else { return false };
    let va = ferros_sys::gpu::ARENA_VA;
    let blocks = (r.x1 - r.x0 + 15) as u32 >> 4;
    let rows = (r.y1 - r.y0) as u32;
    let cm = crate::CHANNELS.load(Ordering::Relaxed);
    let on = |b: u32| if cm >> b & 1 == 1 { 0xFFu64 } else { 0 };
    let mut jobs = [0u64; crate::PHASES];
    let mut nj = 0;
    let mut u = [0u64; 64];
    u[slot::FRONT as usize] = va + FRONT_AT as u64;
    u[slot::BLUE as usize] = va + BLUE_AT as u64;
    let xy = |x: i32, y: i32| (x as u64) | (y as u64) << 16;
    u[slot::RECT as usize] = xy(r.x0, r.y0) | xy(r.x1, r.y1) << 32;
    u[slot::GRID as usize] = blocks as u64 | ((1. / blocks as f32).to_bits() as u64) << 32;
    for (i, w) in wins.iter().enumerate() {
        let s = slot::WIN as usize + 4 * i;
        u[s] = ferros_sys::gpu::SURFACES_VA + ferros_sys::SURFACES_WINDOW + (w.slot * ferros_sys::SURFACE_BYTES) as u64 - ferros_sys::gpu::SURFACES_PA + crate::PORCH as u64;
        u[s + 1] = xy(w.rect.x0, w.rect.y0) | xy(w.rect.x1, w.rect.y1) << 32;
        u[s + 2] = (w.ring.x0 as u64 | (w.ring.x1 as u64) << 16) | (w.ring.y0 as u64 | (w.ring.y1 as u64) << 16) << 32;
        u[s + 3] = w.ring.o as u64 | (w.opaque as u64) << 32;
    }
    let nu = slot::RESERVED as usize + kernel::CONSTS.len();
    u[slot::RESERVED as usize..nu].copy_from_slice(kernel::CONSTS);
    for k in 0..crate::PHASES {
        if mask >> k & 1 == 0 {
            continue;
        }
        u[slot::PHASE as usize] = ferros_sys::gpu::PHASES_VA + (k << 24) as u64;
        u[slot::SCREEN as usize] = width as u64 | (crate::neon::TURN[k] as u64 | on(0) << 8 | on(1) << 16 | on(2) << 24) << 32;
        match c.q.submit(&Job { kernel: c.kernel, groups: (rows * blocks, 1), uniforms: &u[..nu] }) {
            Some(j) => {
                jobs[nj] = j;
                nj += 1;
            }
            None => break,
        }
    }
    let mut ok = nj == (mask & 15).count_ones() as usize;
    for &j in &jobs[..nj] {
        let e = c.q.wait(j);
        ok &= e.status == S_DONE;
        JOB_US.fetch_add(e.gpu_ticks().unwrap_or(0) * 1_000_000 / ferros_sys::freq(), Ordering::Relaxed);
        ROUND_US.fetch_add(e.round_ticks() * 1_000_000 / ferros_sys::freq(), Ordering::Relaxed);
    }
    if !ok {
        // A job that did not end done ended the queue (or the service is gone): open again for the next frame, this one on NEON.
        let reopened = c.q.reopen();
        let mut l = crate::Log { grants: unsafe { GRANTS }, buf: [0; 256], n: 0 };
        l.s(b"display: a GPU compose job failed; the queue opened again: ");
        l.s(if reopened { &b"yes"[..] } else { &b"no"[..] });
        l.s(b"\n");
        l.flush();
        return false;
    }
    JOBS.fetch_add(nj as u64, Ordering::Relaxed);
    cost_add(1, px, ferros_sys::us_since(t0));
    if hits {
        for y in r.y0 as usize..r.y1 as usize {
            hit::row(y, r.x0 as usize, r.x1 as usize, width, height, true);
        }
    }
    true
}

/// The stats line's part (main.rs's once-a-second line), taken and reset: (jobs, their mean GPU µs, their mean round µs, frames left to NEON).
pub fn stats() -> (u64, u64, u64, u64) {
    let j = JOBS.swap(0, Ordering::Relaxed);
    let g = JOB_US.swap(0, Ordering::Relaxed);
    let r = ROUND_US.swap(0, Ordering::Relaxed);
    (j, g / j.max(1), r / j.max(1), NEON_FRAMES.swap(0, Ordering::Relaxed))
}

// ---- the front layer ----

/// What the front layer was made from (`front_update`): the full screen switch, the mask's colour, the glass plan, the bars' rows, the screen; and the bars' rows changed since (`bars_changed`).
static mut FRONT_KEY: [u64; 6] = [u64::MAX; 6];
static mut BAR_DIRTY: [(usize, usize); 2] = [(0, 0); 2];

/// The bars swapped (main.rs, the pass's top): their changed rects' rows made again before the next job reads them. The render thread's.
pub fn bars_changed(r: [draw::Rect; 2]) {
    for (i, r) in r.iter().enumerate() {
        if !r.is_empty() {
            let d = unsafe { &mut (*(&raw mut BAR_DIRTY))[i] };
            *d = if d.0 == d.1 { (r.y0 as usize, r.y1 as usize) } else { (d.0.min(r.y0 as usize), d.1.max(r.y1 as usize)) };
        }
    }
}

fn front_update(width: usize, height: usize) {
    let key = unsafe { [crate::FULL as u64, edge_mask(), crate::GLASS_ROWS as u64, crate::BAR_TOP as u64, crate::CHROME_BOT as u64, (width as u64) << 32 | height as u64] };
    let (all, dirty) = unsafe { (FRONT_KEY != key, BAR_DIRTY) };
    let Some(c) = (unsafe { (*(&raw const CTX)).as_ref() }) else { return };
    // SAFETY: the front layer is W H u64 from FRONT_AT, inside the block (gpu_layout.rs); the render thread is its one writer, and no job runs while it writes (`compose` waits for each).
    let front = unsafe { core::slice::from_raw_parts_mut((c.block + FRONT_AT) as *mut u64, width * height) };
    let mut make = |a: usize, b: usize| {
        for y in a..b.min(height) {
            front_row(y, width, height, &mut front[y * width..(y + 1) * width]);
        }
    };
    if all {
        make(0, height);
        unsafe { FRONT_KEY = key };
    } else {
        for d in dirty {
            make(d.0, d.1);
        }
    }
    unsafe { BAR_DIRTY = [(0, 0); 2] };
}

/// Row y of the front layer: what main.rs `publish_app_rows` composes in front of the scene on that row (the glass's edge and the hole's knockout `glass_front`, the bars `bars_front`), up to the scene, as (channels, what of the scene shows); `PASS` on every column nothing is in front of.
fn front_row(y: usize, width: usize, height: usize, out: &mut [u64]) {
    let rows = unsafe { core::slice::from_raw_parts(crate::GLASS_ROWS as *const GlassRow, height + 1) };
    let g = &rows[y];
    let (first, next) = (g.first as usize, rows[y + 1].first as usize);
    let (ml, mr) = (g.mask_l as usize, g.mask_r as usize);
    let ((bl, br), bw) = bar_line(y, width, height);
    if !(ml > 0 || mr < width || first != next || bl < br) {
        out.fill(PASS);
        return;
    }
    let plan = unsafe { core::slice::from_raw_parts(crate::GLASS_PX as *const (u16, u16, u16, u16), next) };
    let edge_px = &plan[first..next];
    let (el, er) = edge_cols(g, edge_px, width);
    let (hl, hr) = (g.hole_l as usize, g.hole_r as usize);
    for (x, o) in out.iter_mut().enumerate() {
        let bars = x >= bl && x < br;
        let hole = x >= hl && x < hr;
        *o = if !(x < el || x >= er || bars || hole) {
            PASS
        } else {
            let c = if bars { bw[x] } else { CLEAR };
            if x >= el && x < er && !hole { front_bars(c) } else { front_glass(x, g, edge_px, c) }
        };
    }
}

/// main.rs `bars_front` up to the scene.
fn front_bars(c: u64) -> u64 {
    if c >> 63 != 0 { PASS } else { (c & ((1 << 48) - 1)) | trans(c) << 49 }
}

/// main.rs `glass_front` up to the scene: the mask where it is whole (its channels and no-dither bit, nothing of the scene), `PASS` where nothing is in front, else the edge (its coverage and hairline) and the bars' pixel `c` composed front to back (`under_pm`) with what of the scene still shows.
fn front_glass(x: usize, g: &GlassRow, edge_px: &[(u16, u16, u16, u16)], c: u64) -> u64 {
    let mask = edge_mask();
    let whole = mask & ((1 << 49) - 1);
    if x < g.mask_l as usize || x >= g.mask_r as usize {
        return whole;
    }
    let (hl, hr) = (g.hole_l as usize, g.hole_r as usize);
    let (edge_px, hole_px) = edge_px.split_at(edge_px.len() - (hr - hl));
    // As `glass_front`: inside the knockout's columns `x - hl` indexes the hole's pixels, one a column.
    let edge = if x >= hl && x < hr {
        let e = &hole_px[x - hl];
        if e.2 == 0 {
            return whole;
        }
        Some(e)
    } else {
        edge_px.iter().find(|e| e.0 as usize == x)
    };
    if edge.is_none() && c >> 63 != 0 {
        return PASS;
    }
    let (mut acc, mut t) = (0u64, 16384u64);
    if let Some(&(_, _, cov, h)) = edge {
        acc = scale_q16(mask, 65536 - q16(cov));
        t = q16(cov) >> 2;
        (acc, t) = under_pm(acc, t, scale_q16(mask, q16(h)), 16384 - (q16(h) >> 2));
    }
    if c >> 63 == 0 {
        (acc, t) = under_pm(acc, t, c & ((1 << 48) - 1), trans(c));
    }
    acc | t << 49
}

// ---- the check (op 86's 4) ----

/// The gate of GPU-COMPOSE step 2: phase `k` (published by NEON just before, the whole screen) composed again by the GPU into the block (`CHECK_AT`) and compared word for word. → (words that differ, the first's index, NEON's word there, the GPU's), or None when the GPU does not compose this frame.
pub fn check(k: usize, width: usize, height: usize) -> Option<(u64, u64, u32, u32)> {
    if STATE.load(Ordering::Acquire) != READY {
        return None;
    }
    let (wins, n) = eligible(width, height)?;
    let named = VIEW_NAMED.load(Ordering::Acquire);
    if wins[..n].iter().any(|w| (named >> (4 * w.slot)) & 15 != live_gen(w.slot) as u64) {
        return None;
    }
    front_update(width, height);
    let c = unsafe { (*(&raw mut CTX)).as_mut() }?;
    let va = ferros_sys::gpu::ARENA_VA;
    let blocks = (width as u32 + 15) >> 4;
    let cm = crate::CHANNELS.load(Ordering::Relaxed);
    let on = |b: u32| if cm >> b & 1 == 1 { 0xFFu64 } else { 0 };
    let mut u = [0u64; 64];
    u[slot::PHASE as usize] = va + CHECK_AT as u64;
    u[slot::FRONT as usize] = va + FRONT_AT as u64;
    u[slot::BLUE as usize] = va + BLUE_AT as u64;
    u[slot::RECT as usize] = (width as u64 | (height as u64) << 16) << 32;
    u[slot::GRID as usize] = blocks as u64 | ((1. / blocks as f32).to_bits() as u64) << 32;
    u[slot::SCREEN as usize] = width as u64 | (crate::neon::TURN[k] as u64 | on(0) << 8 | on(1) << 16 | on(2) << 24) << 32;
    for (i, w) in wins.iter().enumerate() {
        let s = slot::WIN as usize + 4 * i;
        u[s] = ferros_sys::gpu::SURFACES_VA + ferros_sys::SURFACES_WINDOW + (w.slot * ferros_sys::SURFACE_BYTES) as u64 - ferros_sys::gpu::SURFACES_PA + crate::PORCH as u64;
        u[s + 1] = (w.rect.x0 as u64 | (w.rect.y0 as u64) << 16) | (w.rect.x1 as u64 | (w.rect.y1 as u64) << 16) << 32;
        u[s + 2] = (w.ring.x0 as u64 | (w.ring.x1 as u64) << 16) | (w.ring.y0 as u64 | (w.ring.y1 as u64) << 16) << 32;
        u[s + 3] = w.ring.o as u64 | (w.opaque as u64) << 32;
    }
    let nu = slot::RESERVED as usize + kernel::CONSTS.len();
    u[slot::RESERVED as usize..nu].copy_from_slice(kernel::CONSTS);
    let j = c.q.submit(&Job { kernel: c.kernel, groups: (height as u32 * blocks, 1), uniforms: &u[..nu] })?;
    let e = c.q.wait(j);
    if e.status != S_DONE {
        c.q.reopen();
        return None;
    }
    // SAFETY: the check's words lie in the block (gpu_layout.rs), the phase's in the phases (main.rs `phase_slice`), both W H u32.
    let got = unsafe { core::slice::from_raw_parts((c.block + CHECK_AT) as *const u32, width * height) };
    let want = crate::phase_slice(k, width * height);
    let mut diff = (0u64, 0u64, 0u32, 0u32);
    for (i, (&a, &b)) in want.iter().zip(got).enumerate() {
        if a != b {
            if diff.0 == 0 {
                diff = (0, i as u64, a, b);
            }
            diff.0 += 1;
        }
    }
    Some(diff)
}

/// A check asked (op 86's 4), taken by the next publish of one phase (`check_take`).
static CHECK: AtomicBool = AtomicBool::new(false);

pub fn check_next() {
    CHECK.store(true, Ordering::Release);
}

/// The phase to check after this publish, when one was asked and this publish writes one phase: it goes to NEON, then the GPU composes it again (`check_say`).
pub fn check_take(mask: u32) -> Option<usize> {
    (mask.count_ones() == 1 && STATE.load(Ordering::Acquire) == READY && CHECK.swap(false, Ordering::AcqRel)).then(|| mask.trailing_zeros() as usize)
}

/// The check's line on the console.
pub fn check_say(k: usize, width: usize, height: usize) {
    let mut l = crate::Log { grants: unsafe { GRANTS }, buf: [0; 256], n: 0 };
    match check(k, width, height) {
        Some((n, at, a, b)) => {
            l.s(b"display: GPU compose check, phase A#");
            l.dec(k as u64);
            l.s(b": A#");
            l.dec(n);
            l.s(b" of A#");
            l.dec((width * height) as u64);
            l.s(b" words differ from NEON's");
            if n != 0 {
                l.s(b" (the first at A#");
                l.dec(at);
                l.s(b": NEON G#");
                l.hex(a);
                l.s(b", the GPU G#");
                l.hex(b);
                l.s(b")");
            }
            l.s(b"\n");
        }
        None => l.s(b"display: GPU compose check: the GPU does not compose this frame (step 2's windows, the view, or a failed job)\n"),
    }
    l.flush();
}

// ---- the crossover (GPU-COMPOSE.md knob 0, section 1.6, step 3) ----

/// A path's cost as the render thread measures it, a + b · pixels (µs): `a` what a frame costs before its first pixel (on the GPU the queue's round and the cores' wake; on NEON the workers' wake), followed by the small frames' wall times; `b` each pixel's share, followed by the large frames' (`a` taken off first). Each an average that halves an old frame's weight every 2^4 frames, so the line follows the clocks, the heat and the contention by itself, and nobody picks it. Seeded from compbench (GPU-KISTS.md section 3, "Small damage") until the display's own frames replace it.
#[derive(Clone, Copy)]
struct Cost {
    a: f32,
    b: f32,
}
/// NEON's and the GPU's costs (the render thread's alone).
static mut COSTS: [Cost; 2] = [Cost { a: 64., b: 1. / 1024. }, Cost { a: 512., b: 1. / 2048. }];
/// Frames under 2^14 pixels feed `a`, frames over 2^18 feed `b`.
const SMALL: u32 = 14;
const LARGE: u32 = 18;
/// How fast the averages forget: 2^-4 of a new frame each.
const FORGET: f32 = 1. / (1 << 4) as f32;
/// One frame in 2^5 goes the other way, so the path not taken keeps being measured.
const EXPLORE: u64 = (1 << 5) - 1;
static DECISIONS: AtomicU64 = AtomicU64::new(0);
/// Frames the crossover sent to NEON (since the stats line).
static CROSSED: AtomicU64 = AtomicU64::new(0);

fn cost_at(c: Cost, px: u64) -> f32 {
    c.a + c.b * px as f32
}

/// A frame's wall time on path `p` (0 NEON, 1 the GPU) taken into its cost.
fn cost_add(p: usize, px: u64, us: u64) {
    let c = unsafe { &mut (*(&raw mut COSTS))[p] };
    let us = us as f32;
    if px < 1 << SMALL {
        c.a += (us - c.a) * FORGET;
    } else if px > 1 << LARGE {
        let b = (us - c.a) / px as f32;
        // A large frame faster than the fixed part says nothing of the pixels' share (a fluke of the clocks): left out, so `b` stays positive.
        if b > 0. {
            c.b += (b - c.b) * FORGET;
        }
    }
}

/// Whether the GPU takes a frame of `px` pixels (summed over its phases): the cheaper path by the costs, the other one frame in 2^5.
fn gpu_cheaper(px: u64) -> bool {
    let c = unsafe { *(&raw const COSTS) };
    let gpu = cost_at(c[1], px) < cost_at(c[0], px);
    let n = DECISIONS.fetch_add(1, Ordering::Relaxed);
    if n & EXPLORE == EXPLORE { !gpu } else { gpu }
}

/// A NEON publish's wall time while the GPU compose is on (main.rs `publish_app_mask`): into NEON's cost.
pub fn neon_took(r: draw::Rect, mask: u32, us: u64) {
    if ON.load(Ordering::Acquire) && STATE.load(Ordering::Acquire) == READY {
        cost_add(0, ((r.x1 - r.x0) * (r.y1 - r.y0)) as u64 * (mask & 15).count_ones() as u64, us);
    }
}

/// The costs now, for the stats line: NEON's and the GPU's (a µs, b ns a pixel), the pixels where they cross, the frames the crossover sent to NEON.
pub fn costs() -> ([(u64, u64); 2], u64, u64) {
    let c = unsafe { *(&raw const COSTS) };
    // The cast saturates: a crossing below 0 pixels (the GPU cheaper at every size) says 0.
    let line = if c[0].b > c[1].b { ((c[1].a - c[0].a) / (c[0].b - c[1].b)) as u64 } else { u64::MAX };
    (c.map(|c| (c.a as u64, (c.b * 1000.) as u64)), line, CROSSED.swap(0, Ordering::Relaxed))
}
