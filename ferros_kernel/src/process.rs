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
/// Where the holder took the lock (`site_word`) and when (the counter); the longest hold since boot and its site (DIAG LOCK_HOLD_*): the measure of what keeps the other cores waiting, and of what comes near the watchdog's second.
static LOCK_SITE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static LOCK_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static LOCK_HOLD_MAX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static LOCK_HOLD_SITE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The core holding the kernel lock plus one, 0 when free (platform::stall_check: whom to ask where it is).
pub fn holder() -> usize {
    HOLDER.load(core::sync::atomic::Ordering::Relaxed)
}

/// A lock call site as one word: bits 0-23 the line, bits 24-55 the first four bytes of the file's name (`serv` for service.rs, `proc` for process.rs, `ptli`, `flas`, `part`, `sens`: unique among the kernel's files that take the lock). A line fits 24 bits (no source file is sixteen million lines).
fn site_word(l: &core::panic::Location) -> u64 {
    let f = l.file().as_bytes();
    let base = f.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let mut tag = [0u8; 4];
    for (t, &c) in tag.iter_mut().zip(&f[base..]) {
        *t = c;
    }
    l.line() as u64 | (u32::from_le_bytes(tag) as u64) << 24
}

/// Take the kernel lock. `#[track_caller]` (here and in `with_lock`) makes the call site the one that names the hold: the post-mortem note and the longest-hold measure say which `with_lock` or syscall path held it (the 2026-11-08 reset left only "core 8").
#[track_caller]
fn lock() {
    use core::sync::atomic::Ordering;
    if LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        LOCK_SPINS.fetch_add(1, Ordering::Relaxed);
        let mut spins = 0u32;
        while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
            // Every 4096th turn: is the holder stalled past half the watchdog's window? (platform::stall_check: one SGI to it, its place kept for the post-mortem.) WHY wrapping: a spin past 2^32 turns (minutes, a hang the watchdog ended long before) only feeds this every-4096th test, where a wrap changes nothing.
            spins = spins.wrapping_add(1);
            if (spins & 0xFFF) == 0 {
                crate::platform::stall_check();
            }
        }
    }
    let site = site_word(core::panic::Location::caller());
    HOLDER.store(core() + 1, Ordering::Relaxed);
    LOCK_SITE.store(site, Ordering::Relaxed);
    LOCK_AT.store(crate::platform::cntpct(), Ordering::Relaxed);
    // Who holds it and from where, for the post-mortem (a hang with the lock held names its core and its call site).
    crate::platform::note(0, core() as u64 + 1 | site << 8);
}
fn unlock() {
    use core::sync::atomic::Ordering::Relaxed;
    let held = crate::platform::cntpct().wrapping_sub(LOCK_AT.load(Relaxed));
    if held > LOCK_HOLD_MAX.load(Relaxed) {
        LOCK_HOLD_MAX.store(held, Relaxed);
        LOCK_HOLD_SITE.store(LOCK_SITE.load(Relaxed), Relaxed);
    }
    HOLDER.store(0, Relaxed);
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
    let c = core();
    unsafe { (*(&raw mut CURRENT))[c] = t };
    prof_note(c, t);
}
/// The profiler's view of what this core runs (prof.rs): the thread, its process's epoch, its image base and its arc, which the tick reads without the lock; and, while a run is on, the thread's names, kept once for the host.
fn prof_note(c: usize, t: Option<usize>) {
    let ctx = t.and_then(|t| proc_of(t).map(|p| crate::prof::Ctx { thread: t, epoch: p.epoch, base: p.arc.base + CODE, lo: p.arc.base, hi: p.arc.base + p.arc.size }));
    crate::prof::running(c, ctx);
    if let Some(x) = ctx {
        if crate::prof::on() && crate::prof::unseen(x.thread, x.epoch) {
            prof_keep(x.thread);
        }
    }
}
fn prof_keep(t: usize) {
    if let Some(p) = proc_of(t) {
        crate::prof::seen(t, p.epoch, p.arc.base + CODE, &p.name, thread_name(t));
    }
}
/// A profiler run starts (prof.rs, lock held): every thread alive now, named.
pub fn prof_seed() {
    for t in 0..k().threads.len() {
        if k().threads[t].is_some() {
            prof_keep(t);
        }
    }
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
/// A thread that pets, made runnable again: its pause (a wait on the world, or a call) ends here.
fn unpause(t: usize) {
    if let Some(p) = proc_of(t) {
        if p.pet_thread == t && p.paused_at != 0 {
            p.paused += crate::platform::cntpct() - p.paused_at;
            p.paused_at = 0;
        }
    }
}
/// A thread onto the run queue, and the idle cores that may run it told.
fn enqueue_back(t: usize) {
    unpause(t);
    k().runnable.push_back(t);
    runnable_sync();
    kick(mask_of(t));
}
fn enqueue_front(t: usize) {
    unpause(t);
    k().runnable.push_front(t);
    runnable_sync();
    kick(mask_of(t));
}

static ARCS: Global<Arcs> = Global(UnsafeCell::new(Arcs { free: [const { Vec::new() }; 32], init: false }));
/// The big pool's buddy lists (cachemap POOL_BIG): arcs of 16 MiB and up, 2 MiB blocks, no per-page W^X.
static ARCS_BIG: Global<Arcs> = Global(UnsafeCell::new(Arcs { free: [const { Vec::new() }; 32], init: false }));
const BIG_FROM: usize = 16 << 20;

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

fn arcs_big() -> &'static mut Arcs {
    let a = unsafe { &mut *ARCS_BIG.0.get() };
    if !a.init {
        // The pool as aligned power-of-two chunks, largest first: its end is where the secure carveout begins, not a power of two from its start.
        let (p0, p1) = crate::cachemap::POOL_BIG;
        let mut at = p0;
        while at < p1 {
            let order = (at.trailing_zeros() as usize).min(usize::BITS as usize - 1 - (p1 - at).leading_zeros() as usize);
            a.free[order].push(at);
            at += 1 << order;
        }
        a.init = true;
    }
    a
}

/// Which pool an address is in.
fn pool_of(base: usize) -> (&'static mut Arcs, (usize, usize)) {
    let (b0, b1) = crate::cachemap::POOL_BIG;
    if base >= b0 && base < b1 { (arcs_big(), (b0, b1)) } else { (arcs(), crate::cachemap::POOL) }
}

fn buddy_take(a: &mut Arcs, block: usize) -> Option<usize> {
    let k = block.trailing_zeros() as usize;
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
    Some(base)
}

/// An arc: `size` usable bytes at `base` (a power of two, base aligned to `size`), carved from a `block`-byte buddy block (= `size`, or `2 * size` for a guarded arc, whose upper half is unused guard so an in-arc immediate offset can't reach another arc — SANDBOX.md).
#[derive(Clone, Copy)]
pub struct Arc {
    pub base: usize,
    pub size: usize,
    pub block: usize,
    /// A masked arc's guard above it (GUARD bytes at base + size, kernel-only pages), 0 for a native arc.
    pub guard: usize,
    /// Exact mode's limit (SANDBOX.md, "Modes"): checked code reaches [base, base + limit); 0 in the other modes, whose x27 is the mask.
    pub limit: usize,
}

/// The guard above a masked arc: the reach of an immediate offset past its last byte (SANDBOX.md: 65520 + 15).
const GUARD: usize = 64 << 10;

impl Arc {
    pub fn mask(&self) -> usize {
        self.size - 1
    }
    /// What a process's x27 holds: exact mode's limit, or the mask.
    pub fn x27(&self) -> usize {
        if self.limit != 0 { self.limit } else { self.size - 1 }
    }
    /// The top of what the process may use (its first stack is just under it): the limit in exact mode, the arc's end otherwise.
    pub fn top(&self) -> usize {
        self.base + if self.limit != 0 { self.limit } else { self.size }
    }
}

/// Allocate the smallest arc of at least `bytes` (buddy split from the pool), zeroed. `guard` reserves a guard of the arc's own size on each side (a 2S block, SANDBOX.md): the base is 2S-aligned, hence S-aligned, so masking stays a single `and`, and the usable arc never abuts another.
pub fn arc_alloc(bytes: usize, masked: bool) -> Option<Arc> {
    let want = bytes.max(1 << ARC_MIN_LOG2).next_power_of_two();
    let block = if masked { want * 2 } else { want };
    // Masked (guarded) arcs need the page pool's per-page W^X; a big native arc goes to the big pool, and a small one too when the page pool is full.
    // Both pools carry 4 KiB pages since 2026-11-03, so a masked arc may live in either: the page pool first, the big pool when it is large or the page pool is full.
    let base = if masked && block < BIG_FROM {
        buddy_take(arcs(), block).or_else(|| buddy_take(arcs_big(), block))?
    } else if block >= BIG_FROM {
        buddy_take(arcs_big(), block).or_else(|| buddy_take(arcs(), block))?
    } else {
        buddy_take(arcs(), block).or_else(|| buddy_take(arcs_big(), block))?
    };
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, want) };
    if !masked {
        return Some(Arc { base, size: want, block, guard: 0, limit: 0 });
    }
    // The doubled block is kept only for its alignment: the arc, then a 64 KiB guard nobody can read (its pages kernel-only, so neither another process nor stale data from an arc freed there is in reach), and the rest of the upper half back to the pool as aligned chunks (64 KiB at S + 64 KiB, 128 KiB at S + 128 KiB, ... S/2 at S + S/2). A 128 MiB arc costs 128 MiB and 64 KiB, not 256 MiB.
    crate::cachemap::guard_pages(base + want, GUARD, true);
    let (a, _) = pool_of(base);
    let mut piece = GUARD;
    while piece < want {
        a.free[piece.trailing_zeros() as usize].push(base + want + piece);
        piece <<= 1;
    }
    Some(Arc { base, size: want, block: want, guard: GUARD, limit: 0 })
}

/// Arcs returned to the pools so far: a spawn refused for want of room is tried again only once this has moved (service.rs, the installer after attest), never on a timer.
pub static ARC_FREES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Return an arc to its pool, merging buddies; a masked arc's guard goes back with it.
pub fn arc_free(arc: Arc) {
    ARC_FREES.fetch_add(1, core::sync::atomic::Ordering::Release);
    // Whatever the process held is gone with it: the kernel wipes the arc as it reclaims it, so no process wipes its own secrets on its way out (owner, 2026-11-08: every end is one shot dead, no cleanup; the arc was zeroed only when handed out again until then).
    // After the reset: the grants page and the code are read-only until it returns the arc to writable data.
    crate::cachemap::arc_reset(arc.base, arc.block);
    unsafe { core::ptr::write_bytes(arc.base as *mut u8, 0, arc.size) };
    if arc.guard != 0 {
        crate::cachemap::guard_pages(arc.base + arc.size, arc.guard, false);
        buddy_give(arc.base + arc.size, arc.guard);
    }
    buddy_give(arc.base, arc.block);
}

/// A free block of `block` bytes at `base` back to its pool's lists, merged with its buddies while they are free.
fn buddy_give(base: usize, block: usize) {
    let (a, (p0, p1)) = pool_of(base);
    let mut base = base;
    let mut k = block.trailing_zeros() as usize;
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
    /// Served by the kernel: a per-purpose key derived from the device's ira (`SYS_IRA`) — the Photon device key's root.
    Ira,
    /// Served by the kernel: a scope of the device vault (index into `STORES`): get and set the keys under its prefix, named relative to it (`SYS_STORE`).
    Store(usize),
    /// An interrupt line (index into `IRQS`): `irq_wait` blocks a thread until it fires, `irq_ack` unmasks it again (KERNEL.md "Interrupts to user space").
    Irq(usize),
    /// Served by the kernel: a device's power (index into `POWERS`): rails, clocks and pin modes the driver cannot set itself (`SYS_POWER`).
    Power(usize),
    /// A surface to be: a spawn turns it into one free surface of `surfaces`, the child's own (`Dma` of `surface-k`), at build. Never held: a parent that passes the whole `surfaces` region passes this instead (SYS_SPAWN), and `bridge spawn --dma surface` asks for it by name.
    NewSurface,
    /// Served by the kernel: read-only files on a partition (index into `PARTS`), answered by partfs.rs `part_call`.
    Part(usize),
    /// Served by the kernel: the right to unlock the person's identity class of the vault from their seed, and to lock it (`SYS_UNLOCK`, AUTH.md). The attest screen holds it; init holds it to see whether an identity is unlocked.
    Unlock,
    /// Served by the kernel: the USB driver process's way into the kernel's PT commands (`SYS_PT`, ptlink.rs): hand one over, take its reply.
    Pt,
    /// Served by the kernel: the sensors process's way to publish what it reads (`SYS_SENSORS`, sensorlink.rs): the block the kernel answers SYS_FACTS and SYS_LIGHT from, and the board's identity for the registry.
    Sensors,
    /// Served by the kernel: a window of 4 KiB blocks on the ferros partition (index into `WINDOWS`), read and written by block number within it (`SYS_BLOCK`): the vault process's disk (VAULT-OUT.md).
    Blocks(usize),
    /// Served by the kernel: the vault process's own call (`SYS_VAULT`): the old entries of the kernel's vault, to move into its domains (VAULT-OUT.md, migration). With it, and only with it, a spawn may serve the `vault` endpoint.
    Vault,
    /// Served by the kernel, through the vault process: the holder's own vault domain (VAULT-OUT.md, "Kist, per person"), keyed by the kernel from the holder's ihi (the oracle over its developer key and option) under the person's key, so no kist opens another's data and none opens its own while no one is present. A process passes it on only as the same right: the child's is the child's own domain (`SYS_STORE` resolves it against the caller).
    Domain,
    /// Served by the kernel: the right to launch a kist (KISTS.md): `SYS_SPAWN` in kist mode, naming a capsule by its vault address and handing each declared want its right or an empty slot. The kernel makes each right's object itself (`kist_object`), so the holder needs no other grant to hand out; it holds every right a kist may be handed, and is init's alone.
    Kists,
    /// Served by the kernel: the right to install a kist (SANDBOX.md "Lifecycle"): `SYS_SPAWN` in install mode names a capsule the holder stored in `capsules.`, and the kernel reads it from the vault, gates it, runs the sandbox checker over its masked code and seals it into the `sealed` list, telling the holder each stage on a word in its own arc (`install_request`). The installer's (payloads/kists), never a kist's.
    Install,
    /// A slot left empty: a want the person said no to (owner, 2026-11-08: "a denied right is an empty slot, never a fake"). Its token is zeros and nothing looks it up.
    Empty,
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
    crate::wairua::derive(CAP_KEY_CONTEXT)
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
        Object::Power(i) => (9, i as u64),
        Object::Ira => (10, 0),
        Object::NewSurface => (11, 0),
        Object::Part(i) => (13, i as u64),
        Object::Unlock => (14, 0),
        Object::Pt => (15, 0),
        Object::Sensors => (16, 0),
        Object::Blocks(i) => (17, i as u64),
        Object::Vault => (18, 0),
        Object::Domain => (19, 0),
        Object::Kists => (20, 0),
        Object::Empty => (21, 0),
        Object::Install => (22, 0),
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
    /// TPIDR_EL0, the thread pointer: each thread's own since 2026-10-27, for a std process's thread-locals (STD-TARGET.md) — the runtime points it at the thread's TLS block and the kernel carries it across every entry and slice.
    pub tpidr: u64,
    pub _pad: u64,
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
/// The kernel's core as a mask: the X3 when the kernel moved there (board::x3), else CPU0, where it stayed. "Big" means this core: a thread pinned to the X3 runs here, and the kernel-side switches (SYS_POWER) are served here. Until 2026-11-06 it was the X3 always, so with the kernel on CPU0 (akita, or husky's FERROS_X3=0) a pinned thread waited for a core that never started and every power switch was refused.
pub fn kernel_mask() -> u16 {
    if crate::board::x3() { MASK_BIG } else { 0x0001 }
}
/// The cores that are up: the kernel's, and the workers smp.rs reports.
fn cores_up() -> u16 {
    kernel_mask() | ((crate::smp::UP.load(core::sync::atomic::Ordering::Relaxed) as u16) << 4)
}
/// Whether thread `t` may run on this core. A pin to the X3 means the kernel's core, wherever that is.
fn allowed_here(mask: u16) -> bool {
    let mask = if mask & MASK_BIG != 0 { (mask & !MASK_BIG) | kernel_mask() } else { mask };
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
    /// The process watchdog (KERNEL-OUT.md, owner 2026-11-05), measuring first: when it was spawned, its pets (SYS_PET, once a pass of its main loop), the last one, and the longest stretch between two (from the spawn for the first) with when it ended. Nothing is enforced yet.
    born: u64,
    pets: u64,
    last_pet: u64,
    gap_max: u64,
    gap_at: u64,
    /// From the spawn to the first pet: its start, which the watchdog does not count.
    start: u64,
    /// The thread that pets (the last to), and its pause: since when it has been blocked waiting on the world or on another process (0: not), and how long it has been paused since its last pet. Only that thread's waits pause the clock.
    pet_thread: usize,
    paused_at: u64,
    paused: u64,
    /// Its own vault domain's name (`Object::Domain`): a one-way derivation of its ihi, so the kernel keeps no copy of the ihi itself; None for a process spawned without one.
    domain: Option<[u8; 32]>,
    /// Launched as a kist (`spawn_kist`): its ira-derived keys are its own (`SYS_IRA`), never a system service's.
    kist: bool,
}

struct Endpoint {
    name: Vec<u8>,
    /// Calls waiting for the server: (calling thread, message).
    queue: alloc::collections::VecDeque<(usize, Vec<u8>)>,
    /// The thread that last polled this endpoint (recv with poll): a call wakes it if it is asleep, so a server that sleeps between polls answers at once rather than at its next deadline.
    poller: Option<usize>,
    /// The process (by epoch) that last received on it, a RECV polling or not; 0: none. What CALL_LIVE asks for: that process still alive. A driver that holds the right but has not received yet (still bringing its device up) does not count, and neither does a process that only holds the right to hand it on (init, which stays alive for its keepers since 2026-11-01: counting it, a live call to a dead driver waited forever).
    server: u64,
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

/// Whether `len` bytes at arc offset `off` touch the grants page ([0, CODE)): the kernel's, read-only to the process and to the kernel alike but while the kernel opens it for its own write.
fn in_grants(off: u64, len: usize) -> bool {
    len != 0 && off < CODE as u64
}

impl Process {
    /// The kernel's own writes into the grants page (the tokens, the handle table, the ihi, the preferences): at build, before the page is closed, and through `write_pref_all`, which opens it around the write.
    fn write_grants(&self, off: u64, bytes: &[u8]) {
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), (self.arc.base + off as usize) as *mut u8, bytes.len()) };
    }

    /// Where `len` bytes a syscall names at arc offset `off` lie (owner, 2026-11-08: a syscall buffer resolves or traps): in the arc, used as they are; else inside one of this process's own DMA regions (a heap or a surface outside the arc, named by the pointer the process holds, which `ferros_sys::off` turns into an offset past the arc), used directly, which gives it nothing it did not hold. None anywhere else: the caller traps (`BUF_FAULT`), where until then the offset was masked into the arc and the bytes landed in the process's own memory elsewhere (the display's wallpaper reads, 2026-11-08).
    fn span(&self, off: u64, len: usize) -> Option<usize> {
        let size = self.arc.size as u64;
        // WHY: the offset and length are the process's; the subtraction is taken only once the offset is inside the arc, so it cannot wrap.
        if off < size && len as u64 <= size - off {
            return Some(self.arc.base + off as usize);
        }
        self.dma_span((self.arc.base as u64).wrapping_add(off) & !DMA_ADDR, len)
    }
    /// A buffer the kernel will touch later (a receive buffer, a reply buffer): resolved now, at the syscall that names it, so a bad one traps there and not inside another process's call.
    fn check(&self, off: u64, len: usize) {
        if len != 0 && (self.span(off, len).is_none() || in_grants(off, len)) {
            self.bad_buf(off, len, true);
        }
    }
    /// The buffer could not be resolved: kept for the trap at the syscall's end (the first such buffer of the syscall).
    fn bad_buf(&self, off: u64, len: usize, write: bool) {
        let b = unsafe { &mut *(&raw mut BUF_FAULT) };
        if b.is_none() {
            *b = Some(BufFault { epoch: self.epoch, off, len, write });
        }
    }
    fn read(&self, off: u64, len: usize) -> Vec<u8> {
        if len == 0 {
            return Vec::new();
        }
        let mut v = alloc::vec![0u8; len];
        match self.span(off, len) {
            Some(a) => unsafe { core::ptr::copy_nonoverlapping(a as *const u8, v.as_mut_ptr(), len) },
            None => self.bad_buf(off, len, false),
        }
        v
    }
    /// A buffer a syscall names by address inside a DMA region this process holds (`DMA_ADDR | address`, as the USB driver names its 6 MiB command area): the address, when all `len` bytes lie inside one such region.
    fn dma_span(&self, addr: u64, len: usize) -> Option<usize> {
        let a = (addr & !DMA_ADDR) as usize;
        // The address and length are the process's: an end that wraps would pass the comparison below.
        let end = a.checked_add(len)?;
        self.cspace.iter().find_map(|c| match c.object {
            Object::Dma(i) if DMA[i].size > 0 && a >= DMA[i].base && end <= DMA[i].base + DMA[i].size => Some(a),
            _ => None,
        })
    }
    /// `read` into a slice the caller owns: no heap copy, for a command of megabytes (SYS_PT).
    fn read_into(&self, off: u64, dst: &mut [u8]) {
        if dst.is_empty() {
            return;
        }
        match self.span(off, dst.len()) {
            Some(a) => unsafe { core::ptr::copy_nonoverlapping(a as *const u8, dst.as_mut_ptr(), dst.len()) },
            None => {
                dst.fill(0);
                self.bad_buf(off, dst.len(), false);
            }
        }
    }
    fn write(&self, off: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // Never into the grants page (owner rule: a syscall buffer resolves or traps, 2026-11-08): it is read-only at every level, the kernel's write there faulted under its lock and reset the device (pcie1's console reply at G#A00, the 8a, 2026-11-09). The kernel's own writes there go by `write_grants`.
        if in_grants(off, bytes.len()) {
            self.bad_buf(off, bytes.len(), true);
            return;
        }
        match self.span(off, bytes.len()) {
            // In the arc (Normal write-back memory): one copy. A reply of a quarter MiB (a store transfer, STORE_MAX) went a volatile byte at a time.
            Some(a) if a >= self.arc.base && a + bytes.len() <= self.arc.base + self.arc.size => unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), a as *mut u8, bytes.len()) },
            // In the display's heap, Normal write-back memory (cachemap DISPLAY_HEAP): one copy. The display's wallpaper loader reads 24 MB a quarter MiB at a time into it, and a byte at a time cost 590 ms of a 721 ms load (2026-11-08).
            Some(a) if a >= crate::cachemap::DISPLAY_HEAP.0 && a + bytes.len() <= crate::cachemap::DISPLAY_HEAP.1 => unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), a as *mut u8, bytes.len()) },
            // In one of the process's other DMA regions (some are Device memory, where a wide unaligned store faults): bytes up to an 8-byte boundary, aligned 8-byte stores (which Device memory takes), the tail in bytes.
            Some(a) => {
                // WHY the min: a buffer shorter than the way to the next 8-byte boundary is all head.
                let head = ((8 - (a & 7)) & 7).min(bytes.len());
                for (i, &b) in bytes[..head].iter().enumerate() {
                    unsafe { core::ptr::write_volatile((a + i) as *mut u8, b) };
                }
                let words = (bytes.len() - head) / 8;
                for w in 0..words {
                    let at = head + w * 8;
                    let v = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
                    unsafe { core::ptr::write_volatile((a + at) as *mut u64, v) };
                }
                for (i, &b) in bytes[head + words * 8..].iter().enumerate() {
                    unsafe { core::ptr::write_volatile((a + head + words * 8 + i) as *mut u8, b) };
                }
            }
            None => self.bad_buf(off, bytes.len(), true),
        }
    }
    /// `write` in one copy: a block read hands back 4 KiB at a time (SYS_BLOCK).
    fn write_from(&self, off: u64, src: &[u8]) {
        if src.is_empty() {
            return;
        }
        match self.span(off, src.len()) {
            Some(a) => unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), a as *mut u8, src.len()) },
            None => self.bad_buf(off, src.len(), true),
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
            // An empty slot (a want said no to) holds the zero token and is never a capability: presenting zeros finds nothing.
            if diff == 0 && token.len() == 32 && e.object != Object::Empty {
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
    kk.endpoints.push(Endpoint { name: name.to_vec(), queue: alloc::collections::VecDeque::new(), poller: None, server: 0 });
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
/// SYS_SET_PREF x1 bit 8: the value applied to every process's block but not kept in the vault (a slider while the finger is down).
const PREF_LIVE: u64 = 1 << 8;
/// The device's plain facts for its dashboard (SHELL.md: the device circle's front): x0/x1 a buffer in the caller's arc and its length; up to FACT_COUNT u64 words, little-endian, `FACT_UNKNOWN` where a fact is not known. No capability: these are what the device shows whoever holds it, and none is a key, a handle or a serial. Returns the bytes written.
const SYS_FACTS: u64 = 12;
pub const FACT_COUNT: usize = 88;
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
/// The light scope's samples (usf.rs): x0/x1 a buffer and its length; a 16-byte header {u64 the stream's next sample number, u32 rate Hz, u32 the channel mask} then the latest samples (i32, oldest first) as many as fit, up to a second. No capability: it is the light falling on the back of the device. Returns the bytes written.
const SYS_LIGHT: u64 = 21;
/// A device's power: x0 the `Power` capability, x1 1 = on, 0 = off. Runs only on the kernel's core (the X3), where it cannot interleave with the service loop's own firmware calls (ACPM): a thread pins itself there first (`SYS_PIN` G#100, then any sleep). Returns 0, or ERR.
const SYS_POWER: u64 = 22;
/// ira_key(cap, domain, domain len, out): x0 the Ira capability's token offset, x1/x2 a domain string in the caller's arc (1-64 bytes), x3 a 32-byte buffer that receives `blake3::derive_key("ferros process ira v1 " ‖ domain, ira)`. Returns 32, or ERR (no ira this boot, no capability, bad domain).
const SYS_IRA: u64 = 23;
/// unlock(cap, op, seed): x0 the Unlock capability, x1 the op — 0 the state (bit 0 an identity is unlocked, bit 1 the device is bound to one), 1 unlock the identity whose 32-byte seed is at x2 (the number of entries it has on this device; NOTHING: the device is bound to another identity; ERR: no vault or no ira), 2 lock, 3 bind the device to the unlocked identity, 4 release the device (every identity entry deleted) (0, or ERR). AUTH.md.
const SYS_UNLOCK: u64 = 24;
/// pt(cap, op, ptr, len, offset): the kernel's PT commands for the USB driver process (ptlink.rs). `ptr` is an offset in the arc, or `DMA_ADDR | address` inside a DMA region the caller holds. Op 0 hands over a command of `len` bytes at `ptr` (op 3: the same command again, to act once its reply is out) → 0 or ERR (one is already in, or too large). Op 2 says a transfer is moving (the clock policy counts it as activity) → 0. Op 1 copies its reply from `offset` into `len` bytes at `ptr` → the reply's full length, plus bit 32 when the command waits to act and bit 33 when it gave no reply at all; NOTHING while it runs; ERR when none was handed over.
const SYS_PT: u64 = 25;
/// Bit 63 of a syscall's buffer argument (SYS_PT): the buffer is named by its address inside one of the caller's DMA regions, not by its offset in the arc.
const DMA_ADDR: u64 = 1 << 63;
/// sensors(cap, op, offset, len): the sensors process (sensorlink.rs). Op 0 publishes the block of `len` bytes at `offset` in the caller's arc (at least sensorlink `BYTES`) → 0 or ERR. Op 1 → the board's CDT | MLB << 32 from the device tree (usf.rs `read_board`), for the registry scripts.
const SYS_SENSORS: u64 = 26;
/// status(slot, word): one of a radio driver's eight measure slots for the corners' circles (WINDOWS.md), SYS_FACTS 33-80, eight words a link: the link is the caller's own, by its name (modem, pcie1, bt, nfc, gps, usb), so no capability; anyone else, or a slot past 7, gets ERR. A word is a tagged measure (ferros_sys::measure: the kind in bits 48-63, the value in its unit below), all ones empty; the kernel keeps the words and reads none of them.
const SYS_STATUS: u64 = 27;
pub static STATUS: [[core::sync::atomic::AtomicU64; 8]; 6] = [const { [const { core::sync::atomic::AtomicU64::new(u64::MAX) }; 8] }; 6];
/// A link's tracks (owner, 2026-11-07: "add as many as we need to show all"): beyond its eight measures, up to TRACKS words its driver sets as slots 8 and up of the same call, read with `tracks` (SYS_TRACKS), never through the facts, so the facts keep their size. The GPS driver's satellites first, a word each (ferros_sys::measure::SAT).
pub const TRACKS: usize = 1 << 6;
pub static TRACK: [[core::sync::atomic::AtomicU64; TRACKS]; 6] = [const { [const { core::sync::atomic::AtomicU64::new(u64::MAX) }; TRACKS] }; 6];
/// tracks(link, buf, words): link k's tracks (0 cellular … 5 USB, the facts' order) into `buf`, at most `words` and TRACKS. No capability, as the facts. → the words written, or ERR for no such link.
const SYS_TRACKS: u64 = 31;
/// control(cap, link, slot, word): one of a link's knobs (docs/RADIO-CONTROLS.md), set by whoever holds Settings (the map). Links 0-5 the drivers in the facts' order, 6 the clock (the kernel's own: slot 1 the zone, seconds east, applied at once). Slots 1 to CONTROLS - 1; every set counts the link's generation up, which its driver watches. A kept knob (settings.rs KNOBS) and the zone write the settings document; a vault that refuses it puts the old value back and returns ERR. → 0 or ERR.
const SYS_CONTROL: u64 = 32;
/// controls(link, buf, words): a link's control block into `buf`, at most `words` and CONTROLS: the generation, then slots 1 to 15 (all ones: unset, the driver's own default). No capability: a driver reads its own block, the settings page shows the values. → the words written, or ERR for no such link.
const SYS_CONTROLS: u64 = 33;
/// detail(op, …): a link's text and identifiers as one VSF document (ferros_sys::detail). Op 0, a driver (by its name, as SYS_STATUS): x1/x2 the document and its length, at most DETAIL_MAX, kept as its link's → 0 or ERR. Op 1: x1 the Settings capability's token offset, x2 the link, x3/x4 a buffer and its length → the document's whole length (0 for none yet), its first bytes copied; ERR without the capability. The kernel reads none of it; the reader verifies it.
const SYS_DETAIL: u64 = 34;
/// zone(op, …): the clock's zone (zone.rs, docs/TIME.md). Op 0, the modem (by its name, a narrow right of its own as SYS_STATUS, never Settings): the network's word, x1 its MCC (0 none), x2 NITZ's legal offset in seconds east, x3 flags (bit 0 the offset given, bit 1 the daylight flag given, bit 2 daylight time on). Op 1, the GPS driver (by its name): a new position, x1 latitude and x2 longitude in 2^-23 degrees. Op 2, Settings at x1: this device's choice, x2 1 automatic, x3/x4 its id by hand (length 0: none, the fixed offset). Op 3, Settings at x1: x2 1 follow the identity's choice, 0 this device's own. Op 4, the identity service (by its name, fgtw): the identity's choice, x1 0 by hand, 1 automatic, 2 none (forgotten), x2/x3 its id. Op 5: the state into x1/x2 (zone::STATE_LEN bytes; no capability: the zone is what the clock shows whoever holds the device). Op 6: every zone's id into x1/x2, each its length then its bytes → the list's whole length. Ops 2-4 keep the settings document, put back as it was when the vault refuses. → 0 or ERR, op 5 the bytes written, op 6 the length.
const SYS_ZONE: u64 = 35;
/// lang(op, …): the language the shell speaks (lang.rs, SETTINGS.md), a packed ISO 639-3 code (vsf `wl`'s letters; 0 none, English). Op 0, Settings at x1: this device's own language, x2 the code. Op 1, Settings at x1: x2 1 this device keeps its own, 0 it follows the identity's. Op 2, the identity service (by its name, fgtw, as SYS_ZONE op 4): the identity's language, x1 the code (0: it has none). Each keeps the settings document, put back as it was when the vault refuses. The state is read from the preferences block (PREF_LANG…), not here. → 0 or ERR.
const SYS_LANG: u64 = 36;
/// Each link's control block: words 1 to CONTROLS - 1 (word 0 is the generation, `CONTROL_GEN`), all ones until set.
pub const CONTROLS: usize = 1 << 4;
/// The links with a control block: the six of the facts, and the clock.
pub const CONTROL_LINKS: usize = 7;
/// The clock's link, and its zone's slot.
pub const CONTROL_CLOCK: usize = 6;
pub const CONTROL_ZONE: usize = 1;
pub static CONTROL: [[core::sync::atomic::AtomicU64; CONTROLS]; CONTROL_LINKS] = [const { [const { core::sync::atomic::AtomicU64::new(u64::MAX) }; CONTROLS] }; CONTROL_LINKS];
pub static CONTROL_GEN: [core::sync::atomic::AtomicU64; CONTROL_LINKS] = [const { core::sync::atomic::AtomicU64::new(0) }; CONTROL_LINKS];
/// Keeps the settings document as it now stands (the knobs and the zone with it): → stored. Registered by whoever owns the vault (service.rs), as PREF_STORE.
pub static mut CONTROL_STORE: Option<fn() -> bool> = None;
/// The largest detail document a link keeps.
pub const DETAIL_MAX: usize = 1 << 10;
/// The detail documents and their lengths, touched only under the kernel lock (inside a syscall).
static mut DETAIL: [[u8; DETAIL_MAX]; 6] = [[0; DETAIL_MAX]; 6];
static mut DETAIL_LEN: [usize; 6] = [0; 6];
/// The radio drivers by process name, in the facts' order: the link a SYS_STATUS or SYS_DETAIL caller speaks for.
const LINK_NAMES: [&[u8]; 6] = [b"modem", b"pcie1", b"bt", b"nfc", b"gps", b"usb"];
/// pet(): the process is alive and going round its main loop (the process watchdog, measuring: the kernel records the longest stretch between two pets, `diag pets`). → 0.
const SYS_PET: u64 = 28;
/// block(cap, op, block, buf): 4 KiB blocks of a window on the ferros partition (`WINDOWS`), by number within it: op bit 0 write (else read), bits 8-15 the count (1 to BLOCK_MAX), `buf` the count × 4 KiB in the caller's arc. → 0, or ERR (no such window, out of it, or the UFS command failed). The count moves as one UFS command (a run, ferros_hal `read_run_lun`) once the controller's PRDT stride is known.
const SYS_BLOCK: u64 = 29;
/// The most blocks one `SYS_BLOCK` moves: one UFS command's worth (128 KiB, ferros_hal RUN_MAX), so a 64 KiB piece the vault seals (17 blocks with its nonce, tag and furrow headers) is one call and one command. 16 until 2026-11-08, each block its own command.
const BLOCK_MAX: u64 = ferros_hal::ufs::RUN_MAX as u64;
/// The run's bounce: UFS DMAs only into its own pages (the HSI2 S2MPU allows `.bss.dma.ufs` and nothing else of the kernel's), uncached by the static cache map, so a run lands here and is copied to the caller.
#[repr(C, align(4096))]
struct RunBuf([u8; ferros_hal::ufs::RUN_MAX << 12]);
#[unsafe(link_section = ".bss.dma.ufs")]
static mut UFS_RUN: RunBuf = RunBuf([0; ferros_hal::ufs::RUN_MAX << 12]);
/// SYS_BLOCK's measure (`diag "ufs runs"`, owner 2026-11-08: "24MB from the vault should be imperceptibly fast"): calls, blocks and timer ticks inside the call (the command or commands and the copy), reads then writes.
pub static BLOCK_STATS: [core::sync::atomic::AtomicU64; 6] = [const { core::sync::atomic::AtomicU64::new(0) }; 6];
/// vault(cap, op, x2, x3, buf): the vault process's own call (`Object::Vault`), moving the kernel's vault's old entries into its domains (VAULT-OUT.md, migration); x1 bits 0-7 the op, 8-31 the buffer's length, 32-63 an offset. Op 0: the x2-th entry still to move (a skip past ones that would not move) into `buf` as [class: 1 the person's][the domain's address and sealing keys][the scope prefix's length][the full name] → its length, NOTHING when none is left. Op 1: the value of the entry named at x2/x3, from the offset → its whole length. Op 2: that entry moved, removed from the kernel's vault → 0, NOTHING if it was not there. ERR otherwise.
const SYS_VAULT: u64 = 30;
const SYS_LAST: u64 = 22;

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
    // The GPS chip's HOST_REQ ("I have data"): gpa5-3, EINT53 in the far-alive bank, its own line (zuma.h IRQ_ALIVE_EINT53 = SPI 63), level high.
    IrqLine { name: "gps-host-req", intid: 32 + 63 },
    // The USB controller (DWC3): zuma IRQ_USB32DRD_GIC_0_HSI0, SPI 402, level high.
    IrqLine { name: "usb-dwc3", intid: 32 + 402 },
    // The GPU (zuma.h IRQ_G3D_IRQ*_G3D, zuma-gpu.dtsi, level high): its GPU-control line (resets, power transitions, cache flushes), its job line (the CSF firmware's global and group interfaces), its MMU line (page faults), SPI 346-348.
    IrqLine { name: "gpu-gpu", intid: 32 + 346 },
    IrqLine { name: "gpu-job", intid: 32 + 347 },
    IrqLine { name: "gpu-mmu", intid: 32 + 348 },
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
            // A finger on the glass: the clock policy's sign of someone using the device (service.rs clock_policy).
            if intid == IRQ_TOUCH_INT {
                INPUT_AT.store(crate::platform::cntpct(), core::sync::atomic::Ordering::Relaxed);
            }
            // Frames going out: the clock policy's sign that something is animating at more than the rest flips (a focused widget).
            if intid == IRQ_DECON0_FRAME_START {
                FRAME_STARTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
        v != 0
    }
}
/// When a touch interrupt was last taken by the touch driver (counter ticks; 0 never): the clock policy holds the active clocks for a second after it.
pub static INPUT_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Display frame starts taken by the display server's frame thread, counted for the clock policy.
pub static FRAME_STARTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Fills the facts (service.rs, which owns the vault, the USB link and the buses).
pub static mut FACTS: Option<fn() -> [u64; FACT_COUNT]> = None;

/// The device preferences block in every process's grants page (SETTINGS.md): byte 0 is the number base, tatau's code (10 arabic, 12 DMS, 16 hex, G#8C DLS); byte 1 names under the map's circles; the rest is reserved. Written at spawn and rewritten in every live process when a preference changes, so a process reads the current value from its own memory each time it draws.
pub const PREFS: usize = 0xFC0;
pub const PREF_BASE: usize = 0;
/// Preference 1: names under the map's circles, 0 off (the default: icons identify) or 1 on.
pub const PREF_LABELS: usize = 1;
/// Preference 2: the screen, 0 on (the default) or 1 off — toggled by the kernel on each power-key press (service.rs); the display server darkens the panel, the map stops taking touches.
pub const PREF_SCREEN_OFF: usize = 2;
/// Preference 3: brightness, in 32nds of a stop below the brightest white (0 = white at half the panel's peak; 32 halves it; 255, almost eight stops down, the dimmest), so the slider is continuous (owner, 2026-11-08: "why's brightness not continuous?"; until then whole stops, 0 to 8); the display server turns it into the white its table uses and the panel's own brightness (DISPLAY-CALIB.md, "Decided").
pub const PREF_BRIGHTNESS: usize = 3;
/// Preference 4: the dither, 0 spatial and temporal (four phase frames flipped every 100 ms: 14 bits on the 8-bit link), 1 spatial only (12 bits; the panel can rest at its idle floor between frames).
pub const PREF_DITHER: usize = 4;
/// Preference 5: the theme, an index into ferros_sys::theme::THEMES (0 = grey, the default).
pub const PREF_THEME: usize = 5;
/// Preference 6: the proximity wake (owner's rule, 2026-08-21: the screen lights only if the power key is pressed and nothing is near the proximity sensor), 0 off (the default until it is tuned) or 1 on (service.rs).
pub const PREF_PROX_WAKE: usize = 6;
/// Preference 7: the light scope, 0 off or 1–6 the VD6282 channel its 16 kHz trace follows (in the sensor's register order, VD6283 datasheet: 1 red, 2 visible, 3 blue, 4 green, 5 IR, 6 clear — provisional for the VD6282) (usf.rs, the display's scope widget).
pub const PREF_SCOPE: usize = 7;
/// Preference 12: the keyboard layout on the honeycomb (honeycomb::Layout), 0 Dvorak (the default, owner 2026-10-27) or 1 QWERTY with Q on the bottom row. Bytes 8-11 are the surfaces' generations, 16-31 the wall clock; the language is in the second block (PREF_LANG).
pub const PREF_LAYOUT: usize = 12;
/// How many layouts the honeycomb carries: the valid range of PREF_LAYOUT.
pub const LAYOUTS: u8 = 2;
/// Preference 13: unlock at boot, 0 off (the default) or 1 on — the identity seed is stored so the device unlocks without the handle being typed (owner 2026-10-28; a stolen device then holds the identity, so it is opt-in and warned).
pub const PREF_UNLOCK_AT_BOOT: usize = 13;
/// Preference 14: the UI scale every active app lays its page out by (owner, 2026-11-05: a pinch inside an open circle, "you can easily scale any app any time"), a signed byte of 64ths of an octave: the unit times 2^(s/64), 0 the default (one), from -64 (a half) to 64 (twice).
pub const PREF_SCALE: usize = 14;
/// Preference 15: the radios switched off (ferros_sys::PREF_RADIOS_OFF), a bit each: 0 cellular, 1 WiFi, 2 Bluetooth, 3 NFC, 4 GPS. Kept in the device document; the service loop stops and starts the drivers by it.
pub const PREF_RADIOS_OFF: usize = 15;
/// The UI scale's reach either way, 64ths of an octave: half to twice (proposed 2026-11-05, the owner's to change).
pub const SCALE_REACH: i8 = 64;
/// How many themes ferros_sys carries (ferros_sys::theme::THEMES): the valid range of PREF_THEME.
pub const THEMES: u8 = 4;
/// The second preferences block in the grants page: preferences 32-47 at G#FB0, the 16 bytes under the first block that no token reaches (the tokens, 32 bytes each from G#204, end at G#FA4 with the 109th, the most the page has ever held), so the page holds as many grants as before. The first block is full (bytes 8-11 the surfaces, 16-31 the wall clock). A kernel older than it leaves these bytes zero, which reads as each one's default.
pub const PREFS_HI: usize = 0xFB0;
/// Preferences 32-38, the language (lang.rs; written by the kernel only, never thru SYS_SET_PREF): 32-33 the language in effect, 34-35 this device's own, 36-37 the identity's as its service last wrote it, each a packed ISO 639-3 code (vsf `wl`'s letters, little-endian; 0 none, English); 38 whether the device keeps its own (1) or follows the identity's (0, the default).
pub const PREF_LANG: usize = 32;
pub const PREF_LANG_DEVICE: usize = 34;
pub const PREF_LANG_IDENTITY: usize = 36;
pub const PREF_LANG_OWN: usize = 38;
/// How many preferences there are: the first block's 32 and the second's 16.
const PREF_COUNT: usize = 48;

/// Where preference `index` (below PREF_COUNT) is in a grants page.
fn pref_at(index: usize) -> usize {
    if index < 32 { PREFS + index } else { PREFS_HI + index - 32 }
}

/// A preference as the kernel holds it now.
pub fn pref(index: usize) -> u8 {
    if index < PREF_COUNT { unsafe { PREFS_NOW[index] } } else { 0 }
}
static mut PREFS_NOW: [u8; PREF_COUNT] = [0; PREF_COUNT];

/// Keeps a changed preference in the vault: (preference, value) -> stored. Registered by whoever owns the vault (service.rs).
pub static mut PREF_STORE: Option<fn(usize, u8) -> bool> = None;

/// Set a preference in kernel memory and in every live process (no persistence: the caller has done that, or the value came from the vault).
pub fn set_pref(index: usize, value: u8) {
    if index >= PREF_COUNT {
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
        // The grants page is read-only at every level: opened for the write, closed after.
        crate::cachemap::grants_page(p.arc.base, true);
        p.write_grants(pref_at(index) as u64, &[value]);
        crate::cachemap::grants_page(p.arc.base, false);
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
#[track_caller]
pub fn with_lock<R>(f: impl FnOnce() -> R) -> R {
    lock();
    let r = f();
    unlock();
    r
}

/// `f` under the kernel lock, whether or not this core already holds it (a syscall does; the service loop does not): for code both reach, such as the flash log.
#[track_caller]
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
    if index >= PREF_COUNT {
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
        PREF_BRIGHTNESS => true,
        PREF_DITHER => value <= 1,
        PREF_THEME => value < THEMES,
        PREF_PROX_WAKE => value <= 1,
        PREF_SCOPE => value <= 6,
        PREF_LAYOUT => value < LAYOUTS,
        PREF_UNLOCK_AT_BOOT => value <= 1,
        PREF_SCALE => (value as i8).unsigned_abs() <= SCALE_REACH as u8,
        // Five radios, a bit each.
        PREF_RADIOS_OFF => value < 1 << 5,
        _ => false,
    }
}

/// Whether `value` is valid for preference `index` (settings.rs, reading a document).
pub fn pref_ok(index: usize, value: u8) -> bool {
    pref_valid(index, value)
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
    // The modem's shared memory (cachemap.rs MODEM_WINDOWS, EL0 Device): pktproc's packet rings and buffers, the IPC region (the legacy rings, the handover block, the ROM image), the MSI and boot-register page.
    Dma { name: "modem-pktproc", base: crate::cachemap::MODEM_WINDOWS[0].0, size: crate::cachemap::MODEM_WINDOWS[0].1 - crate::cachemap::MODEM_WINDOWS[0].0 },
    Dma { name: "modem-ipc", base: crate::cachemap::MODEM_WINDOWS[1].0, size: crate::cachemap::MODEM_WINDOWS[1].1 - crate::cachemap::MODEM_WINDOWS[1].0 },
    Dma { name: "modem-msi", base: crate::cachemap::MODEM_WINDOWS[2].0, size: crate::cachemap::MODEM_WINDOWS[2].1 - crate::cachemap::MODEM_WINDOWS[2].0 },
    Dma { name: "pcie1-dma", base: crate::cachemap::DMA_POOL.0, size: crate::cachemap::DMA_POOL.1 - crate::cachemap::DMA_POOL.0 },
    // The USB driver process (USB.md): the controller's registers, mapped for EL0 (Device), and its non-cacheable DMA memory.
    Dma { name: "usb-regs", base: 0x1121_0000, size: 0x1_0000 },
    Dma { name: "usb-dma", base: crate::cachemap::USB_DMA.0, size: crate::cachemap::USB_DMA.1 - crate::cachemap::USB_DMA.0 },
    // The GPU driver process (GPU.md): the GPU's registers and doorbells, mapped for EL0 (Device), and the memory it and the GPU share (Normal non-cacheable; the GPU's S2MPU holds it to this).
    Dma { name: "gpu-regs", base: crate::cachemap::GPU_REGS.0, size: crate::cachemap::GPU_REGS.1 - crate::cachemap::GPU_REGS.0 },
    Dma { name: "gpu-mem", base: crate::cachemap::GPU_MEM.0, size: crate::cachemap::GPU_MEM.1 - crate::cachemap::GPU_MEM.0 },
    // The layer arena (cachemap LAYERS): CPU write-back, the GPU coherent; the GPU driver and the display server share it.
    Dma { name: "layers", base: crate::cachemap::LAYERS.0, size: crate::cachemap::LAYERS.1 - crate::cachemap::LAYERS.0 },
    // The AoC's IPC control block and its services' rings (aoc.rs IPC, cachemap.rs AOC_IPC), EL0 Device memory for the sensors process: com.google.usf's rings lie in its first G#85000 bytes.
    Dma { name: "aoc-ipc", base: crate::cachemap::AOC_IPC.0, size: crate::cachemap::AOC_IPC.1 - crate::cachemap::AOC_IPC.0 },
    Dma { name: "display-fb", base: crate::cachemap::DISPLAY_FB.0, size: crate::cachemap::DISPLAY_FB.1 - crate::cachemap::DISPLAY_FB.0 },
    // Not DMA at all: the display server's cacheable heap, granted the same way (a base the process may use).
    Dma { name: "display-heap", base: crate::cachemap::DISPLAY_HEAP.0, size: crate::cachemap::DISPLAY_HEAP.1 - crate::cachemap::DISPLAY_HEAP.0 },
    // The display's four temporal dither phases (cachemap DISPLAY_PHASES), non-cacheable like the framebuffer.
    Dma { name: "display-phases", base: crate::cachemap::DISPLAY_PHASES.0, size: crate::cachemap::DISPLAY_PHASES.1 - crate::cachemap::DISPLAY_PHASES.0 },
    // The apps' surfaces (cachemap SURFACES): all four, the display server's (it composes them) and the map's (it passes one to each app it starts).
    Dma { name: "surfaces", base: crate::cachemap::SURFACES.0, size: crate::cachemap::SURFACES.1 - crate::cachemap::SURFACES.0 },
    // Each surface alone: what an app holds. Given out by `NewSurface`, freed when its process ends (`SURFACE_OWNER`). Must stay consecutive and in order: `SURFACE0 + k` is surface k.
    Dma { name: "surface-0", base: crate::cachemap::SURFACES.0, size: crate::cachemap::SURFACE_SIZE },
    Dma { name: "surface-1", base: crate::cachemap::SURFACES.0 + crate::cachemap::SURFACE_SIZE, size: crate::cachemap::SURFACE_SIZE },
    Dma { name: "surface-2", base: crate::cachemap::SURFACES.0 + 2 * crate::cachemap::SURFACE_SIZE, size: crate::cachemap::SURFACE_SIZE },
    Dma { name: "surface-3", base: crate::cachemap::SURFACES.0 + 3 * crate::cachemap::SURFACE_SIZE, size: crate::cachemap::SURFACE_SIZE },
    // The display's sprite cache (cachemap DISPLAY_SPRITES): cacheable, the display server's alone.
    Dma { name: "display-sprites", base: crate::cachemap::DISPLAY_SPRITES.0, size: crate::cachemap::DISPLAY_SPRITES.1 - crate::cachemap::DISPLAY_SPRITES.0 },
    // The display's 16-bit canvas (cachemap DISPLAY_CANVAS): its back buffer.
    Dma { name: "display-canvas", base: crate::cachemap::DISPLAY_CANVAS.0, size: crate::cachemap::DISPLAY_CANVAS.1 - crate::cachemap::DISPLAY_CANVAS.0 },
];

/// The surfaces a spawn can give out, and where the first is in `DMA`.
pub const SURFACE_COUNT: usize = 4;
fn surface0() -> usize {
    DMA.iter().position(|r| r.name == "surface-0").unwrap()
}
/// Each surface's owner, by process epoch (0: free). Under the kernel lock.
static mut SURFACE_OWNER: [u64; SURFACE_COUNT] = [0; SURFACE_COUNT];
/// Bytes 8-11 of the preferences block: each surface's generation, 0 while it is free and a new nonzero value each time it is given out. The display server reads them on every pass: a window whose surface's byte changed or went to 0 belongs to a process that has ended (WINDOWS.md: the death notice, with no message to send).
pub const PREF_SURFACES: usize = 8;
static mut SURFACE_GEN: u8 = 0;

/// Give surface k to the process with `epoch`: the first free one, its generation byte to every process. Under the lock.
fn surface_take(epoch: u64) -> Option<usize> {
    let owners = unsafe { &mut *(&raw mut SURFACE_OWNER) };
    let k = owners.iter().position(|&o| o == 0)?;
    owners[k] = epoch;
    // The generation cycles 1-255: 0 means free.
    let g = unsafe { SURFACE_GEN = SURFACE_GEN % 255 + 1; SURFACE_GEN };
    unsafe { (*(&raw mut PREFS_NOW))[PREF_SURFACES + k] = g };
    write_pref_all(PREF_SURFACES + k, g);
    Some(k)
}

/// The surfaces of the process with `epoch` back to free. Under the lock.
fn surfaces_free(epoch: u64) {
    let owners = unsafe { &mut *(&raw mut SURFACE_OWNER) };
    for k in 0..SURFACE_COUNT {
        if owners[k] == epoch {
            owners[k] = 0;
            unsafe { (*(&raw mut PREFS_NOW))[PREF_SURFACES + k] = 0 };
            write_pref_all(PREF_SURFACES + k, 0);
        }
    }
}
/// A named scope of the device vault a spawn can grant: every key under `prefix`. A process names keys relative to its scope and cannot reach outside it (no `/`-style escapes exist: the kernel only ever prepends).
pub struct StoreScope {
    pub name: &'static str,
    pub prefix: &'static str,
}
/// A window of the ferros partition (LU 0) that a process may read and write by block, through `SYS_BLOCK`: `blocks` 4 KiB blocks from `offset` blocks into the partition.
pub struct BlockWindow {
    pub name: &'static str,
    pub offset: u64,
    pub blocks: u64,
}

/// The windows (VAULT-OUT.md, block window first cut): the kernel keeps the UFS driver and moves blocks; it does not hold values, parse them or seal them. `vault`: the vault process's two mirrors, 8 GiB each, from 4 GiB into the partition (past the flash log at 1 GiB and the kernel's own vault at 2 GiB to 2.5 GiB; every ferros partition is at least 77 GiB).
pub static WINDOWS: &[BlockWindow] = &[BlockWindow { name: "vault", offset: 1 << 20, blocks: 1 << 22 }];

pub static STORES: &[StoreScope] = &[
    // The map's own state: the arrangement the owner made by hand.
    StoreScope { name: "map", prefix: "map." },
    // The GPS driver's: its session replay (`gps.replay`, `gps.replay.sky`) and its last fix (`gps.last`).
    StoreScope { name: "gps", prefix: "gps." },
    // The WiFi driver's: its firmware (`wifi.fw`), NVRAM text (`wifi.nvram`), CLM blob (`wifi.clm`) and the network credential (`wifi.cred`; secret, never printed).
    StoreScope { name: "wifi", prefix: "wifi." },
    // The identity service's (payloads/fgtw, part of attest): the identity roots pushed at pairing (`fgtw.identity_seed`, `fgtw.handle_proof`; secret), the fleet chain and key once bound (`fgtw.chain`, `fgtw.fleet_key`).
    StoreScope { name: "fgtw", prefix: "fgtw." },
    // The attest screen's (payloads/ahau): the device's verifier for its identity (`ahau.verifier`), the identity seed under the device's ira key.
    StoreScope { name: "ahau", prefix: "ahau." },
    // The sensors process's (payloads/sensors): the AoC's registry scripts (`aoc.reg.0`, … : tools/husky-aoc-reg.sh), which it pushes to the AoC after the firmware starts.
    StoreScope { name: "aoc", prefix: "aoc." },
    // The GPU driver's (payloads/gpu): the CSF firmware (`gpu.fw`, mali_csffw-r52p0.bin from the device's own vendor image, Arm's: the vault and Harbor, never the repo).
    StoreScope { name: "gpu", prefix: "gpu." },
    // The vault process's proof (payloads/vaulttest): scratch, nothing kept.
    StoreScope { name: "test", prefix: "test." },
    // Installed kists' signed capsules (KISTS.md), the device's: each under its own content address (the blake3 of the capsule, a 32-byte address name), so two people on one device who installed the same app share one copy. The kernel reads them to launch a kist; the installer writes them.
    StoreScope { name: "capsules", prefix: "capsules." },
    // The person's list of installed kists and their answers (`kists.list`, ferros_kist; identity class, kvault IDENTITY): init reads it to launch a kist, the map shows it and writes the person's answers. Stands for the common area's `kists` until the common area's named grants exist (VAULT-OUT.md).
    StoreScope { name: "kists", prefix: "kists." },
];
/// The context deriving a process's own domain name (`Process::domain`) from its ihi.
const KIST_DOMAIN_CONTEXT: &str = "ferros 2026-11-08 kist domain";
/// The context binding a kist's ira-derived key to its gate identity (`SYS_IRA`).
const KIST_IRA_CONTEXT: &str = "ferros 2026-11-08 kist ira";
/// A process's ira key bound to the person present (SYS_IRA x4 bit 0): the WiFi address a person wears on a network, theirs and this device's together.
const PERSON_IRA_CONTEXT: &str = "ferros 2026-11-09 person ira";
/// The scope prefix every kist domain's keys are derived under, followed by the domain's name (`domain_keys`): no `STORES` prefix can collide with it (none holds a zero byte).
const KIST_DOMAIN_PREFIX: &[u8] = b"kist\0";
/// The vault behind `SYS_STORE`, registered by whoever owns it (service.rs): (full key, None = get / Some(value) = set) -> Ok(the value for a get, None if absent) or Err.
pub static mut STORE: Option<fn(&str, Option<&[u8]>) -> Result<Option<Vec<u8>>, ()>> = None;
/// Delete a key (true when it was there) and list every key of the vault (full names): what a std process's vault needs beyond get and set (kete on ferros, STD-TARGET.md).
pub static mut STORE_REMOVE: Option<fn(&str) -> Result<bool, ()>> = None;
pub static mut STORE_KEYS: Option<fn() -> Result<Vec<alloc::string::String>, ()>> = None;
/// The vault's identity half behind `SYS_UNLOCK`, registered with the store (service.rs): (op 0 query, 1 unlock with the seed, 2 lock) -> Ok(unlocked after the op) or Err.
pub static mut UNLOCK: Option<fn(u8, Option<&[u8; 32]>) -> Result<u64, ()>> = None;
/// The identity's entries as one VSF document (service.rs `store_export`), for SYS_UNLOCK op 5.
pub static mut EXPORT: Option<fn() -> Option<alloc::vec::Vec<u8>>> = None;
/// The export being read (op 5 without a vault process), from its build at offset 0 until it is read to its end; the person's plaintext, wiped then. Under the kernel lock.
static mut EXPORT_DOC: Option<alloc::vec::Vec<u8>> = None;
/// An entry kept by 32-byte address rather than by name (SYS_STORE bit 5): kete on ferros keeps its vaults this way — the address is kete's, the kernel scopes it (kvault `addr_scoped`).
pub enum AddrOp<'a> {
    Get,
    Set(&'a [u8]),
    Remove,
    /// The scope's addresses, concatenated.
    List,
}
pub static mut STORE_ADDR: Option<fn(&str, &[u8; 32], AddrOp) -> Result<Option<Vec<u8>>, ()>> = None;
/// A value larger than one transfer arriving in pieces (SYS_STORE bit 1 + bit 2): assembled here until the piece without bit 2 commits it. Keyed by the scope's prefix and the name, or the prefix, a 1 and the address.
static mut STORE_STAGE: Option<(Vec<u8>, Vec<u8>)> = None;
/// The last value read piecewise (key, value), so a large value is fetched from the vault once.
static mut STORE_CACHE: Option<(Vec<u8>, Vec<u8>)> = None;
/// The largest transfer `SYS_STORE` moves either way: 256 KiB, four of the vault process's 64 KiB pieces (ferros_store PIECE), which it opens on four cores at once (payloads/vault `Openers`). 4 KiB until 2026-11-08, when the person's 24 MB wallpaper took about 5900 round trips and 2.9 s (owner: "24MB from the vault should be imperceptibly fast").
pub const STORE_MAX: usize = 1 << 18;

/// The vault process's endpoint (VAULT-OUT.md, step 3). While a process receives on it, every `SYS_STORE` call is forwarded there (`store_forward`), stamped by the kernel with the domain keys of the caller's scope; while none does, the kernel's own vault answers as before. Nothing may call it directly (a message on it is the kernel's), and only a spawn holding the vault right (`Grant::Vault`) may serve it.
const VAULT_EP: &[u8] = b"vault";
/// Names under a scope that stay in the kernel's vault because the kernel reads them itself (aoc.rs: the AoC firmware): never forwarded, never migrated.
const KERNEL_HELD: [&str; 1] = ["aoc.fw."];
/// The domain bases, registered by the vault's owner (service.rs): (the identity's?) -> the class's base key, None while that class is locked.
pub static mut DOMAIN_BASE: Option<fn(bool) -> Option<[u8; 32]>> = None;
const DOMAIN_ADDR_CONTEXT: &str = "ferros 2026-11-07 vault domain address";
const DOMAIN_SEAL_CONTEXT: &str = "ferros 2026-11-07 vault domain seal";
/// Entries may wait in the kernel's vault for the vault process to take (`SYS_VAULT`): set from boot, by a bridge `store put` of a scoped name, and at unlock; cleared when the vault process asks and finds none. The stamp's flag bit 0.
pub static MIGRATE_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// Moved at every identity lock (kvault `lock`): the stamp carries it, and the vault process drops what it holds of the person's (its cache's plaintext) when it changes.
pub static LOCK_EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Bit 62 of a `Calling` state's reply length: the call is a forwarded `SYS_STORE`, whose reply is [the syscall's value u64 LE][the bytes for the caller's buffer].
const STORE_REPLY: u64 = 1 << 62;
/// The last old entry read for migration (name, value), so a large one is fetched from the kernel's vault once.
static mut MIGRATE_CACHE: Option<(Vec<u8>, Vec<u8>)> = None;

/// The vault endpoint, while a live process holds the right to serve it: from its spawn, not its first receive, so a call made while it opens waits for it (the GPU asked for `gpu.fw` 145 ms into a boot, before the vault served, and the kernel's vault, which no longer held it, said absent, 2026-11-07). Should it die first, the waiting calls get ERR (`fail_orphaned_calls`).
fn vault_ep() -> Option<usize> {
    let kk = k();
    let ep = kk.endpoints.iter().position(|e| e.name == VAULT_EP)?;
    let serving = kk.procs.iter().flatten().any(|p| !p.dying && p.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == ep) && c.rights & RIGHT_RECV != 0));
    serving.then_some(ep)
}

/// The scope a full name belongs to, if it may live in the vault process (not reserved, not kept by the kernel).
fn scope_of(name: &str) -> Option<&'static StoreScope> {
    if name.starts_with('\0') || KERNEL_HELD.iter().any(|h| name.starts_with(h)) {
        return None;
    }
    STORES.iter().find(|s| name.starts_with(s.prefix))
}

/// A scope's domain keys in one class (the kernel derives every key; the vault process seals with them, VAULT-OUT.md): the address key, then the sealing key. None while the class is locked. A kist's own domain is a "scope" of `KIST_DOMAIN_PREFIX` and its name, in the person's class.
fn domain_keys(prefix: &[u8], identity: bool) -> Option<[u8; 64]> {
    let mut base = unsafe { DOMAIN_BASE }?(identity)?;
    let mut out = [0u8; 64];
    for (i, ctx) in [DOMAIN_ADDR_CONTEXT, DOMAIN_SEAL_CONTEXT].iter().enumerate() {
        let mut h = blake3::Hasher::new_derive_key(ctx);
        h.update(&base);
        h.update(prefix);
        out[i << 5..(i + 1) << 5].copy_from_slice(h.finalize().as_bytes());
    }
    base.fill(0);
    Some(out)
}

/// `cur` calls endpoint `ep` with `msg` and waits for the reply, which lands at `roff` (at most `rcap` bytes, the flags above `CALL_LIVE` and `STORE_REPLY` kept with it): queued, handed to a server already receiving, or a polling server woken. SYS_CALL's path, and SYS_STORE's when it is forwarded.
fn call_endpoint(f: &mut Frame, cur: usize, ep: usize, msg: Vec<u8>, roff: u64, rcap: u64) -> u64 {
    let kk = k();
    let (Some(p), Some(t)) = (proc_of(cur), th(cur)) else {
        f.x[0] = ERR;
        return 0;
    };
    p.check(roff, (rcap & !(CALL_LIVE | STORE_REPLY)) as usize);
    // Blocked on another process: the watchdog's clock paused for the thread that pets (that process answers for its own hang).
    if cur == p.pet_thread {
        p.paused_at = crate::platform::cntpct();
    }
    t.state = State::Calling(roff, rcap);
    t.frame = *f;
    tr(TR_CALL, cur, ep as u64);
    kk.endpoints[ep].queue.push_back((cur, msg));
    wake_server(ep);
    if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
}

/// `SYS_VAULT` op 0: the `skip`-th entry of the kernel's vault that belongs in the vault process (a scope's, not one the kernel keeps, its class open), with its domain's keys: the one exception to keys only against a live call (VAULT-OUT.md, migration), once per old entry. None left: the pending mark is cleared.
fn migrate_next(p: &Process, skip: usize, buf: u64, len: usize) -> u64 {
    let Some(Ok(names)) = unsafe { STORE_KEYS }.map(|keys| keys()) else { return ERR };
    let mut seen = 0;
    for name in names {
        let Some(sc) = scope_of(&name) else { continue };
        let identity = crate::kvault::identity_class(&name);
        let Some(mut keys) = domain_keys(sc.prefix.as_bytes(), identity) else { continue };
        if seen < skip {
            seen += 1;
            keys.fill(0);
            continue;
        }
        let mut out = Vec::with_capacity(66 + name.len());
        out.push(identity as u8);
        out.extend_from_slice(&keys);
        keys.fill(0);
        out.push(sc.prefix.len() as u8);
        out.extend_from_slice(name.as_bytes());
        let n = out.len();
        if n <= len {
            p.write(buf, &out);
        }
        out.fill(0);
        return if n <= len { n as u64 } else { ERR };
    }
    MIGRATE_PENDING.store(false, core::sync::atomic::Ordering::Release);
    NOTHING
}

/// `SYS_VAULT` op 1: an old entry's value from `offset` (fetched once, kept until another is asked for or it moves).
fn migrate_read(p: &Process, name: &str, offset: usize, buf: u64, len: usize) -> u64 {
    if scope_of(name).is_none() {
        return ERR;
    }
    let cache = unsafe { &mut *(&raw mut MIGRATE_CACHE) };
    if cache.as_ref().is_none_or(|(k, _)| k != name.as_bytes()) {
        *cache = match unsafe { STORE }.map(|store| store(name, None)) {
            Some(Ok(Some(v))) => Some((name.as_bytes().to_vec(), v)),
            Some(Ok(None)) => return NOTHING,
            _ => return ERR,
        };
    }
    let Some((_, v)) = cache.as_ref() else { return ERR };
    // WHY: the offset is the vault process's and may lie past the end (nothing to copy then).
    let from = offset.min(v.len());
    let to = (offset + len).min(v.len());
    p.write(buf, &v[from..to]);
    v.len() as u64
}

/// `SYS_VAULT` op 2: an old entry the vault process has written and committed, removed from the kernel's vault.
fn migrate_done(name: &str) -> u64 {
    if scope_of(name).is_none() {
        return ERR;
    }
    unsafe { *(&raw mut MIGRATE_CACHE) = None };
    match unsafe { STORE_REMOVE }.map(|rm| rm(name)) {
        Some(Ok(true)) => 0,
        Some(Ok(false)) => NOTHING,
        _ => ERR,
    }
}

/// A bridge `store put` (service.rs) of a name the vault process holds: it waits in the kernel's vault until the vault process takes it.
pub fn note_old_put(name: &str) {
    if scope_of(name).is_some() {
        MIGRATE_PENDING.store(true, core::sync::atomic::Ordering::Release);
    }
}

/// Where a forwarded `SYS_STORE` lands: a scope of `STORES`, or the caller's own kist domain (`Object::Domain`, by its name).
enum Where {
    Scope(&'static StoreScope),
    Domain([u8; 32]),
}

/// Where a `SYS_STORE` goes: to the vault process (its endpoint, and the caller's scope or its own domain) while it serves and the name is not one the kernel keeps; None, the kernel's own vault (which holds no kist domain: a domain call with no vault process is refused there).
fn store_route(p: &Process, f: &Frame) -> Option<(usize, Where)> {
    let ep = vault_ep()?;
    let token = p.read(f.x[0], 32);
    let sc = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
        Some((Object::Store(i), r)) if r & RIGHT_SEND != 0 => STORES.get(i)?,
        Some((Object::Domain, r)) if r & RIGHT_SEND != 0 => return p.domain.map(|d| (ep, Where::Domain(d))),
        _ => return None,
    };
    if f.x[1] & 32 == 0 {
        let mut full = alloc::string::String::from(sc.prefix);
        full.extend(p.read(f.x[2], (f.x[3] as usize).min(97)).iter().map(|&b| b as char));
        if KERNEL_HELD.iter().any(|h| full.starts_with(h)) {
            return None;
        }
    }
    Some((ep, Where::Scope(sc)))
}

/// `SYS_STORE` while the vault process serves (VAULT-OUT.md, step 3): the request, stamped with the domain keys of the caller's scope, goes to the vault as a call from the caller, who waits for the reply as for any call. The kernel copies one transfer each way and keeps nothing; keys for the person's domain only while they are unlocked.
///
/// The stamped message: [op: 0 set, 1 get, 2 delete, 3 list][flags: bit 0 old entries pending][lock epoch u32 LE][domains][per domain: class (1 the person's) and the address and sealing keys, 65 bytes][name length][name: the name within the scope, or 1 and the 32-byte address][rest]. Rest: set [offset u64][more u8][bytes]; get [offset u64][length u32]; list [offset u64][length u32][addresses u8]; delete nothing.
fn store_forward(f: &mut Frame, cur: usize, ep: usize, wh: Where) -> u64 {
    let Some(p) = proc_of(cur) else {
        f.x[0] = ERR;
        return 0;
    };
    let (set, more, del, list, raw) = (f.x[1] & 1 != 0, f.x[1] & 4 != 0, f.x[1] & 8 != 0, f.x[1] & 16 != 0, f.x[1] & 32 != 0);
    let offset = if f.x[1] & 2 != 0 { f.x[1] >> 32 } else { 0 };
    let len = ((f.x[1] >> 8) & 0xFF_FFFF) as usize;
    let rel = p.read(f.x[2], (f.x[3] as usize).min(97));
    // WHY: the name and the length are the caller's; the same refusals as the kernel's own path (a name of printable ASCII up to 96 bytes or a 32-byte address; one transfer at most).
    let rel_ok = if raw { rel.len() == 32 || list } else { rel.len() <= 96 && rel.iter().all(|&b| (0x21..0x7F).contains(&b)) && (!rel.is_empty() || list) };
    if !rel_ok || len > STORE_MAX {
        f.x[0] = ERR;
        return 0;
    }
    let mut name = Vec::with_capacity(33);
    if raw && !list {
        name.push(1);
    }
    if !list {
        name.extend_from_slice(&rel);
    }
    // The domain's keys are derived under its prefix: a scope's, or a kist's own (`KIST_DOMAIN_PREFIX` and its name, the person's class always: a kist's data is the person's, open only while they are present).
    let (prefix, classes): (Vec<u8>, &[bool]) = match wh {
        Where::Scope(sc) => {
            let mut full = alloc::string::String::from(sc.prefix);
            if !raw {
                full.extend(rel.iter().map(|&b| b as char));
            }
            // The classes the request covers: a listing both where the scope holds the person's names (the device's always), anything else its own name's.
            (sc.prefix.as_bytes().to_vec(), if list && crate::kvault::identity_within(sc.prefix) { &[false, true] } else if crate::kvault::identity_class(&full) { &[true] } else { &[false] })
        }
        Where::Domain(d) => ([KIST_DOMAIN_PREFIX, &d[..]].concat(), &[true]),
    };
    let mut rest = Vec::with_capacity(13 + len);
    if set {
        rest.extend_from_slice(&offset.to_le_bytes());
        rest.push(more as u8);
        rest.extend_from_slice(&p.read(f.x[4], len));
    } else if !del {
        rest.extend_from_slice(&offset.to_le_bytes());
        rest.extend_from_slice(&(len as u32).to_le_bytes());
        if list {
            rest.push(raw as u8);
        }
    }
    let op = if list { VOP_LIST } else if del { VOP_DELETE } else if set { VOP_SET } else { VOP_GET };
    let Some(msg) = stamp(op, &prefix, classes, &name, &rest) else {
        f.x[0] = ERR;
        return 0;
    };
    let rcap = if set || del { 0 } else { len as u64 };
    call_endpoint(f, cur, ep, msg, f.x[4], rcap | STORE_REPLY)
}

/// The vault process's operations (payloads/vault): the four a `SYS_STORE` call makes, then the kernel's own.
pub const VOP_SET: u8 = 0;
pub const VOP_GET: u8 = 1;
pub const VOP_DELETE: u8 = 2;
pub const VOP_LIST: u8 = 3;
/// Every value of each stamped domain deleted (release: the person's domains, VAULT-OUT.md). Name and rest empty. The value: how many values went, or ERR.
pub const VOP_DROP: u8 = 4;
/// The person's own entries as one VSF document (`SYS_UNLOCK` op 5, the departure's "save first"): name empty; rest [offset u64][length u32][per domain, in the stamp's order: its scope's prefix length u8, the prefix]. The value: the document's whole length, or ERR; the bytes from the offset, at most the length.
pub const VOP_EXPORT: u8 = 5;

/// `SYS_UNLOCK` op 5 while the vault process serves, for a caller holding the Unlock right: its endpoint.
fn export_route(p: &Process, f: &Frame) -> Option<usize> {
    let ep = vault_ep()?;
    let token = p.read(f.x[0], 32);
    matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Unlock, r)) if r & RIGHT_SEND != 0).then_some(ep)
}

/// `SYS_UNLOCK` op 5 forwarded to the vault process as a call from the caller, who waits as for a store call: stamped with the person's domain in every scope that holds their names, less those the identity service keeps to itself (kvault `EXPORT_SKIP`, as the kernel's own export). The vault builds the document and hands it out from the offset (x4), one transfer at most (x3, the caller's length). ERR while the person is locked.
fn export_forward(f: &mut Frame, cur: usize, ep: usize) -> u64 {
    // WHY: the length is the caller's, and the vault answers one transfer at most (SYS_REPLY reads no more): a larger buffer takes its first transfer, the caller asks again from the next offset (ferros_sys `export_identity`).
    let len = (f.x[3] as usize).min(STORE_MAX);
    let scopes: Vec<&str> = STORES.iter().map(|s| s.prefix).filter(|p| crate::kvault::identity_within(p) && !crate::kvault::EXPORT_SKIP.contains(p)).collect();
    let domains: Vec<(&[u8], bool)> = scopes.iter().map(|&p| (p.as_bytes(), true)).collect();
    let mut rest = Vec::with_capacity(12 + scopes.iter().map(|p| 1 + p.len()).sum::<usize>());
    rest.extend_from_slice(&f.x[4].to_le_bytes());
    rest.extend_from_slice(&(len as u32).to_le_bytes());
    for p in &scopes {
        rest.push(p.len() as u8);
        rest.extend_from_slice(p.as_bytes());
    }
    let Some(msg) = stamp_domains(VOP_EXPORT, &domains, &[], &rest) else {
        f.x[0] = ERR;
        return 0;
    };
    call_endpoint(f, cur, ep, msg, f.x[2], len as u64 | STORE_REPLY)
}

/// A stamped request (`store_forward`'s message layout): `prefix`'s domain in each class of `classes`, then `name` and `rest`. A listing leaves out the person's domain while they are locked; anything else needing a locked class is None, as the kernel's vault refuses it.
fn stamp(op: u8, prefix: &[u8], classes: &[bool], name: &[u8], rest: &[u8]) -> Option<Vec<u8>> {
    let domains: Vec<(&[u8], bool)> = classes.iter().map(|&c| (prefix, c)).collect();
    stamp_domains(op, &domains, name, rest)
}

/// A stamped request over domains of several scopes (an export: the person's domain in each scope that holds their names), each a (scope prefix, the person's?) pair; otherwise as `stamp`.
fn stamp_domains(op: u8, domains: &[(&[u8], bool)], name: &[u8], rest: &[u8]) -> Option<Vec<u8>> {
    let mut msg = Vec::with_capacity(8 + domains.len() * 65 + 1 + name.len() + rest.len());
    msg.push(op);
    msg.push(MIGRATE_PENDING.load(core::sync::atomic::Ordering::Acquire) as u8);
    msg.extend_from_slice(&LOCK_EPOCH.load(core::sync::atomic::Ordering::Acquire).to_le_bytes());
    let at_count = msg.len();
    msg.push(0);
    for &(prefix, identity) in domains {
        let Some(mut keys) = domain_keys(prefix, identity) else {
            if op == VOP_LIST && identity {
                continue;
            }
            msg.fill(0);
            return None;
        };
        msg.push(identity as u8);
        msg.extend_from_slice(&keys);
        keys.fill(0);
        msg[at_count] += 1;
    }
    msg.push(name.len() as u8);
    msg.extend_from_slice(name);
    msg.extend_from_slice(rest);
    Some(msg)
}

/// The caller the kernel is on the vault endpoint (VAULT-OUT.md interface 5, the kernel's own channel): no thread behind it. Its call is queued and delivered as any other; the vault's reply lands in `KERNEL_CALL` for the service loop (the bridge's `store` commands, release).
const KERNEL_CALLER: usize = usize::MAX;
/// The kernel's call: in flight (queued, or delivered and not yet answered), and its answer once it came, Ok(the vault's reply: [the value u64 LE][the bytes]) or Err (the vault ended without answering). One at a time.
static mut KERNEL_CALL: (bool, Option<Result<Vec<u8>, ()>>) = (false, None);

fn kernel_call() -> &'static mut (bool, Option<Result<Vec<u8>, ()>>) {
    // SAFETY: only under the kernel lock (the syscalls that answer it, and `vault_call`/`vault_answer`, which take it).
    unsafe { &mut *(&raw mut KERNEL_CALL) }
}

/// The vault answered the kernel's call, or ended without answering: kept for the service loop, woken by the event in case it sleeps.
fn kernel_answered(r: Result<Vec<u8>, ()>) {
    let c = kernel_call();
    if c.0 {
        c.1 = Some(r);
    }
    unsafe { core::arch::asm!("sev") };
}

/// The kernel's own call to the vault process: `msg`, a stamp (`vault_stamp`, `vault_stamp_list`). False when no vault process serves or a call is already in flight; the message is wiped then.
pub fn vault_call(mut msg: Vec<u8>) -> bool {
    with_lock(|| {
        let c = kernel_call();
        let Some(ep) = vault_ep().filter(|_| !c.0) else {
            msg.fill(0);
            return false;
        };
        *c = (true, None);
        tr(TR_CALL, KERNEL_CALLER, ep as u64);
        k().endpoints[ep].queue.push_back((KERNEL_CALLER, msg));
        wake_server(ep);
        true
    })
}

/// The kernel's call's answer once it came: Ok((the value, the bytes)) or Err (the vault ended without answering, or answered short). None while it waits, or when none was made.
pub fn vault_answer() -> Option<Result<(u64, Vec<u8>), ()>> {
    with_lock(|| {
        let c = kernel_call();
        let r = c.1.take()?;
        c.0 = false;
        Some(r.and_then(|m| match m.get(..8) {
            Some(v) => Ok((u64::from_le_bytes(v.try_into().unwrap()), m[8..].to_vec())),
            None => Err(()),
        }))
    })
}

/// Whether a vault process serves the `vault` endpoint (alive, opening or receiving): the kernel's own channel goes there, not to the kernel's vault.
pub fn vault_serving() -> bool {
    with_lock(|| vault_ep().is_some())
}

/// The scope a full name belongs to in the vault process, and the name within it, if it is one the vault takes (a scope's, not one the kernel keeps, a name of printable ASCII up to 96 bytes, as `store_forward` refuses the rest).
pub fn vault_name(full: &str) -> Option<(&'static StoreScope, &[u8])> {
    let sc = scope_of(full)?;
    let rel = &full.as_bytes()[sc.prefix.len()..];
    (!rel.is_empty() && rel.len() <= 96 && rel.iter().all(|&b| (0x21..0x7F).contains(&b))).then_some((sc, rel))
}

/// A stamp for the kernel's own get, set or delete of the full name `full` (`vault_name`): its scope's domain in the name's class. None when the vault does not take the name, or the class is locked.
pub fn vault_stamp(op: u8, full: &str, rest: &[u8]) -> Option<Vec<u8>> {
    let (sc, rel) = vault_name(full)?;
    // Under the kernel lock: the person's keys come from the vault's unlock state, which an unlock or a lock on another core changes.
    with_lock_held_or(|| stamp(op, sc.prefix.as_bytes(), &[crate::kvault::identity_class(full)], rel, rest))
}

/// A stamp for the kernel's own call on an entry of the scope named `scope` that is named by a 32-byte address (an installed kist's capsule, by its blake3): the scope's domain in its class. None for no such scope, or a locked class.
pub fn vault_stamp_addr(op: u8, scope: &str, addr: &[u8; 32], rest: &[u8]) -> Option<Vec<u8>> {
    let sc = STORES.iter().find(|s| s.name == scope)?;
    let mut name = Vec::with_capacity(33);
    name.push(1);
    name.extend_from_slice(addr);
    with_lock_held_or(|| stamp(op, sc.prefix.as_bytes(), &[crate::kvault::identity_class(sc.prefix)], &name, rest))
}

/// A stamp for the kernel's own listing of scope `i` of `STORES` from `offset`, at most `len` bytes (names one per line): every class the scope holds, the person's left out while locked. With its prefix, to make the names whole.
pub fn vault_stamp_list(i: usize, offset: u64, len: u32) -> Option<(Vec<u8>, &'static str)> {
    let sc = STORES.get(i)?;
    let classes: &[bool] = if crate::kvault::identity_within(sc.prefix) { &[false, true] } else { &[false] };
    let mut rest = Vec::with_capacity(13);
    rest.extend_from_slice(&offset.to_le_bytes());
    rest.extend_from_slice(&len.to_le_bytes());
    rest.push(0);
    Some((with_lock_held_or(|| stamp(VOP_LIST, sc.prefix.as_bytes(), classes, &[], &rest))?, sc.prefix))
}

/// Stamps dropping every one of the person's domains in the vault process (each scope that holds the person's names), for release: made while their keys still exist, sent by the service loop before its restart (service.rs). Empty while they are locked: nothing could be stamped, and what is on the disk is already unopenable once release renews the device-only secret.
pub fn vault_stamp_drops() -> Vec<Vec<u8>> {
    with_lock_held_or(|| STORES.iter().filter(|sc| crate::kvault::identity_within(sc.prefix)).filter_map(|sc| stamp(VOP_DROP, sc.prefix.as_bytes(), &[true], &[], &[])).collect())
}

/// A server receiving on `ep` takes the head of its queue now; a polling server asleep between polls is woken (its sleep returns early, which a poll loop reads as "look again").
fn wake_server(ep: usize) {
    let kk = k();
    if let Some(server) = kk.threads.iter().position(|q| matches!(q, Some(q) if matches!(q.state, State::Receiving(e, ..) if e == ep))) {
        deliver(server, ep);
    } else if let Some(s) = kk.endpoints[ep].poller {
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
}

const ENTROPY_CONTEXT: &str = "ferros husky 2026-10-15 entropy syscall v1";
static ENTROPY_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A named block of device registers a spawn can grant (`bridge spawn --hw NAME`). The spawner's authority stands in for a hardware-granting capability until there is one. `via_el3`: only the EL3 monitor can reach it (husky's PMU), so accesses go through SMC_CMD_PRIV_REG and only whitelisted offsets answer.
pub struct Hw {
    pub name: &'static str,
    pub base: usize,
    pub size: usize,
    pub via_el3: bool,
}

/// The partitions a process may read files from (`Grant::Part`), read only (partfs.rs `part_call`): the modem driver's firmware image, its per-unit NV (efs, modem_userdata), the handover signature (persist) and the carrier configs (vendor_a, inside super). The owner's rule (2026-10-29): ferros reads Android's partitions, never writes them.
pub static PARTS: &[&str] = &["modem_a", "efs", "persist", "modem_userdata", "vendor_a", "vendor_b", "modem_b"];

/// The dev hardware registry: blocks a process may be granted by name (WIFI.md §0: PCIe ch1 sits in HSI2).
pub static HARDWARE: &[Hw] = &[
    // The cellular modem on PCIe ch0, in HSI1 (docs/oracle/MODEM-BOOT-RECON.md): the controller's blocks, the CP's config space and its BAR window (the doorbell), and HSI1's clocks, system registers, GPIO, SysMMU and S2MPU; the alive GPIO bank its reset lines share.
    Hw { name: "pcie0-elbi", base: 0x1210_0000, size: 0x2000, via_el3: false },
    Hw { name: "pcie0-soc", base: 0x1211_0000, size: 0x5000, via_el3: false },
    Hw { name: "pcie0-pcs", base: 0x1212_0000, size: 0x5000, via_el3: false },
    Hw { name: "pcie0-udbg", base: 0x1213_0000, size: 0xD000, via_el3: false },
    Hw { name: "pcie0-phy", base: 0x1214_0000, size: 0x3000, via_el3: false },
    Hw { name: "pcie0-dbi", base: 0x1240_0000, size: 0x38_1000, via_el3: false },
    Hw { name: "pcie0-cfg", base: 0x40FF_E000, size: 0x2000, via_el3: false },
    Hw { name: "pcie0-mem", base: 0x4000_0000, size: 0xFF_0000, via_el3: false },
    Hw { name: "cmu-hsi1", base: 0x1200_0000, size: 0x8000, via_el3: false },
    Hw { name: "hsi1-sysreg", base: 0x1202_0000, size: 0x2000, via_el3: false },
    Hw { name: "gpio-hsi1", base: 0x1204_0000, size: 0x1000, via_el3: false },
    Hw { name: "sysmmu-hsi1", base: 0x1206_0000, size: 0x1_0000, via_el3: false },
    Hw { name: "s2mpu-hsi1", base: 0x1209_0000, size: 0x1_0000, via_el3: false },
    Hw { name: "gpio-alive", base: 0x154D_0000, size: 0x1000, via_el3: false },
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
    // The AoC's three mailbox blocks (aoc.rs MBOX_BLOCKS): the sensors process signals the AoC on a service's mailbox after writing to its ring (INTGR0 at +G#20 of each).
    Hw { name: "aoc-mbox", base: 0x1520_0000, size: 0x4_1000, via_el3: false },
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
    // Its clocks, rails and reset are not here on purpose: they are the `touch` power switch (`POWERS`, touch.rs `power`: ACPM and the HSI0 CMU).
    Hw { name: "usi2-spi", base: 0x111D_0000, size: 0x1000, via_el3: false },
    Hw { name: "gpio-gpn0", base: 0x1506_0000, size: 0x1000, via_el3: false },
    // The GPS chip (GPS.md): HSI0 USI3's SPI port (spi_21). Its lines are in banks already above: HOST_REQ in gpio-far-alive (gpa5), MCU_REQ in gpio-gpp0 (gpp4, +G#80), MCU_RESP and nSTANDBY in gpio-hsi2 (gph3). Rails, clocks and pin modes are the kernel's (`POWERS`).
    Hw { name: "usi3-spi", base: 0x111E_0000, size: 0x1000, via_el3: false },
    // The NFC controller (docs/NFC.md): hsi2c_0, the bus it shares with the vibration motor (whose driver is the kernel's diag, never running beside the `nfc` process). Its interrupt line gpa7-6 is read in gpio-alive, above. Pin modes, the reset and the clock pad are the kernel's (`POWERS`).
    Hw { name: "hsi2c0", base: 0x10C8_0000, size: 0x1000, via_el3: false },
    // Bluetooth (docs/BT.md): the APM's USI1 UART (serial_18). The USI mode word and the chip's power lines are the kernel's (`POWERS`).
    Hw { name: "apm-usi1-uart", base: 0x155D_0000, size: 0x1000, via_el3: false },
];

/// A device's power switch a spawn can grant (`bridge spawn --power NAME`): the kernel's sequence for the rails, clocks and pin modes behind a userspace driver.
pub struct PowerSwitch {
    pub name: &'static str,
    pub set: fn(bool) -> bool,
}
pub static POWERS: &[PowerSwitch] = &[PowerSwitch { name: "gps", set: crate::gps::power }, PowerSwitch { name: "nfc", set: crate::nfc::power }, PowerSwitch { name: "bt", set: crate::bt::power }, PowerSwitch { name: "usb", set: crate::ptlink::power }, PowerSwitch { name: "touch", set: crate::touch::power }, PowerSwitch { name: "sensors", set: crate::sensorlink::power }, PowerSwitch { name: "dpu", set: crate::dpu_power::power }, PowerSwitch { name: "panel", set: crate::panel_power::power }, PowerSwitch { name: "panel-vddd", set: crate::panel_power::vddd }, PowerSwitch { name: "android", set: crate::service::android_request }];
/// A syscall buffer that resolved nowhere (`Process::span`): whose (the process's epoch), the offset as given, its length, and whether the kernel was to write it. The syscall that touched it traps at its end (`buf_trap`). Under the kernel lock: one syscall at a time.
#[derive(Clone, Copy)]
struct BufFault {
    epoch: u64,
    off: u64,
    len: usize,
    write: bool,
}
static mut BUF_FAULT: Option<BufFault> = None;
/// The exit value of a process killed for a syscall buffer (production): FAULT with the exception class G#3F, which no hardware fault uses, and the syscall's number.
const BAD_BUFFER: u64 = FAULT | 0x3F << 40;

/// Where an absolute address lies, in words, against the caller's arc and every DMA region: inside one (held or not), or how far before or past the nearest.
fn nearest_region(p: &Process, addr: usize) -> alloc::string::String {
    let mut best: Option<(usize, alloc::string::String)> = None;
    let mut consider = |start: usize, size: usize, name: &str, held: bool| {
        let (d, s) = if addr >= start && addr < start + size {
            (0, alloc::format!("inside {name}{}", if held { "" } else { " (not held)" }))
        } else if addr >= start + size {
            (addr - (start + size), alloc::format!("G#{:X} bytes past {name}", addr - (start + size)))
        } else {
            (start - addr, alloc::format!("G#{:X} bytes before {name}", start - addr))
        };
        if best.as_ref().is_none_or(|b| d < b.0) {
            best = Some((d, s));
        }
    };
    consider(p.arc.base, p.arc.size, "its arc", true);
    for (i, r) in DMA.iter().enumerate() {
        if r.size > 0 {
            consider(r.base, r.size, r.name, p.cspace.iter().any(|c| c.object == Object::Dma(i)));
        }
    }
    best.map(|b| b.1).unwrap_or_default()
}

/// A syscall named a buffer that resolved nowhere (owner, 2026-11-08: "run it in debug/trap mode"): the report (process and thread, syscall and op, the offset, address and length, the nearest region, PC and LR at the svc) to the console and the flash log; then a development build parks the calling thread for good with its memory intact (to read over the bridge; `diag kill` ends it), and a production build ends the process, one shot. Returns the trap's value as the dispatch does.
#[allow(clippy::too_many_arguments)]
fn buf_trap(f: &mut Frame, cur: usize, pi: usize, r: u64, b: BufFault, nr: u64, op: u64, pc: u64, lr: u64) -> u64 {
    let kk = k();
    let Some(p) = kk.procs.get(pi).and_then(|q| q.as_ref()) else { return r };
    let base = p.arc.base as u64;
    let line = alloc::format!(
        "{}: TRAP syscall {} op G#{:X}: a buffer it {} {}: offset G#{:X} (address G#{:X}) length A#{}, {}; pc +G#{:X} lr +G#{:X} (thread {}){}\n",
        core::str::from_utf8(&thread_name(cur)).unwrap_or("?"),
        nr,
        op,
        if b.write { "was to be written into" } else { "named to be read" },
        if b.write && in_grants(b.off, b.len) { "in its grants page, read-only (the kernel's: tokens, preferences)" } else { "that is neither in its arc nor in a region it holds" },
        b.off,
        base.wrapping_add(b.off),
        b.len,
        nearest_region(p, base.wrapping_add(b.off) as usize),
        pc.wrapping_sub(base),
        lr.wrapping_sub(base),
        cur,
        if cfg!(ferros_production) { "; killed" } else { "; parked, memory intact (diag kill ends it)" },
    );
    kk.console.extend_from_slice(line.as_bytes());
    crate::platform::pstore(line.as_bytes());
    crate::flashlog::line(line.trim_end());
    let still_current = cur_get() == Some(cur);
    if cfg!(ferros_production) {
        end_process(pi, cur, BAD_BUFFER | (nr & 0xFF) << 16);
        if still_current {
            return if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 };
        }
        return r;
    }
    // Parked for good: out of every queue, asleep with no deadline, nothing to wake it.
    kk.runnable.retain(|&q| q != cur);
    runnable_sync();
    for ep in kk.endpoints.iter_mut() {
        ep.queue.retain(|(caller, _)| *caller != cur);
        if ep.poller == Some(cur) {
            ep.poller = None;
        }
    }
    if let Some(t) = th(cur) {
        if still_current {
            t.frame = *f;
        }
        t.state = State::Sleeping(u64::MAX);
    }
    if still_current {
        return if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 };
    }
    r
}

/// Who switched each power switch on (a process's epoch; 0 none), and the switches whose owner ended with them on, waiting for the kernel's core (`power_offs_apply`). The owner's rule (2026-11-08): a process's end is one shot dead with no cleanup of its own, and the kernel turns off what it left on. A switch switched on again by another process before that is its, and stays on. Under the kernel lock.
static mut POWER_OWNER: [u64; 16] = [0; 16];
static mut POWER_OFF_PENDING: u32 = 0;

/// Switch `i` set by a process: on, owned by its epoch; off, owned by no one. Not the `android` switch: a launch request, not a power.
fn power_owned(i: usize, epoch: u64) {
    if POWERS[i].name == "android" {
        return;
    }
    unsafe {
        POWER_OWNER[i] = epoch;
        POWER_OFF_PENDING &= !(1 << i);
    }
}

/// A process ended (exit, fault, kill): every switch it left on is to be turned off on the kernel's core.
fn power_orphaned(epoch: u64) {
    unsafe {
        for i in 0..POWERS.len() {
            if POWER_OWNER[i] == epoch && epoch != 0 {
                POWER_OWNER[i] = 0;
                POWER_OFF_PENDING |= 1 << i;
            }
        }
    }
}

/// The switches whose owners ended, turned off now. On the kernel's core only (every switch is set there, as `SYS_POWER` requires): the service loop each pass, and `diag restart` between its kill and its spawn, so the new process finds the switch off rather than turned off under it.
pub fn power_offs_apply() {
    if 1u16 << core() != kernel_mask() {
        return;
    }
    for i in 0..POWERS.len() {
        // Taken and switched under one hold of the lock: a process switching it on again meanwhile owns it, and it stays on.
        let done = with_lock(|| unsafe {
            if POWER_OFF_PENDING >> i & 1 == 0 {
                return None;
            }
            POWER_OFF_PENDING &= !(1 << i);
            Some((POWERS[i].set)(false))
        });
        if let Some(ok) = done {
            crate::flashlog::line(&alloc::format!("power {}: off, its process ended ({})", POWERS[i].name, if ok { "done" } else { "the switch refused" }));
        }
    }
}

pub const ERR: u64 = u64::MAX;
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
    let Some((caller, mut msg)) = kk.endpoints[ep].queue.pop_front() else { return };
    let Some(st) = th(server) else { return };
    let Some(sp) = proc_of(server) else { return };
    let State::Receiving(_, buf, cap, tok_off) = st.state else { return };
    let n = msg.len().min(cap as usize);
    sp.write(buf, &msg[..n]);
    // Wiped once delivered: a forwarded store call carries domain keys (`store_forward`), which must not outlive it in the kernel's heap.
    msg.fill(0);
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
    // The profiler's note of this trap (prof.rs): who called, from where, and the call (x8 for an `svc`, ESR class G#15; G#FFFF for anything else).
    crate::prof::enter(if f.esr >> 26 == 0x15 { f.x[8] } else { 0xFFFF }, f.elr, f.x[29], f.x[30]);
    lock();
    let r = el0_trap(f);
    unlock();
    crate::prof::leave();
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
    // What the trap report names, as the svc found them (a dispatch may switch the frame to another thread).
    let (at_op, at_pc, at_lr, at_epoch) = (f.x[1], f.elr, f.x[30], p.epoch);
    unsafe { *(&raw mut BUF_FAULT) = None };
    let r = match f.x[8] {
        SYS_EXIT => end(f, f.x[0]),
        SYS_THREAD_SPAWN => {
            let (entry, stack, arg) = (f.x[0], f.x[1], f.x[2]);
            let m = p.arc.mask() as u64;
            let mut frame = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0, v: [0; 32], fpcr: 0, fpsr: 0, tpidr: 0, _pad: 0 };
            frame.elr = p.arc.base as u64 + (entry & m);
            frame.sp_el0 = p.arc.base as u64 + (stack & m & !15);
            frame.x[28] = p.arc.base as u64;
            frame.x[27] = p.arc.x27() as u64;
            frame.x[26] = p.arc.base as u64;
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
                            // A wait on the world (x2 bit 0): the watchdog's clock paused while the thread that pets is in it.
                            if f.x[2] & 1 != 0 && cur == p.pet_thread {
                                p.paused_at = t_now;
                            }
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
                // A wait on the world (x3 bit 0): the watchdog's clock paused while the thread that pets is in it.
                if f.x[3] & 1 != 0 && cur == p.pet_thread {
                    p.paused_at = t_now;
                }
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
        SYS_BLOCK => {
            let token = p.read(f.x[0], 32);
            let window = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Blocks(i), r)) if r & RIGHT_SEND != 0 => WINDOWS.get(i),
                _ => None,
            };
            let (write, count, at, buf) = (f.x[1] & 1 != 0, (f.x[1] >> 8) & 0xFF, f.x[2], f.x[3]);
            f.x[0] = match (window, crate::ufs_device::part()) {
                // WHY the bounds: the block number and count are the process's; a window is all it may touch, and the partition must hold the window (the same refusal the kernel's own vault makes).
                (Some(w), Some((first, blocks))) if (1..=BLOCK_MAX).contains(&count) && at < w.blocks && count <= w.blocks - at && w.offset + w.blocks <= blocks as u64 => {
                    use core::sync::atomic::Ordering::Relaxed;
                    let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
                    let base = (first as u64 + w.offset + at) as u32;
                    let n = count as usize;
                    let run = (&raw mut UFS_RUN) as usize;
                    let bytes = unsafe { core::slice::from_raw_parts_mut(run as *mut u8, n << 12) };
                    let t0 = crate::platform::cntpct();
                    // The PRDT's stride, learned at the first access of more than one block (`probe_prdt_stride`, the run buffer its scratch): until it is known a run goes a block a command.
                    if n > 1 && ferros_hal::ufs::PRDT_STRIDE.load(Relaxed) == 0 {
                        u.probe_prdt_stride(0, base, run);
                    }
                    let ok = if write {
                        p.read_into(buf, bytes);
                        u.write_run_lun(0, base, n, run)
                    } else {
                        let ok = u.read_run_lun(0, base, n, run);
                        if ok {
                            p.write_from(buf, bytes);
                        }
                        ok
                    };
                    let s = if write { 3 } else { 0 };
                    BLOCK_STATS[s].fetch_add(1, Relaxed);
                    BLOCK_STATS[s + 1].fetch_add(count, Relaxed);
                    BLOCK_STATS[s + 2].fetch_add(crate::platform::cntpct() - t0, Relaxed);
                    if ok { 0 } else { ERR }
                }
                _ => ERR,
            };
            0
        }
        // The vault process serving: the call goes to it, stamped (VAULT-OUT.md, step 3).
        SYS_VAULT => {
            let token = p.read(f.x[0], 32);
            let ok = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Vault, r)) if r & RIGHT_SEND != 0);
            let (op, len, offset) = (f.x[1] & 0xFF, ((f.x[1] >> 8) & 0xFF_FFFF) as usize, (f.x[1] >> 32) as usize);
            // WHY: the length and the name are the vault process's: one transfer at most, a name as long as a scope's prefix and the longest name the kernel's path accepts.
            let name = || alloc::string::String::from_utf8(p.read(f.x[2], (f.x[3] as usize).min(1 << 7))).unwrap_or_default();
            f.x[0] = if !ok || len > STORE_MAX {
                ERR
            } else {
                match op {
                    0 => migrate_next(p, f.x[2] as usize, f.x[4], len),
                    1 => migrate_read(p, &name(), offset, f.x[4], len),
                    2 => migrate_done(&name()),
                    _ => ERR,
                }
            };
            0
        }
        SYS_STORE if store_route(p, f).is_some() => {
            let (ep, sc) = store_route(p, f).unwrap();
            store_forward(f, cur, ep, sc)
        }
        SYS_STORE => {
            let token = p.read(f.x[0], 32);
            let scope = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Store(i), r)) if r & RIGHT_SEND != 0 => STORES.get(i),
                _ => None,
            };
            // x1: bit 0 set; bit 1 an offset (bits 32-63) into a value larger than one transfer — a read of that piece, or with bit 0 a write of it, staged until the piece without bit 2 (more follow) commits the whole; bit 3 delete; bit 4 list the scope's entries, read like a large value (names relative to the scope one per line, or with bit 5 the addresses, 32 bytes each); bit 5 the entry is named by a 32-byte address at x2 rather than a name. The length in bits 8-31.
            let (set, more, del, list, raw) = (f.x[1] & 1 != 0, f.x[1] & 4 != 0, f.x[1] & 8 != 0, f.x[1] & 16 != 0, f.x[1] & 32 != 0);
            let at = if f.x[1] & 2 != 0 { Some((f.x[1] >> 32) as usize) } else { None };
            let len = ((f.x[1] >> 8) & 0xFF_FFFF) as usize;
            let rel = p.read(f.x[2], (f.x[3] as usize).min(97));
            let rel_ok = if raw { rel.len() == 32 || list } else { rel.len() <= 96 && rel.iter().all(|&b| (0x21..0x7F).contains(&b)) && (!rel.is_empty() || list) };
            f.x[0] = match (scope, unsafe { STORE }, unsafe { STORE_ADDR }) {
                (Some(sc), Some(store), Some(store_addr)) if rel_ok && len <= STORE_MAX => {
                    // The entry's name for the kernel's own bookkeeping: prefix + name, or prefix + 1 + address.
                    let mut key: Vec<u8> = sc.prefix.as_bytes().to_vec();
                    if raw {
                        key.push(1);
                    }
                    key.extend_from_slice(&rel);
                    let addr: [u8; 32] = if raw && rel.len() == 32 { rel[..].try_into().unwrap() } else { [0; 32] };
                    let name = alloc::string::String::from_utf8(key.clone()).unwrap_or_default();
                    let fetch = |op: AddrOp| -> Result<Option<Vec<u8>>, ()> { if raw { store_addr(sc.prefix, &addr, op) } else { match op { AddrOp::Get => store(&name, None), AddrOp::Set(v) => store(&name, Some(v)), _ => Err(()) } } };
                    let cache = unsafe { &mut *(&raw mut STORE_CACHE) };
                    // A listing or a large value is served piecewise from here: fetched once, kept until its last piece is read.
                    let serve = |cache: &mut Option<(Vec<u8>, Vec<u8>)>, p: &Process, at: usize, absent: u64| -> u64 {
                        match cache {
                            Some((_, v)) => {
                                let piece = &v[at.min(v.len())..(at + len).min(v.len())];
                                p.write(f.x[4], piece);
                                let total = v.len();
                                if at + len >= total {
                                    *cache = None;
                                }
                                total as u64
                            }
                            None => absent,
                        }
                    };
                    if list {
                        let mut lk = sc.prefix.as_bytes().to_vec();
                        lk.push(if raw { 2 } else { b'\n' });
                        if cache.as_ref().map(|(k, _)| k != &lk).unwrap_or(true) {
                            *cache = if raw {
                                store_addr(sc.prefix, &addr, AddrOp::List).ok().flatten().map(|v| (lk, v))
                            } else {
                                match unsafe { STORE_KEYS }.map(|keys| keys()) {
                                    Some(Ok(names)) => {
                                        let mut out = alloc::string::String::new();
                                        for n in names.iter().filter_map(|n| n.strip_prefix(sc.prefix)) {
                                            out.push_str(n);
                                            out.push('\n');
                                        }
                                        Some((lk, out.into_bytes()))
                                    }
                                    _ => None,
                                }
                            };
                        }
                        serve(cache, p, at.unwrap_or(0), ERR)
                    } else if del {
                        *cache = None;
                        let r = if raw { store_addr(sc.prefix, &addr, AddrOp::Remove).map(|v| v.is_some()) } else { unsafe { STORE_REMOVE }.map(|rm| rm(&name)).unwrap_or(Err(())) };
                        match r {
                            Ok(true) => 0,
                            Ok(false) => NOTHING,
                            Err(()) => ERR,
                        }
                    } else if set {
                        *cache = None;
                        let piece = p.read(f.x[4], len);
                        // A value past the vault's limit (service.rs STORE_VALUE_MAX: the seal and the read-back each need about twice it in one piece of the kernel heap) is refused, its stage dropped, before it grows the heap.
                        if at.unwrap_or(0) + piece.len() > crate::service::STORE_VALUE_MAX {
                            unsafe { *(&raw mut STORE_STAGE) = None };
                            ERR
                        } else if let Some(at) = at {
                            let stage = unsafe { &mut *(&raw mut STORE_STAGE) };
                            if stage.as_ref().map(|(k, _)| k != &key).unwrap_or(true) {
                                *stage = Some((key.clone(), Vec::new()));
                            }
                            let buf = &mut stage.as_mut().unwrap().1;
                            if buf.len() < at + piece.len() {
                                buf.resize(at + piece.len(), 0);
                            }
                            buf[at..at + piece.len()].copy_from_slice(&piece);
                            if more {
                                0
                            } else {
                                let (_, whole) = stage.take().unwrap();
                                if fetch(AddrOp::Set(&whole)).is_ok() { 0 } else { ERR }
                            }
                        } else if fetch(AddrOp::Set(&piece)).is_ok() {
                            0
                        } else {
                            ERR
                        }
                    } else if let Some(at) = at {
                        if cache.as_ref().map(|(k, _)| k != &key).unwrap_or(true) {
                            *cache = match fetch(AddrOp::Get) {
                                Ok(Some(v)) => Some((key.clone(), v)),
                                _ => None,
                            };
                        }
                        serve(cache, p, at, NOTHING)
                    } else {
                        match fetch(AddrOp::Get) {
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
        SYS_POWER => {
            let token = p.read(f.x[0], 32);
            let found = p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights));
            f.x[0] = match found {
                Some((Object::Power(i), r)) if r & RIGHT_SEND != 0 && 1u16 << core() == kernel_mask() => {
                    let on = f.x[1] == 1;
                    if (POWERS[i].set)(on) {
                        power_owned(i, if on { p.epoch } else { 0 });
                        0
                    } else {
                        ERR
                    }
                }
                _ => {
                    // A refusal at the door says which check (2026-11-04: the display's `panel-vddd` was refused after jumps with no word from the switch itself).
                    let why = match found {
                        Some((Object::Power(i), r)) if r & RIGHT_SEND == 0 => alloc::format!("{}: no send right", POWERS[i].name),
                        Some((Object::Power(i), _)) => alloc::format!("{}: called on core {}, not the kernel's", POWERS[i].name, core()),
                        Some(_) => alloc::string::String::from("the token names no power switch"),
                        None => alloc::string::String::from("no such token"),
                    };
                    crate::flashlog::line(&alloc::format!("SYS_POWER refused: {why}"));
                    ERR
                }
            };
            0
        }
        // The person's export while the vault process serves: it builds the document (VAULT-OUT.md).
        SYS_UNLOCK if f.x[1] == 5 && export_route(p, f).is_some() => {
            let ep = export_route(p, f).unwrap();
            export_forward(f, cur, ep)
        }
        SYS_UNLOCK => {
            let token = p.read(f.x[0], 32);
            let op = f.x[1];
            f.x[0] = match (p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), unsafe { UNLOCK }) {
                (Some((Object::Unlock, r)), Some(unlock)) if r & RIGHT_SEND != 0 => match op {
                    0 => unlock(0, None).unwrap_or(ERR),
                    1 => {
                        let mut seed = [0u8; 32];
                        seed.copy_from_slice(&p.read(f.x[2], 32));
                        let r = unlock(1, Some(&seed));
                        seed.fill(0);
                        r.unwrap_or(ERR)
                    }
                    2..=4 => unlock(op as u8, None).unwrap_or(ERR),
                    // Op 5: the identity's own entries as one VSF document into the caller's buffer (x2, x3 its length) from offset x4, for saving them to the fleet before a departure; the document's whole length, or ERR while locked. With a vault process serving, it answers instead (`export_forward`).
                    5 => {
                        // Built at offset 0 and kept until read to its end, so the pieces of one read are one document (each build stamps its own time and provenance); one transfer a call, as the vault process answers.
                        let kept = unsafe { &mut *(&raw mut EXPORT_DOC) };
                        if f.x[4] == 0 {
                            *kept = unsafe { EXPORT }.and_then(|e| e());
                        }
                        match kept.as_mut() {
                            Some(doc) => {
                                // WHY: the offset and the length are the caller's; past the end there is nothing to copy (slicing past it would panic the kernel), and one transfer at most goes back.
                                let from = (f.x[4] as usize).min(doc.len());
                                let to = from + (f.x[3] as usize).min(STORE_MAX).min(doc.len() - from);
                                p.write(f.x[2], &doc[from..to]);
                                let whole = doc.len() as u64;
                                if to == doc.len() {
                                    doc.fill(0);
                                    *kept = None;
                                }
                                whole
                            }
                            None => ERR,
                        }
                    }
                    _ => ERR,
                },
                _ => ERR,
            };
            0
        }
        SYS_PT => {
            let token = p.read(f.x[0], 32);
            let (op, at, len) = (f.x[1], f.x[2], f.x[3] as usize);
            // The buffer: in the arc (an offset), or in a DMA region the process holds (DMA_ADDR | address); refused when it is neither.
            let dma = if at & DMA_ADDR != 0 { Some(p.dma_span(at, len)) } else { None };
            f.x[0] = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Pt, r)) if r & RIGHT_SEND != 0 && dma != Some(None) => match op {
                    0 | 3 => {
                        let fill = |dst: &mut [u8]| match dma {
                            Some(Some(a)) => unsafe { core::ptr::copy_nonoverlapping(a as *const u8, dst.as_mut_ptr(), dst.len()) },
                            _ => p.read_into(at, dst),
                        };
                        if crate::ptlink::submit(len, op == 3, fill) { 0 } else { ERR }
                    }
                    // A transfer is moving: the clock policy's activity (ptlink.rs MOVING_AT).
                    2 => {
                        let t: u64;
                        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
                        crate::ptlink::MOVING_AT.store(t, core::sync::atomic::Ordering::Relaxed);
                        0
                    }
                    1 => {
                        let put = |piece: &[u8]| match dma {
                            Some(Some(a)) => unsafe { core::ptr::copy_nonoverlapping(piece.as_ptr(), a as *mut u8, piece.len()) },
                            _ => p.write(at, piece),
                        };
                        match crate::ptlink::take(f.x[4] as usize, len, put) {
                            Some((total, replied, act)) => total as u64 | (act as u64) << 32 | (!replied as u64) << 33,
                            None if crate::ptlink::busy() => NOTHING,
                            None => ERR,
                        }
                    }
                    _ => ERR,
                },
                _ => ERR,
            };
            0
        }
        SYS_PET => {
            let now = crate::platform::cntpct();
            if p.last_pet == 0 {
                // The first pet ends the start, which is not counted.
                p.start = now - p.born;
            } else {
                // Since the last pet, less the time this thread was paused (waiting on the world, or on another process).
                let gap = now - p.last_pet - p.paused;
                if gap > p.gap_max {
                    p.gap_max = gap;
                    p.gap_at = now;
                }
            }
            p.paused = 0;
            p.paused_at = 0;
            p.last_pet = now;
            p.pet_thread = cur;
            p.pets += 1;
            f.x[0] = 0;
            0
        }
        SYS_STATUS => {
            let slot = [b"modem" as &[u8], b"pcie1", b"bt", b"nfc", b"gps", b"usb"].iter().position(|n| *n == p.name.as_slice());
            f.x[0] = match slot {
                // The slot comes from the process: bounded so a bad one is refused, not a write past the radio's eight.
                Some(i) if f.x[0] < 8 => {
                    STATUS[i][f.x[0] as usize].store(f.x[1], core::sync::atomic::Ordering::Relaxed);
                    0
                }
                // Slots 8 and up: the link's tracks, bounded as the measures are.
                Some(i) if f.x[0] < (8 + TRACKS) as u64 => {
                    TRACK[i][f.x[0] as usize - 8].store(f.x[1], core::sync::atomic::Ordering::Relaxed);
                    0
                }
                Some(_) => ERR,
                None => ERR,
            };
            0
        }
        SYS_TRACKS => {
            // WHY the bounds: the link and the count are the caller's; there are six links and TRACKS words each.
            f.x[0] = if f.x[0] < 6 {
                let n = (f.x[2] as usize).min(TRACKS);
                let mut bytes = [0u8; TRACKS * 8];
                for k in 0..n {
                    bytes[k * 8..k * 8 + 8].copy_from_slice(&TRACK[f.x[0] as usize][k].load(core::sync::atomic::Ordering::Relaxed).to_le_bytes());
                }
                p.write(f.x[1], &bytes[..n * 8]);
                n as u64
            } else {
                ERR
            };
            0
        }
        SYS_CONTROL => {
            let token = p.read(f.x[0], 32);
            let held = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0);
            let (link, slot, word) = (f.x[1] as usize, f.x[2] as usize, f.x[3]);
            use core::sync::atomic::Ordering::Relaxed;
            // WHY the bounds: the link and the slot are the caller's, indexes into the blocks; slot 0 is the generation, never set from outside.
            f.x[0] = if !held || link >= CONTROL_LINKS || slot == 0 || slot >= CONTROLS {
                ERR
            } else if link == CONTROL_CLOCK && slot == CONTROL_ZONE {
                // The zone, seconds east: within a day of UTC, as the settings document takes it.
                let z = word as i64;
                let was = crate::rtc::TZ.load(Relaxed);
                if z.unsigned_abs() >= 86_400 {
                    ERR
                } else {
                    crate::rtc::set_tz_held(z);
                    if unsafe { CONTROL_STORE }.map(|st| st()).unwrap_or(false) {
                        CONTROL_GEN[link].fetch_add(1, Relaxed);
                        0
                    } else {
                        crate::rtc::set_tz_held(was);
                        ERR
                    }
                }
            } else {
                let was = CONTROL[link][slot].swap(word, Relaxed);
                let kept = crate::settings::KNOBS.iter().any(|k| k.1 == link && k.2 == slot);
                if kept && !unsafe { CONTROL_STORE }.map(|st| st()).unwrap_or(false) {
                    CONTROL[link][slot].store(was, Relaxed);
                    ERR
                } else {
                    CONTROL_GEN[link].fetch_add(1, Relaxed);
                    0
                }
            };
            0
        }
        SYS_CONTROLS => {
            // WHY the bounds: the link and the count are the caller's; there are CONTROL_LINKS blocks of CONTROLS words.
            f.x[0] = if (f.x[0] as usize) < CONTROL_LINKS {
                let link = f.x[0] as usize;
                let n = (f.x[2] as usize).min(CONTROLS);
                let mut bytes = [0u8; CONTROLS * 8];
                for k in 0..n {
                    use core::sync::atomic::Ordering::Relaxed;
                    let w = match k {
                        0 => CONTROL_GEN[link].load(Relaxed),
                        CONTROL_ZONE if link == CONTROL_CLOCK => crate::rtc::TZ.load(Relaxed) as u64,
                        _ => CONTROL[link][k].load(Relaxed),
                    };
                    bytes[k * 8..k * 8 + 8].copy_from_slice(&w.to_le_bytes());
                }
                p.write(f.x[1], &bytes[..n * 8]);
                n as u64
            } else {
                ERR
            };
            0
        }
        SYS_ZONE => {
            let named = |n: &[u8]| p.name.as_slice() == n;
            let settings = |at: u64| {
                let token = p.read(at, 32);
                matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0)
            };
            let keep = || unsafe { CONTROL_STORE }.map(|st| st()).unwrap_or(false);
            // An id named by the caller: WHY the bound, the length is the caller's and an id is at most zone::ID_MAX bytes; a longer one is refused, not cut.
            let id = |at: u64, len: u64| if len as usize <= crate::zone::ID_MAX { crate::zone::Id::new(&p.read(at, len as usize)) } else { None };
            f.x[0] = match f.x[0] {
                0 if named(b"modem") => {
                    let flags = f.x[3];
                    let dst = (flags & 2 != 0).then_some(flags & 4 != 0);
                    // WHY the bound: the offset is the modem's reading of the network; a legal offset lies within a day of UTC, anything else is not one and is dropped (the MCC still counts).
                    let offset = (flags & 1 != 0 && (f.x[2] as i64).unsigned_abs() < 86_400).then_some((f.x[2] as i64 as i32, dst));
                    // WHY the bound: an MCC is three decimal digits (ITU E.212); anything else names no country.
                    crate::zone::network(if f.x[1] < 1000 { f.x[1] as u16 } else { 0 }, offset);
                    0
                }
                1 if named(b"gps") => {
                    let (lat, lon) = (f.x[1] as i64, f.x[2] as i64);
                    // WHY the bounds: the driver's fix in 2^-23 degrees; a latitude past a quarter turn or a longitude past a half is no place, and the arithmetic after assumes them.
                    if lat.unsigned_abs() <= 90 << 23 && lon.unsigned_abs() <= 180 << 23 {
                        crate::zone::position(lat, lon);
                        0
                    } else {
                        ERR
                    }
                }
                2 if settings(f.x[1]) => match id(f.x[3], f.x[4]) {
                    Some(z) if crate::zone::set_device(crate::zone::Choice { auto: f.x[2] == 1, id: z }, keep) => 0,
                    _ => ERR,
                },
                3 if settings(f.x[1]) => {
                    if crate::zone::set_follow(f.x[2] == 1, keep) { 0 } else { ERR }
                }
                4 if named(b"fgtw") => {
                    let c = match (f.x[1], id(f.x[2], f.x[3])) {
                        (2, _) => Some(None),
                        (k @ (0 | 1), Some(z)) => Some(Some(crate::zone::Choice { auto: k == 1, id: z })),
                        _ => None,
                    };
                    match c {
                        Some(c) if crate::zone::set_identity(c, keep) => 0,
                        _ => ERR,
                    }
                }
                5 => {
                    let b = crate::zone::state_bytes();
                    // WHY the min: the buffer is the caller's and may be shorter than the state.
                    let n = b.len().min(f.x[2] as usize);
                    p.write(f.x[1], &b[..n]);
                    n as u64
                }
                6 => {
                    let v = crate::zone::list();
                    // WHY the min: the buffer is the caller's; the return says how long the whole list is.
                    let n = v.len().min(f.x[2] as usize);
                    p.write(f.x[1], &v[..n]);
                    v.len() as u64
                }
                _ => ERR,
            };
            0
        }
        SYS_LANG => {
            let settings = |at: u64| {
                let token = p.read(at, 32);
                matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0)
            };
            let keep = || unsafe { CONTROL_STORE }.map(|st| st()).unwrap_or(false);
            // WHY `valid`: the code is the caller's, and is kept in the vault and handed to every process as two bytes; one that is no packed three letters is refused, not cut.
            f.x[0] = match f.x[0] {
                0 if settings(f.x[1]) && crate::lang::valid(f.x[2]) => {
                    if crate::lang::set_device(f.x[2] as u16, keep) { 0 } else { ERR }
                }
                1 if settings(f.x[1]) => {
                    if crate::lang::set_own(f.x[2] == 1, keep) { 0 } else { ERR }
                }
                2 if p.name.as_slice() == b"fgtw" && crate::lang::valid(f.x[1]) => {
                    if crate::lang::set_identity(f.x[1] as u16, keep) { 0 } else { ERR }
                }
                _ => ERR,
            };
            0
        }
        SYS_DETAIL => {
            let own = LINK_NAMES.iter().position(|n| *n == p.name.as_slice());
            f.x[0] = match f.x[0] {
                // A driver's own document: its length bounded so it fits the link's buffer.
                0 => match own {
                    Some(i) if (f.x[2] as usize) <= DETAIL_MAX => {
                        let n = f.x[2] as usize;
                        let doc = p.read(f.x[1], n);
                        unsafe {
                            (&mut *(&raw mut DETAIL))[i][..n].copy_from_slice(&doc);
                            (*(&raw mut DETAIL_LEN))[i] = n;
                        }
                        0
                    }
                    _ => ERR,
                },
                1 => {
                    let token = p.read(f.x[1], 32);
                    let held = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0);
                    // WHY the bound: the link is the caller's, an index into six documents.
                    if held && (f.x[2] as usize) < LINK_NAMES.len() {
                        let i = f.x[2] as usize;
                        let n = unsafe { (*(&raw const DETAIL_LEN))[i] };
                        // WHY the min: the buffer is the caller's and may be shorter than the document; its length says how much to copy, the return how much there is.
                        let take = n.min(f.x[4] as usize);
                        let doc = unsafe { &(&*(&raw const DETAIL))[i][..take] };
                        p.write_from(f.x[3], doc);
                        n as u64
                    } else {
                        ERR
                    }
                }
                _ => ERR,
            };
            0
        }
        SYS_SENSORS => {
            let token = p.read(f.x[0], 32);
            let (op, off, len) = (f.x[1], f.x[2] as usize, f.x[3] as usize);
            f.x[0] = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                // The block must lie whole in the caller's arc, unwrapped: the kernel reads it in place.
                Some((Object::Sensors, r)) if r & RIGHT_SEND != 0 && op == 0 && len >= crate::sensorlink::BYTES && off.checked_add(len).is_some_and(|e| e <= p.arc.size) => {
                    crate::sensorlink::publish(p.epoch, p.arc.base + off);
                    0
                }
                Some((Object::Sensors, r)) if r & RIGHT_SEND != 0 && op == 1 => crate::sensorlink::CDT.load(core::sync::atomic::Ordering::Relaxed) as u64 | (crate::sensorlink::MLB.load(core::sync::atomic::Ordering::Relaxed) as u64) << 32,
                _ => ERR,
            };
            0
        }
        SYS_IRA => {
            let token = p.read(f.x[0], 32);
            let len = f.x[2] as usize;
            f.x[0] = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Ira, r)) if r & RIGHT_SEND != 0 && (1..=64).contains(&len) => {
                    // A kist's keys are its own (owner, 2026-11-08): derived under a context no system process's name reaches, then bound to the kist's gate identity (its domain name, one way from its ihi: the oracle over its developer key and option), so no kist can ask for a system service's key, nor another kist's. A system process's keys are derived as they always were, byte for byte.
                    let mut context = alloc::vec::Vec::with_capacity(32 + len);
                    context.extend_from_slice(if p.kist { b"ferros kist ira v1 ".as_slice() } else { b"ferros process ira v1 " });
                    context.extend_from_slice(&p.read(f.x[1], len));
                    // Derived where the ira is held (keys.rs): the ira itself never leaves the registers.
                    // Bit 0 of x4: the key worn by the person as well as the device (owner, 2026-11-09: "We mix the ira in with our identity. can't have same address devices on network"): bound to the unlocked identity's domain key, so it differs for every person on every device and exists only while someone is present (none while locked).
                    let person = f.x[4] & 1 != 0;
                    let key = core::str::from_utf8(&context).ok().and_then(crate::ira::derive).and_then(|k| match (p.kist, p.domain) {
                        (false, _) if person => crate::service::domain_base(true).map(|id| {
                            let mut h = blake3::Hasher::new_derive_key(PERSON_IRA_CONTEXT);
                            h.update(&id);
                            h.update(&k);
                            *h.finalize().as_bytes()
                        }),
                        (false, _) => Some(k),
                        // A kist asks for its own keys only.
                        (true, _) if person => None,
                        (true, Some(d)) => {
                            let mut h = blake3::Hasher::new_derive_key(KIST_IRA_CONTEXT);
                            h.update(&d);
                            h.update(&k);
                            Some(*h.finalize().as_bytes())
                        }
                        // A kist without a gate identity has no key of its own to have.
                        (true, None) => None,
                    });
                    match key {
                        Some(key) => {
                            p.write(f.x[3], &key);
                            32
                        }
                        None => ERR,
                    }
                }
                _ => ERR,
            };
            0
        }
        SYS_LIGHT => {
            let want = ((f.x[1] as usize).saturating_sub(16) / 4).min(crate::sensorlink::SCOPE_LEN);
            // The sensors process's ring (sensorlink.rs); with no process publishing, no samples (the kernel has no USF of its own since 2026-11-01).
            use crate::sensorlink::{word, SCOPE_FMT, SCOPE_MASK, SCOPE_N};
            let (end, rate, mask) = (word(SCOPE_N).unwrap_or(0), word(SCOPE_FMT).unwrap_or(0) as u32, word(SCOPE_MASK).unwrap_or(0) as u32);
            let n = want.min(end as usize);
            let mut b = alloc::vec::Vec::with_capacity(16 + 4 * n);
            b.extend_from_slice(&end.to_le_bytes());
            b.extend_from_slice(&rate.to_le_bytes());
            b.extend_from_slice(&mask.to_le_bytes());
            crate::sensorlink::scope_copy(end, n, &mut b);
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
            // x1 bit 8 (`PREF_LIVE`): applied, not kept — a slider under a finger (owner, 2026-11-08: the brightness slider wrote the settings document into the kernel's vault at every 32nd of a stop, a UFS commit under the kernel lock each, and the settings ran at 20 fps); the value is kept by the call without it, at the lift.
            let (index, value, live) = ((f.x[1] & 0xFF) as usize, f.x[2] as u8, f.x[1] & PREF_LIVE != 0);
            f.x[0] = if held && f.x[1] >> 9 == 0 && pref_valid(index, value) && (live || unsafe { PREF_STORE }.map(|st| st(index, value)).unwrap_or(false)) {
                set_pref_locked(index, value);
                0
            } else {
                ERR
            };
            0
        }
        // Install mode (x2 bit 3): a kist the installer stored, read, checked and sealed by the service loop (`install_request`).
        SYS_SPAWN if f.x[2] & 8 != 0 => {
            f.x[0] = install_request(p, pi, &p.read(f.x[0], (f.x[1] as usize).min(4096)));
            0
        }
        SYS_SPAWN if f.x[2] & 2 != 0 => {
            f.x[0] = kist_request(p, &p.read(f.x[0], (f.x[1] as usize).min(4096)), f.x[2] & 4 != 0);
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
            // Which of the request's tokens was refused first, for the console line (2026-11-05: init's 104th slot onward).
            let mut bad_at = usize::MAX;
            for j in 0..n {
                let tok = take(32);
                match p.lookup(&tok).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                    Some((Object::Reply(_), _)) | Some((Object::Console, _)) | None => {
                        bad = true;
                        bad_at = bad_at.min(j);
                    }
                    // The whole surfaces region passes as one surface of it, the child's own.
                    Some((Object::Dma(i), r)) if DMA[i].name == "surfaces" => objects.push((Object::NewSurface, r)),
                    Some(o) => objects.push(o),
                }
            }
            // x2 bit 0: only if no live process has this name — init's keepers start a driver again this way (NOTHING: it is running), never a second beside one that is alive but not serving yet.
            let shown = if name.is_empty() { &key } else { &name };
            if f.x[2] & 1 != 0 && k().procs.iter().flatten().any(|q| q.name == *shown && !q.dying) {
                f.x[0] = NOTHING;
            } else {
                let r = if bad { Err("spawn: a token the caller does not hold (or a reply/console)") } else { spawn_queued(shown, &key, &objects) };
                let kk = k();
                match r {
                    Ok(_) => f.x[0] = 0,
                    Err(e) => {
                        kk.console.extend_from_slice(b"spawn refused: ");
                        kk.console.extend_from_slice(e.as_bytes());
                        if bad {
                            kk.console.extend_from_slice(alloc::format!(" (the request's token {bad_at} of {n}; the caller holds {})", p.cspace.len()).as_bytes());
                        }
                        kk.console.push(b'\n');
                        f.x[0] = ERR;
                    }
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
                    let at = kk.console.len();
                    for &c in &msg {
                        if p.line_start {
                            kk.console.extend_from_slice(&p.name);
                            kk.console.extend_from_slice(b": ");
                        }
                        kk.console.push(c);
                        p.line_start = c == b'\n';
                    }
                    crate::platform::pstore(&kk.console[at..]);
                    let n = 2usize.min(f.x[4] as usize);
                    p.write(f.x[3], &b"ok"[..n]);
                    f.x[0] = n as u64;
                    0
                }
                // Files on a partition, read only (partfs.rs `part_call`), answered at once under the lock this syscall holds.
                Some((Object::Part(i), r)) if r & RIGHT_SEND != 0 => {
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    let reply = crate::partfs::part_call(PARTS[i], &msg, (f.x[4] & !CALL_LIVE) as usize);
                    let n = reply.len().min((f.x[4] & !CALL_LIVE) as usize);
                    p.write(f.x[3], &reply[..n]);
                    f.x[0] = n as u64;
                    0
                }
                // CALL_LIVE (bit 63 of the reply length) and no server receiving on the endpoint — none holds the right, or the one that does has not started receiving yet: ERR now, not a wait for a server that may never come (the IP process at a boot where the WiFi driver failed, 2026-10-29; and while the driver brings its device up).
                Some((Object::Endpoint(ep), r)) if r & RIGHT_SEND != 0 && f.x[4] & CALL_LIVE != 0 && !has_receiver(ep) => {
                    f.x[0] = ERR;
                    0
                }
                Some((Object::Endpoint(ep), r)) if r & RIGHT_SEND != 0 => {
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    // A caller cannot ask for a store reply's decoding (bit 62): only `store_forward` sets it.
                    call_endpoint(f, cur, ep, msg, f.x[3], f.x[4] & !(CALL_LIVE | STORE_REPLY))
                }
                _ => {
                    f.x[0] = ERR;
                    0
                }
            }
        }
        SYS_RECV => {
            let token = p.read(f.x[0], 32);
            p.check(f.x[1], f.x[2] as usize);
            p.check(f.x[3], 32);
            if let Some((Object::Endpoint(ep), r)) = p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                if r & RIGHT_RECV != 0 {
                    kk.endpoints[ep].server = p.epoch;
                }
            }
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
                    if off % 4 == 0 && off < hw.size {
                        crate::platform::note_mmio((hw.base + off) as u64 | ((f.x[8] == SYS_MMIO_WRITE) as u64) << 63);
                    }
                    if off % 4 != 0 || off >= hw.size {
                        ERR
                    } else if f.x[8] == SYS_MMIO_WRITE && crate::s2mpu::is_fence(hw.base) {
                        // A DMA fence is the kernel's: a driver may read its own (for its report), never write it, so the device it drives cannot be let out by the code that talks to it (the modem driver turned HSI1's protection off itself until 2026-11-04).
                        ERR
                    } else if f.x[8] == SYS_MMIO_READ {
                        // A read goes straight to the register even in a block only EL3 may write (the PMU): EL2 reads it, as the kernel's own drivers do, and the EL3 path refuses reads of what it does not list (G#FFFF_FFEA, -22: the modem driver's HSI1 power-domain status, 2026-10-29).
                        unsafe { core::ptr::read_volatile((hw.base + off) as *const u32) as u64 }
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
                // A surface is answered past its porch (cachemap SURFACE_PORCH); every other region at its base.
                Some(Object::Dma(i)) if DMA[i].size > 0 => (DMA[i].base + if DMA[i].name.starts_with("surface-") { crate::cachemap::SURFACE_PORCH } else { 0 }) as u64,
                _ => ERR,
            };
            0
        }
        SYS_ENTROPY => {
            let token = p.read(f.x[0], 32);
            let len = f.x[2] as usize;
            f.x[0] = match p.lookup(&token).map(|i| p.cspace[i].object) {
                Some(Object::Entropy) if len <= 4096 && crate::wairua::draw() => {
                    let n = ENTROPY_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // The same stream as before (BLAKE3 keyed by derive_key(ENTROPY_CONTEXT, wairua), over the counter, as an XOF), computed where the wairua is held: neither it nor the derived key leaves the registers.
                    let mut out = alloc::vec![0u8; len];
                    if crate::keys::stream(ENTROPY_CONTEXT, &n.to_le_bytes(), &mut out) {
                        p.write(f.x[1], &out);
                        len as u64
                    } else {
                        ERR
                    }
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
                    // One transfer, and a forwarded store reply's 8-byte value ahead of it (`store_forward`); a plain caller still gets at most what it asked for.
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(STORE_MAX + 8));
                    let mut handed = false;
                    if caller == KERNEL_CALLER {
                        // The kernel's own call (`vault_call`): kept for the service loop; the server runs on.
                        kernel_answered(Ok(msg));
                    } else if let (Some(ct), Some(cp)) = (th(caller), proc_of(caller)) {
                        if let State::Calling(roff, rcap) = ct.state {
                            if rcap & STORE_REPLY != 0 {
                                // A forwarded store call (`store_forward`): the vault's reply is the syscall's value, then the bytes for the caller's buffer, as many as it asked for. WHY the checks: the reply is the vault process's, its length its own.
                                let data = msg.get(8..).unwrap_or(&[]);
                                let n = data.len().min((rcap & !STORE_REPLY) as usize);
                                cp.write(roff, &data[..n]);
                                ct.frame.x[0] = msg.get(..8).map(|v| u64::from_le_bytes(v.try_into().unwrap())).unwrap_or(ERR);
                            } else {
                                let n = msg.len().min(rcap as usize);
                                cp.write(roff, &msg[..n]);
                                ct.frame.x[0] = n as u64;
                            }
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
    // A buffer of this syscall that resolved nowhere: the caller traps (`buf_trap`). One of another process's (none should be: a server's receive buffer and a caller's reply buffer are its own syscall's) was simply not touched.
    let r = match unsafe { (*(&raw mut BUF_FAULT)).take() } {
        Some(b) if b.epoch == at_epoch => buf_trap(f, cur, pi, r, b, nr, at_op, at_pc, at_lr),
        Some(b) => {
            crate::flashlog::line(&alloc::format!("syscall {nr}: a buffer of another process (epoch {}) at G#{:X} not touched", b.epoch, b.off));
            r
        }
        None => r,
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
/// Every caller whose reply token a process still holds, woken with ERR: that process will never answer it (it ended, or was killed). Without this a client blocked in a call to a server that died stayed in `Calling` for good (ahau in the Photon client's op 3, 2026-10-28). Under the lock.
fn fail_callers(cspace: &[CapEntry]) {
    for c in cspace {
        if let Object::Reply(caller) = c.object {
            if caller == KERNEL_CALLER {
                kernel_answered(Err(()));
            } else if let Some(ct) = th(caller) {
                if matches!(ct.state, State::Calling(..)) {
                    ct.frame.x[0] = ERR;
                    ct.state = State::Runnable;
                    enqueue_back(caller);
                }
            }
        }
    }
}

/// Bit 63 of a CALL's reply length (x4): fail with ERR instead of waiting when no server is receiving on the endpoint (the process that last received on it has ended, or none has received yet), at the call or later when its server ends. Opt-in: a call without it waits for a server as before (a restarted display server takes the map's queued calls).
pub const CALL_LIVE: u64 = 1 << 63;

/// Whether the process that last received on endpoint `ep` is alive (not ending).
fn has_receiver(ep: usize) -> bool {
    let s = k().endpoints[ep].server;
    s != 0 && k().procs.iter().flatten().any(|p| !p.dying && p.epoch == s)
}

/// After a process ends: each CALL_LIVE caller still queued on an endpoint nobody can serve any more gets ERR.
fn fail_orphaned_calls() {
    let kk = k();
    for ep in 0..kk.endpoints.len() {
        // The vault, alive and still opening, has not received yet: the store calls queued for it wait.
        if has_receiver(ep) || vault_ep() == Some(ep) {
            continue;
        }
        // No server left: one started again must receive before CALL_LIVE counts it.
        kk.endpoints[ep].server = 0;
        if kk.endpoints[ep].queue.is_empty() {
            continue;
        }
        let queued: Vec<usize> = kk.endpoints[ep].queue.iter().map(|q| q.0).collect();
        for caller in queued {
            if caller == KERNEL_CALLER {
                // The kernel's own call, queued for a vault that ended: its stamp (domain keys) wiped, the service loop told.
                for q in k().endpoints[ep].queue.iter_mut().filter(|q| q.0 == KERNEL_CALLER) {
                    q.1.fill(0);
                }
                k().endpoints[ep].queue.retain(|q| q.0 != KERNEL_CALLER);
                kernel_answered(Err(()));
            } else if let Some(ct) = th(caller) {
                // A forwarded store call waits as CALL_LIVE does: its frame's x4 is the caller's buffer, so the mark is in the state.
                if matches!(ct.state, State::Calling(..)) && ct.frame.x[4] & CALL_LIVE != 0 || matches!(ct.state, State::Calling(_, c) if c & STORE_REPLY != 0) {
                    ct.frame.x[0] = ERR;
                    ct.state = State::Runnable;
                    enqueue_back(caller);
                    k().endpoints[ep].queue.retain(|q| q.0 != caller);
                }
            }
        }
    }
}

/// Bumped whenever a process is made, ends or is killed: what the service loop watches to act on the process table changing (the GPU's clock, service.rs), with no poll.
pub static PROCS_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
fn end_process(pi: usize, cur: usize, value: u64) {
    PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    // akita bring-up: a process ending (the usb process is the only one there) resets plainly in a FERROS_PROBE=8 build, so it reads apart from the power-up probes 81-85 (board.rs).
    crate::board::probe_log(86);
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
        surfaces_free(p.epoch);
        crate::sensorlink::ended(p.epoch);
        power_orphaned(p.epoch);
        fail_callers(&p.cspace);
        p.cspace.retain(|c| !matches!(c.object, Object::Reply(_)));
        let at = kk.console.len();
        kk.console.extend_from_slice(&name);
        kk.console.extend_from_slice(b": exit G#");
        for i in (0..16).rev() {
            kk.console.push(b"0123456789ABCDEF"[((value >> (4 * i)) & 0xF) as usize]);
        }
        kk.console.push(b'\n');
        crate::platform::pstore(&kk.console[at..]);
        if elsewhere {
            kk.procs[pi].as_mut().unwrap().dying = true;
        } else if let Some(p) = kk.procs[pi].take() {
            arc_free(p.arc);
        }
    }
    fail_orphaned_calls();
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
    crate::platform::pstore(bytes);
    unlock();
    // akita: the RAM log does not survive its resets, so the consoles go to the flash log too (in the ferros partition, board.rs).
    if crate::board::akita() {
        let text = alloc::string::String::from_utf8_lossy(bytes);
        let text = text.trim_end();
        if !text.is_empty() {
            crate::flashlog::line(text);
        }
    }
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
    PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    let kk = k();
    let Some(i) = kk.procs.iter().position(|p| matches!(p, Some(p) if p.name == name)) else { return false };
    surfaces_free(kk.procs[i].as_ref().unwrap().epoch);
    crate::sensorlink::ended(kk.procs[i].as_ref().unwrap().epoch);
    power_orphaned(kk.procs[i].as_ref().unwrap().epoch);
    {
        let p = kk.procs[i].as_mut().unwrap();
        fail_callers(&p.cspace);
        p.cspace.retain(|c| !matches!(c.object, Object::Reply(_)));
    }
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
    // Its endpoints may have no server now: CALL_LIVE callers queued there get ERR, as when a process ends by itself.
    fail_orphaned_calls();
    kk.console.extend_from_slice(name);
    kk.console.extend_from_slice(b": killed\n");
    true
}

/// Whether a process of this name is alive (not one ending).
pub fn running(name: &[u8]) -> bool {
    with_lock(|| k().procs.iter().flatten().any(|p| p.name == name && !p.dying))
}

/// How many processes are alive (running, runnable or blocked).
/// IPC round trips since boot (a call answered by a server).
pub fn round_trips() -> u32 {
    with_lock(|| k().ipc_round_trips)
}

/// `diag pets`: the process watchdog's measurements, one line a running process: its pets, the longest stretch between two (from its spawn for the first) and how long ago that stretch ended, and how long since its last pet. A process that never pets is listed with none.
pub fn pets_report(out: &mut Vec<u8>) {
    lock();
    let (now, f) = (crate::platform::cntpct(), crate::platform::cntfrq());
    let ms = |t: u64| t * 1000 / f;
    for p in k().procs.iter().flatten() {
        if p.dying {
            continue;
        }
        let name = core::str::from_utf8(&p.name).unwrap_or("?");
        if p.pets == 0 {
            out.extend_from_slice(alloc::format!("{name:<10} no pets, up A#{} ms\n", ms(now - p.born)).as_bytes());
        } else {
            out.extend_from_slice(alloc::format!("{name:<10} A#{} pets, longest stretch A#{} ms (ended A#{} ms ago), last pet A#{} ms ago{}, start A#{} ms (not counted), up A#{} ms\n", p.pets, ms(p.gap_max), ms(now - p.gap_at), ms(now - p.last_pet), if p.paused_at != 0 { " (paused now)" } else { "" }, ms(p.start), ms(now - p.born)).as_bytes());
        }
    }
    unlock();
}

pub fn alive() -> usize {
    lock();
    let n = alive_locked();
    unlock();
    n
}
/// The same with the lock already held — from inside a syscall (the facts provider): `alive` there took the lock its own core held and waited on itself forever, which froze the device whenever the device's settings were open (2026-10-23).
pub fn alive_locked() -> usize {
    k().procs.iter().filter(|p| p.is_some()).count()
}
/// Which of `names` have a live process, bit i for names[i] (the facts provider's radios, SYS_FACTS 32); the lock already held, as `alive_locked`.
pub fn named_alive_locked(names: &[&[u8]]) -> u64 {
    let mut m = 0;
    for p in k().procs.iter().flatten() {
        if let Some(i) = names.iter().position(|n| *n == p.name.as_slice()) {
            m |= 1 << i;
        }
    }
    m
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

/// Arc layout: grants page, then code. The grants page (SANDBOX.md, "Handles"): the handle table in its first 512 bytes (slot k's pair at 16k: the granted region's base, then its mask, both zero when the grant is not a region or the region cannot be masked), the u32 token count at `TOKENS`, the 32-byte tokens after it, the preferences at `PREFS`, the ihi at `IHI`. Read-only to every level once written, so a process cannot forge a pair; the kernel opens it to write a preference.
const GRANTS: usize = 0;
pub const HANDLES: usize = 0;
pub const TOKENS: usize = 0x200;
pub const CODE: usize = 0x1000;
pub const STACK: usize = 64 * 1024;

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
    /// The ira-derived key right (syscall 23).
    Ira,
    /// A device's power, by name (syscall 22).
    Power(&'a [u8]),
    /// Read-only files on one of the device's own partitions, by name (`PARTS`): the modem driver's images, NV and carrier configs (partfs.rs `part_call`).
    Part(&'a [u8]),
    /// The identity unlock right (syscall 24).
    Unlock,
    /// The kernel's PT commands, for the USB driver process (syscall 25).
    Pt,
    /// Publishing the sensors' block, for the sensors process (syscall 26).
    Sensors,
    /// A window of blocks on the ferros partition, by name (`WINDOWS`, syscall 29).
    Blocks(&'a [u8]),
    /// The vault process's right (syscall 30), and with it the right to serve `vault`.
    Vault,
    /// The holder's own vault domain (`Object::Domain`).
    Domain,
    /// The right to launch kists (`Object::Kists`): init's.
    Kists,
    /// The right to install kists (`Object::Install`): the installer's (payloads/kists).
    Install,
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
    spawn(name, &g.code, input, grants, Some(ihi), g.masked, g.text_len, g.fixed | (g.bti as u32) << 8 | (g.exact as u32) << 9)
}

/// Install a masked capsule: the gate (signature), the sandbox checker over its instruction span — once — then its seal. Returns the seal for the caller to add to the vault's `sealed` list.
pub fn install_capsule(capsule: &[u8]) -> Result<[u8; 32], &'static str> {
    let g = crate::oracle::gate(capsule)?;
    if !g.masked {
        return Err("install: only masked capsules are installed");
    }
    let mode = if g.exact { ferros_sandbox::Mode::Exact } else if g.fixed != 0 { ferros_sandbox::Mode::Fixed(g.fixed) } else { ferros_sandbox::Mode::Ring };
    ferros_sandbox::check_image_with(&g.code[..g.text_len], mode, g.bti).map_err(|_| "install: masked code failed the sandbox checker")?;
    crate::oracle::seal(&g).ok_or("no system secret: cannot seal")
}

/// Spawn `code` (position-independent, entry at offset 0) as a process holding a console capability (grant slot 0) plus `grants` (slots 1..), and run until nothing is runnable: it exits, or it blocks (a server in recv stays parked for later callers). `input` is copied into the arc after the code; x0 = grants offset, x1 = input offset, x2 = input length.
pub fn spawn(name: &[u8], code: &[u8], input: &[u8], grants: &[Grant], ihi: Option<[u8; 32]>, masked: bool, exec_len: usize, fixed: u32) -> Result<Outcome, &'static str> {
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
    let built = build(name, code, input, &objects, ihi, masked, exec_len, fixed);
    launch(built, input.len())
}

/// A signed native capsule the bridge streamed straight into `arc` at the code offset (`service.rs`, the spawn Write pieces): the gate over it where it lies, the code moved down to the arc's code offset, the rest of the staging zeroed, and the process built in that arc (STD-TARGET.md: a std image is far past the 4 MiB staging slot). On any refusal the arc goes back to the pool.
pub fn spawn_capsule_staged(name: &[u8], arc: Arc, capsule_len: usize, code_offset: usize, code_len: usize, input: &[u8], grants: &[Grant]) -> Result<Outcome, &'static str> {
    let r = (|| {
        if cfg!(ferros_production) {
            return Err("gate: production refuses unmasked native code");
        }
        if !crate::cachemap::vhe() {
            return Err("no VHE: processes need the EL2&0 regime");
        }
        if CODE + capsule_len + input.len() + STACK > arc.size || code_offset + code_len > capsule_len {
            return Err("spawn: the staged capsule does not fit its arc");
        }
        let capsule = unsafe { core::slice::from_raw_parts_mut((arc.base + CODE) as *mut u8, capsule_len) };
        let g = crate::oracle::gate_located(capsule, code_offset, code_len)?;
        let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
        unsafe {
            core::ptr::copy((arc.base + CODE + code_offset) as *const u8, (arc.base + CODE) as *mut u8, code_len);
            core::ptr::write_bytes((arc.base + CODE + code_len) as *mut u8, 0, capsule_len - code_len);
        }
        Ok(ihi)
    })();
    let ihi = match r {
        Ok(i) => i,
        Err(e) => {
            // The pools are the kernel's shared state: under the lock, as every arc taken or given back (a worker frees a process's arc at its end meanwhile).
            with_lock(|| arc_free(arc));
            return Err(e);
        }
    };
    lock();
    let objects = match resolve(grants) {
        Ok(o) => o,
        Err(e) => {
            arc_free(arc);
            unlock();
            return Err(e);
        }
    };
    let built = build_from(name, arc, code_len, input, &objects, Some(ihi), false, code_len, false);
    launch(built, input.len())
}

// ---------------------------------------------------------------- kists (KISTS.md)

/// A kist launch init asked for (`SYS_SPAWN` kist mode), waiting for the service loop to stream its capsule from the vault process (service.rs `KistJob`): its process name, its capsule's vault address, and a right code or `ferros_kist::EMPTY` per slot.
pub struct KistLaunch {
    pub name: Vec<u8>,
    pub capsule: [u8; 32],
    pub codes: Vec<u8>,
}
/// One launch at a time: the service loop streams one capsule through the kernel's channel at once. Under the kernel lock.
static mut KIST_LAUNCH: Option<KistLaunch> = None;
/// A launch being streamed by the service loop, by name: a second ask for it is "already starting" (NOTHING), as for a live one.
static mut KIST_STARTING: Vec<u8> = Vec::new();

/// The object right `code` (ferros_kist::RIGHTS) is handed as, made by the kernel itself: the holder of `Kists` needs no grant of its own for any of them. None for a code outside the vocabulary.
fn kist_object(code: u8) -> Option<(Object, u32)> {
    if code == ferros_kist::EMPTY {
        return Some((Object::Empty, 0));
    }
    Some(match *ferros_kist::RIGHTS.get(code as usize)? {
        "display" => (Object::Endpoint(endpoint(b"display")), RIGHT_SEND),
        "touch" => (Object::Endpoint(endpoint(b"touch")), RIGHT_SEND),
        "surface" => (Object::NewSurface, RIGHT_SEND | RIGHT_RECV),
        "ira" => (Object::Ira, RIGHT_SEND),
        "domain" => (Object::Domain, RIGHT_SEND),
        "entropy" => (Object::Entropy, RIGHT_SEND),
        "gps" => (Object::Endpoint(endpoint(b"gps")), RIGHT_SEND),
        "udp" => (Object::Endpoint(endpoint(b"udp")), RIGHT_SEND),
        _ => return None,
    })
}

/// `SYS_SPAWN` in kist mode (x2 bit 1; bit 2, a relaunch: a live process of that name killed first, NOTHING with none alive): [name length][name][the capsule's 32-byte vault address][the caller's Kists token, 32 bytes][n][n right codes, `EMPTY` for a slot said no to]. Queued for the service loop, which streams the capsule from the vault and checks every slot against the capsule's own declaration before anything runs (`spawn_kist`). 0 queued, NOTHING already running or starting, ERR refused (the reason on the console): no Kists right, a code outside the vocabulary, nobody present (a kist's data and rights are the person's), or another launch still streaming.
fn kist_request(p: &Process, req: &[u8], replace: bool) -> u64 {
    let mut at = 0usize;
    let mut take = |n: usize| -> Vec<u8> { let s = req[at.min(req.len())..(at + n).min(req.len())].to_vec(); at += n; s };
    let nl = take(1).first().copied().unwrap_or(0) as usize;
    let name = take(nl);
    let capsule = take(32);
    let token = take(32);
    let n = take(1).first().copied().unwrap_or(0) as usize;
    let codes = take(n);
    let refuse = |why: &str| {
        let kk = k();
        kk.console.extend_from_slice(b"kist launch refused: ");
        kk.console.extend_from_slice(why.as_bytes());
        kk.console.push(b'\n');
        ERR
    };
    if !matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Kists, r)) if r & RIGHT_SEND != 0) {
        return refuse("the caller holds no Kists right");
    }
    if name.is_empty() || capsule.len() != 32 || codes.len() != n || codes.iter().any(|&c| kist_object(c).is_none()) {
        return refuse("a malformed request, or a right outside the vocabulary");
    }
    let kk = k();
    if unsafe { DOMAIN_BASE }.and_then(|b| b(true)).is_none() {
        return refuse("nobody is present: a kist runs only for an attested person");
    }
    let slot = unsafe { &mut *(&raw mut KIST_LAUNCH) };
    if slot.is_some() {
        return refuse("another kist is still starting");
    }
    let alive = kk.procs.iter().flatten().any(|q| q.name == name && !q.dying);
    // A relaunch (the person changed an answer): the running kist killed, one shot dead, before it is launched again with the answers as they stand. Not running: nothing to relaunch.
    if replace {
        if !alive {
            return NOTHING;
        }
        kill_locked(&name);
    } else if alive || unsafe { &*(&raw const KIST_STARTING) }.as_slice() == name.as_slice() {
        return NOTHING;
    }
    *slot = Some(KistLaunch { name, capsule: capsule[..].try_into().unwrap(), codes });
    unsafe { core::arch::asm!("sev") };
    0
}

/// Whether a process is ending but not yet gone (a thread of it still on a core: its arc returns at that thread's next trap).
pub fn any_dying() -> bool {
    with_lock(|| k().procs.iter().flatten().any(|p| p.dying))
}

/// The service loop takes the waiting launch, marking its name as starting until `kist_done`.
pub fn kist_take() -> Option<KistLaunch> {
    with_lock(|| {
        let l = unsafe { (*(&raw mut KIST_LAUNCH)).take() }?;
        unsafe { *(&raw mut KIST_STARTING) = l.name.clone() };
        Some(l)
    })
}

/// The launch the service loop was streaming has ended, started or not.
pub fn kist_done() {
    with_lock(|| unsafe { (*(&raw mut KIST_STARTING)).clear() });
}

/// Where a capsule's code lies inside it, from its `masked` section's table entry (sandboxed code, SANDBOX.md) or else its `aarch64` one (native, development only): the byte vector's offset and length, which must end within the capsule's `whole` length (the gate checks the framing again over the signed bytes), and whether it is masked. Needs only the header and the section's first bytes.
pub fn locate_code(capsule: &[u8], whole: usize) -> Result<(usize, usize, bool), &'static str> {
    let (header, _) = vsf::file_format::VsfHeader::decode(capsule).map_err(|_| "kist: the capsule's header does not decode")?;
    let masked = header.fields.iter().any(|f| f.name == "masked");
    let entry = header.fields.iter().find(|f| f.name == if masked { "masked" } else { "aarch64" }).ok_or("kist: no code section")?;
    let from = entry.offset_bytes;
    // WHY the window: the code's `vb` marker sits within the section's first bytes (its bracket, the field's name and colon); the bound keeps a capsule whose table points past its end from reading past it.
    for p in from..(from + 64).min(capsule.len().saturating_sub(2)) {
        if capsule[p] == b'v' && capsule[p + 1] == b'b' {
            let mut q = p + 2;
            if let Ok(bits) = vsf::ewe::read_uint::<u64>(capsule, &mut q) {
                let len = (bits >> 3) as usize;
                if bits & 7 == 0 && len > 0 && q + len <= whole {
                    return Ok((q, len, masked));
                }
            }
        }
    }
    Err("kist: the code's byte vector is not where its section begins")
}

/// The arc a kist's capsule needs, from its first bytes (`head`: the header, the declaration and the start of its code) and its whole length: the capsule staged at the code offset, or the code with its zeroed tail (`image_tail`), whichever is more, the entry stack and the room a streamed spawn leaves (the bridge's 64 KiB).
pub fn kist_need(head: &[u8], total: usize) -> Result<usize, &'static str> {
    let (co, cl, _) = locate_code(head, total)?;
    if co + 56 > head.len() {
        return Err("kist: the code's header is past the first bytes read");
    }
    if !has_image_header(&head[co..co + 56]) {
        return Err("kist: the image has no header (ferros_sys::entry!), so its .bss is unknown");
    }
    let tail = image_tail(&head[co..co + 56]);
    Ok(CODE + total.max(cl + tail) + STACK + (1 << 16))
}

/// Whether a kist's capsule carries masked code, from its first bytes (its header's table names the section): its arc is then taken masked, with the guard above it (SANDBOX.md "Placement and guards").
pub fn kist_masked(head: &[u8], total: usize) -> bool {
    locate_code(head, total).is_ok_and(|c| c.2)
}

/// A masked kist's code where it lies in its staged capsule, gated, its modes read and its seal found in `sealed` (32-byte entries): SANDBOX.md's "spawn runs only sealed images", one BLAKE3 over the image and no checker. Returns the developer's key and identity for the ihi, and the modes.
fn kist_sealed(capsule: &mut [u8], co: usize, cl: usize, sealed: &[u8]) -> Result<(crate::oracle::Located, crate::oracle::Modes), &'static str> {
    let g = crate::oracle::gate_located(capsule, co, cl)?;
    let m = crate::oracle::masked_modes(capsule, co, cl)?;
    let s = crate::oracle::seal_parts(&g.developer_pubkey, &g.hp, true, &m, &capsule[co..co + cl]).ok_or("no system secret: cannot check a seal")?;
    if !sealed.chunks_exact(32).any(|e| e == s) {
        return Err("gate: masked kist not installed on this device (no seal)");
    }
    Ok((g, m))
}

/// A kist whose capsule the service loop streamed from the vault into `arc` at the code offset (`total` bytes): it must be the capsule named (its blake3 is its vault address), pass the gate where it lies, and declare every right handed to it in the slot it is handed in (`ferros_kist::want_codes`: nothing undeclared can be granted). Masked code runs only with its seal in `sealed` (the installer's, `install_kist`), the checker never run again; native code only in a development build (the bridge's `kist install`). Then the code moves down, the rest is zeroed, and the process is built with exactly the launch's objects, queued to run. On any refusal the arc goes back to the pool.
pub fn spawn_kist(l: &KistLaunch, arc: Arc, total: usize, sealed: &[u8]) -> Result<(), &'static str> {
    let mut arc = arc;
    let r = (|| {
        if CODE + total + STACK > arc.size {
            return Err("kist: the capsule does not fit its arc");
        }
        let capsule = unsafe { core::slice::from_raw_parts_mut((arc.base + CODE) as *mut u8, total) };
        if blake3::hash(capsule).as_bytes() != &l.capsule {
            return Err("kist: the vault's capsule is not the one named (its blake3)");
        }
        let wants = ferros_kist::want_codes(capsule)?;
        if wants.len() != l.codes.len() || wants.iter().zip(&l.codes).any(|(&w, &c)| c != ferros_kist::EMPTY && c != w) {
            return Err("kist: a right the capsule does not declare in that slot");
        }
        let (co, cl, masked) = locate_code(capsule, total)?;
        let (g, modes) = if masked {
            let (g, m) = kist_sealed(capsule, co, cl, sealed)?;
            (g, Some(m))
        } else {
            if cfg!(ferros_production) {
                return Err("gate: production refuses unmasked native code");
            }
            (crate::oracle::gate_located(capsule, co, cl)?, None)
        };
        // A masked arc is the one its code was checked for: fixed mode's exactly 2^K, which a staged capsule's arc is not (it holds the whole capsule first).
        if modes.as_ref().is_some_and(|m| m.fixed != 0 && arc.size != 1 << m.fixed) {
            return Err("kist: a fixed-mode kist is not launched from the vault yet (its arc must be exactly the sealed size)");
        }
        if masked != (arc.guard != 0) {
            return Err("kist: the staged arc's kind is not the code's (masked or native)");
        }
        let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
        unsafe {
            core::ptr::copy((arc.base + CODE + co) as *const u8, (arc.base + CODE) as *mut u8, cl);
            core::ptr::write_bytes((arc.base + CODE + cl) as *mut u8, 0, total - cl);
        }
        Ok((ihi, cl, modes))
    })();
    let (ihi, cl, modes) = match r {
        Ok(x) => x,
        Err(e) => {
            with_lock(|| arc_free(arc));
            return Err(e);
        }
    };
    // Exact mode: the limit is what the image needs, as `build` sets it (the staged capsule's rest, past it, was zeroed above).
    if modes.as_ref().is_some_and(|m| m.exact) {
        let tail = image_tail(unsafe { core::slice::from_raw_parts((arc.base + CODE) as *const u8, cl) });
        arc.limit = (CODE + cl + tail + STACK_GUARD + STACK + 15) & !15;
    }
    let (masked, exec_len, bti) = match &modes {
        Some(m) => (true, m.text_len, m.bti),
        None => (false, cl, false),
    };
    with_lock(|| {
        let kk = k();
        if kk.procs.iter().flatten().any(|q| q.name == l.name && !q.dying) {
            arc_free(arc);
            return Err("kist: a process of that name started meanwhile");
        }
        // Every slot's object made here (`kist_object`), the codes checked at the request.
        let objects: Vec<(Object, u32)> = l.codes.iter().filter_map(|&c| kist_object(c)).collect();
        match build_from(&l.name, arc, cl, b"", &objects, Some(ihi), masked, exec_len, bti) {
            Ok((me, _, _)) => {
                if let Some(q) = th(me).map(|t| t.proc).and_then(|pi| kk.procs.get_mut(pi)).and_then(|q| q.as_mut()) {
                    q.kist = true;
                }
                enqueue_back(me);
                Ok(())
            }
            Err(e) => {
                arc_free(arc);
                Err(e)
            }
        }
    })
}

/// An install the installer asked for (`install_request`): the capsule by its vault address, and where to tell it each stage: the process (its slot and epoch, so a word is never written into a successor's arc) and the word's address in its arc.
pub struct KistInstall {
    pub capsule: [u8; 32],
    proc: usize,
    epoch: u64,
    word: usize,
}
/// One install at a time, beside one launch: the service loop streams one capsule through the kernel's channel at once (launches first). Under the kernel lock.
static mut KIST_INSTALL: Option<KistInstall> = None;

/// The stages an install is told in (bits 56-63 of the word; ferros_sys `install` mirrors them): reading the capsule from the vault (the bytes read so far below), checking its code, sealing it, done (the word's low bits 0), refused (the low byte the reason, `INSTALL_WHY_*`, said on the console too).
pub const INSTALL_READING: u64 = 0;
pub const INSTALL_CHECKING: u64 = 1;
pub const INSTALL_SEALING: u64 = 2;
pub const INSTALL_DONE: u64 = 3;
pub const INSTALL_REFUSED: u64 = 4;
/// Why an install was refused: the vault's capsule is not the one named, its sections do not read, it is native code (a development build: never installed from the network, owner 2026-11-10), its signature does not verify, its masked fields do not read, the checker refused its code, no system secret to seal with, the vault would not keep the seal, the vault could not read it, no arc for it.
pub const INSTALL_WHY_NOT_NAMED: u64 = 1;
pub const INSTALL_WHY_SECTIONS: u64 = 2;
pub const INSTALL_WHY_NATIVE: u64 = 3;
pub const INSTALL_WHY_SIGNATURE: u64 = 4;
pub const INSTALL_WHY_MODES: u64 = 5;
pub const INSTALL_WHY_CHECKER: u64 = 6;
pub const INSTALL_WHY_SECRET: u64 = 7;
pub const INSTALL_WHY_KEPT: u64 = 8;
pub const INSTALL_WHY_READ: u64 = 9;
pub const INSTALL_WHY_ROOM: u64 = 10;

/// `SYS_SPAWN` in install mode (x2 bit 3): [the capsule's 32-byte address in `capsules.`][the caller's Install token, 32 bytes][the word's offset in its arc, u64 LE]. Queued for the service loop (`install_take`), which reads the capsule from the vault and tells the word each stage, waking whoever waits on it (`install_tell`). 0 queued, ERR refused (the reason on the console): no Install right, a malformed request, another install still under way.
fn install_request(p: &Process, pi: usize, req: &[u8]) -> u64 {
    let refuse = |why: &str| {
        let kk = k();
        kk.console.extend_from_slice(b"kist install refused: ");
        kk.console.extend_from_slice(why.as_bytes());
        kk.console.push(b'\n');
        ERR
    };
    if req.len() < 72 {
        return refuse("a malformed request");
    }
    if !matches!(p.lookup(&req[32..64]).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Install, r)) if r & RIGHT_SEND != 0) {
        return refuse("the caller holds no Install right");
    }
    let slot = unsafe { &mut *(&raw mut KIST_INSTALL) };
    if slot.is_some() {
        return refuse("another install is still under way");
    }
    let off = u64::from_le_bytes(req[64..72].try_into().unwrap()) as usize;
    // The word inside the caller's own arc, as `SYS_WAIT` finds it: masked into it and aligned.
    let word = p.arc.base + (off & p.arc.mask() & !7);
    *slot = Some(KistInstall { capsule: req[..32].try_into().unwrap(), proc: pi, epoch: p.epoch, word });
    unsafe { core::arch::asm!("sev") };
    0
}

/// The service loop takes the waiting install (after any launch).
pub fn install_take() -> Option<KistInstall> {
    with_lock(|| unsafe { (*(&raw mut KIST_INSTALL)).take() })
}

/// The installer told a stage: `stage` in bits 56-63, `value` below (the bytes read, or the reason), written into its word and every thread of it waiting there woken. Nothing when the process has ended (its slot empty or a successor's).
pub fn install_tell(i: &KistInstall, stage: u64, value: u64) {
    with_lock(|| {
        let kk = k();
        if !kk.procs.get(i.proc).and_then(|q| q.as_ref()).is_some_and(|q| q.epoch == i.epoch && !q.dying) {
            return;
        }
        unsafe { core::ptr::write_volatile(i.word as *mut u64, stage << 56 | value & ((1 << 56) - 1)) };
        for (t, x) in kk.threads.iter_mut().enumerate() {
            if let Some(x) = x {
                if x.proc == i.proc && matches!(x.state, State::Waiting(a, _) if a == i.word) {
                    x.state = State::Runnable;
                    enqueue_back(t);
                }
            }
        }
    });
}

/// An install's capsule, read from the vault into `arc` at the code offset (`total` bytes), checked as SANDBOX.md's lifecycle says, once: it is the capsule named (its blake3), its code is masked (native code is a development build, never installed from the network), its signature verifies where it lies (`gate_located`, the Ed25519 egg; the installer checked every egg before it stored it), and the sandbox checker passes its instruction span in its mode. Returns the code's span in the arc, for `install_seal`. The arc is the caller's to free.
pub fn install_check(arc: &Arc, total: usize, addr: &[u8; 32]) -> Result<(crate::oracle::Located, crate::oracle::Modes, usize, usize), u64> {
    let capsule = unsafe { core::slice::from_raw_parts_mut((arc.base + CODE) as *mut u8, total) };
    let say = |why: &str| push_console(alloc::format!("kist install: {why}\n").as_bytes());
    if blake3::hash(capsule).as_bytes() != addr {
        say("the vault's capsule is not the one named (its blake3)");
        return Err(INSTALL_WHY_NOT_NAMED);
    }
    let (co, cl, masked) = locate_code(capsule, total).map_err(|e| {
        say(e);
        INSTALL_WHY_SECTIONS
    })?;
    if !masked {
        say("native code: a development build is not installed from the network");
        return Err(INSTALL_WHY_NATIVE);
    }
    let g = crate::oracle::gate_located(capsule, co, cl).map_err(|e| {
        say(e);
        INSTALL_WHY_SIGNATURE
    })?;
    let m = crate::oracle::masked_modes(capsule, co, cl).map_err(|e| {
        say(e);
        INSTALL_WHY_MODES
    })?;
    let mode = if m.exact { ferros_sandbox::Mode::Exact } else if m.fixed != 0 { ferros_sandbox::Mode::Fixed(m.fixed) } else { ferros_sandbox::Mode::Ring };
    if let Err(v) = ferros_sandbox::check_image_with(&capsule[co..co + m.text_len], mode, m.bti) {
        say(&alloc::format!("the checker refused its code at +G#{:X} (instruction G#{:08X})", v.offset, v.insn));
        return Err(INSTALL_WHY_CHECKER);
    }
    Ok((g, m, co, cl))
}

/// The seal of an install that passed `install_check`: keyed by this device's system secret, so it is good on this device alone.
pub fn install_seal(arc: &Arc, g: &crate::oracle::Located, m: &crate::oracle::Modes, co: usize, cl: usize) -> Result<[u8; 32], u64> {
    let code = unsafe { core::slice::from_raw_parts((arc.base + CODE + co) as *const u8, cl) };
    crate::oracle::seal_parts(&g.developer_pubkey, &g.hp, true, m, code).ok_or_else(|| {
        push_console(b"kist install: no system secret: cannot seal\n");
        INSTALL_WHY_SECRET
    })
}

/// The second half of a spawn, entered with the lock held: the built process runs its first slice on this core and the outcome comes back.
fn launch(built: Result<(usize, Arc, usize), &'static str>, input_len: usize) -> Result<Outcome, &'static str> {
    let (me, arc, in_off) = match built {
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
    unsafe { ferros_el0_enter(arc.base + CODE, arc.top() - 16, arc.base, arc.x27(), GRANTS as u64, in_off as u64, input_len as u64) };
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
            // The vault endpoint carries domain keys (`store_forward`): served only by a spawn that holds the vault right, called only by the kernel.
            Grant::Serve(n) if *n == VAULT_EP && !grants.iter().any(|g| matches!(g, Grant::Vault)) => return Err("serving the vault needs the vault right"),
            Grant::Use(n) if *n == VAULT_EP => return Err("the vault endpoint is the kernel's to call"),
            Grant::Serve(n) => (Object::Endpoint(endpoint(n)), RIGHT_RECV),
            Grant::Use(n) => (Object::Endpoint(endpoint(n)), RIGHT_SEND),
            Grant::Vault => (Object::Vault, RIGHT_SEND),
            Grant::Domain => (Object::Domain, RIGHT_SEND),
            Grant::Kists => (Object::Kists, RIGHT_SEND),
            Grant::Install => (Object::Install, RIGHT_SEND),
            Grant::Hardware(n) => match HARDWARE.iter().position(|h| h.name.as_bytes() == *n) {
                Some(i) => (Object::Hardware(i), RIGHT_SEND | RIGHT_RECV),
                None => return Err("no such hardware block"),
            },
            Grant::Entropy => (Object::Entropy, RIGHT_SEND),
            Grant::Dma(b"surface") => (Object::NewSurface, RIGHT_SEND | RIGHT_RECV),
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
            Grant::Power(n) => match POWERS.iter().position(|r| r.name.as_bytes() == *n) {
                Some(i) => (Object::Power(i), RIGHT_SEND),
                None => return Err("no such power switch"),
            },
            Grant::Ira => (Object::Ira, RIGHT_SEND),
            Grant::Unlock => (Object::Unlock, RIGHT_SEND),
            Grant::Blocks(n) => match WINDOWS.iter().position(|w| w.name.as_bytes() == *n) {
                Some(i) => (Object::Blocks(i), RIGHT_SEND),
                None => return Err("no such block window"),
            },
            Grant::Pt => (Object::Pt, RIGHT_SEND),
            Grant::Sensors => (Object::Sensors, RIGHT_SEND),
            Grant::Part(n) => match PARTS.iter().position(|p| p.as_bytes() == *n) {
                Some(i) => (Object::Part(i), RIGHT_SEND),
                None => return Err("no such partition grant"),
            },
        });
    }
    Ok(v)
}

/// The zeroed memory a std image asks for after its code — its `.bss` and its heap — from the header `ferros_rt` puts at offset 8: the eight bytes `ferrosRT`, then the byte count (STD-TARGET.md). The flat image cannot carry `.bss` (the input used to land right after the code), so the image says how much the arc must grow and the input moves past it. A native payload has no header: 0.
/// Whether the image carries ferros_rt's header ("ferrosRT" at offset 8): its zeroed tail is known.
fn has_image_header(code: &[u8]) -> bool {
    code.len() >= 56 && &code[8..16] == b"ferrosRT"
}

/// The page under the first thread's stack, kernel-only (owner, 2026-11-09): a stack run past its 64 KiB, or statics grown up into it, faults there with the process named, not into memory it does not own.
const STACK_GUARD: usize = 1 << 12;

fn image_tail(code: &[u8]) -> usize {
    if code.len() >= 56 && &code[8..16] == b"ferrosRT" {
        // Two words, summed here: .bss plus 16 (the linker script's own arithmetic) and the heap (a --defsym the script only forwards; summing them in the script took the default heap once, 2026-10-27).
        let w = |at: usize| u64::from_le_bytes(code[at..at + 8].try_into().unwrap()) as usize;
        (w(16) + w(48)).min(1 << 30)
    } else {
        0
    }
}

/// Make a process — arc, capability table (console first, then `objects` in order), grants page, code, input — and its first thread, in the tables, not yet running. Returns (the thread's index, the arc, the input's offset).
fn build(name: &[u8], code: &[u8], input: &[u8], objects: &[(Object, u32)], ihi: Option<[u8; 32]>, masked: bool, exec_len: usize, fixed: u32) -> Result<(usize, Arc, usize), &'static str> {
    cap_key().ok_or("no wairua: refusing to mint capabilities")?;
    let inline = input.len(); // bridge: full inline (pcie1 still reads input from its arc); sandboxed apps carry no input, so their arcs stay small either way. INLINE_INPUT becomes the cap once pcie1 streams firmware via SYS_INPUT (option 1).
    // No header, no spawn (owner, 2026-11-09): without it the .bss is unknown, and the arc sized from the file alone put the stack on the process's own statics (the display and bt, 2026-11-08). Every payload carries it (ferros_sys::entry!, tools/payload-check.sh).
    if !has_image_header(code) {
        return Err("spawn: the image has no header (ferros_sys::entry!), so its .bss is unknown");
    }
    let tail = image_tail(code);
    // `fixed` carries the mode word: bits 0-7 fixed mode's K (0: not fixed), bit 8 BTI (guarded code pages), bit 9 exact mode.
    let (fixed, bti, exact) = (fixed & 0xFF, fixed >> 8 & 1 == 1, fixed >> 9 & 1 == 1);
    let need = CODE + code.len() + tail + inline + STACK_GUARD + STACK;
    // Fixed mode (SANDBOX.md, "Modes"): the arc is exactly the sealed 2^K, the size every `bfi` in the code assumes.
    if fixed != 0 && need > 1 << fixed {
        return Err("spawn: the image does not fit its fixed arc");
    }
    let mut arc = arc_alloc(if fixed != 0 { 1 << fixed } else { need }, masked).ok_or("arc pool exhausted")?;
    // Exact mode: the limit is what the image needs, rounded to the 16 bytes the stack pointer must keep; the arc around it is the power of two the pool hands out, its rest the room the limit can grow into.
    if exact {
        arc.limit = (need + 15) & !15;
    }
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), (arc.base + CODE) as *mut u8, code.len()) };
    build_from(name, arc, code.len(), input, objects, ihi, masked, exec_len, bti)
}

/// The process in an arc that already holds its code at the code offset (`build` copied it; a streamed spawn staged it there): the capability table, the grants page, the ihi, the preferences, the input past the code's zeroed tail, W^X, and the first thread.
fn build_from(name: &[u8], arc: Arc, code_len: usize, input: &[u8], objects: &[(Object, u32)], ihi: Option<[u8; 32]>, masked: bool, exec_len: usize, bti: bool) -> Result<(usize, Arc, usize), &'static str> {
    cap_key().ok_or("no wairua: refusing to mint capabilities")?;
    if !has_image_header(unsafe { core::slice::from_raw_parts((arc.base + CODE) as *const u8, code_len.min(56)) }) {
        return Err("spawn: the image has no header (ferros_sys::entry!), so its .bss is unknown");
    }
    let tail = image_tail(unsafe { core::slice::from_raw_parts((arc.base + CODE) as *const u8, code_len) });
    // Room for the stack's guard page between the image (its tail and the input after it) and the stack; an arc too small is refused before anything is made (a staged arc is sized by `kist_need`, with room).
    let floor = arc.top() - STACK - STACK_GUARD;
    if floor < arc.base + CODE + code_len + tail + input.len() {
        return Err("spawn: no room for the stack's guard page above the image");
    }
    let epoch = EPOCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    let mut cspace = alloc::vec![CapEntry { token: mint(Object::Console, RIGHT_SEND, epoch), object: Object::Console, rights: RIGHT_SEND }];
    for &(object, rights) in objects {
        let object = match object {
            Object::NewSurface => Object::Dma(surface0() + surface_take(epoch).ok_or("no free surface (four are in use)")?),
            o => o,
        };
        cspace.push(CapEntry { token: mint(object, rights, epoch), object, rights });
    }
    // The tokens end at PREFS_HI (G#FB0, the second preferences block, then the first at G#FC0 and the ihi at G#FE0): a 109th slot's token was once written over the preferences and overwritten by the next preferences write, so init's spawn of the GPU service was refused for a token it held (2026-11-05). Refused here, with the reason, rather than corrupted.
    if TOKENS + 4 + 32 * cspace.len() > PREFS_HI {
        return Err("spawn: more grants than the grants page holds (108 slots and the console)");
    }
    let zero = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0, v: [0; 32], fpcr: 0, fpsr: 0, tpidr: 0, _pad: 0 };
    // An empty slot's token is zeros: a want said no to reads as no grant at all.
    for c in cspace.iter_mut().filter(|c| c.object == Object::Empty) {
        c.token = [0; 32];
    }
    let domain = ihi.map(|i| blake3::derive_key(KIST_DOMAIN_CONTEXT, &i));
    let p = Process { arc, cspace, epoch, dying: false, name: name.to_vec(), line_start: true, input: input.to_vec(), born: crate::platform::cntpct(), pets: 0, last_pet: 0, gap_max: 0, gap_at: 0, start: 0, pet_thread: usize::MAX, paused_at: 0, paused: 0, domain, kist: false };
    PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    // Grants page: the handle table, the count, then the tokens in slot order. Then the code, then the input.
    p.write_grants(TOKENS as u64, &(p.cspace.len() as u32).to_le_bytes());
    for (i, c) in p.cspace.iter().enumerate() {
        p.write_grants((TOKENS + 4 + 32 * i) as u64, &c.token);
        // The table holds the first 32 slots, the pair load's reach (init holds a hundred grants and uses no handle; its tokens begin where the table ends).
        if i < 32 {
            let (base, mask) = handle_of(c.object);
            p.write_grants((HANDLES + 16 * i) as u64, &base.to_le_bytes());
            p.write_grants((HANDLES + 16 * i + 8) as u64, &mask.to_le_bytes());
        }
    }
    if let Some(i) = ihi {
        p.write_grants(IHI as u64, &i);
    }
    let prefs = unsafe { &*(&raw const PREFS_NOW) };
    p.write_grants(PREFS as u64, &prefs[..32]);
    p.write_grants(PREFS_HI as u64, &prefs[32..]);
    let in_off = CODE + code_len + tail;
    p.write(in_off as u64, input);
    crate::cachemap::sync_code(arc.base + CODE, code_len);
    // W^X (SANDBOX.md): a masked process may execute only its checked .text ([CODE, CODE+exec_len)), read-only, and may not write it; everything else is non-executable. A native dev process gets the whole arc RWX. The arc returns to writable data on free (arc_free → arc_reset).
    crate::cachemap::set_arc_wx(arc.base, arc.block, arc.base + CODE, arc.base + CODE + exec_len, !masked, bti);
    // The grants page read-only (the handle table, the tokens, the preferences, the ihi): nothing in it is the process's to write.
    crate::cachemap::grants_page(arc.base, false);
    // The stack's guard: the page under it (`floor`, checked above to lie past the image, its tail and the input).
    crate::cachemap::guard_pages(floor, STACK_GUARD, true);
    // The frame a queued start resumes into: exactly what `ferros_el0_enter` sets up (entry, stack at the arc's top, the arc's base and mask in x28/x27 for masked code, grants/input/length in x0-x2, EL0t with interrupts open).
    let mut frame = zero;
    frame.elr = (arc.base + CODE) as u64;
    frame.sp_el0 = (arc.top() - 16) as u64;
    frame.spsr = 0;
    frame.x[28] = arc.base as u64;
    frame.x[27] = arc.x27() as u64;
    // Fixed mode's address register starts at the base: its bits above K are the base's for good, since only `bfi x26, xN, #0, #K` writes it.
    frame.x[26] = arc.base as u64;
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
    let (me, _, _) = build(name, &g.code, b"", objects, Some(ihi), g.masked, g.text_len, g.fixed | (g.bti as u32) << 8 | (g.exact as u32) << 9)?;
    enqueue_back(me);
    Ok(me)
}

/// A granted region as a handle (SANDBOX.md, "Handles"): its base and its mask, for masked code to reach it as `base + (offset & mask)`. A region is maskable when it is a power of two in size and aligned to it; an MMIO block of any other size (20 KiB, 52 KiB, 3.5 MiB) gets no handle and is reached through the kernel's mmio calls instead, never rounded up over its neighbours. A grant that is not a region, or a region that cannot be masked, is (0, 0): native code reaches it as it always has, masked code cannot.
fn handle_of(object: Object) -> (u64, u64) {
    let (base, size) = match object {
        Object::Dma(i) => (DMA[i].base, DMA[i].size),
        Object::Hardware(h) if !HARDWARE[h].via_el3 => (HARDWARE[h].base, HARDWARE[h].size),
        _ => return (0, 0),
    };
    // Exact or nothing (ISOMEM Claim 5: a grant reaches its own bytes and no neighbour's): a region the mask can bound exactly is a power of two at a base aligned to it. Rounding a 20 KiB block up to 32 KiB would hand the process the next block's registers too; such a block is reached through the kernel's mmio calls, which check each offset.
    if size == 0 || !size.is_power_of_two() || base % size != 0 {
        return (0, 0);
    }
    (base as u64, (size - 1) as u64)
}

/// `diag arcs`: every process's arc, for reading the pools' state: name, base, size, block, under the lock.
pub fn arcs_report() -> alloc::string::String {
    lock();
    let mut s = alloc::string::String::new();
    for p in k().procs.iter().flatten() {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{} base G#{:X} size G#{:X} block G#{:X} {}\n", core::str::from_utf8(&p.name).unwrap_or("?"), p.arc.base, p.arc.size, p.arc.block, if p.arc.guard != 0 { "masked" } else { "native" }));
    }
    unlock();
    s
}

/// A signed native capsule spawned with no ihi, for a device that has no ira yet (akita, storage-off until its partitions are backed up: board.rs). The gate still proves the signature over the content; only the oracle's ihi is missing, since it needs the system secret the ira gives. Masked code is refused (its seal needs that secret too), and so is everything on a production build.
pub fn spawn_capsule_no_ihi(name: &[u8], capsule: &[u8], input: &[u8], grants: &[Grant]) -> Result<Outcome, &'static str> {
    let g = crate::oracle::gate(capsule)?;
    if g.masked {
        return Err("gate: masked code needs the system secret (no ira here)");
    }
    if cfg!(ferros_production) {
        return Err("gate: production refuses unmasked native code");
    }
    spawn(name, &g.code, input, grants, None, false, g.text_len, g.fixed | (g.bti as u32) << 8 | (g.exact as u32) << 9)
}
