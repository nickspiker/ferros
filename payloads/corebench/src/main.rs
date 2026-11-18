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

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let which = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
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

ferros_sys::entry!(payload_main);
