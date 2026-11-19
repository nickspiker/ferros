//! Mailboxes, the test and the measure (docs/KERNEL.md "Mailboxes"; owner, 2026-11-19: "an unlimited number of items accessing it"): a writer that publishes at a set rate, and readers, many threads of many processes, that wait on the mailbox, read each publish whole and say how late they woke.
//!
//! Spawn, the writer (the test mailbox's writer's region in slot 1):
//!   ferros-bridge spawn mbtest.bin 00<log2 Hz><count, 2 bytes LE><log2 words> --name mbw --dma test-mailbox
//! Each publish carries its number in every payload word, so a reader that took a torn publish for a whole one would see two numbers.
//! At the end: the publishes, what each cost the writer (its `publish`, the one wake included), and how many waiting threads each wake made runnable.
//!
//! Spawn, the readers (a view in slot 1: `test-view`, or `gps-view` for the GPS driver's):
//!   ferros-bridge spawn mbtest.bin 01<threads><count, 2 bytes LE><flags> --name mbr0 --dma test-view
//! Flags bit 0: the GPS mailbox, claimed with the view as the proof (so the driver runs) and read as ferros_mailbox::gps's layout.
//! Each thread waits for the next publish (SYS_WAIT on the sequence word), reads it with single looks (`try_read`, a look a publish landed in counted and looked at again), checks it, and keeps how late it woke (its read against the publish's counter stamp), until it has read `count` publishes or the mailbox died.
//! The first thread then says the sums for all of them on one line, and the wake delays as a histogram of powers of two microseconds.
#![no_std]
#![no_main]

use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};
use ferros_sys::mailbox::{gps, Got, Reader, Writer};
use ferros_sys::{cap, claim, console, exit, freq, now, sleep_until, thread_exit, thread_spawn, wait, wake};

/// The most reader threads a process runs.
const THREADS: usize = 1 << 4;
/// A reader thread's stack.
const STACK: usize = 1 << 13;
static mut STACKS: [[u8; STACK]; THREADS] = [[0; STACK]; THREADS];
/// The most payload words a test publish carries.
const WORDS: usize = 1 << 10;
/// The histogram's buckets: wake delays under 2^b us in bucket b, the last everything past.
const BUCKETS: usize = 1 << 4;

/// What the reader threads count, summed over all of them.
static FRESH: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicU64 = AtomicU64::new(0);
static TORN: AtomicU64 = AtomicU64::new(0);
static DEAD: AtomicU64 = AtomicU64::new(0);
static LATE_SUM: AtomicU64 = AtomicU64::new(0);
static LATE_MAX: AtomicU64 = AtomicU64::new(0);
static HIST: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
/// GPS mode: the publishes with a fix, and the most tracks one carried.
static FIXES: AtomicU64 = AtomicU64::new(0);
static TRACKS: AtomicU64 = AtomicU64::new(0);
/// The reader threads done.
static DONE: AtomicU64 = AtomicU64::new(0);
/// The readers' run: the view's base, the publishes to read, the flags (set before the threads start).
static RUN: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

struct Line {
    buf: [u8; 512],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            // WHY the bound: a line longer than the buffer is cut, not written past it.
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        Ok(())
    }
}

fn input(at: u64, len: u64, k: u64) -> u8 {
    // WHY the bound: the input is the host's; a byte past it reads as 0.
    if k < len { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + at + k) as *const u8) } } else { 0 }
}

/// Microseconds in `ticks` of the counter.
fn us(ticks: u64) -> u64 {
    ticks * 1_000_000 / freq()
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, at: u64, len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let mut l = Line { buf: [0; 512], n: 0 };
    let count = input(at, len, 2) as u64 | (input(at, len, 3) as u64) << 8;
    if input(at, len, 0) == 0 {
        // WHY the min: the size is the host's input; a publish past the payload buffer would read past it.
        writer(grants, input(at, len, 1) as u32, count, 1usize << input(at, len, 4).min(10), &mut l);
    }
    // Readers: the view mapped, GPS claimed in GPS mode, the threads started; the first is this one.
    // WHY the bounds: the count is the host's input; a thread past the stacks would have none, and no thread at all reads nothing.
    let threads = (input(at, len, 1) as usize).clamp(1, THREADS);
    let flags = input(at, len, 4) as u64;
    let token = cap(grants, 1);
    let Some(_) = Reader::map(token) else {
        let _ = write!(l, "mbtest: no view in slot 1\n");
        console(grants, &l.buf[..l.n]);
        exit(1);
    };
    if flags & 1 != 0 && !claim::claim(claim::GPS, token) {
        let _ = write!(l, "mbtest: the GPS claim was refused\n");
        console(grants, &l.buf[..l.n]);
        exit(1);
    }
    RUN[0].store(token, Ordering::Relaxed);
    RUN[1].store(count, Ordering::Relaxed);
    RUN[2].store(flags, Ordering::Release);
    let stacks = unsafe { &mut *(&raw mut STACKS) };
    for (t, s) in stacks.iter_mut().enumerate().take(threads).skip(1) {
        thread_spawn(reader, s, t as u64);
    }
    let t0 = now();
    read_all(0);
    loop {
        let d = DONE.load(Ordering::Acquire);
        if d >= threads as u64 {
            break;
        }
        wait(&DONE, d, 0);
    }
    if flags & 1 != 0 {
        claim::release(claim::GPS);
    }
    let f = FRESH.load(Ordering::Relaxed);
    let _ = write!(l, "mbtest: A#{threads} readers, A#{f} whole reads in A#{} ms, A#{} publishes skipped, A#{} looks a publish landed in, A#{} torn reads taken for whole, A#{} saw it dead; woke A#{} us late on average, A#{} us at most", us(now() - t0) / 1000, SKIPPED.load(Ordering::Relaxed), BUSY.load(Ordering::Relaxed), TORN.load(Ordering::Relaxed), DEAD.load(Ordering::Relaxed), us(LATE_SUM.load(Ordering::Relaxed)) / f.max(1), us(LATE_MAX.load(Ordering::Relaxed)));
    if flags & 1 != 0 {
        let _ = write!(l, "; A#{} reads held a fix, A#{} tracks at most", FIXES.load(Ordering::Relaxed), TRACKS.load(Ordering::Relaxed));
    }
    let _ = write!(l, "\nmbtest: woke under 2^b us, b = 0..:");
    for h in HIST.iter() {
        let _ = write!(l, " A#{}", h.load(Ordering::Relaxed));
    }
    let _ = write!(l, "\n");
    console(grants, &l.buf[..l.n]);
    exit(if TORN.load(Ordering::Relaxed) == 0 { 0 } else { 2 });
}

extern "C" fn reader(t: u64) -> ! {
    read_all(t);
    thread_exit(0)
}

/// One reader thread: `count` publishes read, or the mailbox dead.
fn read_all(_t: u64) {
    let (token, count, flags) = (RUN[0].load(Ordering::Relaxed), RUN[1].load(Ordering::Relaxed), RUN[2].load(Ordering::Acquire));
    let r = Reader::map(token).unwrap();
    let mut out = [0u64; WORDS];
    let (mut seen, mut got) = (r.seq() & !1, 0u64);
    while got < count {
        r.wait(seen, 0);
        let g = loop {
            match r.try_read(&mut out) {
                Got::Busy { seq } => {
                    BUSY.fetch_add(1, Ordering::Relaxed);
                    if seq & 1 != 0 {
                        r.wait(seq, 0);
                    }
                }
                g => break g,
            }
        };
        match g {
            Got::Fresh { seq, kind, len, at } => {
                let late = now().wrapping_sub(at);
                LATE_SUM.fetch_add(late, Ordering::Relaxed);
                LATE_MAX.fetch_max(late, Ordering::Relaxed);
                // WHY the min: a delay past the last bucket's bound is counted in it.
                let b = (64 - us(late).leading_zeros() as usize).min(BUCKETS - 1);
                HIST[b].fetch_add(1, Ordering::Relaxed);
                // A sequence more than two on: publishes came and went while this thread slept or read.
                if seen != 0 && seq > seen + 2 {
                    SKIPPED.fetch_add((seq - seen - 2) / 2, Ordering::Relaxed);
                }
                seen = seq;
                got += 1;
                FRESH.fetch_add(1, Ordering::Relaxed);
                if flags & 1 != 0 {
                    if let Some(gp) = (kind == gps::KIND).then(|| gps::Gps::decode(&out)).flatten() {
                        FIXES.fetch_add(gp.valid as u64, Ordering::Relaxed);
                        TRACKS.fetch_max(gp.n as u64, Ordering::Relaxed);
                    }
                } else {
                    // The test writer's payload: its publish number in every word, so two numbers are a torn read.
                    // WHY the min: `len` is the writer's word; the check stays in this buffer.
                    let n = len.min(WORDS);
                    if out[..n].iter().any(|&w| w != out[0]) {
                        TORN.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Got::Dead { seq } => {
                DEAD.fetch_add(1, Ordering::Relaxed);
                // A test writer gone: done. The GPS driver may come back (its claim stands): wait on.
                if flags & 1 == 0 {
                    break;
                }
                seen = seq;
            }
            Got::Never => {}
            Got::Busy { .. } => {}
        }
    }
    DONE.fetch_add(1, Ordering::Release);
    wake(&DONE, 1);
}

/// The writer: `count` publishes of `words` words at 2^`log_hz` a second, each stamped and woken; then what they cost.
fn writer(grants: u64, log_hz: u32, count: u64, words: usize, l: &mut Line) -> ! {
    let Some(w) = Writer::take(cap(grants, 1)) else {
        let _ = write!(l, "mbtest: no test mailbox to write (slot 1), or another writer\n");
        console(grants, &l.buf[..l.n]);
        exit(1);
    };
    // WHY the min: the rate is the host's input; past 2^16 a second the period is under a microsecond of the counter, and a shift past 63 is no number.
    let period = freq() >> log_hz.min(16);
    let mut payload = [0u64; WORDS];
    let (mut cost_sum, mut cost_max, mut woke_sum, mut woke_min, mut woke_max) = (0u64, 0u64, 0u64, u64::MAX, 0u64);
    let t0 = now();
    for k in 0..count {
        // The test's own pace: a deadline per publish, the rate it measures at.
        sleep_until(t0 + (k + 1) * period);
        payload[..words].fill(k + 1);
        let a = now();
        let (_, woke) = w.publish(1, &payload[..words]);
        let c = now() - a;
        cost_sum += c;
        cost_max = cost_max.max(c);
        woke_sum += woke;
        woke_min = woke_min.min(woke);
        woke_max = woke_max.max(woke);
    }
    let _ = write!(l, "mbtest: A#{count} publishes of A#{words} words at A#{} a second; each cost the writer A#{} ns on average, A#{} ns at most (the publish and its one wake); each woke A#{} to A#{} waiting threads, A#{} on average\n", 1u64 << log_hz, cost_sum * 1_000_000_000 / freq() / count.max(1), cost_max * 1_000_000_000 / freq(), woke_min, woke_max, woke_sum / count.max(1));
    console(grants, &l.buf[..l.n]);
    exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_report(env!("CARGO_BIN_NAME"), info)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
