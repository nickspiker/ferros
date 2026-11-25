//! The POSIX calls C and C++ libraries make (feature `cxx`, STD-TARGET.md "C++ through ferros_rt"): threads, mutexes, condition variables and semaphores on ferros threads and the kernel's futex, the clocks on the generic counter, the thread's `errno`, and the few string and process calls openh264 makes.
//! Every wait blocks in the kernel (WAIT on a word, SLEEP to a deadline); nothing here polls, and the one spin is the mutex's, lock.rs's 128 loads before it sleeps.
//! The layouts of `pthread_t`, `pthread_mutex_t`, `pthread_cond_t` and `sem_t` are these, and the C headers that declare them (std/openh264-sys2/c/include) follow them.
//!
//! Every clock is the counter, monotonic since the device started: C code takes differences, and makes absolute deadlines for timed waits that the same clock then reads, so no wall time is needed and none is given (no UNIX time leaves this crate).

use core::alloc::Layout;
use core::cell::Cell;
use core::ffi::{c_char, c_int, c_long, c_void};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::futex::{futex_wait, futex_wait_until, futex_wake, futex_wake_all};
use crate::{lock, thread, time};

// Linux's numbers, which C code compares against.
const EAGAIN: c_int = 11;
const EBUSY: c_int = 16;
const EINVAL: c_int = 22;
const ETIMEDOUT: c_int = 110;

const NANOS: u128 = 1_000_000_000;

#[thread_local]
static ERRNO: Cell<c_int> = Cell::new(0);

/// The thread's `errno`.
#[unsafe(no_mangle)]
pub extern "C" fn __errno_location() -> *mut c_int {
    ERRNO.as_ptr()
}

fn fail(e: c_int) -> c_int {
    ERRNO.set(e);
    -1
}

// The clocks.

#[repr(C)]
pub struct Timespec {
    pub tv_sec: c_long,
    pub tv_nsec: c_long,
}

#[repr(C)]
pub struct Timeval {
    pub tv_sec: c_long,
    pub tv_usec: c_long,
}

/// Any clock: the counter's time since the device started.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clock_gettime(_clock: c_int, ts: *mut Timespec) -> c_int {
    let (t, hz) = (time::now(), time::hz());
    unsafe { ts.write(Timespec { tv_sec: (t / hz) as c_long, tv_nsec: ((t % hz) as u128 * NANOS / hz as u128) as c_long }) };
    0
}

/// The counter's time, as `clock_gettime` gives it, in microseconds: never the wall clock.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gettimeofday(tv: *mut Timeval, _tz: *mut c_void) -> c_int {
    let (t, hz) = (time::now(), time::hz());
    unsafe { tv.write(Timeval { tv_sec: (t / hz) as c_long, tv_usec: ((t % hz) as u128 * 1_000_000 / hz as u128) as c_long }) };
    0
}

/// An absolute time on that clock as a counter deadline.
/// The times come from the clocks above plus the caller's timeout (openh264's milliseconds), so they are never negative; a negative one would read as a deadline far off, a wait that never times out, loud in its own way.
fn deadline(at: &Timespec) -> u64 {
    let hz = time::hz() as u128;
    let t = at.tv_sec as u64 as u128 * hz + at.tv_nsec as u64 as u128 * hz / NANOS;
    // WHY max(1): 0 tells the kernel "no deadline"; a deadline at counter 0 is long past, and 1 says the same.
    (t as u64).max(1)
}

/// Sleep in the kernel until `usec` microseconds have passed.
#[unsafe(no_mangle)]
pub extern "C" fn usleep(usec: u32) -> c_int {
    let hz = time::hz() as u128;
    time::sleep_until(time::now() + (usec as u128 * hz / 1_000_000) as u64);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn sched_yield() -> c_int {
    thread::yield_now();
    0
}

/// The CPUs a process's threads run on: the kernel's four A715 workers, what std's `available_parallelism` says on ferros (the X3 runs the kernel and the display).
const CPUS: u64 = 4;

/// The CPU set openh264 counts its threads from (its Linux arm): `CPUS` bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sched_getaffinity(_pid: c_int, _size: usize, set: *mut u64) -> c_int {
    unsafe { set.write((1 << CPUS) - 1) };
    0
}

/// No process numbers on ferros: one process asks, so one number.
#[unsafe(no_mangle)]
pub extern "C" fn getpid() -> c_int {
    0
}

/// `strncpy`: at most `n` bytes of `s`, the rest of the `n` zeros.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strncpy(d: *mut c_char, s: *const c_char, n: usize) -> *mut c_char {
    let mut i = 0;
    unsafe {
        while i < n && *s.add(i) != 0 {
            *d.add(i) = *s.add(i);
            i += 1;
        }
        while i < n {
            *d.add(i) = 0;
            i += 1;
        }
    }
    d
}

// Threads.

/// A thread `pthread_create` started: what it runs, what it returned, and its ferros handle for the join; `pthread_t` is its address.
struct Pthread {
    start: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    arg: *mut c_void,
    ret: *mut c_void,
    handle: Option<thread::Handle>,
}

/// The thread's own `Pthread`, 0 on a thread `pthread_create` did not start (the main thread, std's threads).
#[thread_local]
static SELF: Cell<usize> = Cell::new(0);

#[repr(C)]
pub struct PthreadAttr {
    stack: usize,
    scope: c_int,
    policy: c_int,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_init(a: *mut PthreadAttr) -> c_int {
    unsafe { a.write(PthreadAttr { stack: 0, scope: 0, policy: 0 }) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pthread_attr_destroy(_a: *mut PthreadAttr) -> c_int {
    0
}

/// Kept and not acted on: every ferros thread is the system's to schedule.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_setscope(a: *mut PthreadAttr, scope: c_int) -> c_int {
    unsafe { (*a).scope = scope };
    0
}

/// Kept and not acted on: the kernel has one scheduling class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_setschedpolicy(a: *mut PthreadAttr, policy: c_int) -> c_int {
    unsafe { (*a).policy = policy };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_attr_setstacksize(a: *mut PthreadAttr, size: usize) -> c_int {
    unsafe { (*a).stack = size };
    0
}

extern "C" fn pthread_entry(p: u64) {
    let rec = p as *mut Pthread;
    SELF.set(p as usize);
    let ret = unsafe { ((*rec).start)((*rec).arg) };
    unsafe { (*rec).ret = ret };
}

/// A ferros thread on a stack from the heap (the attribute's size, or std's 1 MiB default), with a guard page under it (thread.rs).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_create(t: *mut usize, attr: *const PthreadAttr, start: unsafe extern "C" fn(*mut c_void) -> *mut c_void, arg: *mut c_void) -> c_int {
    let stack = match unsafe { attr.as_ref() } {
        Some(a) if a.stack != 0 => a.stack,
        _ => thread::DEFAULT_STACK,
    };
    let layout = Layout::new::<Pthread>();
    let rec = unsafe { crate::alloc::alloc(layout) } as *mut Pthread;
    if rec.is_null() {
        return EAGAIN;
    }
    unsafe { rec.write(Pthread { start, arg, ret: core::ptr::null_mut(), handle: None }) };
    match thread::spawn(pthread_entry, stack, rec as u64) {
        Ok(h) => {
            unsafe {
                (*rec).handle = Some(h);
                t.write(rec as usize);
            }
            0
        }
        Err(()) => {
            unsafe { crate::alloc::dealloc(rec as *mut u8, layout) };
            EAGAIN
        }
    }
}

/// Wait for the thread to end, hand back what it returned, and free it (its stack, TLS block and record).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_join(t: usize, ret: *mut *mut c_void) -> c_int {
    let rec = t as *mut Pthread;
    let Some(h) = (unsafe { (*rec).handle.take() }) else { return EINVAL };
    thread::join(h);
    unsafe {
        if !ret.is_null() {
            ret.write((*rec).ret);
        }
        crate::alloc::dealloc(rec as *mut u8, Layout::new::<Pthread>());
    }
    0
}

/// The thread's `pthread_t`: its record when `pthread_create` started it, else its TLS block's address (TPIDR_EL0), which no live record shares.
#[unsafe(no_mangle)]
pub extern "C" fn pthread_self() -> usize {
    let s = SELF.get();
    if s != 0 {
        return s;
    }
    let tp: usize;
    unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack, preserves_flags)) };
    tp
}

// Mutexes: lock.rs's futex lock on the mutex's one word (0 free, 1 held, 2 held and waited).

#[repr(C)]
pub struct PthreadMutex {
    word: AtomicU64,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_mutex_init(m: *mut PthreadMutex, _attr: *const c_void) -> c_int {
    unsafe { m.write(PthreadMutex { word: AtomicU64::new(0) }) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pthread_mutex_destroy(_m: *mut PthreadMutex) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_mutex_lock(m: *mut PthreadMutex) -> c_int {
    lock::acquire(unsafe { &(*m).word });
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_mutex_trylock(m: *mut PthreadMutex) -> c_int {
    if lock::try_acquire(unsafe { &(*m).word }) { 0 } else { EBUSY }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_mutex_unlock(m: *mut PthreadMutex) -> c_int {
    lock::release(unsafe { &(*m).word });
    0
}

// Condition variables: a sequence word, std's futex condvar's shape. A waiter reads it, lets the mutex go and sleeps while it still holds that value; a signal moves it on and wakes one sleeper, a broadcast all.

#[repr(C)]
pub struct PthreadCond {
    seq: AtomicU64,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_init(c: *mut PthreadCond, _attr: *const c_void) -> c_int {
    unsafe { c.write(PthreadCond { seq: AtomicU64::new(0) }) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pthread_cond_destroy(_c: *mut PthreadCond) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_wait(c: *mut PthreadCond, m: *mut PthreadMutex) -> c_int {
    let (seq, word) = unsafe { (&(*c).seq, &(*m).word) };
    let s = seq.load(Ordering::Relaxed);
    lock::release(word);
    futex_wait(seq, s, None);
    lock::acquire(word);
    0
}

/// As `pthread_cond_wait`, to an absolute time on the clock above: ETIMEDOUT when it passed first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_timedwait(c: *mut PthreadCond, m: *mut PthreadMutex, at: *const Timespec) -> c_int {
    let (seq, word) = unsafe { (&(*c).seq, &(*m).word) };
    let until = deadline(unsafe { &*at });
    let s = seq.load(Ordering::Relaxed);
    lock::release(word);
    let woke = futex_wait_until(seq, s, until);
    lock::acquire(word);
    if woke { 0 } else { ETIMEDOUT }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_signal(c: *mut PthreadCond) -> c_int {
    let seq = unsafe { &(*c).seq };
    seq.fetch_add(1, Ordering::Release);
    futex_wake(seq);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_cond_broadcast(c: *mut PthreadCond) -> c_int {
    let seq = unsafe { &(*c).seq };
    seq.fetch_add(1, Ordering::Release);
    futex_wake_all(seq);
    0
}

// Semaphores: the count, and how many waiters may be asleep, so a post with nobody waiting makes no syscall.
// A waiter counts itself in before it reads the count and sleeps while the count is 0; a post adds to the count before it reads the sleepers (both SeqCst), so either the poster sees the sleeper and wakes it, or the sleeper sees the count (the kernel's WAIT compares the word, so a post between the read and the sleep ends the sleep at once).

#[repr(C)]
pub struct Sem {
    count: AtomicU64,
    sleepers: AtomicU64,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sem_init(s: *mut Sem, _shared: c_int, value: u32) -> c_int {
    unsafe { s.write(Sem { count: AtomicU64::new(value as u64), sleepers: AtomicU64::new(0) }) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn sem_destroy(_s: *mut Sem) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sem_post(s: *mut Sem) -> c_int {
    let s = unsafe { &*s };
    s.count.fetch_add(1, Ordering::SeqCst);
    if s.sleepers.load(Ordering::SeqCst) != 0 {
        futex_wake(&s.count);
    }
    0
}

/// Take one from the count if it has one.
fn sem_take(s: &Sem) -> bool {
    let mut c = s.count.load(Ordering::Relaxed);
    while c != 0 {
        match s.count.compare_exchange_weak(c, c - 1, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(now) => c = now,
        }
    }
    false
}

/// Wait for the count, to `until` (counter ticks, 0 none): false when the deadline passed first.
fn sem_wait_until(s: &Sem, until: u64) -> bool {
    if sem_take(s) {
        return true;
    }
    s.sleepers.fetch_add(1, Ordering::SeqCst);
    let taken = loop {
        if sem_take(s) {
            break true;
        }
        if !futex_wait_until(&s.count, 0, until) && !sem_take(s) {
            break false;
        }
    };
    s.sleepers.fetch_sub(1, Ordering::SeqCst);
    taken
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sem_wait(s: *mut Sem) -> c_int {
    sem_wait_until(unsafe { &*s }, 0);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sem_trywait(s: *mut Sem) -> c_int {
    if sem_take(unsafe { &*s }) { 0 } else { fail(EAGAIN) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn sem_timedwait(s: *mut Sem, at: *const Timespec) -> c_int {
    if sem_wait_until(unsafe { &*s }, deadline(unsafe { &*at })) { 0 } else { fail(ETIMEDOUT) }
}
