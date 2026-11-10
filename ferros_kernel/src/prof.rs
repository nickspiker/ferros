//! The whole-system sampling profiler (owner, 2026-11-08: "we need to flame test this thing"). Every core that has a ring takes a tick from its own EL2 virtual timer (CNTHV, PPI 28) at `hz` a second and writes one record into its own ring: where the core was (a process at EL0, or the kernel), which thread of which process it ran, and the call chain by frame pointers. `tools/flame` pulls the rings over the bridge, symbolises every address with the right ELF and prints the report and a flame graph.
//!
//! The timer, not the PMU: the PMU is open to EL0 (pmu.rs), where processes time their own loops with the cycle counter; a profiler that reset it under them would spoil every such measurement. The timer also ticks through `wfe`, so idle shows as idle.
//!
//! **IRQs masked:** a syscall runs from its trap to its eret with IRQs masked (exception entry masks them, and nothing in the trap path opens them), so a tick that falls inside one is taken only on the way out, at the first instruction after the `svc`, and the time looks like the caller's. Two things undo that. The timer's TVAL, read at the tick, says how long the tick waited (`late`); the record's weight is the number of periods that passed (1 + late / period), so a 100 ms syscall weighs 100 ms however many ticks it swallowed. And each syscall leaves a note on its core as it enters (the caller, its frame pointer, link register and return address) and as it leaves: a tick that waited past the period's eighth and whose deadline fell before the last syscall on this core ended belongs to that syscall, and its record carries the caller's own user stack and the syscall's number (flag `SYSCALL`). Where in the kernel a syscall spends its time cannot be sampled this way (no interrupt reaches masked code on these cores: no FEAT_NMI, and group 0 belongs to EL3); `diag "ufs runs"`, `diag sched` and timing inside the kernel answer that.
//!
//! **Fault-safe call chains:** the handler follows x29 frame records only inside the interrupted thread's own arc (EL0) or the kernel's stacks (`__bss_start` to `__stack_top`: the worker and X3 stacks are in .bss, the main stack just past it), 8-byte aligned, each next record above the last (so the walk ends), at most `DEPTH` addresses. It dereferences nothing else. A build without frame pointers still gives the sampled address and the link register: a flat profile.
//!
//! **Never the kernel lock:** the tick reads what this core's own `cur_set` left in `CUR` (process.rs, written under the lock before every eret into a thread) and writes only this core's ring and counters. The names of the threads seen (`SEEN`) are kept by `cur_set` itself, under the lock.
//!
//! **A ring:** a run of u64 words per core, records back to back until it is full (the start of a run is kept, later ticks are counted as dropped). A record:
//! - word 0: CNTPCT at the tick;
//! - word 1: bits 0-7 the number of addresses n, bits 8-15 the flags (`F_EL2` the core was in the kernel, `F_HELD` the tick waited on masked IRQs, `F_SYSCALL` the stack is the syscall caller's), bits 16-31 the syscall number (x8) when `F_SYSCALL`, bits 32-63 the weight in periods;
//! - word 2: bits 0-31 the thread index plus one (0: no thread), bits 32-63 the low half of its process's epoch;
//! - then n addresses: the sampled PC, the link register, then each frame's return address. A process's are offsets into its image (its arc base plus the code offset subtracted: the ELF links at 0), the kernel's are its own addresses.
//!
//! `diag "prof start [hz]"` (default 4096), `diag "prof stop"`, `diag "prof"` (state), `diag "prof pull"` (the index again), `diag "prof free"` (the rings back to the heap). The index is a VSF document (sections `prof`, one `core` per ring, one `thread` per thread seen) whose address and length sit in `FERROS_PROF_ANCHOR`, which the host finds by symbol in the kernel ELF.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::{Acquire, Relaxed}};

/// Addresses kept per record: the PC, the link register, and up to 30 return addresses.
pub const DEPTH: usize = 1 << 5;
/// Words before a record's addresses.
const HEAD: usize = 3;
/// A ring's bytes per core when the heap has them (halved until it does, down to `RING_MIN`): 1 MiB is ~4000 records of a 30-deep chain, a second at 4096 Hz.
const RING: usize = 1 << 20;
const RING_MIN: usize = 1 << 16;
/// The default rate: 4096 ticks a second on each core.
const HZ_DEFAULT: u64 = 1 << 12;
/// The highest rate taken: a tick costs about a microsecond and a half, so 65536 a second would hold every core ~10 % of the time.
const HZ_MAX: u64 = 1 << 16;
/// The EL2 virtual timer's interrupt (PPI 12, INTID 28: the GIC's standard place for it, as the zuma DT's PMU sits at the standard PPI 7).
pub const INTID: u32 = 28;

pub const F_EL2: u64 = 1;
pub const F_HELD: u64 = 2;
pub const F_SYSCALL: u64 = 4;

/// The index's anchor, found by its symbol in the kernel ELF: a magic word, the index's address, its length, and the run it describes (counting from 1).
pub const MAGIC: u64 = u64::from_le_bytes(*b"ferrprof");
#[used]
#[unsafe(no_mangle)]
pub static FERROS_PROF_ANCHOR: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

static ON: AtomicBool = AtomicBool::new(false);
/// Timer ticks between samples (CNTFRQ / hz).
static PERIOD: AtomicU64 = AtomicU64::new(0);
static HZ: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicU64 = AtomicU64::new(0);
static STOPPED: AtomicU64 = AtomicU64::new(0);
static RUN: AtomicU64 = AtomicU64::new(0);

/// One core's ring and its counters; only that core's tick writes them while a run is on.
struct Ring {
    /// The ring's address (0: this core has none) and its length in words.
    buf: AtomicU64,
    cap: AtomicU64,
    /// Words written.
    pos: AtomicU64,
    /// Ticks taken, records written, records whose tick had waited (`F_HELD`), periods dropped for want of room.
    ticks: AtomicU64,
    samples: AtomicU64,
    held: AtomicU64,
    dropped: AtomicU64,
    /// Counter ticks spent inside the handler (from its first statement to its last), for the overhead.
    cost: AtomicU64,
    /// The run this core's timer was armed for (0: stopped).
    armed: AtomicU64,
}
static RINGS: [Ring; 16] = [const { Ring { buf: AtomicU64::new(0), cap: AtomicU64::new(0), pos: AtomicU64::new(0), ticks: AtomicU64::new(0), samples: AtomicU64::new(0), held: AtomicU64::new(0), dropped: AtomicU64::new(0), cost: AtomicU64::new(0), armed: AtomicU64::new(0) } }; 16];
/// The rings' memory, owned here and touched only by the service loop's `prof` commands (one core); the ticks reach it through `Ring::buf`.
static mut STORE: [Option<alloc::vec::Vec<u64>>; 16] = [const { None }; 16];
/// The last index document (`pull`); the anchor points into it.
static mut INDEX: Option<alloc::vec::Vec<u8>> = None;

/// What a core runs (process.rs `cur_set`): the thread plus one (0 none), its process's epoch, the image base its addresses are taken from (arc base + code offset), and the arc's bounds for the frame walk.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub thread: usize,
    pub epoch: u64,
    pub base: usize,
    pub lo: usize,
    pub hi: usize,
}
struct Cur {
    thread: AtomicU64,
    epoch: AtomicU64,
    base: AtomicU64,
    lo: AtomicU64,
    hi: AtomicU64,
}
static CUR: [Cur; 16] = [const { Cur { thread: AtomicU64::new(0), epoch: AtomicU64::new(0), base: AtomicU64::new(0), lo: AtomicU64::new(0), hi: AtomicU64::new(0) } }; 16];
/// The last syscall on each core: entered at `t0`, left at `t1` (u64::MAX while inside), its number, its caller (as `CUR`) and the caller's ELR, x29 and x30 at the `svc`.
struct Call {
    t0: AtomicU64,
    t1: AtomicU64,
    nr: AtomicU64,
    thread: AtomicU64,
    epoch: AtomicU64,
    base: AtomicU64,
    lo: AtomicU64,
    hi: AtomicU64,
    elr: AtomicU64,
    fp: AtomicU64,
    lr: AtomicU64,
}
static CALL: [Call; 16] = [const { Call { t0: AtomicU64::new(0), t1: AtomicU64::new(0), nr: AtomicU64::new(0), thread: AtomicU64::new(0), epoch: AtomicU64::new(0), base: AtomicU64::new(0), lo: AtomicU64::new(0), hi: AtomicU64::new(0), elr: AtomicU64::new(0), fp: AtomicU64::new(0), lr: AtomicU64::new(0) } }; 16];

/// A thread seen during the run, for the host's names: kept under the kernel lock (process.rs `cur_set` and `seed`).
struct Seen {
    thread: usize,
    epoch: u64,
    base: usize,
    process: alloc::vec::Vec<u8>,
    name: alloc::vec::Vec<u8>,
}
static mut SEEN: alloc::vec::Vec<Seen> = alloc::vec::Vec::new();
/// Per thread index, the low half of the epoch it was last seen with (0: not yet): the common case answered without searching `SEEN`.
static SEEN_AT: [AtomicU64; 1 << 10] = [const { AtomicU64::new(0) }; 1 << 10];

/// This core's index: MPIDR Aff1, four bits (husky: 0-8), so every per-core array of 16 holds it.
fn core() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)) };
    ((mpidr >> 8) & 0xF) as usize
}

fn timer_set(ticks: u64) {
    unsafe { core::arch::asm!("msr S3_4_C14_C3_0, {t}", "mov {c}, #1", "msr S3_4_C14_C3_1, {c}", "isb", t = in(reg) ticks, c = out(reg) _, options(nostack)) };
}
fn timer_off() {
    unsafe { core::arch::asm!("msr S3_4_C14_C3_1, xzr", "isb", options(nostack)) };
}

pub fn on() -> bool {
    ON.load(Relaxed)
}

/// process.rs `cur_set`: what this core runs now (None: the kernel itself). Always kept, so a run that starts mid-slice still knows the thread.
pub fn running(c: usize, ctx: Option<Ctx>) {
    let s = &CUR[c & 0xF];
    match ctx {
        Some(x) => {
            s.epoch.store(x.epoch, Relaxed);
            s.base.store(x.base as u64, Relaxed);
            s.lo.store(x.lo as u64, Relaxed);
            s.hi.store(x.hi as u64, Relaxed);
            s.thread.store(x.thread as u64 + 1, Relaxed);
        }
        None => s.thread.store(0, Relaxed),
    }
}

/// Whether thread `t` of the process with `epoch` is not yet in `SEEN` (kernel lock held).
pub fn unseen(t: usize, epoch: u64) -> bool {
    let e = epoch & 0xFFFF_FFFF;
    // WHY: thread indices grow with the kernel's thread table, which has no fixed bound; the array answers the indices it has and the table is searched for the rest.
    if t < SEEN_AT.len() {
        return SEEN_AT[t].load(Relaxed) != e;
    }
    !unsafe { (*(&raw const SEEN)).iter().any(|s| s.thread == t && s.epoch & 0xFFFF_FFFF == e) }
}

/// Keep thread `t`'s names (kernel lock held).
pub fn seen(t: usize, epoch: u64, base: usize, process: &[u8], name: alloc::vec::Vec<u8>) {
    if t < SEEN_AT.len() {
        SEEN_AT[t].store(epoch & 0xFFFF_FFFF, Relaxed);
    }
    unsafe { (*(&raw mut SEEN)).push(Seen { thread: t, epoch, base, process: process.to_vec(), name }) };
}

/// A syscall (or any trap from EL0) enters on this core: the caller and where it called from, before the trap may switch threads.
pub fn enter(nr: u64, elr: u64, fp: u64, lr: u64) {
    if !ON.load(Relaxed) {
        return;
    }
    let c = core();
    let (s, k) = (&CUR[c], &CALL[c]);
    k.t1.store(u64::MAX, Relaxed);
    k.t0.store(crate::platform::cntpct(), Relaxed);
    k.nr.store(nr, Relaxed);
    k.thread.store(s.thread.load(Relaxed), Relaxed);
    k.epoch.store(s.epoch.load(Relaxed), Relaxed);
    k.base.store(s.base.load(Relaxed), Relaxed);
    k.lo.store(s.lo.load(Relaxed), Relaxed);
    k.hi.store(s.hi.load(Relaxed), Relaxed);
    k.elr.store(elr, Relaxed);
    k.fp.store(fp, Relaxed);
    k.lr.store(lr, Relaxed);
}

/// The trap is on its way back to EL0 (or out of it).
pub fn leave() {
    if !ON.load(Relaxed) {
        return;
    }
    let k = &CALL[core()];
    // Only a syscall whose entry was noted: a run started inside a syscall must not lend it the last noted caller.
    if k.t1.load(Relaxed) == u64::MAX {
        k.t1.store(crate::platform::cntpct(), Relaxed);
    }
}

unsafe extern "C" {
    static __bss_start: u8;
    static __stack_top: u8;
}

/// Follow x29 frame records from `fp`, writing each return address minus `base` at `out`, while the record lies in [lo, hi): returns how many were written (at most `room`).
/// SAFETY: `out` has `room` words; every load is of an 8-byte-aligned address whose 16 bytes lie inside [lo, hi), which the caller guarantees is mapped memory (the running thread's arc, or the kernel's image and stack).
unsafe fn walk(mut fp: u64, lo: u64, hi: u64, base: u64, out: *mut u64, room: usize) -> usize {
    let mut n = 0;
    // WHY each bound: x29 is the interrupted code's to set, and a function built without frame pointers uses it as it likes. A record outside [lo, hi) or misaligned would be a load from anywhere (a fault in the interrupt handler, or a read of device memory); a next record not above this one could loop; `room` is the record's space in the ring. The end is tested as `fp <= hi - 16`, not `fp + 16 <= hi`: an x29 near the top of the address space would wrap the sum past the test. `hi >= 16` first, for the no-arc case (lo = hi = 0), where `hi - 16` would wrap.
    while n < room && fp & 7 == 0 && fp >= lo && hi >= 16 && fp <= hi - 16 {
        let next = unsafe { core::ptr::read_volatile(fp as *const u64) };
        let ret = unsafe { core::ptr::read_volatile((fp + 8) as *const u64) };
        // An offset into the image; a return address from a damaged record may lie below the base, and wraps to one the host shows as unknown.
        unsafe { out.add(n).write(ret.wrapping_sub(base)) };
        n += 1;
        if next <= fp {
            break;
        }
        fp = next;
    }
    n
}

/// The tick (main.rs `.Lirq_prof`, IRQs masked, every caller-saved register saved): `elr`/`spsr` as the interrupt found them, `fp`/`lr` the interrupted x29 and x30.
#[unsafe(no_mangle)]
pub extern "C" fn ferros_prof_tick(elr: u64, spsr: u64, fp: u64, lr: u64) {
    let t0 = crate::platform::cntpct();
    let c = core();
    let r = &RINGS[c];
    // How long ago the deadline passed, on the timer's own counter (TVAL = CVAL - count, signed 32 bits; read before the re-arm rewrites it).
    let tv: u64;
    unsafe { core::arch::asm!("mrs {}, S3_4_C14_C3_0", out(reg) tv, options(nomem, nostack)) };
    if !ON.load(Acquire) {
        timer_off();
        r.armed.store(0, Relaxed);
        return;
    }
    let period = PERIOD.load(Relaxed);
    timer_set(period);
    r.ticks.fetch_add(1, Relaxed);
    let tv = tv as u32 as i32;
    // WHY: an interrupt is taken only after the deadline (tv <= 0); a positive tv is a level interrupt whose line had already dropped when it was acknowledged, which waited for nothing.
    let late = if tv < 0 { (-(tv as i64)) as u64 } else { 0 };
    let weight = 1 + late / period;
    let held = late > period >> 3;
    let (buf, cap, pos) = (r.buf.load(Relaxed), r.cap.load(Relaxed), r.pos.load(Relaxed));
    // WHY: the ring is fixed; a record that might not fit is dropped (counted) rather than written past its end.
    if buf == 0 || pos + (HEAD + DEPTH) as u64 > cap {
        r.dropped.fetch_add(weight, Relaxed);
        r.cost.fetch_add(crate::platform::cntpct() - t0, Relaxed);
        return;
    }
    let w = (buf as *mut u64).wrapping_add(pos as usize);
    let el2 = spsr & 0xF != 0;
    let k = &CALL[c];
    // The deadline, on CNTPCT: t0 - late cannot wrap, since the timer was armed after the counter started and the wait is shorter than the time since. A held tick whose deadline came before the last syscall here had ended was waiting in that syscall (taken at EL0 after its eret, or in the kernel after a trap that left EL0 for good); t1 = u64::MAX would be a syscall still running here, which a tick cannot interrupt.
    let k1 = k.t1.load(Relaxed);
    let syscall = held && k.t0.load(Relaxed) != 0 && k1 != u64::MAX && k1 >= t0 - late;
    let mut flags = if held { F_HELD } else { 0 };
    let (thread, epoch, nr, base, lo, hi, pc, lr, fp) = if syscall {
        flags |= F_SYSCALL;
        (k.thread.load(Relaxed), k.epoch.load(Relaxed), k.nr.load(Relaxed) & 0xFFFF, k.base.load(Relaxed), k.lo.load(Relaxed), k.hi.load(Relaxed), k.elr.load(Relaxed), k.lr.load(Relaxed), k.fp.load(Relaxed))
    } else {
        let s = &CUR[c];
        let thread = s.thread.load(Relaxed);
        let epoch = if thread != 0 { s.epoch.load(Relaxed) } else { 0 };
        if el2 {
            flags |= F_EL2;
            let lo = (&raw const __bss_start) as u64;
            let hi = (&raw const __stack_top) as u64;
            (thread, epoch, 0, 0, lo, hi, elr, lr, fp)
        } else if thread != 0 {
            (thread, epoch, 0, s.base.load(Relaxed), s.lo.load(Relaxed), s.hi.load(Relaxed), elr, lr, fp)
        } else {
            // At EL0 with no thread noted: the addresses as they are, no walk (no arc to bound it).
            (0, 0, 0, 0, 0, 0, elr, lr, fp)
        }
    };
    unsafe {
        w.add(3).write(pc.wrapping_sub(base));
        w.add(4).write(lr.wrapping_sub(base));
        let n = 2 + walk(fp, lo, hi, base, w.add(5), DEPTH - 2);
        w.write(t0);
        w.add(1).write(n as u64 | flags << 8 | nr << 16 | weight << 32);
        w.add(2).write(thread & 0xFFFF_FFFF | epoch << 32);
        r.pos.store(pos + (HEAD + n) as u64, Relaxed);
    }
    r.samples.fetch_add(1, Relaxed);
    if held {
        r.held.fetch_add(1, Relaxed);
    }
    r.cost.fetch_add(crate::platform::cntpct() - t0, Relaxed);
}

/// This core's timer on, if a run is on and it has a ring (smp.rs: each worker's loop; the kernel's core arms itself at `start`).
pub fn poll() {
    if !ON.load(Acquire) {
        return;
    }
    let r = &RINGS[core()];
    // Armed for an earlier run only (its flag left set by a core switched off before its timer stopped, which comes back with the timer reset): armed again.
    if r.armed.load(Relaxed) != RUN.load(Relaxed) && r.buf.load(Relaxed) != 0 {
        arm_here();
    }
}

fn arm_here() {
    crate::killswitch::enable_ppi_here(INTID);
    RINGS[core()].armed.store(RUN.load(Relaxed), Relaxed);
    timer_set(PERIOD.load(Relaxed));
}

/// The cores a run samples: the kernel's (this one) and every worker that is up.
fn cores() -> u16 {
    let mut m = 1u16 << core();
    let up = crate::smp::UP.load(Relaxed);
    for (k, &aff) in crate::smp::WORKERS.iter().enumerate() {
        if up & (1 << k) != 0 {
            m |= 1 << ((aff >> 8) & 0xF);
        }
    }
    m
}

/// Whether every tick of the last run is over: a tick that read ON before `stop` cleared it writes for a few microseconds more, and a run started or freed under it would lose its record or take its memory. Two periods after the stop none can be left (a tick held by masked IRQs waits before it reads ON, not after).
fn settled() -> bool {
    let t1 = STOPPED.load(Relaxed);
    t1 == 0 || crate::platform::cntpct() - t1 >= PERIOD.load(Relaxed) << 1
}

/// `diag "prof …"`.
pub fn diag(args: &str, out: &mut alloc::vec::Vec<u8>) {
    let mut it = args.split_whitespace();
    match it.next() {
        Some("start") => start(it.next(), out),
        Some("stop") => {
            stop();
            report(out);
            pull(out);
        }
        Some("pull") => pull(out),
        Some("free") => free(out),
        None => report(out),
        Some(_) => out.extend_from_slice(b"prof [start [hz] | stop | pull | free]\n"),
    }
}

fn start(hz: Option<&str>, out: &mut alloc::vec::Vec<u8>) {
    if ON.load(Relaxed) {
        out.extend_from_slice(b"prof: already running (prof stop first)\n");
        return;
    }
    if !settled() {
        out.extend_from_slice(b"prof: the last run stopped a moment ago; again\n");
        return;
    }
    let hz = match hz.map(|h| h.trim_start_matches("A#").parse::<u64>()) {
        None => HZ_DEFAULT,
        // WHY: the rate comes from the bridge; 0 would divide by zero below, and above HZ_MAX the ticks would eat the cores they measure.
        Some(Ok(h)) if h != 0 && h <= HZ_MAX => h,
        _ => {
            out.extend_from_slice(alloc::format!("prof: the rate is a number of ticks a second, A#1 to A#{HZ_MAX}\n").as_bytes());
            return;
        }
    };
    let f = crate::platform::cntfrq();
    let mask = cores();
    // The rings: kept from the last run when there is one, else made, each the largest of RING, RING/2, … RING_MIN the heap gives.
    let store = unsafe { &mut *(&raw mut STORE) };
    let mut bytes = 0;
    for c in 0..16 {
        if mask & (1 << c) == 0 {
            // A core that is not up now (a worker gone since the last run) keeps its memory but is out of this run.
            RINGS[c].buf.store(0, Relaxed);
            continue;
        }
        if store[c].is_none() {
            let mut size = RING;
            while size >= RING_MIN {
                let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
                if v.try_reserve_exact(size >> 3).is_ok() {
                    v.resize(size >> 3, 0);
                    store[c] = Some(v);
                    break;
                }
                size >>= 1;
            }
        }
        let r = &RINGS[c];
        match &mut store[c] {
            Some(v) => {
                r.buf.store(v.as_mut_ptr() as u64, Relaxed);
                r.cap.store(v.len() as u64, Relaxed);
                bytes += v.len() << 3;
            }
            None => r.buf.store(0, Relaxed),
        }
        for a in [&r.pos, &r.ticks, &r.samples, &r.held, &r.dropped, &r.cost] {
            a.store(0, Relaxed);
        }
        CALL[c].t0.store(0, Relaxed);
        CALL[c].t1.store(0, Relaxed);
    }
    crate::process::with_lock(|| {
        unsafe { (*(&raw mut SEEN)).clear() };
        for a in SEEN_AT.iter() {
            a.store(0, Relaxed);
        }
        crate::process::prof_seed();
    });
    HZ.store(hz, Relaxed);
    PERIOD.store(f / hz, Relaxed);
    STARTED.store(crate::platform::cntpct(), Relaxed);
    STOPPED.store(0, Relaxed);
    RUN.fetch_add(1, Relaxed);
    // Release: a tick that sees the run on (Acquire) sees its period and rings.
    ON.store(true, core::sync::atomic::Ordering::Release);
    // The kernel's core now; the workers from their loops, woken by SGI 1 (the wake process::kick sends) in case they sleep.
    arm_here();
    unsafe { core::arch::asm!("dsb sy", "sev", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) (1u64 << 40) | (1u64 << 24)) };
    out.extend_from_slice(alloc::format!("prof: sampling at A#{hz} Hz on cores {}, rings A#{} KiB in all\n", cores_text(mask), bytes >> 10).as_bytes());
}

fn cores_text(mask: u16) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for c in 0..16 {
        if mask & (1 << c) != 0 {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&alloc::format!("A#{c}"));
        }
    }
    s
}

fn stop() {
    if ON.swap(false, Relaxed) {
        STOPPED.store(crate::platform::cntpct(), Relaxed);
    }
    timer_off();
    RINGS[core()].armed.store(0, Relaxed);
}

/// A RELOAD is about to replace the kernel: the timer off on this core (the workers are switched off by smp::stop_all), so the next kernel never meets a tick it has no handler for.
pub fn shutdown() {
    stop();
}

fn report(out: &mut alloc::vec::Vec<u8>) {
    let f = crate::platform::cntfrq();
    let now = crate::platform::cntpct();
    let (t0, t1) = (STARTED.load(Relaxed), STOPPED.load(Relaxed));
    let span = if t1 != 0 { t1 - t0 } else { now - t0 };
    out.extend_from_slice(alloc::format!("prof: {} at A#{} Hz, A#{} ms\n", if ON.load(Relaxed) { "running" } else if t0 == 0 { "never started" } else { "stopped" }, HZ.load(Relaxed), span * 1000 / f).as_bytes());
    for (c, r) in RINGS.iter().enumerate() {
        if r.buf.load(Relaxed) == 0 {
            continue;
        }
        let (ticks, samples, held, dropped, cost) = (r.ticks.load(Relaxed), r.samples.load(Relaxed), r.held.load(Relaxed), r.dropped.load(Relaxed), r.cost.load(Relaxed));
        // ns per tick and the share of the core, in hundredths of a percent; nothing to divide when the core took no tick.
        let ns = if ticks != 0 { cost * 1_000_000_000 / f / ticks } else { 0 };
        let share = if span != 0 { cost * 10_000 / span } else { 0 };
        out.extend_from_slice(alloc::format!("core A#{c}: A#{samples} records (A#{held} held), A#{dropped} periods dropped, A#{ticks} ticks, ring A#{}/A#{} words, handler A#{ns} ns a tick = A#{}.{:02} % of the core{}\n", r.pos.load(Relaxed), r.cap.load(Relaxed), share / 100, share % 100, if r.armed.load(Relaxed) != 0 { ", armed" } else { "" }).as_bytes());
    }
}

fn prof_schema() -> vsf::schema::SectionSchema {
    use vsf::schema::TypeConstraint::AnyUnsigned;
    vsf::schema::SectionSchema::new("prof").field("hz", AnyUnsigned).field("cntfrq", AnyUnsigned).field("period", AnyUnsigned).field("started", AnyUnsigned).field("stopped", AnyUnsigned).field("run", AnyUnsigned).field("depth", AnyUnsigned).field("head", AnyUnsigned)
}
fn core_schema() -> vsf::schema::SectionSchema {
    use vsf::schema::TypeConstraint::AnyUnsigned;
    vsf::schema::SectionSchema::new("core").field("core", AnyUnsigned).field("ring", AnyUnsigned).field("words", AnyUnsigned).field("cap", AnyUnsigned).field("ticks", AnyUnsigned).field("samples", AnyUnsigned).field("held", AnyUnsigned).field("dropped", AnyUnsigned).field("cost", AnyUnsigned)
}
fn thread_schema() -> vsf::schema::SectionSchema {
    use vsf::schema::TypeConstraint::{AnyUnsigned, AsciiText};
    vsf::schema::SectionSchema::new("thread").field("thread", AnyUnsigned).field("epoch", AnyUnsigned).field("base", AnyUnsigned).field("process", AsciiText).field("name", AsciiText)
}

fn u(v: u64) -> vsf::VsfType {
    vsf::VsfType::u(v as usize, false)
}
fn ascii(b: &[u8]) -> vsf::VsfType {
    vsf::VsfType::a(vsf::Ascii::checked("name", b.iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '_' }).collect::<alloc::string::String>()))
}

/// Build the index document and point the anchor at it.
fn pull(out: &mut alloc::vec::Vec<u8>) {
    if ON.load(Relaxed) {
        out.extend_from_slice(b"prof: still running (prof stop first)\n");
        return;
    }
    let seen: alloc::vec::Vec<(usize, u64, usize, alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)> = crate::process::with_lock(|| unsafe { (*(&raw const SEEN)).iter().map(|s| (s.thread, s.epoch, s.base, s.process.clone(), s.name.clone())).collect() });
    let doc = (|| -> Option<alloc::vec::Vec<u8>> {
        let head = prof_schema().build().set("hz", u(HZ.load(Relaxed))).ok()?.set("cntfrq", u(crate::platform::cntfrq())).ok()?.set("period", u(PERIOD.load(Relaxed))).ok()?.set("started", u(STARTED.load(Relaxed))).ok()?.set("stopped", u(STOPPED.load(Relaxed))).ok()?.set("run", u(RUN.load(Relaxed))).ok()?.set("depth", u(DEPTH as u64)).ok()?.set("head", u(HEAD as u64)).ok()?.encode().ok()?;
        let mut b = vsf::VsfBuilder::new().creation_time_oscillations(crate::settings::eagle_now()).provenance_only().add_unboxed("prof", head);
        for (c, r) in RINGS.iter().enumerate() {
            if r.buf.load(Relaxed) == 0 {
                continue;
            }
            let s = core_schema().build().set("core", u(c as u64)).ok()?.set("ring", u(r.buf.load(Relaxed))).ok()?.set("words", u(r.pos.load(Relaxed))).ok()?.set("cap", u(r.cap.load(Relaxed))).ok()?.set("ticks", u(r.ticks.load(Relaxed))).ok()?.set("samples", u(r.samples.load(Relaxed))).ok()?.set("held", u(r.held.load(Relaxed))).ok()?.set("dropped", u(r.dropped.load(Relaxed))).ok()?.set("cost", u(r.cost.load(Relaxed))).ok()?.encode().ok()?;
            b = b.add_unboxed("core", s);
        }
        for (t, epoch, base, process, name) in &seen {
            let s = thread_schema().build().set("thread", u(*t as u64)).ok()?.set("epoch", u(*epoch)).ok()?.set("base", u(*base as u64)).ok()?.set("process", ascii(process)).ok()?.set("name", ascii(name)).ok()?.encode().ok()?;
            b = b.add_unboxed("thread", s);
        }
        b.build().ok()
    })();
    let Some(doc) = doc else {
        out.extend_from_slice(b"prof: the index document did not build\n");
        return;
    };
    let index = unsafe { &mut *(&raw mut INDEX) };
    *index = Some(doc);
    let d = index.as_ref().map(|d| (d.as_ptr() as u64, d.len() as u64)).unwrap_or((0, 0));
    FERROS_PROF_ANCHOR[1].store(d.0, Relaxed);
    FERROS_PROF_ANCHOR[2].store(d.1, Relaxed);
    FERROS_PROF_ANCHOR[3].store(RUN.load(Relaxed), Relaxed);
    FERROS_PROF_ANCHOR[0].store(MAGIC, Relaxed);
    out.extend_from_slice(alloc::format!("prof: index A#{} bytes, A#{} threads (tools/flame reads it)\n", d.1, seen.len()).as_bytes());
}

fn free(out: &mut alloc::vec::Vec<u8>) {
    if ON.load(Relaxed) {
        out.extend_from_slice(b"prof: still running (prof stop first)\n");
        return;
    }
    if !settled() {
        out.extend_from_slice(b"prof: the run stopped a moment ago; again\n");
        return;
    }
    let store = unsafe { &mut *(&raw mut STORE) };
    let mut bytes = 0;
    for (c, s) in store.iter_mut().enumerate() {
        RINGS[c].buf.store(0, Relaxed);
        RINGS[c].cap.store(0, Relaxed);
        bytes += s.take().map(|v| v.len() << 3).unwrap_or(0);
    }
    FERROS_PROF_ANCHOR[0].store(0, Relaxed);
    unsafe { *(&raw mut INDEX) = None };
    crate::process::with_lock(|| unsafe { *(&raw mut SEEN) = alloc::vec::Vec::new() });
    out.extend_from_slice(alloc::format!("prof: A#{} KiB back to the heap\n", bytes >> 10).as_bytes());
}
