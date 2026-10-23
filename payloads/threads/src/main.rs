//! Threads, the test (KERNEL.md "Threads, interrupts and cores"): two worker threads of this process each add to a shared counter a million times across slice ticks while the first thread waits on a word (`wait`); each worker bumps the word and wakes it when done (`wake`), then leaves (`thread_exit`). The first thread reports the count (two million if every add landed and the threads really shared the arc), how many times its wait returned, and the time. `bridge run target/aarch64-unknown-none/release/threads.bin`.
#![no_std]
#![no_main]

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::{console, exit, freq, now, thread_exit, thread_spawn, wait, wake};

static COUNTER: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static mut STACKS: [[u8; 16384]; 2] = [[0; 16384]; 2];
const ROUNDS: u64 = 1_000_000;

extern "C" fn worker(arg: u64) -> ! {
    for i in 0..ROUNDS {
        COUNTER.fetch_add(1, Ordering::Relaxed);
        // Some arithmetic between adds, so the loop spans slices rather than finishing inside one.
        core::hint::black_box(i.wrapping_mul(arg + 3));
    }
    DONE.fetch_add(1, Ordering::Release);
    wake(&DONE, 8);
    thread_exit(arg)
}

struct Line {
    buf: [u8; 192],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    // PIE: the `dyn Write` vtable below is an absolute address in .rodata, zero until relocated (a call through it is a jump to 0 — the first run of this test, 2026-10-23).
    unsafe { ferros_sys::relocate() };
    let t0 = now();
    let stacks = unsafe { &mut *(&raw mut STACKS) };
    let r0 = thread_spawn(worker, &mut stacks[0], 1);
    let r1 = thread_spawn(worker, &mut stacks[1], 2);
    let mut waits = 0u64;
    let mut early = 0u64;
    loop {
        let d = DONE.load(Ordering::Acquire);
        if d >= 2 {
            break;
        }
        waits += 1;
        if wait(&DONE, d, 0) == 1 {
            early += 1;
        }
    }
    let us = now().wrapping_sub(t0) * 1_000_000 / freq();
    let mut l = Line { buf: [0; 192], n: 0 };
    let _ = write!(l, "threads: spawn {r0} {r1}; counter {} (want {}); waits {waits} ({early} returned at once); {} us\n", COUNTER.load(Ordering::Relaxed), 2 * ROUNDS, us);
    console(grants, &l.buf[..l.n]);
    exit(if COUNTER.load(Ordering::Relaxed) == 2 * ROUNDS { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
