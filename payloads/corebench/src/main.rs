//! corebench: what a pinned thread gets from each kind of worker core (2026-11-18, the A510s brought up as workers for Lumis's camera intake: smp.rs, KERNEL.md "Cores").
//! Per core class (the A510s, `CORES_LITTLE`, then the A715s, `CORES_WORKERS`): the clock as the governor raises it under a pinned load (a dependent add chain timed every 2^-5 s for 2 s, one add a cycle on every core of the SoC, so adds per microsecond read as MHz), then one thread's steady rates (the clock, adds per cycle over four chains, NEON FMLA over eight chains and one chain's latency), then four threads at once (the class's four cores; the A510s share a NEON unit between two cores), then the clock after half a second idle.
//! Spawned from the host: `ferros-bridge spawn corebench.bin [hex]`; input byte 0: 0 both classes, 1 the A510s only, 2 the A715s only. Grant slots: 0 console. Development, headless.
#![no_std]
#![no_main]

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::{console, exit, freq, now, pin, sleep_until, thread_exit, thread_spawn, wait, wake};

struct Line {
    b: [u8; 512],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // WHY the min: a formatted line longer than the buffer would index past it and panic the process; the console gets the line cut instead.
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 512], n: 0 };
        let _ = writeln!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

/// Ticks for `passes` x 64 dependent adds: one add a cycle on every core here, so the count over the time is the clock.
#[inline(never)]
fn dep_adds(passes: u64) -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!(
            "mrs {t0}, cntvct_el0",
            "mov {a}, #0",
            "2:",
            ".rept 64",
            "add {a}, {a}, #1",
            ".endr",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "mrs {t1}, cntvct_el0",
            "sub {t}, {t1}, {t0}",
            t0 = out(reg) _, t1 = out(reg) _, a = out(reg) _, n = inout(reg) passes => _, t = out(reg) t,
        );
    }
    t
}

/// Ticks for `passes` x 256 adds in four independent chains: how many simple integer operations a cycle the core issues.
#[inline(never)]
fn wide_adds(passes: u64) -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!(
            "mrs {t0}, cntvct_el0",
            "2:",
            ".rept 64",
            "add {a}, {a}, #1",
            "add {b}, {b}, #1",
            "add {c}, {c}, #1",
            "add {d}, {d}, #1",
            ".endr",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "mrs {t1}, cntvct_el0",
            "sub {t}, {t1}, {t0}",
            t0 = out(reg) _, t1 = out(reg) _, a = out(reg) _, b = out(reg) _, c = out(reg) _, d = out(reg) _, n = inout(reg) passes => _, t = out(reg) t,
        );
    }
    t
}

/// Ticks for `passes` x 128 FMLA (4 lanes of f32 each, 8 flops) in eight independent chains: the NEON pipe's throughput.
#[inline(never)]
fn neon_fmla(passes: u64) -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!(
            "movi v0.4s, #0", "movi v1.4s, #0", "movi v2.4s, #0", "movi v3.4s, #0",
            "movi v4.4s, #0", "movi v5.4s, #0", "movi v6.4s, #0", "movi v7.4s, #0",
            "fmov v16.4s, #1.0", "fmov v17.4s, #0.5",
            "mrs {t0}, cntvct_el0",
            "2:",
            ".rept 16",
            "fmla v0.4s, v16.4s, v17.4s", "fmla v1.4s, v16.4s, v17.4s", "fmla v2.4s, v16.4s, v17.4s", "fmla v3.4s, v16.4s, v17.4s",
            "fmla v4.4s, v16.4s, v17.4s", "fmla v5.4s, v16.4s, v17.4s", "fmla v6.4s, v16.4s, v17.4s", "fmla v7.4s, v16.4s, v17.4s",
            ".endr",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "mrs {t1}, cntvct_el0",
            "sub {t}, {t1}, {t0}",
            t0 = out(reg) _, t1 = out(reg) _, n = inout(reg) passes => _, t = out(reg) t,
            out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _, out("v16") _, out("v17") _,
        );
    }
    t
}

/// Ticks for `passes` x 64 FMLA on one chain: the NEON latency.
#[inline(never)]
fn neon_dep(passes: u64) -> u64 {
    let t: u64;
    unsafe {
        core::arch::asm!(
            "movi v0.4s, #0",
            "fmov v16.4s, #1.0", "fmov v17.4s, #0.5",
            "mrs {t0}, cntvct_el0",
            "2:",
            ".rept 64",
            "fmla v0.4s, v16.4s, v17.4s",
            ".endr",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "mrs {t1}, cntvct_el0",
            "sub {t}, {t1}, {t0}",
            t0 = out(reg) _, t1 = out(reg) _, n = inout(reg) passes => _, t = out(reg) t,
            out("v0") _, out("v16") _, out("v17") _,
        );
    }
    t
}

/// Operations a microsecond for `ops` in `ticks`.
fn per_us(ops: u64, ticks: u64) -> u64 {
    ops * freq() / ticks / 1_000_000
}

/// One thread's steady rates: (MHz, adds per cycle in 256ths, NEON GFLOP/s in 1000ths, cycles per dependent FMLA in 256ths).
fn steady() -> (u64, u64, u64, u64) {
    let p = 1u64 << 18;
    let mhz = per_us(p * 64, dep_adds(p));
    let wide = per_us(p * 256, wide_adds(p));
    let fl = per_us((p >> 1) * 128 * 8, neon_fmla(p >> 1));
    let dep = per_us((p >> 2) * 64, neon_dep(p >> 2));
    (mhz, wide * 256 / mhz, fl, mhz * 256 / dep)
}

static GO: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static RESULTS: [[AtomicU64; 4]; 4] = [const { [const { AtomicU64::new(0) }; 4] }; 4];
static mut STACKS: [[u8; 1 << 15]; 3] = [[0; 1 << 15]; 3];

/// A parallel thread: waits for the go, measures, reports into its row.
extern "C" fn worker(k: u64) -> ! {
    while GO.load(Ordering::Acquire) == 0 {
        wait(&GO, 0, 0);
    }
    let r = steady();
    for (s, v) in RESULTS[k as usize].iter().zip([r.0, r.1, r.2, r.3]) {
        s.store(v, Ordering::Relaxed);
    }
    DONE.fetch_add(1, Ordering::Release);
    wake(&DONE, 1);
    thread_exit(0)
}

fn class(g: u64, name: &str, mask: u64) {
    pin(mask);
    // A sleep so the next run lands on a core of the mask.
    sleep_until(now() + (freq() >> 10));
    // The clock under this load, every 2^-5 s for 2 s: the governor's window is 2^-6 s.
    let f = freq();
    let t0 = now();
    let mut ramp = [0u64; 64];
    for (i, m) in ramp.iter_mut().enumerate() {
        let p = 1u64 << 12;
        *m = per_us(p * 64, dep_adds(p));
        let next = t0 + (i as u64 + 1) * (f >> 5);
        while now() < next {
            core::hint::black_box(dep_adds(1 << 6));
        }
    }
    let mut l = Line { b: [0; 512], n: 0 };
    let _ = write!(l, "corebench {name}: MHz every 2^-5 s under load:");
    for m in ramp {
        let _ = write!(l, " A#{m}");
    }
    let _ = writeln!(l);
    console(g, &l.b[..l.n]);
    let (mhz, wide, fl, dep) = steady();
    // Flops a cycle in 256ths: MFLOP/s over MHz.
    let fpc = fl * 256 / mhz;
    say!(g, "corebench {name}: one thread: A#{mhz} MHz, A#{}.{:02} adds a cycle (four chains), NEON FMLA A#{}.{:03} GFLOP/s (A#{}.{:02} flops a cycle), one FMLA chain A#{}.{:02} cycles a step", wide >> 8, (wide & 255) * 100 >> 8, fl / 1000, fl % 1000, fpc >> 8, (fpc & 255) * 100 >> 8, dep >> 8, (dep & 255) * 100 >> 8);
    // Four at once: three threads beside this one, all pinned alike (threads inherit the mask).
    GO.store(0, Ordering::Release);
    DONE.store(0, Ordering::Release);
    for k in 0..3u64 {
        let s = unsafe { &mut (*(&raw mut STACKS))[k as usize] };
        thread_spawn(worker, s, k + 1);
    }
    sleep_until(now() + (f >> 6));
    GO.store(1, Ordering::Release);
    wake(&GO, 8);
    let r = steady();
    for (s, v) in RESULTS[0].iter().zip([r.0, r.1, r.2, r.3]) {
        s.store(v, Ordering::Relaxed);
    }
    while DONE.load(Ordering::Acquire) < 3 {
        let d = DONE.load(Ordering::Acquire);
        wait(&DONE, d, now() + f);
    }
    for (k, row) in RESULTS.iter().enumerate() {
        let (mhz, _, fl, _) = (row[0].load(Ordering::Relaxed), row[1].load(Ordering::Relaxed), row[2].load(Ordering::Relaxed), row[3].load(Ordering::Relaxed));
        say!(g, "corebench {name}: four at once, thread A#{k}: A#{mhz} MHz, NEON FMLA A#{}.{:03} GFLOP/s", fl / 1000, fl % 1000);
    }
    // Half a second idle, then the clock at once: where the governor let it fall.
    sleep_until(now() + (f >> 1));
    let p = 1u64 << 12;
    let idle = per_us(p * 64, dep_adds(p));
    say!(g, "corebench {name}: after 2^-1 s idle: A#{idle} MHz");
}

// ---------------------------------------------------------------- the watch (input byte 0 = 3)

/// The watch's measures a sample: the clock by the PMU's cycles over the counter (MHz), dependent adds a cycle (256ths), NEON FMLA flops a cycle (256ths), and three dependent-load chases (ns a load in 16ths): one in the core's own L2, one in the L3, one in DRAM.
const MEASURES: usize = 6;
const MEASURE_NAMES: [&str; MEASURES] = ["MHz", "adds/cyc x256", "flops/cyc x256", "L2 ns x16", "L3 ns x16", "DRAM ns x16"];
/// The cores a probe may sit on: the A510s 0-3, the A715s 4-7, the X3 8.
const CORES: usize = 9;
/// Each core's measures over the last whole second (sums and the samples counted), the second they are for.
static SUMS: [[AtomicU64; MEASURES]; CORES] = [const { [const { AtomicU64::new(0) }; MEASURES] }; CORES];
static SAMPLES: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
static LOWEST_MHZ: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
/// The watch's start (counter ticks) and its end; the chases' buffers (addresses) and line counts.
static T0: AtomicU64 = AtomicU64::new(0);
static END: AtomicU64 = AtomicU64::new(0);
static L2_BASE: AtomicU64 = AtomicU64::new(0);
static L3_BASE: AtomicU64 = AtomicU64::new(0);
static DRAM_BASE: AtomicU64 = AtomicU64::new(0);
/// The L2 chase per core (each its own, so no core finds another's lines): 2^17 bytes, inside every core's L2 here.
const L2_BYTES: usize = 1 << 17;
/// The L3 chase, one for all: 2^22 bytes, under the DSU's L3.
const L3_BYTES: usize = 1 << 22;
/// The DRAM chase, one for all: 2^26 bytes, past every cache and the SLC.
const DRAM_BYTES: usize = 1 << 26;
/// Loads a chase sample.
const CHASE: u64 = 1 << 9;
/// The load's streams (+5 u16 mask): each thread reads its own source and adds every byte into u16 sums twice its size, as the camera's integrator does with a frame, with no camera.
const LOAD_SRC: usize = 1 << 24;
static LOAD_BASE: AtomicU64 = AtomicU64::new(0);
static LOAD_PASSES: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];
static mut WATCH_STACKS: [[u8; 1 << 14]; 2 * CORES] = [[0; 1 << 14]; 2 * CORES];

/// A random cycle thru the lines of `bytes` at `base` (Sattolo's shuffle by an xorshift, each line's first word the next line's address), so every load depends on the last and no prefetcher guesses the next.
fn chain(base: usize, bytes: usize, seed: u64) {
    let lines = bytes >> 6;
    let p = base as *mut u64;
    // The order first, as indices in each line's first word, then turned into addresses.
    for i in 0..lines {
        unsafe { *p.add(i << 3) = i as u64 };
    }
    let mut x = seed | 1;
    for i in (1..lines).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let j = (x % i as u64) as usize;
        unsafe { core::ptr::swap(p.add(i << 3), p.add(j << 3)) };
    }
    // Now line k holds perm[k]; link perm[k] -> perm[k+1] by address.
    let mut first = 0u64;
    let mut prev = usize::MAX;
    for k in 0..lines {
        let at = unsafe { *p.add(k << 3) } as usize;
        if prev == usize::MAX {
            first = at as u64;
        } else {
            // Stash the link in the line's second word while the first still holds the order.
            unsafe { *p.add((prev << 3) + 1) = (base + (at << 6)) as u64 };
        }
        prev = at;
    }
    unsafe { *p.add((prev << 3) + 1) = (base + ((first as usize) << 6)) as u64 };
    for i in 0..lines {
        unsafe { *p.add(i << 3) = *p.add((i << 3) + 1) };
    }
}

/// `n` dependent loads from `at`: (where the chase ended, counter ticks).
#[inline(never)]
fn chase(at: u64, n: u64) -> (u64, u64) {
    let (end, t): (u64, u64);
    unsafe {
        core::arch::asm!(
            "isb",
            "mrs {t0}, cntvct_el0",
            "2:",
            "ldr {p}, [{p}]",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            "isb",
            "mrs {t1}, cntvct_el0",
            "sub {t}, {t1}, {t0}",
            t0 = out(reg) _, t1 = out(reg) _, p = inout(reg) at => end, n = inout(reg) n => _, t = out(reg) t,
        );
    }
    (end, t)
}

/// One probe a core: every 2^-4 s its six measures, summed by the second for the reporter.
extern "C" fn probe(core: u64) -> ! {
    let c = core as usize;
    ferros_sys::pin(1 << core);
    sleep_until(now() + (freq() >> 10));
    let f = freq();
    let l2 = L2_BASE.load(Ordering::Relaxed) + core * L2_BYTES as u64;
    chain(l2 as usize, L2_BYTES, 0x9E37_79B9_7F4A_7C15 ^ core);
    let (mut p2, mut p3, mut pd) = (l2, L3_BASE.load(Ordering::Relaxed), DRAM_BASE.load(Ordering::Relaxed));
    // Each core starts the shared chases at its own place on them.
    for _ in 0..core * 64 {
        p3 = unsafe { *(p3 as *const u64) };
        pd = unsafe { *(pd as *const u64) };
    }
    let t0 = T0.load(Ordering::Relaxed);
    let mut i = 0u64;
    let mut lowest = u64::MAX;
    while now() < END.load(Ordering::Relaxed) {
        i += 1;
        sleep_until(t0 + i * (f >> 4));
        let c0 = ferros_sys::pmu::cycles();
        let t = dep_adds(1 << 8);
        let c1 = ferros_sys::pmu::cycles();
        let cyc = c1 - c0;
        let mhz = cyc * f / t / 1_000_000;
        let adds = (1u64 << 14) * 256 / cyc;
        let c2 = ferros_sys::pmu::cycles();
        let _ = neon_fmla(1 << 6);
        let flops = (1u64 << 6) * 128 * 8 * 256 / (ferros_sys::pmu::cycles() - c2);
        let (e, t2) = chase(p2, CHASE);
        p2 = e;
        let (e, t3) = chase(p3, CHASE);
        p3 = e;
        let (e, td) = chase(pd, CHASE);
        pd = e;
        let ns16 = |t: u64| t * 16_000_000_000 / f / CHASE;
        for (s, v) in SUMS[c].iter().zip([mhz, adds, flops, ns16(t2), ns16(t3), ns16(td)]) {
            s.fetch_add(v, Ordering::Relaxed);
        }
        SAMPLES[c].fetch_add(1, Ordering::Relaxed);
        lowest = lowest.min(mhz);
        LOWEST_MHZ[c].store(lowest, Ordering::Relaxed);
    }
    DONE.fetch_add(1, Ordering::Release);
    wake(&DONE, 1);
    thread_exit(0)
}

/// One load stream a core: its source read 64 bytes a load and widened into u16 sums (read, add, write), pass after pass, until the watch ends.
extern "C" fn load(core: u64) -> ! {
    use core::arch::aarch64::*;
    let c = core as usize;
    ferros_sys::pin(1 << core);
    sleep_until(now() + (freq() >> 10));
    let base = LOAD_BASE.load(Ordering::Relaxed) as usize + c * 3 * LOAD_SRC;
    let (src, acc) = (base as *const u8, (base + LOAD_SRC) as *mut u16);
    while now() < END.load(Ordering::Relaxed) {
        for i in 0..LOAD_SRC / 64 {
            // SAFETY: block i of this core's source (2^24 bytes) and its u16 sums (2^25 bytes) inside the memory grown for the loads.
            unsafe {
                let v = vld1q_u8_x4(src.add(i * 64));
                let a = acc.add(i * 64);
                for (k, q) in [v.0, v.1, v.2, v.3].into_iter().enumerate() {
                    let lo = vaddw_u8(vld1q_u16(a.add(k * 16)), vget_low_u8(q));
                    let hi = vaddw_u8(vld1q_u16(a.add(k * 16 + 8)), vget_high_u8(q));
                    vst1q_u16(a.add(k * 16), lo);
                    vst1q_u16(a.add(k * 16 + 8), hi);
                }
            }
        }
        LOAD_PASSES[c].fetch_add(1, Ordering::Relaxed);
    }
    DONE.fetch_add(1, Ordering::Release);
    wake(&DONE, 1);
    thread_exit(0)
}

/// The watch: input +1 the probes' cores (u16 mask, 0: G#1FF), +3 seconds (0: 32), +5 the load's cores (u16 mask, 0: none). Once a second a line a measure, every probed core's mean over that second; the load's passes a second beside.
fn watch(g: u64, b: &dyn Fn(u64) -> u8) {
    let h = |k: u64| u16::from_le_bytes([b(k), b(k + 1)]) as u64;
    let probes = match h(1) {
        0 => 0x1FF,
        m => m & 0x1FF,
    };
    let secs = match h(3) {
        0 => 32,
        s => s,
    };
    let loads = h(5) & 0x1FF;
    let f = freq();
    let bytes = CORES * L2_BYTES + L3_BYTES + DRAM_BYTES + CORES * 3 * LOAD_SRC * (loads != 0) as usize;
    let Ok((_, was)) = ferros_sys::arc::grow(bytes) else {
        say!(g, "corebench watch: no memory (A#{} MiB)", bytes >> 20);
        exit(1);
    };
    let base = ferros_sys::arc_base() + was as u64;
    L2_BASE.store(base, Ordering::Relaxed);
    L3_BASE.store(base + (CORES * L2_BYTES) as u64, Ordering::Relaxed);
    DRAM_BASE.store(base + (CORES * L2_BYTES + L3_BYTES) as u64, Ordering::Relaxed);
    LOAD_BASE.store(base + (CORES * L2_BYTES + L3_BYTES + DRAM_BYTES) as u64, Ordering::Relaxed);
    chain((base + (CORES * L2_BYTES) as u64) as usize, L3_BYTES, 0x2545_F491_4F6C_DD1D);
    chain((base + (CORES * L2_BYTES + L3_BYTES) as u64) as usize, DRAM_BYTES, 0x1405_7B7E_F767_814F);
    say!(g, "corebench watch: probes on G#{probes:X}, load on G#{loads:X}, A#{secs} s; a sample every 2^-4 s: {:?}", MEASURE_NAMES);
    let t0 = now() + (f >> 3);
    T0.store(t0, Ordering::Relaxed);
    END.store(t0 + secs * f, Ordering::Relaxed);
    DONE.store(0, Ordering::Relaxed);
    let mut threads = 0;
    let mut stack = 0;
    for k in 0..CORES as u64 {
        if probes >> k & 1 == 1 {
            thread_spawn(probe, unsafe { &mut (*(&raw mut WATCH_STACKS))[stack] }, k);
            stack += 1;
            threads += 1;
        }
        if loads >> k & 1 == 1 {
            thread_spawn(load, unsafe { &mut (*(&raw mut WATCH_STACKS))[stack] }, k);
            stack += 1;
            threads += 1;
        }
    }
    // The reporter sits on no core of its own: it sleeps to each second's end and reads what the probes summed.
    let mut last = [[0u64; MEASURES]; CORES];
    let mut last_n = [0u64; CORES];
    let mut last_p = [0u64; CORES];
    for s in 1..=secs {
        sleep_until(t0 + s * f + (f >> 5));
        let mut now_s = [[0u64; MEASURES]; CORES];
        let mut now_n = [0u64; CORES];
        for c in 0..CORES {
            for m in 0..MEASURES {
                now_s[c][m] = SUMS[c][m].load(Ordering::Relaxed);
            }
            now_n[c] = SAMPLES[c].load(Ordering::Relaxed);
        }
        for m in 0..MEASURES {
            let mut l = Line { b: [0; 512], n: 0 };
            let _ = write!(l, "watch s A#{s} {}:", MEASURE_NAMES[m]);
            for c in 0..CORES {
                if probes >> c & 1 == 0 {
                    continue;
                }
                let n = now_n[c] - last_n[c];
                // WHY the zero: a second in which a probe took no sample (its core busy past the second) has no mean to divide out.
                let v = if n == 0 { 0 } else { (now_s[c][m] - last[c][m]) / n };
                let _ = write!(l, " c{c} A#{v}");
            }
            let _ = writeln!(l);
            console(g, &l.b[..l.n]);
        }
        if loads != 0 {
            let mut l = Line { b: [0; 512], n: 0 };
            let _ = write!(l, "watch s A#{s} load MB/s read:");
            for c in 0..CORES {
                if loads >> c & 1 == 1 {
                    let p = LOAD_PASSES[c].load(Ordering::Relaxed);
                    let _ = write!(l, " c{c} A#{}", (p - last_p[c]) * LOAD_SRC as u64 / 1_000_000);
                    last_p[c] = p;
                }
            }
            let _ = writeln!(l);
            console(g, &l.b[..l.n]);
        }
        last = now_s;
        last_n = now_n;
    }
    while DONE.load(Ordering::Acquire) < threads {
        let d = DONE.load(Ordering::Acquire);
        wait(&DONE, d, now() + f);
    }
    let mut l = Line { b: [0; 512], n: 0 };
    let _ = write!(l, "corebench watch: done; the lowest MHz a sample:");
    for c in 0..CORES {
        if probes >> c & 1 == 1 {
            let _ = write!(l, " c{c} A#{}", LOWEST_MHZ[c].load(Ordering::Relaxed));
        }
    }
    let _ = writeln!(l);
    console(g, &l.b[..l.n]);
    exit(0);
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let b = |k: u64| if k < input_len { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input + k) as *const u8) } } else { 0 };
    let which = b(0);
    if which == 3 {
        watch(g, &b);
    }
    if which != 2 {
        class(g, "A510 (G#000F)", ferros_sys::CORES_LITTLE);
    }
    if which != 1 {
        class(g, "A715 (G#00F0)", ferros_sys::CORES_WORKERS);
    }
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(payload_main, grows 1 << 30);
