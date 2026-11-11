//! copybench: how fast an arc moves (docs/ARCS.md step 3; owner, 2026-11-11: "I'm curious how fast it really is").
//! Two 16 MiB buffers in .bss, pinned to the X3 (memory pmu_timing_pin_x3: an unpinned timing lands on an A715), each copy timed on the generic timer: the compiler's memcpy, then a loop of 16-byte words (ldp/stp of q registers), sixteen copies each, best and mean in MB/s.
//! Spawned from the host: `ferros-bridge spawn copybench.bin`. Grant slots: 0 console.
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{console, exit, freq, now};

const SIZE: usize = 1 << 24;
const ROUNDS: u32 = 1 << 4;

static mut SRC: [u128; SIZE / 16] = [0; SIZE / 16];
static mut DST: [u128; SIZE / 16] = [0; SIZE / 16];

struct Line {
    b: [u8; 256],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &c in s.as_bytes() {
            if self.n < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        Ok(())
    }
}

/// MB/s for `bytes` in `ticks` of the generic timer.
fn rate(bytes: u64, ticks: u64) -> u64 {
    bytes * freq() / ticks / 1_000_000
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    ferros_sys::pin(ferros_sys::CORES_BIG);
    ferros_sys::sleep_until(now() + freq() / 1000);
    let (src, dst) = unsafe { (&mut *(&raw mut SRC), &mut *(&raw mut DST)) };
    // Every page touched once on both sides before any timing, and the source given content, so no copy is timed against a first touch.
    for (i, w) in src.iter_mut().enumerate() {
        *w = (i as u128).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    dst.fill(1);
    let mut report = |name: &str, f: &mut dyn FnMut(&mut [u128], &[u128])| {
        let (mut best, mut total) = (u64::MAX, 0u64);
        for _ in 0..ROUNDS {
            let t = now();
            f(dst, src);
            let d = now().wrapping_sub(t);
            best = best.min(d);
            total += d;
            core::hint::black_box(&dst[dst.len() - 1]);
        }
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = writeln!(l, "copybench: {name}: 16 MiB copied {} times, best {} MB/s, mean {} MB/s", ROUNDS, rate(SIZE as u64, best), rate(SIZE as u64 * ROUNDS as u64, total));
        console(grants, &l.b[..l.n]);
    };
    report("memcpy", &mut |d, s| unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), d.as_mut_ptr(), s.len()) });
    report("16-byte loop", &mut |d, s| {
        for (a, b) in d.iter_mut().zip(s.iter()) {
            *a = *b;
        }
    });
    let ok = src.iter().zip(dst.iter()).all(|(a, b)| a == b);
    console(grants, if ok { b"copybench: the copy reads back the same\n" } else { b"copybench: the copy DIFFERS\n" });
    exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

ferros_sys::entry!(payload_main);
