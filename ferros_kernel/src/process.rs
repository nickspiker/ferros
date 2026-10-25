//! Processes: code at EL0 in an arc, talking to the world only through capabilities (KERNEL.md: Execution Model, Modules 2-4; ORACLE.md spawn).
//!
//! A process is one arc — a power-of-two, aligned span of the arc pool, zeroed when allocated — and a capability space. The kernel enters it with `eret` to EL0 holding the arc in two registers the code may not write (x28 = base, x27 = mask); compiled code reaches memory only as `base + (offset & mask)`. EL0 itself takes away the system controls: a process can't mask the killswitch, issue SMCs or touch MMIO, DMA or kernel memory (the static map gives EL0 the pool only).
//!
//! The syscall boundary is `svc #0` with the call number in x8 (all offsets are arc offsets, masked by the kernel; ERR = all ones):
//! - 0 `exit(code)` — the process ends; its arc and capabilities cease to exist.
//! - 1 `call(cap, msg, msg_len, reply, reply_cap)` — present a 32-byte token (at arc offset `cap`) holding the send right and send `msg`; the caller blocks until the reply is written at `reply` (at most `reply_cap` bytes); x0 = its length.
//! - 2 `recv(cap, buf, buf_cap, reply_token, flags)` — wait on an endpoint the token holds the receive right for; x0 = the message length, and a single-use reply token is written at `reply_token` (32 bytes). Flags bit 0 = poll: if nothing is queued, x0 = NOTHING (all ones but one) at once — for a server that also polls hardware.
//! - 3 `reply(reply_token, msg, msg_len)` — answer a call; the reply token ceases to exist (it cannot be used twice), the caller becomes runnable, the server carries on.
//! - 4 `mmio_read(grant, offset)` / 5 `mmio_write(grant, offset, value)` — a hardware grant (an arc of device registers, ASID.md) checked by the kernel on every access: 4-byte aligned, inside the grant, else ERR. Blocks only the EL3 monitor can reach (the PMU) go through it. Drivers are processes holding these; when toka-compiled drivers exist, masked direct access replaces the syscall. Husky hazard: reading a block whose power domain is off makes the next EL3 call hang (BOOT.md EL3 poison rule), so a driver checks power state first.
//! - 6 `entropy(cap, buf, len)` — present a token holding the entropy right; `len` bytes (at most 4096) of fresh randomness are written at `buf`, x0 = `len`. The stream is BLAKE3 keyed from this session's wairua (KOHI) over a kernel counter, so no two calls in a power session see the same bytes and nothing from a previous session repeats. ERR without a wairua: a driver that needs randomness (the WiFi firmware's RNG seed) gets none rather than a predictable one.
//! - 8 `input(off, buf, len)` — copy `len` bytes (≤ 4096) of the spawn input at `off` into the arc at `buf`; x0 = bytes copied (0 past the end). Large one-shot input (a driver's firmware) streams through a small arc buffer this way instead of bloating the arc; small input is also inlined at `x1` on entry.
//! - 7 `dma(cap)` — present a token holding a DMA grant; x0 = the physical base of that DMA region (its size is in the registry, `DMA`). DMA regions are 2 MiB blocks of non-cacheable memory the process reads and writes directly (its device's rings and buffers); the device's DMA window covers exactly that block (BOOT.md/WIFI.md: S2MPU fence).
//! There is no "print", no PID and no ambient authority: what a process may reach is exactly the tokens it was granted at spawn (written at arc offset 0: u32 count, then the 32-byte tokens). An object you hold no token for does not exist for you — an unknown token gets the same answer as a wrong one.
//!
//! IPC is synchronous rendezvous (Module 4): a call to an endpoint whose server waits in recv switches straight to the server with the message copied arc to arc (no shared memory); otherwise the call queues and the caller blocks. Switching processes is swapping the saved register frame the trap path returns through.
//!
//! Time slices: while a process runs, the EL2 timer (PPI 26) is armed for 10 ms. When it fires at EL0 the process is parked runnable and the kernel gets the CPU back, so a process that never blocks can't starve the USB service loop (which resumes runnable processes one slice at a time, `run_slice`) or stop the watchdog pets.
//!
//! Tokens are BLAKE3 keyed with a per-session kernel key (derived from the wairua) over domain, object, rights, nonce and the process's epoch; every spawn gets a new epoch, so a dead process's tokens match nothing.
//!
//! Any other exception from EL0 (a fault, an undefined instruction) ends the process, not the kernel.

use alloc::vec::Vec;
use core::cell::UnsafeCell;

// ---------------------------------------------------------------- arcs

/// Smallest arc: 64 KiB. The pool (cachemap::POOL) is a power of two.
const ARC_MIN_LOG2: u32 = 16;

struct Arcs {
    /// Free blocks per size class (log2 size → block bases).
    free: [Vec<usize>; 32],
    init: bool,
}

struct Global<T>(UnsafeCell<T>);
// Every access is under `LOCK` (see `lock`); the IRQ handler touches none of it.
unsafe impl<T> Sync for Global<T> {}

// ---------------------------------------------------------------- the kernel lock and the cores
//
// One lock over the kernel's shared state (KERNEL.md "Cores"): taken for the microseconds a trap, a slice hand-out or a service-loop call holds it, never across a wait or a run at EL0. Every `pub fn` here takes it; everything private assumes it is held. `LOCK_SPINS` counts contended acquisitions for DIAG.
static LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub static LOCK_SPINS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The core holding the kernel lock, plus one (0: free): lets code that runs both inside syscalls and from the service loop take the lock only when it does not already hold it (`with_lock_held_or`).
static HOLDER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn lock() {
    use core::sync::atomic::Ordering;
    if LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        LOCK_SPINS.fetch_add(1, Ordering::Relaxed);
        while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    HOLDER.store(core() + 1, Ordering::Relaxed);
    // Who holds it, for the post-mortem (a hang with the lock held names its core).
    crate::platform::note(0, core() as u64 + 1);
}
fn unlock() {
    HOLDER.store(0, core::sync::atomic::Ordering::Relaxed);
    crate::platform::note(0, 0);
    LOCK.store(false, core::sync::atomic::Ordering::Release);
}
/// This core's index: MPIDR Aff1 (husky: 0-3 the A510s, 4-7 the A715s, 8 the X3).
pub fn core() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
    ((mpidr >> 8) & 0xF) as usize
}
/// The thread each core is running at EL0, if any.
static mut CURRENT: [Option<usize>; 16] = [None; 16];
fn cur_get() -> Option<usize> {
    unsafe { (*(&raw const CURRENT))[core()] }
}
fn cur_set(t: Option<usize>) {
    unsafe { (*(&raw mut CURRENT))[core()] = t };
}
/// Whether thread `t` is at EL0 on some core right now — the test for "running elsewhere" when a process ends: a thread's `Running` state alone is not it (a stale `Running` with no core kept a dead display process's thread forever and its arc with it, 2026-10-23).
fn on_a_core(t: usize) -> bool {
    unsafe { (*(&raw const CURRENT)).iter().any(|c| *c == Some(t)) }
}
/// Frames found with IRQs masked in their saved PSTATE when about to be resumed (fixed on the spot), and slice ticks re-armed after a tick was taken at EL2 during a syscall: DIAG SPSR_MASKED, TICK_REARMS (the 2026-10-23 freeze: a worker "running" a thread at EL0 that no tick ever reached).
pub static SPSR_MASKED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static TICK_REARMS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// A frame about to be resumed at EL0 must have IRQs (and FIQs) open, or the slice timer and the killswitch never reach that core again.
fn frame_check(f: &mut Frame) {
    // Interrupts open, and EL0t (M = 0): a frame resumed at any other EL runs the thread's code in the kernel's EL, where its syscalls are skipped by the same-EL handler and it spins forever untimed.
    if f.spsr & 0xCF != 0 {
        f.spsr &= !0xCF;
        SPSR_MASKED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}
/// Slices run per core, for DIAG.
pub static SLICES: [core::sync::atomic::AtomicU64; 16] = [const { core::sync::atomic::AtomicU64::new(0) }; 16];
/// How many threads the run queue holds, readable without the lock: an idle core looks here before it takes the lock (five cores taking the lock on every `sev` contended it 100,000 times in a few seconds, 2026-10-23).
pub static RUNNABLE_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
fn runnable_sync() {
    RUNNABLE_N.store(k().runnable.len(), core::sync::atomic::Ordering::Release);
}
/// Wake every other core: `sev` for one in `wfe` (the kernel's core, on the event stream) and SGI 1 to all others for the workers in `wfi` (smp.rs idle loop). ICC_SGI1R_EL1 with IRM (bit 40) = every core but this one.
pub fn kick(mask: u16) {
    unsafe {
        core::arch::asm!("sev");
        // Only when a worker may run it: the render thread, pinned to the X3, sat in the queue whenever the X3 was busy, and every kick for it woke four cores to find nothing (2026-10-23).
        if mask & MASK_WORKERS != 0 || mask & cores_up() == 0 {
            core::arch::asm!("msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) (1u64 << 40) | (1u64 << 24));
        }
    }
}
fn mask_of(t: usize) -> u16 {
    k().threads.get(t).and_then(|x| x.as_ref()).map(|x| x.mask).unwrap_or(0xFFFF)
}
/// A thread onto the run queue, and the idle cores that may run it told.
fn enqueue_back(t: usize) {
    k().runnable.push_back(t);
    runnable_sync();
    kick(mask_of(t));
}
fn enqueue_front(t: usize) {
    k().runnable.push_front(t);
    runnable_sync();
    kick(mask_of(t));
}

static ARCS: Global<Arcs> = Global(UnsafeCell::new(Arcs { free: [const { Vec::new() }; 32], init: false }));

fn arcs() -> &'static mut Arcs {
    let a = unsafe { &mut *ARCS.0.get() };
    if !a.init {
        let (p0, p1) = crate::cachemap::POOL;
        let size = p1 - p0;
        a.free[size.trailing_zeros() as usize].push(p0);
        a.init = true;
    }
    a
}

/// An arc: `size` usable bytes at `base` (a power of two, base aligned to `size`), carved from a `block`-byte buddy block (= `size`, or `2 * size` for a guarded arc, whose upper half is unused guard so an in-arc immediate offset can't reach another arc — SANDBOX.md).
#[derive(Clone, Copy)]
pub struct Arc {
    pub base: usize,
    pub size: usize,
    pub block: usize,
}

impl Arc {
    pub fn mask(&self) -> usize {
        self.size - 1
    }
}

/// Allocate the smallest arc of at least `bytes` (buddy split from the pool), zeroed. `guard` reserves a guard of the arc's own size on each side (a 2S block, SANDBOX.md): the base is 2S-aligned, hence S-aligned, so masking stays a single `and`, and the usable arc never abuts another.
pub fn arc_alloc(bytes: usize, guard: bool) -> Option<Arc> {
    let want = bytes.max(1 << ARC_MIN_LOG2).next_power_of_two();
    let block = if guard { want * 2 } else { want };
    let k = block.trailing_zeros() as usize;
    let a = arcs();
    let mut j = k;
    while j < 32 && a.free[j].is_empty() {
        j += 1;
    }
    if j >= 32 {
        return None;
    }
    let base = a.free[j].pop()?;
    while j > k {
        j -= 1;
        a.free[j].push(base + (1 << j));
    }
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, want) };
    Some(Arc { base, size: want, block })
}

/// Return an arc to the pool, merging buddies.
pub fn arc_free(arc: Arc) {
    crate::cachemap::arc_reset(arc.base, arc.block);
    let a = arcs();
    let (p0, p1) = crate::cachemap::POOL;
    let mut base = arc.base;
    let mut k = arc.block.trailing_zeros() as usize;
    while (1usize << k) < p1 - p0 {
        let buddy = p0 + ((base - p0) ^ (1 << k));
        match a.free[k].iter().position(|&b| b == buddy) {
            Some(i) => {
                a.free[k].swap_remove(i);
                base = base.min(buddy);
                k += 1;
            }
            None => break,
        }
    }
    a.free[k].push(base);
}

// ---------------------------------------------------------------- capabilities

/// Kernel objects a capability can name.
#[derive(Clone, Copy, PartialEq)]
pub enum Object {
    /// Served by the kernel: what a process sends is returned to whoever spawned it.
    Console,
    /// An IPC endpoint (index into the endpoint table).
    Endpoint(usize),
    /// A single-use reply to the process that called (index into the process table).
    Reply(usize),
    /// A hardware grant: an arc of device registers (index into the grant table). ASID.md: "process gets arcs, hardware gets a grant".
    Hardware(usize),
    /// Served by the kernel: randomness from the session's wairua.
    Entropy,
    /// A DMA region (index into `DMA`).
    Dma(usize),
    /// Served by the kernel: the right to change the device's preferences (`SYS_SET_PREF`, SETTINGS.md). Reading them needs no capability: every process finds them in its own grants page.
    Settings,
    /// Served by the kernel: a scope of the device vault (index into `STORES`): get and set the keys under its prefix, named relative to it (`SYS_STORE`).
    Store(usize),
    /// An interrupt line (index into `IRQS`): `irq_wait` blocks a thread until it fires, `irq_ack` unmasks it again (KERNEL.md "Interrupts to user space").
    Irq(usize),
}

pub const RIGHT_SEND: u32 = 1;
pub const RIGHT_RECV: u32 = 2;

struct CapEntry {
    token: [u8; 32],
    object: Object,
    rights: u32,
}

const CAP_DOMAIN: &[u8] = b"ferros.cap.v1";
const CAP_KEY_CONTEXT: &str = "ferros.cap-key.v1";

static EPOCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static NONCE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The per-session key every token is MACed under (derived from the wairua, so spawning is the first thing that needs KOHI). None = no wairua: refuse to spawn.
fn cap_key() -> Option<[u8; 32]> {
    crate::wairua::draw().map(|w| blake3::derive_key(CAP_KEY_CONTEXT, &w))
}

fn mint(object: Object, rights: u32, epoch: u64) -> [u8; 32] {
    let key = cap_key().unwrap_or([0u8; 32]);
    let (tag, id) = match object {
        Object::Console => (0u8, 0u64),
        Object::Endpoint(i) => (1, i as u64),
        Object::Reply(i) => (2, i as u64),
        Object::Entropy => (4, 0),
        Object::Dma(i) => (5, i as u64),
        Object::Hardware(i) => (3, i as u64),
        Object::Settings => (6, 0),
        Object::Store(i) => (7, i as u64),
        Object::Irq(i) => (8, i as u64),
    };
    let nonce = NONCE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let mut h = blake3::Hasher::new_keyed(&key);
    h.update(CAP_DOMAIN);
    h.update(&[tag]);
    h.update(&id.to_le_bytes());
    h.update(&rights.to_le_bytes());
    h.update(&nonce.to_le_bytes());
    h.update(&epoch.to_le_bytes());
    *h.finalize().as_bytes()
}

// ---------------------------------------------------------------- processes and endpoints

/// Saved process registers (built by `.Lel0_sync` on exception entry; the same layout is kept per blocked process).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Frame {
    pub x: [u64; 31],
    pub sp_el0: u64,
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
    pub far: u64,
    /// v0–v31 and the FP control/status registers: saved on every EL0 entry and restored on every eret since 2026-10-17 — before, a process's floating point lived only until its next syscall or slice tick (the kernel's own memcpy uses the vector registers), which zeroed every f32 the display server carried across one.
    pub v: [u128; 32],
    pub fpcr: u64,
    pub fpsr: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Running,
    Runnable,
    /// In recv: (endpoint, buf, buf_cap, reply-token offset).
    Receiving(usize, u64, u64, u64),
    /// Called and waiting for the reply: (reply offset, reply cap).
    Calling(u64, u64),
    /// Asleep until the generic timer count reaches the deadline (SYS_SLEEP); `wake_sleepers` makes it runnable again.
    Sleeping(u64),
    /// In `wait` on a word of its arc (the word's address, and a deadline or 0): `wake` on that word by another thread of the process makes it runnable (x0 = 0), or the deadline passing does (x0 = 2).
    Waiting(usize, u64),
}

/// A thread: what the scheduler runs. A saved frame, a state, and the process it belongs to; threads of one process share its arc and every capability in it (KERNEL.md "Threads, interrupts and cores"). Threads are not addressable: nothing outside the kernel names one.
struct Thread {
    proc: usize,
    frame: Frame,
    state: State,
    /// Which cores may run it (bit = core index): the workers by default; a thread pins itself with `SYS_PIN` (the display's render thread to the X3). If none of its cores is up, any core takes it.
    mask: u16,
}
/// The A715s (cores 4-7): where processes run unless they ask otherwise.
pub const MASK_WORKERS: u16 = 0x00F0;
/// The X3 (core 8): the kernel's core, for the one real compute path (the display's render thread) and otherwise idle between service-loop passes.
pub const MASK_BIG: u16 = 0x0100;
/// The cores that are up: the kernel's, and the workers smp.rs reports.
fn cores_up() -> u16 {
    MASK_BIG | ((crate::smp::UP.load(core::sync::atomic::Ordering::Relaxed) as u16) << 4)
}
/// Whether thread `t` may run on this core.
fn allowed_here(mask: u16) -> bool {
    let here = 1u16 << core();
    mask & here != 0 || mask & cores_up() == 0
}

struct Process {
    arc: Arc,
    cspace: Vec<CapEntry>,
    epoch: u64,
    /// Ended (exit, a fault, a kill) while a thread of it still ran on another core: that thread leaves at its next trap, and the last one out frees the arc.
    dying: bool,
    name: Vec<u8>,
    /// The process's console output is at the start of a line (so the next byte gets the name tag).
    line_start: bool,
    /// The spawn input, kept in kernel memory and pulled in by the `input` syscall (so a driver's arc stays small while large one-shot data — firmware — streams through a chunk buffer). Inlined into the arc too only when it is small.
    input: Vec<u8>,
}

struct Endpoint {
    name: Vec<u8>,
    /// Calls waiting for the server: (calling thread, message).
    queue: alloc::collections::VecDeque<(usize, Vec<u8>)>,
    /// The thread that last polled this endpoint (recv with poll): a call wakes it if it is asleep, so a server that sleeps between polls answers at once rather than at its next deadline.
    poller: Option<usize>,
}

struct Kernel {
    procs: Vec<Option<Process>>,
    /// The threads; every index in `runnable`, `current`, an endpoint queue, a poller or a reply capability is one of these.
    threads: Vec<Option<Thread>>,
    endpoints: Vec<Endpoint>,
    runnable: alloc::collections::VecDeque<usize>,
    /// Everything any process sent its console during the current kernel entry, each line tagged with the sender's name.
    console: Vec<u8>,
    /// Threads that ended during the current entry: (thread, exit value).
    exited: Vec<(usize, u64)>,
    ipc_round_trips: u32,
    trace_at: usize,
}

/// The scheduler trace (`diag sched`): the last TRACE scheduling events, (count, event, process, detail), written at `trace_at` round the ring. Static, not on the kernel heap.
static TRACE_RING: Global<[(u64, u8, u8, u16, u8); TRACE]> = Global(UnsafeCell::new([(0, 0, 0, 0, 0); TRACE]));

const TRACE: usize = 4096;
const TR_CALL: u8 = 1;
const TR_DELIVER: u8 = 2;
const TR_WAKE_POLL: u8 = 3;
const TR_REPLY: u8 = 4;
const TR_SLEEP: u8 = 5;
const TR_WAKE: u8 = 6;
const TR_SWITCH: u8 = 7;
const TR_RUN: u8 = 8;
const TR_PREEMPT: u8 = 9;
const TR_PARK: u8 = 10;
const TR_POLL_MISS: u8 = 11;
const TR_RECV_BLOCK: u8 = 12;
const TR_YIELD: u8 = 13;
const TR_NAMES: [&str; 14] = ["-", "call", "deliver", "wake-poll", "reply", "sleep", "wake", "switch", "run", "preempt", "park", "poll-miss", "recv-block", "yield"];

/// One scheduling event into the ring.
fn tr(ev: u8, who: usize, detail: u64) {
    let kk = k();
    let now: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) now) };
    unsafe { (*TRACE_RING.0.get())[kk.trace_at] = (now, ev, who as u8, detail as u16, core() as u8) };
    kk.trace_at = (kk.trace_at + 1) % TRACE;
}

/// Every thread with its process and state (`diag threads`).
pub fn threads_report(out: &mut Vec<u8>) {
    lock();
    let kk = k();
    let now: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) now) };
    let f = crate::platform::cntfrq();
    for (i, t) in kk.threads.iter().enumerate() {
        let Some(t) = t else { continue };
        let st = match t.state {
            State::Running => alloc::format!("running"),
            State::Runnable => alloc::format!("runnable"),
            State::Receiving(ep, ..) => alloc::format!("recv on endpoint A#{ep}"),
            State::Calling(..) => alloc::format!("calling"),
            State::Sleeping(d) => alloc::format!("sleeping, A#{} ms left", d.saturating_sub(now) * 1000 / f),
            State::Waiting(a, d) if a < 1024 => alloc::format!("irq_wait on INTID A#{a}{}, pending {}", if d != 0 { alloc::format!(", A#{} ms left", d.saturating_sub(now) * 1000 / f) } else { alloc::string::String::new() }, unsafe { core::ptr::read_volatile(&raw const __irq_pending[a]) }),
            State::Waiting(a, d) => alloc::format!("wait on word G#{a:X}{}", if d != 0 { alloc::format!(", A#{} ms left", d.saturating_sub(now) * 1000 / f) } else { alloc::string::String::new() }),
        };
        out.extend_from_slice(alloc::format!("thread A#{i} {} (process A#{}, cores G#{:X}, spsr G#{:X}, pc +G#{:X}): {st}\n", alloc::string::String::from_utf8_lossy(&thread_name(i)), t.proc, t.mask, t.frame.spsr, t.frame.elr.wrapping_sub(kk.procs.get(t.proc).and_then(|p| p.as_ref()).map(|p| p.arc.base as u64).unwrap_or(0)) & 0xFF_FFFF).as_bytes());
    }
    out.extend_from_slice(alloc::format!("runnable queue: {:?}; running per core {:?}; slices per core {:?}; lock contended A#{}\n", kk.runnable, unsafe { *(&raw const CURRENT) }, SLICES.iter().map(|s| s.load(core::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>(), LOCK_SPINS.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
    unlock();
}

/// The trace, oldest first, as text: time since the first entry in microseconds, the event, the process name and the detail. Then per call: the wait before the server took it, the server's time to the reply, and the caller's wait after the reply before it ran again — the three parts of a round trip.
pub fn sched_report(out: &mut Vec<u8>) {
    lock();
    sched_report_locked(out);
    unlock();
}
fn sched_report_locked(out: &mut Vec<u8>) {
    let kk = k();
    let f = crate::platform::cntfrq();
    let ring = unsafe { &*TRACE_RING.0.get() };
    let n = TRACE;
    let name = |i: u8| -> alloc::string::String { alloc::string::String::from_utf8_lossy(&thread_name(i as usize)).into_owned() };
    let entries: Vec<(u64, u8, u8, u16, u8)> = (0..n).map(|k| ring[(kk.trace_at + k) % n]).filter(|e| e.0 != 0).collect();
    let Some(&(t0, ..)) = entries.first() else { return };
    let us = |t: u64| t.wrapping_sub(t0) * 1_000_000 / f;
    // The round trips: a call by X, the deliver to the server, the reply to X, X's next run or switch.
    let mut trips: Vec<(u8, u8, u64, u64, u64)> = Vec::new();
    for (k, e) in entries.iter().enumerate() {
        if e.1 != TR_CALL {
            continue;
        }
        let mut deliver = None;
        let mut reply = None;
        let mut back = None;
        for e2 in &entries[k + 1..] {
            if deliver.is_none() && e2.1 == TR_DELIVER && e2.3 == e.3 {
                deliver = Some(e2);
            } else if deliver.is_some() && reply.is_none() && e2.1 == TR_REPLY && e2.2 == e.2 {
                reply = Some(e2.0);
            } else if reply.is_some() && (e2.1 == TR_RUN || e2.1 == TR_SWITCH) && e2.2 == e.2 {
                back = Some(e2.0);
                break;
            }
        }
        if let (Some(d), Some(r), Some(b)) = (deliver, reply, back) {
            trips.push((e.2, d.2, d.0 - e.0, r - d.0, b - r));
        }
    }
    out.extend_from_slice(alloc::format!("sched trace: A#{} events over A#{} ms; A#{} round trips\n", entries.len(), us(entries.last().unwrap().0) / 1000, trips.len()).as_bytes());
    // The slow trips in full, the rest summed per (caller, server).
    let mut slow = 0;
    for &(c, sv, w, h, b) in &trips {
        if (w + h + b) * 1_000_000 / f > 2_000 && slow < 64 {
            slow += 1;
            out.extend_from_slice(alloc::format!("  slow: {} -> {}: waited A#{} us for the server, server A#{} us, waited A#{} us after the reply\n", name(c), name(sv), w * 1_000_000 / f, h * 1_000_000 / f, b * 1_000_000 / f).as_bytes());
        }
    }
    let mut pairs: Vec<(u8, u8, u32, u64, u64, u64)> = Vec::new();
    for &(c, sv, w, h, b) in &trips {
        match pairs.iter_mut().find(|p| p.0 == c && p.1 == sv) {
            Some(p) => {
                p.2 += 1;
                p.3 += w;
                p.4 += h;
                p.5 += b;
            }
            None => pairs.push((c, sv, 1, w, h, b)),
        }
    }
    for &(c, sv, k, w, h, b) in &pairs {
        out.extend_from_slice(alloc::format!("  {} -> {}: A#{} trips, mean wait A#{} us, server A#{} us, after A#{} us\n", name(c), name(sv), k, w * 1_000_000 / f / k as u64, h * 1_000_000 / f / k as u64, b * 1_000_000 / f / k as u64).as_bytes());
    }
    // The last 512 events, so a stall just now is in view.
    let start = entries.len().saturating_sub(512);
    for e in &entries[start..] {
        out.extend_from_slice(alloc::format!("{:>9} c{} {:<10} {:<8} {}\n", us(e.0), e.4, TR_NAMES[e.1 as usize], name(e.2), e.3).as_bytes());
    }
}

static KERNEL: Global<Kernel> = Global(UnsafeCell::new(Kernel {
    procs: Vec::new(),
    threads: Vec::new(),
    endpoints: Vec::new(),
    runnable: alloc::collections::VecDeque::new(),
    console: Vec::new(),
    exited: Vec::new(),
    ipc_round_trips: 0,
    trace_at: 0,
}));

fn k() -> &'static mut Kernel {
    unsafe { &mut *KERNEL.0.get() }
}

/// The thread at `t`.
fn th(t: usize) -> Option<&'static mut Thread> {
    k().threads.get_mut(t).and_then(|x| x.as_mut())
}
/// The process thread `t` belongs to.
fn proc_of(t: usize) -> Option<&'static mut Process> {
    let pi = k().threads.get(t).and_then(|x| x.as_ref()).map(|x| x.proc)?;
    k().procs.get_mut(pi).and_then(|x| x.as_mut())
}
/// A thread's name for the console and the trace: its process's name, with `.N` for every thread but the process's first.
fn thread_name(t: usize) -> Vec<u8> {
    let kk = k();
    let Some(Some(x)) = kk.threads.get(t) else { return alloc::format!("t{t}").into_bytes() };
    let mut n = kk.procs.get(x.proc).and_then(|p| p.as_ref()).map(|p| p.name.clone()).unwrap_or_default();
    let first = kk.threads.iter().position(|y| matches!(y, Some(y) if y.proc == x.proc));
    if first != Some(t) {
        let k = kk.threads[..t].iter().filter(|y| matches!(y, Some(y) if y.proc == x.proc)).count();
        n.extend_from_slice(alloc::format!(".{k}").as_bytes());
    }
    n
}

impl Process {
    fn read(&self, off: u64, len: usize) -> Vec<u8> {
        let m = self.arc.mask();
        (0..len).map(|i| unsafe { core::ptr::read_volatile((self.arc.base + ((off as usize + i) & m)) as *const u8) }).collect()
    }
    fn write(&self, off: u64, bytes: &[u8]) {
        let m = self.arc.mask();
        for (i, &b) in bytes.iter().enumerate() {
            unsafe { core::ptr::write_volatile((self.arc.base + ((off as usize + i) & m)) as *mut u8, b) };
        }
    }
    /// Find the capability for a presented token: compare against every entry without an early exit, so the time taken says nothing about which (if any) matched. Returns the entry index.
    fn lookup(&self, token: &[u8]) -> Option<usize> {
        let mut hit = None;
        for (i, e) in self.cspace.iter().enumerate() {
            let mut diff = 0u8;
            for (a, b) in e.token.iter().zip(token.iter()) {
                diff |= a ^ b;
            }
            if diff == 0 && token.len() == 32 {
                hit = Some(i);
            }
        }
        hit
    }
}

/// The endpoint registry for dev spawns: named endpoints created on first mention (`bridge spawn --serve NAME` / `--use NAME`). The spawner's authority stands in for an endpoint-creating capability until there is one.
fn endpoint(name: &[u8]) -> usize {
    let kk = k();
    if let Some(i) = kk.endpoints.iter().position(|e| e.name == name) {
        return i;
    }
    kk.endpoints.push(Endpoint { name: name.to_vec(), queue: alloc::collections::VecDeque::new(), poller: None });
    kk.endpoints.len() - 1
}

const SYS_EXIT: u64 = 0;
const SYS_CALL: u64 = 1;
const SYS_RECV: u64 = 2;
const SYS_REPLY: u64 = 3;
/// mmio_read(grant, offset) → x0 = the 32-bit register; mmio_write(grant, offset, value).
const SYS_MMIO_READ: u64 = 4;
const SYS_MMIO_WRITE: u64 = 5;
/// entropy(cap, buf, len) → x0 = len.
const SYS_ENTROPY: u64 = 6;
/// dma(cap) → x0 = the region's physical base.
const SYS_DMA: u64 = 7;
/// input(off, buf, len) → bytes of the spawn input copied into the arc at `buf`.
const SYS_INPUT: u64 = 8;
/// 9 `sleep(deadline)`: park until CNTPCT reaches `deadline`, so a periodic process (the display server's frame clock) costs nothing between frames instead of spinning through every slice the service loop hands out.
const SYS_SLEEP: u64 = 9;
/// Start a signed capsule from the vault as a new process, handing it copies of some of the caller's own capabilities (never more rights than the caller holds; reply capabilities do not pass). x0/x1: request offset and length in the caller's arc, [name len][name][vault key len][vault key][token count][tokens, 32 bytes each]. The child is queued, not entered: the caller carries on. Returns 0, or ERR with the reason on the console.
const SYS_SPAWN: u64 = 10;
/// Change a device preference: x0 the Settings capability's token offset, x1 which preference, x2 its new value. The kernel checks the value, keeps it in the vault through `PREF_STORE`, and rewrites it in every live process's preferences block at once. Returns 0 or ERR.
const SYS_SET_PREF: u64 = 11;
/// The device's plain facts for its dashboard (SHELL.md: the device circle's front): x0/x1 a buffer in the caller's arc and its length; up to FACT_COUNT u64 words, little-endian, `FACT_UNKNOWN` where a fact is not known. No capability: these are what the device shows whoever holds it, and none is a key, a handle or a serial. Returns the bytes written.
const SYS_FACTS: u64 = 12;
pub const FACT_COUNT: usize = 32;
pub const FACT_UNKNOWN: u64 = u64::MAX;
/// A scope of the device vault: x0 the Store capability's token offset, x1 the operation (bit 0: 0 get, 1 set) with the buffer's length in bits 8 and up, x2/x3 the key's offset and length (relative to the scope's prefix, at most 64 bytes of printable ASCII), x4 the buffer. A get copies at most the buffer's length and returns the value's whole length, NOTHING if the key is absent; a set stores the buffer (at most STORE_MAX) and returns 0. ERR otherwise.
const SYS_STORE: u64 = 13;
/// thread_spawn(entry, stack_top, arg): a new thread of this process at arc offset `entry` with SP_EL0 at arc offset `stack_top` and x0 = `arg`, runnable at once (the caller carries on). Returns 0 or ERR.
const SYS_THREAD_SPAWN: u64 = 14;
/// thread_exit(value): this thread ends; the last thread of a process ends the process as `exit` does.
const SYS_THREAD_EXIT: u64 = 15;
/// wait(word, expected, deadline): if the 8-byte word at arc offset `word` still holds `expected`, block until another thread of this process wakes it (returns 0) or CNTPCT reaches `deadline` (returns 2; 0 = no deadline); returns 1 at once if the word had already changed. A futex with a timeout: threads hand work to each other without IPC, and a server's loop waits for a request or its next deadline in one call.
const SYS_WAIT: u64 = 16;
/// wake(word, count): make up to `count` threads of this process waiting on that word runnable; returns how many.
const SYS_WAKE: u64 = 17;
/// irq_wait(cap, deadline): block until the line fires (0), or the deadline passes (2; 0 = none); if it fired while no one waited, return 0 at once. The line is masked at the GIC from the moment it fires until `irq_ack`.
const SYS_IRQ_WAIT: u64 = 18;
/// irq_ack(cap): the driver has handled its device (and cleared its own pending bit there): the line is unmasked at the GIC.
const SYS_IRQ_ACK: u64 = 19;
/// pin(mask): the cores this thread may run on from now (bit = core index); 0 = the workers. Returns 0.
const SYS_PIN: u64 = 20;
/// The light scope's samples (usf.rs): x0/x1 a buffer and its length; a 16-byte header {u64 the stream's next sample number, u32 rate Hz, u32 the channel mask} then the latest samples (i32, oldest first) as many as fit, up to a second. No capability: it is the light falling on the back of the phone. Returns the bytes written.
const SYS_LIGHT: u64 = 21;
const SYS_LAST: u64 = 21;

/// A named interrupt line a spawn can grant (`bridge spawn --irq NAME`): the GIC INTID (SPI + 32). Enabled and routed to the kernel's core when granted; `.Lirq` (main.rs) masks a line that fires and marks it pending; the waiting thread handles the device and acks. The kernel's own lines (the keys, the slice timer) are not here.
pub struct IrqLine {
    pub name: &'static str,
    pub intid: u32,
}
/// gpn0-0, the touch controller's INT: SPI 0 (IRQ_ALIVE_CUSTOM_EINT0, zuma-pinctrl.dtsi; gpn0 is a one-pin bank with its own line).
pub const IRQ_TOUCH_INT: u32 = 32;
/// DECON0 "frame_done": SPI 242 (zuma-drm-dpu.dtsi); "frame_start": SPI 243.
pub const IRQ_DECON0_FRAME_DONE: u32 = 274;
pub const IRQ_DECON0_FRAME_START: u32 = 275;
pub static IRQS: &[IrqLine] = &[
    // The touch controller's INT line: gpn0-0 (docs/oracle/TOUCH.md), the far-alive EINT bank.
    IrqLine { name: "touch-int", intid: IRQ_TOUCH_INT },
    // DECON0's frame-done: the frame the panel is reading has gone out; the next can be written.
    IrqLine { name: "decon0-frame-done", intid: IRQ_DECON0_FRAME_DONE },
    // DECON0's frame-start: the trigger took and the frame is going out (what the display's `settle` waits for).
    IrqLine { name: "decon0-frame-start", intid: IRQ_DECON0_FRAME_START },
];
/// Set by `.Lirq` with any pending byte (and a `sev`), cleared when `wake_sleepers` has looked: an idle core looks for interrupt waiters only when this says so.
#[unsafe(no_mangle)]
pub static mut __irq_any: u64 = 0;
/// The earliest deadline any sleeping or timed-waiting thread has (u64::MAX = none), kept by `deadlines_sync` under the lock: an idle core takes the lock to wake sleepers only once it has passed, instead of every millisecond (160,000 contended acquisitions in 80 s at idle before this, 2026-10-23).
pub static NEXT_WAKE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);
fn deadlines_sync() {
    let kk = k();
    let mut next = u64::MAX;
    for t in kk.threads.iter().flatten() {
        match t.state {
            State::Sleeping(d) => next = next.min(d),
            State::Waiting(_, d) if d != 0 => next = next.min(d),
            _ => {}
        }
    }
    NEXT_WAKE.store(next, core::sync::atomic::Ordering::Release);
}
/// One byte per INTID, set by `.Lirq` when a granted line fires (the line is masked at the same time), cleared by `irq_wait` when a thread takes it.
#[unsafe(no_mangle)]
pub static mut __irq_pending: [u8; 1024] = [0; 1024];
/// Threads block on an interrupt as `Waiting(intid, deadline)`: an INTID is far below any arc address, so the two never meet.
fn irq_pending_take(intid: u32) -> bool {
    unsafe {
        let p = &raw mut __irq_pending[intid as usize];
        let v = core::ptr::read_volatile(p);
        if v != 0 {
            core::ptr::write_volatile(p, 0);
            // A finger on the glass: the clock policy's sign of someone using the phone (service.rs clock_policy).
            if intid == IRQ_TOUCH_INT {
                INPUT_AT.store(crate::platform::cntpct(), core::sync::atomic::Ordering::Relaxed);
            }
        }
        v != 0
    }
}
/// When a touch interrupt was last taken by the touch driver (counter ticks; 0 never): the clock policy holds the active clocks for a second after it.
pub static INPUT_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Fills the facts (service.rs, which owns the vault, the USB link and the buses).
pub static mut FACTS: Option<fn() -> [u64; FACT_COUNT]> = None;

/// The device preferences block in every process's grants page (SETTINGS.md): byte 0 is the number base, tatau's code (10 arabic, 12 DMS, 16 hex, G#8C DLS); byte 1 names under the map's circles; the rest is reserved. Written at spawn and rewritten in every live process when a preference changes, so a process reads the current value from its own memory each time it draws.
pub const PREFS: usize = 0xFC0;
pub const PREF_BASE: usize = 0;
/// Preference 1: names under the map's circles, 0 off (the default: icons identify) or 1 on.
pub const PREF_LABELS: usize = 1;
/// Preference 2: the screen, 0 on (the default) or 1 off — toggled by the kernel on each power-key press (service.rs); the display server darkens the panel, the map stops taking touches.
pub const PREF_SCREEN_OFF: usize = 2;
/// Preference 3: brightness, in stops below the brightest white (0 = white at half the panel's peak; each step halves it; 8 is the dimmest); the display server turns it into the white its table uses and the panel's own brightness (DISPLAY-CALIB.md, "Decided").
pub const PREF_BRIGHTNESS: usize = 3;
/// Preference 4: the dither, 0 spatial and temporal (four phase frames flipped every 100 ms: 14 bits on the 8-bit link), 1 spatial only (12 bits; the panel can rest at its idle floor between frames).
pub const PREF_DITHER: usize = 4;
/// Preference 5: the theme, an index into ferros_sys::theme::THEMES (0 = grey, the default).
pub const PREF_THEME: usize = 5;
/// Preference 6: the proximity wake (owner's rule, 2026-08-21: the screen lights only if the power key is pressed and nothing is near the proximity sensor), 0 off (the default until it is tuned) or 1 on (service.rs).
pub const PREF_PROX_WAKE: usize = 6;
/// Preference 7: the light scope, 0 off or 1–6 the VD6282 channel its 16 kHz trace follows (in the sensor's register order, VD6283 datasheet: 1 red, 2 visible, 3 blue, 4 green, 5 IR, 6 clear — provisional for the VD6282) (usf.rs, the display's scope widget).
pub const PREF_SCOPE: usize = 7;
/// How many themes ferros_sys carries (ferros_sys::theme::THEMES): the valid range of PREF_THEME.
pub const THEMES: u8 = 4;

/// A preference as the kernel holds it now.
pub fn pref(index: usize) -> u8 {
    if index < 32 { unsafe { PREFS_NOW[index] } } else { 0 }
}
static mut PREFS_NOW: [u8; 32] = [0; 32];

/// Keeps a changed preference in the vault: (preference, value) -> stored. Registered by whoever owns the vault (service.rs).
pub static mut PREF_STORE: Option<fn(usize, u8) -> bool> = None;

/// Set a preference in kernel memory and in every live process (no persistence: the caller has done that, or the value came from the vault).
pub fn set_pref(index: usize, value: u8) {
    if index >= 32 {
        return;
    }
    unsafe { (*(&raw mut PREFS_NOW))[index] = value };
    if index == PREF_BASE {
        if let Some(b) = tatau::Base::from_code(value) {
            tatau::set_base(b);
        }
    }
    lock();
    write_pref_all(index, value);
    unlock();
}

/// The preference into every live process's page, with the kernel lock already held (from inside a syscall: `set_pref` there waited on its own core's lock). Then every thread waiting on a word wakes (its `wait` returns 0 and its loop looks again — every wait in the system is a loop that re-checks), so a server asleep until its next deadline sees the change now: the screen came back on the power key only at the display's next tick, anywhere up to a second later (owner, 2026-10-24). Interrupt waits are left alone.
fn write_pref_all(index: usize, value: u8) {
    let kk = k();
    for p in kk.procs.iter().flatten() {
        p.write((PREFS + index) as u64, &[value]);
    }
    for i in 0..kk.threads.len() {
        if let Some(x) = kk.threads[i].as_mut() {
            if matches!(x.state, State::Waiting(a, _) if a >= 1024) {
                x.state = State::Runnable;
                x.frame.x[0] = 0;
                enqueue_back(i);
            }
        }
    }
}

/// Run `f` with the kernel lock held — the service loop's way into the vault, which is part of the kernel's locked state (syscalls reach it with the lock held; KERNEL.md "Cores"). Inside `f`, only the `_held` variants of lock-taking functions.
pub fn with_lock<R>(f: impl FnOnce() -> R) -> R {
    lock();
    let r = f();
    unlock();
    r
}

/// `f` under the kernel lock, whether or not this core already holds it (a syscall does; the service loop does not): for code both reach, such as the flash log.
pub fn with_lock_held_or<R>(f: impl FnOnce() -> R) -> R {
    if HOLDER.load(core::sync::atomic::Ordering::Relaxed) == core() + 1 {
        f()
    } else {
        with_lock(f)
    }
}

/// `set_pref` for a caller already holding the kernel lock (a syscall, or `with_lock`).
pub fn set_pref_held(index: usize, value: u8) {
    set_pref_locked(index, value)
}

/// `set_wall` for a caller already holding the kernel lock.
pub fn set_wall_held(base: u64, tz: i64) {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&base.to_le_bytes());
    b[8..].copy_from_slice(&tz.to_le_bytes());
    unsafe { (&mut *(&raw mut PREFS_NOW))[PREF_WALL..PREF_WALL + 16].copy_from_slice(&b) };
    for p in k().procs.iter().flatten() {
        p.write((PREFS + PREF_WALL) as u64, &b);
    }
}

/// `set_pref` for the syscall path, where the lock is held.
fn set_pref_locked(index: usize, value: u8) {
    if index >= 32 {
        return;
    }
    unsafe { (*(&raw mut PREFS_NOW))[index] = value };
    if index == PREF_BASE {
        if let Some(b) = tatau::Base::from_code(value) {
            tatau::set_base(b);
        }
    }
    write_pref_all(index, value);
}

/// Bytes 16-31 of the preferences block: the wall clock's base (unix seconds × counter frequency − the counter, u64 LE; 0 = not read yet) and the time zone (seconds east of UTC, i64 LE), so a process reads the time from its own memory and counter with no syscall (rtc.rs; ferros_sys::local_time). Set by the kernel when either changes.
pub const PREF_WALL: usize = 16;
pub fn set_wall(base: u64, tz: i64) {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&base.to_le_bytes());
    b[8..].copy_from_slice(&tz.to_le_bytes());
    unsafe { (&mut *(&raw mut PREFS_NOW))[PREF_WALL..PREF_WALL + 16].copy_from_slice(&b) };
    lock();
    for p in k().procs.iter().flatten() {
        p.write((PREFS + PREF_WALL) as u64, &b);
    }
    unlock();
}

/// Whether `value` is valid for preference `index`.
fn pref_valid(index: usize, value: u8) -> bool {
    match index {
        PREF_BASE => tatau::Base::from_code(value).is_some(),
        PREF_LABELS => value <= 1,
        PREF_SCREEN_OFF => value <= 1,
        PREF_BRIGHTNESS => value <= 8,
        PREF_DITHER => value <= 1,
        PREF_THEME => value < THEMES,
        PREF_PROX_WAKE => value <= 1,
        PREF_SCOPE => value <= 6,
        _ => false,
    }
}

/// Where `SYS_SPAWN` finds capsules: whoever owns the vault registers a reader (vault key -> bytes) while it is alive. Kernel-side only; a process never reads the vault through it, it only names a capsule to start, and the gate still checks the capsule's signature.
pub static mut CAPSULES: Option<fn(&[u8]) -> Option<Vec<u8>>> = None;
/// Once pcie1 streams its firmware via SYS_INPUT, input above this is streamed only and the arc stays small; today the full input is still inlined as a bridge (see spawn).
#[allow(dead_code)]
const INLINE_INPUT: usize = 4096;

/// A named DMA region a spawn can grant (`bridge spawn --dma NAME`): memory a device DMAs into, mapped non-cacheable and EL0 read/write (cachemap DMA_POOL). One per device, held for the boot: a driver that dies may leave its device still pointing into it, so it is never handed to anything else.
pub struct Dma {
    pub name: &'static str,
    pub base: usize,
    pub size: usize,
}
pub static DMA: &[Dma] = &[
    Dma { name: "pcie1-dma", base: crate::cachemap::DMA_POOL.0, size: crate::cachemap::DMA_POOL.1 - crate::cachemap::DMA_POOL.0 },
    Dma { name: "display-fb", base: crate::cachemap::DISPLAY_FB.0, size: crate::cachemap::DISPLAY_FB.1 - crate::cachemap::DISPLAY_FB.0 },
    // Not DMA at all: the display server's cacheable heap, granted the same way (a base the process may use).
    Dma { name: "display-heap", base: crate::cachemap::DISPLAY_HEAP.0, size: crate::cachemap::DISPLAY_HEAP.1 - crate::cachemap::DISPLAY_HEAP.0 },
    // The display's four temporal dither phases (cachemap DISPLAY_PHASES), non-cacheable like the framebuffer.
    Dma { name: "display-phases", base: crate::cachemap::DISPLAY_PHASES.0, size: crate::cachemap::DISPLAY_PHASES.1 - crate::cachemap::DISPLAY_PHASES.0 },
    // The apps' 16-bit surface (cachemap APP_SURFACE): cacheable, shared by the display server and the app in front.
    Dma { name: "app-surface", base: crate::cachemap::APP_SURFACE.0, size: crate::cachemap::APP_SURFACE.1 - crate::cachemap::APP_SURFACE.0 },
    // The display's sprite cache (cachemap DISPLAY_SPRITES): cacheable, the display server's alone.
    Dma { name: "display-sprites", base: crate::cachemap::DISPLAY_SPRITES.0, size: crate::cachemap::DISPLAY_SPRITES.1 - crate::cachemap::DISPLAY_SPRITES.0 },
    // The display's 16-bit canvas (cachemap DISPLAY_CANVAS): its back buffer.
    Dma { name: "display-canvas", base: crate::cachemap::DISPLAY_CANVAS.0, size: crate::cachemap::DISPLAY_CANVAS.1 - crate::cachemap::DISPLAY_CANVAS.0 },
];
/// A named scope of the device vault a spawn can grant: every key under `prefix`. A process names keys relative to its scope and cannot reach outside it (no `/`-style escapes exist: the kernel only ever prepends).
pub struct StoreScope {
    pub name: &'static str,
    pub prefix: &'static str,
}
pub static STORES: &[StoreScope] = &[
    // The map's own state: the arrangement the owner made by hand.
    StoreScope { name: "map", prefix: "map." },
];
/// The vault behind `SYS_STORE`, registered by whoever owns it (service.rs): (full key, None = get / Some(value) = set) -> Ok(the value for a get, None if absent) or Err.
pub static mut STORE: Option<fn(&str, Option<&[u8]>) -> Result<Option<Vec<u8>>, ()>> = None;
/// The largest value `SYS_STORE` moves either way.
pub const STORE_MAX: usize = 4096;

const ENTROPY_CONTEXT: &str = "ferros husky 2026-10-15 entropy syscall v1";
static ENTROPY_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A named block of device registers a spawn can grant (`bridge spawn --hw NAME`). The spawner's authority stands in for a hardware-granting capability until there is one. `via_el3`: only the EL3 monitor can reach it (husky's PMU), so accesses go through SMC_CMD_PRIV_REG and only whitelisted offsets answer.
pub struct Hw {
    pub name: &'static str,
    pub base: usize,
    pub size: usize,
    pub via_el3: bool,
}

/// The dev hardware registry: blocks a process may be granted by name (WIFI.md §0: PCIe ch1 sits in HSI2).
pub static HARDWARE: &[Hw] = &[
    Hw { name: "pcie1-elbi", base: 0x1312_0000, size: 0x2000, via_el3: false },
    Hw { name: "pcie1-soc", base: 0x1313_0000, size: 0x5000, via_el3: false },
    Hw { name: "pcie1-pcs", base: 0x1314_0000, size: 0x5000, via_el3: false },
    Hw { name: "pcie1-udbg", base: 0x1315_0000, size: 0xD000, via_el3: false },
    Hw { name: "pcie1-phy", base: 0x1316_0000, size: 0x3000, via_el3: false },
    Hw { name: "pcie1-dbi", base: 0x1340_0000, size: 0x38_1000, via_el3: false },
    Hw { name: "pcie1-cfg", base: 0x60FF_E000, size: 0x2000, via_el3: false },
    Hw { name: "pcie1-mem", base: 0x6000_0000, size: 0xFF_0000, via_el3: false },
    Hw { name: "hsi2-sysreg", base: 0x1302_0000, size: 0x2000, via_el3: false },
    Hw { name: "gpio-hsi2", base: 0x1304_0000, size: 0x1000, via_el3: false },
    Hw { name: "gpio-far-alive", base: 0x154E_0000, size: 0x1000, via_el3: false },
    Hw { name: "cmu-hsi2", base: 0x1300_0000, size: 0x8000, via_el3: false },
    Hw { name: "pmu", base: 0x1546_0000, size: 0x1_0000, via_el3: true },
    // The display pipe (DISPLAY-DESIGN.md §3; sizes from zuma-drm-dpu.dtsi): DECON0's four blocks and its DQE, layer 0's DMA / DPP / SRAMC / HDR blocks, DSIM0 with its PHY and the DPU sysreg, and the GPIO bank with the panel's TE pin.
    Hw { name: "decon0-main", base: 0x1947_0000, size: 0x6000, via_el3: false },
    Hw { name: "decon0-win", base: 0x1948_0000, size: 0xE000, via_el3: false },
    Hw { name: "decon0-sub", base: 0x1949_0000, size: 0x1_0000, via_el3: false },
    Hw { name: "decon0-wincon", base: 0x194A_0000, size: 0xE000, via_el3: false },
    Hw { name: "dqe0", base: 0x194D_0000, size: 0xF000, via_el3: false },
    Hw { name: "dqe0-cgc", base: 0x194F_0000, size: 0x1_0000, via_el3: false },
    Hw { name: "dpu-dma0", base: 0x1990_0000, size: 0x1000, via_el3: false },
    Hw { name: "dpp0", base: 0x1993_0000, size: 0x1000, via_el3: false },
    Hw { name: "sramc0", base: 0x1995_0000, size: 0x1000, via_el3: false },
    Hw { name: "hdrcomm0", base: 0x1996_0000, size: 0x1000, via_el3: false },
    Hw { name: "hdr0", base: 0x1998_0000, size: 0x1000, via_el3: false },
    Hw { name: "dsim0", base: 0x1944_0000, size: 0x1000, via_el3: false },
    Hw { name: "dphy0", base: 0x1946_0000, size: 0x1000, via_el3: false },
    Hw { name: "dpu-sysreg", base: 0x1942_1000, size: 0x1000, via_el3: false },
    Hw { name: "gpio-gpp0", base: 0x1084_0000, size: 0x1000, via_el3: false },
    // The touch controller (docs/oracle/TOUCH.md): the HSI0 USI2 SPI port it answers on, and the GPIO bank carrying its interrupt line (gpn0 pin 0, read as a level).
    // Its clocks, rails and reset are not here on purpose: the kernel's `touch::up` does those (ACPM and the HSI0 CMU), so the server never needs a privileged path.
    Hw { name: "usi2-spi", base: 0x111D_0000, size: 0x1000, via_el3: false },
    Hw { name: "gpio-gpn0", base: 0x1506_0000, size: 0x1000, via_el3: false },
];
const ERR: u64 = u64::MAX;
/// What a polling `recv` returns when no message is waiting.
pub const NOTHING: u64 = u64::MAX - 1;
/// What `spawn` reports for a process still running when its slice ended (it continues in later slices).
pub const PREEMPTED: u64 = 0x5110 << 48;
/// Time slice, in generic-timer ticks per millisecond multiples.
const SLICE_MS: u64 = 10;
/// Exit value marking a fault: FAULT | EC << 40 | faulting PC (arc offset) << 16 | fault address (arc offset, low 16 bits).
pub const FAULT: u64 = 0xFA << 56;
/// What `spawn` reports for a process that is still alive, blocked in recv or call.
pub const PARKED: u64 = 0xB10C << 48;

/// Copy the head of an endpoint's queue into a server waiting in recv: message into its buffer, a fresh single-use reply token into its arc and cspace, x0 = message length. The server becomes runnable.
fn deliver(server: usize, ep: usize) {
    let kk = k();
    let Some((caller, msg)) = kk.endpoints[ep].queue.pop_front() else { return };
    let Some(st) = th(server) else { return };
    let Some(sp) = proc_of(server) else { return };
    let State::Receiving(_, buf, cap, tok_off) = st.state else { return };
    let n = msg.len().min(cap as usize);
    sp.write(buf, &msg[..n]);
    let token = mint(Object::Reply(caller), RIGHT_SEND, sp.epoch);
    sp.cspace.push(CapEntry { token, object: Object::Reply(caller), rights: RIGHT_SEND });
    sp.write(tok_off, &token);
    st.frame.x[0] = n as u64;
    st.state = State::Runnable;
    // To the front: the caller is about to block, and the server it woke is the thread to run now (a direct hand-off, on this core).
    enqueue_front(server);
    tr(TR_DELIVER, server, ep as u64);
}

/// Take the next runnable process (if any) and make it the one the trap returns into.
fn switch_to_next(f: &mut Frame) -> bool {
    while let Some(next) = pick_runnable() {
        if let Some(np) = th(next) {
            if np.state == State::Runnable {
                np.state = State::Running;
                *f = np.frame;
                frame_check(f);
                cur_set(Some(next));
                tr(TR_SWITCH, next, 0);
                return true;
            }
        }
    }
    cur_set(None);
    tr(TR_PARK, 255, 0);
    false
}

/// The first queued thread this core may run, taken off the queue (a dead entry is dropped and the next tried).
fn pick_runnable() -> Option<usize> {
    let kk = k();
    let i = kk.runnable.iter().position(|&t| kk.threads.get(t).and_then(|x| x.as_ref()).is_none_or(|x| allowed_here(x.mask)))?;
    let t = kk.runnable.remove(i);
    runnable_sync();
    t
}

/// Called from `.Lel0_sync` for every synchronous exception taken from EL0, with the running process's live frame. Returns 0 to eret through `f` (which may now be another process's frame), 1 to leave EL0 (value in f.x[0]).
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_trap(f: &mut Frame) -> u64 {
    lock();
    let r = el0_trap(f);
    unlock();
    if r == 0 {
        // Back to EL0 with a slice timer still running: a tick that fired during this syscall was taken at EL2 (.Lirq_tick clears the timer and returns), and without this the thread would run on untimed until its next syscall (2026-10-23).
        let ctl: u64;
        unsafe { core::arch::asm!("mrs {}, cnthp_ctl_el2", out(reg) ctl) };
        if ctl & 1 == 0 {
            TICK_REARMS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let ticks = crate::platform::cntfrq() / 1000;
            unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", t = in(reg) ticks, c = out(reg) _) };
        }
        // The frame the trap returns through (this thread's or the one it switched to) must have interrupts open.
        frame_check(f);
    }
    r
}

/// A thread of a dying process traps (its process ended on another core while it ran): it leaves now, the last one freeing the arc. Returns true if the thread is gone.
fn drain_dying(cur: usize) -> bool {
    let kk = k();
    let Some(pi) = kk.threads[cur].as_ref().map(|t| t.proc) else { return true };
    if !kk.procs.get(pi).and_then(|p| p.as_ref()).is_some_and(|p| p.dying) {
        return false;
    }
    kk.threads[cur] = None;
    if !kk.threads.iter().any(|x| matches!(x, Some(x) if x.proc == pi)) {
        if let Some(p) = kk.procs[pi].take() {
            arc_free(p.arc);
        }
    }
    true
}

fn el0_trap(f: &mut Frame) -> u64 {
    let kk = k();
    let Some(cur) = cur_get() else {
        f.x[0] = ERR;
        return 1;
    };
    if drain_dying(cur) {
        return if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 };
    }
    let ec = (f.esr >> 26) & 0x3F;
    let pi = kk.threads[cur].as_ref().map(|t| t.proc).unwrap_or(usize::MAX);
    let base = proc_of(cur).map(|p| p.arc.base as u64).unwrap_or(0);
    // The whole process ends (exit, a fault): every thread of it goes, then the arc.
    let end = |f: &mut Frame, value: u64| -> u64 {
        end_process(pi, cur, value);
        if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
    };
    if ec != 0x15 {
        // Not a syscall: the process faulted. It ends; the kernel carries on. Its full state goes to the console first (ESR, ELR/FAR/SP as arc offsets, every register), so a fault is debuggable from the console alone.
        let v = FAULT | ec << 40 | (f.elr.wrapping_sub(base) & 0xFF_FFFF) << 16 | (f.far.wrapping_sub(base) & 0xFFFF);
        fault_report(f, base);
        return end(f, v);
    }
    let p = proc_of(cur).unwrap();
    let t = th(cur).unwrap();
    // Crumbs round every syscall (G#90 + number entering, G#70 + number leaving), so a watchdog reset inside one is readable from PREV_EXIT.
    let nr = f.x[8];
    crate::platform::crumb(0x90 + nr.min(SYS_LAST));
    let r = match f.x[8] {
        SYS_EXIT => end(f, f.x[0]),
        SYS_THREAD_SPAWN => {
            let (entry, stack, arg) = (f.x[0], f.x[1], f.x[2]);
            let m = p.arc.mask() as u64;
            let mut frame = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0, v: [0; 32], fpcr: 0, fpsr: 0 };
            frame.elr = p.arc.base as u64 + (entry & m);
            frame.sp_el0 = p.arc.base as u64 + (stack & m & !15);
            frame.x[28] = p.arc.base as u64;
            frame.x[27] = m;
            frame.x[0] = arg;
            let mask = t.mask;
            let nt = add_thread(Thread { proc: pi, frame, state: State::Runnable, mask });
            enqueue_back(nt);
            tr(TR_RUN, nt, 0xFFFF);
            f.x[0] = 0;
            0
        }
        SYS_PIN => {
            t.mask = if f.x[0] as u16 == 0 { MASK_WORKERS } else { f.x[0] as u16 };
            f.x[0] = 0;
            0
        }
        SYS_THREAD_EXIT => {
            let others = kk.threads.iter().enumerate().any(|(i, x)| i != cur && matches!(x, Some(x) if x.proc == pi));
            if !others {
                end(f, f.x[0])
            } else {
                kk.threads[cur] = None;
                for ep in kk.endpoints.iter_mut() {
                    if ep.poller == Some(cur) {
                        ep.poller = None;
                    }
                }
                for q in kk.procs.iter_mut().flatten() {
                    q.cspace.retain(|c| !matches!(c.object, Object::Reply(r) if r == cur));
                }
                kk.exited.push((cur, f.x[0]));
                if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
            }
        }
        SYS_IRQ_WAIT | SYS_IRQ_ACK => {
            let token = p.read(f.x[0], 32);
            match p.lookup(&token).map(|i| p.cspace[i].object) {
                Some(Object::Irq(i)) => {
                    let intid = IRQS[i].intid;
                    if nr == SYS_IRQ_ACK {
                        crate::killswitch::unmask_spi(intid);
                        f.x[0] = 0;
                        0
                    } else if irq_pending_take(intid) {
                        f.x[0] = 0;
                        0
                    } else {
                        crate::killswitch::enable_spi(intid);
                        let deadline = f.x[1];
                        let t_now: u64;
                        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t_now) };
                        if deadline != 0 && deadline <= t_now {
                            f.x[0] = 2;
                            0
                        } else {
                            t.state = State::Waiting(intid as usize, deadline);
                            t.frame = *f;
                            t.frame.x[0] = 0;
                            tr(TR_SLEEP, cur, intid as u64);
                            if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
                        }
                    }
                }
                _ => {
                    f.x[0] = ERR;
                    0
                }
            }
        }
        SYS_WAIT => {
            let addr = p.arc.base + ((f.x[0] as usize) & p.arc.mask() & !7);
            let now = unsafe { core::ptr::read_volatile(addr as *const u64) };
            let deadline = f.x[2];
            let t_now: u64;
            unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t_now) };
            if now != f.x[1] {
                f.x[0] = 1;
                0
            } else if deadline != 0 && deadline <= t_now {
                f.x[0] = 2;
                0
            } else {
                t.state = State::Waiting(addr, deadline);
                t.frame = *f;
                t.frame.x[0] = 0;
                tr(TR_SLEEP, cur, 0xFFFF);
                if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
            }
        }
        SYS_WAKE => {
            let addr = p.arc.base + ((f.x[0] as usize) & p.arc.mask() & !7);
            let mut n = 0u64;
            for (i, x) in kk.threads.iter_mut().enumerate() {
                if n >= f.x[1] {
                    break;
                }
                if let Some(x) = x {
                    if x.proc == pi && matches!(x.state, State::Waiting(a, _) if a == addr) {
                        x.state = State::Runnable;
                        enqueue_back(i);
                        tr(TR_WAKE, i, 0xFFFF);
                        n += 1;
                    }
                }
            }
            f.x[0] = n;
            0
        }
        SYS_STORE => {
            let token = p.read(f.x[0], 32);
            let scope = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Store(i), r)) if r & RIGHT_SEND != 0 => STORES.get(i),
                _ => None,
            };
            let (set, len) = (f.x[1] & 1 == 1, (f.x[1] >> 8) as usize);
            let rel = p.read(f.x[2], (f.x[3] as usize).min(65));
            f.x[0] = match (scope, unsafe { STORE }) {
                (Some(sc), Some(store)) if !rel.is_empty() && rel.len() <= 64 && rel.iter().all(|&b| (0x21..0x7F).contains(&b)) && len <= STORE_MAX => {
                    let mut key = alloc::string::String::from(sc.prefix);
                    key.push_str(core::str::from_utf8(&rel).unwrap_or(""));
                    if set {
                        let value = p.read(f.x[4], len);
                        if store(&key, Some(&value)).is_ok() { 0 } else { ERR }
                    } else {
                        match store(&key, None) {
                            Ok(Some(v)) => {
                                p.write(f.x[4], &v[..v.len().min(len)]);
                                v.len() as u64
                            }
                            Ok(None) => NOTHING,
                            Err(()) => ERR,
                        }
                    }
                }
                _ => ERR,
            };
            0
        }
        SYS_LIGHT => {
            let want = ((f.x[1] as usize).saturating_sub(16) / 4).min(crate::usf::SCOPE_LEN);
            let (end, rate, mask) = crate::usf::scope_head();
            let n = want.min(end as usize);
            let mut b = alloc::vec::Vec::with_capacity(16 + 4 * n);
            b.extend_from_slice(&end.to_le_bytes());
            b.extend_from_slice(&rate.to_le_bytes());
            b.extend_from_slice(&mask.to_le_bytes());
            crate::usf::scope_copy(end, n, &mut b);
            p.write(f.x[0], &b);
            f.x[0] = b.len() as u64;
            0
        }
        SYS_FACTS => {
            let facts = unsafe { FACTS }.map(|f| f()).unwrap_or([FACT_UNKNOWN; FACT_COUNT]);
            let mut b = [0u8; FACT_COUNT * 8];
            for (i, v) in facts.iter().enumerate() {
                b[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
            }
            let n = (f.x[1] as usize).min(b.len());
            p.write(f.x[0], &b[..n]);
            f.x[0] = n as u64;
            0
        }
        SYS_SET_PREF => {
            let token = p.read(f.x[0], 32);
            let held = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0);
            let (index, value) = (f.x[1] as usize, f.x[2] as u8);
            f.x[0] = if held && pref_valid(index, value) && unsafe { PREF_STORE }.map(|st| st(index, value)).unwrap_or(false) {
                set_pref_locked(index, value);
                0
            } else {
                ERR
            };
            0
        }
        SYS_SPAWN => {
            // Parse [name len][name][key len][key][n][n tokens] from the caller's arc and resolve each token in the caller's own table: a child gets only what its parent holds, at the same rights, and never a reply.
            let req = p.read(f.x[0], (f.x[1] as usize).min(4096));
            let mut at = 0usize;
            let mut take = |n: usize| -> Vec<u8> { let s = req[at.min(req.len())..(at + n).min(req.len())].to_vec(); at += n; s };
            let nl = take(1).first().copied().unwrap_or(0) as usize;
            let name = take(nl);
            let kl = take(1).first().copied().unwrap_or(0) as usize;
            let key = take(kl);
            let n = take(1).first().copied().unwrap_or(0) as usize;
            let mut objects = Vec::new();
            let mut bad = false;
            for _ in 0..n {
                let tok = take(32);
                match p.lookup(&tok).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                    Some((Object::Reply(_), _)) | Some((Object::Console, _)) | None => bad = true,
                    Some(o) => objects.push(o),
                }
            }
            let r = if bad { Err("spawn: a token the caller does not hold (or a reply/console)") } else { spawn_queued(if name.is_empty() { &key } else { &name }, &key, &objects) };
            let kk = k();
            match r {
                Ok(_) => f.x[0] = 0,
                Err(e) => {
                    kk.console.extend_from_slice(b"spawn refused: ");
                    kk.console.extend_from_slice(e.as_bytes());
                    kk.console.push(b'\n');
                    f.x[0] = ERR;
                }
            }
            0
        }
        SYS_SLEEP => {
            let now: u64;
            unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) now) };
            if f.x[0] <= now {
                // A deadline already passed is a yield: anyone runnable goes first, this process comes back round. A server whose wake time had fallen into the past spun its whole slice on sleep/recv and held every client until the slice timer (the display, 2026-10-23); now it gives the core up on each pass instead.
                f.x[0] = 0;
                if kk.runnable.is_empty() {
                    0
                } else {
                    t.state = State::Runnable;
                    t.frame = *f;
                    enqueue_back(cur);
                    tr(TR_YIELD, cur, 0);
                    if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
                }
            } else {
                t.state = State::Sleeping(f.x[0]);
                t.frame = *f;
                t.frame.x[0] = 0;
                tr(TR_SLEEP, cur, ((f.x[0] - now) * 1000 / crate::platform::cntfrq()).min(65535));
                if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
            }
        }
        SYS_CALL => {
            let token = p.read(f.x[0], 32);
            match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Console, r)) if r & RIGHT_SEND != 0 => {
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    for &c in &msg {
                        if p.line_start {
                            kk.console.extend_from_slice(&p.name);
                            kk.console.extend_from_slice(b": ");
                        }
                        kk.console.push(c);
                        p.line_start = c == b'\n';
                    }
                    let n = 2usize.min(f.x[4] as usize);
                    p.write(f.x[3], &b"ok"[..n]);
                    f.x[0] = n as u64;
                    0
                }
                Some((Object::Endpoint(ep), r)) if r & RIGHT_SEND != 0 => {
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    t.state = State::Calling(f.x[3], f.x[4]);
                    t.frame = *f;
                    tr(TR_CALL, cur, ep as u64);
                    kk.endpoints[ep].queue.push_back((cur, msg));
                    // A server thread already waiting on this endpoint takes it now.
                    if let Some(server) = kk.threads.iter().position(|q| matches!(q, Some(q) if matches!(q.state, State::Receiving(e, ..) if e == ep))) {
                        deliver(server, ep);
                    } else if let Some(s) = kk.endpoints[ep].poller {
                        // A polling server asleep between polls: wake it now (its sleep returns early, which a poll loop reads as "look again").
                        if let Some(q) = th(s) {
                            // Asleep, or in a timed interrupt wait (a driver that serves an endpoint and waits for its device in one loop): back with 2, as at its deadline.
                            if matches!(q.state, State::Sleeping(_)) || matches!(q.state, State::Waiting(a, _) if a < 1024) {
                                if matches!(q.state, State::Waiting(..)) {
                                    q.frame.x[0] = 2;
                                }
                                q.state = State::Runnable;
                                enqueue_back(s);
                                tr(TR_WAKE_POLL, s, ep as u64);
                            }
                        }
                    }
                    if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
                }
                _ => {
                    f.x[0] = ERR;
                    0
                }
            }
        }
        SYS_RECV => {
            let token = p.read(f.x[0], 32);
            match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                // x4 bit 0 = poll: with nothing queued, return NOTHING at once instead of blocking — for a server that must also keep polling its hardware (the WiFi driver's rings).
                Some((Object::Endpoint(ep), r)) if r & RIGHT_RECV != 0 && f.x[4] & 1 != 0 && kk.endpoints[ep].queue.is_empty() => {
                    kk.endpoints[ep].poller = Some(cur);
                    f.x[0] = NOTHING;
                    tr(TR_POLL_MISS, cur, ep as u64);
                    0
                }
                Some((Object::Endpoint(ep), r)) if r & RIGHT_RECV != 0 => {
                    t.state = State::Receiving(ep, f.x[1], f.x[2], f.x[3]);
                    t.frame = *f;
                    tr(TR_RECV_BLOCK, cur, ep as u64);
                    if !kk.endpoints[ep].queue.is_empty() {
                        deliver(cur, ep);
                    }
                    if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
                }
                _ => {
                    f.x[0] = ERR;
                    0
                }
            }
        }
        SYS_MMIO_READ | SYS_MMIO_WRITE => {
            // A hardware grant: offset (x1) must be 4-byte aligned and inside the grant; the access goes straight to the register, or through the EL3 monitor for blocks only it can reach (the PMU).
            let token = p.read(f.x[0], 32);
            f.x[0] = match p.lookup(&token).map(|i| p.cspace[i].object) {
                Some(Object::Hardware(g)) => {
                    let hw = &HARDWARE[g];
                    let off = f.x[1] as usize;
                    if off % 4 != 0 || off >= hw.size {
                        ERR
                    } else if f.x[8] == SYS_MMIO_READ {
                        if hw.via_el3 { crate::platform::priv_reg((hw.base + off) as u64, false, 0) & 0xFFFF_FFFF } else { unsafe { core::ptr::read_volatile((hw.base + off) as *const u32) as u64 } }
                    } else {
                        if hw.via_el3 {
                            crate::platform::priv_reg((hw.base + off) as u64, true, f.x[2] as u32);
                        } else {
                            unsafe { core::ptr::write_volatile((hw.base + off) as *mut u32, f.x[2] as u32) };
                        }
                        0
                    }
                }
                _ => ERR,
            };
            0
        }
        SYS_INPUT => {
            let off = f.x[0] as usize;
            let len = (f.x[2] as usize).min(4096);
            let slice: Vec<u8> = p.input.get(off..).map(|t| t[..len.min(t.len())].to_vec()).unwrap_or_default();
            p.write(f.x[1], &slice);
            f.x[0] = slice.len() as u64;
            0
        }
        SYS_DMA => {
            let token = p.read(f.x[0], 32);
            f.x[0] = match p.lookup(&token).map(|i| p.cspace[i].object) {
                Some(Object::Dma(i)) if DMA[i].size > 0 => DMA[i].base as u64,
                _ => ERR,
            };
            0
        }
        SYS_ENTROPY => {
            let token = p.read(f.x[0], 32);
            let len = f.x[2] as usize;
            f.x[0] = match (p.lookup(&token).map(|i| p.cspace[i].object), crate::wairua::draw()) {
                (Some(Object::Entropy), Some(w)) if len <= 4096 => {
                    let n = ENTROPY_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    let mut h = blake3::Hasher::new_keyed(&blake3::derive_key(ENTROPY_CONTEXT, &w));
                    h.update(&n.to_le_bytes());
                    let mut out = alloc::vec![0u8; len];
                    h.finalize_xof().fill(&mut out);
                    p.write(f.x[1], &out);
                    len as u64
                }
                _ => ERR,
            };
            0
        }
        SYS_REPLY => {
            let token = p.read(f.x[0], 32);
            match p.lookup(&token) {
                Some(i) if matches!(p.cspace[i].object, Object::Reply(_)) => {
                    let Object::Reply(caller) = p.cspace[i].object else { unreachable!() };
                    // Single use: the token leaves the cspace before anything else happens.
                    p.cspace.swap_remove(i);
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    let mut handed = false;
                    if let (Some(ct), Some(cp)) = (th(caller), proc_of(caller)) {
                        if let State::Calling(roff, rcap) = ct.state {
                            let n = msg.len().min(rcap as usize);
                            cp.write(roff, &msg[..n]);
                            ct.frame.x[0] = n as u64;
                            ct.state = State::Runnable;
                            // The caller runs now: it waited. The server goes to the back of the queue (KERNEL.md "Threads, interrupts and cores": direct hand-off both ways).
                            enqueue_front(caller);
                            kk.ipc_round_trips += 1;
                            tr(TR_REPLY, caller, cur as u64);
                            handed = true;
                        }
                    }
                    f.x[0] = 0;
                    if handed {
                        t.state = State::Runnable;
                        t.frame = *f;
                        enqueue_back(cur);
                        if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
                    } else {
                        0
                    }
                }
                _ => {
                    f.x[0] = ERR;
                    0
                }
            }
        }
        _ => {
            f.x[0] = ERR;
            0
        }
    };
    // Leaving: the number, the process the trap entered from (bits 8-15), the process it returns into or FF when it leaves EL0 (bits 16-23).
    crate::platform::crumb(0x70 + nr.min(SYS_LAST) | (cur as u64) << 8 | cur_get().map(|c| c as u64).unwrap_or(0xFF) << 16);
    deadlines_sync();
    r
}

/// A thread into the table (a free slot or the end). Returns its index.
fn add_thread(t: Thread) -> usize {
    let kk = k();
    match kk.threads.iter().position(|q| q.is_none()) {
        Some(i) => {
            kk.threads[i] = Some(t);
            i
        }
        None => {
            kk.threads.push(Some(t));
            kk.threads.len() - 1
        }
    }
}

/// End process `pi` from thread `cur` (exit, a fault, the last thread leaving): every thread of it removed from the tables and the endpoints, every reply capability pointing at any of them dropped, its exit said on the console, its arc freed.
fn end_process(pi: usize, cur: usize, value: u64) {
    let kk = k();
    let mine: Vec<usize> = kk.threads.iter().enumerate().filter(|(_, x)| matches!(x, Some(x) if x.proc == pi)).map(|(i, _)| i).collect();
    // A thread running on another core right now is left to leave at its own next trap (`drain_dying`): its arc must stay until then.
    let elsewhere = mine.iter().any(|&i| i != cur && on_a_core(i));
    for &i in &mine {
        if i != cur && on_a_core(i) {
            continue;
        }
        kk.threads[i] = None;
        kk.runnable.retain(|&r| r != i);
        runnable_sync();
        for ep in kk.endpoints.iter_mut() {
            ep.queue.retain(|(caller, _)| *caller != i);
            if ep.poller == Some(i) {
                ep.poller = None;
            }
        }
        for q in kk.procs.iter_mut().flatten() {
            q.cspace.retain(|c| !matches!(c.object, Object::Reply(r) if r == i));
        }
    }
    if let Some(p) = kk.procs.get_mut(pi).and_then(|x| x.as_mut()) {
        // Say how it ended on the console: a process that outlives its first slice (spawn already returned) has no one else to hear its exit value — a fault or a panic would otherwise vanish.
        if !p.line_start {
            kk.console.push(b'\n');
        }
        let name = p.name.clone();
        kk.console.extend_from_slice(&name);
        kk.console.extend_from_slice(b": exit G#");
        for i in (0..16).rev() {
            kk.console.push(b"0123456789ABCDEF"[((value >> (4 * i)) & 0xF) as usize]);
        }
        kk.console.push(b'\n');
        if elsewhere {
            kk.procs[pi].as_mut().unwrap().dying = true;
        } else if let Some(p) = kk.procs[pi].take() {
            arc_free(p.arc);
        }
    }
    kk.exited.push((cur, value));
}

/// A faulting process's state on the console: `name: FAULT esr … elr+… far+… sp+…` then x0..x30, eight per line (G# hex; the three addresses as offsets into its arc).
fn fault_report(f: &Frame, base: u64) {
    let kk = k();
    let Some(cur) = cur_get() else { return };
    let name = thread_name(cur);
    let hex = |out: &mut Vec<u8>, v: u64| {
        out.extend_from_slice(b"G#");
        for i in (0..16).rev() {
            out.push(b"0123456789ABCDEF"[((v >> (4 * i)) & 0xF) as usize]);
        }
    };
    let mut o = Vec::new();
    o.extend_from_slice(&name);
    o.extend_from_slice(b": FAULT esr ");
    hex(&mut o, f.esr);
    o.extend_from_slice(b" elr+");
    hex(&mut o, f.elr.wrapping_sub(base));
    o.extend_from_slice(b" far ");
    hex(&mut o, f.far);
    o.extend_from_slice(b" sp+");
    hex(&mut o, f.sp_el0.wrapping_sub(base));
    o.push(b'\n');
    for (i, r) in f.x.iter().enumerate() {
        if i % 8 == 0 {
            o.extend_from_slice(&name);
            o.extend_from_slice(b":  x");
            o.push(b'0' + (i / 10) as u8);
            o.push(b'0' + (i % 10) as u8);
        }
        o.push(b' ');
        hex(&mut o, *r);
        if i % 8 == 7 || i == 30 {
            o.push(b'\n');
        }
    }
    kk.console.extend_from_slice(&o);
}

/// The slice ran out while the current process was at EL0: park it runnable (back of the queue) and leave to the kernel.
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_preempt(f: &mut Frame) -> u64 {
    lock();
    if let Some(cur) = cur_get() {
        if !drain_dying(cur) {
            if let Some(p) = th(cur) {
                p.frame = *f;
                p.state = State::Runnable;
                enqueue_back(cur);
            }
            tr(TR_PREEMPT, cur, 0);
        }
    }
    cur_set(None);
    f.x[0] = PREEMPTED;
    unlock();
    1
}

unsafe extern "C" {
    /// main.rs: enter EL0 at `entry` with SP_EL0 = `sp`, x28 = arc base, x27 = arc mask, x0..x2 = args; returns when no process is left to run (the value of the last to leave, PARKED or PREEMPTED).
    fn ferros_el0_enter(entry: usize, sp: usize, base: usize, mask: usize, a0: u64, a1: u64, a2: u64) -> u64;
    /// main.rs: resume a parked process from its saved frame; returns like `ferros_el0_enter`.
    fn ferros_el0_resume(frame: *const Frame) -> u64;
}

fn arm_slice() {
    static PPI: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !PPI.swap(true, core::sync::atomic::Ordering::Relaxed) {
        crate::killswitch::enable_ppi(26);
    }
    let ticks = crate::platform::cntfrq() / 1000 * SLICE_MS;
    unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", t = in(reg) ticks, c = out(reg) _) };
}

fn disarm_slice() {
    unsafe { core::arch::asm!("msr cnthp_ctl_el2, xzr", "isb") };
}

/// Give the next runnable process one time slice (called from the service loop). Returns false if nothing was runnable. Its console output collects until `take_console`.
/// Sleepers whose deadline has passed become runnable (called before every slice).
fn wake_sleepers() {
    let kk = k();
    unsafe { core::ptr::write_volatile(&raw mut __irq_any, 0) };
    let now: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) now) };
    for (i, p) in kk.threads.iter_mut().enumerate() {
        if let Some(p) = p {
            if let State::Sleeping(d) = p.state {
                if d <= now {
                    p.state = State::Runnable;
                    enqueue_back(i);
                    tr(TR_WAKE, i, ((now - d) * 1000 / crate::platform::cntfrq()).min(65535));
                }
            } else if let State::Waiting(a, d) = p.state {
                // An interrupt the thread waits on has fired: it runs first (the device is waiting on it).
                if a < 1024 && irq_pending_take(a as u32) {
                    p.state = State::Runnable;
                    p.frame.x[0] = 0;
                    enqueue_front(i);
                    tr(TR_WAKE, i, a as u64);
                    continue;
                }
                // A timed wait whose deadline passed: back with 2.
                if d != 0 && d <= now {
                    p.state = State::Runnable;
                    p.frame.x[0] = 2;
                    enqueue_back(i);
                    tr(TR_WAKE, i, 0xFFFE);
                }
            }
        }
    }
    deadlines_sync();
}

pub fn run_slice() -> bool {
    lock();
    wake_sleepers();
    let kk = k();
    let Some(next) = pick_runnable() else {
        unlock();
        return false;
    };
    let Some(p) = th(next) else {
        unlock();
        return true;
    };
    if p.state != State::Runnable {
        unlock();
        return true;
    }
    p.state = State::Running;
    let mut frame = p.frame;
    frame_check(&mut frame);
    cur_set(Some(next));
    tr(TR_RUN, next, kk.runnable.len() as u64);
    SLICES[core()].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    unlock();
    arm_slice();
    unsafe { ferros_el0_resume(&frame) };
    disarm_slice();
    lock();
    cur_set(None);
    unlock();
    true
}

/// Append to the kernel's process console (e.g. a boot-time spawn's output, kept for `diag console`).
pub fn push_console(bytes: &[u8]) {
    lock();
    k().console.extend_from_slice(bytes);
    unlock();
}

/// Drain what processes have sent their consoles since the last drain.
pub fn take_console() -> Vec<u8> {
    lock();
    let v = core::mem::take(&mut k().console);
    unlock();
    v
}

/// End a process by name from outside (the service loop, `diag restart`), whatever it is doing: its arc freed, its calls still queued on any endpoint dropped, and every reply capability pointing at it removed from every other process — so a late reply can never reach whatever process takes its slot next. A server's queued callers stay queued on the endpoint for the next server. Returns whether a process of that name was found. Never call it for the process that is running (only from the kernel's own loop, where none is).
pub fn kill(name: &[u8]) -> bool {
    lock();
    let r = kill_locked(name);
    unlock();
    r
}
fn kill_locked(name: &[u8]) -> bool {
    let kk = k();
    let Some(i) = kk.procs.iter().position(|p| matches!(p, Some(p) if p.name == name)) else { return false };
    let mine: Vec<usize> = kk.threads.iter().enumerate().filter(|(_, x)| matches!(x, Some(x) if x.proc == i)).map(|(k, _)| k).collect();
    // A thread of it running on another core leaves at its next trap (`drain_dying`); the arc goes with the last of them.
    let elsewhere = mine.iter().any(|&t| on_a_core(t));
    if elsewhere {
        kk.procs[i].as_mut().unwrap().dying = true;
    } else if let Some(p) = kk.procs[i].take() {
        arc_free(p.arc);
    }
    for t in mine {
        if on_a_core(t) {
            continue;
        }
        kk.threads[t] = None;
        kk.runnable.retain(|&r| r != t);
        runnable_sync();
        for ep in kk.endpoints.iter_mut() {
            ep.queue.retain(|(caller, _)| *caller != t);
            if ep.poller == Some(t) {
                ep.poller = None;
            }
        }
        for q in kk.procs.iter_mut().flatten() {
            q.cspace.retain(|c| !matches!(c.object, Object::Reply(r) if r == t));
        }
    }
    kk.console.extend_from_slice(name);
    kk.console.extend_from_slice(b": killed\n");
    true
}

/// How many processes are alive (running, runnable or blocked).
pub fn alive() -> usize {
    lock();
    let n = alive_locked();
    unlock();
    n
}
/// The same with the lock already held — from inside a syscall (the facts provider): `alive` there took the lock its own core held and waited on itself forever, which froze the phone whenever the device's settings were open (2026-10-23).
pub fn alive_locked() -> usize {
    k().procs.iter().filter(|p| p.is_some()).count()
}

/// What a spawn left behind.
pub struct Outcome {
    /// The spawned process's exit value, or PARKED if it is still alive (blocked in recv or call).
    pub exit: u64,
    /// Everything any process sent its console while this spawn ran, tagged with the sender's name.
    pub console: Vec<u8>,
    /// Completed IPC call/reply round trips during this spawn.
    pub round_trips: u32,
}

/// Arc layout: grants page, then code.
const GRANTS: usize = 0;
const CODE: usize = 0x1000;
const STACK: usize = 64 * 1024;

/// A capability to grant at spawn: a dev endpoint by name, to serve (receive) or to use (send).
#[derive(Clone, Copy)]
pub enum Grant<'a> {
    Serve(&'a [u8]),
    Use(&'a [u8]),
    /// A hardware grant from the registry, by name.
    Hardware(&'a [u8]),
    /// The entropy right (syscall 6).
    Entropy,
    /// A DMA region from the registry, by name (syscall 7).
    Dma(&'a [u8]),
    /// The right to change device preferences (syscall 11).
    Settings,
    /// A scope of the device vault, by name (syscall 13).
    Store(&'a [u8]),
    /// An interrupt line from the registry, by name (syscalls 18, 19).
    Irq(&'a [u8]),
}

/// Where a process finds its *ihi* (the oracle's output for its capsule): the last 32 bytes of the grants page.
pub const IHI: usize = 0xFE0;

/// Spawn a signed capsule: it must pass the gate (oracle.rs); the oracle's ihi is written into the process's arc at `IHI` and the kernel keeps no copy. Then as `spawn`.
pub fn spawn_capsule(name: &[u8], capsule: &[u8], input: &[u8], grants: &[Grant], sealed: &[u8]) -> Result<Outcome, &'static str> {
    let g = crate::oracle::gate(capsule)?;
    // Masked (sandboxed) code runs only if it was installed — its seal is in the vault's `sealed` list (`sealed`: 32-byte entries) — and then without re-checking: the checker ran once at install (SANDBOX.md, run and gun). Native dev code spawns unchecked here; a production build refuses it entirely.
    if g.masked {
        let s = crate::oracle::seal(&g).ok_or("no system secret: cannot check a seal")?;
        if !sealed.chunks_exact(32).any(|e| e == s) {
            return Err("gate: masked capsule not installed (no seal)");
        }
    } else if cfg!(ferros_production) {
        return Err("gate: production refuses unmasked native code");
    }
    let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
    spawn(name, &g.code, input, grants, Some(ihi), g.masked, g.text_len)
}

/// Install a masked capsule: the gate (signature), the sandbox checker over its instruction span — once — then its seal. Returns the seal for the caller to add to the vault's `sealed` list.
pub fn install_capsule(capsule: &[u8]) -> Result<[u8; 32], &'static str> {
    let g = crate::oracle::gate(capsule)?;
    if !g.masked {
        return Err("install: only masked capsules are installed");
    }
    ferros_sandbox::check_image(&g.code[..g.text_len]).map_err(|_| "install: masked code failed the sandbox checker")?;
    crate::oracle::seal(&g).ok_or("no system secret: cannot seal")
}

/// Spawn `code` (position-independent, entry at offset 0) as a process holding a console capability (grant slot 0) plus `grants` (slots 1..), and run until nothing is runnable: it exits, or it blocks (a server in recv stays parked for later callers). `input` is copied into the arc after the code; x0 = grants offset, x1 = input offset, x2 = input length.
pub fn spawn(name: &[u8], code: &[u8], input: &[u8], grants: &[Grant], ihi: Option<[u8; 32]>, guard: bool, exec_len: usize) -> Result<Outcome, &'static str> {
    if !crate::cachemap::vhe() {
        return Err("no VHE: processes need the EL2&0 regime");
    }
    lock();
    let objects = match resolve(grants) {
        Ok(o) => o,
        Err(e) => {
            unlock();
            return Err(e);
        }
    };
    let (me, arc, in_off) = match build(name, code, input, &objects, ihi, guard, exec_len) {
        Ok(b) => b,
        Err(e) => {
            unlock();
            return Err(e);
        }
    };
    let kk = k();
    kk.exited.clear();
    let trips0 = kk.ipc_round_trips;
    if let Some(p) = th(me) {
        p.state = State::Running;
    }
    cur_set(Some(me));
    unlock();
    arm_slice();
    unsafe { ferros_el0_enter(arc.base + CODE, arc.base + arc.size - 16, arc.base, arc.mask(), GRANTS as u64, in_off as u64, input.len() as u64) };
    disarm_slice();
    lock();
    let kk = k();
    cur_set(None);
    let exit = match kk.exited.iter().find(|&&(i, _)| i == me) {
        Some(&(_, v)) => v,
        None if matches!(th(me).map(|p| p.state), Some(State::Runnable)) => PREEMPTED,
        None => PARKED,
    };
    let out = Outcome { exit, console: core::mem::take(&mut kk.console), round_trips: kk.ipc_round_trips - trips0 };
    unlock();
    Ok(out)
}

/// The grants' kernel objects and rights.
fn resolve(grants: &[Grant]) -> Result<Vec<(Object, u32)>, &'static str> {
    let mut v = Vec::new();
    for g in grants {
        v.push(match g {
            Grant::Serve(n) => (Object::Endpoint(endpoint(n)), RIGHT_RECV),
            Grant::Use(n) => (Object::Endpoint(endpoint(n)), RIGHT_SEND),
            Grant::Hardware(n) => match HARDWARE.iter().position(|h| h.name.as_bytes() == *n) {
                Some(i) => (Object::Hardware(i), RIGHT_SEND | RIGHT_RECV),
                None => return Err("no such hardware block"),
            },
            Grant::Entropy => (Object::Entropy, RIGHT_SEND),
            Grant::Dma(n) => match DMA.iter().position(|r| r.name.as_bytes() == *n) {
                Some(i) => (Object::Dma(i), RIGHT_SEND | RIGHT_RECV),
                None => return Err("no such DMA region"),
            },
            Grant::Settings => (Object::Settings, RIGHT_SEND),
            Grant::Store(n) => match STORES.iter().position(|r| r.name.as_bytes() == *n) {
                Some(i) => (Object::Store(i), RIGHT_SEND),
                None => return Err("no such store scope"),
            },
            // The line is armed at the GIC by the first `irq_wait`, not here: init holds every grant and waits on none.
            Grant::Irq(n) => match IRQS.iter().position(|r| r.name.as_bytes() == *n) {
                Some(i) => (Object::Irq(i), RIGHT_SEND),
                None => return Err("no such interrupt line"),
            },
        });
    }
    Ok(v)
}

/// Make a process — arc, capability table (console first, then `objects` in order), grants page, code, input — and its first thread, in the tables, not yet running. Returns (the thread's index, the arc, the input's offset).
fn build(name: &[u8], code: &[u8], input: &[u8], objects: &[(Object, u32)], ihi: Option<[u8; 32]>, guard: bool, exec_len: usize) -> Result<(usize, Arc, usize), &'static str> {
    cap_key().ok_or("no wairua: refusing to mint capabilities")?;
    let inline = input.len(); // bridge: full inline (pcie1 still reads input from its arc); sandboxed apps carry no input, so their arcs stay small either way. INLINE_INPUT becomes the cap once pcie1 streams firmware via SYS_INPUT (option 1).
    let arc = arc_alloc(CODE + code.len() + inline + STACK, guard).ok_or("arc pool exhausted")?;
    let epoch = EPOCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    let mut cspace = alloc::vec![CapEntry { token: mint(Object::Console, RIGHT_SEND, epoch), object: Object::Console, rights: RIGHT_SEND }];
    for &(object, rights) in objects {
        cspace.push(CapEntry { token: mint(object, rights, epoch), object, rights });
    }
    let zero = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0, v: [0; 32], fpcr: 0, fpsr: 0 };
    let p = Process { arc, cspace, epoch, dying: false, name: name.to_vec(), line_start: true, input: input.to_vec() };
    // Grants page: count, then the tokens in slot order. Then the code, then the input.
    p.write(GRANTS as u64, &(p.cspace.len() as u32).to_le_bytes());
    for (i, c) in p.cspace.iter().enumerate() {
        p.write((GRANTS + 4 + 32 * i) as u64, &c.token);
    }
    if let Some(i) = ihi {
        p.write(IHI as u64, &i);
    }
    p.write(PREFS as u64, unsafe { &*(&raw const PREFS_NOW) });
    p.write(CODE as u64, code);
    let in_off = CODE + code.len();
    p.write(in_off as u64, input);
    crate::cachemap::sync_code(arc.base + CODE, code.len());
    // W^X (SANDBOX.md): a masked process may execute only its checked .text ([CODE, CODE+exec_len)), read-only, and may not write it; everything else is non-executable. A native dev process gets the whole arc RWX. The arc returns to writable data on free (arc_free → arc_reset).
    crate::cachemap::set_arc_wx(arc.base, arc.block, arc.base + CODE, arc.base + CODE + exec_len, !guard);
    // The frame a queued start resumes into: exactly what `ferros_el0_enter` sets up (entry, stack at the arc's top, the arc's base and mask in x28/x27 for masked code, grants/input/length in x0-x2, EL0t with interrupts open).
    let mut frame = zero;
    frame.elr = (arc.base + CODE) as u64;
    frame.sp_el0 = (arc.base + arc.size - 16) as u64;
    frame.spsr = 0;
    frame.x[28] = arc.base as u64;
    frame.x[27] = arc.mask() as u64;
    frame.x[0] = GRANTS as u64;
    frame.x[1] = in_off as u64;
    frame.x[2] = input.len() as u64;
    let kk = k();
    let pi = match kk.procs.iter().position(|q| q.is_none()) {
        Some(i) => {
            kk.procs[i] = Some(p);
            i
        }
        None => {
            kk.procs.push(Some(p));
            kk.procs.len() - 1
        }
    };
    let me = add_thread(Thread { proc: pi, frame, state: State::Runnable, mask: MASK_WORKERS });
    Ok((me, arc, in_off))
}

/// `SYS_SPAWN`'s work: the capsule by vault key through the gate (and, masked, its seal), then a process holding `objects`, queued to run.
fn spawn_queued(name: &[u8], key: &[u8], objects: &[(Object, u32)]) -> Result<usize, &'static str> {
    let read = unsafe { CAPSULES }.ok_or("spawn: no vault to read capsules from")?;
    let capsule = read(key).ok_or("spawn: no such capsule in the vault")?;
    let sealed = read(b"sealed").unwrap_or_default();
    let g = crate::oracle::gate(&capsule)?;
    if g.masked {
        let s = crate::oracle::seal(&g).ok_or("no system secret: cannot check a seal")?;
        if !sealed.chunks_exact(32).any(|e| e == s) {
            return Err("gate: masked capsule not installed (no seal)");
        }
    } else if cfg!(ferros_production) {
        return Err("gate: production refuses unmasked native code");
    }
    let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
    let (me, _, _) = build(name, &g.code, b"", objects, Some(ihi), g.masked, g.text_len)?;
    enqueue_back(me);
    Ok(me)
}
