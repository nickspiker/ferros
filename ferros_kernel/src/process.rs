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

/// Smallest arc: 64 KiB (a masked arc's size; it still takes a whole frame).
const ARC_MIN_LOG2: u32 = 16;

/// The windows' address space (cachemap ARC_VA) as a buddy allocator: a window is a power of two at an address aligned to it, so a masked arc's base is aligned to any size it can grow to.
struct Arcs {
    /// Free blocks per size class (log2 size → block bases): up to 2^39, past the 64 GiB of windows.
    free: [Vec<usize>; 40],
    init: bool,
}

struct Global<T>(UnsafeCell<T>);
// Every access is under `LOCK` (see `lock`); the IRQ handler touches none of it.
unsafe impl<T> Sync for Global<T> {}

// ---------------------------------------------------------------- the kernel lock and the cores
//
// One lock over the kernel's shared state (KERNEL.md "Cores"): taken for the microseconds a trap, a slice hand-out or a service-loop call holds it, never across a wait or a run at EL0. Every `pub fn` here takes it; everything private assumes it is held. `LOCK_SPINS` counts contended acquisitions for DIAG.
/// The lock is a ticket lock (2026-11-18, slit-robust): a core takes the next ticket and waits until the serving word reaches it; unlock moves the serving word on. Free when the two are equal.
/// Why not test-and-set (the lock until then, the waiters spinning on a load and racing for the store once it read free): it has no order, and under Lumis's thread pool (its threads yielding every 10 us on two A715s, each yield thru the lock, 56k holds a second) the camera's frame thread's longest wait in three runs of four was 852 to 1029 us (once for a SYS_WAKE between its acknowledgements and its ack of the line) while no hold in those runs but the spawn's and one other was 666 us or longer: the others kept winning the race. Its interrupt line stayed masked meanwhile, the next frame's descriptor went unreleased, and the slit stream died on LATE_CONFIG (the Pro, b965). In ticket order a wait is at most the holds of the cores ahead, eight at most.
static TICKET_NEXT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static TICKET_SERVING: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static LOCK_SPINS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The core holding the kernel lock, plus one (0: free): lets code that runs both inside syscalls and from the service loop take the lock only when it does not already hold it (`with_lock_held_or`).
static HOLDER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// Where the holder took the lock (`site_word`) and when (the counter); the longest hold since boot and its site (DIAG LOCK_HOLD_*): the measure of what keeps the other cores waiting, and of what comes near the watchdog's second.
static LOCK_SITE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static LOCK_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static LOCK_HOLD_MAX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static LOCK_HOLD_SITE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The last holds of the kernel lock longer than 1/128 s, each [taken at (counter), held (ticks), site]: what stopped every other core, on a timeline (2026-11-12, measuring attest to the deck). `LONG_HOLD_N` counts them all.
pub const LONG_HOLDS: usize = 1 << 5;
pub static LONG_HOLD: [[core::sync::atomic::AtomicU64; 3]; LONG_HOLDS] = [const { [const { core::sync::atomic::AtomicU64::new(0) }; 3] }; LONG_HOLDS];
pub static LONG_HOLD_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Per core: the contended takes of the kernel lock and the counter ticks spent waiting for it (`diag sched`): the cores' time lost to the one lock, where the profile of 2026-11-11 put 17 % of the workers'.
pub static LOCK_WAIT: [[core::sync::atomic::AtomicU64; 2]; 16] = [const { [const { core::sync::atomic::AtomicU64::new(0) }; 2] }; 16];

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
    // The order (`Rank`): a core holding a storage lock never takes the kernel lock, or a syscall on another core holding the kernel lock and waiting for that storage lock would wait for ever.
    // Said the first time a path does it, with its place, rather than as a rare hang.
    let ranks = RANKS[core()].load(Ordering::Relaxed);
    if ranks != 0 {
        panic!("lock order: the kernel lock taken at {} while this core holds storage locks G#{ranks:X}", site_text_of(site_word(core::panic::Location::caller())));
    }
    // A ticket: the lock is taken in the order it was asked for (`TICKET_NEXT`, `TICKET_SERVING`).
    let my = TICKET_NEXT.fetch_add(1, Ordering::Relaxed);
    if TICKET_SERVING.load(Ordering::Acquire) != my {
        LOCK_SPINS.fetch_add(1, Ordering::Relaxed);
        // The wait is timed only when there is one: the uncontended take reads no counter.
        let t0 = crate::platform::cntpct();
        let mut spins = 0u32;
        // The holder this wait was behind, as last seen (devsched `irq_lock_wait`).
        let mut by = (0u64, 0u64);
        // The waiters spin on a plain load of their shared copy of the serving word; only the holder writes it, once, at its unlock.
        while TICKET_SERVING.load(Ordering::Acquire) != my {
            by = (LOCK_SITE.load(Ordering::Relaxed), crate::devsched::WHAT.load(Ordering::Relaxed));
            core::hint::spin_loop();
            // Every 4096th turn: is the holder stalled past half the watchdog's window? (platform::stall_check: one SGI to it, its place kept for the post-mortem.) WHY wrapping: a spin past 2^32 turns (minutes, a hang the watchdog ended long before) only feeds this every-4096th test, where a wrap changes nothing.
            spins = spins.wrapping_add(1);
            if (spins & 0xFFF) == 0 {
                crate::platform::stall_check();
            }
        }
        let w = &LOCK_WAIT[core()];
        let waited = crate::platform::cntpct() - t0;
        w[0].fetch_add(1, Ordering::Relaxed);
        w[1].fetch_add(waited, Ordering::Relaxed);
        // A thread running for its device's interrupt waited: the part of a device's deadline the lock took, and behind whom.
        if ON_IRQ[core()].load(Ordering::Relaxed) {
            crate::devsched::irq_lock_wait(waited, by.0, by.1);
        }
    }
    let site = site_word(core::panic::Location::caller());
    HOLDER.store(core() + 1, Ordering::Relaxed);
    LOCK_SITE.store(site, Ordering::Relaxed);
    // Not a syscall's hold until the syscall path says it is (`ferros_el0_trap`).
    crate::devsched::WHAT.store(0, Ordering::Relaxed);
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
    // Every hold past 1/128 s, the last LONG_HOLDS of them: when (the counter at the take), how long, from where (`diag`'s lock lines).
    if held > crate::platform::cntfrq() >> 7 {
        let i = LONG_HOLD_N.fetch_add(1, Relaxed) as usize % LONG_HOLDS;
        LONG_HOLD[i][0].store(LOCK_AT.load(Relaxed), Relaxed);
        LONG_HOLD[i][1].store(held, Relaxed);
        LONG_HOLD[i][2].store(LOCK_SITE.load(Relaxed), Relaxed);
    }
    // Every hold by length, by syscall or site, the long ones kept and traced (devsched.rs, `diag lock`); still holding, so the trace's ring is ours.
    if crate::devsched::hold(LOCK_AT.load(Relaxed), held, LOCK_SITE.load(Relaxed)) {
        let what = crate::devsched::WHAT.load(Relaxed);
        // WHY the min: the trace's detail is 16 bits of microseconds; a hold of 65 ms or more reads as 65535 rather than wrapping to a short one.
        tr(TR_LOCK, if what >> 63 != 0 { (what >> 16 & 0xFF) as usize } else { 0xFF }, (held * 1_000_000 / crate::platform::cntfrq()).min(0xFFFF));
    }
    HOLDER.store(0, Relaxed);
    crate::platform::note(0, 0);
    // Only the holder writes the serving word, so the add is the next ticket's turn. WHY wrapping (the atomic's own add): the tickets count round 2^32 together, and only equality is ever asked.
    TICKET_SERVING.fetch_add(1, core::sync::atomic::Ordering::Release);
    // The flash log lines this core said while it held the lock, written now that every other core may go on (flashlog.rs `deferred`).
    crate::flashlog::deferred();
}

/// A lock site word as text (`proc* line A#2033`): the four bytes of the file's name it keeps, then the line.
pub fn site_text_of(w: u64) -> alloc::string::String {
    let tag: alloc::string::String = ((w >> 24) as u32).to_le_bytes().iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '?' }).collect();
    alloc::format!("{tag}* line A#{}", w & 0xFF_FFFF)
}

// ---------------------------------------------------------------- the storage locks (2026-11-18, lock-holds)
//
// The kernel lock held the vault, the flash log and the UFS as well as the tables, so a capsule read (24 to 148 ms), the AoC's firmware (43 to 130 ms a piece) or a settings write (3.3 ms) stopped every core's syscalls for its whole length, and a camera stream at 96 a second died behind them (`diag lock`, the Pro, 2026-11-16 and 2026-11-18).
// Each of those now has a lock of its own, ranked under the kernel lock:
// - rank 1, `VAULT`: the kernel's vault (kvault's KVault: manifestus's state, the names, the seals, the seal keys' use), every read and write of it;
// - rank 2, `PART`: the partition reader's open file (partfs.rs `part_call`, the modem driver's firmware and NV);
// - rank 3, `LOG`: the flash log's ring, its pre-boot buffer and its personal key;
// - rank 4, `UFS`: the UFS controller, one command at a time with its data block and the run bounce (`UFS_RUN`), and the link's hibernate;
// - rank 5, `FENCE`: the camera front end's S2MPU fence and the region it follows (camera_power.rs).
// The order is the kernel lock, then the ranks upward.
// A core holding one never takes the kernel lock or a lower rank: `lock` and `Rank::take` panic on it, so a wrong order shows the first time its path runs, with its place, never as a rare deadlock.
// Each is taken again freely on the core that holds it (`Rank::with`), as `with_lock_held_or` is.
// What this buys: the service loop reads the vault holding `VAULT` alone, so the other cores' syscalls go on, and a syscall that needs the vault or the UFS lets the kernel lock go first (`unlocked`).
// A syscall that still takes one of them under the kernel lock waits for its holder with every other core waiting behind it: those waits are counted (`Rank::kernel_waits`, `diag lock`), the measure of what is left to move.

/// Per core: the storage locks it holds, bit r for rank r.
static RANKS: [core::sync::atomic::AtomicU32; 16] = [const { core::sync::atomic::AtomicU32::new(0) }; 16];

/// Whether this core holds a storage lock of rank `r` or above.
pub fn ranks_from(r: u32) -> bool {
    RANKS[core()].load(core::sync::atomic::Ordering::Relaxed) >> r != 0
}

/// A storage lock (above): a spinlock with its rank, its holder and its measures.
pub struct Rank {
    rank: u32,
    pub name: &'static str,
    word: core::sync::atomic::AtomicBool,
    /// The core holding it, plus one (0: free).
    holder: core::sync::atomic::AtomicUsize,
    at: core::sync::atomic::AtomicU64,
    site: core::sync::atomic::AtomicU64,
    /// Holds since boot, the longest (ticks) and where it was taken.
    pub holds: core::sync::atomic::AtomicU64,
    pub max: core::sync::atomic::AtomicU64,
    pub max_site: core::sync::atomic::AtomicU64,
    /// Waits for it made by a core holding the kernel lock: how many, ticks in all, the longest, and the site the longest waited from. Each of those ticks every other core may have waited too.
    pub kernel_waits: [core::sync::atomic::AtomicU64; 4],
}

/// For the stall record (platform.rs `stall_check`): the UFS lock's holder and where it took it, in the kernel lock note's form (bits 0-7 the core + 1, 8-31 the line, 32-63 the file's first four bytes), and the storage locks `core` holds (`RANKS`).
/// Why (2026-11-19, the Pro, b1016): a watchdog reset with core 0 silent and IRQs masked holding the kernel lock taken at syscall entry, during a RAW clip's direct writes; whether a UFS wait was behind it was not recorded.
pub fn stall_extra(core: usize) -> (u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let site = UFS.site.load(Relaxed);
    let ufs = UFS.holder.load(Relaxed) as u64 & 0xFF | site << 8;
    (ufs, RANKS[core & 15].load(Relaxed) as u64)
}

impl Rank {
    const fn new(rank: u32, name: &'static str) -> Self {
        use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
        Rank { rank, name, word: AtomicBool::new(false), holder: AtomicUsize::new(0), at: AtomicU64::new(0), site: AtomicU64::new(0), holds: AtomicU64::new(0), max: AtomicU64::new(0), max_site: AtomicU64::new(0), kernel_waits: [const { AtomicU64::new(0) }; 4] }
    }

    /// Its rank (above).
    pub fn rank(&self) -> u32 {
        self.rank
    }

    /// Whether this core holds it.
    pub fn mine(&self) -> bool {
        self.holder.load(core::sync::atomic::Ordering::Relaxed) == core() + 1
    }

    #[track_caller]
    fn take(&self) {
        use core::sync::atomic::Ordering::{Acquire, Relaxed};
        let c = core();
        let site = site_word(core::panic::Location::caller());
        // A higher rank held here: taking this one now is the wrong order (above).
        let ranks = RANKS[c].load(Relaxed);
        if ranks >> (self.rank + 1) != 0 {
            panic!("lock order: {} (rank {}) taken at {} while this core holds G#{ranks:X}", self.name, self.rank, site_text_of(site));
        }
        if self.word.compare_exchange(false, true, Acquire, Relaxed).is_err() {
            let t0 = crate::platform::cntpct();
            let mut spins = 0u32;
            // Test, then test-and-set, as the kernel lock spins.
            while self.word.load(Relaxed) || self.word.compare_exchange_weak(false, true, Acquire, Relaxed).is_err() {
                core::hint::spin_loop();
                // WHY wrapping: as the kernel lock's spin, a count past 2^32 turns only feeds the every-4096th look.
                spins = spins.wrapping_add(1);
                if (spins & 0xFFF) == 0 {
                    crate::platform::stall_check();
                }
            }
            if HOLDER.load(Relaxed) == c + 1 {
                let waited = crate::platform::cntpct() - t0;
                let w = &self.kernel_waits;
                w[0].fetch_add(1, Relaxed);
                w[1].fetch_add(waited, Relaxed);
                if waited > w[2].load(Relaxed) {
                    w[2].store(waited, Relaxed);
                    w[3].store(site, Relaxed);
                }
            }
        }
        self.holder.store(c + 1, Relaxed);
        self.site.store(site, Relaxed);
        self.at.store(crate::platform::cntpct(), Relaxed);
        RANKS[c].fetch_or(1 << self.rank, Relaxed);
    }

    fn give(&self) {
        use core::sync::atomic::Ordering::{Relaxed, Release};
        let held = crate::platform::cntpct().wrapping_sub(self.at.load(Relaxed));
        self.holds.fetch_add(1, Relaxed);
        if held > self.max.load(Relaxed) {
            self.max.store(held, Relaxed);
            self.max_site.store(self.site.load(Relaxed), Relaxed);
        }
        RANKS[core()].fetch_and(!(1 << self.rank), Relaxed);
        self.holder.store(0, Relaxed);
        self.word.store(false, Release);
    }

    /// `f` holding this lock, taken now unless this core holds it already.
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        if self.mine() {
            return f();
        }
        self.take();
        let r = f();
        self.give();
        r
    }

    /// `diag lock`'s line for it.
    pub fn report(&self) -> alloc::string::String {
        use core::sync::atomic::Ordering::Relaxed;
        let us = |t: u64| t * 1_000_000 / crate::platform::cntfrq();
        let w = &self.kernel_waits;
        alloc::format!(
            "  {}: A#{} holds, longest A#{} us at {}; waited for under the kernel lock A#{} times, A#{} us in all, longest A#{} us at {}\n",
            self.name,
            self.holds.load(Relaxed),
            us(self.max.load(Relaxed)),
            site_text_of(self.max_site.load(Relaxed)),
            w[0].load(Relaxed),
            us(w[1].load(Relaxed)),
            us(w[2].load(Relaxed)),
            site_text_of(w[3].load(Relaxed))
        )
    }

    /// `diag "lock reset"`: its measures zeroed.
    pub fn reset(&self) {
        use core::sync::atomic::Ordering::Relaxed;
        for a in [&self.holds, &self.max, &self.max_site].into_iter().chain(self.kernel_waits.iter()) {
            a.store(0, Relaxed);
        }
    }
}

/// The kernel's vault (rank 1).
pub static VAULT: Rank = Rank::new(1, "vault");
/// The partition reader (rank 2).
pub static PART: Rank = Rank::new(2, "partition files");
/// The flash log (rank 3).
pub static LOG: Rank = Rank::new(3, "flash log");
/// The UFS controller (rank 4).
pub static UFS: Rank = Rank::new(4, "ufs");
/// The camera front end's S2MPU fence (rank 5).
pub static FENCE: Rank = Rank::new(5, "camera fence");
/// Every storage lock, for `diag lock`.
pub static RANKED: [&Rank; 5] = [&VAULT, &PART, &LOG, &UFS, &FENCE];

/// From the moment the service loop lends its vault to the syscalls (service.rs `run`), other cores reach it, and every read or write of its blocks holds `VAULT` (kvault.rs `Region`, which panics otherwise). Before, the boot is one core's alone.
pub static VAULT_SHARED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Inside a syscall: `work` with the kernel lock let go and interrupts open, the lock taken back before this returns (the SYS_POWER pattern of 2026-11-16, made one).
/// What the caller must keep to: no reference into the kernel's tables taken before this (`p`, `t`, an entry of `k()`) is used after it, since another core may change or move them meanwhile (a spawn grows the process table); `work` reads and writes no process's memory (an arc can shrink or give back frames under it from another thread), only kernel buffers.
/// What holds meanwhile: the calling thread stays current on this core, so its process's arc is neither freed (only its last thread's drain frees it) nor moved (`hold_try` waits for its threads to leave the cores); its process may be killed meanwhile, and the trap drains it at its next entry as for any thread that ran on while its process ended elsewhere.
/// An interrupt taken here at EL2 marks its line pending and returns; a slice tick taken here is re-armed at the trap's end; a yield asked of this core meanwhile, or a hold of the thread's process (SGI 3 both, which parks nothing at EL2), is asked again once the interrupts are masked, so it is pending at the eret and the thread parks there as if it had come with them masked.
#[track_caller]
fn unlocked<R>(work: impl FnOnce() -> R) -> R {
    use core::sync::atomic::Ordering::Relaxed;
    let what = crate::devsched::WHAT.load(Relaxed);
    let any_before = unsafe { core::ptr::read_volatile(&raw const __irq_any) };
    unlock();
    unsafe { core::arch::asm!("msr daifclr, #2") };
    let r = work();
    unsafe { core::arch::asm!("msr daifset, #2") };
    lock();
    crate::devsched::WHAT.store(what, Relaxed);
    let c = core();
    // The same for a hold of this thread's process asked meanwhile (`hold`, an arc to move or change: SGI 3 too), which waits for the thread to leave the core and would otherwise wait for its next trap or its slice's end.
    let held = cur_get().and_then(proc_of).is_some_and(|p| p.hold.is_some());
    // And for a device line gone pending meanwhile (`__irq_any`, perhaps on this core, where it was taken at EL2 and parked nothing): with the interrupts masked it would have parked the thread at the eret, so the line's waiter was looked for at once; the thread parks there now as it would have.
    let line = any_before == 0 && unsafe { core::ptr::read_volatile(&raw const __irq_any) } != 0;
    if YIELD[c].load(Relaxed) != 0 || held || line {
        // SGI 3 to this core, as `make_room` sends it.
        unsafe { core::arch::asm!("dsb sy", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) ((c as u64) << 16) | (3u64 << 24) | 1) };
    }
    r
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
/// Per core: a yield asked of it for a device thread (devsched `first`, `make_room`), so it is asked once; cleared when the core next switches.
static YIELD: [core::sync::atomic::AtomicUsize; 16] = [const { core::sync::atomic::AtomicUsize::new(0) }; 16];
/// Per core: the thread it runs began its run for its device's interrupt (`Thread::on_irq`), readable without the lock by `lock` on the same core.
static ON_IRQ: [core::sync::atomic::AtomicBool; 16] = [const { core::sync::atomic::AtomicBool::new(false) }; 16];
/// When each core's current thread began running there (counter ticks): what `cur_set` charges to the thread it takes off (`Thread::ran`).
static mut RUN_AT: [u64; 16] = [0; 16];
/// The register grants of the process each core runs (`Grants`, null for none): what its MMIO syscall reads without the kernel lock (`mmio_fast`). Written only by `cur_set` on its own core, under the lock, together with `CURRENT`; read only by that same core.
static mut FAST: [*const Grants; 16] = [core::ptr::null(); 16];
fn cur_set(t: Option<usize>) {
    let c = core();
    let now = crate::platform::cntpct();
    unsafe {
        // The thread leaving the core is charged its run (it may be gone: ended or exited in this trap).
        if let Some(x) = (*(&raw const CURRENT))[c].and_then(th) {
            x.ran = credit(x, now) + (now - (*(&raw const RUN_AT))[c]);
            x.ran_at = now;
        }
        (*(&raw mut RUN_AT))[c] = now;
        (*(&raw mut CURRENT))[c] = t;
        (*(&raw mut FAST))[c] = t.and_then(proc_of).map_or(core::ptr::null(), |p| &*p.fast as *const Grants);
    }
    // A thread made runnable by a call, a reply or its device now runs: how long that took (`WAKE_LAT`).
    if let Some(x) = t.and_then(th) {
        let woke_at = x.woke;
        if x.woke != 0 {
            // WHY saturating: the stamp may come from another core, and a counter read is not ordered by the lock (an `mrs`, not a memory access), so it can read a tick past this one; a wrap would put 2^64 ticks in the longest wait.
            wake_count(x.wkind as usize, now.saturating_sub(x.woke));
            x.woke = 0;
        }
        x.last_core = c as u8;
        x.on_irq = x.urgent;
        x.irq_run_at = now;
        x.urgent = false;
        // A device line's waiter runs: from its interrupt to here, per line (devsched.rs, `diag dev`); the trace's event first, as this run may freeze it.
        if x.irq_line != 0 {
            x.run_line = x.irq_line;
            let i = x.irq_line as usize - 1;
            let (at, landed, noticed) = (x.irq_at, x.irq_core as usize, x.irq_noticed as usize);
            x.irq_line = 0;
            // WHY saturating: `at` is another core's counter read (see `ran`).
            tr(TR_IRQ_RUN, t.unwrap(), (now.saturating_sub(at) * 1_000_000 / crate::platform::cntfrq()).min(0xFFFF));
            crate::devsched::ran(i, IRQS[i].intid as usize, at, woke_at, now, landed, noticed, c);
        }
    }
    // A yield asked of this core is answered by whatever it runs now (devsched `first`, `ferros_el0_preempt`).
    YIELD[c].store(0, core::sync::atomic::Ordering::Relaxed);
    ON_IRQ[c].store(t.and_then(th).is_some_and(|x| x.on_irq), core::sync::atomic::Ordering::Relaxed);
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
/// How many queued threads an A510 may run (`may_run`), readable without the lock: an idle A510 looks only when this is not zero (smp.rs), so with nothing pinned there the A510s never take the lock. Counted only while an A510 is up.
pub static RUNNABLE_LITTLE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// How many queued threads the kernel's core may run (`may_run`), readable without the lock: the service loop gives out a slice only when this is not zero or a sleeper's deadline is due (service.rs), so the passes a busy worker cluster's `sev`s start do not each take the lock for a run queue that holds nothing for it (2026-11-19: 4,800 such passes a second under a Lumis save, every one taking the lock in `run_slice`).
/// Counted when the queue changes; a change of what a queued thread may run on that leaves the queue as it is (a `diag` switch, cores stopped) is seen at the queue's next change.
pub static RUNNABLE_KERNEL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
fn runnable_sync() {
    let q = &k().runnable;
    RUNNABLE_N.store(q.len(), core::sync::atomic::Ordering::Release);
    let km = kernel_mask();
    RUNNABLE_KERNEL.store(q.iter().filter(|&&t| mask_of(t) & km != 0).count(), core::sync::atomic::Ordering::Release);
    let little = if cores_up() & MASK_LITTLE == 0 { 0 } else { q.iter().filter(|&&t| mask_of(t) & MASK_LITTLE != 0).count() };
    RUNNABLE_LITTLE.store(little, core::sync::atomic::Ordering::Release);
}
/// Wake every other core: `sev` for one in `wfe` (the kernel's core, on the event stream) and SGI 1 to all others for the workers in `wfi` (smp.rs idle loop). ICC_SGI1R_EL1 with IRM (bit 40) = every core but this one.
pub fn kick(mask: u16) {
    unsafe {
        core::arch::asm!("sev");
        // Only when a worker may run it: the render thread, pinned to the X3, sat in the queue whenever the X3 was busy, and every kick for it woke four cores to find nothing (2026-10-23).
        if mask & (MASK_WORKERS | MASK_LITTLE) != 0 || mask & cores_up() == 0 {
            core::arch::asm!("msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) (1u64 << 40) | (1u64 << 24));
        }
    }
}
fn mask_of(t: usize) -> u16 {
    k().threads.get(t).and_then(|x| x.as_ref()).map(may_run).unwrap_or(0xFFFF)
}
/// The cores thread `x` may run on now, among those up: its mask (a pin to the X3 meaning the kernel's core), narrowed by the dedicated core (devsched `core`: a device thread whose mask holds it runs there alone, every other thread anywhere else it may), and every core up when none of its own is.
fn may_run(x: &Thread) -> u16 {
    let up = cores_up();
    let mask = if x.mask & MASK_BIG != 0 {
        (x.mask & !MASK_BIG) | kernel_mask()
    } else if x.mask == MASK_WORKERS && !x.device && X3_SHARE.load(core::sync::atomic::Ordering::Relaxed) {
        // `sched x3 on`: an unpinned thread may run on the kernel's core too while it is free.
        x.mask | kernel_mask()
    } else {
        x.mask
    };
    let dc = crate::devsched::CORE.load(core::sync::atomic::Ordering::Relaxed) & up;
    let m = if dc == 0 {
        mask
    } else if x.device && mask & dc != 0 {
        dc
    } else if mask & !dc & up != 0 {
        mask & !dc
    } else {
        mask
    };
    if m & up == 0 { up } else { m & up }
}
/// Whether thread `x` may run on this core.
fn runs_here(x: &Thread) -> bool {
    may_run(x) >> core() & 1 != 0
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
/// A thread onto the back of the run queue, and the idle cores that may run it told. Behind everyone, its front credit is whole again (`ran` = 0, see `enqueue_woken`).
fn enqueue_back(t: usize) {
    queue_back(t);
    kick(mask_of(t));
}
/// `enqueue_back` with no core told: for a yield, whose own core picks from the queue at once (`SYS_SLEEP`). A kick for every yield woke every idle core into the lock to find the yielder already taken (2026-11-17, rayon's idle workers yielding every few microseconds: the lock's waits on the X3 and the A715s were 6 to 9 % of their time).
fn queue_back(t: usize) {
    unpause(t);
    if let Some(x) = th(t) {
        x.ran = 0;
        x.ran_at = crate::platform::cntpct();
        // Still current here (a slice's end, a yield, a server after its reply): its run so far is counted from now, or `cur_set` would charge it to the fresh credit as it leaves.
        if cur_get() == Some(t) {
            unsafe { (*(&raw mut RUN_AT))[core()] = crate::platform::cntpct() };
        }
    }
    k().runnable.push_back(t);
    runnable_sync();
}
fn enqueue_front(t: usize) {
    unpause(t);
    k().runnable.push_front(t);
    runnable_sync();
    kick(mask_of(t));
}
/// A thread that yielded its core for the thread at the head of the queue (`ferros_el0_preempt`, asked by `make_room` or `make_way`): just behind that head, so it runs again next after it, and the woken thread its core was asked for is still the one its core picks (at the very front it was picked again itself whenever devsched `first`'s urgency did not apply).
fn enqueue_second(t: usize) {
    unpause(t);
    let q = &mut k().runnable;
    // WHY the min: the head may be gone already (another core took the woken thread), and an insert past the end panics; with the queue empty it goes to the front.
    q.insert(q.len().min(1), t);
    runnable_sync();
    kick(mask_of(t));
}

// ---------------------------------------------------------------- who runs next after a wake
//
// Three ways a woken thread joins the run queue (2026-11-11, the Pro: Mahere's forty threads kept the four workers busy, and every call to the WiFi driver that found it asleep, and every continuation of the driver after a reply, went to the back of the queue and waited ~9.7 ms for a core):
// - `handoff`: the thread that woke it blocks now on this core (a call, a recv answered at once, a reply's caller), so it runs here next, in what is left of the slice the core's timer was armed for. Nothing is taken from anyone: the slice timer is armed once per `run_slice` and not again on a switch, so a chain of hand-offs on one core spends one slice, and its end puts the thread then running at the back. Pinned elsewhere or held, it is woken as below instead.
// - `enqueue_woken`: the waker carries on (a server after its reply, an interrupt, the kernel's own call): to the front while the thread's front credit holds (`credit`: its run since it last waited at the back of the queue, leaking an eighth of the time passing, under a slice), else to the back.
// - `enqueue_back`: everything else (a deadline, a word, a spawn, a slice's end).
//
// The bound (fairness): a thread X waiting at the back is passed only by threads woken to the front, and a thread T gets there only while its spent credit is under a slice. Over X's wait W, T can run ahead of X at most a slice (its credit), plus the slice it may be in when the credit runs out, plus W/8 (the leak); once spent, T's next wake puts it behind X, and only a run from there (after X's) renews it. Hand-offs add nothing: they spend the slice of the thread that blocked, one slice per `run_slice`, and its end puts the thread then running at the back. So with N threads woken this way on C cores, W ≤ 2·N·slice / (C − N/8): bounded while those threads together could not fill the cores at an eighth each (N < 8·C, thirty-two on the four workers; ferros has about a dozen servers and drivers woken this way). The old rule (deliver and interrupt wakes always to the front) had no bound at all; the leak keeps a light thread (touch, the WiFi driver) at the front for good instead of sending it behind the queue once every slice of its work.

/// A woken thread runs here next (see above): no kick, this core takes it under the same hold of the lock. Otherwise `enqueue_woken`.
fn handoff(t: usize, kind: usize) {
    woken(t, kind);
    if place_here(t) {
        HANDOFFS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    } else {
        enqueue_woken(t, kind);
    }
}
/// Thread `t` to the front with no kick, when this core will take it at its next pick (allowed here, its process not held); false, nothing done, otherwise.
fn place_here(t: usize) -> bool {
    let kk = k();
    let here = th(t).is_some_and(|x| runs_here(x) && !kk.procs.get(x.proc).and_then(|p| p.as_ref()).is_some_and(|p| p.hold.is_some()));
    if here {
        unpause(t);
        kk.runnable.push_front(t);
        runnable_sync();
    }
    here
}
/// A woken thread to the front while it has front credit left (`ran` under a slice), else to the back (see above). True when it went to the front.
fn enqueue_woken(t: usize, kind: usize) -> bool {
    woken(t, kind);
    let slice = crate::platform::cntfrq() / 1000 * SLICE_MS;
    // A thread still current here (a server queued after its reply) has its run so far counted too: `cur_set` charges it only as it leaves.
    let (c, now) = (core(), crate::platform::cntpct());
    let now_run = if cur_get() == Some(t) { now - unsafe { (*(&raw const RUN_AT))[c] } } else { 0 };
    if th(t).is_some_and(|x| credit(x, now) + now_run < slice) {
        enqueue_front(t);
        // A light thread a call, a word or its deadline woke takes a busy worker's core if none is idle (`make_way`); an interrupt's waiter has devsched's `make_room`, and a server going on after its reply or a caller after it are mid-exchange on a core already.
        if kind == WAKE_CALL || kind == WAKE_WORD || kind == WAKE_TIMER {
            make_way(t);
        }
        true
    } else {
        SPENT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        enqueue_back(t);
        false
    }
}

/// A device thread `t` woken by its line `line` (IRQS index) with front credit left, under devsched `first`: if no core it may run on is idle and the interrupt did not land on one of them (an interrupt taken at EL0 preempts what runs there already), the first such core running a compute thread, from the core `t` last ran on, is asked to yield (SGI 3, `ferros_el0_preempt`: its thread keeps its place at the front, behind `t`, which `pick_runnable` takes first). A core running a device thread is never asked. Under the kernel lock.
fn make_room(t: usize, line: usize, landed: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    let Some(x) = th(t) else { return };
    let m = may_run(x);
    let cur = unsafe { &*(&raw const CURRENT) };
    // The landing core is not taken as free: an interrupt it took in the kernel (its idle look, a syscall) preempted nothing, and it may go on to run a compute thread (2026-11-16, the dedicated core: 25 runs over 5 ms behind the camera's own server thread). One that preempted its thread shows that core idle here, or asks it again at no more cost than a second trip thru the scheduler.
    let _ = landed;
    if (0..16).any(|c| m >> c & 1 != 0 && cur[c].is_none()) {
        return;
    }
    let start = x.last_core as usize;
    for j in 0..16 {
        // The cores from the last one it ran on, round the sixteen a mask names.
        let c = (start + j) & 15;
        let Some(r) = cur[c] else { continue };
        if m >> c & 1 == 0 || th(r).is_none_or(|y| y.on_irq && !over_budget(r, y)) || YIELD[c].load(Relaxed) != 0 {
            continue;
        }
        YIELD[c].store(1, Relaxed);
        crate::devsched::LAT[line][9].fetch_add(1, Relaxed);
        tr(TR_YIELD_ASK, t, c as u64);
        // SGI 3 to core c, as `hold` sends it.
        unsafe { core::arch::asm!("dsb sy", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) ((c as u64) << 16) | (3u64 << 24) | 1) };
        return;
    }
}

/// `diag "sched old on|off"`: the scheduler as before 2026-11-17 (KERNEL.md Order item 8), for measuring the two in adjacent runs on one boot: every yield under the lock and kicking every core, futex and deadline wakes to the back, no `make_way`, deadlines noticed only between slices and idle workers on their event stream, a thread asked to yield kept at the very front and only under devsched `first`. Off by default.
pub static OLD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// `diag "sched x3 on|off"`: a thread on the workers' default mask (not pinned, not a device thread) may run on the kernel's core too, when it is free; a thread pinned there (the render thread) woken while one runs there has it yield (`make_way`), as an interrupt's waiter has under devsched `first` (`make_room`). Off by default: measured 2026-11-17, the X3 sat ~60 % idle thru a Photon pinch while the four A715s were saturated.
pub static X3_SHARE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// `diag "sched slice <n>"`: the slice timer at 2^-n s (`slice_ticks`); 0, the default, is `SLICE_MS`. Only the timer: the front credit stays a `SLICE_MS` (`enqueue_woken`), so the fairness bound above reads as before. Measured 2026-11-17: with eight compute threads (rayon's four and the display's four publish workers) on four cores, a parallel section's last worker waited out a whole 10 ms slice of another's.
pub static SLICE_LOG2: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
fn slice_ticks() -> u64 {
    match SLICE_LOG2.load(core::sync::atomic::Ordering::Relaxed) {
        0 => crate::platform::cntfrq() / 1000 * SLICE_MS,
        n => crate::platform::cntfrq() >> n,
    }
}
/// A woken thread's least claim on a core already running another (`make_way`): that thread has run at least 2^-11 s (about half a millisecond) of its turn and longer than the woken one's front credit.
const WAY_RUN_LOG2: u32 = 11;
/// Per core asked: how many times `make_way` asked it to yield, for `diag sched`.
pub static WAY_ASKS: [core::sync::atomic::AtomicU64; 16] = [const { core::sync::atomic::AtomicU64::new(0) }; 16];
/// `make_way`'s looks: [0] light threads woken by a call, a word or a deadline, [1] of those that found no worker idle, [2] of those with no thread long enough in its turn to ask.
pub static WAY_LOOKS: [core::sync::atomic::AtomicU64; 3] = [const { core::sync::atomic::AtomicU64::new(0) }; 3];
/// Thread `t`, woken to the front by a word, a deadline or a call (`enqueue_woken`, its front credit unspent): when no worker it may run on is idle, the worker whose thread has run longest in its turn is asked to yield for it (SGI 3, as `make_room` asks for a device's thread), so a light thread never waits out a busy thread's slice (2026-11-17, Photon's scroll: with rayon's four workers computing a whole 10 ms slice on the four A715s, the display's receive thread, the touch server and the map waited 2.4 to 4.5 ms, and every futex wake that found the workers busy waited up to a slice).
/// Never asked: the X3 (the render thread's core and the kernel's), the waker's own core, a thread of the woken one's own process (a parallel section's threads share the work, and the woken one taking its sibling's core gains nothing), a device thread inside its interrupt's run, a core already asked, a thread that has run less than `WAY_RUN_LOG2` or less than the woken thread's spent credit (so two light threads never take turns preempting each other). The thread asked keeps its place just behind the head of the queue (`ferros_el0_preempt`). Under the kernel lock.
fn make_way(t: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    if OLD.load(Relaxed) {
        return;
    }
    let Some(x) = th(t) else { return };
    let m = may_run(x);
    let cur = unsafe { &*(&raw const CURRENT) };
    WAY_LOOKS[0].fetch_add(1, Relaxed);
    if (0..16).any(|c| m >> c & 1 != 0 && cur[c].is_none()) {
        return;
    }
    WAY_LOOKS[1].fetch_add(1, Relaxed);
    let now = crate::platform::cntpct();
    let mut best = None;
    let mut longest = credit(x, now) + (crate::platform::cntfrq() >> WAY_RUN_LOG2);
    for c in 0..16 {
        // Not this core: its thread is the waker, in the kernel now, and if it blocks next this core takes the woken thread itself; an SGI to itself would land on whatever it switched to.
        if (m & (MASK_WORKERS | MASK_LITTLE | kernel_mask())) >> c & 1 == 0 || YIELD[c].load(Relaxed) != 0 || c == core() {
            continue;
        }
        let Some(r) = cur[c] else { continue };
        let Some(y) = th(r) else { continue };
        // The kernel's core: only a visitor there (`sched x3 on`, a thread not pinned to it) is asked, and for a thread pinned there (the render thread) whatever its process and however briefly the visitor has run: the core is the pinned threads' first.
        let x3 = 1u16 << c == kernel_mask();
        if x3 && y.mask & MASK_BIG != 0 {
            continue;
        }
        if x3 && x.mask & MASK_BIG != 0 {
            best = Some(c);
            break;
        }
        if y.proc == x.proc || (y.on_irq && !over_budget(r, y)) {
            continue;
        }
        // WHY saturating: `RUN_AT[c]` is core c's counter read, which this core's read is not ordered against (see `cur_set`).
        let run = now.saturating_sub(unsafe { (*(&raw const RUN_AT))[c] });
        if run >= longest {
            longest = run;
            best = Some(c);
        }
    }
    let Some(c) = best else {
        WAY_LOOKS[2].fetch_add(1, Relaxed);
        return;
    };
    YIELD[c].store(1, Relaxed);
    WAY_ASKS[c].fetch_add(1, Relaxed);
    tr(TR_YIELD_ASK, t, c as u64);
    // SGI 3 to core c, as `make_room` and `hold` send it.
    unsafe { core::arch::asm!("dsb sy", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) ((c as u64) << 16) | (3u64 << 24) | 1) };
}

/// The core a line its thread `x` now waits on is routed to (SYS_IRQ_WAIT, the thread on this core): the kernel's core, as since 2026-11-13, unless devsched `follow` or `core` is on; then the one core `x` may run on when it has one (its pin, or the dedicated core), else this core when `x` may run here (where it waits from, so it likely runs here next), else the kernel's.
/// A worker switched off (the dark profile, a reload) takes its lines home first (`irq_routes_home`), so no line is left routed to a core that is off.
/// An armed line (irqarm.rs) follows its waiter whatever `follow` says (2026-11-18, slit-robust, the Pro): its deadline is a frame's blanking, and on the kernel's core its waiter was noticed only when the service loop's pass came round to `run_slice` (876 us behind the battery gauge's poll, 6 ms once) whenever every A715 ran Lumis; landing on the waiter's core, the interrupt parks what runs there and that core runs the waiter at once.
fn irq_route(x: &Thread, intid: u32) -> usize {
    use core::sync::atomic::Ordering::Relaxed;
    let home = kernel_mask().trailing_zeros() as usize;
    let follow = crate::devsched::FOLLOW.load(Relaxed) || crate::irqarm::armed(intid);
    if !follow && crate::devsched::CORE.load(Relaxed) == 0 {
        return home;
    }
    let m = may_run(x);
    if m.count_ones() == 1 {
        m.trailing_zeros() as usize
    } else if follow && m >> core() & 1 != 0 {
        core()
    } else {
        home
    }
}

/// Every granted line routed back to the kernel's core: before workers go off (smp `stop_all`), since a line routed to a core that is off is never delivered (killswitch.rs `enable_spi`). Only the routing register is written; whether a line is enabled is left as it is.
pub fn irq_routes_home() {
    let home = kernel_mask().trailing_zeros() as usize;
    for l in IRQS {
        crate::killswitch::route_spi(l.intid, home);
    }
}

/// A run for a device's interrupt is protected from yields (`make_room`) for 2^10 us (devsched `FIRST_BUDGET_US`; the camera's answer measured 100 to 340 us): past it the thread runs on but loses the priority of that wake, and the flash log says so once a boot.
fn over_budget(t: usize, y: &Thread) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let budget = crate::platform::cntfrq() * crate::devsched::FIRST_BUDGET_US / 1_000_000;
    // WHY saturating: the stamp is another core's counter read (`cur_set` on that core), which this read is not ordered against.
    let over = crate::platform::cntpct().saturating_sub(y.irq_run_at) > budget;
    if over && !crate::devsched::BUDGET_SAID.swap(true, Relaxed) {
        crate::flashlog::line(&alloc::format!("dev first: {} ran past its 2^10 us on its device's interrupt; it runs on without the priority (said once a boot)", alloc::string::String::from_utf8_lossy(&thread_name(t))));
    }
    over
}

/// A device line's interrupt taken for its waiter `x` (`wake_sleepers`): when it came and on which core (`.Lirq`'s stamps), which line, and the core noticing it now, for the measure of its run (`cur_set`). The stamps were written before the pending byte this caller just took; the fence orders the reads after that take.
fn irq_stamp(x: &mut Thread, intid: usize) {
    core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
    let line = IRQS.iter().position(|l| l.intid as usize == intid);
    x.irq_line = line.map_or(0, |i| i as u8 + 1);
    unsafe {
        x.irq_at = core::ptr::read_volatile(&raw const __irq_at[intid]);
        x.irq_core = core::ptr::read_volatile(&raw const __irq_core[intid]);
    }
    x.irq_noticed = core() as u8;
}
/// The leak of the front credit: an eighth of the time passing (2^3). A thread that runs less than an eighth of a core over time never spends its credit, so a driver woken by its device (touch, a few % of a core) or a server woken by its callers is never sent behind the queue for its work; one that runs more spends it a slice later.
const CREDIT_LEAK_LOG2: u32 = 3;
/// Thread `x`'s spent credit as of `now`: `ran` less the leak since `ran_at`. WHY saturating: a leaky bucket empties to nothing, never below (a long sleep is not credit in advance); and the leak's time is taken saturating too, as `ran_at` may come from another core's counter read, which the lock does not order (`cur_set`).
fn credit(x: &Thread, now: u64) -> u64 {
    x.ran.saturating_sub(now.saturating_sub(x.ran_at) >> (CREDIT_LEAK_LOG2 - x.device as u32 * DEVICE_LEAK_MORE))
}
/// A device thread's credit leaks a quarter of the time passing, not an eighth (2026-11-18, slit-robust, the Pro): the camera's frame thread answers a slit every 812 us, 60 to 100 us of a core with its syscalls and up to 323 us now and then, about an eighth; its credit ran out after seconds of Lumis drawing on every A715, its next interrupt's wake went to the back of the queue (not urgent, no core made for it), it ran 2.8 to 8 ms late with its line masked, and the next frame's descriptor went unreleased (LATE_CONFIG). Under a quarter of a core a driver keeps the front for good; the bound above holds with 4 in place of 8 (N under 4·C device threads, sixteen on the four workers).
const DEVICE_LEAK_MORE: u32 = 1;

/// The wake's stamp, for its wake-to-run time (`cur_set`).
fn woken(t: usize, kind: usize) {
    if let Some(x) = th(t) {
        x.woke = crate::platform::cntpct();
        x.wkind = kind as u8;
    }
}

/// The kinds of wake `WAKE_LAT` measures: a server by a call; a caller by its reply; a server going on after its reply; an interrupt's waiter; a sleeper or a timed wait at its deadline (measured from the deadline, so the time to notice it counts); a word's waiter (`wake`).
pub const WAKE_CALL: usize = 0;
pub const WAKE_REPLY: usize = 1;
pub const WAKE_AFTER_REPLY: usize = 2;
pub const WAKE_IRQ: usize = 3;
pub const WAKE_TIMER: usize = 4;
pub const WAKE_WORD: usize = 5;
pub const WAKE_KINDS: usize = 6;
pub const WAKE_NAMES: [&str; WAKE_KINDS] = ["call", "reply", "after reply", "interrupt", "deadline", "word"];
/// Wake to run, per kind (`diag sched`): [0] wakes, [1] ticks from the wake to the run in all, [2] the longest.
pub static WAKE_LAT: [[core::sync::atomic::AtomicU64; 3]; WAKE_KINDS] = [const { [const { core::sync::atomic::AtomicU64::new(0) }; 3] }; WAKE_KINDS];
/// The same as a histogram: bucket b holds waits of [2^b, 2^(b+1)) ticks (b = 0 also 0 and 1 tick).
pub static WAKE_HIST: [[core::sync::atomic::AtomicU64; 64]; WAKE_KINDS] = [const { [const { core::sync::atomic::AtomicU64::new(0) }; 64] }; WAKE_KINDS];
/// Hand-offs (a woken thread run next on the waker's core), and woken threads sent to the back for a spent credit (`enqueue_woken`).
pub static HANDOFFS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Device threads parked mid-run for their interrupt by another line's interrupt and kept at the front (`ferros_el0_preempt`).
pub static PREEMPT_KEPT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Yields (`sleep(0)` and past deadlines, SYS_SLEEP): [0] answered without the lock (nothing queued, `ferros_el0_trap`), [1] under it with nothing queued this core may run, [2] that gave the core to another thread, [3] that queued and took the core back.
pub static YIELDS: [core::sync::atomic::AtomicU64; 4] = [const { core::sync::atomic::AtomicU64::new(0) }; 4];
pub static SPENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
fn wake_count(kind: usize, ticks: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let w = &WAKE_LAT[kind];
    w[0].fetch_add(1, Relaxed);
    w[1].fetch_add(ticks, Relaxed);
    w[2].fetch_max(ticks, Relaxed);
    // The bucket by the top bit, branchless: 63 - clz(ticks | 1) is 0..=63.
    WAKE_HIST[kind][63 - (ticks | 1).leading_zeros() as usize].fetch_add(1, Relaxed);
}


/// The windows' free address space (cachemap ARC_VA).
static ARCS: Global<Arcs> = Global(UnsafeCell::new(Arcs { free: [const { Vec::new() }; 40], init: false }));

fn arcs() -> &'static mut Arcs {
    let a = unsafe { &mut *ARCS.0.get() };
    if !a.init {
        // The windows' space as aligned power-of-two chunks, largest first (64 GiB: one chunk).
        let (p0, p1) = crate::cachemap::ARC_VA;
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

fn buddy_take(a: &mut Arcs, block: usize) -> Option<usize> {
    let k = block.trailing_zeros() as usize;
    let mut j = k;
    while j < a.free.len() && a.free[j].is_empty() {
        j += 1;
    }
    if j >= a.free.len() {
        return None;
    }
    let base = a.free[j].pop()?;
    while j > k {
        j -= 1;
        a.free[j].push(base + (1 << j));
    }
    Some(base)
}

/// A free window of `block` bytes at `base` back to the windows' space, merged with its buddies while they are free.
fn buddy_give(base: usize, block: usize) {
    let a = arcs();
    let (p0, p1) = crate::cachemap::ARC_VA;
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

/// An arc (ARCS.md, built 2026-11-11): a window of addresses (`window` bytes at `base`, a power of two aligned to its size, in cachemap ARC_VA) of which the first `mapped` bytes are 2 MiB frames from the pool (frames.rs), wherever those lie, and the first `size` of those the process's. Before 2026-11-11 an arc was a physically contiguous power-of-two block of one of two boot-time pools, at its own physical address.
#[derive(Clone, Copy)]
pub struct Arc {
    pub base: usize,
    /// What the process may use: [base, base + size). Masked arcs a power of two (the mask x27 describes, the alignment the base gives); native arcs a whole number of frames.
    pub size: usize,
    /// The frames mapped: `size` rounded up to whole frames. Past `size`, the last frame's pages are kernel-only (a masked arc's guard lies there or in the unmapped rest of the window).
    pub mapped: usize,
    /// A masked arc's guard above it (GUARD bytes at base + size that no process reaches), 0 for a native arc.
    pub guard: usize,
    /// Exact mode's limit (SANDBOX.md, "Modes"): checked code reaches [base, base + limit); 0 in the other modes, whose x27 is the mask.
    pub limit: usize,
    /// The most `size` may grow to (SYS_ARC): the image's whole declared heap counted (`image_tail_ceiling`), a power of two for a masked arc; `size` itself for one that never grows (fixed mode, an image without ferros_rt's growing heap).
    pub ceiling: usize,
    /// The window's whole span: the ceiling plus the guard, to a power of two. Unmapped past `mapped`.
    pub window: usize,
}

/// The guard above a masked arc: the reach of an immediate offset past its last byte (SANDBOX.md: 65520 + 15).
const GUARD: usize = 64 << 10;

impl Arc {
    /// What a process's x27 holds: exact mode's limit, or the mask (a native arc's size less one: ferros_rt reads its top from it, `start.rs`).
    pub fn x27(&self) -> usize {
        if self.limit != 0 { self.limit } else { self.size - 1 }
    }
    /// The top of what the process may use (its first stack is just under it): the limit in exact mode, the arc's end otherwise.
    pub fn top(&self) -> usize {
        self.base + if self.limit != 0 { self.limit } else { self.size }
    }
    /// The 8-byte word a syscall names at arc offset `off` (rounded down to 8), None when it is not inside the arc. Was `base + (off & mask)` while every arc was a power of two; a native arc is any number of frames now, where the mask would not keep an offset inside it.
    pub fn word(&self, off: u64) -> Option<usize> {
        // WHY: the offset is the process's; a word past the arc's size lies in kernel-only pages or an unmapped slot of its window, where the kernel's own access would fault at EL2. size >= 64 KiB, so the subtraction cannot wrap.
        let o = off & !7;
        (o <= self.size as u64 - 8).then_some(self.base + o as usize)
    }
}

/// An arc's size for `bytes` in its mode: a masked arc a power of two (64 KiB at least, as its mask needs), a native arc whole frames.
fn arc_shape(bytes: usize, masked: bool) -> usize {
    if masked { bytes.max(1 << ARC_MIN_LOG2).next_power_of_two() } else { bytes.max(1).next_multiple_of(crate::frames::FRAME) }
}

/// The frames an arc of `bytes` in its mode takes at its making (`arc_alloc_room`): what a caller without the kernel lock has made clean first (frames.rs `prepare`), so the take under it zeroes none.
pub fn arc_frames(bytes: usize, masked: bool) -> usize {
    arc_shape(bytes, masked).div_ceil(crate::frames::FRAME)
}

/// An arc of at least `bytes` that never grows (`arc_alloc_room`).
pub fn arc_alloc(bytes: usize, masked: bool) -> Option<Arc> {
    arc_alloc_room(bytes, bytes, masked)
}

/// An arc of at least `bytes`, zeroed, in a window of its own with room to grow to `ceiling` (SYS_ARC): masked arcs a power of two with the guard above, native arcs whole frames. None when the pool has too few free frames, which need not lie together anywhere.
pub fn arc_alloc_room(bytes: usize, ceiling: usize, masked: bool) -> Option<Arc> {
    let size = arc_shape(bytes, masked);
    // WHY the max: a ceiling under the size asked for is a caller's sum that left something out (a staged capsule bigger than its image); the arc never starts above its own ceiling.
    let ceiling = arc_shape(ceiling, masked).max(size);
    let guard = if masked { GUARD } else { 0 };
    // The window: a power of two that holds the ceiling and the guard, so a masked arc's base is aligned to any size it grows to and every address past its guard is unmapped.
    let window = (ceiling + guard).next_power_of_two().max(crate::frames::FRAME);
    if crate::frames::free() < size.div_ceil(crate::frames::FRAME) {
        return None;
    }
    let base = buddy_take(arcs(), window)?;
    let mut arc = Arc { base, size: 0, mapped: 0, guard, limit: 0, ceiling, window };
    // Counted free above, under the same lock: the frames are there, and the window goes back if they were not.
    if arc_resize(&mut arc, size).is_err() {
        buddy_give(base, window);
        return None;
    }
    Some(arc)
}

/// Make the arc's size `size` (a shape its mode takes, at most its ceiling): frames mapped for every slot it now reaches, from anywhere in the pool, and given back from every slot it no longer does; inside its last frame the pages up to `size` the process's, those past it kernel-only (a masked arc's guard). Err(ARC_WHY_MEMORY) when the pool has too few free frames: nothing changed then. Every core's TLB forgets what was taken away before its frames are wiped and handed on.
fn arc_resize(arc: &mut Arc, size: usize) -> Result<(), u64> {
    use crate::frames::FRAME;
    let mapped = size.next_multiple_of(FRAME);
    if mapped > arc.mapped && crate::frames::free() < (mapped - arc.mapped) / FRAME {
        return Err(ARC_WHY_MEMORY);
    }
    // The old last frame's pages between the two sizes change hands (a masked arc smaller than a frame): the process's when growing, kernel-only when shrinking.
    let part = if arc.mapped < mapped { arc.mapped } else { mapped };
    if size > arc.size && part > arc.size {
        crate::cachemap::pages_open(arc.base + arc.size, part - arc.size, true);
    }
    if size < arc.size && part > size {
        crate::cachemap::pages_open(arc.base + size, part - size, false);
    }
    let mut at = arc.mapped;
    while at < mapped {
        // Counted free above, under the same lock: every take succeeds.
        let f = crate::frames::take().unwrap();
        // The bytes of this frame that are the process's: all of it but in the last frame of a masked arc smaller than a frame.
        let usable = if size - at < FRAME { size - at } else { FRAME };
        crate::cachemap::map_slot(arc.base + at, crate::frames::pa(f), crate::frames::table(f), usable);
        at += FRAME;
    }
    crate::cachemap::mapped();
    if mapped < arc.mapped {
        let mut gone = Vec::with_capacity((arc.mapped - mapped) / FRAME);
        let mut at = mapped;
        while at < arc.mapped {
            if let Some(t) = crate::cachemap::slot_table(arc.base + at) {
                gone.push(crate::frames::of_table(t));
                crate::cachemap::set_slot(arc.base + at, 0);
            }
            at += FRAME;
        }
        crate::cachemap::unmapped(arc.base + mapped, arc.mapped - mapped);
        for f in gone {
            crate::frames::give(f);
        }
    }
    arc.size = size;
    arc.mapped = mapped;
    Ok(())
}

/// Arcs returned to the pools so far: a spawn refused for want of room is tried again only once this has moved (service.rs, the stower after attest), never on a timer.
pub static ARC_FREES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Return an arc's frames to the pool and its window to the windows' space. Whatever the process held is gone with it: its frames are wiped as they go back (owner, 2026-11-08: every end is one shot dead, no cleanup; the arc was zeroed only when handed out again until then).
pub fn arc_free(arc: Arc) {
    ARC_FREES.fetch_add(1, core::sync::atomic::Ordering::Release);
    let mut taken = Vec::with_capacity(arc.mapped / crate::frames::FRAME);
    let mut at = 0;
    while at < arc.mapped {
        if let Some(t) = crate::cachemap::slot_table(arc.base + at) {
            taken.push(crate::frames::of_table(t));
            crate::cachemap::set_slot(arc.base + at, 0);
        }
        at += crate::frames::FRAME;
    }
    // Every core forgets the window before its frames are wiped and handed on.
    crate::cachemap::unmapped(arc.base, arc.mapped);
    for f in taken {
        crate::frames::give(f);
    }
    buddy_give(arc.base, arc.window);
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
    /// A surface to be: a spawn turns it into one free slot of `surfaces` with frames of its own behind it, the child's (`Dma` of a `surface`), at build. Never held: a parent that passes the whole `surfaces` region passes this instead (SYS_SPAWN), and `bridge spawn --dma surface` asks for it by name.
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
    /// Served by the kernel: the right to launch a kist (KISTS.md): `SYS_SPAWN` in kist mode, naming a capsule by its vault address and handing each declared want its right or an empty slot; the implicit set (ferros_kist::IMPLICIT) the kernel hands every kist itself. The kernel makes each right's object itself (`kist_object`), so the holder needs no other grant to hand out; it holds every right a kist may be handed, and is init's alone.
    Kists,
    /// Served by the kernel: the right to stow a kist (SANDBOX.md "Lifecycle"): `SYS_SPAWN` in stow mode names a capsule the holder stored in `capsules.`, and the kernel reads it from the vault, gates it, runs the sandbox checker over its masked code and seals it into the `sealed` list, telling the holder each stage on a word in its own arc (`stow_request`). The stower's (payloads/kists), never a kist's.
    Stow,
    /// A slot left empty: a want the person said no to (owner, 2026-11-08: "a denied right is an empty slot, never a fake"). Its token is zeros and nothing looks it up.
    Empty,
    /// A GPU block to be (GPU-KISTS.md plan step 1, 2026-11-12; frames of the pool since 2026-11-13): a spawn turns it into a block of 2^n bytes of the blocks' window, the child's own (`Dma` of a `gpu-block`), its frames zeroed as they are taken (`block_take`), or the largest it can give down to 32 MiB (`BLOCK_LEAST_LOG`); an empty slot when none is free. Never held, as `NewSurface` is not: a parent that passes the whole window (`arena`) passes a 32 MiB one of these instead (SYS_SPAWN). Its holder asks for more by SYS_DMA x1 = 2.
    NewRegion(u8),
    /// A second surface to be, for a kist's GPU mailbox (GPU-KISTS.md 1.4): a spawn turns it into a free surface when the child already got its window's and another stays free for the next app's window; an empty slot otherwise.
    SpareSurface,
    /// A puck's square to be (docs/PUCKS.md, 2026-11-18): a spawn turns it into a free slot of the pucks' window with no frames yet, the child's own (`Dma` of a `puck`, `puck_take`); it asks for its frames by SYS_DMA x1 = 6. Handed to a kist that declares `puck` in its surface's place.
    NewPuck,
    /// Served by the kernel: a RAW clip's extent of the `clips` window (index into `EXTENT_GRANTS`), given by the vault process to the process whose extent it opened (SYS_BLOCK x1 bit 3), written and read by that process alone by block number within it (docs/RAW-VIDEO.md "The recorder": the clip's bytes go straight to the flash, not thru the vault process).
    Extent(usize),
    /// Served by the kernel, through the vault process: the hold (docs/STORE.md, 2026-11-19): items kept for the person in their items domain, of the kinds the holder's right names (`Process::items`), every call stamped with who calls and those kinds (`items_forward`), so the vault never takes a kist's word for what it may open.
    Items,
}

/// What a holder of `Object::Items` may do (ferros_kist `ItemsDecl`): the kinds it makes and opens as its capsule declares them, or any kind (`any`: a development spawn's `--items`, the bench's).
pub struct ItemRight {
    pub any: bool,
    pub makes: Vec<[u8; 32]>,
    pub opens: Vec<[u8; 32]>,
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
        Object::Stow => (22, 0),
        Object::NewRegion(n) => (23, n as u64),
        Object::SpareSurface => (24, 0),
        Object::NewPuck => (25, 0),
        Object::Extent(i) => (26, i as u64),
        Object::Items => (27, 0),
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
    /// Its SYS_ARC waits for the rest of its process to leave the cores (`hold`): the masked arc's mask or limit changes in every thread at once.
    Held,
}

/// A thread: what the scheduler runs. A saved frame, a state, and the process it belongs to; threads of one process share its arc and every capability in it (KERNEL.md "Threads, interrupts and cores"). Threads are not addressable: nothing outside the kernel names one.
struct Thread {
    proc: usize,
    frame: Frame,
    state: State,
    /// Which cores may run it (bit = core index): the workers by default; a thread pins itself with `SYS_PIN` (the display's render thread to the X3). If none of its cores is up, any core takes it.
    mask: u16,
    /// When its last forwarded store call went to the vault process (`store_forward`), for `STORE_GETS`, and what it was: the op (`VOP_*`) in bits 0-6, a set's `more` bit 7, the length in bits 8-31, the offset above.
    called: u64,
    what: u64,
    /// Its front credit spent (`enqueue_woken`), in counter ticks: what it has run on any core since it last went to the back of the run queue (`cur_set` charges it), leaking an eighth of the time passing (`credit`); at a slice it is spent. And when it was last brought up to date.
    ran: u64,
    ran_at: u64,
    /// When a wake that `WAKE_LAT` measures made it runnable (0: none pending), and which kind (`WAKE_*`).
    woke: u64,
    wkind: u8,
    /// The interrupt that woke it, for its run's measure (devsched `ran`): when it came (`__irq_at`), the line (`IRQS` index plus one, 0 none), the core it landed on and the core that noticed it.
    irq_at: u64,
    irq_line: u8,
    irq_core: u8,
    irq_noticed: u8,
    /// A device thread: it has waited on an interrupt (SYS_IRQ_WAIT). Never asked to yield for another (devsched `first`), and the dedicated core's (devsched `core`).
    device: bool,
    /// Woken by its own device's interrupt with front credit left and not yet run: picked ahead of every other thread while devsched `first` is on.
    urgent: bool,
    /// Its run now began as an urgent one (its device's interrupt): never asked to yield for another device thread meanwhile; any other run may be (devsched `first`, `make_room`).
    on_irq: bool,
    /// The line of its run for its interrupt (`IRQS` index plus one, 0 none), kept for the run as `irq_line` is not: `ferros_el0_preempt` asks a core for it with it (`make_room`).
    run_line: u8,
    /// When its current run began (`cur_set`): an `on_irq` run's budget counts from here (`over_budget`).
    irq_run_at: u64,
    /// The core it last ran on: where a yield is asked first.
    last_core: u8,
    /// Its word wait (`State::Waiting` on an arc word) ends for a call to an endpoint it polls (SYS_WAIT x3 bit 1, `wake_server`).
    wait_call: bool,
    /// The name it gave itself (SYS_THREAD_NAME: std's thread name), its first 16 bytes, zero past the end; all zero for none. A fault names the thread by it (`fault_report`).
    label: [u8; 16],
}

/// The longest forwarded store call (`STORE_GETS[3]`): its caller's name (`pin_key`), and what it was (`Thread::what`); and the calls answered while the clock policy held each of its states (0 not yet applied: the boot's own clocks, 1 rest, 2 active, 3 dark; service.rs `clock_state`), so a slow read says whether the cores were at their reading clocks (2026-11-11).
pub static STORE_LONGEST: [core::sync::atomic::AtomicU64; 2] = [const { core::sync::atomic::AtomicU64::new(0) }; 2];
pub static STORE_BY_CLOCK: [core::sync::atomic::AtomicU64; 4] = [const { core::sync::atomic::AtomicU64::new(0) }; 4];

/// Forwarded store calls answered (`SYS_REPLY` of a `store_forward` call; owner, 2026-11-11: the display waited 283 ms on the vault's reads of its wallpaper while the vault process served a fraction of that): [0] calls, [1] bytes handed back, [2] timer ticks from the call to the vault's reply (the stamp, the wait for the vault's thread to be given a core, its serving), [3] the longest of those, [4] ticks copying the replies into the callers. `diag "store gets [reset]"`. Under the kernel lock.
pub static STORE_GETS: [core::sync::atomic::AtomicU64; 5] = [const { core::sync::atomic::AtomicU64::new(0) }; 5];

/// A process whose `SYS_PIN` is ignored (its threads stay on the workers), its name's first eight bytes little-endian, 0 for none: `diag "pin off <name>"`, `diag "pin on"` (2026-11-11), to measure on one boot whether the vault's bulk reads are faster on the X3, where each request waits for the core behind the render thread and the kernel's service pass, or on the workers, where a call is handed off on the caller's own core.
pub static PIN_OFF: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A name's first eight bytes as `PIN_OFF` keeps them.
pub fn pin_key(name: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    // WHY the min: the key is the name's first eight bytes; a shorter name is padded with zeros.
    let n = name.len().min(8);
    b[..n].copy_from_slice(&name[..n]);
    u64::from_le_bytes(b)
}
/// The A715s (cores 4-7): where processes run unless they ask otherwise.
pub const MASK_WORKERS: u16 = 0x00F0;
/// The A510s (cores 0-3, 2026-11-18): only threads that pin there run there (`ferros_sys::CORES_LITTLE`), at their cluster's own clock (service.rs `governor`).
pub const MASK_LITTLE: u16 = 0x000F;
/// The X3 (core 8): the kernel's core, for the one real compute path (the display's render thread) and otherwise idle between service-loop passes.
pub const MASK_BIG: u16 = 0x0100;
/// The kernel's core as a mask: the X3 when the kernel moved there (board::x3), else CPU0, where it stayed. "Big" means this core: a thread pinned to the X3 runs here, and the kernel-side switches (SYS_POWER) are served here. Until 2026-11-06 it was the X3 always, so with the kernel on CPU0 (akita, or husky's FERROS_X3=0) a pinned thread waited for a core that never started and every power switch was refused.
pub fn kernel_mask() -> u16 {
    if crate::board::x3() { MASK_BIG } else { 0x0001 }
}
/// The cores that are up: the kernel's, and the workers smp.rs reports (its mask is by core already: bit = MPIDR Aff1).
fn cores_up() -> u16 {
    kernel_mask() | crate::smp::UP.load(core::sync::atomic::Ordering::Relaxed) as u16
}

/// The most of a process's last console line kept for its end's flash log line (`Process::words`).
const WORDS_MAX: usize = 1 << 9;

struct Process {
    arc: Arc,
    cspace: Vec<CapEntry>,
    epoch: u64,
    /// Ended (exit, a fault, a kill) while a thread of it still ran on another core: that thread leaves at its next trap, and the last one out frees the arc.
    dying: bool,
    name: Vec<u8>,
    /// The process's console output is at the start of a line (so the next byte gets the name tag).
    line_start: bool,
    /// Its index among the console ring's writers (`ferros_console::ConRing::owner`), looked up at its first write and checked at each.
    con: u8,
    /// Its last console line (the one in progress, else the last complete one), up to `WORDS_MAX` bytes (reserved at the spawn, so a write never grows it): written to the flash log when it ends (`end_process`), so a process's own account of its death (a panic's line) survives a console drain or the reset that follows.
    words: Vec<u8>,
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
    /// A kist's gate identity as the gate found it at launch (`spawn_kist`): its developer's Ed25519 key and its option (`hp`), both public, what its alias is derived from (SYS_IRA's alias bit, FACETS.md). None for anything not launched by the kist job, so nothing else has an alias and no kist can name another's identity.
    gate: Option<([u8; 32], [u8; 32])>,
    /// A kist's facets as the person granted them at launch (docs/FACETS.md): bit 0 be you, bit 1 people. Stamped on its every call on `facets` (`facets_stamp`); 0 for anything not launched by the kist job.
    facets: u8,
    /// A kist's capsule address (its vault address, the blake3 of the signed capsule), as launched: what names it in the person's list, for the rights it is using (`CLAIMS`). None for a system process.
    capsule: Option<[u8; 32]>,
    /// The hold's right (`Object::Items`): the kinds it makes and opens, stamped on its every items call (`items_forward`). None without the right.
    items: Option<alloc::boxed::Box<ItemRight>>,
    /// The arc's top at spawn (its size, exact mode's limit): a shrink (SYS_ARC) never goes under it, so the grants page, the code, the input and the first stack stay.
    floor: usize,
    /// SYS_ARC grows and shrinks granted so far (`diag arcs`).
    grows: u32,
    shrinks: u32,
    /// Held off the cores (ARCS.md step 3): what waits for its last thread to leave (`hold_try`), and when it was asked (the pause, measured).
    hold: Option<Hold>,
    /// Moves so far (`diag arcs`).
    moves: u32,
    /// Ranges of the arc whose frames were given back while the arc kept its size (ARCS.md step 4): arc offsets, whole frames. A masked arc's power-of-two slack between its image and its first stack at spawn, and what the process gives back (SYS_ARC `ARC_RELEASE`). Unmapped: the process faults there, and no syscall buffer may lie there (`span`, `word`).
    holes: Vec<(usize, usize)>,
    /// Pages of the arc it made kernel-only itself (SYS_ARC `ARC_GUARD`): the guards under its threads' stacks, arc offsets with the permission bits each page had (a native arc's spawn-time pages are executable, grown ones not). Only these may be handed back by `ARC_UNGUARD`, and a fault in one says a stack ran past its end (`fault_report`).
    guards: Vec<(usize, u64)>,
    /// Its register grants for the MMIO syscall without the kernel lock (`mmio_fast`), in a box of its own: the Process moves when the process table grows, the box does not.
    fast: alloc::boxed::Box<Grants>,
    /// The end of its image's code, an arc offset (CODE plus the checked text of a masked image, the whole image of a native one): a return address past it ends a frame walk (`frames`).
    text_end: usize,
    /// The panic it told the kernel of (SYS_PANIC), for its end (`death_line`); None until then.
    panicked: Option<alloc::boxed::Box<Panicked>>,
}

/// A panic a process told the kernel of (SYS_PANIC, ferros_sys `panic_tell`), kept for its end: the message, where (the file as the compiler named it, the line and column), the thread, and the call chain from the call (`frames`).
struct Panicked {
    message: Vec<u8>,
    file: Vec<u8>,
    line: u32,
    column: u32,
    thread: Vec<u8>,
    frames: Vec<u64>,
}

/// The most frames a death keeps (ferros_ledger death FRAMES_MAX).
const FRAMES: usize = ferros_ledger::death::FRAMES_MAX;

/// The call chain at a trap, innermost first: `pc`, then the return address of each x29 frame record from `fp`, all as offsets into the process's arc.
/// WHY each check: x29 and the stack are the process's own to write, and a panicking or faulting process is the likeliest to have them wrong (and a function built without frame pointers uses x29 as it likes).
/// - A record is read only where `span` resolves all 16 of its bytes (the arc but the frames it gave back, or a DMA region it holds: all mapped for the kernel), and only 8-byte aligned: anywhere else the read would be of unmapped or foreign memory, a fault at EL2 under the kernel lock.
/// - Each next record must lie above the last (stacks grow down): PROOF the walk ends on any chain, a cycle included.
/// - At most FRAMES are taken (what the ledger keeps).
/// - A return address outside the image's code (`text_end`) ends it: past it the records are not frame records, and what they hold is noise.
fn frames(p: &Process, pc: u64, fp: u64) -> Vec<u64> {
    let base = p.arc.base as u64;
    let code = CODE as u64..p.text_end as u64;
    let mut out = Vec::with_capacity(FRAMES);
    out.push(pc.wrapping_sub(base));
    let mut fp = fp;
    while out.len() < FRAMES && fp & 7 == 0 {
        let Some(a) = p.span(fp.wrapping_sub(base), 16) else { break };
        // SAFETY: `span` resolved these 16 bytes inside the process's arc or a DMA region it holds, identity-mapped and the kernel's to read; `a` keeps fp's alignment (the arc and the regions start on pages).
        let (next, ret) = unsafe { (core::ptr::read_volatile(a as *const u64), core::ptr::read_volatile((a + 8) as *const u64)) };
        let r = ret.wrapping_sub(base);
        if !code.contains(&r) {
            break;
        }
        out.push(r);
        if next <= fp {
            break;
        }
        fp = next;
    }
    out
}

/// A thread's name for a death: the kernel's (`name.k`), with the name it gave itself (SYS_THREAD_NAME) in brackets when it gave one, as a fault's console line has it.
fn thread_called(t: usize) -> Vec<u8> {
    let mut n = thread_name(t);
    if let Some(x) = k().threads.get(t).and_then(|x| x.as_ref()) {
        let len = x.label.iter().position(|&c| c == 0).unwrap_or(16);
        if len != 0 {
            n.extend_from_slice(b" (");
            n.extend_from_slice(&x.label[..len]);
            n.push(b')');
        }
    }
    n
}

/// What `mmio_fast` reads of a process without the kernel lock. Made at spawn and never changed after (a process's grants are fixed at its spawn: only reply tokens come and go in its cspace, and they are never registers), but for `ended`.
struct Grants {
    /// Per grant slot (the cspace index, the token at `TOKENS + 4 + 32 * slot` in its grants page): the HARDWARE index plus one, 0 for a grant that is not a register block.
    hw: Vec<u8>,
    /// Set (SeqCst) when the process ends, before the kernel waits for any core still in a register access of it (`mmio_quiesce`).
    ended: core::sync::atomic::AtomicBool,
}

/// Per core: the `Grants` whose register access is in flight there (its address; 0 none). With `Grants::ended` this is the handshake that makes a process's end wait for its accesses (`mmio_fast`, `mmio_quiesce`).
static MMIO_BUSY: [core::sync::atomic::AtomicUsize; 16] = [const { core::sync::atomic::AtomicUsize::new(0) }; 16];
/// Register syscalls answered without the kernel lock, and those that took it (a token not presented from its grants-page slot, a write thru EL3, a process ending), for `diag sched`.
pub static MMIO_PATHS: [core::sync::atomic::AtomicU64; 2] = [const { core::sync::atomic::AtomicU64::new(0) }; 2];

struct Endpoint {
    name: Vec<u8>,
    /// Calls waiting for the server: (calling thread, message).
    queue: alloc::collections::VecDeque<(usize, Vec<u8>)>,
    /// The thread that last polled this endpoint (recv with poll): a call wakes it if it is asleep, so a server that sleeps between polls answers at once rather than at its next deadline.
    poller: Option<usize>,
    /// The process (by epoch) that last received on it, a RECV polling or not; 0: none. What CALL_LIVE asks for: that process still alive. A driver that holds the right but has not received yet (still bringing its device up) does not count, and neither does a process that only holds the right to hand it on (init, which stays alive for its keepers since 2026-11-01: counting it, a live call to a dead driver waited forever).
    server: u64,
    /// Its bit in `ON_DEMAND_WANT` plus one when its server is started on demand (`ON_DEMAND`), else 0.
    on_demand: u8,
}

struct Kernel {
    procs: Vec<Option<Process>>,
    /// The threads; every index in `runnable`, `current`, an endpoint queue, a poller or a reply capability is one of these.
    threads: Vec<Option<Thread>>,
    endpoints: Vec<Endpoint>,
    runnable: alloc::collections::VecDeque<usize>,
    /// Everything processes and the kernel sent the console, each process's lines stamped and tagged with its name: one fixed ring, overwritten oldest first (ferros_console; 2026-11-14, the `Vec` it replaced grew under a ping flood until the heap ran out and the Pro rebooted).
    console: ferros_console::ConRing,
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
/// A hold of the kernel lock past devsched's long mark: the thread whose syscall held it (255 the kernel's own), microseconds.
const TR_LOCK: u8 = 14;
/// A device line's waiter ran: the thread, microseconds from the interrupt.
const TR_IRQ_RUN: u8 = 15;
/// A core asked to yield for a device thread (devsched `first`): the thread asked for, the core.
const TR_YIELD_ASK: u8 = 16;
/// A device line's interrupt noticed (`wake_sleepers`): the waiter, the core it landed on in bits 8-15 and the INTID's low byte below.
const TR_IRQ: u8 = 17;
const TR_NAMES: [&str; 18] = ["-", "call", "deliver", "wake-poll", "reply", "sleep", "wake", "switch", "run", "preempt", "park", "poll-miss", "recv-block", "yield", "lock-held", "irq-run", "yield-ask", "irq"];

/// One scheduling event into the ring, unless the ring is frozen (devsched `FREEZE_*`: a late device thread stops it so what led there stays).
fn tr(ev: u8, who: usize, detail: u64) {
    if crate::devsched::FROZEN.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
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
            // Who has the call (2026-11-20: Lumis.2 "calling" for good after the camera ended, and nothing said on what): queued on an endpoint, by its name and whether a live process receives there, or taken by the process that holds its reply token.
            State::Calling(..) => match kk.endpoints.iter().position(|e| e.queue.iter().any(|q| q.0 == i)) {
                Some(ep) => alloc::format!("calling, queued on `{}` (A#{ep}), {}", alloc::string::String::from_utf8_lossy(&kk.endpoints[ep].name), if has_receiver(ep) { "its server alive" } else { "no server receiving" }),
                None => match kk.procs.iter().flatten().find(|p| p.cspace.iter().any(|c| matches!(c.object, Object::Reply(r) if r == i))) {
                    Some(p) => alloc::format!("calling, taken by {}{}", alloc::string::String::from_utf8_lossy(&p.name), if p.dying { " (dying)" } else { "" }),
                    None => alloc::format!("calling, held by nobody"),
                },
            },
            State::Sleeping(d) => alloc::format!("sleeping, A#{} ms left", d.saturating_sub(now) * 1000 / f),
            State::Waiting(a, d) if a < 1024 => alloc::format!("irq_wait on INTID A#{a}{}, pending {}", if d != 0 { alloc::format!(", A#{} ms left", d.saturating_sub(now) * 1000 / f) } else { alloc::string::String::new() }, unsafe { core::ptr::read_volatile(&raw const __irq_pending[a]) }),
            State::Waiting(a, d) => alloc::format!("wait on word G#{a:X}{}", if d != 0 { alloc::format!(", A#{} ms left", d.saturating_sub(now) * 1000 / f) } else { alloc::string::String::new() }),
            State::Held => alloc::format!("held: its arc changes once its process is off the cores"),
        };
        out.extend_from_slice(alloc::format!("thread A#{i} {} (process A#{}, cores G#{:X}, spsr G#{:X}, pc +G#{:X}): {st}\n", alloc::string::String::from_utf8_lossy(&thread_name(i)), t.proc, t.mask, t.frame.spsr, t.frame.elr.wrapping_sub(kk.procs.get(t.proc).and_then(|p| p.as_ref()).map(|p| p.arc.base as u64).unwrap_or(0)) & 0xFF_FFFF).as_bytes());
    }
    out.extend_from_slice(alloc::format!("runnable queue: {:?}; running per core {:?}; slices per core {:?}; lock contended A#{}\n", kk.runnable, unsafe { *(&raw const CURRENT) }, SLICES.iter().map(|s| s.load(core::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>(), LOCK_SPINS.load(core::sync::atomic::Ordering::Relaxed)).as_bytes());
    unlock();
}

/// The trace, oldest first, as text: time since the first entry in microseconds, the event, the process name and the detail. Then per call: the wait before the server took it, the server's time to the reply, and the caller's wait after the reply before it ran again — the three parts of a round trip.
/// `all`: every event the ring holds rather than the last 512 (`diag "sched all"`: rayon's yields after a parallel section fill 512 in a few milliseconds).
pub fn sched_report(out: &mut Vec<u8>, all: bool) {
    sched_counters(out);
    lock();
    sched_report_locked(out, all);
    unlock();
}

/// The scheduler's running counts since boot or the last `diag "sched reset"`, ahead of the trace: wake to run per kind of wake (how long a woken thread waited for a core: the 2026-11-11 measure was ip's calls waiting ~9.7 ms for the WiFi driver), hand-offs and spent credits, the lock's wait per core, and the register syscalls by path.
fn sched_counters(out: &mut Vec<u8>) {
    use core::sync::atomic::Ordering::Relaxed;
    let f = crate::platform::cntfrq();
    let us = |t: u64| t * 1_000_000 / f;
    out.extend_from_slice(b"wake to run (us):\n");
    for (k, w) in WAKE_LAT.iter().enumerate() {
        let n = w[0].load(Relaxed);
        if n == 0 {
            continue;
        }
        out.extend_from_slice(alloc::format!("  {:<12} A#{} wakes, mean A#{}, longest A#{};", WAKE_NAMES[k], n, us(w[1].load(Relaxed)) / n, us(w[2].load(Relaxed))).as_bytes());
        // The histogram's buckets that hold anything, each by its floor: from 2^b ticks.
        for (b, h) in WAKE_HIST[k].iter().enumerate() {
            let c = h.load(Relaxed);
            if c != 0 {
                out.extend_from_slice(alloc::format!(" >=A#{}:A#{}", ((1u128 << b) * 1_000_000 / f as u128) as u64, c).as_bytes());
            }
        }
        out.push(b'\n');
    }
    out.extend_from_slice(alloc::format!("hand-offs A#{}, woken to the back for a spent slice A#{}, device threads parked mid-run by another line kept at the front A#{}\n", HANDOFFS.load(Relaxed), SPENT.load(Relaxed), PREEMPT_KEPT.load(Relaxed)).as_bytes());
    let y: [u64; 4] = core::array::from_fn(|i| YIELDS[i].load(Relaxed));
    out.extend_from_slice(alloc::format!("yields: A#{} without the lock, A#{} with nothing queued here, A#{} to another thread, A#{} back to itself\n", y[0], y[1], y[2], y[3]).as_bytes());
    let w: [u64; 3] = core::array::from_fn(|i| WAY_LOOKS[i].load(Relaxed));
    out.extend_from_slice(alloc::format!("light wakes (call, word, deadline): A#{}, A#{} with no worker idle, A#{} of them with none to ask; ", w[0], w[1], w[2]).as_bytes());
    out.extend_from_slice(b"cores asked to make way for one:");
    for (c, a) in WAY_ASKS.iter().enumerate() {
        let n = a.load(Relaxed);
        if n != 0 {
            out.extend_from_slice(alloc::format!(" c{c} A#{n}").as_bytes());
        }
    }
    out.push(b'\n');
    out.extend_from_slice(b"lock waits per core (contended takes, us waited):");
    for (c, w) in LOCK_WAIT.iter().enumerate() {
        let n = w[0].load(Relaxed);
        if n != 0 {
            out.extend_from_slice(alloc::format!(" c{c} A#{n}/A#{}", us(w[1].load(Relaxed))).as_bytes());
        }
    }
    out.extend_from_slice(alloc::format!("\nregister syscalls: A#{} without the lock, A#{} with it\n", MMIO_PATHS[0].load(Relaxed), MMIO_PATHS[1].load(Relaxed)).as_bytes());
}

/// `diag "sched reset"`: the counters `sched_counters` shows, zeroed.
pub fn sched_reset() {
    use core::sync::atomic::Ordering::Relaxed;
    WAKE_LAT.iter().flatten().chain(WAKE_HIST.iter().flatten()).chain(LOCK_WAIT.iter().flatten()).chain(MMIO_PATHS.iter()).chain(YIELDS.iter()).chain(WAY_ASKS.iter()).chain(WAY_LOOKS.iter()).chain([&HANDOFFS, &SPENT]).for_each(|a| a.store(0, Relaxed));
    // The lock's holds and the device lines' runs with them (devsched.rs), and the trace thawed.
    crate::devsched::lock_reset();
    crate::devsched::lat_reset();
}
fn sched_report_locked(out: &mut Vec<u8>, all: bool) {
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
    let start = if all { 0 } else { entries.len().saturating_sub(512) };
    for e in &entries[start..] {
        out.extend_from_slice(alloc::format!("{:>9} c{} {:<10} {:<8} {}\n", us(e.0), e.4, TR_NAMES[e.1 as usize], name(e.2), e.3).as_bytes());
    }
}

static KERNEL: Global<Kernel> = Global(UnsafeCell::new(Kernel {
    procs: Vec::new(),
    threads: Vec::new(),
    endpoints: Vec::new(),
    runnable: alloc::collections::VecDeque::new(),
    console: ferros_console::ConRing::new(),
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
/// A thread's name for a report outside the lock (devsched.rs), the lock taken unless this core holds it.
pub fn thread_name_of(t: usize) -> Vec<u8> {
    with_lock_held_or(|| thread_name(t))
}
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
            // Not over frames the process gave back (`holes`): unmapped, the kernel's own access there would fault at EL2.
            return (!self.in_hole(off as usize, len)).then_some(self.arc.base + off as usize);
        }
        self.dma_span((self.arc.base as u64).wrapping_add(off) & !DMA_ADDR, len)
    }
    /// `len` bytes at `off` where they lie, for a copy straight into another process (a forwarded store reply of up to STORE_GET_MAX, `SYS_REPLY`): no heap copy, no zeroed buffer first. None when they resolve nowhere (kept for the trap at the syscall's end, as `read` does).
    fn view(&self, off: u64, len: usize) -> Option<&[u8]> {
        if len == 0 {
            return Some(&[]);
        }
        match self.span(off, len) {
            // SAFETY: `span` resolved all `len` bytes inside this process's arc or one of its DMA regions, identity-mapped and the kernel's to read; the slice lives only for the copy that follows, under the kernel lock.
            Some(a) => Some(unsafe { core::slice::from_raw_parts(a as *const u8, len) }),
            None => {
                self.bad_buf(off, len, false);
                None
            }
        }
    }
    /// Whether `len` bytes at arc offset `off` touch a range the process gave back (`holes`).
    fn in_hole(&self, off: usize, len: usize) -> bool {
        self.holes.iter().any(|&(lo, hi)| off < hi && off + len > lo)
    }
    /// The 8-byte word a syscall names at arc offset `off` (`Arc::word`), None also over a range the process gave back.
    fn word(&self, off: u64) -> Option<usize> {
        let a = self.arc.word(off)?;
        (!self.in_hole(a - self.arc.base, 8)).then_some(a)
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
            // Never the camera's view: read-only at EL2 too (cachemap `cam_view_map`), so a kernel write into a buffer there would fault under the lock.
            Object::Dma(i) if DMA[i].name == "camera-view" => None,
            // Nor a mailbox's view, read-only at EL2 as well (cachemap `mbox_map`); a reader names its sequence word to SYS_WAIT alone (`mbox_wait_word`).
            Object::Dma(i) if mbox_of(i).is_some_and(|m| m.1) => None,
            Object::Dma(i) if region(i).1 > 0 && a >= region(i).0 && end <= region(i).0 + region(i).1 => Some(a),
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
    let on_demand = ON_DEMAND.iter().position(|n| *n == name).map_or(0, |i| i as u8 + 1);
    kk.endpoints.push(Endpoint { name: name.to_vec(), queue: alloc::collections::VecDeque::new(), poller: None, server: 0, on_demand });
    kk.endpoints.len() - 1
}

/// Endpoints whose server the kernel starts when a call finds none (docs/TORCH.md "Who starts it"; owner, 2026-11-11: the quick settings' torch). init's grants page is full, and a server that runs only while its device is in use holds no arc the rest of the time: the torch process ends itself once the LED is dark and nothing waits. Each name is a process the service loop knows how to start (service.rs `shell_grants`); its index is its bit in `ON_DEMAND_WANT`. The audio service the same way (docs/AUDIO.md, 2026-11-11): started by a kist's first call on `audio`, ended by itself once every stream has played out.
/// The icon decoder too (std/icondec, 2026-11-20): started by the stower's first call on `icons` at a stow, ended by itself once it has handed an icon back.
pub static ON_DEMAND: &[&[u8]] = &[b"torch", b"audio", b"camera", b"icons"];
/// The on-demand servers a call found missing since the service loop last looked (`on_demand_take`).
static ON_DEMAND_WANT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// A call on `ep` (or a call left queued on it by a server that ended): if its server is started on demand and none is receiving, the service loop is asked to start one, and woken (`sev`), never on a timer. A server spawned but not yet at its first receive counts as missing here; the service loop sees it alive and leaves it.
fn on_demand_note(ep: usize) {
    let d = k().endpoints[ep].on_demand;
    if d != 0 && !has_receiver(ep) {
        ON_DEMAND_WANT.fetch_or(1 << (d - 1), core::sync::atomic::Ordering::AcqRel);
        unsafe { core::arch::asm!("sev") };
    }
}

/// Which on-demand servers are wanted (bits by `ON_DEMAND` index), cleared as read: the service loop's, once a pass.
pub fn on_demand_take() -> u32 {
    ON_DEMAND_WANT.swap(0, core::sync::atomic::Ordering::AcqRel)
}

/// An on-demand server that could not be started (no capsule in the vault, no arc free): every call waiting on its endpoint gets ERR now rather than waiting for a server that is not coming. How many were answered. Under the kernel lock.
pub fn on_demand_refuse_locked(name: &[u8]) -> usize {
    let kk = k();
    let Some(ep) = kk.endpoints.iter().position(|e| e.name == name) else { return 0 };
    let queued: Vec<usize> = kk.endpoints[ep].queue.iter().map(|q| q.0).filter(|&c| c != KERNEL_CALLER).collect();
    for &caller in queued.iter() {
        if let Some(ct) = th(caller) {
            ct.frame.x[0] = ERR;
            ct.state = State::Runnable;
            enqueue_back(caller);
        }
    }
    kk.endpoints[ep].queue.retain(|q| q.0 == KERNEL_CALLER);
    queued.len()
}

/// The system processes the kernel starts again at their death by a panic or a fault (owner, 2026-11-21, after the deck panicked on the Pro and the device sat frozen with nothing to answer a touch: "keyboard, speaker, whatever you think needs to be on the restart list"; docs/KERNEL.md "Restarts"). Each a name service.rs `shell_grants` knows; its index is its bit in `KEPT_WANT`, and the order is the order the service loop starts them when several died at once: the display before the deck (the deck hands a fresh display its scene), the identity service before the deck, init last (it starts only what is missing).
/// A plain exit is the process's own choice (usb with no cable ends after its tries, and init's keeper starts it again) and a kill from outside is the kernel's, the bridge's or the person's: neither is answered here.
/// The vault (`vault_ended`) and the on-demand servers (`ON_DEMAND`: the speaker, the torch, the camera, the icon decoder) have restarts of their own; their deaths count toward the same crash-loop rule (`crash_answer`).
pub static KEPT: [&[u8]; 11] = [b"display", b"touch", b"ahau", b"usb", b"ip", b"nfc", b"gpu", b"fgtw", b"map", b"kists", b"init"];
/// The kept processes that died and are to be started again (bits by `KEPT` index), cleared as read (`kept_take`): the service loop's, woken by `sev` at the death.
static KEPT_WANT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// How each kept process last died (`ferros_kist::restart::End` as a byte, by `KEPT` index), for the note the service loop posts once it is started again.
static KEPT_END: [core::sync::atomic::AtomicU8; 11] = [const { core::sync::atomic::AtomicU8::new(0) }; 11];
/// How the vault process last ended having served (an `End` byte, 0 none since the service loop last took it), for its restart's note.
static VAULT_END: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Each restarted process's last deaths and whether its restarts stopped (ferros_kist::restart `Deaths`), by name: the kept, the on-demand servers and the vault, entered at their first death. Under the kernel lock.
static mut DEATHS: Vec<(&'static [u8], ferros_kist::restart::Deaths)> = Vec::new();
/// Restart notes waiting for the service loop to post them (it posts once a person is present: a note is the person's): the process, how it ended, whether its restarts stopped. Under the kernel lock.
static mut NOTICES: Vec<(&'static [u8], ferros_kist::restart::End, bool)> = Vec::new();

/// The `'static` name of a process the kernel restarts (`KEPT`, `ON_DEMAND`, the vault), or None for any other.
fn restarted_name(name: &[u8]) -> Option<&'static [u8]> {
    KEPT.iter().chain(ON_DEMAND.iter()).copied().chain(core::iter::once(VAULT_EP)).find(|n| *n == name)
}

/// How a process ended as a restart reads it (death_line's reading of the exit value): a fault (`FAULT` in the top byte), a panic (G#DEAD, a payload's handler; 101, std's main panicking), or None for a plain exit.
fn crash_end(value: u64) -> Option<ferros_kist::restart::End> {
    use ferros_kist::restart::End;
    if value >> 56 == FAULT >> 56 {
        Some(End::Fault)
    } else if value == 0xDEAD || value == 101 {
        Some(End::Panic)
    } else {
        None
    }
}

/// A death of `name` counted (ferros_kist::restart `Deaths::died`, timestamps compared now, never a timer): true when it makes a crash loop, which stops its restarts; said then on the console and in the flash log, and noted for the person. Under the kernel lock.
fn death_counted(name: &'static [u8], end: ferros_kist::restart::End) -> bool {
    use ferros_kist::restart::{Deaths, LOOP_DEATHS, WINDOW_S};
    let d = unsafe { &mut *(&raw mut DEATHS) };
    let i = match d.iter().position(|e| e.0 == name) {
        Some(i) => i,
        None => {
            d.push((name, Deaths::new()));
            d.len() - 1
        }
    };
    let was = d[i].1.stopped;
    let looped = d[i].1.died(crate::platform::cntpct(), crate::platform::cntfrq());
    if looped && !was {
        let nm = core::str::from_utf8(name).unwrap_or("?");
        let line = alloc::format!("restarts: {nm} died A#{LOOP_DEATHS} times within A#{WINDOW_S} s (the last by {}): not started again until `diag restart {nm}`", end.word());
        k().console.extend_from_slice(line.as_bytes());
        k().console.push(b'\n');
        crate::flashlog::line(&line);
        unsafe { (*(&raw mut NOTICES)).push((name, end, true)) };
    }
    looped
}

/// A process ended by itself (`end_process`, never a kill): a kept one (`KEPT`) dead by a panic or a fault is started again by the service loop, woken now, unless this death makes a crash loop (`death_counted`); an on-demand server's crash is counted the same way, and its next start refused once it loops (`restart_stopped`). The vault counts in `vault_ended`. Under the kernel lock.
fn crash_answer(name: &[u8], value: u64) {
    let Some(end) = crash_end(value) else { return };
    let Some(name) = restarted_name(name).filter(|n| *n != VAULT_EP) else { return };
    if death_counted(name, end) {
        return;
    }
    if let Some(i) = KEPT.iter().position(|n| *n == name) {
        KEPT_END[i].store(end as u8, core::sync::atomic::Ordering::Release);
        KEPT_WANT.fetch_or(1 << i, core::sync::atomic::Ordering::AcqRel);
        if name == GPU_EP {
            GPU_RESTART.store(true, core::sync::atomic::Ordering::Release);
        }
        unsafe { core::arch::asm!("sev") };
    }
}

/// The GPU service's endpoint.
const GPU_EP: &[u8] = b"gpu";
/// The GPU service died by a panic or a fault and the service loop is to start it again (`crash_answer`): calls on `gpu` wait for the new one meanwhile. Cleared by the service loop once its restart has been tried (`gpu_restart_tried`).
static GPU_RESTART: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether `ep` is the GPU service's endpoint and no GPU service is running or coming: no live process holds its receive right (the kernel spawns the service before init and nobody else is given that right, so one that holds it is the service, perhaps still booting the GPU before its first receive) and no restart of it is pending (`GPU_RESTART`).
/// A call there is answered ERR at once, a plain one too (2026-11-22: after a jump from Android no GPU service was started, and Lumis 0.2.42's queue open, a plain call, waited for one until the owner stopped it). Under the kernel lock.
fn gpu_absent(ep: usize) -> bool {
    let kk = k();
    kk.endpoints[ep].name == GPU_EP && !has_receiver(ep) && !GPU_RESTART.load(core::sync::atomic::Ordering::Acquire) && !kk.procs.iter().flatten().any(|q| !q.dying && q.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == ep) && c.rights & RIGHT_RECV != 0))
}

/// The service loop has tried the GPU service's restart (`GPU_RESTART`): when it is not running after it (refused, no capsule, a crash loop), every call waiting on `gpu` gets ERR now rather than waiting for a service that is not coming. How many were answered. Takes the kernel lock.
pub fn gpu_restart_tried() -> usize {
    with_lock(|| {
        GPU_RESTART.store(false, core::sync::atomic::Ordering::Release);
        match k().endpoints.iter().position(|e| e.name == GPU_EP) {
            Some(ep) if gpu_absent(ep) => on_demand_refuse_locked(GPU_EP),
            _ => 0,
        }
    })
}

/// Which kept processes died and are to be started again (bits by `KEPT` index), cleared as read: the service loop's, once a pass.
pub fn kept_take() -> u32 {
    KEPT_WANT.swap(0, core::sync::atomic::Ordering::AcqRel)
}

/// How kept process `i` last died.
pub fn kept_end(i: usize) -> ferros_kist::restart::End {
    ferros_kist::restart::End::from_u8(KEPT_END[i].load(core::sync::atomic::Ordering::Acquire)).unwrap_or(ferros_kist::restart::End::Panic)
}

/// How the vault process last ended having served, taken (None when it has not since the last take: a kill, or nothing).
pub fn vault_end_take() -> Option<ferros_kist::restart::End> {
    ferros_kist::restart::End::from_u8(VAULT_END.swap(0, core::sync::atomic::Ordering::AcqRel))
}

/// Whether `name`'s restarts stopped (a crash loop). Takes the kernel lock.
pub fn restart_stopped(name: &[u8]) -> bool {
    with_lock(|| unsafe { (*(&raw const DEATHS)).iter().any(|e| e.0 == name && e.1.stopped) })
}

/// A restart by hand (`diag restart`): `name`'s count of deaths and its stop cleared, so it is kept again. Whether it had stopped. Takes the kernel lock.
pub fn restart_clear(name: &[u8]) -> bool {
    with_lock(|| {
        let d = unsafe { &mut *(&raw mut DEATHS) };
        let was = d.iter().any(|e| e.0 == name && e.1.stopped);
        d.retain(|e| e.0 != name);
        was
    })
}

/// The restart notes waiting, taken. Takes the kernel lock.
pub fn notices_take() -> Vec<(&'static [u8], ferros_kist::restart::End, bool)> {
    with_lock(|| core::mem::take(unsafe { &mut *(&raw mut NOTICES) }))
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
/// 9 `sleep(deadline)`: park until CNTPCT reaches `deadline`, so a periodic process (the display server's frame clock) costs nothing between frames instead of spinning thru every slice the service loop hands out. Returns 0 at the deadline, 1 when a call to an endpoint the thread polls ended it early (one already waiting returns 1 at once).
const SYS_SLEEP: u64 = 9;
/// Start a signed capsule from the vault as a new process, handing it copies of some of the caller's own capabilities (never more rights than the caller holds; reply capabilities do not pass). x0/x1: request offset and length in the caller's arc, [name len][name][vault key len][vault key][token count][tokens, 32 bytes each]. The child is queued, not entered: the caller carries on. Returns 0, or ERR with the reason on the console.
const SYS_SPAWN: u64 = 10;
/// Change a device preference: x0 the Settings capability's token offset, x1 which preference, x2 its new value. The kernel checks the value, keeps it in the vault (`keep_settings`: written at the trap's end, the kernel lock let go), and rewrites it in every live process's preferences block at once. Returns 0 or ERR.
const SYS_SET_PREF: u64 = 11;
/// SYS_SET_PREF x1 bit 8: the value applied to every process's block but not kept in the vault (a slider while the finger is down).
const PREF_LIVE: u64 = 1 << 8;
/// The device's plain facts for its dashboard (SHELL.md: the device circle's front): x0/x1 a buffer in the caller's arc and its length; up to FACT_COUNT u64 words, little-endian, `FACT_UNKNOWN` where a fact is not known. No capability: these are what the device shows whoever holds it, and none is a key, a handle or a serial. Returns the bytes written.
const SYS_FACTS: u64 = 12;
pub const FACT_COUNT: usize = 90;
pub const FACT_UNKNOWN: u64 = u64::MAX;
/// A scope of the device vault: x0 the Store capability's token offset, x1 the operation (bit 0: 0 get, 1 set) with the buffer's length in bits 8 and up, x2/x3 the key's offset and length (relative to the scope's prefix, at most 64 bytes of printable ASCII), x4 the buffer. A get copies at most the buffer's length and returns the value's whole length, NOTHING if the key is absent; a set stores the buffer (at most STORE_MAX) and returns 0. ERR otherwise.
const SYS_STORE: u64 = 13;
/// thread_spawn(entry, stack_top, arg): a new thread of this process at arc offset `entry` with SP_EL0 at arc offset `stack_top` and x0 = `arg`, runnable at once (the caller carries on). Returns 0 or ERR.
const SYS_THREAD_SPAWN: u64 = 14;
/// thread_exit(value): this thread ends; the last thread of a process ends the process as `exit` does.
const SYS_THREAD_EXIT: u64 = 15;
/// wait(word, expected, deadline): if the 8-byte word at arc offset `word` still holds `expected`, block until another thread of this process wakes it (returns 0) or CNTPCT reaches `deadline` (returns 2; 0 = no deadline); returns 1 at once if the word had already changed. A futex with a timeout: threads hand work to each other without IPC, and a server's loop waits for a request or its next deadline in one call. x3 bit 0: a wait on the world (the watchdog paused); bit 1: a call to an endpoint the thread polls ends it too (returns 3; one already waiting returns 3 at once).
const SYS_WAIT: u64 = 16;
/// wake(word, count): make up to `count` threads of this process waiting on that word runnable; returns how many.
const SYS_WAKE: u64 = 17;
/// irq_wait(cap, deadline): block until the line fires (0), or the deadline passes (2; 0 = none); if it fired while no one waited, return 0 at once. The line is masked at the GIC from the moment it fires until `irq_ack`. A call to an endpoint the thread polls ends it with 2 as well.
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
/// unlock(cap, op, seed): x0 the Unlock capability, x1 the op: 0 the state (bit 0 an identity is unlocked, bit 1 the device is bound to one, bit 2 the person key held), 1 unlock the identity whose 32-byte seed is at x2 (the number of entries it has on this device; NOTHING: the device is bound to another identity; ERR: no vault or no ira), 2 lock, 3 bind the device to the unlocked identity, 4 release the device (every identity entry deleted) (0, or ERR), 5 the person's export, 6-9 attest with a tag (`tag_syscall`: unlock by a tag, keep, forget, list), 10 the person key, 11 the identity's doorbell (x2 a word: the state and a change count, rung at every change, `identity_ring`). AUTH.md.
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
/// claim(op, …): the rights in use (`CLAIMS`; owner, 2026-11-10: GPS "stays green but no visuals for satellites since we're not rx'ing ... until something like mahere calls it or I open the GPS screen ... or Photon a friend pings for my location", and "we need to be able to see what kists are using each permission"). A right by its code (ferros_kist::RIGHTS); only GPS is claimed today. Op 0: claim x1 the right, x2 the token offset of what proves the caller may use it (for GPS: Settings, the settings page's, the `gps` endpoint's send right, a kist's granted want, or a GPS mailbox view) → 0 (also when it held one already), ERR refused (no such right or proof, nobody present, GPS switched off). Op 1: release x1 the right → 0. Op 2: who claims x1 the right, x2 the Settings token, x3/x4 a buffer and its length → how many claims (all of them; records past the buffer are not written), each `CLAIM_REC` bytes: 0 a system process or 1 a kist, then the kist's capsule address (32 bytes, zeros for a system process); ERR without Settings. Op 3: the GPS driver asked thru the kernel for a Settings holder that holds a GPS claim (the map's page: init's grants page has no slot left for its own `use gps`): x1 the driver's op (0 FIX, 2 SATS, payloads/gps), x2 the Settings token, x3/x4 the reply's buffer and its length with CALL_LIVE set, the call the caller's own as SYS_CALL makes it → the reply's length, ERR at once when no driver is receiving yet; or x1 = 3 | the change count last seen << 8, the driver's WATCH, its length bare (no CALL_LIVE): held until the driver's fixes or fix messages move, taken by a driver that starts later, → 4 bytes, the count now. The position passes thru, never kept. Op 4: the claims' doorbell, x1 a word in the caller's arc, x2 the Settings token: GPS_CLAIMS written there now and at every change, its waiters woken → 0.
const SYS_CLAIM: u64 = 37;
/// bell(source, word): a doorbell on what the settings pages show (owner, 2026-11-10: "Needs a doorbell on the driver's fix counter, for sure. 1s poll can desync too, that needs fixd."). x0 the source: 0-5 a link in the facts' order (its eight measures, its tracks and its detail document: counted when its driver sets one to a value other than the one it held, and when the driver ends and they empty), `BELL_FACTS` the device's facts a settings page shows (`bell_facts`). `BELL_SURFACES` (9), for the holder of the `surfaces` region alone (the display server): the windows' owners in their old form, the word not a count but slots 0 to 3's live generations, byte k slot k's (its preference byte 8 + k while the process it was given to is alive, 0 otherwise) and a change count above bit 32 (`surfaces_ring`; 2026-11-11, Mahere killed from outside and its window kept the glass); kept for a display from before 2026-11-12. `BELL_SLOTS` (12), for the same holder: every slot's, bits 4k to 4k + 3 slot k's generation (1 to 15) while its owner lives, no count. `BELL_IDENTITY` (13, 2026-11-17), for anyone: the identity's lock epoch (`LOCK_EPOCH`), no count. x1 a word in the caller's arc, 8-aligned as SYS_WAIT takes one: the kernel writes the source's change count there now and at every change, waking whatever of the process waits on it (`bell_ring`). One word a source a process; a second registration moves it. No capability: the measures, tracks and facts are what SYS_FACTS and SYS_TRACKS give anyone, and a detail's count says when it changed, never what it says. → 0, ERR for no such source (or the surfaces' or the slots' without the region).
const SYS_BELL: u64 = 38;
/// not_started(op, …): the kist launches init answered "started" for that the kernel's kist job did not start after all, and the kists that started and then ended by a fault, a panic or a kill from outside (`NOT_STARTED`; owner, 2026-11-11: Mahere tapped, nothing opened, "it looked stuck"; then Mahere faulting at once, "no toast or anything"), for the Settings holder alone (the map), as the claims' users list. Op 0: x1 the Settings token, x2/x3 a buffer and its length → how many records (all of them; whole records past the buffer are not written), each `NOT_STARTED_REC` bytes: the reason (ferros_kist::launch::why), the capsule's address (32 bytes), the kists' bell count it rang (u64 LE). Op 1: the kists' doorbell, x1 a word in the caller's arc, x2 the Settings token: the bell's change count written there now and whenever a launch does not start, a kist starts, or a kist ends any way, its waiters woken → 0. Op 2: the kists alive (owner, 2026-11-11: their circles glow running, stopped or not running), x1 the Settings token, x2/x3 a buffer and its length → how many (all of them; whole ones past the buffer are not written), each a capsule's address (32 bytes) of a process the kist job started that is not ending (`kists_live`). ERR without Settings, or for no such op.
const SYS_NOT_STARTED: u64 = 39;
/// arc(op, bytes): the caller's own arc grows or shrinks (ARCS.md step 2; owner, 2026-11-11: "why we cannot scale on demand, that was the whole point of ring memory"). No capability: a process reaches only its own arc, and only as far as its ceiling (`Arc::ceiling`: the most its build declared, its header's heap counted whole; fixed mode's size). Op 0 `ARC_GROW`: at least x1 more bytes (native: whole frames; ring: the mask doubles until they fit; exact: the limit moves) → x0 the new top (the arc's size, exact mode's limit), x1 the old one; the bytes between are new, zeroed, the process's. Op 1 `ARC_SHRINK`: keep the first x1 bytes (rounded up as the mode needs; never under what the process was spawned with) and give the rest back → x0 the new top, x1 the old. Op 2 `ARC_SIZE` → x0 the top, x1 the ceiling. Op 3 `ARC_RELEASE`: the frames wholly inside x2 bytes from arc offset x1 given back while the arc keeps its size (step 4; never the first frame, nor one the kernel holds a word in): a hole the process must not touch again until a grow over it → x0 the bytes given back, x1 the top. Ops 4 `ARC_GUARD` and 5 `ARC_UNGUARD` (2026-11-14): the page at arc offset x1 (4 KiB aligned) made kernel-only, the guard under a thread's stack (ferros_rt `thread::spawn`), or made the process's data again → x0 0; guard takes only a page the process may write and keeps its permission bits, unguard puts them back and only on a page it guarded (never the grants page, its code, the entry stack's guard or a masked arc's), a release or a shrink over a guard forgets it; a kernel from before refuses both (`ARC_WHY_OP`), and the stack goes unguarded. A masked arc's grow or shrink while another of its threads runs elsewhere holds the process off the cores first (`hold`, step 3): its mask or limit changes in every thread at once. Refused: x0 = ERR, x1 the reason (`ARC_WHY_*`), said on the console too.
const SYS_ARC: u64 = 40;
/// offer(op, stow, …): the kist offered over USB to stow (`OFFER`; docs/KISTS.md "Stowing", ferros_kist::offer; owner, 2026-11-11: "Just seems to me that we should just make a proper checked path and start using it"), for the stower alone: x1 the offset of its Stow token (`Object::Stow`). Op 0: the offers' doorbell, x2 a word in the caller's arc: the offers' change count written there now and whenever a host offers a kist, its waiters woken (`bell_register`, `BELL_OFFERS`) → 0. Op 1: the offer not yet ended, x2/x3 a buffer and its length (40 at least) → 0 with [capsule 32][length u64 LE] written (an offer still OFFERED becomes CHECKING), NOTHING when there is none. Op 2: its state as the stower has it, x2/x3 [capsule 32][state u8][why u8][flags u8] → 0, NOTHING when that capsule is not the offer or the offer has ended; the service loop answers a host's held read (`offer_state`). ERR without the Stow right, or for no such op.
const SYS_OFFER: u64 = 41;
/// caller(reply): what the process behind a call holds that its server decides by (2026-11-12, the display's isolation: until then any app could show, draw into, raise or read the events of another app's surface by naming it). x0 the offset of a reply token the caller holds (SYS_RECV's, not yet replied) → bits 32 to 47 the surface slots that process holds (slot k as its own, or inside a region holding it whole: the display server's and the GPU service's `surfaces`; bits 0 to 3 for four surfaces until 2026-11-12), bit 8 the Settings capability (the map, the attest screen: the system's window management), bit 9 a kist (`CALLER_*`); NOTHING for a token that is no reply this process holds, or one for the kernel's own call. No capability: the token is the proof, and only the server the call came to holds it. A kernel from before answers ERR (no such syscall), which a server takes as a refusal.
/// x1, since 2026-11-18 (docs/CAMERA.md "Serving while a frame waits"; the Pro that day: the camera driver sat 14 years in a frame's wait for a holder already gone): a word in the server's arc, 8-aligned as SYS_WAIT takes one (0 none, as before): the end of the process behind the call watched (`end_watch`). When that process ends, any way (`end_process`, `kill_locked`), the kernel sets bit 63 of the word (an atomic or, so a count the server keeps in the low bits stays whole) and wakes the server's threads waiting on it (`end_watch_ring`), so a server learns its holder is gone in the wait it already makes, with no poll and no call to fail first. The answer then carries `CALLER_WATCHED`; a kernel from before ignores x1 and never sets it. One watch a process: a second moves it.
const SYS_CALLER: u64 = 42;
/// notify(op, …): notes, what kists have waiting on the person (notify.rs, docs/NOTIFY.md; owner, 2026-11-12: "I think we try it with the in view, it can notify me, out of view, silent"). No capability for a kist: the kernel knows it by its capsule. Op 0 post: x1/x2 a note (ferros_kist::note, at most note::MAX bytes), a kist's own; or, from a Settings holder that is not a kist, x3 its Settings token and x4 a 32-byte source, the system's note on a kist's behalf → the note's number (0-15) | the kind the kernel decided << 8; NOTHING nobody present, or the kist at a count (WAITING waiting, or POSTS since the person last looked); ERR not a note. Op 1 withdraw: x1 the number (G#FF all) of the caller's own, or the system's with x2 a Settings token and x3 the source → how many went. Op 2, a kist: the note the person opened (x1 its number, x2/x3 a buffer) → the document's length (written whole if it fits), NOTHING when that is not the one opened. Ops 3-6 for a Settings holder that is not a kist (the deck): 3 the list (x1 the token, x2/x3 a buffer) → how many, each `notify::REC` bytes; 4 the doorbell (x1 a word, x2 the token; `BELL_NOTES`); 5 seen (x1 the token, x2 a 32-byte source, x3 a number or G#FF, x4 1 the person opened that note) → how many went; 6 one note's document (x1 the token, x2 [source 32][number u8], x3/x4 a buffer) → its length, NOTHING none. ERR for anything else.
const SYS_NOTIFY: u64 = 43;
/// thread_name(name, len): the calling thread names itself (2026-11-14: "Photon.9: FAULT" did not say which of Photon's threads it was); std passes its thread's name (`thread::Builder::name`). x0/x1 the name in the caller's arc; its first 16 bytes are kept, and a fault's console line carries them (`fault_report`). No capability: a thread names only itself, and the name is only ever printed. → 0. A kernel from before answers ERR (no such syscall), which ferros_rt ignores.
const SYS_THREAD_NAME: u64 = 44;
/// Arm the caller's interrupt line (x0, an `Irq` token) with a program of register operations inside one of its hardware grants (x1, a `Hardware` token), x2 and x3 the program's address and bytes (ferros_layout irqarm; 0 bytes disarms): run at each interrupt before the line is masked and its waiter woken, without the kernel lock (irqarm.rs; docs/KERNEL.md "Armed actions").
const SYS_IRQ_ARM: u64 = 45;
/// panic(message, length, file, length, line | column << 32): the calling process is panicking (ferros_sys `panic_tell`, from a payload's panic handler or std's hook thru ferros_rt). The kernel keeps the message's first 2^9 bytes and the file's first 2^8 (UTF-8, as the compiler and the panic made them), the line and column, the calling thread's name and the call chain from this call (`frames`), and the process's end carries them to the flash log, the console and the glass (`death_line`). It does not end the process: the exit that follows does. A second report replaces the first. No capability: a process tells only of itself. → 0. A kernel from before answers ERR, and the exit says what it always said.
const SYS_PANIC: u64 = 46;
/// mailbox(op, …) (2026-11-19, docs/KERNEL.md "Mailboxes"; owner: "a capability based public read only mailbox so you could have an unlimited number of items accessing it"). Op 0, open: x1 the mailbox (ferros_mailbox `GPS`, `MOTION`, ...), x2 the token offset of the proof (`mbox_open`), x3 an arc offset for the view's token (32 bytes) → the view's base, or ERR.
/// Everything else is a syscall that was there: SYS_DMA on a mailbox's region maps it (the writer's making the caller its writer), SYS_WAIT on its sequence word waits for a publish, and SYS_WAKE on it by the writer wakes every reader waiting.
const SYS_MAILBOX: u64 = 47;
#[path = "notify.rs"]
mod notify;
pub use notify::{clear as notes_clear, kind_name as note_kind_name, kist_named as note_kist_named, post_notice as note_post_notice, post_system as note_post_system, report as notes_report};
pub const CALLER_SETTINGS: u64 = 1 << 8;
pub const CALLER_KIST: u64 = 1 << 9;
/// SYS_CALLER's answer when its x1 word now watches the caller's end (`end_watch`).
pub const CALLER_WATCHED: u64 = 1 << 62;
pub const ARC_GROW: u64 = 0;
pub const ARC_SHRINK: u64 = 1;
pub const ARC_SIZE: u64 = 2;
pub const ARC_RELEASE: u64 = 3;
pub const ARC_GUARD: u64 = 4;
pub const ARC_UNGUARD: u64 = 5;
/// Why SYS_ARC refused: past the arc's ceiling, too few free frames in the pool, (no longer: a masked arc with another of its threads running on another core, which is held now), under what the process was spawned with, over a word the kernel holds in it (a bell, the sensors' block, the stower's word), an unknown op.
pub const ARC_WHY_CEILING: u64 = 1;
pub const ARC_WHY_MEMORY: u64 = 2;
/// Answered only before step 3 held the process instead (2026-11-11): kept so the number is never reused.
#[allow(dead_code)]
pub const ARC_WHY_BUSY: u64 = 3;
pub const ARC_WHY_FLOOR: u64 = 4;
pub const ARC_WHY_HELD: u64 = 5;
pub const ARC_WHY_OP: u64 = 6;
/// ARC_GUARD on a page that is not a writable data page of the process's, or ARC_UNGUARD on one it did not guard.
pub const ARC_WHY_PAGE: u64 = 7;
/// A record of SYS_CLAIM op 2: the kind, then a capsule address.
pub const CLAIM_REC: usize = 33;
/// Each link's control block: words 1 to CONTROLS - 1 (word 0 is the generation, `CONTROL_GEN`), all ones until set.
pub const CONTROLS: usize = 1 << 4;
/// The links with a control block: the six of the facts, and the clock.
pub const CONTROL_LINKS: usize = 7;
/// The clock's link, and its zone's slot.
pub const CONTROL_CLOCK: usize = 6;
pub const CONTROL_ZONE: usize = 1;
pub static CONTROL: [[core::sync::atomic::AtomicU64; CONTROLS]; CONTROL_LINKS] = [const { [const { core::sync::atomic::AtomicU64::new(u64::MAX) }; CONTROLS] }; CONTROL_LINKS];
pub static CONTROL_GEN: [core::sync::atomic::AtomicU64; CONTROL_LINKS] = [const { core::sync::atomic::AtomicU64::new(0) }; CONTROL_LINKS];
/// Writes the settings document (the preferences, the knobs, the zone and the language) into the vault: (the document, its generation) -> kept, a write older than one already kept answered kept without writing. Registered by whoever owns the vault (service.rs `settings_write`), which takes the vault's lock for it.
pub static mut SETTINGS_WRITE: Option<fn(&[u8], u64) -> bool> = None;
/// The settings document's snapshots, counted under the kernel lock (`settings_snapshot`), so their writes, made later without it, land in the order the changes were made: a write never puts an older document over a newer one.
static SETTINGS_GEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Per core: the syscall it runs changed a kept setting (`keep_settings`), so the trap keeps the document once the kernel lock is let go (`ferros_el0_trap`).
static KEEP_ASKED: [core::sync::atomic::AtomicBool; 16] = [const { core::sync::atomic::AtomicBool::new(false) }; 16];

/// The settings document as it stands now and its generation, for a write without the kernel lock (`SETTINGS_WRITE`). With the kernel lock held: the preferences, the zone and the language it reads are the kernel's state.
pub fn settings_snapshot() -> Option<(Vec<u8>, u64)> {
    let doc = crate::settings::document(None)?;
    Some((doc, SETTINGS_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1))
}

/// A syscall changed a setting that is kept (SYS_SET_PREF, SYS_CONTROL, SYS_ZONE, SYS_LANG): the document is snapshotted at the trap's end and written once the kernel lock is let go (`ferros_el0_trap`), before the caller goes on, so the change is on the flash when its call returns, as before.
/// Until 2026-11-18 the write was made here, under the kernel lock: 3.3 ms of a UFS commit after a tap on the map, every core waiting (`diag lock`).
/// Whether there is a vault to keep it in; a write that then fails answers the caller ERR, the setting applied but not kept (before, a refused write left it unapplied: undoing it now would race the changes after it).
fn keep_settings() -> bool {
    KEEP_ASKED[core()].store(true, core::sync::atomic::Ordering::Relaxed);
    unsafe { SETTINGS_WRITE }.is_some()
}
/// The largest detail document a link keeps.
pub const DETAIL_MAX: usize = 1 << 10;
/// The detail documents and their lengths, touched only under the kernel lock (inside a syscall).
static mut DETAIL: [[u8; DETAIL_MAX]; 6] = [[0; DETAIL_MAX]; 6];
static mut DETAIL_LEN: [usize; 6] = [0; 6];
/// The radio drivers by process name, in the facts' order: the link a SYS_STATUS or SYS_DETAIL caller speaks for.
const LINK_NAMES: [&[u8]; 6] = [b"modem", b"pcie1", b"bt", b"nfc", b"gps", b"usb"];
/// pet(): the process is alive and going round its main loop (the process watchdog, measuring: the kernel records the longest stretch between two pets, `diag pets`). → 0.
const SYS_PET: u64 = 28;
/// block(cap, op, block, buf): 4 KiB blocks of a window on the ferros partition (`WINDOWS`), by number within it: op bit 0 write (else read), bit 1 timed, bits 8-23 the count (1 to BLOCK_MAX; bits 8-15 until 2026-11-11, which a count under 256 still fits), `buf` the count × 4 KiB in the caller's arc. → 0, or ERR (no such window, out of it, or the UFS command failed). Timed (bit 1, ferros_sys `block_read_timed`): x1 returns the timer ticks of the UFS command (the link's wake, the cache upkeep, the doorbell to completion; a write's copy in counts with it) and x2 those of a read's copy to the caller, so the vault process's read report can split the two (owner, 2026-11-11: the 24 MB read measured before it is fixed); x1 and x2 are left as they were otherwise. The count moves as one UFS command (a run, ferros_hal `read_run_lun`) once the controller's PRDT stride is known.
const SYS_BLOCK: u64 = 29;
/// The most blocks one `SYS_BLOCK` moves: one UFS command's worth (1 MiB since 2026-11-11, ferros_hal RUN_MAX), so the vault's read-ahead of a large value takes 256 blocks (about fifteen of its 64 KiB pieces) a command. 32 (128 KiB) from 2026-11-08, one piece a command; 16 before, each block its own command.
const BLOCK_MAX: u64 = ferros_hal::ufs::RUN_MAX as u64;
/// A read of at least this many blocks is a large value moving (the vault's read-ahead): the clock policy's activity, as a PT transfer is (ptlink.rs MOVING_AT; a 24 MB read at the rest clocks has the workers that open its pieces at their 402 MHz floor). A piece's own run (17 blocks) and the index walk's single blocks are not.
const MOVING_BLOCKS: u64 = 1 << 7;
/// The run's bounce: UFS DMAs only into its own pages (the HSI2 S2MPU allows `.bss.dma.ufs` and nothing else of the kernel's), so a run lands here and is copied to the caller. Normal write-back since 2026-11-11 (cachemap.rs `ufs_run_range`), the only `.bss.dma` pages that are: the run's commands clean it to the point of coherency before the doorbell and clean and invalidate it after (ferros_hal `send_command_awake`), and nothing writes it between the two, so no line can be written back over what the device put there and the copy out reads what the device wrote. Before, every block of a read was copied out of non-cacheable memory a word at a time, each load its own trip to DRAM with no prefetch.
#[repr(C, align(4096))]
struct RunBuf([u8; ferros_hal::ufs::RUN_MAX << 12]);
#[unsafe(link_section = ".bss.dma.ufs")]
static mut UFS_RUN: RunBuf = RunBuf([0; ferros_hal::ufs::RUN_MAX << 12]);
/// The run bounce's address range, for the cache map (cacheable, the rest of `.bss.dma` not).
pub fn ufs_run_range() -> (usize, usize) {
    let a = (&raw const UFS_RUN) as usize;
    (a, a + (ferros_hal::ufs::RUN_MAX << 12))
}
/// `out.len() / 4096` consecutive blocks (1 to RUN_MAX) of `lun` from `lba` into `out` thru the run bounce: one UFS command once the PRDT stride is known (learned here at the first run, as SYS_BLOCK learns it), else a block a command. Whether every command completed. Under the UFS lock (`UFS`), as every UFS command is, the copy out of the bounce with it: partfs.rs reads Android's partitions with it (the modem's 98 MB firmware went a block a command until 2026-11-11).
pub fn ufs_read_run(lun: u8, lba: u32, out: &mut [u8]) -> bool {
    UFS.with(|| ufs_read_run_held(lun, lba, out))
}
fn ufs_read_run_held(lun: u8, lba: u32, out: &mut [u8]) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let n = out.len() >> 12;
    let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
    let run = (&raw mut UFS_RUN) as usize;
    if n > 1 && ferros_hal::ufs::PRDT_STRIDE.load(Relaxed) == 0 {
        u.probe_prdt_stride(lun, lba, run);
    }
    let ok = u.read_run_lun(lun, lba, n, run);
    if ok {
        out.copy_from_slice(unsafe { core::slice::from_raw_parts(run as *const u8, n << 12) });
    }
    ok
}
/// `ufs_read_run`'s other half: `src.len() / 4096` consecutive blocks (1 to RUN_MAX) written to `lun` from `lba` thru the run bounce, one UFS command once the PRDT stride is known (learned first, by reads, as `ufs_read_run` learns it), else a block a command. Under the UFS lock, the copy into the bounce with it: kvault.rs `Region::write_run` (the kernel's own vault, 2026-11-22: it wrote a block a command).
pub fn ufs_write_run(lun: u8, lba: u32, src: &[u8]) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    UFS.with(|| {
        let n = src.len() >> 12;
        let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
        let run = (&raw mut UFS_RUN) as usize;
        if n > 1 && ferros_hal::ufs::PRDT_STRIDE.load(Relaxed) == 0 {
            u.probe_prdt_stride(lun, lba, run);
        }
        // SAFETY: the bounce holds RUN_MAX blocks and the caller moves at most that many (kvault.rs KV_RUN); it is touched only under the UFS lock, held here.
        unsafe { core::slice::from_raw_parts_mut(run as *mut u8, n << 12) }.copy_from_slice(src);
        u.write_run_lun(lun, lba, n, run)
    })
}
/// `lun`'s write cache to the medium (ferros_hal `sync_cache_lun`, under `DURABLE`), under the UFS lock: kvault's flush and SYS_BLOCK's (op bit 6).
pub fn ufs_sync(lun: u8) -> bool {
    UFS.with(|| ferros_hal::ufs::UfsController::new(0x1320_0000).sync_cache_lun(lun))
}
/// Whether a run moves as one UFS command (the stride known, runs not turned off by `diag "ufs runs off"`): partfs.rs sizes its reads by it.
pub fn ufs_runs_on() -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    ferros_hal::ufs::RUNS_ON.load(Relaxed) && matches!(ferros_hal::ufs::PRDT_STRIDE.load(Relaxed), 16 | 128)
}
/// SYS_BLOCK's measure (`diag "ufs runs"`, owner 2026-11-08: "24MB from the vault should be imperceptibly fast"): calls, blocks and timer ticks inside the call (the command or commands and the copy), reads then writes; then the reads' ticks in the UFS command alone, and the reads' commands of at least MOVING_BLOCKS blocks.
pub static BLOCK_STATS: [core::sync::atomic::AtomicU64; 8] = [const { core::sync::atomic::AtomicU64::new(0) }; 8];
/// The owner's fresh start is zeroing the flash (service.rs `vault fresh`): every SYS_BLOCK refused from then to its reset, since the block calls no longer hold the kernel lock that keeps the other cores out while it runs.
pub static BLOCKS_SHUT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The flash's WriteBooster was asked on this boot (the `clips` window's first write), and what came of it: bits 0-2 the three flags the device accepted, bit 8 asked (`diag "ufs link"` says it).
pub static CLIPS_WB: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub static CLIPS_WB_SET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The counter at the `clips` window's last write: a RAW clip being recorded (service.rs `recording`, the A715s held at `CLIPS_KHZ`).
pub static CLIPS_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A clip's extent given to the process recording it (`Object::Extent`): its holder (the process's epoch, 0 a free slot), its first block within the `clips` window and its blocks. Under the kernel lock.
#[derive(Clone, Copy)]
pub struct ExtentGrant {
    pub holder: u64,
    pub first: u64,
    pub blocks: u64,
}
/// Extents given at once at most: one a recording process, a few recording together.
pub const EXTENT_GRANTS_MOST: usize = 1 << 3;
pub static mut EXTENT_GRANTS: [ExtentGrant; EXTENT_GRANTS_MOST] = [ExtentGrant { holder: 0, first: 0, blocks: 0 }; EXTENT_GRANTS_MOST];

/// A process's end: the extents it was given go (their capabilities die with its cspace), so a clip that ends with its writer no longer holds the A715s' recording ceiling (service.rs `recording`). Under the kernel lock.
fn extents_free(epoch: u64) {
    let grants = unsafe { &mut *(&raw mut EXTENT_GRANTS) };
    for g in grants.iter_mut().filter(|g| g.holder == epoch) {
        g.holder = 0;
    }
    EXTENTS_OPEN.store(grants.iter().any(|g| g.holder != 0), core::sync::atomic::Ordering::Relaxed);
}

/// Whether any clip's extent is given to a writer now: what holds the A715s at the recording ceiling (service.rs `recording`; the owner, 2026-11-19: any heat cut for RAW video, scoped to an open clip).
pub static EXTENTS_OPEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// SYS_BLOCK x1 bit 3 on the `clips` window (the vault process's, while it serves a call): give the caller its reply token names the extent of `blocks` blocks from window block `first`; x4 the offset of [the reply token 32][the given token 32, written]. Bit 4 with bit 3: take every extent of that caller back instead (its seal). 0 or ERR. Under the kernel lock.
/// WHY the bounds: `first` and `blocks` are the vault process's words; an extent outside its window would reach blocks the vault keeps or no window holds.
fn extent_give(p: &Process, w: &BlockWindow, first: u64, blocks: u64, io_off: u64, take_back: bool) -> u64 {
    if w.name != "clips" {
        return ERR;
    }
    let token = p.read(io_off, 32);
    let holder = match p.lookup(&token).map(|i| p.cspace[i].object) {
        Some(Object::Reply(t)) if t != KERNEL_CALLER => proc_of(t).filter(|q| q.epoch != p.epoch && !q.dying),
        _ => None,
    };
    let Some(q) = holder else { return ERR };
    let grants = unsafe { &mut *(&raw mut EXTENT_GRANTS) };
    // The holder's earlier extents go: one clip at a time a process (and all of them at a take back).
    for (i, g) in grants.iter_mut().enumerate() {
        if g.holder == q.epoch {
            g.holder = 0;
            q.cspace.retain(|c| !matches!(c.object, Object::Extent(j) if j == i));
        }
    }
    if take_back {
        EXTENTS_OPEN.store(grants.iter().any(|g| g.holder != 0), core::sync::atomic::Ordering::Relaxed);
        return 0;
    }
    if blocks == 0 || first >= w.blocks || blocks > w.blocks - first {
        return ERR;
    }
    // A slot free, or one whose holder has ended.
    let Some(i) = grants.iter().position(|g| g.holder == 0 || !k().procs.iter().flatten().any(|r| r.epoch == g.holder && !r.dying)) else { return ERR };
    grants[i] = ExtentGrant { holder: q.epoch, first, blocks };
    EXTENTS_OPEN.store(true, core::sync::atomic::Ordering::Relaxed);
    let t = mint(Object::Extent(i), RIGHT_SEND, q.epoch);
    q.cspace.push(CapEntry { token: t, object: Object::Extent(i), rights: RIGHT_SEND });
    p.write(io_off + 32, &t);
    0
}
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
    // The AoC's mailbox block f1 (G#1522_0000), its AoC-to-AP side: zuma-aoc.dtsi `mbox_aoc_f1`, zuma.h IRQ_MAILBOX_AOCF12AP_ALIVE = SPI 78, level high. Channels 16-31 of the AoC's 48 (`mbox-channels`); the audio playback services with an interrupt sit on channel 20 (aoc_alsa.h PCM_CHANNEL), f1's bit 4: the audio service's period clock (docs/AUDIO.md).
    IrqLine { name: "aoc-mbox-f1", intid: 32 + 78 },
    // The AoC's mailbox block a32 (G#1520_0000), its AoC-to-AP side: zuma-aoc.dtsi `mbox_aoc_a32`, zuma.h IRQ_MAILBOX_AOCA322AP_ALIVE = SPI 77, level high. Channels 0-15; com.google.usf rings channel 1 when it writes the AP a message: the sensors process's doorbell (2026-11-12: it woke every 2 ms to look before this).
    IrqLine { name: "aoc-mbox-a32", intid: 32 + 77 },
    // PCIe channel 1's controller (the WiFi chip's link): zuma.h IRQ_PCIE_GEN3A_1_HSI2, SPI 439, level high (zuma-pcie.dtsi pcie_1, its first `interrupts` entry). Channel 1 has no `separated-msi`, so the Exynos RC driver takes its MSIs on this one line, thru ELBI PCIE_IRQ2 bit 17 (IRQ_MSI_RISING_ASSERT, enabled by PCIE_IRQ2_EN bit 17; pcie-exynos-rc.c exynos_pcie_rc_irq_handler and msi_init). Granted to whoever drives the controller (`build_from`), for payloads/pcie1's wait on the chip (2026-11-11).
    IrqLine { name: "pcie1", intid: 32 + 439 },
    // PCIe channel 0's MSI control block 0 (the cellular modem's link): zuma.h IRQ_PCIE_GEN3_2L_MSI_0_HSI1, SPI 416, level high. Channel 0 has `separated-msi` (zuma-pcie.dtsi pcie_0): block k of the DesignWare MSI receiver raises its own SPI 416 + k, and the CP's four vectors (data 0-3) are block 0's (pcie-exynos-rc.c exynos_pcie_msi0_handler). For payloads/modem's wait on the CP (2026-11-13).
    IrqLine { name: "modem-msi", intid: 32 + 416 },
    // The CP's CP2AP_WAKEUP: gpa6-5, EINT 25 of the alive bank (pinctrl-gs.c zuma_pin_alive gpa6 from EINT 20), zuma.h IRQ_ALIVE_EINT25 = SPI 35, level high (the EINT block's own pending bit). What the CP raises when it wants the link back from its idle suspend, when no MSI can come (modem_ctrl_s5100.c ap_wakeup_handler).
    IrqLine { name: "modem-wakeup", intid: 32 + 35 },
    // The NFC controller's IRQ: gpa7-6, EINT 32 of the alive bank (gpa7 from EINT 26), zuma.h IRQ_ALIVE_EINT32 = SPI 42, level high. For payloads/nfc's wait on the chip instead of reading the pin every millisecond (2026-11-13).
    IrqLine { name: "nfc-irq", intid: 32 + 42 },
    // The Bluetooth UART (APM USI1, serial_18): zuma.h IRQ_APM_USI1_UART_INT_ALIVE, SPI 71, level high. For payloads/bt's wait on received bytes instead of a 4 ms look (2026-11-13).
    IrqLine { name: "bt-uart", intid: 32 + 71 },
    // The ISP front end's line 0: zuma.h IRQ_ISPFE_0_ISPFE, SPI 479, level high (zuma-isp.dtsi `lwis_isp_fe`: csis_core_cmn_ctl and every local channel's image and PD path events ride it; lines 1 to 4 carry the statistics blocks and the PDMA). The camera driver's frame clock: it answers the front end on it (ferros_cam `FRAME_ACK`) instead of the kernel's 15 to 50 ms spin.
    IrqLine { name: "ispfe0", intid: 32 + 479 },
];

/// Interrupt lines that go with a hardware grant (2026-11-11 for pcie1, the table 2026-11-13): a process granted the block on the left gets each line on the right as slots after its last, in this order (`build_from`), so init's list and the keepers' restart list (service.rs `shell_grants`) need no slots for them. A driver finds its lines by asking (`irq_wait` with a past deadline answers ERR for a slot that is no line).
const LINES_WITH: &[(&str, &[&str])] = &[("pcie1-elbi", &["pcie1"]), ("pcie0-elbi", &["modem-msi", "modem-wakeup"]), ("hsi2c0", &["nfc-irq"]), ("apm-usi1-uart", &["bt-uart"])];
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
/// Per INTID, written by `.Lirq` before the pending byte: the counter when a granted line's interrupt was taken, and the core that took it (MPIDR Aff1). What devsched measures a device thread's wake from.
#[unsafe(no_mangle)]
pub static mut __irq_at: [u64; 1024] = [0; 1024];
#[unsafe(no_mangle)]
pub static mut __irq_core: [u8; 1024] = [0; 1024];
/// Threads block on an interrupt as `Waiting(intid, deadline)`: an INTID is far below any arc address, so the two never meet.
fn irq_pending_take(intid: u32) -> bool {
    unsafe {
        let p = &raw mut __irq_pending[intid as usize];
        let v = core::ptr::read_volatile(p);
        if v != 0 {
            core::ptr::write_volatile(p, 0);
            crate::irqarm::untake(intid);
            // A finger on the glass: the clock policy's sign of someone using the device (service.rs clock_policy).
            if intid == IRQ_TOUCH_INT {
                note_input();
            }
            // Frames going out: the clock policy's sign that something is animating at more than the rest flips (a focused widget).
            if intid == IRQ_DECON0_FRAME_START {
                FRAME_STARTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                // The frame mailbox published by whoever took it, once its walk of the threads is over (`frame_mailbox_flush`): its wake walks them too.
                FRAME_DUE.store(true, core::sync::atomic::Ordering::Release);
            }
        }
        v != 0
    }
}
/// A display frame start taken and not yet published in the frame mailbox (`frame_mailbox_flush`).
static FRAME_DUE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The display's frames (2026-11-20, docs/GPU-KISTS.md "The display's frames"; Lumis: "a display frame event, so a kernel runs once per display frame"): at a frame start taken for the display (`irq_pending_take`), the frame mailbox (ferros_mailbox `FRAME`, the kernel its one writer, any process its reader) published with the starts and the pictures armed so far, the header's time the counter now, and every reader waiting woken. Its frame taken from the pool at the first start (`mbox_frame`); none to take, nothing published. Under the lock, after any walk of the threads.
fn frame_mailbox_flush() {
    if !FRAME_DUE.swap(false, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let Some(f) = mbox_frame(ferros_mailbox::FRAME) else { return };
    // SAFETY: the frame is the pool's, kept for this mailbox for the boot, at its own address the kernel's read-write (cachemap `frame_block`), 2 MiB = `WORDS` words; the kernel is its one writer (nobody is granted `frame-mailbox`), under the lock.
    let words = unsafe { core::slice::from_raw_parts(crate::frames::pa(f) as *const core::sync::atomic::AtomicU64, ferros_mailbox::WORDS) };
    let payload = [FRAME_STARTS.load(core::sync::atomic::Ordering::Relaxed), PRESENTS.load(core::sync::atomic::Ordering::Relaxed)];
    ferros_mailbox::publish(words, ferros_mailbox::frame::KIND, crate::platform::cntpct(), &payload);
    mbox_wake(mbox_view_seq(ferros_mailbox::FRAME), u64::MAX);
}

/// The storm guard's per-line state, by `IRQS` index (2026-11-13): a modem build that acked its MSI line without clearing the CP's MSI status watchdog-reset the 8a (PREV_EXIT G#A3, inside SYS_IRQ_ACK).
/// A level line its device still asserts fires again the moment it is unmasked: each ack then costs an interrupt, a wake SGI to every core and a preemption, as fast as the driver can ack, and nothing the driver does ends it.
/// Room for `1 << 5` lines: `IRQS` holds fewer, and an index past it panics (a table grown past this wants this grown with it).
const IRQ_GUARDED: usize = 1 << 5;
/// Unmasks in a row (an `irq_ack`, or an `irq_wait` with nothing pending, which unmasks too) that found their line still asserted at the distributor (GICD_ISPENDR, read before the unmask): the driver did not clear its source, so the unmask fires it again at once.
/// An unmask that finds the line quiet starts it from zero.
static IRQ_RUN: [core::sync::atomic::AtomicU32; IRQ_GUARDED] = [const { core::sync::atomic::AtomicU32::new(0) }; IRQ_GUARDED];
/// For `diag irqs`, the measure the threshold was set by: unmasks, unmasks that found the line asserted, and the longest run of those.
static IRQ_UNMASKS: [core::sync::atomic::AtomicU32; IRQ_GUARDED] = [const { core::sync::atomic::AtomicU32::new(0) }; IRQ_GUARDED];
static IRQ_ASSERTED: [core::sync::atomic::AtomicU32; IRQ_GUARDED] = [const { core::sync::atomic::AtomicU32::new(0) }; IRQ_GUARDED];
static IRQ_LONGEST: [core::sync::atomic::AtomicU32; IRQ_GUARDED] = [const { core::sync::atomic::AtomicU32::new(0) }; IRQ_GUARDED];
/// 1 once a line stormed: left masked, every ack and wait on it answered ERR, until a process is next built holding it (`build_from`).
static IRQ_STORMED: [core::sync::atomic::AtomicU32; IRQ_GUARDED] = [const { core::sync::atomic::AtomicU32::new(0) }; IRQ_GUARDED];
/// More asserted unmasks in a row than this is a storm: the line is masked for good.
/// Set against normal traffic on the 8a (`diag irqs`, docs/KERNEL.md "Interrupt storms"); `diag "irqs storm N"` changes it until the next boot.
pub static IRQ_STORM_RUN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1 << 10);

/// The storm guard, before line `i` (INTID `intid`) is unmasked for `who`: 1 when it has stormed (it must stay masked), else 0.
/// No branch on the line's state: the run and the counts are arithmetic on the asserted bit; the one branch is a storm's first unmask, which reports it.
/// The killswitch's lines (INTID 60, 61, 64) are the kernel's own and in no grant (`IRQS`); they are left out of the storm test here as well, so no grant could ever mask them.
fn irq_storm_check(i: usize, intid: u32, who: &[u8]) -> u32 {
    use core::sync::atomic::Ordering::Relaxed;
    let asserted = crate::killswitch::spi_pending(intid) as u32;
    let run = (IRQ_RUN[i].load(Relaxed) + 1) * asserted;
    IRQ_RUN[i].store(run, Relaxed);
    IRQ_UNMASKS[i].fetch_add(1, Relaxed);
    IRQ_ASSERTED[i].fetch_add(asserted, Relaxed);
    IRQ_LONGEST[i].fetch_max(run, Relaxed);
    let killswitch = (intid == 60) as u32 | (intid == 61) as u32 | (intid == 64) as u32;
    let storm = (run > IRQ_STORM_RUN.load(Relaxed)) as u32 & (killswitch ^ 1);
    let was = IRQ_STORMED[i].fetch_or(storm, Relaxed);
    if storm & (was ^ 1) != 0 {
        crate::flashlog::line(&alloc::format!("line {} (INTID A#{intid}) storms: masked; the process must clear its source ({} unmasked it A#{run} times in a row with the line still asserted)", IRQS[i].name, alloc::string::String::from_utf8_lossy(who)));
    }
    was | storm
}

/// Acks per line, by `IRQS` index: a driver acks only after an interrupt it answered, never on a wait that timed out, so a count that moves is a device delivering.
/// The clock policy reads the camera's (`ispfe0`, the front end's frame clock) as the sign of a running stream (service.rs `camera_streaming`).
pub static IRQ_ACKS: [core::sync::atomic::AtomicU32; IRQ_GUARDED] = [const { core::sync::atomic::AtomicU32::new(0) }; IRQ_GUARDED];

/// `irq_ack` for line `i`, guarded: unmasked unless it has stormed (the unmask of a stormed line writes 0 to GICD_ISENABLER, which changes nothing: no branch), 0, or ERR once it has.
fn irq_ack_guarded(i: usize, intid: u32, who: &[u8]) -> u64 {
    IRQ_ACKS[i].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let stormed = irq_storm_check(i, intid, who);
    crate::killswitch::unmask_spi_if(intid, stormed ^ 1);
    (stormed as u64).wrapping_neg()
}

/// A process built holding line `i`: a storm of the process before it is forgotten, the line this one's to arm again.
fn irq_guard_reset(i: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    IRQ_STORMED[i].store(0, Relaxed);
    IRQ_RUN[i].store(0, Relaxed);
}

/// `diag irqs`: each granted line that has been unmasked: its unmasks, those that found it still asserted, the longest run of those in a row, and whether it stormed.
pub fn irqs_report(out: &mut Vec<u8>) {
    use core::sync::atomic::Ordering::Relaxed;
    out.extend_from_slice(alloc::format!("storm guard: more than A#{} unmasks in a row with the line still asserted masks it\n", IRQ_STORM_RUN.load(Relaxed)).as_bytes());
    for (i, l) in IRQS.iter().enumerate() {
        let acks = IRQ_UNMASKS[i].load(Relaxed);
        if acks == 0 {
            continue;
        }
        out.extend_from_slice(alloc::format!("{} (INTID A#{}): A#{acks} unmasks, A#{} still asserted, longest run A#{}{}\n", l.name, l.intid, IRQ_ASSERTED[i].load(Relaxed), IRQ_LONGEST[i].load(Relaxed), if IRQ_STORMED[i].load(Relaxed) != 0 { ", STORMED: masked" } else { "" }).as_bytes());
    }
    crate::irqarm::report(out);
}

/// When a touch interrupt was last taken by the touch driver, or an injected batch handed to it (counter ticks; 0 never): the adaptive clock raises the clusters at once on a change (service.rs `governor`).
pub static INPUT_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The first input not yet followed by a frame (counter ticks; 0 none), for the touch-to-frame measure (`present_seen`).
static INPUT_WAIT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Frames the display has armed: writes to DECON0's shadow update request (G#1947_0050, the display's `present` and `arm`), one per new picture. Unlike the frame-start interrupt (`FRAME_STARTS`), a panel that DECON re-sends on every TE (the 8a's AK3B, payloads/display `PANEL_HOLDS` 0: 120 a second at rest) does not count here.
pub static PRESENTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// When the last frame was armed (`note_write`), and when the last frame came within 2^-`ANIM_GAP_LOG2` s of the one before it: an animation running (`diag clock gap <n>`, 4: 62.5 ms, so 16 frames a second or more; the rest state's 10 Hz redraws are 100 ms apart).
pub static PRESENT_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static ANIM_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static ANIM_GAP_LOG2: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(4);
/// Touch to frame: from the first input after the last frame (`INPUT_WAIT`) to the next frame armed, counted in eight buckets by the power of two of its milliseconds (under 1, 1, 2-3, 4-7, 8-15, 16-31, 32-63, 64 and over), with the sum and the longest in counter ticks (`diag clock`).
/// A finger held still makes no frame, so the next frame that comes for another reason closes its wait: the long buckets hold those, the short ones a moving finger's.
pub static INPUT_TO_FRAME: [core::sync::atomic::AtomicU64; 8] = [const { core::sync::atomic::AtomicU64::new(0) }; 8];
pub static INPUT_TO_FRAME_SUM: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static INPUT_TO_FRAME_MAX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Calls on the GPU service (`call_endpoint`): the adaptive clock's sign of GPU work, which the cores' busy time does not show.
pub static GPU_CALLS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The DECON0 register the display writes once per new frame (`PRESENTS`).
const DECON0_SHD_REG_UP_REQ: usize = 0x1947_0050;

/// Input arrived: a touch interrupt taken, or an injected batch handed to the touch process.
fn note_input() {
    use core::sync::atomic::Ordering::Relaxed;
    let now = crate::platform::cntpct();
    INPUT_AT.store(now, Relaxed);
    let _ = INPUT_WAIT.compare_exchange(0, now, Relaxed, Relaxed);
}

/// A register write from a process: a new frame when it is DECON0's shadow update request.
fn note_write(addr: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    if addr != DECON0_SHD_REG_UP_REQ {
        return;
    }
    PRESENTS.fetch_add(1, Relaxed);
    // Frames coming close together are an animation (the clock policy's lift, service.rs `interactive`): this frame within `ANIM_GAP_LOG2` of the last. The rest state's redraws (the clock's 10 Hz flip, 100 ms apart) never are.
    let now = crate::platform::cntpct();
    let last = PRESENT_AT.swap(now, Relaxed);
    if now.wrapping_sub(last) < crate::platform::cntfrq() >> ANIM_GAP_LOG2.load(Relaxed) {
        ANIM_AT.store(now, Relaxed);
    }
    let t = INPUT_WAIT.swap(0, Relaxed);
    if t != 0 {
        let d = crate::platform::cntpct().wrapping_sub(t);
        let ms = d * 1000 / crate::platform::cntfrq();
        // The bucket: 0 under a millisecond, then one per power of two of the milliseconds (1, 2-3, 4-7, ..., 32-63).
        // WHY the min: a wait has no upper end (a finger held still for seconds closes at the next frame), and there are eight buckets; the last holds 64 ms and over.
        let b = (64 - ms.leading_zeros() as usize).min(7);
        INPUT_TO_FRAME[b].fetch_add(1, Relaxed);
        INPUT_TO_FRAME_SUM.fetch_add(d, Relaxed);
        INPUT_TO_FRAME_MAX.fetch_max(d, Relaxed);
    }
}
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
/// Preference 12: the keyboard layout on the honeycomb (honeycomb::Layout), 0 Dvorak (the default, owner 2026-10-27) or 1 QWERTY with Q on the bottom row. Bytes 8-11 are slots 0 to 3's surface generations for a display from before 2026-11-12 (`PREF_SURFACES`; the windows' bell carries them since, but the kernel still writes them, so they are not free: preference 8 as "Start into" read a window's generation as Android, 2026-11-21), 16-31 the wall clock; the language is in the second block (PREF_LANG).
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
/// The second preferences block in the grants page: preferences 32-47 at G#FB0, the 16 bytes under the first block that no token reaches (the tokens, 32 bytes each from G#204, end at G#FA4 with the 109th, the most the page has ever held), so the page holds as many grants as before. The first block is full (bytes 8-11 the surfaces' until 2026-11-12, zero since, 16-31 the wall clock). A kernel older than it leaves these bytes zero, which reads as each one's default.
pub const PREFS_HI: usize = 0xFB0;
/// Preferences 32-38, the language (lang.rs; written by the kernel only, never thru SYS_SET_PREF): 32-33 the language in effect, 34-35 this device's own, 36-37 the identity's as its service last wrote it, each a packed ISO 639-3 code (vsf `wl`'s letters, little-endian; 0 none, English); 38 whether the device keeps its own (1) or follows the identity's (0, the default).
pub const PREF_LANG: usize = 32;
pub const PREF_LANG_DEVICE: usize = 34;
pub const PREF_LANG_IDENTITY: usize = 36;
pub const PREF_LANG_OWN: usize = 38;
/// Preference 39: the sound mode (owner, 2026-11-11: the quick row's "sound/vibrate/silent"), 0 sound (the default), 1 vibrate, 2 silent. A device setting, kept in the device document as `sound`; whatever makes a sound or a buzz reads it (nothing does yet).
pub const PREF_SOUND: usize = 39;
/// How many sound modes there are: the valid range of PREF_SOUND.
pub const SOUNDS: u8 = 3;
/// Preference 40: do not disturb, the identity's (owner, 2026-11-11: "a fleet/identity preference so it follows the person"), 0 off (the default) or 1 on. Written here only by the identity service (payloads/fgtw, from the person's space: settings flow identity to device only, SETTINGS.md), never by a Settings holder; kept in the device document as `dnd_identity` so the device keeps the person's last word thru a boot before the space is read.
pub const PREF_DND: usize = 40;
/// Preference 41: the radios that were switched off when the quick row's "radios off" switched them all off (preference 15's bits), so the next tap puts back exactly those (owner, 2026-11-11: "all radios off (restores what was on)"). Kept in the device document as `radios_before`.
pub const PREF_RADIOS_BEFORE: usize = 41;
/// Preferences 42-45: the torch's settings (owner, 2026-11-11: "like flashlight we may want to control the brightness and strobe frequency and duty cycle and timeout"; ferros_sys::torch is their one encoding, docs/TORCH.md "Settings"), device settings (the LED and its driver are this device's): 42 the level a tap lights it at (0 never set: the whole dimmer), 43 the strobe (0 steady, 1-193 32nds of an octave up from a flash every 4 s), 44 the lit share of a flash (a signed byte, 256ths either side of a half, within 120), 45 when it turns itself off (0 never, 1-161 16ths of an octave up from 16 s). Kept in the device document as `torch_level`, `torch_strobe`, `torch_duty`, `torch_timeout`.
pub const PREF_TORCH_LEVEL: usize = 42;
pub const PREF_TORCH_STROBE: usize = 43;
pub const PREF_TORCH_DUTY: usize = 44;
pub const PREF_TORCH_TIMEOUT: usize = 45;
/// The torch's ranges (ferros_sys::torch `STROBE_STEPS`, `DUTY_REACH`, `TIMEOUT_STEPS`).
const TORCH_STROBE_STEPS: u8 = 6 * (1 << 5) + 1;
const TORCH_DUTY_REACH: i8 = 120;
const TORCH_TIMEOUT_STEPS: u8 = 10 * (1 << 4) + 1;
/// Preference 46: the sound's volume (docs/AUDIO.md "Volume"; ferros_sys::audio is its one encoding), a device setting (the speakers are this device's): 0 never set (the audio service's default, two octaves under full scale), 1-255 dimmest to loudest, a 16th of an octave of amplitude a step. Kept in the device document as `volume`.
pub const PREF_VOLUME: usize = 46;
/// Preference 47: what the helm's buttons do (ferros_sys PREF_HELM and its bits), a person's choice kept in the device document as `helm`, 0 every default.
pub const PREF_HELM: usize = 47;
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
    wake_word_waiters();
}

/// Every thread waiting on a word made runnable, its `wait` returning 0 (every wait in the system is a loop that re-checks): a preference or a link's control block changed, and a driver asleep until its device or its next deadline looks at it now. With the kernel lock held. Interrupt waits are left alone.
fn wake_word_waiters() {
    let kk = k();
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

/// The panic's take of the kernel lock (platform.rs `report_panic`): once, if it is free, never waited for, never given back.
/// Every syscall on every core then waits at the lock, so nothing redraws over the panic's screen (the display server flips and draws thru syscalls) until the reset.
/// A lock held already (by the panicking core, or another) is left as it is.
pub fn seize_for_panic() -> bool {
    use core::sync::atomic::Ordering;
    // Free when no ticket is out past the serving one: the next ticket taken only then.
    let s = TICKET_SERVING.load(Ordering::Acquire);
    // WHY wrapping: the tickets count round 2^32 as the atomic add does.
    let took = TICKET_NEXT.compare_exchange(s, s.wrapping_add(1), Ordering::Acquire, Ordering::Relaxed).is_ok();
    if took {
        HOLDER.store(core() + 1, Ordering::Relaxed);
    }
    took
}

/// Run `f` with the kernel lock held: the service loop's way into the kernel's tables. Inside `f`, only the `_held` variants of lock-taking functions.
/// The vault, the flash log and the UFS have locks of their own since 2026-11-18 (`Rank`): a closure that touches only them takes those, not this.
/// `#[track_caller]` names the hold by the caller's line: from 2026-11-17 (the panic screen's `seize_for_panic` put between the attribute and this function) to 2026-11-18 every hold taken here read as one site, process.rs's own line inside it, in `diag lock`.
#[track_caller]
pub fn with_lock<R>(f: impl FnOnce() -> R) -> R {
    lock();
    let r = f();
    unlock();
    r
}

/// `f` under the kernel lock, whether or not this core already holds it (a syscall does; the service loop does not): for code both reach, such as a seal move's queue (`seal_ok`). Never from under a storage lock (`Rank`): there it panics when it must take the lock.
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
        PREF_SOUND => value < SOUNDS,
        PREF_DND => value <= 1,
        PREF_RADIOS_BEFORE => value < 1 << 5,
        // Five choices, a bit each (ferros_sys `helm`).
        PREF_HELM => value < 1 << 5,
        PREF_TORCH_LEVEL => true,
        PREF_TORCH_STROBE => value <= TORCH_STROBE_STEPS,
        PREF_TORCH_DUTY => (value as i8).unsigned_abs() <= TORCH_DUTY_REACH as u8,
        PREF_TORCH_TIMEOUT => value <= TORCH_TIMEOUT_STEPS,
        PREF_VOLUME => true,
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
    // The GNSS core's firmware memory (cp_rmem's GNSS_FW, cachemap.rs MODEM_WINDOWS[3]): the 8a's kepler.bin and chip configuration copied here for the CP (GPS.md "The core is the CP's"); granted by hand after `cell` until init's list has a slot for it.
    Dma { name: "modem-gnss", base: crate::cachemap::MODEM_WINDOWS[3].0, size: crate::cachemap::MODEM_WINDOWS[3].1 - crate::cachemap::MODEM_WINDOWS[3].0 },
    Dma { name: "pcie1-dma", base: crate::cachemap::DMA_POOL.0, size: crate::cachemap::DMA_POOL.1 - crate::cachemap::DMA_POOL.0 },
    // The USB driver process (USB.md): the controller's registers, mapped for EL0 (Device), and its non-cacheable DMA memory.
    Dma { name: "usb-regs", base: 0x1121_0000, size: 0x1_0000 },
    Dma { name: "usb-dma", base: crate::cachemap::USB_DMA.0, size: crate::cachemap::USB_DMA.1 - crate::cachemap::USB_DMA.0 },
    // The GPU driver process (GPU.md): the GPU's registers and doorbells, mapped for EL0 (Device), and the memory it and the GPU share (Normal non-cacheable; the GPU's S2MPU holds it to this).
    Dma { name: "gpu-regs", base: crate::cachemap::GPU_REGS.0, size: crate::cachemap::GPU_REGS.1 - crate::cachemap::GPU_REGS.0 },
    Dma { name: "gpu-mem", base: crate::cachemap::GPU_MEM.0, size: crate::cachemap::GPU_MEM.1 - crate::cachemap::GPU_MEM.0 },
    // The GPU blocks' window whole (cachemap GPUB_VA; until 2026-11-13 the fixed 128 MiB layer arena at G#9800_0000, by the same name): CPU write-back, the GPU coherent. The GPU service's (it reads a client's kernel from the client's own block) and init's to pass on, which passes a block of it (SYS_SPAWN); no client holds it whole.
    Dma { name: "arena", base: crate::cachemap::GPUB_VA.0, size: crate::cachemap::GPUB_VA.1 - crate::cachemap::GPUB_VA.0 },
    // Each slot alone, a GPU client's own block when given (`NewRegion`, `block_grow`): its frames taken from the pool as it is given and as its holder asks for more (SYS_DMA x1 = 2), given back when its holder ends; its size the frames it holds (`region`). Must stay consecutive and in order: `gpub0() + k` is slot k. Never granted by name (`resolve`), only given.
    gpu_block_slot(0), gpu_block_slot(1), gpu_block_slot(2), gpu_block_slot(3), gpu_block_slot(4), gpu_block_slot(5), gpu_block_slot(6), gpu_block_slot(7),
    gpu_block_slot(8), gpu_block_slot(9), gpu_block_slot(10), gpu_block_slot(11), gpu_block_slot(12), gpu_block_slot(13), gpu_block_slot(14), gpu_block_slot(15),
    // The AoC's IPC control block and its services' rings (aoc.rs IPC, cachemap.rs AOC_IPC), EL0 Device memory for the sensors process: com.google.usf's rings lie in its first G#85000 bytes.
    Dma { name: "aoc-ipc", base: crate::cachemap::AOC_IPC.0, size: crate::cachemap::AOC_IPC.1 - crate::cachemap::AOC_IPC.0 },
    Dma { name: "display-fb", base: crate::cachemap::DISPLAY_FB.0, size: crate::cachemap::DISPLAY_FB.1 - crate::cachemap::DISPLAY_FB.0 },
    // Not DMA at all: the display server's cacheable heap, granted the same way (a base the process may use).
    Dma { name: "display-heap", base: crate::cachemap::DISPLAY_HEAP.0, size: crate::cachemap::DISPLAY_HEAP.1 - crate::cachemap::DISPLAY_HEAP.0 },
    // The display's four temporal dither phases (cachemap DISPLAY_PHASES), non-cacheable like the framebuffer.
    Dma { name: "display-phases", base: crate::cachemap::DISPLAY_PHASES.0, size: crate::cachemap::DISPLAY_PHASES.1 - crate::cachemap::DISPLAY_PHASES.0 },
    // The apps' surfaces (cachemap SURF_VA): the whole window, every slot, the display server's (it composes them), the GPU service's (it maps a client's into its view) and init's (it passes one to each app it starts).
    Dma { name: "surfaces", base: crate::cachemap::SURF_VA.0, size: crate::cachemap::SURF_VA.1 - crate::cachemap::SURF_VA.0 },
    // Each slot alone: what an app holds. Given out by `NewSurface`, its frames taken then from the pool and given back when its process ends (`surface_take`, `surfaces_free`). Must stay consecutive and in order: `surface0() + k` is slot k. Never granted by name (`resolve`), only given.
    surface_slot(0), surface_slot(1), surface_slot(2), surface_slot(3), surface_slot(4), surface_slot(5), surface_slot(6), surface_slot(7),
    surface_slot(8), surface_slot(9), surface_slot(10), surface_slot(11), surface_slot(12), surface_slot(13), surface_slot(14), surface_slot(15),
    // The camera driver's region (cachemap CAM_VA, docs/CAMERA.md "The driver"): one run of frames side by side, taken from the pool when the driver asks (SYS_DMA x1 = 3, `cam_ask`), non-cacheable, the front end's S2MPUs fenced to exactly them (s2mpu.rs `fence_cam`), wiped back into the pool when the driver ends (`cam_free`); its size the frames it holds (`region`).
    Dma { name: "camera", base: crate::cachemap::CAM_VA.0, size: crate::cachemap::CAM_VA.1 - crate::cachemap::CAM_VA.0 },
    // The camera's view (cachemap CAM_VIEW_VA, docs/CAMERA.md "The view"): the frames of the driver's image ring, read-only, given by the driver to the kist that opened the camera (SYS_DMA x1 = 4, `cam_view_give`) and taken back at its close or when either ends (`cam_view_drop`); its size the frames it shows (`region`). Never granted by name nor passed on, only given; never a syscall buffer (`dma_span`).
    Dma { name: "camera-view", base: crate::cachemap::CAM_VIEW_VA.0, size: crate::cachemap::CAM_VIEW_VA.1 - crate::cachemap::CAM_VIEW_VA.0 },
    // The display's sprite cache (cachemap DISPLAY_SPRITES): cacheable, the display server's alone.
    Dma { name: "display-sprites", base: crate::cachemap::DISPLAY_SPRITES.0, size: crate::cachemap::DISPLAY_SPRITES.1 - crate::cachemap::DISPLAY_SPRITES.0 },
    // The display's 16-bit canvas (cachemap DISPLAY_CANVAS): its back buffer.
    Dma { name: "display-canvas", base: crate::cachemap::DISPLAY_CANVAS.0, size: crate::cachemap::DISPLAY_CANVAS.1 - crate::cachemap::DISPLAY_CANVAS.0 },
    // The pucks' squares (cachemap PUCK_VA, docs/PUCKS.md): each slot alone, a puck's own, handed at its launch in its surface's place (`puck_take`), its frames as it asks (SYS_DMA x1 = 6, `puck_ask`). Must stay consecutive and in order: `puck0() + j` is slot j. Never granted by name, only given. The display reads them by address (it holds the surfaces' window and runs native).
    puck_slot(0), puck_slot(1), puck_slot(2), puck_slot(3), puck_slot(4), puck_slot(5), puck_slot(6), puck_slot(7),
    puck_slot(8), puck_slot(9), puck_slot(10), puck_slot(11), puck_slot(12), puck_slot(13), puck_slot(14), puck_slot(15),
    // The mailboxes (cachemap MBOX_VA and MBOX_VIEW_VA, docs/KERNEL.md "Mailboxes", 2026-11-19): each one's writer's region (`gps-mailbox`, read-write), then each one's readers' view (`gps-view`, read-only), in ferros_mailbox's order, a frame of the pool behind both from the first ask (`mbox_frame`). Granted by name, or a view opened with a proof (SYS_MAILBOX). Must stay consecutive and in order: `mbox0() + k` is mailbox k's writer's region, `mbox0() + MBOXES + k` its view.
    mbox_slot(0, false), mbox_slot(1, false), mbox_slot(2, false), mbox_slot(3, false), mbox_slot(4, false), mbox_slot(5, false),
    mbox_slot(0, true), mbox_slot(1, true), mbox_slot(2, true), mbox_slot(3, true), mbox_slot(4, true), mbox_slot(5, true),
];

/// Mailbox k's writer's region (`view` false) or its readers' view as a DMA region (`DMA`): its frame's address in that window.
const fn mbox_slot(k: usize, view: bool) -> Dma {
    if view {
        Dma { name: ferros_mailbox::VIEW_NAMES[k], base: crate::cachemap::MBOX_VIEW_VA.0 + k * crate::frames::FRAME, size: crate::frames::FRAME }
    } else {
        Dma { name: ferros_mailbox::BOX_NAMES[k], base: crate::cachemap::MBOX_VA.0 + k * crate::frames::FRAME, size: crate::frames::FRAME }
    }
}
/// How many mailboxes there are.
const MBOXES: usize = crate::cachemap::MBOXES;
const _: () = assert!(MBOXES == 6 && ferros_mailbox::BYTES == crate::frames::FRAME, "the DMA table lists six mailboxes, a frame each");
/// Where the mailboxes' regions begin in `DMA`.
fn mbox0() -> usize {
    DMA.iter().position(|r| r.name == ferros_mailbox::BOX_NAMES[0]).unwrap()
}
/// DMA region `i` as a mailbox's: (its number, whether it is the view); None for any other region.
fn mbox_of(i: usize) -> Option<(usize, bool)> {
    let m = mbox0();
    (i >= m && i < m + 2 * MBOXES).then(|| ((i - m) % MBOXES, i - m >= MBOXES))
}
/// A mailbox's state: the frame behind it (frames.rs index, None until the first ask; kept for the boot, so a reader's view never moves) and its writer (a process epoch, 0 none).
#[derive(Clone, Copy)]
struct Mbox {
    frame: Option<usize>,
    writer: u64,
}
/// Under the kernel lock.
static mut MBOX: [Mbox; MBOXES] = [Mbox { frame: None, writer: 0 }; MBOXES];
/// Mailbox k's sequence word at the readers' window: the address every waiter on it is kept under (`State::Waiting`), whichever window it named.
fn mbox_view_seq(k: usize) -> usize {
    crate::cachemap::MBOX_VIEW_VA.0 + k * crate::frames::FRAME + 8 * ferros_mailbox::SEQ
}

/// Mailbox k's frame, taken from the pool (zeroed: sequence 0, nothing published) and mapped in both windows the first time anyone asks, its writer or a reader; kept for the boot. None when the pool has no frame left. Under the lock.
fn mbox_frame(k: usize) -> Option<usize> {
    let m = unsafe { &mut (*(&raw mut MBOX))[k] };
    if m.frame.is_none() {
        let f = crate::frames::take_top()?;
        crate::cachemap::mbox_map(k, crate::frames::pa(f));
        crate::cachemap::surface_unmapped(crate::cachemap::MBOX_VA.0 + k * crate::frames::FRAME, crate::frames::FRAME);
        crate::cachemap::surface_unmapped(crate::cachemap::MBOX_VIEW_VA.0 + k * crate::frames::FRAME, crate::frames::FRAME);
        m.frame = Some(f);
        let line = alloc::format!("mailbox {}: a frame of the pool at G#{:X}, kept for the boot\n", ferros_mailbox::NAMES[k], crate::frames::pa(f));
        k_console(&line);
    }
    m.frame
}

/// A line on the kernel's console.
fn k_console(line: &str) {
    k().console.extend_from_slice(line.as_bytes());
}

/// SYS_DMA on a mailbox's region (x1 = 0): its frame (`mbox_frame`), and for the writer's region the caller made its one writer; the region's base. ERR, said on the console: the pool has no frame, or another process writes it (one still ending: its threads leave at their next trap, then `mailboxes_gone` frees it).
fn mbox_dma(p: &Process, i: usize) -> u64 {
    let (k, view) = mbox_of(i).unwrap();
    let name = ferros_mailbox::NAMES[k];
    if mbox_frame(k).is_none() {
        k_console(&alloc::format!("mailbox {name}: the frame pool has no frame left\n"));
        return ERR;
    }
    if !view {
        // The frame mailbox is the kernel's to write (`frame_mailbox_flush`): a process given its writer's region by name would be a second writer under one seqlock.
        if k == ferros_mailbox::FRAME {
            k_console(&alloc::format!("mailbox {name}: {} asked to write it: the kernel writes it\n", core::str::from_utf8(&p.name).unwrap_or("?")));
            return ERR;
        }
        let m = unsafe { &mut (*(&raw mut MBOX))[k] };
        if m.writer != 0 && m.writer != p.epoch {
            k_console(&alloc::format!("mailbox {name}: {} asked to write it while another process does\n", core::str::from_utf8(&p.name).unwrap_or("?")));
            return ERR;
        }
        m.writer = p.epoch;
    }
    DMA[i].base as u64
}

/// The mailbox sequence word a SYS_WAIT names outside the caller's arc (2026-11-19): `off` resolved as any buffer outside the arc is (`span`), at the sequence word of a mailbox whose writer's region or view the caller holds, its frame mapped; the readers' address of that word, under which every waiter on the mailbox is kept. None otherwise (the wait traps as before).
fn mbox_wait_word(p: &Process, off: u64) -> Option<usize> {
    let a = ((p.arc.base as u64).wrapping_add(off) & !DMA_ADDR) as usize;
    p.cspace.iter().find_map(|c| match c.object {
        Object::Dma(i) => mbox_of(i).filter(|_| region(i).1 > 0 && a == DMA[i].base + 8 * ferros_mailbox::SEQ).map(|(k, _)| mbox_view_seq(k)),
        _ => None,
    })
}

/// The mailbox sequence word a SYS_WAKE names outside the caller's arc: the writer's own, of a mailbox it writes (`MBOX` writer); the readers' address of that word, whose waiters in every process the wake makes runnable. None otherwise (the wake wakes nothing, as before).
fn mbox_wake_word(p: &Process, off: u64) -> Option<usize> {
    let a = ((p.arc.base as u64).wrapping_add(off) & !DMA_ADDR) as usize;
    (0..MBOXES).find(|&k| unsafe { (*(&raw const MBOX))[k].writer } == p.epoch && a == crate::cachemap::MBOX_VA.0 + k * crate::frames::FRAME + 8 * ferros_mailbox::SEQ).map(mbox_view_seq)
}

/// Up to `count` threads of any process waiting on mailbox word `addr` (`mbox_view_seq`) made runnable, as SYS_WAKE wakes a word's: how many. The one scan of the threads a wake costs whatever reads; the writer makes no call per reader and holds no list of them. Under the lock.
fn mbox_wake(addr: usize, count: u64) -> u64 {
    let kk = k();
    let mut n = 0u64;
    for i in 0..kk.threads.len() {
        if n >= count {
            break;
        }
        if let Some(x) = th(i).filter(|x| matches!(x.state, State::Waiting(a, _) if a == addr)) {
            x.state = State::Runnable;
            if OLD.load(core::sync::atomic::Ordering::Relaxed) {
                woken(i, WAKE_WORD);
                enqueue_back(i);
            } else {
                enqueue_woken(i, WAKE_WORD);
            }
            tr(TR_WAKE, i, 0xFFFF);
            n += 1;
        }
    }
    n
}

/// A word of a GPU block that a SYS_WAIT or SYS_WAKE names outside the caller's arc (2026-11-20, docs/GPU-KISTS.md "The queue"): its address in the blocks' window, when it lies in the frames of a block that is held and the caller holds that block (its owner) or the whole window (`arena`, the GPU service).
/// A kist's GPU queue lives in its own block: it waits on the word the service bumps as its jobs end, and the service's thread for it waits on the word the kist bumps as it submits, each woken by the other's wake, with no call either way.
/// Waiters on one word are kept under its window address whatever process they are in, as a mailbox's are (`mbox_wake`): that address is the same in every holder's view, and no arc word lies in the window.
fn gpub_word(p: &Process, off: u64) -> Option<usize> {
    let a = ((p.arc.base as u64).wrapping_add(off) & !DMA_ADDR) as usize;
    let (lo, hi) = crate::cachemap::GPUB_VA;
    // WHY: the address is the process's own; outside the window, or not a whole aligned word, it names no block word.
    if a < lo || a >= hi || a % 8 != 0 {
        return None;
    }
    let j = (a - lo) / crate::cachemap::GPU_BLOCK_MOST;
    let b = unsafe { &(*(&raw const BLOCKS))[j] };
    // WHY: only a held block's frames are mapped from the slot's start (`block_grow`); past them the window is unmapped, where the kernel's own read of the word would fault.
    if !b.held || a - (lo + j * crate::cachemap::GPU_BLOCK_MOST) >= b.n * crate::frames::FRAME {
        return None;
    }
    let holds = p.cspace.iter().any(|c| match c.object {
        Object::Dma(i) if DMA[i].name == "arena" => true,
        Object::Dma(i) if DMA[i].name == "gpu-block" => i - gpub0() == j && b.owner == p.epoch,
        _ => false,
    });
    holds.then_some(a)
}

/// Every thread waiting on a word inside `[lo, hi)` made runnable, whatever process it is in: how many. At a GPU block's owner's end the GPU service's thread for its queue waits on a word of that block (`gpub_word`), and is woken so it finds its client gone and ends its group. Under the lock.
fn wake_range(lo: usize, hi: usize) -> u64 {
    let kk = k();
    let mut n = 0u64;
    for i in 0..kk.threads.len() {
        if let Some(x) = th(i).filter(|x| matches!(x.state, State::Waiting(a, _) if a >= lo && a < hi)) {
            x.state = State::Runnable;
            if OLD.load(core::sync::atomic::Ordering::Relaxed) {
                woken(i, WAKE_WORD);
                enqueue_back(i);
            } else {
                enqueue_woken(i, WAKE_WORD);
            }
            tr(TR_WAKE, i, 0xFFFF);
            n += 1;
        }
    }
    n
}

/// SYS_MAILBOX op 0: mailbox `id`'s view for the caller, proved by the token at arc offset `proof`; the view's token written at arc offset `out` (the one it holds already, or a new one), answered the view's base (`mbox_dma`). ERR: no such mailbox, the proof refused, no frame.
/// The proof, per mailbox: GPS, what proves a GPS claim (Settings, the `gps` right as its endpoint's send right) or a GPS view; motion, none (gravity and the field are what SYS_FACTS gives anyone today; a `sensors` right is the owner's to decide, rights_implicit_set); the thermometer and the display's frames, none (the owner's, 2026-11-20); the microphones and the test mailbox are granted by name only.
fn mbox_open(p: &mut Process, id: u64, proof: u64, out: u64) -> u64 {
    // WHY the bound: the number is the caller's, an index into the mailboxes.
    if id >= MBOXES as u64 {
        return ERR;
    }
    let n = id as usize;
    let held = |at: u64| {
        let token = p.read(at, 32);
        p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)).filter(|o| o.1 & RIGHT_SEND != 0).map(|o| o.0)
    };
    let proved = match n {
        ferros_mailbox::GPS => match held(proof) {
            Some(Object::Settings) => true,
            Some(Object::Endpoint(e)) => k().endpoints[e].name == b"gps",
            Some(Object::Dma(i)) => DMA[i].name == ferros_mailbox::VIEW_NAMES[ferros_mailbox::GPS],
            _ => false,
        },
        ferros_mailbox::MOTION => true,
        // The display's frames: when a picture starts on the glass is what everyone sees (2026-11-20).
        ferros_mailbox::FRAME => true,
        // The IR thermometer (owner, 2026-11-20: open, no permission necessary, an implicit right as the display or the GPU).
        ferros_mailbox::THERMO => true,
        _ => false,
    };
    if !proved {
        return ERR;
    }
    let vi = mbox0() + MBOXES + n;
    let base = mbox_dma(p, vi);
    if base == ERR {
        return ERR;
    }
    let token = match p.cspace.iter().find(|c| c.object == Object::Dma(vi)) {
        Some(c) => c.token,
        None => {
            let t = mint(Object::Dma(vi), RIGHT_SEND, p.epoch);
            // The view's handle in the grants page's table (2026-11-20, the thermometer puck): masked code reaches a region only thru its handle (SANDBOX.md "Handles"), and the table was written only at the build, so a masked reader that opened a view held it and could not read it (ferros_sys::mailbox `View`).
            // At the first free pair past the slots the build wrote (the page's token count): for a process that opens one view, its grant count, which it knows from its own layout. Not the capability's place in `cspace`, which a server's reply capabilities move.
            // WHY the bound: the table holds `(TOKENS - HANDLES) / 16` pairs; a view opened with none free still serves native readers and the waits, and has no handle.
            // SAFETY: the grants page is the arc's first page, the kernel's to read at its own address; the count and the pairs are words the kernel wrote.
            let built = unsafe { core::ptr::read_volatile((p.arc.base + TOKENS) as *const u32) } as usize;
            let free = (built..(TOKENS - HANDLES) / 16).find(|&s| unsafe { core::ptr::read_volatile((p.arc.base + HANDLES + 16 * s) as *const u64) } == 0);
            if let Some(slot) = free {
                let (b, m) = handle_of(Object::Dma(vi));
                // The grants page is read-only at every level: opened for the write, closed after, as a preference's (`write_pref_all`).
                crate::cachemap::grants_page(p.arc.base, true);
                p.write_grants((HANDLES + 16 * slot) as u64, &b.to_le_bytes());
                p.write_grants((HANDLES + 16 * slot + 8) as u64, &m.to_le_bytes());
                crate::cachemap::grants_page(p.arc.base, false);
            }
            p.cspace.push(CapEntry { token: t, object: Object::Dma(vi), rights: RIGHT_SEND });
            t
        }
    };
    p.write(out, &token);
    base
}

/// The process with `epoch` is gone, every thread of it (its arc is freed next): each mailbox it wrote is marked dead and its payload zeroed (ferros_mailbox `kill`, thru the frame's own address, the kernel's), and every reader waiting on it woken to see so. Called where the arc is freed (`end_process`, `kill_locked`, `drain_dying`'s last thread), never before: a thread of it still on another core could otherwise finish a publish over the mark. Under the lock.
fn mailboxes_gone(epoch: u64) {
    for k in 0..MBOXES {
        let m = unsafe { &mut (*(&raw mut MBOX))[k] };
        if m.writer != epoch {
            continue;
        }
        m.writer = 0;
        // A writer was made only after its frame was taken (`mbox_dma`).
        let f = m.frame.expect("a mailbox's writer has its frame");
        // SAFETY: the frame is the pool's, kept for this mailbox for the boot, at its own address the kernel's read-write (cachemap `frame_block`), 2 MiB = `WORDS` words.
        let words = unsafe { core::slice::from_raw_parts(crate::frames::pa(f) as *const core::sync::atomic::AtomicU64, ferros_mailbox::WORDS) };
        let s = ferros_mailbox::kill(words);
        let n = mbox_wake(mbox_view_seq(k), u64::MAX);
        k_console(&alloc::format!("mailbox {}: its writer ended; dead at sequence G#{:X}, A#{n} waiting readers woken\n", ferros_mailbox::NAMES[k], s & !ferros_mailbox::DEAD));
    }
}

/// Mailbox k's readers woken to a dead mailbox when no writer holds it (the service loop, when the driver a claim asked for did not start): the sequence moved on, still dead (ferros_mailbox `kill`, nothing published to lose), every waiter woken. Nothing while a writer holds it or before its frame exists (no reader can be waiting then). Under the lock.
pub fn mailbox_ring_dead(k: usize) {
    let m = unsafe { (*(&raw const MBOX))[k] };
    let Some(f) = m.frame.filter(|_| m.writer == 0) else { return };
    // SAFETY: as `mailboxes_gone`: the frame is this mailbox's for the boot, its own address the kernel's.
    let words = unsafe { core::slice::from_raw_parts(crate::frames::pa(f) as *const core::sync::atomic::AtomicU64, ferros_mailbox::WORDS) };
    ferros_mailbox::kill(words);
    mbox_wake(mbox_view_seq(k), u64::MAX);
}

/// `diag arcs`'s mailboxes: each one with a frame, its writer, its sequence and its waiting readers.
fn mailboxes_report(s: &mut alloc::string::String) {
    let kk = k();
    for kx in 0..MBOXES {
        let m = unsafe { (*(&raw const MBOX))[kx] };
        let Some(f) = m.frame else { continue };
        let writer = kk.procs.iter().flatten().find(|q| q.epoch == m.writer && m.writer != 0).map(|q| alloc::string::String::from_utf8_lossy(&q.name).into_owned());
        // SAFETY: as `mailboxes_gone`: the frame is this mailbox's for the boot.
        let seq = unsafe { core::ptr::read_volatile((crate::frames::pa(f) + 8 * ferros_mailbox::SEQ) as *const u64) };
        let addr = mbox_view_seq(kx);
        let waiting = kk.threads.iter().flatten().filter(|x| matches!(x.state, State::Waiting(a, _) if a == addr)).count();
        let _ = core::fmt::Write::write_fmt(s, format_args!("mailbox A#{kx} {}: writer {}, sequence G#{:X}{}, A#{waiting} readers waiting, frame G#{:X}\n", ferros_mailbox::NAMES[kx], writer.as_deref().unwrap_or("none"), seq & !ferros_mailbox::DEAD, if seq & ferros_mailbox::DEAD != 0 { " dead" } else { "" }, crate::frames::pa(f)));
    }
}

/// Slot k of the surfaces' window as a DMA region (`DMA`): its addresses, whatever frames are behind them.
const fn surface_slot(k: usize) -> Dma {
    Dma { name: "surface", base: crate::cachemap::SURF_VA.0 + k * crate::cachemap::SURFACE_SIZE, size: crate::cachemap::SURFACE_SIZE }
}

/// The surfaces a spawn can give out at once (cachemap SURFACE_SLOTS), and where the first slot is in `DMA`.
pub const SURFACE_SLOTS: usize = crate::cachemap::SURFACE_SLOTS;
const _: () = assert!(SURFACE_SLOTS == 16, "the DMA table lists sixteen surface slots");
fn surface0() -> usize {
    DMA.iter().position(|r| r.name == "surface").unwrap()
}
/// The most surfaces one process holds that it asked for (SYS_DMA x1 = 10): a kist's two layers, each a mailbox of two buffers (2026-11-20, docs/GPU-KISTS.md "Layers").
const SURFACES_ASKED_MOST: usize = 4;

/// SYS_DMA x1 = 10 on a surface the caller holds (2026-11-20, docs/GPU-KISTS.md "Layers"; Lumis: "layers as mailboxes, at least two per kist"): another surface of its own, its token written at arc offset `out`, answered its base past the porch as SYS_DMA answers a surface's. A window on it is a layer the display composes front to back with the caller's others; two of them make a layer's mailbox (op 22's 36-byte form). ERR, said on the console: the caller holds `SURFACES_ASKED_MOST` already, no slot is free, or the pool is short. Under the lock.
fn surface_ask(p: &mut Process, out: u64) -> u64 {
    let s0 = surface0();
    let held = p.cspace.iter().filter(|c| matches!(c.object, Object::Dma(i) if i >= s0 && i < s0 + SURFACE_SLOTS)).count();
    let name = alloc::string::String::from_utf8_lossy(&p.name).into_owned();
    if held >= SURFACES_ASKED_MOST {
        k_console(&alloc::format!("surface ask: {name} holds A#{held} surfaces, the most a process asks for\n"));
        return ERR;
    }
    match surface_take(p.epoch) {
        Ok(k) => {
            let t = mint(Object::Dma(s0 + k), RIGHT_SEND | RIGHT_RECV, p.epoch);
            p.cspace.push(CapEntry { token: t, object: Object::Dma(s0 + k), rights: RIGHT_SEND | RIGHT_RECV });
            p.write(out, &t);
            surfaces_ring();
            (region(s0 + k).0 + crate::cachemap::SURFACE_PORCH) as u64
        }
        Err(why) => {
            k_console(&alloc::format!("surface ask: {name}: {why}\n"));
            ERR
        }
    }
}

/// The frames behind one surface.
const SURFACE_FRAMES: usize = crate::cachemap::SURFACE_SIZE / crate::frames::FRAME;

/// A slot of the surfaces' window (2026-11-12, owner: replace the fixed four with surfaces taken on demand): whose it is (a process epoch, 0 free), its generation (1 to 15, a new one each time the slot is given out, so a window shown under the last owner's never passes for the new owner's), and the frames behind it while `held` (given out, or given back but still draining a GPU job: `surface_release` returns them).
#[derive(Clone, Copy)]
struct Slot {
    owner: u64,
    generation: u8,
    held: bool,
    frames: [u32; SURFACE_FRAMES],
}
/// Every slot. Under the kernel lock.
static mut SLOTS: [Slot; SURFACE_SLOTS] = [Slot { owner: 0, generation: 0, held: false, frames: [0; SURFACE_FRAMES] }; SURFACE_SLOTS];
/// The blank frame every slot not held maps in each of its blocks (cachemap SURF_VA), taken from the pool with the first surface given out (frames.rs index; None until then, every slot unmapped). Zeroed then and never anyone's: the display may write a glyph into it, racing a window whose owner just ended, and read it back, and no process's pixels are ever there.
static mut SURFACE_BLANK: Option<usize> = None;
/// Slots (bit k) whose owner ended while a job of its was on the GPU: their frames kept and the slot not given out again until the GPU service has taken its next request, so that job, still running, writes nothing a new owner holds (`gpu_drain`, `gpu_drained`). Under the kernel lock.
static mut SURF_DRAINING: u32 = 0;

/// The blank frame (`SURFACE_BLANK`), taken the first time a surface or a GPU block is given out: then every surface slot maps it, so a slot given back always maps something; a GPU block given back maps it too (`block_release`). None only when the pool has no frame left. Under the lock.
fn blank_frame() -> Option<usize> {
    if let Some(b) = unsafe { SURFACE_BLANK } {
        return Some(b);
    }
    let b = crate::frames::take_top()?;
    unsafe { SURFACE_BLANK = Some(b) };
    for s in 0..SURFACE_SLOTS {
        surface_map_slot(s, &[b as u32; SURFACE_FRAMES]);
    }
    Some(b)
}

/// Give the process with `epoch` a surface: the first free slot, 16 frames from the top of the pool (frames.rs `take_top`, zeroed as they are taken: the last owner's pixels are never the next one's, on the CPU or the GPU), mapped in the slot (cachemap `surface_map`) and admitted thru the GPU's fence (s2mpu `gpu_admit`), its generation new. The slot's index, or why none was given. Under the lock.
fn surface_take(epoch: u64) -> Result<usize, &'static str> {
    let slots = unsafe { &mut *(&raw mut SLOTS) };
    let k = slots.iter().position(|s| s.owner == 0 && !s.held).ok_or("no free surface (all sixteen are in use)")?;
    blank_frame().ok_or("the frame pool has too few free frames for a surface")?;
    let mut frames = [0u32; SURFACE_FRAMES];
    for j in 0..SURFACE_FRAMES {
        match crate::frames::take_top() {
            Some(f) => frames[j] = f as u32,
            None => {
                for &f in &frames[..j] {
                    crate::frames::give(f as usize);
                }
                return Err("the frame pool has too few free frames for a surface");
            }
        }
    }
    surface_map_slot(k, &frames);
    for &f in &frames {
        crate::s2mpu::gpu_admit(crate::frames::pa(f as usize), true);
    }
    crate::s2mpu::gpu_sync();
    let s = &mut slots[k];
    // The step: 1 to 15 in turn, 0 being free (the windows' bell holds four bits a slot).
    s.generation = s.generation % 15 + 1;
    s.owner = epoch;
    s.held = true;
    s.frames = frames;
    surface_pref(k, s.generation);
    Ok(k)
}

/// Slot k's blocks pointed at `frames` (each 2 MiB block one frame) and every core's TLB told.
fn surface_map_slot(k: usize, frames: &[u32; SURFACE_FRAMES]) {
    let va = crate::cachemap::SURF_VA.0 + k * crate::cachemap::SURFACE_SIZE;
    for (j, &f) in frames.iter().enumerate() {
        crate::cachemap::surface_map(va + j * crate::frames::FRAME, crate::frames::pa(f as usize));
    }
    crate::cachemap::surface_unmapped(va, crate::cachemap::SURFACE_SIZE);
}

/// Slot k's frames back to the pool: the slot mapping the blank frame first and every TLB told, the frames shut out of the GPU's fence, then each wiped as it goes back (frames.rs `give`). Only once nothing of its owner's can still write them: its process ended and no GPU job of it running (`surfaces_free`, `gpu_drained`). Under the lock.
fn surface_release(k: usize) {
    let slots = unsafe { &mut *(&raw mut SLOTS) };
    let frames = slots[k].frames;
    // A slot held means a surface was given out, and the first one given made the blank.
    let b = unsafe { SURFACE_BLANK }.unwrap() as u32;
    surface_map_slot(k, &[b; SURFACE_FRAMES]);
    for &f in &frames {
        crate::s2mpu::gpu_admit(crate::frames::pa(f as usize), false);
    }
    crate::s2mpu::gpu_sync();
    for &f in &frames {
        crate::frames::give(f as usize);
    }
    slots[k].held = false;
}

/// The surfaces of the process with `epoch` back to free, their frames to the pool (or held back while a GPU job of its runs, `gpu_drain`), and the windows' owners rung (`surfaces_ring`). Under the lock.
fn surfaces_free(epoch: u64) {
    let slots = unsafe { &mut *(&raw mut SLOTS) };
    let draining = unsafe { SURF_DRAINING };
    for k in 0..SURFACE_SLOTS {
        if slots[k].owner == epoch {
            slots[k].owner = 0;
            surface_pref(k, 0);
            if draining >> k & 1 == 0 {
                surface_give_back(k);
            }
        }
    }
    surfaces_ring();
}

/// The GPU fence's tables made again (s2mpu `fence_gpu`): every held surface's and GPU block's frames admitted, as they were. Under the lock.
pub fn gpu_refence() {
    let slots = unsafe { &*(&raw const SLOTS) };
    for s in slots.iter().filter(|s| s.held) {
        for &f in &s.frames {
            crate::s2mpu::gpu_admit(crate::frames::pa(f as usize), true);
        }
    }
    let blocks = unsafe { &*(&raw const BLOCKS) };
    for b in blocks.iter().filter(|b| b.held) {
        for &f in &b.frames[..b.n] {
            crate::s2mpu::gpu_admit(crate::frames::pa(f as usize), true);
        }
    }
    let (first, n) = unsafe { (*(&raw const GPU_CAM)).admitted };
    for f in first..first + n {
        crate::s2mpu::gpu_admit_ro(crate::frames::pa(f));
    }
    // The display's phases while its view holds them (`dv_phases`).
    if unsafe { (*(&raw const DV)).phases } {
        let (a, b) = crate::cachemap::DISPLAY_PHASES;
        let mut pa = a;
        while pa < b {
            crate::s2mpu::gpu_admit(pa, true);
            pa += crate::frames::FRAME;
        }
    }
}

/// `diag arcs`'s surfaces: each slot held, its owner, generation and frames' first address.
fn surfaces_report(s: &mut alloc::string::String) {
    let kk = k();
    let slots = unsafe { &*(&raw const SLOTS) };
    let mut n = 0;
    for (k, slot) in slots.iter().enumerate().filter(|(_, x)| x.held) {
        n += 1;
        let name = kk.procs.iter().flatten().find(|p| p.epoch == slot.owner).map(|p| core::str::from_utf8(&p.name).unwrap_or("?")).unwrap_or(if slot.owner == 0 { "(draining)" } else { "(ended)" });
        let _ = core::fmt::Write::write_fmt(s, format_args!("surface A#{k} {name} generation A#{} at G#{:X} frames from G#{:X}\n", slot.generation, crate::cachemap::SURF_VA.0 + k * crate::cachemap::SURFACE_SIZE, crate::frames::pa(slot.frames[0] as usize)));
    }
    let _ = core::fmt::Write::write_fmt(s, format_args!("surfaces: A#{n} of A#{SURFACE_SLOTS} held, A#{} MiB\n", n * (crate::cachemap::SURFACE_SIZE >> 20)));
}

/// The surface slots' bell word as last rung (`BELL_SLOTS`): bits 4k to 4k + 3 slot k's live generation, 0 for a slot whose owner has ended or that nobody holds.
static SLOTS_LIVE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The old surfaces' bell word as last rung (`BELL_SURFACES`, for a display from before 2026-11-12, which knows four surfaces): byte k slot k's live generation for slots 0 to 3, the change count above bit 32.
static SURFACES_LIVE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Bytes 8-11 of the preferences block: slots 0 to 3's generations, 0 while free, for a display from before 2026-11-12 (it reads them beside `BELL_SURFACES`); the slots' bell alone carries them all.
const PREF_SURFACES: usize = 8;
/// How many slots a display from before knows.
const SURFACES_OLD: usize = 4;

/// Slot k's preference byte, for a display from before (`PREF_SURFACES`): its generation, or 0. Under the lock.
fn surface_pref(k: usize, g: u8) {
    if k < SURFACES_OLD {
        unsafe { (*(&raw mut PREFS_NOW))[PREF_SURFACES + k] = g };
        write_pref_all(PREF_SURFACES + k, g);
    }
}

/// The windows' owners, from the kernel's own table (owner, 2026-11-11, on the Pro: Mahere killed with `diag kill`, its window stayed in the display's table, op 5 still said an app was in front, and the map ignored every gesture but the corners until the display was restarted): each slot's generation while the process it was given to is alive and not ending, 0 otherwise, rung into the display's bell word when that differs from what was rung last, waking its thread there; and the first four in the old form (`BELL_SURFACES`). Called where a surface changes hands: freed (every end, `end_process` and `kill_locked` alike, thru `surfaces_free`) and given (once its process is in the table, `build_from`). A slot's generation changes each time it is given, so the word comes back to a value it held while a slot changed hands in between only after fifteen hand-outs of that slot between two of the display's looks. Under the lock.
fn surfaces_ring() {
    let kk = k();
    let slots = unsafe { &*(&raw const SLOTS) };
    let mut live = 0u64;
    for (s, slot) in slots.iter().enumerate() {
        if slot.owner != 0 && kk.procs.iter().flatten().any(|p| p.epoch == slot.owner && !p.dying) {
            live |= (slot.generation as u64) << (4 * s);
        }
    }
    if SLOTS_LIVE.load(core::sync::atomic::Ordering::Acquire) == live {
        return;
    }
    SLOTS_LIVE.store(live, core::sync::atomic::Ordering::Release);
    bell_ring(BELL_SLOTS, live);
    let mut old = 0u64;
    for s in 0..SURFACES_OLD {
        old |= ((live >> (4 * s)) & 15) << (8 * s);
    }
    let was = SURFACES_LIVE.load(core::sync::atomic::Ordering::Acquire);
    if was & 0xFFFF_FFFF != old {
        let v = ((was >> 32) + 1) << 32 | old;
        SURFACES_LIVE.store(v, core::sync::atomic::Ordering::Release);
        bell_ring(BELL_SURFACES, v);
    }
}

/// The GPU blocks a spawn can give out at once (cachemap GPU_BLOCK_SLOTS), and where the first slot is in `DMA`.
pub const GPU_BLOCK_SLOTS: usize = crate::cachemap::GPU_BLOCK_SLOTS;
const _: () = assert!(GPU_BLOCK_SLOTS == 16, "the DMA table lists sixteen GPU block slots");
fn gpub0() -> usize {
    DMA.iter().position(|r| r.name == "gpu-block").unwrap()
}
/// Slot k of the GPU blocks' window as a DMA region (`DMA`): its addresses, the most its block may grow to; `region` answers the bytes of frames it holds.
const fn gpu_block_slot(k: usize) -> Dma {
    Dma { name: "gpu-block", base: crate::cachemap::GPUB_VA.0 + k * crate::cachemap::GPU_BLOCK_MOST, size: crate::cachemap::GPU_BLOCK_MOST }
}
/// The most frames behind one block: the ceiling's.
const BLOCK_FRAMES: usize = crate::cachemap::GPU_BLOCK_MOST / crate::frames::FRAME;
/// The smallest block a spawn gives when it cannot give what was asked (2^25, 32 MiB: the old arena's quarter, what the attest screen's mark, the wallpaper's maker and gputest work in), and what a kist is given at launch, before it asks for what it wants (`kist_gpu`, SYS_DMA x1 = 2).
const BLOCK_LEAST_LOG: u8 = 25;
/// Frames a GPU block may never take from the pool (2^5, 64 MiB): arcs grow on demand, and a block taken to the last frame would leave the next process's first frame, or a running one's growth, with none.
const BLOCK_RESERVE: usize = 1 << 5;

/// A slot of the GPU blocks' window (2026-11-13, the coordinator's call after the owner's "mahere does not load high rez" on the 8a: the fixed 128 MiB arena left Mahere 32 or 64 MiB): whose it is (a process epoch, 0 free), and the frames behind its first `n` 2 MiB blocks while `held` (given out, or given back but still draining a GPU job: `block_release` returns them).
#[derive(Clone, Copy)]
struct Block {
    owner: u64,
    held: bool,
    n: usize,
    frames: [u32; BLOCK_FRAMES],
}
/// Every slot. Under the kernel lock.
static mut BLOCKS: [Block; GPU_BLOCK_SLOTS] = [Block { owner: 0, held: false, n: 0, frames: [0; BLOCK_FRAMES] }; GPU_BLOCK_SLOTS];
/// Blocks (bit k) whose owner ended while a job of its was on the GPU: their frames kept and the slot not given out again until the GPU service has taken its next request, so that job, still running, writes nothing a new owner holds (`gpu_drain`; the surfaces' own are `SURF_DRAINING`). Under the kernel lock.
static mut GPU_DRAINING: u32 = 0;

/// DMA region `i`'s base and size: a GPU block's size the frames it holds now (0 while nobody holds it); every other region as `DMA` lists it.
fn region(i: usize) -> (usize, usize) {
    if DMA[i].name == "gpu-block" {
        let b = unsafe { &(*(&raw const BLOCKS))[i - gpub0()] };
        return (DMA[i].base, if b.held && b.owner != 0 { b.n * crate::frames::FRAME } else { 0 });
    }
    if DMA[i].name == "camera" {
        let c = unsafe { &*(&raw const CAM) };
        return (DMA[i].base, c.n * crate::frames::FRAME);
    }
    if DMA[i].name == "camera-view" {
        let v = unsafe { &*(&raw const CAM_VIEW) };
        return (DMA[i].base, if v.holder != 0 { v.n * crate::frames::FRAME } else { 0 });
    }
    // A mailbox's region: its frame once taken (`mbox_frame`), nothing before, so no syscall buffer resolves into an unmapped block.
    if let Some((k, _)) = mbox_of(i) {
        return (DMA[i].base, if unsafe { (*(&raw const MBOX))[k].frame }.is_some() { DMA[i].size } else { 0 });
    }
    (DMA[i].base, DMA[i].size)
}

/// The camera driver's frames (docs/CAMERA.md "The driver"; owner, 2026-11-14: frames from the pool on demand, zeroed on take, wiped on give-back, the camera's S2MPUs fenced to exactly them, freed when the process ends): whose (a process epoch, 0 none), the run's first frame and how many.
struct Cam {
    owner: u64,
    first: usize,
    n: usize,
}
/// Under the kernel lock.
static mut CAM: Cam = Cam { owner: 0, first: 0, n: 0 };
/// The most frames the camera's run may hold: its window (cachemap CAM_VA).
const CAM_FRAMES: usize = (crate::cachemap::CAM_VA.1 - crate::cachemap::CAM_VA.0) / crate::frames::FRAME;
/// Frames the camera's run may never take from the pool: as a GPU block's (`BLOCK_RESERVE`), so the next process's first frame and a running one's growth still find one.
const CAM_RESERVE: usize = 1 << 5;
/// The top of the low 4 GiB: a run asked to lie under it (SYS_DMA x1 = 3, x3 bit 0) for a device whose high address bits may not reach DRAM.
const LOW_4GIB: usize = 1 << 32;

/// SYS_DMA x1 = 3 on the `camera` region: the caller takes `n` frames side by side (frames.rs `take_run_unzeroed`; under 4 GiB when `low`), zeroed, cleaned out of the caches (the front end writes them by DMA, the driver reads them non-cacheable), mapped in the camera's window from its start, and fenced (camera_power.rs `set_region`). Answers the run's physical start, which the driver writes into its job (ferros_cam job). A caller already holding a run of at least `n` frames gets its start again. ERR, with why on the console: another process holds it, past the window, the pool without such a run.
/// Called under the kernel lock (a syscall's), which it lets go while the frames are zeroed and cleaned (2026-11-18, `unlocked`: 4.8 ms of it at a camera open, `diag lock`): they are taken from the pool first, so no one else can be given them, and the region is reserved for the caller with no frames yet, so no other process takes it meanwhile and the caller's own end frees nothing of it (`cam_free` passes a region of no frames by); they are mapped and fenced once the lock is back, and given back then if the caller ended meanwhile. `p` is not used after the lock is let go.
fn cam_ask(p: &Process, cur: usize, n: u64, low: bool) -> u64 {
    let c = unsafe { &mut *(&raw mut CAM) };
    let t0 = crate::platform::cntpct();
    let say = |what: &str| {
        let line = alloc::format!("camera region: {} asked for A#{n} frames{}: {what}; A#{} MiB of the pool free\n", core::str::from_utf8(&p.name).unwrap_or("?"), if low { " under 4 GiB" } else { "" }, (crate::frames::free() * crate::frames::FRAME) >> 20);
        k().console.extend_from_slice(line.as_bytes());
    };
    if c.owner != 0 && c.owner != p.epoch {
        say("another process holds the camera's region");
        return ERR;
    }
    // WHY: `n` is the caller's word; a run past the window could not be mapped, and none of no frames is no region.
    if n == 0 || n as usize > CAM_FRAMES {
        say("not a run the camera's window holds");
        return ERR;
    }
    let n = n as usize;
    if c.owner == p.epoch {
        if n <= c.n {
            return crate::frames::pa(c.first) as u64;
        }
        say("it holds a smaller run already");
        return ERR;
    }
    if crate::frames::free() < n + CAM_RESERVE {
        say("the pool would be left too few frames");
        return ERR;
    }
    let Some((first, dirty)) = crate::frames::take_run_unzeroed(n, if low { LOW_4GIB } else { usize::MAX }) else {
        say("no run of free frames that long");
        return ERR;
    };
    // Reserved: the caller's, with no frames yet.
    let epoch = p.epoch;
    *c = Cam { owner: epoch, first, n: 0 };
    let name = p.name.clone();
    let t_lock = crate::platform::cntpct();
    unlocked(|| {
        for f in first..first + n {
            // A frame not known clean zeroed here, as a take zeroes it (under the lock, until 2026-11-18); then every line cleaned and invalidated (the front end writes it by DMA, the driver reads it non-cacheable).
            if dirty[f - first] {
                crate::frames::wipe(f);
            }
            crate::frames::clean_invalidate(f);
        }
    });
    let t_back = crate::platform::cntpct();
    let c = unsafe { &mut *(&raw mut CAM) };
    let say = |what: &str| {
        let line = alloc::format!("camera region: {} asked for A#{n} frames{}: {what}; A#{} MiB of the pool free\n", core::str::from_utf8(&name).unwrap_or("?"), if low { " under 4 GiB" } else { "" }, (crate::frames::free() * crate::frames::FRAME) >> 20);
        k().console.extend_from_slice(line.as_bytes());
    };
    // The caller ended meanwhile (its `cam_free` passed the reserved region by): the frames go back and the region is free again.
    if !proc_of(cur).is_some_and(|q| q.epoch == epoch && !q.dying) {
        if c.owner == epoch {
            *c = Cam { owner: 0, first: 0, n: 0 };
        }
        for f in first..first + n {
            crate::frames::give(f);
        }
        say("its process ended while the frames were made ready; given back");
        return ERR;
    }
    for f in first..first + n {
        crate::cachemap::cam_map(crate::cachemap::CAM_VA.0 + (f - first) * crate::frames::FRAME, Some(crate::frames::pa(f)));
    }
    crate::cachemap::surface_unmapped(crate::cachemap::CAM_VA.0, n * crate::frames::FRAME);
    *c = Cam { owner: epoch, first, n };
    let pa = crate::frames::pa(first);
    let fenced = crate::camera_power::set_region(pa, n * crate::frames::FRAME);
    let us = |a: u64, b: u64| b.wrapping_sub(a) * 1_000_000 / crate::platform::cntfrq();
    say(&alloc::format!("given at G#{pa:X}, taken and cleaned in A#{} us (A#{} us of it with the kernel lock let go), S2MPUs G#{fenced:X} (0 while the switch is off)", us(t0, crate::platform::cntpct()), us(t_lock, t_back)));
    pa as u64
}

/// The camera's view (docs/CAMERA.md "The view"; owner, 2026-11-15: "why the ridiculously low camera resolution?"): which kist holds it (a process epoch, 0 none), the driver that gave it (its epoch), and the frames of the driver's run it shows, from `first`, `n` of them, read-only at cachemap `CAM_VIEW_VA`.
struct CamView {
    holder: u64,
    driver: u64,
    first: usize,
    n: usize,
    /// Mapped write-back (cachemap `cam_view_map`'s `cached`, docs/CAMERA.md "A cacheable view"), else non-cacheable.
    cached: bool,
}
/// Under the kernel lock.
static mut CAM_VIEW: CamView = CamView { holder: 0, driver: 0, first: 0, n: 0, cached: false };
/// The most frames the view may show: its window.
const CAM_VIEW_FRAMES: usize = (crate::cachemap::CAM_VIEW_VA.1 - crate::cachemap::CAM_VIEW_VA.0) / crate::frames::FRAME;
fn cam_view_index() -> usize {
    DMA.iter().position(|r| r.name == "camera-view").unwrap()
}

/// SYS_DMA x1 = 4 on the `camera` region, the driver's alone: the process waiting on the reply token at arc offset `reply_off` (the caller of the OPEN being answered) is given a read-only view of frames `first` to `first + n` of the driver's run (the image ring), a capability of its own for the `camera-view` region, whose token is written at `out_off` in the driver's arc for its reply to carry; a view held already, by anyone, is taken back first (`cam_view_drop`). The view's base, which the holder also gets from SYS_DMA on its token. ERR, said on the console: the caller holds no run, the frames are past it or past the view's window, the token answers no process. Under the lock.
/// With `cached` the view is mapped write-back (docs/CAMERA.md "A cacheable view"), its buffers made right by `cam_lines` as the driver hands each one over.
fn cam_view_give(p: &Process, reply_off: u64, first: u64, n: u64, out_off: u64, cached: bool) -> u64 {
    let c = unsafe { &*(&raw const CAM) };
    let say = |what: &str| {
        let line = alloc::format!("camera view: {}: {what}\n", core::str::from_utf8(&p.name).unwrap_or("?"));
        k().console.extend_from_slice(line.as_bytes());
    };
    if c.owner != p.epoch || c.n == 0 {
        say("not the holder of the camera's region");
        return ERR;
    }
    // WHY: `first` and `n` are the driver's words; a view past its run would map frames it does not hold, and one past the window could not be mapped.
    if n == 0 || n as usize > CAM_VIEW_FRAMES || first >= c.n as u64 || n > c.n as u64 - first {
        say("frames outside its run or past the view's window");
        return ERR;
    }
    let token = p.read(reply_off, 32);
    let holder = match p.lookup(&token).map(|i| p.cspace[i].object) {
        Some(Object::Reply(t)) if t != KERNEL_CALLER => proc_of(t).filter(|q| q.epoch != p.epoch && !q.dying),
        _ => None,
    };
    let Some(q) = holder else {
        say("the reply token answers no other process");
        return ERR;
    };
    // The blank frame the view leaves behind when it is taken back, taken now so the taking back cannot fail.
    if blank_frame().is_none() {
        say("the frame pool has no frame for the blank the view leaves");
        return ERR;
    }
    cam_view_drop("given anew");
    // A GPU space that may still map the frames (the view before, its GPU unmap not yet said): the frames shut out of its fence before this holder's frames land in them.
    if unsafe { (*(&raw const GPU_CAM)).stale } != 0 {
        gpu_cam_shut();
    }
    let (first, n) = (first as usize, n as usize);
    for j in 0..n {
        crate::cachemap::cam_view_map(crate::cachemap::CAM_VIEW_VA.0 + j * crate::frames::FRAME, crate::frames::pa(c.first + first + j), cached);
    }
    crate::cachemap::surface_unmapped(crate::cachemap::CAM_VIEW_VA.0, n * crate::frames::FRAME);
    let vi = cam_view_index();
    let t = mint(Object::Dma(vi), RIGHT_SEND, q.epoch);
    q.cspace.push(CapEntry { token: t, object: Object::Dma(vi), rights: RIGHT_SEND });
    unsafe { *(&raw mut CAM_VIEW) = CamView { holder: q.epoch, driver: p.epoch, first, n, cached } };
    p.write(out_off, &t);
    say(&alloc::format!("A#{n} frames of its run from A#{first} shown read-only{} to {} at G#{:X}", if cached { ", write-back" } else { "" }, core::str::from_utf8(&q.name).unwrap_or("?"), crate::cachemap::CAM_VIEW_VA.0));
    crate::cachemap::CAM_VIEW_VA.0 as u64
}

/// The camera's frames on the GPU (2026-11-20, docs/GPU-KISTS.md "The camera ring on the GPU"; the owner, the same day: the ring read-only to its holder's GPU whenever it holds the camera open, "unless it's a security issue").
/// The conditions that make it the same as the CPU view the holder already has: the holder alone (its calls on `gpu` carry the view in their stamp, `gpu_stamp`, no other's), read-only (the GPU's fence admits the frames read-only, `s2mpu::gpu_admit_ro`, and the service maps them read-only), and gone from its GPU address space when the view goes, before the frames serve anyone else.
/// - `admitted`: the frames (first index in the pool, count) the GPU's fence reads, from the holder's first GPU call carrying the view until the view is taken back and the service has unmapped it.
/// - `mapped`: the epoch whose GPU calls carried the view.
/// - `stale`: an epoch whose GPU address space may still map frames of a view taken back, until the GPU service says it has stopped that client's jobs and unmapped them with its TLB flushed (SYS_DMA x1 = 9, `gpu_cam_settled`). Meanwhile no stamp carries a view, a view given anew finds the old frames shut out of the fence first, and a run given back waits in `drain` instead of the pool.
struct GpuCam {
    admitted: (usize, usize),
    mapped: u64,
    stale: u64,
    drain: (usize, usize),
}
/// Under the kernel lock.
static mut GPU_CAM: GpuCam = GpuCam { admitted: (0, 0), mapped: 0, stale: 0, drain: (0, 0) };
// ferros_sys::gpu `CAMERA_VIEW_WINDOW` and `CAMERA_VIEW_BYTES` name the view's window to the GPU service, which reads a stamp's camera frames by them.
const _: () = assert!(crate::cachemap::CAM_VIEW_VA.0 == 0x51_2000_0000 && crate::cachemap::CAM_VIEW_VA.1 - crate::cachemap::CAM_VIEW_VA.0 == 512 << 20, "ferros_sys::gpu's camera view window");

/// The admitted camera frames shut out of the GPU's fence. Under the lock.
fn gpu_cam_shut() {
    let g = unsafe { &mut *(&raw mut GPU_CAM) };
    if g.admitted.1 == 0 {
        return;
    }
    for f in g.admitted.0..g.admitted.0 + g.admitted.1 {
        crate::s2mpu::gpu_admit(crate::frames::pa(f), false);
    }
    crate::s2mpu::gpu_sync();
    g.admitted = (0, 0);
}

/// The view taken back while its holder's GPU calls carried it: that epoch stale until the GPU service has unmapped it, the service's bell rung (`BELL_GPU_CAMERA`). The frames stay admitted meanwhile, so a job of the holder's still reading one is stopped by the service, not by the fence (a bus fault the GPU may not ride out); a view given anew shuts them first (`cam_view_give`). Under the lock.
fn gpu_cam_taken(holder: u64) {
    let g = unsafe { &mut *(&raw mut GPU_CAM) };
    if holder == 0 || g.mapped != holder {
        gpu_cam_shut();
        return;
    }
    g.mapped = 0;
    g.stale = holder;
    bell_count(BELL_GPU_CAMERA);
}

/// SYS_DMA x1 = 9 on `arena`, the GPU service's: with `epoch` 0, the stale epoch now (0: none); with the stale epoch, the service says it has stopped that client's jobs and unmapped the camera's frames from its address space with the GPU's TLB flushed: the frames shut out of the fence, a run given back meanwhile returned to the pool now. → the stale epoch (asked), 0 (settled), ERR (not the stale epoch). Under the lock.
fn gpu_cam_settled(epoch: u64) -> u64 {
    let g = unsafe { &mut *(&raw mut GPU_CAM) };
    if epoch == 0 {
        return g.stale;
    }
    if g.stale != epoch {
        return ERR;
    }
    g.stale = 0;
    gpu_cam_shut();
    let (first, n) = g.drain;
    g.drain = (0, 0);
    for f in first..first + n {
        crate::frames::give(f);
    }
    if n > 0 {
        let line = alloc::format!("camera region: A#{n} frames held for the GPU's unmap wiped back into the pool\n");
        k().console.extend_from_slice(line.as_bytes());
    }
    0
}

/// The camera view for a caller's GPU stamp (`gpu_stamp`): when it holds the view and no other epoch is stale, the view's region in the camera's view window and its frames' physical addresses, its frames admitted read-only to the GPU's fence, and the caller marked as the one whose GPU space maps them. None otherwise. Under the lock.
fn gpu_cam_stamp(p: &Process) -> Option<((usize, usize), Vec<u64>)> {
    let v = unsafe { &*(&raw const CAM_VIEW) };
    let c = unsafe { &*(&raw const CAM) };
    let g = unsafe { &mut *(&raw mut GPU_CAM) };
    if v.holder != p.epoch || v.n == 0 || g.stale != 0 {
        return None;
    }
    let first = c.first + v.first;
    if g.admitted != (first, v.n) {
        gpu_cam_shut();
        for f in first..first + v.n {
            crate::s2mpu::gpu_admit_ro(crate::frames::pa(f));
        }
        crate::s2mpu::gpu_sync();
        g.admitted = (first, v.n);
    }
    g.mapped = p.epoch;
    Some(((crate::cachemap::CAM_VIEW_VA.0, v.n * crate::frames::FRAME), (first..first + v.n).map(|f| crate::frames::pa(f) as u64).collect()))
}

/// The display's GPU view (GPU-COMPOSE.md 2.1 to 2.3, 2026-11-21; the owner's condition for the compose on the GPU: "not sacrifice security"): what the display's GPU space may map besides its own block, and when what it mapped may go back to the pool.
/// - The phases (`display-phases`, fixed memory), read-write, for the caller that holds them (the display alone): admitted to the GPU's fence from its first stamp until its view is let go (`dv_drop`), so no other space can reach them (none maps them: the service maps a space from its stamp alone).
/// - The windows it composes, read-only: the surface slots its last op 5 named with their generations (`named`), stamped while each is still held under that generation by a live owner (`gpu_stamp`). The display reads every surface on the CPU already; its GPU view is less (read-only, only what it composes).
/// - A surface given back while a stamp named it (`viewed`) keeps its frames held and admitted (`held`) until the GPU service says the display's installed tables, made from a stamp at or after the last that named it, map it no more (SYS_DMA x1 = 10, `dv_settled`): then it is wiped back into the pool. So no compose job reads a frame that is someone else's by then (the proof: GPU-COMPOSE.md 2.2).
/// Under the kernel lock.
struct DispView {
    /// The display whose stamps carry the view (0: none since the last `dv_drop`).
    epoch: u64,
    /// Each slot's generation as its last op 5 named it, 0 not named.
    named: [u8; SURFACE_SLOTS],
    /// The display's stamps so far, and each slot's last stamp that named it.
    seq: u64,
    last: [u64; SURFACE_SLOTS],
    /// Slots some stamp named and no settle has said gone; and of those, the ones given back meanwhile, held from the pool.
    viewed: u32,
    held: u32,
    /// The phases admitted to the GPU's fence.
    phases: bool,
    /// Its display ended (or the GPU service did) with GPU work that may still run: what stamps up to `end_seq` named goes at the service's next request (`gpu_drained`) once `end_epoch` holds no GPU work, or at the service's letting go of `end_epoch` (`gpu_released`; 0 for the service's own end).
    ending: bool,
    end_seq: u64,
    end_epoch: u64,
}
static mut DV: DispView = DispView { epoch: 0, named: [0; SURFACE_SLOTS], seq: 0, last: [0; SURFACE_SLOTS], viewed: 0, held: 0, phases: false, ending: false, end_seq: 0, end_epoch: 0 };

/// Surface slot k's frames back to the pool, as its owner and any GPU job of its owner's are done with them: at once (`surface_release`), or, while the display's GPU view may map them (`DV.viewed`), held until its settle (`dv_settled`). Under the lock.
fn surface_give_back(s: usize) {
    let dv = unsafe { &mut *(&raw mut DV) };
    if dv.viewed >> s & 1 == 1 {
        dv.held |= 1 << s;
        let line = alloc::format!("surface A#{s}: given back while the display's GPU view names it, held until the GPU service unmaps it\n");
        k().console.extend_from_slice(line.as_bytes());
        return;
    }
    surface_release(s);
}

/// The phases' frames (`display-phases`, 2 MiB each) admitted to the GPU's fence (`on`) or shut out of it. Under the lock.
fn dv_phases(on: bool) {
    let (a, b) = crate::cachemap::DISPLAY_PHASES;
    let mut pa = a;
    while pa < b {
        crate::s2mpu::gpu_admit(pa, on);
        pa += crate::frames::FRAME;
    }
    crate::s2mpu::gpu_sync();
}

/// The display's view let go up to its stamp `upto`: no GPU space of the display's maps what those stamps named any more (its group ended and the service let it go, or the service and its GPU are gone and the next service has reset it). Every slot no later stamp named is no longer viewed, a held one wiped back into the pool; with no stamp since, the phases are shut out of the fence and the view is none. Stamps after `upto` (the next display's, or the same display's to the next service) keep what they named. Under the lock.
fn dv_drop(why: &str, upto: u64) {
    let dv = unsafe { &mut *(&raw mut DV) };
    let mut back = 0;
    for j in 0..SURFACE_SLOTS {
        if dv.viewed >> j & 1 == 1 && dv.last[j] <= upto {
            dv.viewed &= !(1 << j);
            if dv.held >> j & 1 == 1 {
                dv.held &= !(1 << j);
                surface_release(j);
                back += 1;
            }
        }
    }
    dv.ending = false;
    let none_since = dv.seq <= upto;
    if none_since {
        dv.epoch = 0;
        dv.named = [0; SURFACE_SLOTS];
        if dv.phases {
            dv.phases = false;
            dv_phases(false);
        }
    }
    let line = alloc::format!("display view: let go to its stamp A#{upto} ({why}), A#{back} held surfaces back to the pool{}\n", if none_since { ", the phases shut out of the GPU's fence" } else { "" });
    k().console.extend_from_slice(line.as_bytes());
}

/// The display (`epoch`) or the GPU service (`service`) ending (`gpu_ending`): a display with no GPU work that can still run (no call taken, `busy`, no queue held) has its view let go at once; otherwise it goes once that work cannot run: at the service's next request for a call (`gpu_drained`), at its letting go of the display's epoch for a queue (`gpu_released`), at the next service's first request (after its GPU's reset) when the service itself ends. Under the lock.
fn dv_ending(epoch: u64, busy: bool, held: bool, service: bool) {
    let dv = unsafe { &mut *(&raw mut DV) };
    if !service && (dv.epoch == 0 || dv.epoch != epoch) {
        return;
    }
    if service || busy || held {
        dv.ending = true;
        dv.end_seq = dv.seq;
        dv.end_epoch = if service { 0 } else { epoch };
    } else {
        dv_drop("its display ended", dv.seq);
    }
}

/// SYS_DMA x1 = 10 on `arena`, the GPU service's: its installed tables for the display came from stamp `seq` and map the surface slots in `mask` (bit k slot k), with the GPU's walks and L2 dropped since any table that mapped more (`space_install`'s LOCK and FLUSH_PT). Every viewed slot outside `mask` that no stamp after `seq` named is no longer viewed, and goes back to the pool now if it was given back meanwhile. → 0. Under the lock.
fn dv_settled(seq: u64, mask: u64) -> u64 {
    let dv = unsafe { &mut *(&raw mut DV) };
    for j in 0..SURFACE_SLOTS {
        if dv.viewed >> j & 1 == 1 && mask >> j & 1 == 0 && dv.last[j] <= seq {
            dv.viewed &= !(1 << j);
            if dv.held >> j & 1 == 1 {
                dv.held &= !(1 << j);
                surface_release(j);
                let line = alloc::format!("surface A#{j}: the display's GPU view settled, wiped back into the pool\n");
                k().console.extend_from_slice(line.as_bytes());
            }
        }
    }
    0
}

/// The display's view for its GPU stamp (`gpu_stamp`), when `p` holds the phases: [0][seq u64][the phases' base u64][bytes u64][m u8][m × (slot, generation)][m × 16 frames] (ferros_sys::gpu `View`). An op 5 with a list (`[5][cached][n][n × (slot u8, generation u8)]`, ferros_sys `Queue::view`) names the windows anew; every other call carries what the last one named, less what has gone since. The phases admitted to the GPU's fence at the first. None for any other caller. Under the lock.
fn dv_stamp(p: &Process, msg: &[u8]) -> Option<Vec<u8>> {
    let phases = p.cspace.iter().find_map(|c| match c.object {
        Object::Dma(i) if DMA[i].name == "display-phases" => Some(region(i)),
        _ => None,
    })?;
    // Not init, which holds the phases (and the blocks' window whole) only to hand them on: only the display stamps a view.
    if p.cspace.iter().any(|c| matches!(c.object, Object::Dma(i) if DMA[i].name == "arena")) {
        return None;
    }
    let dv = unsafe { &mut *(&raw mut DV) };
    if dv.epoch != p.epoch {
        // A display after one whose view was let go, or the first: nothing named yet. (One whose view is still held, `ending`, keeps its viewed and held slots: they go at its settle or its drop.)
        dv.epoch = p.epoch;
        dv.named = [0; SURFACE_SLOTS];
    }
    // WHY the checks: the list is the caller's bytes; a slot past the table or a count past what arrived names nothing.
    if msg.len() >= 3 && msg[0] == 5 && msg.len() >= 3 + 2 * msg[2] as usize {
        dv.named = [0; SURFACE_SLOTS];
        for j in 0..msg[2] as usize {
            let (k, g) = (msg[3 + 2 * j] as usize, msg[4 + 2 * j]);
            if k < SURFACE_SLOTS {
                dv.named[k] = g;
            }
        }
    }
    if !dv.phases {
        dv_phases(true);
        dv.phases = true;
    }
    dv.seq += 1;
    let slots = unsafe { &*(&raw const SLOTS) };
    let shown: Vec<usize> = (0..SURFACE_SLOTS).filter(|&k| dv.named[k] != 0 && slots[k].held && slots[k].owner != 0 && slots[k].generation == dv.named[k]).collect();
    let mut out = Vec::with_capacity(26 + shown.len() * (2 + 8 * SURFACE_FRAMES));
    out.push(0);
    out.extend_from_slice(&dv.seq.to_le_bytes());
    out.extend_from_slice(&(phases.0 as u64).to_le_bytes());
    out.extend_from_slice(&(phases.1 as u64).to_le_bytes());
    out.push(shown.len() as u8);
    for &k in &shown {
        out.extend_from_slice(&[k as u8, slots[k].generation]);
        dv.viewed |= 1 << k;
        dv.last[k] = dv.seq;
    }
    for &k in &shown {
        for &fr in &slots[k].frames {
            out.extend_from_slice(&(crate::frames::pa(fr as usize) as u64).to_le_bytes());
        }
    }
    Some(out)
}

/// The view taken back (the driver's close, SYS_DMA x1 = 5; either end of it ending, `cam_free`; a new view given): every block of it the blank frame (still read-only, so a reader in a frame as it goes reads zeros of nobody's, process.rs `blank_frame`), every TLB told, then the holder's capability gone, so SYS_DMA on its token answers ERR. Under the lock.
fn cam_view_drop(why: &str) {
    let v = unsafe { &mut *(&raw mut CAM_VIEW) };
    if v.holder == 0 {
        return;
    }
    // The blank frame was taken when the view was given (`cam_view_give`), so it is there.
    let blank = unsafe { SURFACE_BLANK }.expect("camera view: the blank frame its give took");
    for j in 0..v.n {
        crate::cachemap::cam_view_map(crate::cachemap::CAM_VIEW_VA.0 + j * crate::frames::FRAME, crate::frames::pa(blank), false);
    }
    crate::cachemap::surface_unmapped(crate::cachemap::CAM_VIEW_VA.0, v.n * crate::frames::FRAME);
    let vi = cam_view_index();
    let kk = k();
    if let Some(q) = kk.procs.iter_mut().flatten().find(|q| q.epoch == v.holder) {
        q.cspace.retain(|c| !matches!(c.object, Object::Dma(i) if i == vi));
    }
    gpu_cam_taken(v.holder);
    let line = alloc::format!("camera view: taken back ({why}), A#{} frames\n", v.n);
    kk.console.extend_from_slice(line.as_bytes());
    *v = CamView { holder: 0, driver: 0, first: 0, n: 0, cached: false };
}

/// SYS_DMA x1 = 6 (2026-11-18, docs/CAMERA.md "A cacheable view"): bytes `off` to `off + len` of the camera's frames cleaned and invalidated to the point of coherency (`dc civac` a line), then a barrier; on the `camera` region by the driver (offsets in its run), on `camera-view` by the holder (offsets in the view).
/// The driver calls it over a buffer as it hands the buffer to the holder (its MAP), after the frame's DMA_DONE: the front end does not snoop, so a line any core kept from an earlier read of the buffer, or fetched by speculation while the front end wrote it, is dropped, and the holder's write-back reads see DRAM.
/// The lines are clean (no write-back alias of a camera frame is written after the region's take cleaned it), so the clean writes nothing back and the invalidate loses nothing.
/// Clean and invalidate, never invalidate alone (`dc ivac`): the range's frames are found under the lock and maintained with it let go (`unlocked`, so a buffer's 2^18 lines hold nobody up), and a run given back meanwhile may be another process's frames by then, whose dirty lines a clean writes out where an invalidate would discard them.
/// By the kernel's own alias (the frames at their own address, `frame_block`, read-write at EL2), not the view's, which is read-only to EL2 too; a run lies in one range of the pool, so its addresses are side by side.
/// Answers the counter's ticks it took (the cost, for the driver's console line); ERR when the caller holds neither or the range is not inside it.
fn cam_lines(epoch: u64, view: bool, off: u64, len: u64) -> u64 {
    let c = unsafe { &*(&raw const CAM) };
    let v = unsafe { &*(&raw const CAM_VIEW) };
    let (first, n) = if view {
        if v.holder != epoch || v.holder == 0 {
            return ERR;
        }
        (c.first + v.first, v.n)
    } else {
        if c.owner != epoch || c.n == 0 {
            return ERR;
        }
        (c.first, c.n)
    };
    // WHY: the offset and length are the process's; a line past its frames would be another process's memory, or an address the kernel's map does not hold (a fault at EL2, under no lock but with the kernel's state mid-syscall).
    // The sum is tested only after both are known under the size, so it cannot wrap.
    let size = (n * crate::frames::FRAME) as u64;
    if off >= size || len > size || off + len > size {
        return ERR;
    }
    let a = crate::frames::pa(first) + off as usize;
    let len = len as usize;
    unlocked(|| {
        let t0 = crate::platform::cntpct();
        let mut x = a & !63;
        unsafe { core::arch::asm!("dsb sy") };
        while x < a + len {
            unsafe { core::arch::asm!("dc civac, {x}", x = in(reg) x) };
            x += 64;
        }
        unsafe { core::arch::asm!("dsb sy") };
        crate::platform::cntpct() - t0
    })
}

/// The camera's run back to the pool when its holder ends (`end_process`, `kill_locked`): the front end's S2MPUs shut first while its domain is on (the engine may still be writing: a holder that ended mid-stream leaves the switch on until the service loop's next pass), the window unmapped and every TLB told, then each frame wiped as it goes back (frames.rs `give`). Under the lock.
/// The view goes first when either its holder or the driver that gave it ends (`cam_view_drop`).
fn cam_free(epoch: u64) {
    {
        let v = unsafe { &*(&raw const CAM_VIEW) };
        if v.holder != 0 && (v.holder == epoch || v.driver == epoch) {
            cam_view_drop(if v.holder == epoch { "its holder ended" } else { "the driver ended" });
        }
    }
    let c = unsafe { &mut *(&raw mut CAM) };
    if c.owner != epoch || c.n == 0 {
        return;
    }
    let shut = crate::camera_power::set_region(0, 0);
    for j in 0..c.n {
        crate::cachemap::cam_map(crate::cachemap::CAM_VA.0 + j * crate::frames::FRAME, None);
    }
    crate::cachemap::surface_unmapped(crate::cachemap::CAM_VA.0, c.n * crate::frames::FRAME);
    // While a GPU space may still map them (`GPU_CAM` stale: its holder's view was taken back above, or before), kept from the pool, and admitted to the GPU as they were, until the GPU service says it stopped that client's jobs and unmapped them (`gpu_cam_settled` shuts them out then): a job still reading one is stopped by the service, never by a fence closed under it. Else shut out of the fence (none admitted, as no space maps them) and given back.
    let held = unsafe { (*(&raw const GPU_CAM)).stale } != 0;
    if held {
        // One run waits at a time: a second given back before the first's unmap was said (a driver started and ended within it) leaves the first out of the pool for the boot, never in it while a GPU space may reach it.
        unsafe { (*(&raw mut GPU_CAM)).drain = (c.first, c.n) };
    } else {
        gpu_cam_shut();
        for f in c.first..c.first + c.n {
            crate::frames::give(f);
        }
    }
    let line = alloc::format!("camera region: A#{} frames from G#{:X} {}, S2MPUs G#{shut:X} first (0 while the switch is off)\n", c.n, crate::frames::pa(c.first), if held { "held from the pool until the GPU service unmaps them" } else { "wiped back into the pool" });
    k().console.extend_from_slice(line.as_bytes());
    *c = Cam { owner: 0, first: 0, n: 0 };
}

/// Give the process with `epoch` a GPU block of `bytes` (a power of two from a frame to `GPU_BLOCK_MOST`): the first free slot of the blocks' window, grown to `bytes` (`block_grow`). The slot's index, or why none was given. Under the lock.
fn block_take(epoch: u64, bytes: usize) -> Result<usize, &'static str> {
    let draining = unsafe { GPU_DRAINING };
    let free = |b: &Block, k: usize| b.owner == 0 && !b.held && draining >> k & 1 == 0;
    let k = unsafe { (*(&raw const BLOCKS)).iter().enumerate().position(|(k, b)| free(b, k)) }.ok_or("no free GPU block (all sixteen are in use)")?;
    // A block given back maps the blank frame, so it is taken before the first block is.
    blank_frame().ok_or("the frame pool has no free frame")?;
    block_grow(k, bytes)?;
    unsafe { (*(&raw mut BLOCKS))[k].owner = epoch };
    Ok(k)
}

/// Grow slot k's block to `bytes` (a power of two from a frame to `GPU_BLOCK_MOST`; no smaller than it is, which leaves it as it is): the frames it lacks from the top of the pool (frames.rs `take_top`, zeroed as they are taken: the last owner's bytes are never the next one's, on the CPU or the GPU), mapped after the ones it holds (cachemap `gpub_map`), so its holder's pointers and its kernels' offsets keep their meaning, and admitted thru the GPU's fence (s2mpu `gpu_admit`). Refused, with why, when the pool would be left fewer than `BLOCK_RESERVE` free frames; nothing is taken then. Under the lock.
fn block_grow(k: usize, bytes: usize) -> Result<(), &'static str> {
    if !bytes.is_power_of_two() || bytes < crate::frames::FRAME || bytes > crate::cachemap::GPU_BLOCK_MOST {
        return Err("a GPU block is a power of two from 2 MiB to the ceiling");
    }
    let b = unsafe { &mut (*(&raw mut BLOCKS))[k] };
    let want = bytes / crate::frames::FRAME;
    if want <= b.n {
        return Ok(());
    }
    if crate::frames::free() < want - b.n + BLOCK_RESERVE {
        return Err("the frame pool has too few free frames for that GPU block");
    }
    let had = b.n;
    for j in had..want {
        match crate::frames::take_top() {
            Some(f) => b.frames[j] = f as u32,
            // WHY: the count above was taken under the same lock, so this is a pool that lied about its free frames: given back whole, loud in the reason.
            None => {
                for &f in &b.frames[had..j] {
                    crate::frames::give(f as usize);
                }
                return Err("the frame pool ran out while giving a GPU block");
            }
        }
    }
    let va = DMA[gpub0() + k].base;
    for j in had..want {
        let pa = crate::frames::pa(b.frames[j] as usize);
        crate::cachemap::gpub_map(va + j * crate::frames::FRAME, pa);
        crate::s2mpu::gpu_admit(pa, true);
    }
    crate::s2mpu::gpu_sync();
    // A slot given back before maps the blank frame where these now are: every core told to forget it.
    crate::cachemap::surface_unmapped(va + had * crate::frames::FRAME, (want - had) * crate::frames::FRAME);
    b.n = want;
    b.held = true;
    Ok(())
}

/// Slot k's frames back to the pool: its blocks mapping the blank frame first and every TLB told, the frames shut out of the GPU's fence, then each wiped as it goes back (frames.rs `give`). Only once nothing of its owner's can still write them: its process ended and no GPU job of it running (`regions_free`, `gpu_drained`). Under the lock.
fn block_release(k: usize) {
    let b = unsafe { &mut (*(&raw mut BLOCKS))[k] };
    let va = DMA[gpub0() + k].base;
    // A held block means one was given out, and the first one given made the blank.
    let blank = crate::frames::pa(unsafe { SURFACE_BLANK }.unwrap());
    for j in 0..b.n {
        crate::cachemap::gpub_map(va + j * crate::frames::FRAME, blank);
    }
    crate::cachemap::surface_unmapped(va, b.n * crate::frames::FRAME);
    for &f in &b.frames[..b.n] {
        crate::s2mpu::gpu_admit(crate::frames::pa(f as usize), false);
    }
    crate::s2mpu::gpu_sync();
    for &f in &b.frames[..b.n] {
        crate::frames::give(f as usize);
    }
    b.n = 0;
    b.held = false;
}

/// The blocks of the process with `epoch` back to free, their frames to the pool (or held back while a GPU job of its runs, `gpu_drain`). Under the lock.
fn regions_free(epoch: u64) {
    let draining = unsafe { GPU_DRAINING };
    for k in 0..GPU_BLOCK_SLOTS {
        if unsafe { (*(&raw const BLOCKS))[k].owner } == epoch {
            unsafe { (*(&raw mut BLOCKS))[k].owner = 0 };
            if draining >> k & 1 == 0 {
                block_release(k);
            }
        }
    }
}

/// SYS_DMA x1 = 2: the caller's own GPU block (DMA region `i`) grown to 2^`log` bytes (2026-11-13, the coordinator's call: a kist asks for the GPU memory it wants, in powers of two, up to `GPU_BLOCK_MOST`). Its size after, or ERR with why said on the console beside the pool's state. Under the lock.
/// Called under the kernel lock (a syscall's), which it lets go while the growth's frames are made clean when the pool has fewer known clean than it asks (`frames::prepare`, 2026-11-22, kernlock: a camera opening grows Lumis's block from 64 to 512 MiB, 224 frames, about 20 ms of zeroing under the lock when none is clean); `p` is not used after.
fn block_ask(p: &Process, cur: usize, i: usize, log: u64) -> u64 {
    let j = i - gpub0();
    let (epoch, name) = (p.epoch, p.name.clone());
    let had_n = unsafe { (*(&raw const BLOCKS))[j].n };
    // WHY the bounds: `log` is the caller's word; a shift past the word's width or past the ceiling is refused below (`block_grow`), so nothing is made ready for it.
    if log < usize::BITS as u64 && 1usize << log <= crate::cachemap::GPU_BLOCK_MOST {
        let want = (1usize << log) / crate::frames::FRAME;
        if want > had_n && want - had_n > crate::frames::clean_free() {
            unlocked(|| crate::frames::prepare(want - had_n));
            // Ended on another core meanwhile (its `regions_free` passed the block by or freed it): nothing to grow.
            if !proc_of(cur).is_some_and(|q| q.epoch == epoch && !q.dying) {
                return ERR;
            }
        }
    }
    let (owner, held) = unsafe { ((*(&raw const BLOCKS))[j].owner, (*(&raw const BLOCKS))[j].held) };
    // WHY: `log` is the caller's word, and a shift past the word's width is no size; past the ceiling (or under a frame) `block_grow` says why.
    let had = unsafe { (*(&raw const BLOCKS))[j].n } * crate::frames::FRAME;
    let t0 = crate::platform::cntpct();
    let r = if owner != epoch || !held {
        Err("not the caller's GPU block")
    } else if log >= usize::BITS as u64 {
        Err("a GPU block is a power of two from 2 MiB to the ceiling")
    } else {
        block_grow(j, 1usize << log)
    };
    let have = unsafe { (*(&raw const BLOCKS))[j].n } * crate::frames::FRAME;
    match r {
        Ok(()) => {
            // The growth's cost under the lock (its frames zeroed where the pool had not wiped them already), once a block.
            if have != had {
                let us = crate::platform::cntpct().wrapping_sub(t0) * 1_000_000 / crate::platform::cntfrq();
                let line = alloc::format!("gpu block: {} grew from A#{} to A#{} MiB in A#{us} us, A#{} MiB of the pool free\n", core::str::from_utf8(&name).unwrap_or("?"), had >> 20, have >> 20, (crate::frames::free() * crate::frames::FRAME) >> 20);
                k().console.extend_from_slice(line.as_bytes());
            }
            have as u64
        }
        Err(why) => {
            let line = alloc::format!("gpu block: {} asked for 2^A#{log} bytes (holding A#{} MiB, the most A#{} MiB): {why}, A#{} MiB of the pool free: refused\n", core::str::from_utf8(&name).unwrap_or("?"), have >> 20, crate::cachemap::GPU_BLOCK_MOST >> 20, (crate::frames::free() * crate::frames::FRAME) >> 20);
            k().console.extend_from_slice(line.as_bytes());
            ERR
        }
    }
}

/// `diag arcs`'s GPU blocks: each held, its owner, its size and its first frame's address.
fn blocks_report(s: &mut alloc::string::String) {
    let kk = k();
    let blocks = unsafe { &*(&raw const BLOCKS) };
    let mut n = 0;
    let mut bytes = 0;
    for (j, b) in blocks.iter().enumerate().filter(|(_, x)| x.held) {
        n += 1;
        bytes += b.n * crate::frames::FRAME;
        let name = kk.procs.iter().flatten().find(|p| p.epoch == b.owner).map(|p| core::str::from_utf8(&p.name).unwrap_or("?")).unwrap_or(if b.owner == 0 { "(draining)" } else { "(ended)" });
        let _ = core::fmt::Write::write_fmt(s, format_args!("gpu block A#{j} {name} A#{} MiB at G#{:X} frames from G#{:X}\n", (b.n * crate::frames::FRAME) >> 20, DMA[gpub0() + j].base, crate::frames::pa(b.frames[0] as usize)));
    }
    let _ = core::fmt::Write::write_fmt(s, format_args!("gpu blocks: A#{n} of A#{GPU_BLOCK_SLOTS} held, A#{} MiB, the most a block A#{} MiB\n", bytes >> 20, crate::cachemap::GPU_BLOCK_MOST >> 20));
}

/// The pucks' squares (docs/PUCKS.md, 2026-11-18): a slot of the pucks' window per puck, handed at its launch in its surface's place, its frames taken as the puck asks for them (SYS_DMA x1 = 6, `puck_ask`) and given back as it asks for fewer or ends.
pub const PUCK_SLOTS: usize = crate::cachemap::PUCK_SLOTS;
const _: () = assert!(PUCK_SLOTS == 16, "the DMA table lists sixteen puck slots");
/// The most frames behind one square: its slot's addresses.
const PUCK_FRAMES: usize = crate::cachemap::PUCK_SIZE / crate::frames::FRAME;
fn puck0() -> usize {
    DMA.iter().position(|r| r.name == "puck").unwrap()
}
/// Slot j of the pucks' window as a DMA region (`DMA`): its addresses whole, whatever frames are behind them, so a masked puck's handle spans the slot from its launch (`handle_of`); what it holds is `PuckSlot::n`.
const fn puck_slot(j: usize) -> Dma {
    Dma { name: "puck", base: crate::cachemap::PUCK_VA.0 + j * crate::cachemap::PUCK_SIZE, size: crate::cachemap::PUCK_SIZE }
}

/// A slot of the pucks' window: whose (a process epoch, 0 free), its generation (1 to 15, new at each hand-out, as a surface slot's), and the frames it holds from its start.
#[derive(Clone, Copy)]
struct PuckSlot {
    owner: u64,
    generation: u8,
    n: usize,
    frames: [u32; PUCK_FRAMES],
}
/// Every slot. Under the kernel lock.
static mut PUCKS: [PuckSlot; PUCK_SLOTS] = [PuckSlot { owner: 0, generation: 0, n: 0, frames: [0; PUCK_FRAMES] }; PUCK_SLOTS];
/// The squares' bell word as last rung (`BELL_PUCKS`): bits 4j to 4j + 3 slot j's generation while its puck lives, 0 for a slot nobody holds.
static PUCKS_LIVE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Give the process with `epoch` a square: the first free slot, no frames yet, every block of it mapping the blank frame (a read of the display's before the puck asks for frames, or after it gives them back, lands on nothing anyone owns). Under the lock.
fn puck_take(epoch: u64) -> Result<usize, &'static str> {
    let slots = unsafe { &mut *(&raw mut PUCKS) };
    let j = slots.iter().position(|s| s.owner == 0).ok_or("no free puck square (all sixteen are in use)")?;
    let blank = crate::frames::pa(blank_frame().ok_or("the frame pool has no free frame")?);
    let va = DMA[puck0() + j].base;
    for b in 0..PUCK_FRAMES {
        crate::cachemap::puck_map(va + b * crate::frames::FRAME, blank);
    }
    crate::cachemap::surface_unmapped(va, crate::cachemap::PUCK_SIZE);
    let s = &mut slots[j];
    s.generation = s.generation % 15 + 1;
    s.owner = epoch;
    s.n = 0;
    pucks_ring();
    Ok(j)
}

/// Slot j's square resized to `want` frames: the frames it lacks from the top of the pool (zeroed as they are taken), or those past `want` back to the pool (the blank frame mapped where they were, every core told, then each wiped as it goes back). Under the lock.
fn puck_resize(j: usize, want: usize) -> Result<(), &'static str> {
    let s = unsafe { &mut (*(&raw mut PUCKS))[j] };
    let va = DMA[puck0() + j].base;
    if want > s.n {
        // The same floor a GPU block keeps: a square taken to the pool's last frames would leave the next process's first frame, or an arc's growth, with none.
        if crate::frames::free() < want - s.n + BLOCK_RESERVE {
            return Err("the frame pool has too few free frames for that square");
        }
        let had = s.n;
        for b in had..want {
            match crate::frames::take_top() {
                Some(f) => s.frames[b] = f as u32,
                // WHY: the count above was taken under the same lock, so this is a pool that lied about its free frames: given back whole, loud in the reason.
                None => {
                    for &f in &s.frames[had..b] {
                        crate::frames::give(f as usize);
                    }
                    return Err("the frame pool ran out while giving a square");
                }
            }
        }
        for b in had..want {
            crate::cachemap::puck_map(va + b * crate::frames::FRAME, crate::frames::pa(s.frames[b] as usize));
        }
        crate::cachemap::surface_unmapped(va + had * crate::frames::FRAME, (want - had) * crate::frames::FRAME);
    } else if want < s.n {
        let blank = crate::frames::pa(unsafe { SURFACE_BLANK }.unwrap());
        for b in want..s.n {
            crate::cachemap::puck_map(va + b * crate::frames::FRAME, blank);
        }
        crate::cachemap::surface_unmapped(va + want * crate::frames::FRAME, (s.n - want) * crate::frames::FRAME);
        for &f in &s.frames[want..s.n] {
            crate::frames::give(f as usize);
        }
    }
    s.n = want;
    Ok(())
}

/// The bytes a square of `n` frames holds past its porch: none without a frame (the porch lies in the first).
fn square_bytes(n: usize) -> usize {
    if n == 0 { 0 } else { n * crate::frames::FRAME - crate::cachemap::SURFACE_PORCH }
}

/// SYS_DMA x1 = 6 (2026-11-18): the caller's own square (DMA region `i`) resized to hold `bytes` past its porch, in whole frames, larger or smaller. Its bytes held after (frames, less the porch), or ERR with why on the console.
fn puck_ask(p: &Process, i: usize, bytes: u64) -> u64 {
    let j = i - puck0();
    let owner = unsafe { (*(&raw const PUCKS))[j].owner };
    // WHY the bound: `bytes` is the caller's word; past the slot's addresses no frame could be mapped.
    let r = if owner != p.epoch {
        Err("not the caller's square")
    } else if bytes > (crate::cachemap::PUCK_SIZE - crate::cachemap::SURFACE_PORCH) as u64 {
        Err("past the square's addresses")
    } else {
        puck_resize(j, (bytes as usize + crate::cachemap::SURFACE_PORCH).div_ceil(crate::frames::FRAME))
    };
    match r {
        Ok(()) => square_bytes(unsafe { (*(&raw const PUCKS))[j].n }) as u64,
        Err(why) => {
            let line = alloc::format!("puck square: {} asked for A#{bytes} bytes: {why}, A#{} MiB of the pool free: refused\n", core::str::from_utf8(&p.name).unwrap_or("?"), (crate::frames::free() * crate::frames::FRAME) >> 20);
            k().console.extend_from_slice(line.as_bytes());
            ERR
        }
    }
}

/// The squares of the process with `epoch` back to free, their frames to the pool, and the display rung (`pucks_ring`). Under the lock.
fn pucks_free(epoch: u64) {
    let slots = unsafe { &mut *(&raw mut PUCKS) };
    let mut any = false;
    for j in 0..PUCK_SLOTS {
        if slots[j].owner == epoch {
            // A slot held means a square was given, and giving it made the blank, so the resize to none cannot fail.
            let _ = puck_resize(j, 0);
            slots[j].owner = 0;
            any = true;
        }
    }
    if any {
        pucks_ring();
    }
}

/// The squares' owners for the display (`BELL_PUCKS`), from the kernel's own table: slot j's generation while its owner lives and is not ending, so the display reads a square only under the generation the puck's own call named (display puck.rs).
fn pucks_ring() {
    let kk = k();
    let slots = unsafe { &*(&raw const PUCKS) };
    let mut live = 0u64;
    for (j, s) in slots.iter().enumerate() {
        if s.owner != 0 && kk.procs.iter().flatten().any(|p| p.epoch == s.owner && !p.dying) {
            live |= (s.generation as u64) << (4 * j);
        }
    }
    if PUCKS_LIVE.swap(live, core::sync::atomic::Ordering::AcqRel) != live {
        bell_ring(BELL_PUCKS, live);
    }
}

/// The square process `p` holds, for the stamp on its `puck` calls (`puck_stamp`): its slot, generation and the bytes it holds past the porch; None when it holds none.
fn puck_held(p: &Process) -> Option<(u8, u8, u32)> {
    let p0 = puck0();
    p.cspace.iter().find_map(|c| match c.object {
        Object::Dma(i) if i >= p0 && i < p0 + PUCK_SLOTS => {
            let s = unsafe { &(*(&raw const PUCKS))[i - p0] };
            (s.owner == p.epoch).then(|| ((i - p0) as u8, s.generation, square_bytes(s.n) as u32))
        }
        _ => None,
    })
}

/// `diag arcs`'s squares: each held, its owner, generation and size.
fn pucks_report(s: &mut alloc::string::String) {
    let kk = k();
    let slots = unsafe { &*(&raw const PUCKS) };
    let mut n = 0;
    for (j, x) in slots.iter().enumerate().filter(|(_, x)| x.owner != 0) {
        n += 1;
        let name = kk.procs.iter().flatten().find(|p| p.epoch == x.owner).map(|p| core::str::from_utf8(&p.name).unwrap_or("?")).unwrap_or("(ended)");
        let _ = core::fmt::Write::write_fmt(s, format_args!("puck square A#{j} {name} generation A#{} A#{} MiB at G#{:X}\n", x.generation, (x.n * crate::frames::FRAME) >> 20, DMA[puck0() + j].base));
    }
    let _ = core::fmt::Write::write_fmt(s, format_args!("puck squares: A#{n} of A#{PUCK_SLOTS} held\n"));
}

/// Whether a thread of process `pi` has a call on the `gpu` endpoint that its server has taken and not answered: a job of its may be on the GPU now. Looked at before the process's reply tokens are dropped (`end_process`, `kill_locked`). Under the lock.
fn gpu_job_in_flight(pi: usize) -> bool {
    let kk = k();
    let ep = endpoint(b"gpu");
    let mine = |t: usize| kk.threads.get(t).and_then(|x| x.as_ref()).is_some_and(|x| x.proc == pi);
    kk.procs.iter().flatten().any(|q| q.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == ep) && c.rights & RIGHT_RECV != 0) && q.cspace.iter().any(|c| matches!(c.object, Object::Reply(t) if mine(t))))
}

/// A process ending with a GPU job in flight (`gpu_job_in_flight`): its blocks and surfaces held back from the next owner until the GPU service takes its next request (`gpu_drained`), their frames kept in the pool's stead. The service runs one job at a time and ends a job it gives up on (its group terminated) before it takes the next request, so by then nothing of the ended process's runs on the GPU. Called before its blocks and surfaces are freed. Under the lock.
fn gpu_drain(epoch: u64) {
    let blocks = unsafe { &*(&raw const BLOCKS) };
    let slots = unsafe { &*(&raw const SLOTS) };
    for (j, b) in blocks.iter().enumerate() {
        if b.owner == epoch {
            unsafe {
                GPU_DRAINING |= 1 << j;
                BLOCK_DRAIN_EPOCH[j] = epoch;
            }
        }
    }
    for (k, s) in slots.iter().enumerate() {
        if s.owner == epoch {
            unsafe {
                SURF_DRAINING |= 1 << k;
                SURF_DRAIN_EPOCH[k] = epoch;
            }
        }
    }
}

/// The epochs whose GPU work the GPU service holds (2026-11-20, docs/GPU-KISTS.md "The queue"; SYS_DMA x1 = 7 on `arena`).
/// A client with a queue submits jobs without a call, so no held call says a job of its is on the GPU (`gpu_job_in_flight`): the service says so instead, from its queue's open until its group has ended.
/// A held epoch's blocks and surfaces stay drained past its end until the service lets it go (`gpu_hold`), whatever requests the service takes meanwhile.
/// The queue word is the client's `SUBMIT` (ferros_gpuq), which the service's thread for that queue waits on: at the client's end the kernel sets its bit 63 and wakes it, so the thread finds its client gone even when the end comes between its look and its wait.
/// At the service's end the kernel sets bit 63 of every held queue's `DONE`, 64 bytes on, which the client waits on, so no client waits for ever on a service that is gone.
/// One a slot of the service's seven, and one spare: (epoch, the queue word's window address or 0). Under the kernel lock.
static mut GPU_HOLDS: [(u64, usize); 8] = [(0, 0); 8];
/// The epoch each draining block (`GPU_DRAINING`) and surface (`SURF_DRAINING`) was drained for, so a held epoch's are kept while the rest go at the service's next request. Under the kernel lock.
static mut BLOCK_DRAIN_EPOCH: [u64; GPU_BLOCK_SLOTS] = [0; GPU_BLOCK_SLOTS];
static mut SURF_DRAIN_EPOCH: [u64; SURFACE_SLOTS] = [0; SURFACE_SLOTS];
/// Bit 63 of a queue word, set by the kernel at an end (`GPU_HOLDS`): ferros_gpuq `GONE`.
const GPU_QUEUE_GONE: u64 = 1 << 63;
/// `DONE`'s distance from `SUBMIT` in a queue's header, in bytes (ferros_gpuq `DONE`, word 8).
const GPU_QUEUE_DONE: usize = 64;

/// Whether the GPU service holds `epoch`'s GPU work (`GPU_HOLDS`). Under the lock.
fn gpu_held(epoch: u64) -> bool {
    epoch != 0 && unsafe { (*(&raw const GPU_HOLDS)).iter().any(|h| h.0 == epoch) }
}

/// Bit 63 set in the queue word at `a` (a window address `gpu_hold` checked) and its waiters woken, in every process.
fn gpu_queue_mark(a: usize) {
    // SAFETY: `a` is an aligned word inside a held block's frames (`gpu_hold` checked it, and a held epoch's block stays held, drained, until the hold is let go), mapped in the kernel's own view of the blocks' window.
    unsafe { (*(a as *const core::sync::atomic::AtomicU64)).fetch_or(GPU_QUEUE_GONE, core::sync::atomic::Ordering::SeqCst) };
    mbox_wake(a, u64::MAX);
}

/// A process ending (`end_process`, `kill_locked`), before its blocks and surfaces are freed: drained when a job of its may be on the GPU (a call the service has taken, `busy`, or work the service holds for it, `gpu_held`).
/// Its held queue's word is marked and the service's threads waiting on words of its blocks woken, so the one serving its queue finds it gone, ends its group and lets it go. Under the lock.
fn gpu_ending(epoch: u64, busy: bool) {
    // The GPU service itself ending (the holder of the whole blocks' window): every held queue's `DONE` marked, so its client stops waiting; the holds go with it, and what they kept stays drained until the next service's first request, which it takes only after its GPU's reset (payloads/gpu), as an ended client's call-held job always was (`gpu_drained`).
    let service = k().procs.iter().flatten().any(|p| p.epoch == epoch && p.cspace.iter().any(|c| matches!(c.object, Object::Dma(i) if DMA[i].name == "arena")));
    if service {
        let holds = unsafe { &mut *(&raw mut GPU_HOLDS) };
        for h in holds.iter_mut() {
            if h.1 != 0 {
                gpu_queue_mark(h.1 + GPU_QUEUE_DONE);
            }
            *h = (0, 0);
        }
    }
    let held = gpu_held(epoch);
    if busy || held {
        gpu_drain(epoch);
    }
    dv_ending(epoch, busy, held, service);
    if held {
        for h in unsafe { (*(&raw const GPU_HOLDS)).iter() } {
            if h.0 == epoch && h.1 != 0 {
                gpu_queue_mark(h.1);
            }
        }
        let blocks = unsafe { &*(&raw const BLOCKS) };
        for (j, b) in blocks.iter().enumerate() {
            if b.owner == epoch && b.held {
                let base = DMA[gpub0() + j].base;
                wake_range(base, base + b.n * crate::frames::FRAME);
            }
        }
    }
}

/// SYS_DMA x1 = 7 on `arena`, the GPU service's: `epoch`'s GPU work held (`on`), its queue's `SUBMIT` word at window address `word` (0: none), or let go once its group has ended.
/// The word must be an aligned word of a block that epoch holds, with its `DONE` inside the same frames.
/// Letting go of an epoch that has ended gives its drained blocks and surfaces back now. 0, or ERR when the holds are full, a hold names epoch 0, or the word is none of that epoch's. Under the lock.
fn gpu_hold(epoch: u64, on: bool, word: u64) -> u64 {
    let holds = unsafe { &mut *(&raw mut GPU_HOLDS) };
    if on {
        let a = word as usize;
        let (lo, hi) = crate::cachemap::GPUB_VA;
        // WHY: the epoch and the word are the service's to name, and the kernel writes the word at that epoch's end: only a held block's own frames, that epoch's, may be written there.
        let ok = a == 0
            || (a >= lo && a < hi && a % 8 == 0 && {
                let j = (a - lo) / crate::cachemap::GPU_BLOCK_MOST;
                let b = unsafe { &(*(&raw const BLOCKS))[j] };
                b.held && b.owner == epoch && a + GPU_QUEUE_DONE + 8 <= lo + j * crate::cachemap::GPU_BLOCK_MOST + b.n * crate::frames::FRAME
            });
        if epoch == 0 || !ok {
            return ERR;
        }
        if let Some(h) = holds.iter_mut().find(|h| h.0 == epoch) {
            h.1 = a;
            return 0;
        }
        return match holds.iter().position(|h| h.0 == 0) {
            Some(i) => {
                holds[i] = (epoch, a);
                0
            }
            None => ERR,
        };
    }
    for h in holds.iter_mut().filter(|h| h.0 == epoch) {
        *h = (0, 0);
    }
    // An ending process's blocks and surfaces were freed (drained) at its end, its threads elsewhere or not.
    if gpu_alive(epoch) == 0 {
        gpu_released(epoch);
    }
    0
}

/// SYS_DMA x1 = 8 on `arena`: whether a process with `epoch` is alive and not ending (1) or not (0), for the GPU service's thread that a client's end woke (`gpu_ending`). Under the lock.
fn gpu_alive(epoch: u64) -> u64 {
    k().procs.iter().flatten().any(|p| p.epoch == epoch && !p.dying) as u64
}

/// Every block and surface drained for `epoch` given back: its owner ended and the service let its GPU work go. Under the lock.
fn gpu_released(epoch: u64) {
    let (ending, end_epoch, end_seq) = unsafe { ((*(&raw const DV)).ending, (*(&raw const DV)).end_epoch, (*(&raw const DV)).end_seq) };
    if ending && end_epoch != 0 && end_epoch == epoch {
        dv_drop("the GPU service let its display go", end_seq);
    }
    for j in 0..GPU_BLOCK_SLOTS {
        let (owner, held) = unsafe { ((*(&raw const BLOCKS))[j].owner, (*(&raw const BLOCKS))[j].held) };
        if unsafe { GPU_DRAINING } >> j & 1 == 1 && unsafe { BLOCK_DRAIN_EPOCH[j] } == epoch {
            unsafe {
                GPU_DRAINING &= !(1 << j);
                BLOCK_DRAIN_EPOCH[j] = 0;
            }
            if owner == 0 && held {
                block_release(j);
            }
        }
    }
    let slots = unsafe { &*(&raw const SLOTS) };
    for k in 0..SURFACE_SLOTS {
        if unsafe { SURF_DRAINING } >> k & 1 == 1 && unsafe { SURF_DRAIN_EPOCH[k] } == epoch {
            unsafe {
                SURF_DRAINING &= !(1 << k);
                SURF_DRAIN_EPOCH[k] = 0;
            }
            if slots[k].owner == 0 && slots[k].held {
                surface_give_back(k);
            }
        }
    }
}

/// The kernel's stamp ahead of every call on the identity service's `facets` endpoint (docs/FACETS.md "To build" 2, 2026-11-13; ferros_sys::facets `Stamp` reads it): [flags u8: bit 0 a kist the kist job launched, bit 1 be you granted, bit 2 people granted, bit 3 the person present, bit 4 its alias held][the kist's developer key 32][its option 32][its alias's Ed25519 public key 32, zeros without bit 4]. Written here for every call, so no caller can write one: the service binds only the alias the kernel derives for the kist that calls, answers only what the person granted it, and only while they are present. The alias's public key only; the seed is wiped here. Under the lock.
fn facets_stamp(p: &Process, msg: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(97 + msg.len());
    let (developer, option) = p.gate.filter(|_| p.kist).unwrap_or(([0; 32], [0; 32]));
    let present = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire);
    let alias = match p.gate {
        Some((d, o)) if p.kist => crate::service::alias(&d, &o).map(|mut s| {
            let mut sk = ed25519_dalek::SigningKey::from_bytes(&s);
            let pk = sk.verifying_key().to_bytes();
            // The signing key holds the seed by value and wipes nothing when dropped.
            crate::keys::wipe_value_to_dram(&mut sk);
            crate::keys::wipe_to_dram(&mut s);
            pk
        }),
        _ => None,
    };
    out.push((p.gate.is_some() && p.kist) as u8 | (p.facets & 3) << 1 | (present as u8) << 3 | (alias.is_some() as u8) << 4);
    out.extend_from_slice(&developer);
    out.extend_from_slice(&option);
    out.extend_from_slice(&alias.unwrap_or([0; 32]));
    out.extend_from_slice(&msg);
    out
}

/// The kernel's stamp ahead of every call on the `puck` endpoint (docs/PUCKS.md, 2026-11-17): the caller's kist identity as the gate found it at launch, its developer's Ed25519 key and its option (64 bytes), or zeros for a caller that is no kist (the display then finds no circle for it).
/// Then its square (2026-11-18): the slot, generation and bytes it holds, from the kernel's own table, so the display reads a square only where and as far as its puck holds one.
/// Lean on purpose: a puck calls at each of its deadlines and, while its circle is in view, at its sensor's rate (the compass), so no alias is derived here as `facets_stamp` does.
fn puck_stamp(p: &Process, msg: Vec<u8>) -> Vec<u8> {
    let (developer, option) = p.gate.filter(|_| p.kist).unwrap_or(([0; 32], [0; 32]));
    let mut out = Vec::with_capacity(PUCK_STAMP + msg.len());
    out.extend_from_slice(&developer);
    out.extend_from_slice(&option);
    // Its square as the kernel holds it (2026-11-18): slot, generation, a zero, then the bytes past the porch, u32; slot G#FF when it holds none.
    let (slot, generation, bytes) = puck_held(p).unwrap_or((0xFF, 0, 0));
    out.extend_from_slice(&[slot, generation, 0, 0]);
    out.extend_from_slice(&bytes.to_le_bytes());
    out.extend_from_slice(&msg);
    out
}
/// The stamp's length: identity 64, square 8.
const PUCK_STAMP: usize = 72;

/// The kernel's stamp ahead of every call on the `gpu` endpoint (GPU-KISTS.md plan step 1, 2026-11-12; ferros_sys::gpu `Stamp` reads it): [the caller's epoch u64][flags u8: bit 0 a kist, bit 1 a production build, bit 2 frames follow, bit 3 the display's view follows (2026-11-21, `dv_stamp`)][n u8][n × (base u64, size u64)][then, for each region in order, its frames' physical addresses, u64 each, its size over 2 MiB of them], the regions of every GPU-reachable DMA grant it holds: its GPU block (its slot's address in the blocks' window and the bytes of frames it holds, since 2026-11-13; a block of the fixed layer arena at its physical address until then), each surface it holds (one slot, at its address in the surfaces' window). Written here, in front of the caller's bytes, for every call, so no caller can write one: the service maps a client's address space from these regions and frames and from nothing a client says. A holder of a whole window (the display, init, the GPU service) is stamped with none of it: no client's memory is its to have the GPU write. Under the lock.
/// A block's frames change only as it grows (`block_grow` appends; nothing takes one back while it is held), and a surface's never while it is held, so the regions alone name the frames for a caller: the service keys a client's address space by them.
fn gpu_stamp(p: &Process, msg: Vec<u8>) -> Vec<u8> {
    let slots = unsafe { &*(&raw const SLOTS) };
    let blocks = unsafe { &*(&raw const BLOCKS) };
    let (s0, g0) = (surface0(), gpub0());
    let mut regions: Vec<(usize, usize)> = Vec::new();
    let mut frames: Vec<u64> = Vec::new();
    for c in &p.cspace {
        if let Object::Dma(i) = c.object {
            let (b, n) = region(i);
            if n == 0 || regions.contains(&(b, n)) {
                continue;
            }
            if DMA[i].name == "gpu-block" {
                regions.push((b, n));
                frames.extend(blocks[i - g0].frames[..blocks[i - g0].n].iter().map(|&f| crate::frames::pa(f as usize) as u64));
            } else if DMA[i].name == "surface" && slots[i - s0].held {
                regions.push((b, n));
                frames.extend(slots[i - s0].frames.iter().map(|&f| crate::frames::pa(f as usize) as u64));
            }
        }
    }
    // The camera's view, its holder's alone (2026-11-20, `GPU_CAM`): its frames read-only to its GPU jobs while it holds the view, last, so a stamp's other regions keep their order.
    if let Some((r, f)) = gpu_cam_stamp(p) {
        regions.push(r);
        frames.extend(f);
    }
    // The display's view (GPU-COMPOSE.md 2.1, 2026-11-21): the phases and the windows it composes, read-only, after the frames (bit 3; `dv_stamp`).
    let view = dv_stamp(p, &msg);
    let mut out = Vec::with_capacity(10 + 16 * regions.len() + 8 * frames.len() + msg.len());
    out.extend_from_slice(&p.epoch.to_le_bytes());
    out.push(p.kist as u8 | (cfg!(ferros_production) as u8) << 1 | 1 << 2 | (view.is_some() as u8) << 3);
    // WHY no check: a process holds a handful of DMA grants (init, the most, about a dozen), far under the byte's 255.
    out.push(regions.len() as u8);
    for (b, n) in regions {
        out.extend_from_slice(&(b as u64).to_le_bytes());
        out.extend_from_slice(&(n as u64).to_le_bytes());
    }
    for f in frames {
        out.extend_from_slice(&f.to_le_bytes());
    }
    if let Some(v) = view {
        out.extend_from_slice(&v);
    }
    out.extend_from_slice(&msg);
    out
}

/// The GPU service took a request: whatever waited for its last job to end may be given out again, a drained block's or surface's frames back to the pool. Under the lock.
fn gpu_drained() {
    let (ending, end_epoch, end_seq) = unsafe { ((*(&raw const DV)).ending, (*(&raw const DV)).end_epoch, (*(&raw const DV)).end_seq) };
    if ending && !gpu_held(end_epoch) {
        dv_drop(if end_epoch == 0 { "the GPU service ended, the next one serves" } else { "the GPU service took its next request" }, end_seq);
    }
    // A held epoch's drains stay (`GPU_HOLDS`): its queue's jobs run with no call, so a request taken says nothing of them.
    let kept = |e: u64| gpu_held(e);
    let mut d = unsafe { GPU_DRAINING };
    for j in 0..GPU_BLOCK_SLOTS {
        if d >> j & 1 == 1 && kept(unsafe { BLOCK_DRAIN_EPOCH[j] }) {
            d &= !(1 << j);
        }
    }
    unsafe { GPU_DRAINING &= !d };
    for j in 0..GPU_BLOCK_SLOTS {
        let (owner, held) = unsafe { ((*(&raw const BLOCKS))[j].owner, (*(&raw const BLOCKS))[j].held) };
        if d >> j & 1 == 1 && owner == 0 && held {
            block_release(j);
        }
    }
    let slots = unsafe { &*(&raw const SLOTS) };
    let mut d = unsafe { SURF_DRAINING };
    for k in 0..SURFACE_SLOTS {
        if d >> k & 1 == 1 && kept(unsafe { SURF_DRAIN_EPOCH[k] }) {
            d &= !(1 << k);
        }
    }
    unsafe { SURF_DRAINING &= !d };
    for k in 0..SURFACE_SLOTS {
        if d >> k & 1 == 1 && slots[k].owner == 0 && slots[k].held {
            surface_give_back(k);
        }
    }
}

/// SYS_CALLER's bit for surface slot k: bit `CALLER_SURFACE0 + k` (bits 32 to 47 since 2026-11-12, sixteen slots; until then bits 0 to 3).
pub const CALLER_SURFACE0: u32 = 32;

/// What process `p` holds that a server decides by (SYS_CALLER): bit `CALLER_SURFACE0 + k` for surface slot k (and bit k for slots 0 to 3, the old layout), held as its own or inside a region that holds it whole (the display's and the GPU service's whole window); `CALLER_SETTINGS` for the Settings capability; `CALLER_KIST` for a kist. From its capability table, so a surface passed on at a spawn counts for both holders, and one freed at an end for neither (the table goes with the process).
fn caller_holds(p: &Process) -> u64 {
    let s0 = crate::cachemap::SURF_VA.0;
    let size = crate::cachemap::SURFACE_SIZE;
    let mut bits = (p.kist as u64) * CALLER_KIST;
    for c in &p.cspace {
        match c.object {
            Object::Dma(i) => {
                let (b, n) = region(i);
                for k in 0..SURFACE_SLOTS {
                    let at = s0 + k * size;
                    if b <= at && at + size <= b + n {
                        // Slots 0 to 3 at bits 0 to 3 as well, for a display from before (four surfaces), which reads them there.
                        bits |= 1 << (CALLER_SURFACE0 as usize + k) | if k < SURFACES_OLD { 1 << k } else { 0 };
                    }
                }
            }
            Object::Settings if c.rights & RIGHT_SEND != 0 => bits |= CALLER_SETTINGS,
            _ => {}
        }
    }
    bits
}

/// Whether process `p` holds the `surfaces` region (the display server: it composes every app's surface, so it alone may watch their owners).
fn holds_surfaces(p: &Process) -> bool {
    p.cspace.iter().any(|c| matches!(c.object, Object::Dma(i) if DMA[i].name == "surfaces"))
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
/// What the partition holds, by the code that places it: the old ferros_vault file at its start (shim, 256 blocks), the old text log at 1 GiB and the flash log's ring after it to 1 GiB + 192 MiB (flashlog.rs), the kernel's vault at 2 GiB to 2.5 GiB (kvault.rs), the vault process at 4 GiB to 20 GiB (`vault` above); from 20 GiB to the partition's end (128 GiB on husky, 77.7 GiB on akita) nothing else is written by any code (docs/RAW-VIDEO.md, proven 2026-11-19).
/// `clips` (docs/RAW-VIDEO.md "The recorder", 2026-11-19): the vault process's single-copy extents (manifestus `extent`), RAW video written once at the flash's rate, no mirror; 48 GiB from 24 GiB in, inside the proven-free stretch on both boards (`CLIPS_*` below hold it there at build time).
/// `rawbench` (development): the flash's sustained rate measured by std/rawbench, 32 GiB from 72 GiB in, past `clips` (it writes the same bytes everywhere); on akita (77.7 GiB) it lies past the partition's end and the kernel refuses it.
#[cfg(not(ferros_production))]
pub static WINDOWS: &[BlockWindow] = &[BlockWindow { name: "vault", offset: 1 << 20, blocks: 1 << 22 }, BlockWindow { name: "clips", offset: CLIPS_OFFSET, blocks: CLIPS_BLOCKS }, BlockWindow { name: "rawbench", offset: RAWBENCH_OFFSET, blocks: 1 << 23 }];
#[cfg(ferros_production)]
pub static WINDOWS: &[BlockWindow] = &[BlockWindow { name: "vault", offset: 1 << 20, blocks: 1 << 22 }, BlockWindow { name: "clips", offset: CLIPS_OFFSET, blocks: CLIPS_BLOCKS }];

/// The `clips` window: 6 x 2^20 blocks in (24 GiB), 12 x 2^20 blocks long (48 GiB).
pub const CLIPS_OFFSET: u64 = 6 << 20;
pub const CLIPS_BLOCKS: u64 = 12 << 20;
/// The development `rawbench` window's start: 18 x 2^20 blocks (72 GiB), past `clips`.
pub const RAWBENCH_OFFSET: u64 = 18 << 20;
/// The stretch nothing else writes: from the vault process's window's end (20 GiB) to the smallest ferros partition's end (akita's, A#20357627 blocks, 77.7 GiB; husky's is A#33554427).
pub const PROVEN_FREE_FROM: u64 = (1 << 20) + (1 << 22);
pub const PROVEN_FREE_TO: u64 = 20_357_627;
// The hard bound on the recording region, at build time: `clips` inside the proven-free stretch, so a clip can never land on the vault's mirrors, the flash log or the kernel's vault, nor past akita's partition.
const _: () = assert!(CLIPS_OFFSET >= PROVEN_FREE_FROM && CLIPS_OFFSET + CLIPS_BLOCKS <= PROVEN_FREE_TO);
// rawbench (development) writes the same bytes all over its window: never over `clips`.
const _: () = assert!(RAWBENCH_OFFSET >= CLIPS_OFFSET + CLIPS_BLOCKS);

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
    // Stowed kists' signed capsules (KISTS.md), the device's: each under its own content address (the blake3 of the capsule, a 32-byte address name), so two people on one device who stowed the same app share one copy. The kernel reads them to launch a kist; the stower writes them.
    StoreScope { name: "capsules", prefix: "capsules." },
    // The person's list of stowed kists and their answers (`kists.list`, ferros_kist; identity class, kvault IDENTITY): init reads it to launch a kist, the map shows it and writes the person's answers. Stands for the common area's `kists` until the common area's named grants exist (VAULT-OUT.md).
    StoreScope { name: "kists", prefix: "kists." },
    // The camera driver's (payloads/camera, docs/CAMERA.md "The driver"): the main sensor's init (`cam.boitata.init`), the CPU's setup writes around the PDMA start (`cam.ispfe.prog`), or both as one bundle (`cam.frame`), and the front end's PDMA job (`cam.boitata.pdma`): the vendor's settings, generated from oracle captures, the vault's and Harbor's, never the repo.
    StoreScope { name: "cam", prefix: "cam." },
];
/// The context deriving a process's own domain name (`Process::domain`) from its ihi.
const KIST_DOMAIN_CONTEXT: &str = "ferros 2026-11-08 kist domain";
/// The context binding a kist's ira-derived key to its gate identity (`SYS_IRA`).
const KIST_IRA_CONTEXT: &str = "ferros 2026-11-08 kist ira";
/// A process's ira key bound to the person present (SYS_IRA x4 bit 0): the WiFi address a person wears on a network, theirs and this device's together.
const PERSON_IRA_CONTEXT: &str = "ferros 2026-11-09 person ira";
/// A system process's key for this power session (SYS_IRA x4 bit 2), from the wairua: the WiFi address while nobody is present, the same on every join in one boot and unlinkable to any other boot.
const SESSION_KEY_CONTEXT: &str = "ferros 2026-11-14 process session key";
/// The root those keys come from (`wairua::derive`), one per power session.
const SESSION_ROOT_CONTEXT: &str = "ferros 2026-11-14 process session root";
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
/// Attest with a tag (AUTH.md), SYS_UNLOCK ops 6-9 (`tag_syscall`), as the vault's owner serves them (service.rs `store_tag`).
pub enum TagOp<'a> {
    /// Unlock by the tag whose UID this is; the seed into the second, on success.
    Unlock(&'a [u8], &'a mut [u8; 32]),
    /// Keep the tag whose UID this is for the unlocked identity.
    Keep(&'a [u8]),
    /// Forget the tag whose id starts so, or every tag.
    Forget(Option<[u8; crate::kvault::TAG_SHOWN]>),
    /// The kept tags, `TAG_ENTRY` bytes each, into the vector.
    List(&'a mut Vec<u8>),
}
pub static mut TAG: Option<fn(TagOp) -> Result<u64, ()>> = None;
/// One kept tag as op 9 lists it: the first `TAG_SHOWN` bytes of its id, then when it was kept (Eagle time in oscillations, i64 LE).
pub const TAG_ENTRY: usize = crate::kvault::TAG_SHOWN + 8;
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
/// The largest a get (`SYS_STORE` op get, forwarded to the vault process) reads in one call: 1 MiB since 2026-11-11, sixteen pieces the vault process fetches and opens as one batch on its four cores, and a quarter of the round trips of a large value read at STORE_MAX (93 for the 24 MB wallpaper, each with its stamp, its hand-offs and the reader's turn between them). The reply goes from the vault process's buffer straight into the caller's (`SYS_REPLY`), never thru the kernel's heap. Only while the vault process serving has said it answers that much (`SYS_VAULT` op 3, `VAULT_GET`): one from before has a reply buffer of one STORE_MAX transfer. Sets, lists and the kernel's own vault stay at STORE_MAX; a larger get is refused (as a kernel from before refuses it), and a reader drops to STORE_MAX (ferros_sys STORE_GET_MAX).
pub const STORE_GET_MAX: usize = 1 << 20;
/// The largest plain reply (`SYS_REPLY`, 2026-11-14): 8 MiB, past the camera's preview frame at bin 2 (2040 x 1536 samples of 16 bits and a 64-byte header, 6 MiB; docs/CAMERA.md "The endpoint"). A reply past `STORE_MAX` is copied from the server's memory straight into the caller's under the kernel lock (about a millisecond for the preview frame); a full raw frame wants a mapping instead (KISTS.md "Lumis").
pub const REPLY_MAX: usize = 1 << 23;

/// The vault process's endpoint (VAULT-OUT.md, step 3). While a process receives on it, every `SYS_STORE` call is forwarded there (`store_forward`), stamped by the kernel with the domain keys of the caller's scope; while none does, the kernel's own vault answers as before. Nothing may call it directly (a message on it is the kernel's), and only a spawn holding the vault right (`Grant::Vault`) may serve it.
const VAULT_EP: &[u8] = b"vault";
/// Names under a scope that stay in the kernel's vault because the kernel reads them itself (aoc.rs: the AoC firmware): never forwarded, never migrated.
const KERNEL_HELD: [&str; 1] = ["aoc.fw."];
/// The device's domain base, registered by the vault's owner (service.rs): (the identity's?) -> the base key; always None for the person's, whose base is held sealed (`domain_keys` asks the kernel's vault for their keys instead).
pub static mut DOMAIN_BASE: Option<fn(bool) -> Option<[u8; 32]>> = None;
const DOMAIN_ADDR_CONTEXT: &str = "ferros 2026-11-07 vault domain address";
const DOMAIN_SEAL_CONTEXT: &str = "ferros 2026-11-07 vault domain seal";
/// Entries may wait in the kernel's vault for the vault process to take (`SYS_VAULT`): set from boot, by a bridge `store put` of a scoped name, and at unlock; cleared when the vault process asks and finds none. The stamp's flag bit 0.
pub static MIGRATE_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// Moved at every identity lock (kvault `lock`): the stamp carries it, and the vault process drops what it holds of the person's (its cache's plaintext) when it changes.
pub static LOCK_EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Kists' aliases handed out this boot (SYS_IRA's alias bit), for `diag identity`.
pub static ALIASES_GIVEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Bit 62 of a `Calling` state's reply length: the call is a forwarded `SYS_STORE`, whose reply is [the syscall's value u64 LE][the bytes for the caller's buffer].
const STORE_REPLY: u64 = 1 << 62;
/// The last old entry read for migration (name, value), so a large one is fetched from the kernel's vault once.
static mut MIGRATE_CACHE: Option<(Vec<u8>, Vec<u8>)> = None;

/// The vault process this boot (`VAULT_STATE`): none started yet, so the kernel's own vault answers every store call (a boot that carries no vault process, or before it is spawned); started, so every store call for it goes to its endpoint, alive or being started again; gone, it ended before it ever served (its open would end it again), so store calls for it are refused.
/// Before 2026-11-12 a store call found no vault process once it had ended and went to the kernel's vault, which no longer holds what the vault process keeps and said "absent": on the 8a, after the vault process died during a Photon stow, the person's list read "not found", and an absent list is what the identity service's sync reads as the person removing every kist (docs/VAULT-OUT.md "Its end").
pub const VAULT_NONE: u8 = 0;
pub const VAULT_ON: u8 = 1;
pub const VAULT_GONE: u8 = 2;
static VAULT_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(VAULT_NONE);
/// The vault process ended after it had served, or was killed: the service loop starts it again (`vault_wanted`), and the calls queued for it wait meanwhile.
static VAULT_WANTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The vault's state (`VAULT_NONE`, `VAULT_ON`, `VAULT_GONE`).
pub fn vault_state() -> u8 {
    VAULT_STATE.load(core::sync::atomic::Ordering::Acquire)
}

/// Whether the vault process is to be started again (it ended having served, or was killed), cleared as read: the service loop's, once a pass.
pub fn vault_wanted() -> bool {
    VAULT_WANTED.swap(false, core::sync::atomic::Ordering::AcqRel)
}

/// A process holding the right to serve the vault endpoint was built: store calls go to it from now on, and to its successor whenever it ends.
fn vault_spawned(objects: &[(Object, u32)]) {
    let kk = k();
    if objects.iter().any(|&(o, r)| matches!(o, Object::Endpoint(e) if kk.endpoints[e].name == VAULT_EP) && r & RIGHT_RECV != 0) {
        VAULT_STATE.store(VAULT_ON, core::sync::atomic::Ordering::Release);
    }
}

/// A process ended (exit, a fault): if it served the vault, it is started again when it had served (the service loop, woken now), or the vault is gone when it ended before its first receive (an open that ends it ends the next one too: the 8a's 24 MiB heap ran out at every open, 2026-11-12). Under the kernel lock.
fn vault_ended(p: &Process, value: u64) {
    let kk = k();
    let Some(ep) = kk.endpoints.iter().position(|e| e.name == VAULT_EP) else { return };
    if !p.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == ep) && c.rights & RIGHT_RECV != 0) {
        return;
    }
    if kk.endpoints[ep].server == p.epoch {
        // Every end of a vault that served is answered with a restart, so every one counts toward the crash-loop rule (docs/KERNEL.md "Restarts"): looping, the vault is gone as one that will not start is, its waiting calls refused.
        let end = crash_end(value).unwrap_or(ferros_kist::restart::End::Exit);
        if death_counted(VAULT_EP, end) {
            VAULT_STATE.store(VAULT_GONE, core::sync::atomic::Ordering::Release);
            let n = vault_refuse_locked();
            crate::flashlog::line(&alloc::format!("vault process ended (exit G#{value:016X}) after it served, in a crash loop: not started again, A#{n} waiting calls refused"));
            return;
        }
        VAULT_END.store(end as u8, core::sync::atomic::Ordering::Release);
        crate::flashlog::line(&alloc::format!("vault process ended (exit G#{value:016X}) after it served: started again, the store calls waiting for it meanwhile"));
        VAULT_WANTED.store(true, core::sync::atomic::Ordering::Release);
        unsafe { core::arch::asm!("sev") };
    } else {
        crate::flashlog::line(&alloc::format!("vault process ended (exit G#{value:016X}) before it served: not started again, its store calls refused"));
        VAULT_STATE.store(VAULT_GONE, core::sync::atomic::Ordering::Release);
    }
}

/// The vault process could not be started again: gone for the rest of the boot (until one is started by hand), every store call waiting for it answered ERR. How many were. Under the kernel lock.
pub fn vault_refuse_locked() -> usize {
    VAULT_STATE.store(VAULT_GONE, core::sync::atomic::Ordering::Release);
    // The kernel's own call too (a bridge store command), its stamp wiped, as `fail_orphaned_calls` answers it.
    if let Some(ep) = k().endpoints.iter().position(|e| e.name == VAULT_EP) {
        let q = &mut k().endpoints[ep].queue;
        if q.iter().any(|c| c.0 == KERNEL_CALLER) {
            for c in q.iter_mut().filter(|c| c.0 == KERNEL_CALLER) {
                crate::keys::wipe_to_dram(&mut c.1);
            }
            q.retain(|c| c.0 != KERNEL_CALLER);
            kernel_answered(Err(()));
        }
    }
    on_demand_refuse_locked(VAULT_EP)
}

/// The endpoint a store call for the vault process goes to (`store_route`, `export_route`): the vault endpoint while a vault process is alive or being started again (`VAULT_ON`), where the call waits for it; None before any was started (the kernel's own vault answers then). A vault that is gone has the call refused (`Err`).
fn vault_route() -> Option<Result<usize, ()>> {
    match vault_state() {
        VAULT_NONE => vault_ep().map(Ok),
        VAULT_ON => k().endpoints.iter().position(|e| e.name == VAULT_EP).map(Ok),
        _ => Some(Err(())),
    }
}

/// The vault endpoint, while a live process holds the right to serve it: from its spawn, not its first receive, so a call made while it opens waits for it (the GPU asked for `gpu.fw` 145 ms into a boot, before the vault served, and the kernel's vault, which no longer held it, said absent, 2026-11-07). Should it die first, the waiting calls get ERR (`fail_orphaned_calls`).
fn vault_ep() -> Option<usize> {
    let kk = k();
    let ep = kk.endpoints.iter().position(|e| e.name == VAULT_EP)?;
    let serving = kk.procs.iter().flatten().any(|p| !p.dying && p.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == ep) && c.rights & RIGHT_RECV != 0));
    serving.then_some(ep)
}

/// The largest get the vault process serving now has said it answers (`SYS_VAULT` op 3: its epoch, the length), else STORE_MAX: a vault process from before 2026-11-11 has a reply buffer of one STORE_MAX transfer, and a larger get would index past it. Under the kernel lock.
static mut VAULT_GET: (u64, usize) = (u64::MAX, STORE_MAX);

/// The get bound for the kernel's own call (`vault_call`), as for a process's forwarded get: a kist's capsule is read a megabyte a call where the vault process answers that much (2026-11-17: Photon's 92 MB went in 351 calls of 256 KiB, each batch of four pieces waiting on its slowest helper). Takes the kernel lock.
pub fn kernel_get_max() -> usize {
    with_lock(|| vault_ep().map(vault_get_max).unwrap_or(STORE_MAX))
}

/// The get bound for the vault process serving `ep` (`VAULT_GET` when that process said it, STORE_MAX otherwise).
fn vault_get_max(ep: usize) -> usize {
    let kk = k();
    let (epoch, max) = unsafe { VAULT_GET };
    let said = kk.procs.iter().flatten().any(|p| !p.dying && p.epoch == epoch && p.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == ep) && c.rights & RIGHT_RECV != 0));
    if said { max } else { STORE_MAX }
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
    // The person's: from their domain key held under the wairua, derived in registers (KEY_REGISTERS.md "Sealed under the wairua at rest"); the same bytes as the software derivation below.
    if identity {
        return crate::service::identity_domain_keys(prefix, DOMAIN_ADDR_CONTEXT, DOMAIN_SEAL_CONTEXT);
    }
    let mut base = unsafe { DOMAIN_BASE }?(identity)?;
    let mut out = [0u8; 64];
    for (i, ctx) in [DOMAIN_ADDR_CONTEXT, DOMAIN_SEAL_CONTEXT].iter().enumerate() {
        let mut h = blake3::Hasher::new_derive_key(ctx);
        h.update(&base);
        h.update(prefix);
        out[i << 5..(i + 1) << 5].copy_from_slice(h.finalize().as_bytes());
        // The hasher's buffer holds the base.
        crate::keys::wipe_value_to_dram(&mut h);
    }
    crate::keys::wipe_to_dram(&mut base);
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
    // A call on the GPU service goes in behind the kernel's stamp of who calls and what GPU memory it holds (`gpu_stamp`): the service maps the job's address space from it alone, never from the caller's word.
    let msg = if kk.endpoints[ep].name == b"gpu" {
        GPU_CALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        gpu_stamp(p, msg)
    } else {
        msg
    };
    // An injected batch (the touch process's op 3, development builds only: docs/INPUT-INJECT.md) is input as a finger's interrupt is, for the adaptive clock and the touch-to-frame measure.
    if msg.first() == Some(&3) && kk.endpoints[ep].name == b"touch" {
        note_input();
    }
    // A call on the identity service's facets goes in behind the kernel's stamp of the kist that calls (`facets_stamp`).
    let msg = if kk.endpoints[ep].name == b"facets" { facets_stamp(p, msg) } else { msg };
    // A call on the pucks' endpoint goes in behind the caller's kist identity (`puck_stamp`): the display keys a puck's circle by it, never by anything the puck says.
    let msg = if kk.endpoints[ep].name == b"puck" { puck_stamp(p, msg) } else { msg };
    kk.endpoints[ep].queue.push_back((cur, msg));
    wake_server(ep, true);
    if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
}

/// `SYS_VAULT` op 0: the `skip`-th entry of the kernel's vault that belongs in the vault process (a scope's, not one the kernel keeps, its class open), with its domain's keys: the one exception to keys only against a live call (VAULT-OUT.md, migration), once per old entry. None left: the pending mark is cleared.
/// `names` the kernel's vault's, listed with the kernel lock let go (SYS_VAULT); the keys derived and written under it.
fn migrate_next(p: &Process, names: Vec<alloc::string::String>, skip: usize, buf: u64, len: usize) -> u64 {
    let mut seen = 0;
    for name in names {
        let Some(sc) = scope_of(&name) else { continue };
        let identity = crate::kvault::identity_class(&name);
        let Some(mut keys) = domain_keys(sc.prefix.as_bytes(), identity) else { continue };
        if seen < skip {
            seen += 1;
            crate::keys::wipe_to_dram(&mut keys);
            continue;
        }
        let mut out = Vec::with_capacity(66 + name.len());
        out.push(identity as u8);
        out.extend_from_slice(&keys);
        crate::keys::wipe_to_dram(&mut keys);
        out.push(sc.prefix.len() as u8);
        out.extend_from_slice(name.as_bytes());
        let n = out.len();
        if n <= len {
            p.write(buf, &out);
        }
        crate::keys::wipe_to_dram(&mut out);
        return if n <= len { n as u64 } else { ERR };
    }
    MIGRATE_PENDING.store(false, core::sync::atomic::Ordering::Release);
    NOTHING
}

/// `SYS_VAULT` op 1: an old entry's value from `offset` (fetched once, kept until another is asked for or it moves). `got` the vault's answer when the cache did not hold it, read with the kernel lock let go (SYS_VAULT).
fn migrate_read(p: &Process, name: &str, offset: usize, buf: u64, len: usize, got: Option<Result<Option<Vec<u8>>, ()>>) -> u64 {
    if scope_of(name).is_none() {
        return ERR;
    }
    let cache = unsafe { &mut *(&raw mut MIGRATE_CACHE) };
    if let Some(got) = got {
        *cache = match got {
            Ok(Some(v)) => Some((name.as_bytes().to_vec(), v)),
            Ok(None) => return NOTHING,
            Err(()) => return ERR,
        };
    }
    // The cache this name's or nothing: no vault to read from (`STORE` unset) leaves it as it was.
    let Some((_, v)) = cache.as_ref().filter(|(k, _)| k == name.as_bytes()) else { return ERR };
    // WHY: the offset is the vault process's and may lie past the end (nothing to copy then).
    let from = offset.min(v.len());
    let to = (offset + len).min(v.len());
    p.write(buf, &v[from..to]);
    v.len() as u64
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

/// Whether a `SYS_STORE` names the Stow right: the stower's call on a kist's own domain (`kist_store_route`).
fn names_stow(p: &Process, f: &Frame) -> bool {
    let token = p.read(f.x[0], 32);
    matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Stow, r)) if r & RIGHT_SEND != 0)
}

/// The stower's `SYS_STORE` on a kist's own domain (docs/KISTS.md "The unstow sheet"): the kist named by its identity (x2 [developer 32][option 32], x3 64), its domain the one a process of that kist gets at spawn (the oracle over the two, then `KIST_DOMAIN_CONTEXT`), so nothing but the kernel computes it and the stower never holds its keys. The vault process's endpoint and the domain; Err with no vault process (the kernel's own vault holds no kist domain), no system secret, or an identity of another length.
fn kist_store_route(p: &Process, f: &Frame) -> Result<(usize, [u8; 32]), ()> {
    let ep = vault_route().ok_or(())??;
    // WHY: the length is the caller's; an identity is exactly two keys.
    if f.x[3] != 64 {
        return Err(());
    }
    let id = p.read(f.x[2], 64);
    let developer: [u8; 32] = id[..32].try_into().unwrap();
    let ihi = crate::oracle::ihi(&developer, &id[32..]).ok_or(())?;
    Ok((ep, blake3::derive_key(KIST_DOMAIN_CONTEXT, &ihi)))
}

/// Where a `SYS_STORE` goes: to the vault process (its endpoint, and the caller's scope or its own domain) while one is started this boot (`vault_route`: alive, or being started again, the call waiting for it) and the name is not one the kernel keeps; Err, refused (the vault process is gone); None, the kernel's own vault (no vault process started yet; it holds no kist domain: a domain call then is refused there).
fn store_route(p: &Process, f: &Frame) -> Option<Result<(usize, Where), ()>> {
    let route = vault_route()?;
    let token = p.read(f.x[0], 32);
    let sc = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
        Some((Object::Store(i), r)) if r & RIGHT_SEND != 0 => STORES.get(i)?,
        Some((Object::Domain, r)) if r & RIGHT_SEND != 0 => return p.domain.map(|d| route.map(|ep| (ep, Where::Domain(d)))),
        _ => return None,
    };
    if f.x[1] & 32 == 0 {
        let mut full = alloc::string::String::from(sc.prefix);
        full.extend(p.read(f.x[2], (f.x[3] as usize).min(97)).iter().map(|&b| b as char));
        if KERNEL_HELD.iter().any(|h| full.starts_with(h)) {
            return None;
        }
    }
    Some(route.map(|ep| (ep, Where::Scope(sc))))
}

/// `SYS_STORE` while the vault process serves (VAULT-OUT.md, step 3): the request, stamped with the domain keys of the caller's scope, goes to the vault as a call from the caller, who waits for the reply as for any call. The kernel copies one transfer each way and keeps nothing; keys for the person's domain only while they are unlocked.
///
/// The stamped message: [op: 0 set, 1 get, 2 delete, 3 list][flags: bit 0 old entries pending][lock epoch u32 LE][domains][per domain: class (1 the person's) and the address and sealing keys, 65 bytes][name length][name: the name within the scope, or 1 and the 32-byte address][rest]. Rest: set [offset u64][more u8][bytes]; get [offset u64][length u32]; list [offset u64][length u32][addresses u8]; delete nothing.
fn store_forward(f: &mut Frame, cur: usize, ep: usize, wh: Where) -> u64 {
    let Some(p) = proc_of(cur) else {
        f.x[0] = ERR;
        return 0;
    };
    if f.x[1] & STORE_EXTENT != 0 {
        return extent_forward(f, cur, ep, wh);
    }
    let (set, more, del, list, raw) = (f.x[1] & 1 != 0, f.x[1] & 4 != 0, f.x[1] & 8 != 0, f.x[1] & 16 != 0, f.x[1] & 32 != 0);
    let offset = if f.x[1] & 2 != 0 { f.x[1] >> 32 } else { 0 };
    let len = ((f.x[1] >> 8) & 0xFF_FFFF) as usize;
    let rel = p.read(f.x[2], (f.x[3] as usize).min(97));
    // WHY: the name and the length are the caller's; the same refusals as the kernel's own path (a name of printable ASCII up to 96 bytes or a 32-byte address; one transfer at most: a get's own, larger, STORE_GET_MAX).
    let rel_ok = if raw { rel.len() == 32 || list } else { rel.len() <= 96 && rel.iter().all(|&b| (0x21..0x7F).contains(&b)) && (!rel.is_empty() || list) };
    let most = if set || del || list { STORE_MAX } else { vault_get_max(ep) };
    if !rel_ok || len > most {
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
    if let Some(t) = th(cur) {
        t.called = crate::platform::cntpct();
        t.what = op as u64 | (more as u64) << 7 | (len as u64) << 8 | offset << 32;
    }
    call_endpoint(f, cur, ep, msg, f.x[4], rcap | STORE_REPLY)
}

/// `SYS_STORE` x1 bit 6: a single-copy extent of the vault process (docs/RAW-VIDEO.md "The recorder"): bulk data such as a RAW video clip, written once at the flash's rate into the `clips` window, no mirror, its record (where, how long, its BLAKE3) kept by the vault under the caller's name in the caller's domain.
pub const STORE_EXTENT: u64 = 1 << 6;
/// The largest transfer of an extent's write or read: one UFS command (BLOCK_MAX blocks, 1 MiB).
pub const EXTENT_MAX: usize = BLOCK_MAX as usize * 4096;

/// An extent call (x1 bit 6), forwarded as `VOP_EXTENT`: x1 bits 0-2 the extent op (`EOP_*`), bit 5 the name a 32-byte address, bits 8-31 the length of the caller's buffer at x4, bits 32-63 a block within the extent; x2/x3 the name, as every store call.
/// Rest: [extent op u8][block u64][length u32][the caller's bytes: open its blocks u64, write the blocks' bytes, seal its length u64 and BLAKE3; nothing for read, info and delete].
fn extent_forward(f: &mut Frame, cur: usize, ep: usize, wh: Where) -> u64 {
    let Some(p) = proc_of(cur) else {
        f.x[0] = ERR;
        return 0;
    };
    // A refusal's reason on the console, written as `push_console` writes it but without taking the kernel lock, which this syscall holds and which is not reentrant.
    // 63916ff9 called `push_console` here: a refused call spun on its own core's lock with interrupts masked until the watchdog reset the Pro (Lumis's seals from an A510, which held no keys, 2026-11-19).
    let say = |why: &str| {
        let line = alloc::format!("extent: {} refused: {why}\n", core::str::from_utf8(&p.name).unwrap_or("?"));
        k().console.extend_from_slice(line.as_bytes());
        crate::platform::pstore(line.as_bytes());
    };
    let (eop, raw) = ((f.x[1] & 7) as u8, f.x[1] & 32 != 0);
    let block = f.x[1] >> 32;
    let len = ((f.x[1] >> 8) & 0xFF_FFFF) as usize;
    let rel = p.read(f.x[2], (f.x[3] as usize).min(97));
    // WHY: the op, the name and the length are the caller's: a name as a value's (printable ASCII up to 96 bytes, or a 32-byte address); a write or read whole blocks of at most one UFS command; an open its block count, a seal its length and hash, an info its record's buffer.
    let rel_ok = if raw { rel.len() == 32 } else { !rel.is_empty() && rel.len() <= 96 && rel.iter().all(|&b| (0x21..0x7F).contains(&b)) };
    let len_ok = match eop {
        EOP_OPEN => len == 32,
        EOP_WRITE | EOP_READ => len != 0 && len & 4095 == 0 && len <= EXTENT_MAX && (eop == EOP_WRITE || len <= vault_get_max(ep)),
        EOP_SEAL => len == 40,
        EOP_INFO => len == EXTENT_INFO,
        EOP_DELETE => len == 0,
        EOP_SPACE => len == EXTENT_SPACE,
        _ => false,
    };
    if !rel_ok || !len_ok {
        say(if !rel_ok { "the name is not printable ASCII (G#21 to G#7E, no spaces) of 1 to 96 bytes" } else { "the buffer's length is not the op's" });
        f.x[0] = ERR;
        return 0;
    }
    let mut name = Vec::with_capacity(33);
    if raw {
        name.push(1);
    }
    name.extend_from_slice(&rel);
    let (prefix, classes): (Vec<u8>, &[bool]) = match wh {
        Where::Scope(sc) => {
            let mut full = alloc::string::String::from(sc.prefix);
            if !raw {
                full.extend(rel.iter().map(|&b| b as char));
            }
            (sc.prefix.as_bytes().to_vec(), if crate::kvault::identity_class(&full) { &[true] } else { &[false] })
        }
        Where::Domain(d) => ([KIST_DOMAIN_PREFIX, &d[..]].concat(), &[true]),
    };
    let carried = matches!(eop, EOP_OPEN | EOP_WRITE | EOP_SEAL);
    let mut rest = [0u8; 13];
    rest[0] = eop;
    rest[1..9].copy_from_slice(&block.to_le_bytes());
    rest[9..13].copy_from_slice(&(len as u32).to_le_bytes());
    let Some(mut msg) = stamp(VOP_EXTENT, &prefix, classes, &name, &rest) else {
        say("its domain's keys are not open (the person's class while they are away, or this core holds no copy of the keys yet)");
        f.x[0] = ERR;
        return 0;
    };
    // The caller's bytes copied once, from where they lie straight onto the stamp (a MiB a write: the zeroed buffer and the two copies before cost about a millisecond of the clip's write path, measured 2026-11-19).
    if carried {
        match p.view(f.x[4], len) {
            Some(b) => {
                msg.reserve_exact(len);
                msg.extend_from_slice(b);
            }
            None => {
                say(&alloc::format!("its buffer of A#{} bytes at G#{:X} lies in no memory of its own", len, f.x[4]));
                crate::keys::wipe_to_dram(&mut msg);
                f.x[0] = ERR;
                return 0;
            }
        }
    }
    let rcap = if matches!(eop, EOP_READ | EOP_INFO | EOP_OPEN | EOP_SPACE) { len as u64 } else { 0 };
    if let Some(t) = th(cur) {
        t.called = crate::platform::cntpct();
        t.what = VOP_EXTENT as u64 | (len as u64) << 8 | block << 32;
    }
    call_endpoint(f, cur, ep, msg, f.x[4], rcap | STORE_REPLY)
}

/// `SYS_STORE` x1 bit 7: a call on the hold (docs/STORE.md "The calls", ferros_items `wire`): x0 the `Object::Items` token, x2/x3 the call (its op, handle, offset, length and payload), x4 the reply's buffer, x1 bits 8-31 its length.
pub const STORE_ITEMS: u64 = 1 << 7;
/// The hold's calls forwarded to the vault process (`items_forward`): the person's items domain stamped (`ITEMS_PREFIX`, identity class), rest [the caller's stamp][the call].
pub const VOP_ITEMS: u8 = 8;
/// The person's items domain: one domain under their identity class, every kist's items in it, each kept by the card's maker.
pub const ITEMS_PREFIX: &[u8] = b"items.";
/// The longest call: an extent's or a body's write of one UFS command, and its head.
pub const ITEMS_CALL_MAX: usize = EXTENT_MAX + (1 << 6);

/// Who calls on the hold, as the kernel knows it, ahead of its call (payloads/vault `items`): [flags u8: bit 0 a kist, bit 1 any kind, bit 2 the person (the kernel's own call, `items_person_stamp`)][developer 32][option 32][name length u8][name][makes u16 LE][the kinds, 32 each][opens u16 LE][the kinds]. A kist is its gate identity; a development spawn its name under a zero developer key (it has no identity of its own).
/// WHY the bounds: the name and the kinds are the process's own and its capsule's; a name past a byte's count is cut, never wrapped; a list past `ITEMS_KINDS_MOST` refuses the call (None) rather than cut a right silently, since the stamp and a transfer must fit the vault's room for one message (payloads/vault: a transfer and 64 KiB).
fn items_stamp(p: &Process) -> Option<Vec<u8>> {
    let r = p.items.as_ref()?;
    if r.makes.len() > ITEMS_KINDS_MOST || r.opens.len() > ITEMS_KINDS_MOST {
        return None;
    }
    let (developer, option) = match p.gate.filter(|_| p.kist) {
        Some(g) => g,
        None => ([0; 32], blake3::derive_key("ferros items 2026-11-19 development spawn", &p.name)),
    };
    let name = &p.name[..p.name.len().min(255)];
    let mut out = Vec::with_capacity(70 + name.len() + 32 * (r.makes.len() + r.opens.len()));
    out.push(p.kist as u8 | (r.any as u8) << 1);
    out.extend_from_slice(&developer);
    out.extend_from_slice(&option);
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    for list in [&r.makes, &r.opens] {
        out.extend_from_slice(&(list.len() as u16).to_le_bytes());
        for k in list.iter() {
            out.extend_from_slice(k);
        }
    }
    Some(out)
}

/// The most kinds a kist's `items` right names in either list (makes, opens): 2^8 of each is 16 KiB of stamp, inside the vault's room beside a transfer.
pub const ITEMS_KINDS_MOST: usize = 1 << 8;

/// The person's own call on the hold (the kernel's channel: the bridge's `items`, the bench and the owner's pulls): every kind, no kist.
pub fn items_person_stamp() -> Vec<u8> {
    let mut out = alloc::vec![1u8 << 2];
    out.extend_from_slice(&[0; 64]);
    out.extend_from_slice(&[0, 0, 0, 0, 0]);
    out
}

/// The kernel's own call on the hold, stamped for the person: the person's items domain's keys, then `rest` (the person's stamp and the call). None while nobody is present.
pub fn vault_stamp_items(call: &[u8]) -> Option<Vec<u8>> {
    let mut rest = items_person_stamp();
    rest.extend_from_slice(call);
    let doms = items_domains(call)?;
    with_lock_held_or(|| stamp_domains(VOP_ITEMS, &doms.iter().map(|p| (&p[..], true)).collect::<Vec<_>>(), b"", &rest))
}

/// ferros_items `wire::OP_USAGE`: what is on the device, read only.
pub const ITEMS_OP_USAGE: u8 = 16;
/// The most kists one usage call names (ferros_items `wire::USAGE_KISTS`): each one's domain keys are derived in the call, under the kernel lock.
pub const ITEMS_USAGE_KISTS: usize = 1 << 6;
/// The person's list of stowed kists' scope, read by the vault for a usage call's names.
const KISTS_PREFIX: &[u8] = b"kists.";

/// The domains a call on the hold is stamped with, all the person's: their items domain; for a usage call also their kists' list (`kists.`) and the own domain of each kist the call names ([developer 32][option 32] each, as the stower names one: `kist_store_route`). None for a usage call naming more than `ITEMS_USAGE_KISTS` or not whole identities, or while no oracle secret is open.
/// A kist's domain sized this way says how much a stowed kist keeps for itself, never what: the vault answers its values' count and bytes from their heads alone.
fn items_domains(call: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut v = alloc::vec![ITEMS_PREFIX.to_vec()];
    if call.first() == Some(&ITEMS_OP_USAGE) {
        // WHY: the payload is the caller's: whole identities, a bounded count of them (each costs a derivation here).
        let ids = call.get(45..)?;
        if ids.len() % 64 != 0 || ids.len() / 64 > ITEMS_USAGE_KISTS {
            return None;
        }
        v.push(KISTS_PREFIX.to_vec());
        for id in ids.chunks_exact(64) {
            let developer: [u8; 32] = id[..32].try_into().unwrap();
            let ihi = crate::oracle::ihi(&developer, &id[32..])?;
            v.push([KIST_DOMAIN_PREFIX, &blake3::derive_key(KIST_DOMAIN_CONTEXT, &ihi)[..]].concat());
        }
    }
    Some(v)
}

/// A call on the hold (`STORE_ITEMS`), forwarded as `VOP_ITEMS`: the person's items domain's keys, the caller's stamp, then its call. Refused without the right, while nobody is present (the person's keys are not open), or for a call or reply past its bounds.
fn items_forward(f: &mut Frame, cur: usize, ep: usize) -> u64 {
    let Some(p) = proc_of(cur) else {
        f.x[0] = ERR;
        return 0;
    };
    let token = p.read(f.x[0], 32);
    let held = p.lookup(&token).is_some_and(|i| p.cspace[i].object == Object::Items && p.cspace[i].rights & RIGHT_SEND != 0);
    let (len, call) = (((f.x[1] >> 8) & 0xFF_FFFF) as usize, f.x[3] as usize);
    // WHY: the lengths are the caller's: a call of at least its head and at most one transfer, a reply no larger than the vault answers.
    let Some(stamp) = items_stamp(p).filter(|_| held && call >= 45 && call <= ITEMS_CALL_MAX && len <= vault_get_max(ep)) else {
        f.x[0] = ERR;
        return 0;
    };
    let Some(b) = p.view(f.x[2], call) else {
        f.x[0] = ERR;
        return 0;
    };
    let Some(doms) = items_domains(b) else {
        f.x[0] = ERR;
        return 0;
    };
    let Some(mut msg) = stamp_domains(VOP_ITEMS, &doms.iter().map(|p| (&p[..], true)).collect::<Vec<_>>(), b"", &stamp) else {
        f.x[0] = ERR;
        return 0;
    };
    msg.reserve_exact(call);
    msg.extend_from_slice(b);
    if let Some(t) = th(cur) {
        t.called = crate::platform::cntpct();
        t.what = VOP_ITEMS as u64 | (len as u64) << 8;
    }
    call_endpoint(f, cur, ep, msg, f.x[4], len as u64 | STORE_REPLY)
}

/// The extent ops (x1 bits 0-2 with `STORE_EXTENT`; payloads/vault `extent`).
/// Open: reserve the u64 of blocks at the start of x4's 32 bytes under the name (a fresh extent; an open one of the same name is dropped first) → the blocks reserved, ERR when no stretch that long is free; the 32 bytes then hold the extent's own SYS_BLOCK capability (`Object::Extent`), or zeros from a vault process that gives none.
pub const EOP_OPEN: u8 = 0;
/// Write the buffer's whole blocks at the block in x1's top half, inside the extent, once, not read back → 0.
pub const EOP_WRITE: u8 = 1;
/// Seal: the extent's length in bytes (u64 LE) and its BLAKE3 (32 bytes) recorded; its blocks past the length freed → 0.
pub const EOP_SEAL: u8 = 2;
/// Read the buffer's length of whole blocks from the block in x1's top half → the bytes read.
pub const EOP_READ: u8 = 3;
/// Its record into the buffer (`EXTENT_INFO` bytes: blocks u64, bytes u64, sealed u8, BLAKE3 32, recorded at Eagle i64, the vault's first block u64) → the blocks, NOTHING when the name has none.
pub const EOP_INFO: u8 = 4;
/// Drop it → 0, NOTHING when the name has none.
pub const EOP_DELETE: u8 = 5;
pub const EXTENT_INFO: usize = 8 + 8 + 1 + 32 + 8 + 8;
/// The recording space, read only (the name any of the caller's, it is not looked up) → the clips area's free blocks; the buffer (`EXTENT_SPACE` bytes, u64 LE each): the area's blocks, the blocks every extent takes, the longest free stretch, the blocks the caller's own domain's extents take, how many they are.
pub const EOP_SPACE: u8 = 6;
/// The kernel's own channel only, for the bridge (service.rs `store_job` G#FB and G#FA; refused from a process here, as every op past EOP_SPACE is): the clips area's records (the caller's domain's owner hash, then first, blocks, bytes, sealed, at, owner a record), and a drop of the caller's domain's extent at a first block (its name a derived key that may be lost).
pub const EOP_LIST: u8 = 7;
pub const EOP_DROP_AT: u8 = 8;
/// The kernel's own channel only (service.rs `store_job` G#F8, development builds): a sealed extent dropped by its first block whatever domain owns it, for one whose owning build no longer exists; refused from a process as every op past EOP_SPACE is.
pub const EOP_DROP_ORPHAN: u8 = 9;
pub const EXTENT_SPACE: usize = 5 * 8;

/// The stower's call on a kist's own domain (`kist_store_route`; docs/KISTS.md "The unstow sheet"): x1 bit 3 drops it whole (`VOP_DROP`, the value how many values went: the person cleared what it kept for itself), x1 bit 4 sizes it (`VOP_SIZE`, the value its bytes, and its count of values written at x4, 8 bytes). The person's class always (a kist's data is theirs), so only while they are present; anything else is refused.
fn kist_domain_forward(f: &mut Frame, cur: usize, ep: usize, d: &[u8; 32]) -> u64 {
    let op = match (f.x[1] & 8 != 0, f.x[1] & 16 != 0) {
        (true, false) => VOP_DROP,
        (false, true) => VOP_SIZE,
        _ => {
            f.x[0] = ERR;
            return 0;
        }
    };
    let Some(msg) = stamp(op, &[KIST_DOMAIN_PREFIX, &d[..]].concat(), &[true], &[], &[]) else {
        f.x[0] = ERR;
        return 0;
    };
    if let Some(t) = th(cur) {
        t.called = crate::platform::cntpct();
        t.what = op as u64;
    }
    let rcap = if op == VOP_SIZE { 8 } else { 0 };
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
/// How many values each stamped domain holds and their bytes in all (a kist's own domain, sized for its unstow sheet: `kist_domain_forward`): name and rest empty. The value: the bytes, or ERR; the reply's bytes: the count of values, u64 LE.
pub const VOP_SIZE: u8 = 6;
/// A single-copy extent (`extent_forward`): name the caller's, rest [extent op][block u64][length u32][bytes]. The value and reply by op (`EOP_*`).
pub const VOP_EXTENT: u8 = 7;

/// `SYS_UNLOCK` op 5 while the vault process serves, for a caller holding the Unlock right: its endpoint.
fn export_route(p: &Process, f: &Frame) -> Option<Result<usize, ()>> {
    let route = vault_route()?;
    let token = p.read(f.x[0], 32);
    matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Unlock, r)) if r & RIGHT_SEND != 0).then_some(route)
}

/// `SYS_UNLOCK` ops 6-9, attest with a tag (AUTH.md "Attest with a tag"):
/// - 6 unlock by tag (x2 the UID, x3 its length, x4 32 bytes the seed lands in), the Unlock right only → the entry count as op 1, NOTHING (the device is bound to another identity), or ERR: one refusal for a tag kept for nothing here and for one whose record does not open, no detail;
/// - 7 keep a tag for the unlocked identity (x2, x3 the UID) → 0 kept, 1 kept already, ERR (nobody present, or it could not be kept);
/// - 8 forget (x2 the first `TAG_SHOWN` bytes of a tag's id as op 9 gives them, x3 that length; x3 0: every tag) → how many went;
/// - 9 list (x2 a buffer, x3 its length) → how many tags are kept, as many `TAG_ENTRY` entries written as the buffer holds whole.
/// Ops 7-9 are also a Settings holder's (the settings page keeps tags and holds no right to unlock); none acts while nobody is present.
fn tag_syscall(p: &Process, f: &Frame) -> u64 {
    let token = p.read(f.x[0], 32);
    let op = f.x[1];
    let right = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
        Some((Object::Unlock, r)) => r & RIGHT_SEND != 0,
        Some((Object::Settings, r)) => r & RIGHT_SEND != 0 && op != 6,
        _ => false,
    };
    let Some(tag) = (unsafe { TAG }) else { return ERR };
    if !right {
        return ERR;
    }
    // WHY the lengths: the UID and its length are the caller's (and, before it, the radio's); an NFC-A UID is 4, 7 or 10 bytes (ISO/IEC 14443-3) and anything else is not one, refused before anything is read.
    let uid_len = |n: u64| matches!(n, 4 | 7 | 10);
    match op {
        6 if uid_len(f.x[3]) => {
            let mut uid = p.read(f.x[2], f.x[3] as usize);
            let mut seed = [0u8; 32];
            let r = tag(TagOp::Unlock(&uid, &mut seed));
            if r.is_ok_and(|n| n != NOTHING) {
                p.write(f.x[4], &seed);
            }
            crate::keys::wipe_to_dram(&mut seed);
            crate::keys::wipe_to_dram(&mut uid);
            r.unwrap_or(ERR)
        }
        7 if uid_len(f.x[3]) => {
            let mut uid = p.read(f.x[2], f.x[3] as usize);
            let r = tag(TagOp::Keep(&uid));
            crate::keys::wipe_to_dram(&mut uid);
            r.unwrap_or(ERR)
        }
        8 if f.x[3] == 0 => tag(TagOp::Forget(None)).unwrap_or(ERR),
        8 if f.x[3] == crate::kvault::TAG_SHOWN as u64 => {
            let mut which = [0u8; crate::kvault::TAG_SHOWN];
            which.copy_from_slice(&p.read(f.x[2], crate::kvault::TAG_SHOWN));
            tag(TagOp::Forget(Some(which))).unwrap_or(ERR)
        }
        9 => {
            let mut list = Vec::new();
            match tag(TagOp::List(&mut list)) {
                Ok(n) => {
                    // WHY: the buffer's length is the caller's; only whole entries that fit go back.
                    let fit = (f.x[3] as usize / TAG_ENTRY * TAG_ENTRY).min(list.len());
                    p.write(f.x[2], &list[..fit]);
                    n
                }
                Err(()) => ERR,
            }
        }
        _ => ERR,
    }
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
    if let Some(t) = th(cur) {
        t.called = crate::platform::cntpct();
        t.what = VOP_EXPORT as u64 | (len as u64) << 8;
    }
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
            crate::keys::wipe_to_dram(&mut msg);
            return None;
        };
        msg.push(identity as u8);
        msg.extend_from_slice(&keys);
        crate::keys::wipe_to_dram(&mut keys);
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
            crate::keys::wipe_to_dram(&mut msg);
            return false;
        };
        *c = (true, None);
        tr(TR_CALL, KERNEL_CALLER, ep as u64);
        k().endpoints[ep].queue.push_back((KERNEL_CALLER, msg));
        wake_server(ep, false);
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

/// A name under no scope this kernel knows but shaped as a scope's name (a label of lowercase letters and digits, a dot, then a name of printable ASCII up to 96 bytes): its prefix (the label and the dot) and the name within it.
/// Such a name may be held by the vault process under a scope another kernel knows, a newer one or an older one: a domain's keys come from its prefix alone (`domain_keys`), so this kernel reaches it as that kernel did.
/// Why (2026-11-15, the Pro): the camera kernel, RAM-reloaded on b873, knew the `cam` scope and moved `cam.frame` and `cam.boitata.pdma` from the kernel's vault into the vault process; the flashed kernel, from before that scope, looked only in its own vault after the cold boot of b875 and said absent, and the entries were stored again by hand.
/// Only the bridge's gets and removes reach these (service.rs `foreign_job`), after the kernel's own vault said absent; a put still lands in the kernel's vault, which a kernel that knows the scope moves later.
pub fn foreign_name(full: &str) -> Option<(&str, &[u8])> {
    if scope_of(full).is_some() || KERNEL_HELD.iter().any(|h| full.starts_with(h)) {
        return None;
    }
    let dot = full.find('.')?;
    let (label, rel) = (&full.as_bytes()[..dot], &full.as_bytes()[dot + 1..]);
    let label_ok = !label.is_empty() && label.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (label_ok && !rel.is_empty() && rel.len() <= 96 && rel.iter().all(|&b| (0x21..0x7F).contains(&b))).then_some((&full[..=dot], rel))
}

/// A stamp for the kernel's own get, set or delete of the full name `full` (`vault_name`, or `foreign_name` for a get or a delete): its scope's domain in the name's class. None when the vault does not take the name, or the class is locked.
pub fn vault_stamp(op: u8, full: &str, rest: &[u8]) -> Option<Vec<u8>> {
    let (prefix, rel) = match vault_name(full) {
        Some((sc, rel)) => (sc.prefix, rel),
        None if op != VOP_SET => foreign_name(full)?,
        None => return None,
    };
    // Under the kernel lock: the person's keys come from the vault's unlock state, which an unlock or a lock on another core changes.
    with_lock_held_or(|| stamp(op, prefix.as_bytes(), &[crate::kvault::identity_class(full)], rel, rest))
}

/// A stamp for the kernel's own call on an entry of the scope named `scope` that is named by a 32-byte address (a stowed kist's capsule, by its blake3): the scope's domain in its class. None for no such scope, or a locked class.
pub fn vault_stamp_addr(op: u8, scope: &str, addr: &[u8; 32], rest: &[u8]) -> Option<Vec<u8>> {
    let sc = STORES.iter().find(|s| s.name == scope)?;
    let mut name = Vec::with_capacity(33);
    name.push(1);
    name.extend_from_slice(addr);
    with_lock_held_or(|| stamp(op, sc.prefix.as_bytes(), &[crate::kvault::identity_class(sc.prefix)], &name, rest))
}

/// A stamp for the kernel's own call on a name in a kist's own domain (the bridge's `store extent --kist`: a RAW clip Lumis recorded, pulled to the host; development builds only), the kist named by its identity as the stower names one (`kist_store_route`): the person's class always, so only while they are present. None with no system secret or a locked class.
pub fn vault_stamp_kist(op: u8, developer: &[u8; 32], option: &[u8; 32], name: &[u8], rest: &[u8]) -> Option<Vec<u8>> {
    let ihi = crate::oracle::ihi(developer, option)?;
    let d = blake3::derive_key(KIST_DOMAIN_CONTEXT, &ihi);
    with_lock_held_or(|| stamp(op, &[KIST_DOMAIN_PREFIX, &d[..]].concat(), &[true], name, rest))
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

/// Each kist's own domain (`KIST_DOMAIN_CONTEXT` over its ihi, as a process of it gets at spawn) for `ids` ([developer 32][option 32] each), each once: a kist named twice (stowed, and kept with answers after an unstow) is one domain. None for any when no system secret is open.
fn kist_domains(ids: &[u8]) -> Vec<[u8; 32]> {
    let mut out: Vec<[u8; 32]> = Vec::new();
    for id in ids.chunks_exact(64) {
        let developer: [u8; 32] = id[..32].try_into().unwrap();
        let Some(ihi) = crate::oracle::ihi(&developer, &id[32..]) else { continue };
        let d = blake3::derive_key(KIST_DOMAIN_CONTEXT, &ihi);
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// A removal's dry run as lines of text (SYS_UNLOCK op 15's dry bit, `diag "identity remove"`; AUTH.md "Remove this device"): the kernel's half (service.rs `remove_preview_text`), the person's domain in each scope the release drops in the vault process, and the kists' own domains from `ids` (as the identity service read them from the person's list). Nothing changed. Under the kernel lock.
pub fn remove_dry_text(ids: &[u8]) -> alloc::string::String {
    let mut t = alloc::string::String::from("remove this device (dry run): nothing erased\n");
    match crate::service::remove_preview_text() {
        Some(k) => t.push_str(&k),
        None => t.push_str("the kernel's vault: not open\n"),
    }
    t.push_str("vault process, the person's domain dropped in each scope:");
    for sc in STORES.iter().filter(|sc| crate::kvault::identity_within(sc.prefix)) {
        t.push(' ');
        t.push_str(sc.prefix);
    }
    t.push_str(" and the hold (the person's items and their clips)\n");
    t.push_str(&alloc::format!("vault process, the kists' own domains of the person's dropped: A#{} (from A#{} kists named)\n", kist_domains(ids).len(), ids.len() >> 6));
    t.push_str("not touched: the device class (drivers, firmware, the stowed capsules, the device's settings), any other identity, the factory data in devinfo\nthen: the power off\n");
    t
}

/// Stamps dropping every one of the person's domains in the vault process (each scope that holds the person's names), for release: made while their keys still exist, sent by the service loop before its restart (service.rs). Empty while they are locked: nothing could be stamped, and what is on the disk is already unopenable once release renews the device-only secret.
pub fn vault_stamp_drops() -> Vec<Vec<u8>> {
    // The person's items domain goes with them (docs/STORE.md "Removal": release drops their items with their domains), its clips' extents with it (the vault drops a domain's extents by its owner tag).
    with_lock_held_or(|| STORES.iter().filter(|sc| crate::kvault::identity_within(sc.prefix)).filter_map(|sc| stamp(VOP_DROP, sc.prefix.as_bytes(), &[true], &[], &[])).chain(stamp(VOP_DROP, ITEMS_PREFIX, &[true], &[], &[])).collect())
}

/// A server receiving on `ep` takes the head of its queue now; a polling server asleep between polls is woken (its sleep returns early with 1, which a poll loop reads as "look again"). `handoff`: the caller blocks right after on this core, so the server runs here next (`handoff`); the kernel's own call (`vault_call`) carries on, and the server is woken to the front (`enqueue_woken`). Until 2026-11-11 a sleeping poller woken this way went to the back of the run queue: the WiFi driver waited ~9.7 ms for a core behind Mahere's threads on each of ip's calls.
fn wake_server(ep: usize, handoff: bool) {
    let kk = k();
    if let Some(server) = kk.threads.iter().position(|q| matches!(q, Some(q) if matches!(q.state, State::Receiving(e, ..) if e == ep))) {
        deliver(server, ep, handoff);
    } else if let Some(s) = kk.endpoints[ep].poller {
        if let Some(q) = th(s) {
            // Asleep (back with 1: a call ended the sleep), in a timed interrupt wait (a driver that serves an endpoint and waits for its device in one loop: back with 2, as at its deadline), or in a word wait that asked for it (back with 3, SYS_WAIT x3 bit 1).
            if matches!(q.state, State::Sleeping(_)) || matches!(q.state, State::Waiting(a, _) if a < 1024) || (matches!(q.state, State::Waiting(..)) && q.wait_call) {
                q.frame.x[0] = match q.state {
                    State::Sleeping(_) => 1,
                    State::Waiting(a, _) if a < 1024 => 2,
                    _ => 3,
                };
                q.state = State::Runnable;
                if handoff {
                    self::handoff(s, WAKE_CALL);
                } else {
                    enqueue_woken(s, WAKE_CALL);
                }
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
    // The speaker amplifiers (AUDIO.md, payloads/amp): HSI0 USI1's SPI port (spi_19). The right's select (gpn1-0) is in gpio-gpn0 and the left's reset (gpp1-1) in gpio-gpp0, above. Clocks, buck A, pin modes and the right's reset are the kernel's (`POWERS`).
    Hw { name: "usi1-spi", base: 0x111C_0000, size: 0x1000, via_el3: false },
    // The NFC controller (docs/NFC.md): hsi2c_0, the bus it shares with the vibration motor, both run by the `nfc` process (KERNEL-OUT.md decision 8: the bus has one owner). Its interrupt line gpa7-6 is read in gpio-alive, above. Pin modes, both chips' resets and the clock pad are the kernel's (`POWERS`).
    Hw { name: "hsi2c0", base: 0x10C8_0000, size: 0x1000, via_el3: false },
    // Bluetooth (docs/BT.md): the APM's USI1 UART (serial_18). The USI mode word and the chip's power lines are the kernel's (`POWERS`).
    Hw { name: "apm-usi1-uart", base: 0x155D_0000, size: 0x1000, via_el3: false },
    // The torch (docs/TORCH.md, payloads/torch): hsi2c_14, the LM3644 flash driver's bus, which it shares with the camera PMIC (the `torch` and `camera` switches hold it one at a time). Pins, clock source, the controller's bring-up and the chip's enable are the kernel's (`POWERS`).
    Hw { name: "hsi2c14", base: 0x1098_0000, size: 0x1000, via_el3: false },
    // The camera driver (docs/CAMERA.md "The driver", payloads/camera): the main sensor's bus (hsi2c_1, its lens actuator on it too), the ISP front end whole (its core, statistics blocks and the PDMA engine, 1 MiB: zuma-isp.dtsi `lwis_isp_fe`), the CSI links and the combo D-PHY (`csis-link-phy`, up to the front end's first SYSMMU), and SYSREG_ISPFE's first page (the receiver's input select). The buses' pins and clocks, the module's rails, the front end's domain and clocks, the SYSMMUs and the S2MPUs are the kernel's (the `camera` switch, camera_power.rs).
    Hw { name: "hsi2c1", base: 0x1088_0000, size: 0x1000, via_el3: false },
    // The other cameras' sensor buses (docs/CAMERA.md "The other cameras"): hsi2c_3 the ultrawide's (its lens actuator on it too), hsi2c_2 the front camera's, hsi2c_4 the tele's. Their pins, clock sources and modules' power are the `camera` switch's, as hsi2c_1's are.
    Hw { name: "hsi2c3", base: 0x108A_0000, size: 0x1000, via_el3: false },
    Hw { name: "hsi2c2", base: 0x1089_0000, size: 0x1000, via_el3: false },
    Hw { name: "hsi2c4", base: 0x108B_0000, size: 0x1000, via_el3: false },
    Hw { name: "ispfe", base: 0x1C30_0000, size: 0x10_0000, via_el3: false },
    Hw { name: "csis", base: 0x1C04_0000, size: 0xD_0000, via_el3: false },
    Hw { name: "ispfe-sysreg", base: 0x1C02_0000, size: 0x1000, via_el3: false },
];

/// A device's power switch a spawn can grant (`bridge spawn --power NAME`): the kernel's sequence for the rails, clocks and pin modes behind a userspace driver.
pub struct PowerSwitch {
    pub name: &'static str,
    pub set: fn(bool) -> bool,
}
/// The switches SYS_POWER and `power_offs_apply` run with the kernel lock let go (2026-11-16, kernlock): each a kernel-side sequence for a device whose driver reaches it only thru lock-free register syscalls, and whose hardware no path under the lock touches from another core. Bluetooth's is pcie1's too (off before WL_REG_ON is cycled): it held the lock 19.5 ms twice in a camera stream.
/// Since 2026-11-18 also:
/// - the sensors' (sensorlink.rs `power`): it sets one atomic for the service loop (`START`) and touches no hardware at all; the AoC's start it asks for is the service loop's, on this same core.
/// - the camera's (camera_power.rs; 43 ms of the kernel lock at an open, `diag lock`), kept under the lock until now because its region's fence, under the kernel lock on any core (`cam_ask`, `cam_free`), writes the S2MPUs the switch also writes. The fence and the region it follows now have a lock of their own (`FENCE`), which both sides take, so the lock that kept them apart is no longer the kernel's; the S2MPUs sit in the front end's domain, which the switch brings on once and never gates again (`FE_ON`), so a fence from another core never finds it down. The rest of the switch is the camera PMIC's bus (hsi2c_14, shared with the torch, whose switch is refused while this one holds it, both on the kernel's core), ACPM (the kernel's core alone), the domain's EL3 calls (the pd gating pass, the kernel's core alone), and the front end's clocks, sysmmus, engine and receiver, which no path under the kernel lock touches.
/// - the USB link's (ptlink.rs `power`; 11 ms of the kernel lock at a reload's boot, `diag lock`, the eUSB2 PHY's and the repeater's waits): its link reset, the one piece of the lock's state it touches, is taken under the lock inside it (`with_lock_held_or`); the rest is hsi2c_11 (the TCPC and the repeater, which only the kernel's core reaches: the battery gauge's polls and the diag commands are the service loop's), ACPM (the kernel's core only, as for the camera's), the eUSB2 PHY, the controller's DCTL and HSI0's S2MPU and SYSMMU, which nothing else in the kernel writes after boot.
/// Not the display's (`dpu`: EL3 calls into the PMU, which a via-EL3 grant's SYS_MMIO on another core also makes), the panel's (its reset line shares GPA6_CON with the volume-up key's pin mode) nor android's: unaudited, they keep the lock.
const SWITCH_UNLOCKED: [&str; 10] = ["gps", "nfc", "bt", "touch", "amp", "torch", "sensors", "camera", "usb", "gpu-clock"];
pub static POWERS: &[PowerSwitch] = &[PowerSwitch { name: "gps", set: crate::gps::power }, PowerSwitch { name: "nfc", set: crate::nfc::power }, PowerSwitch { name: "bt", set: crate::bt::power }, PowerSwitch { name: "usb", set: crate::ptlink::power }, PowerSwitch { name: "touch", set: crate::touch::power }, PowerSwitch { name: "sensors", set: crate::sensorlink::power }, PowerSwitch { name: "dpu", set: crate::dpu_power::power }, PowerSwitch { name: "panel", set: crate::panel_power::power }, PowerSwitch { name: "panel-vddd", set: crate::panel_power::vddd }, PowerSwitch { name: "android", set: crate::service::android_request }, PowerSwitch { name: "amp", set: crate::amp::power }, PowerSwitch { name: "torch", set: crate::torch::power }, PowerSwitch { name: "camera", set: crate::camera_power::power }, PowerSwitch { name: "gpu-clock", set: crate::service::gpu_clock }];
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
        let (base, size) = region(i);
        if size > 0 {
            consider(base, size, r.name, p.cspace.iter().any(|c| c.object == Object::Dma(i)));
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
/// Written under the kernel lock only; an atomic so the service loop can see that nothing waits without taking the lock (`power_offs_apply`).
static POWER_OFF_PENDING: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Switch `i` set by a process: on, owned by its epoch; off, owned by no one. Not the `android` switch: a launch request, not a power.
fn power_owned(i: usize, epoch: u64) {
    if POWERS[i].name == "android" {
        return;
    }
    unsafe {
        POWER_OWNER[i] = epoch;
        POWER_OFF_PENDING.fetch_and(!(1 << i), core::sync::atomic::Ordering::Relaxed);
    }
}

/// A process ended (exit, fault, kill): every switch it left on is to be turned off on the kernel's core.
fn power_orphaned(epoch: u64) {
    unsafe {
        for i in 0..POWERS.len() {
            if POWER_OWNER[i] == epoch && epoch != 0 {
                POWER_OWNER[i] = 0;
                POWER_OFF_PENDING.fetch_or(1 << i, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// The switches whose owners ended, turned off now. On the kernel's core only (every switch is set there, as `SYS_POWER` requires): the service loop each pass, and `diag restart` between its kill and its spawn, so the new process finds the switch off rather than turned off under it.
pub fn power_offs_apply() {
    if 1u16 << core() != kernel_mask() {
        return;
    }
    // Nothing waiting: no lock taken.
    // Before 2026-11-19 every pass took the kernel lock once per switch (13), about 33,000 holds a second at the 2,500 passes a second of a Lumis save, and the X3 spent 36 % of its time waiting for and letting go of the lock here (`flame`, KERNEL.md "The service loop's lock holds").
    // A bit is set only under the lock by a process's end; one set after this read is seen at the next pass, and one cleared after it (the switch taken again) is read again under the lock below.
    let waiting = POWER_OFF_PENDING.load(core::sync::atomic::Ordering::Relaxed);
    if waiting == 0 {
        return;
    }
    for i in 0..POWERS.len() {
        if waiting >> i & 1 == 0 {
            continue;
        }
        // Taken under the lock; a switch in `SWITCH_UNLOCKED` switched with it let go (as SYS_POWER switches it, 2026-11-16). A process switching it on again can only do so on this core, so not before this returns: then it owns it and it stays on.
        let free = SWITCH_UNLOCKED.contains(&POWERS[i].name);
        // Whether it was pending, and a switch not in `SWITCH_UNLOCKED` switched under this same hold, as before.
        let (pending, under) = with_lock(|| {
            let p = POWER_OFF_PENDING.fetch_and(!(1 << i), core::sync::atomic::Ordering::Relaxed) >> i & 1 != 0;
            (p, (p && !free).then(|| (POWERS[i].set)(false)))
        });
        let done = if pending && free { Some((POWERS[i].set)(false)) } else { under };
        if let Some(ok) = done {
            crate::flashlog::line(&alloc::format!("power {}: off, its process ended ({})", POWERS[i].name, if ok { "done" } else { "the switch refused" }));
        }
    }
}

pub const ERR: u64 = u64::MAX;
/// What a polling `recv` returns when no message is waiting.
pub const NOTHING: u64 = u64::MAX - 1;
/// What a kist launch (`SYS_SPAWN` kist mode) answers for a kist already running on a surface: this bit, the surface in bits 8-15, its generation (preference byte 8 + surface) in bits 0-7 (`kist_request`). Bit 62: no length, address or other answer reaches it.
pub const RUNNING_ON: u64 = 1 << 62;
/// What `spawn` reports for a process still running when its slice ended (it continues in later slices).
pub const PREEMPTED: u64 = 0x5110 << 48;
/// Time slice, in generic-timer ticks per millisecond multiples.
const SLICE_MS: u64 = 10;
/// Exit value marking a fault: FAULT | EC << 40 | faulting PC (arc offset) << 16 | fault address (arc offset, low 16 bits).
pub const FAULT: u64 = 0xFA << 56;
/// What `spawn` reports for a process that is still alive, blocked in recv or call.
pub const PARKED: u64 = 0xB10C << 48;

/// Copy the head of an endpoint's queue into a server waiting in recv: message into its buffer, a fresh single-use reply token into its arc and cspace, x0 = message length. The server becomes runnable.
fn deliver(server: usize, ep: usize, handoff: bool) {
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
    // The caller is about to block, and the server it woke is the thread to run now (a direct hand-off, on this core); the kernel's own call wakes it to the front. A server taking its own queue's head in recv is already this core's: not a wake to measure.
    if server == cur_get().unwrap_or(usize::MAX) {
        if !place_here(server) {
            enqueue_front(server);
        }
    } else if handoff {
        self::handoff(server, WAKE_CALL);
    } else {
        enqueue_woken(server, WAKE_CALL);
    }
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
/// Whether this core may take queued thread `t` now: allowed here, and its process not held (a thread of a held process waits in the queue until the hold ends, `hold_try`: its arc is moving or changing its mask). A dead entry counts, so `pick_runnable` drops it.
fn may_pick(t: usize) -> bool {
    let kk = k();
    kk.threads.get(t).and_then(|x| x.as_ref()).is_none_or(|x| runs_here(x) && !kk.procs.get(x.proc).and_then(|p| p.as_ref()).is_some_and(|p| p.hold.is_some()))
}
fn pick_runnable() -> Option<usize> {
    let kk = k();
    let ok = may_pick;
    // devsched `first`: a thread its device's interrupt woke (with front credit left) goes ahead of every other, wherever the queue holds it.
    let urgent = |t: usize| kk.threads.get(t).and_then(|x| x.as_ref()).is_some_and(|x| x.urgent);
    let first = crate::devsched::FIRST.load(core::sync::atomic::Ordering::Relaxed);
    let i = first.then(|| kk.runnable.iter().position(|&t| urgent(t) && ok(t))).flatten().or_else(|| kk.runnable.iter().position(|&t| ok(t)))?;
    let t = kk.runnable.remove(i);
    runnable_sync();
    t
}

/// `mmio_read`/`mmio_write` without the kernel lock (2026-11-11: a PCIe read in L1.2 takes tens of µs, and with every register access holding the lock the other cores spent 17 % of their time waiting for it). Returns the syscall's value, or None for the locked path (`el0_trap`), which answers everything this does not.
///
/// What it reads, and why each is safe with no lock (the thread T of process P is on this core, IRQs masked as the trap entered, from the entry to the eret):
/// - `FAST[c]`: written only by `cur_set` on this same core, set to P's `Grants` when T began running here; it changes only when this core switches, which it is not doing.
/// - P's `Grants`: a box P owns. P is dropped only when none of its threads is on another core (`end_process` and `kill_locked` keep a process with a thread elsewhere as `dying`; `drain_dying` drops it with its last thread, and T is still in the table), and T is `CURRENT` here, so the box outlives this access. Its `hw` is written once at spawn; P's grants never change after it (only reply tokens enter or leave its cspace).
/// - The token: not read at all. It is presented by its slot in P's grants page (the offset `ferros_sys::cap` gives), a page the kernel wrote at spawn and maps read-only at every level, so the token at slot s is the token of cspace entry s, and `hw[s]` says what the constant-time lookup would have found. A token presented from anywhere else (copied into the heap) goes the locked way, unchanged.
/// - Revocation: P's grants are revoked only by its end. `mmio_quiesce` (under the lock, in `end_process` and `kill_locked`) sets `ended` and then waits while any core's `MMIO_BUSY` names P's grants; here `MMIO_BUSY` is set before `ended` is read, both SeqCst. In the one total order either this read of `ended` comes after the store (it sees true and takes the locked path, which drains T) or the store of `MMIO_BUSY` comes before the quiesce reads it (the quiesce waits for this access). So once a process's end returns, none of its register accesses is in flight or can start: what it switched on is turned off (`power_offs_apply`) and its device handed on only after its last access, as under the lock.
/// - The arc, the run queue, the cspace: not touched. A hold (an arc move or a mask change) waits for T to leave the core as before: the hold's SGI stays pending while IRQs are masked here and parks T as soon as the eret lands at EL0.
fn mmio_fast(f: &Frame) -> Option<u64> {
    use core::sync::atomic::Ordering::{Relaxed, Release, SeqCst};
    let c = core();
    let g = unsafe { (*(&raw const FAST))[c] };
    if g.is_null() {
        return None;
    }
    // SAFETY: see above: P's box, alive while T is current on this core.
    let g = unsafe { &*g };
    // The slot the token was presented from. WHY the checks: the offset is the process's; one not on a slot of its grants page (below it, between two tokens, past its grants) is the locked path's to answer. Below the slots wraps to a huge number and fails the length test with the rest.
    let at = f.x[0].wrapping_sub((TOKENS + 4) as u64);
    let slot = (at >> 5) as usize;
    if at & 31 != 0 || slot >= g.hw.len() || g.hw[slot] == 0 {
        return None;
    }
    let hw = &HARDWARE[g.hw[slot] as usize - 1];
    let write = f.x[8] == SYS_MMIO_WRITE;
    // A write only EL3 may make (the PMU) goes thru the monitor's SMC under the lock, as before: not a hot path, and the SMC is the kernel's.
    if write && hw.via_el3 {
        return None;
    }
    let off = f.x[1] as usize;
    // WHY: the offset is the process's; aligned and inside the grant, as the locked path requires. A DMA fence is never written by its driver (the locked path's rule).
    if off % 4 != 0 || off >= hw.size || (write && crate::s2mpu::is_fence(hw.base)) {
        MMIO_PATHS[0].fetch_add(1, Relaxed);
        return Some(ERR);
    }
    let busy = &MMIO_BUSY[c];
    busy.store(g as *const Grants as usize, SeqCst);
    if g.ended.load(SeqCst) {
        busy.store(0, Release);
        return None;
    }
    let nr = f.x[8];
    crate::platform::crumb(0x90 + nr);
    crate::platform::note_mmio((hw.base + off) as u64 | (write as u64) << 63);
    // An access to a PCIe link's window noted with its start, so one the endpoint never answers is ended by the service loop (pcie_guard.rs).
    let link = crate::pcie_guard::begin(c, hw.base + off);
    let v = if write {
        unsafe { core::ptr::write_volatile((hw.base + off) as *mut u32, f.x[2] as u32) };
        note_write(hw.base + off);
        0
    } else {
        unsafe { core::ptr::read_volatile((hw.base + off) as *const u32) as u64 }
    };
    if link {
        crate::pcie_guard::end(c);
    }
    busy.store(0, Release);
    crate::platform::crumb(0x70 + nr);
    MMIO_PATHS[0].fetch_add(1, Relaxed);
    Some(v)
}

/// Process P ends (under the lock, `end_process` and `kill_locked`): no register access of it starts after this, and one in flight on another core finishes before it returns (`mmio_fast` has the argument). The wait is one register access at most: the core in it holds no lock and masks its interrupts until its eret.
fn mmio_quiesce(g: &Grants) {
    use core::sync::atomic::Ordering::SeqCst;
    g.ended.store(true, SeqCst);
    let me = g as *const Grants as usize;
    for b in &MMIO_BUSY {
        while b.load(SeqCst) == me {
            core::hint::spin_loop();
        }
    }
}

/// Called from `.Lel0_sync` for every synchronous exception taken from EL0, with the running process's live frame. Returns 0 to eret through `f` (which may now be another process's frame), 1 to leave EL0 (value in f.x[0]).
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_trap(f: &mut Frame) -> u64 {
    // The profiler's note of this trap (prof.rs): who called, from where, and the call (x8 for an `svc`, ESR class G#15; G#FFFF for anything else).
    crate::prof::enter(if f.esr >> 26 == 0x15 { f.x[8] } else { 0xFFFF }, f.elr, f.x[29], f.x[30]);
    // A register access, answered without the kernel lock when it can be (`mmio_fast`): back to the same thread, nothing else changes.
    if f.esr >> 26 == 0x15 && (f.x[8] == SYS_MMIO_READ || f.x[8] == SYS_MMIO_WRITE) {
        if let Some(v) = mmio_fast(f) {
            f.x[0] = v;
            crate::prof::leave();
            // The same check as every return to EL0 (the killswitch and the slice timer need IRQs open there). The tick re-arm below is not needed: this path never unmasks, so a tick that fires meanwhile stays pending and parks the thread as the eret lands.
            frame_check(f);
            return 0;
        }
    }
    // A yield (`sleep(0)`, std's `yield_now`) with nothing queued, no interrupt waiting to be noticed and no deadline passed: back to the same thread without the lock, as the locked path would answer (its endpoint test is for a sleep with a deadline, `sleep_until_or_call`). rayon's idle workers yield 32 times before they sleep, and with every yield taking the lock four cores of them kept the X3's render thread waiting on it (2026-11-17). What it reads: three atomics a core may read at any time; a thread queued just after the look is the next yield's, or the slice's end's, and a tick or a hold's SGI that comes meanwhile stays pending (IRQs are masked from the trap to the eret) and parks the thread as the eret lands, as for `mmio_fast`.
    if f.esr >> 26 == 0x15 && f.x[8] == SYS_SLEEP && f.x[0] == 0 && !OLD.load(core::sync::atomic::Ordering::Relaxed) {
        use core::sync::atomic::Ordering::{Acquire, Relaxed};
        let idle = RUNNABLE_N.load(Acquire) == 0 && unsafe { core::ptr::read_volatile(&raw const __irq_any) } == 0 && NEXT_WAKE.load(Acquire) > crate::platform::cntpct();
        if idle {
            YIELDS[0].fetch_add(1, Relaxed);
            crate::prof::leave();
            frame_check(f);
            return 0;
        }
    }
    lock();
    // A deadline passed while every core that could notice it ran a thread (`wake_sleepers` runs only between slices): noticed by any trap now, so a sleeper's lateness is bounded by the next syscall anywhere, not by the next slice's end (2026-11-17, Photon's scroll: map, touch and display.1 waited 2.4 to 4.5 ms past their deadlines behind four rayon workers' slices).
    if NEXT_WAKE.load(core::sync::atomic::Ordering::Acquire) <= crate::platform::cntpct() && !OLD.load(core::sync::atomic::Ordering::Relaxed) {
        wake_sleepers();
    }
    // What this hold is (devsched `WHAT`, `diag lock`): the syscall, its x1's low byte, the thread.
    crate::devsched::WHAT.store(1 << 63 | (f.x[8] & 0xFF) | (f.x[1] & 0xFF) << 8 | (cur_get().unwrap_or(0xFFFF) as u64 & 0xFFFF) << 16, core::sync::atomic::Ordering::Relaxed);
    // SYS_NOTIFY's first op of an unlock reads the person's notes from the vault: here, before `el0_trap` takes any reference into the tables, with the lock let go (notify.rs `preload_held`); a process ended meanwhile is drained at `el0_trap`'s entry.
    if f.x[8] == SYS_NOTIFY && (f.esr >> 26) & 0x3F == 0x15 {
        notify::preload_held();
    }
    let r = el0_trap(f);
    // A kept setting changed (`keep_settings`): the document as it stands, snapshotted under the lock, then written with the lock let go and interrupts open (`unlocked`: no table is touched, only the document in the heap, under the vault's lock), before the caller goes on.
    // A write that failed answers ERR, to the thread that asked if it is still the one here (the four calls that ask never switch).
    if KEEP_ASKED[core()].swap(false, core::sync::atomic::Ordering::Relaxed) {
        if let Some((doc, at)) = settings_snapshot() {
            let asker = cur_get();
            let ok = unlocked(|| unsafe { SETTINGS_WRITE }.is_some_and(|w| w(&doc, at)));
            if !ok {
                k().console.extend_from_slice(b"settings: the document was not kept (the vault refused the write)\n");
                if asker.is_some() && cur_get() == asker && r == 0 {
                    f.x[0] = ERR;
                }
            }
        }
    }
    // The notes' vault writes this syscall made (notify.rs `keep`), the same way.
    notify::flush_held();
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
            // Its mailboxes marked dead only now, with no thread of it left to publish over the mark.
            mailboxes_gone(p.epoch);
            arc_free(p.arc);
            // The slot empty only now: the table changed again (the process count the facts' bell watches, service.rs `bell_facts`).
            PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
            // Its end looked at the endpoints again, now that it is out of the table: a call queued while it was dying asked for an on-demand server the service loop then saw alive (`named_alive_locked` counts a dying process) and did not start, and nothing asked again (the Pro, b1064, 2026-11-20: the camera ended after Lumis's LIST with a thread still on another core, Lumis's OPEN came in between, and it waited for good).
            fail_orphaned_calls();
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
        // The whole of ESR, FAR and the PC for `end_process`'s line on the glass and its entry in the flash log (the exit value keeps only parts of them), with the link register (a leaf function's caller, which no frame record holds yet), the call chain from the faulting frame's x29 and the thread.
        let fault = proc_of(cur).map(|p| Fault { esr: f.esr, far: f.far, pc: f.elr.wrapping_sub(base), lr: f.x[30].wrapping_sub(base), frames: frames(p, f.elr, f.x[29]), thread: thread_called(cur) });
        unsafe { *(&raw mut LAST_FAULT) = fault };
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
            // WHY: the entry and the stack are the process's offsets, and a thread must start inside its own arc (a masked process's first instruction is checked code only there). Masked into the arc while every arc was a power of two; a native arc is any number of frames now, so they are checked: one outside is refused.
            if entry >= p.arc.size as u64 || stack > p.arc.size as u64 {
                f.x[0] = ERR;
            } else {
                let mut frame = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0, v: [0; 32], fpcr: 0, fpsr: 0, tpidr: 0, _pad: 0 };
                frame.elr = p.arc.base as u64 + entry;
                frame.sp_el0 = p.arc.base as u64 + (stack & !15);
                frame.x[28] = p.arc.base as u64;
                frame.x[27] = p.arc.x27() as u64;
                frame.x[26] = p.arc.base as u64;
                frame.x[0] = arg;
                let mask = t.mask;
                let nt = add_thread(Thread { proc: pi, frame, state: State::Runnable, mask, called: 0, what: 0, ran: 0, ran_at: 0, woke: 0, wkind: 0, irq_at: 0, irq_line: 0, irq_core: 0, irq_noticed: 0, device: false, urgent: false, on_irq: false, run_line: 0, irq_run_at: 0, last_core: 0, wait_call: false, label: [0; 16] });
                enqueue_back(nt);
                tr(TR_RUN, nt, 0xFFFF);
                f.x[0] = 0;
            }
            0
        }
        SYS_PANIC => {
            // WHY the mins: the message and the file are the process's, any length; the ledger keeps only their first 2^9 and 2^8 bytes (ferros_ledger death), so only those are read.
            let message = p.read(f.x[0], (f.x[1] as usize).min(ferros_ledger::death::MESSAGE_MAX));
            let file = p.read(f.x[2], (f.x[3] as usize).min(ferros_ledger::death::FILE_MAX));
            let chain = frames(p, f.elr, f.x[29]);
            p.panicked = Some(alloc::boxed::Box::new(Panicked { message, file, line: f.x[4] as u32, column: (f.x[4] >> 32) as u32, thread: thread_called(cur), frames: chain }));
            f.x[0] = 0;
            0
        }
        SYS_THREAD_NAME => {
            // WHY the min: the name is the process's; only its first 16 bytes are kept, so only they are read.
            let n = if f.x[1] < 16 { f.x[1] as usize } else { 16 };
            let name = p.read(f.x[0], n);
            t.label = [0; 16];
            t.label[..n].copy_from_slice(&name);
            f.x[0] = 0;
            0
        }
        SYS_PIN => {
            let off = PIN_OFF.load(core::sync::atomic::Ordering::Relaxed);
            t.mask = if f.x[0] as u16 == 0 || (off != 0 && off == pin_key(&p.name)) { MASK_WORKERS } else { f.x[0] as u16 };
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
        SYS_IRQ_ARM => {
            let (lt, ht) = (p.read(f.x[0], 32), p.read(f.x[1], 32));
            let line = p.lookup(&lt).map(|i| p.cspace[i].object);
            let hw = p.lookup(&ht).map(|i| p.cspace[i].object);
            f.x[0] = match (line, hw) {
                (Some(Object::Irq(i)), _) if f.x[3] == 0 => {
                    crate::irqarm::disarm(i, IRQS[i].intid);
                    0
                }
                (Some(Object::Irq(i)), Some(Object::Hardware(g))) => {
                    let h = &HARDWARE[g];
                    // Checked once here, never at the interrupt: a grant only EL3 may write, a DMA fence (the kernel's), or a PCIe window (a read never answered would wedge the core taking the interrupt) is refused; then the program against the grant's size (ferros_layout irqarm `check`: whole registers inside it, known operations, sane masks).
                    // WHY the length test: x3 is the caller's; past the most a program can be it is refused before anything is read.
                    if f.x[3] as usize > ferros_layout::irqarm::OPS_MOST * ferros_layout::irqarm::OP_BYTES || h.via_el3 || crate::s2mpu::is_fence(h.base) || crate::pcie_guard::link_of(h.base).is_some() || crate::pcie_guard::link_of(h.base + h.size - 4).is_some() {
                        ERR
                    } else {
                        let bytes = p.read(f.x[2], f.x[3] as usize);
                        match ferros_layout::irqarm::check(&bytes, h.size) {
                            Ok((ops, n)) => {
                                crate::irqarm::arm(i, IRQS[i].intid, h.base, &ops[..n], p.epoch);
                                0
                            }
                            Err(_) => ERR,
                        }
                    }
                }
                _ => ERR,
            };
            0
        }
        SYS_IRQ_WAIT | SYS_IRQ_ACK => {
            let token = p.read(f.x[0], 32);
            match p.lookup(&token).map(|i| p.cspace[i].object) {
                Some(Object::Irq(i)) => {
                    let intid = IRQS[i].intid;
                    // A thread that waits on a line is a device thread from now on (devsched: never asked to yield for another, and the dedicated core's).
                    if nr == SYS_IRQ_WAIT {
                        t.device = true;
                    }
                    if nr == SYS_IRQ_ACK {
                        f.x[0] = irq_ack_guarded(i, intid, &p.name);
                        0
                    } else if IRQ_STORMED[i].load(core::sync::atomic::Ordering::Relaxed) != 0 {
                        // A line that stormed (`irq_storm_check`): masked for good, and its holder told so on every wait.
                        f.x[0] = ERR;
                        0
                    } else if irq_pending_take(intid) {
                        // Back at once: the interrupt came while the thread was not waiting (devsched: a late wait, and how long the interrupt sat).
                        frame_mailbox_flush();
                        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                        // WHY saturating: the stamp is another core's counter read (devsched `ran`).
                        crate::devsched::late(i, crate::platform::cntpct().saturating_sub(unsafe { core::ptr::read_volatile(&raw const __irq_at[intid as usize]) }));
                        f.x[0] = 0;
                        0
                    } else if kk.endpoints.iter().any(|e| e.poller == Some(cur) && !e.queue.is_empty()) {
                        // A call already waiting on an endpoint this thread polls: back at once with 2, as `wake_server` would have woken it (2026-11-10). A driver that waits on its device and its endpoint in one wait (the GPS driver, with no deadline since then) could otherwise miss a call that came between its last poll and this wait, and answer it only at its device's next interrupt.
                        f.x[0] = 2;
                        0
                    } else if irq_storm_check(i, intid, &p.name) != 0 {
                        // The wait unmasks the line as an ack does, so it is guarded the same: a driver that waits again and again without clearing its source storms as one that acks.
                        f.x[0] = ERR;
                        0
                    } else {
                        // Not for an armed line whose interrupt is taken and not yet marked pending (irqarm.rs `TAKEN`): `.Lirq` masks it a few instructions on, and an unmask landing after that fires it again, running its program twice on one event.
                        if !crate::irqarm::taken(intid) {
                            crate::killswitch::enable_spi_to(intid, irq_route(t, intid));
                        }
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
                            if deadline != 0 {
                                NEXT_WAKE.fetch_min(deadline, core::sync::atomic::Ordering::AcqRel);
                            }
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
        // A word in the arc, or a mailbox's sequence word thru a region the caller holds (2026-11-19, `mbox_wait_word`): waiters of every process on one mailbox are kept under its view's address, so its writer's wake finds them all.
        SYS_WAIT => match p.word(f.x[0]).or_else(|| mbox_wait_word(p, f.x[0])).or_else(|| gpub_word(p, f.x[0])) {
            // A word outside the arc: the syscall resolves or traps (`bad_buf`).
            None => {
                p.bad_buf(f.x[0], 8, false);
                f.x[0] = ERR;
                0
            }
            Some(addr) => {
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
                } else if f.x[3] & 2 != 0 && kk.endpoints.iter().any(|e| e.poller == Some(cur) && !e.queue.is_empty()) {
                    // x3 bit 1, a call ends it: one already waiting on an endpoint this thread polls answers 3 at once, as `sleep` answers 1 and `irq_wait` 2.
                    f.x[0] = 3;
                    0
                } else {
                    // A wait on the world (x3 bit 0): the watchdog's clock paused while the thread that pets is in it.
                    if f.x[3] & 1 != 0 && cur == p.pet_thread {
                        p.paused_at = t_now;
                    }
                    // x3 bit 1 (2026-11-11): a call to an endpoint this thread polls ends the wait with 3 (`wake_server`), so a server can wait on a word of its own (a doorbell its other thread rings) and on its callers at once: the IP stack's idle wait. Opt-in: a server waiting on its own work (the display's publish workers) must not be ended by every client's call.
                    t.wait_call = f.x[3] & 2 != 0;
                    t.state = State::Waiting(addr, deadline);
                    t.frame = *f;
                    t.frame.x[0] = 0;
                    if deadline != 0 {
                        NEXT_WAKE.fetch_min(deadline, core::sync::atomic::Ordering::AcqRel);
                    }
                    tr(TR_SLEEP, cur, 0xFFFF);
                    if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
                }
            }
        },
        // A mailbox's writer waking its readers (2026-11-19): its own sequence word, every process's waiters on that mailbox, in the one scan below (`mbox_wake`).
        SYS_WAKE if p.word(f.x[0]).is_none() && mbox_wake_word(p, f.x[0]).is_some() => {
            f.x[0] = mbox_wake(mbox_wake_word(p, f.x[0]).unwrap(), f.x[1]);
            0
        }
        // A word of a GPU block, by its owner or the GPU service (2026-11-20, `gpub_word`): every process's waiters on it, as a mailbox's writer wakes its readers.
        SYS_WAKE if p.word(f.x[0]).is_none() && gpub_word(p, f.x[0]).is_some() => {
            f.x[0] = mbox_wake(gpub_word(p, f.x[0]).unwrap(), f.x[1]);
            0
        }
        SYS_WAKE => {
            // A word outside the arc has no waiter (a wait there is refused): none woken.
            let addr = p.word(f.x[0]);
            let mut n = 0u64;
            for i in 0..kk.threads.len() {
                if n >= f.x[1] {
                    break;
                }
                if let Some(x) = th(i).filter(|x| x.proc == pi && matches!(x.state, State::Waiting(a, _) if Some(a) == addr)) {
                    x.state = State::Runnable;
                    // As any other wake: to the front while its front credit holds, a busy worker asked to make way when none is idle (2026-11-17: at the back always, a futex wake that found the A715s busy waited behind the whole queue, up to a slice).
                    if OLD.load(core::sync::atomic::Ordering::Relaxed) {
                        woken(i, WAKE_WORD);
                        enqueue_back(i);
                    } else {
                        enqueue_woken(i, WAKE_WORD);
                    }
                    tr(TR_WAKE, i, 0xFFFF);
                    n += 1;
                }
            }
            f.x[0] = n;
            0
        }
        SYS_ARC => arc_call(f, cur, pi),
        SYS_BLOCK => {
            let token = p.read(f.x[0], 32);
            // A window by name, or a clip's extent given to this process: an extent is the `clips` window narrowed to its own blocks (so every bound below holds it there).
            let window = match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Blocks(i), r)) if r & RIGHT_SEND != 0 => WINDOWS.get(i).map(|w| BlockWindow { name: w.name, offset: w.offset, blocks: w.blocks }),
                Some((Object::Extent(i), r)) if r & RIGHT_SEND != 0 => {
                    let g = unsafe { (*(&raw const EXTENT_GRANTS))[i] };
                    (g.holder == p.epoch).then_some(BlockWindow { name: "clips", offset: CLIPS_OFFSET + g.first, blocks: g.blocks })
                }
                _ => None,
            };
            // Bit 3: the vault process gives (or, with bit 4, takes back) a clip's extent of its `clips` window (`extent_give`).
            let give = (f.x[1] & 8 != 0).then(|| {
                let named = matches!(p.lookup(&token).map(|i| p.cspace[i].object), Some(Object::Blocks(_)));
                match &window {
                    // Bit 5 with bit 3: whether a live writer holds the extent from window block x2 (1) or none does (0): the vault recovers an open extent only when nobody is writing it.
                    Some(w) if named && f.x[1] & 32 != 0 && w.name == "clips" => unsafe { (*(&raw const EXTENT_GRANTS)).iter().any(|g| g.holder != 0 && g.first == f.x[2] && k().procs.iter().flatten().any(|r| r.epoch == g.holder && !r.dying)) as u64 },
                    Some(w) if named => extent_give(p, w, f.x[2], f.x[3], f.x[4], f.x[1] & 16 != 0),
                    _ => ERR,
                }
            });
            let window = window.as_ref();
            let (write, timed, count, at, buf) = (f.x[1] & 1 != 0, f.x[1] & 2 != 0, (f.x[1] >> 8) & 0xFFFF, f.x[2], f.x[3]);
            f.x[0] = match (window, crate::ufs_device::part()) {
                _ if give.is_some() => give.unwrap_or(ERR),
                // The owner's fresh start is zeroing the flash (service.rs `vault fresh`): nothing more is read or written before its reset.
                _ if BLOCKS_SHUT.load(core::sync::atomic::Ordering::SeqCst) => ERR,
                // Op bit 6: the device's write cache to the medium (SYNCHRONIZE CACHE of LU 0, the whole unit, so any window's writes; the owner, 2026-11-22: a flush per commit). The vault process's manifestus flush after each copy's batch and spine block. x1: the ticks it took. With the kernel lock let go, under the UFS lock.
                (Some(_), Some(_)) if f.x[1] & 64 != 0 => {
                    let t0 = crate::platform::cntpct();
                    let ok = unlocked(|| ufs_sync(0));
                    f.x[1] = crate::platform::cntpct() - t0;
                    if ok { 0 } else { ERR }
                }
                // Op bit 2 (development, docs/RAW-VIDEO.md): the flash at a queue deeper than one, on the `rawbench` window only (it writes the same bytes everywhere, so never where anything is kept): x4's low 32 bits commands of `count` blocks at consecutive places from `at`, its bits 32-39 the depth (1 to QUEUE_MOST); every command's data the caller's `count` blocks (a write) or the bounce (a read, nothing copied back).
                // The answer: x1 the ticks from the first doorbell to the last completion, x2 the commands that completed well.
                // WHY the bounds: the commands, depth and place are the process's; all of them inside the window, the depth inside the slots the HAL keeps.
                #[cfg(not(ferros_production))]
                (Some(w), Some((first, blocks))) if f.x[1] & 4 != 0 && w.name == "rawbench" && (1..=BLOCK_MAX).contains(&count) && (1..=ferros_hal::ufs::QUEUE_MOST as u64).contains(&(f.x[4] >> 32 & 0xFF)) && (f.x[4] & 0xFFFF_FFFF) != 0 && at < w.blocks && (f.x[4] & 0xFFFF_FFFF) * count <= w.blocks - at && w.offset + w.blocks <= blocks as u64 => {
                    use core::sync::atomic::Ordering::Relaxed;
                    let base = (first as u64 + w.offset + at) as u32;
                    let (n, cmds, depth) = (count as usize, (f.x[4] & 0xFFFF_FFFF) as usize, (f.x[4] >> 32 & 0xFF) as usize);
                    let mut data = alloc::vec![0u8; n << 12];
                    if write {
                        p.read_into(buf, &mut data);
                    }
                    let (ok, ticks) = unlocked(|| {
                        UFS.with(|| {
                            let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
                            let run = (&raw mut UFS_RUN) as usize;
                            if ferros_hal::ufs::PRDT_STRIDE.load(Relaxed) == 0 {
                                u.probe_prdt_stride(0, base, run);
                            }
                            // SAFETY: the bounce holds RUN_MAX blocks and n is at most BLOCK_MAX (RUN_MAX), checked above; it is touched only under the UFS lock, held here.
                            unsafe { core::slice::from_raw_parts_mut(run as *mut u8, n << 12) }.copy_from_slice(&data);
                            u.run_queued(0, base, n, cmds, depth, run, write)
                        })
                    });
                    f.x[1] = ticks;
                    f.x[2] = ok as u64;
                    if ok == cmds { 0 } else { ERR }
                }
                // WHY the bounds: the block number and count are the process's; a window is all it may touch, and the partition must hold the window (the same refusal the kernel's own vault makes).
                (Some(w), Some((first, blocks))) if (1..=BLOCK_MAX).contains(&count) && at < w.blocks && count <= w.blocks - at && w.offset + w.blocks <= blocks as u64 => {
                    use core::sync::atomic::Ordering::Relaxed;
                    let base = (first as u64 + w.offset + at) as u32;
                    let n = count as usize;
                    // The command runs with the kernel lock let go (2026-11-18, `unlocked`: it held every core up to 1.2 ms a run, `diag lock`), under the UFS lock, which the run bounce is one with.
                    // The caller's memory is read and written only under the kernel lock (another of its threads may shrink its arc or give frames back meanwhile), so the bytes go thru a copy in the kernel heap on either side: about 50 us a MiB under the kernel lock, against the command's whole length before.
                    let command = move |data: Option<&[u8]>, out: Option<&mut Vec<u8>>| -> (bool, u64) {
                        let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
                        let run = (&raw mut UFS_RUN) as usize;
                        // The PRDT's stride, learned at the first access of more than one block (`probe_prdt_stride`, the run buffer its scratch): until it is known a run goes a block a command.
                        if n > 1 && ferros_hal::ufs::PRDT_STRIDE.load(Relaxed) == 0 {
                            u.probe_prdt_stride(0, base, run);
                        }
                        // SAFETY: the bounce holds RUN_MAX blocks and n is at most BLOCK_MAX (RUN_MAX), checked above; it is touched only under the UFS lock, held here.
                        let bytes = unsafe { core::slice::from_raw_parts_mut(run as *mut u8, n << 12) };
                        if let Some(d) = data {
                            bytes.copy_from_slice(d);
                            return (u.write_run_lun(0, base, n, run), crate::platform::cntpct());
                        }
                        let ok = u.read_run_lun(0, base, n, run);
                        let t = crate::platform::cntpct();
                        if let (true, Some(o)) = (ok, out) {
                            o.extend_from_slice(bytes);
                        }
                        (ok, t)
                    };
                    // The recording region's first write this boot turns the flash's WriteBooster on (docs/RAW-VIDEO.md: its SLC cache took about 33 GB at 1 to 1.7 GB/s where the direct rate is 556 MB/s), with its flushes (when idle, and in hibernate), as Linux does at every probe; the flags are volatile, so it is asked again each boot, and left on.
                    if write && w.name == "clips" {
                        CLIPS_AT.store(crate::platform::cntpct(), Relaxed);
                    }
                    if write && w.name == "clips" && !CLIPS_WB.swap(true, Relaxed) {
                        let r = unlocked(|| {
                            UFS.with(|| {
                                let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
                                u.h8_wake_now();
                                // fWriteBoosterEn G#0E, fWBBufferFlushEn G#0F, fWBBufferFlushDuringHibernate G#10 (JEDEC UFS 3.1).
                                (0..3u8).fold(0u64, |acc, k| acc | (u.set_flag(0x0E + k) as u64) << k)
                            })
                        });
                        CLIPS_WB_SET.store(r | 1 << 8, Relaxed);
                    }
                    let t0 = crate::platform::cntpct();
                    let (ok, t_cmd) = if write {
                        // One copy from where the caller's bytes lie, no zeroed buffer first (a bad buffer is kept for the trap at the syscall's end, as `read_into` keeps it, and writes zeros).
                        let data = match p.view(buf, n << 12) {
                            Some(b) => b.to_vec(),
                            None => alloc::vec![0u8; n << 12],
                        };
                        unlocked(|| UFS.with(|| command(Some(&data), None)))
                    } else {
                        if count >= MOVING_BLOCKS {
                            crate::ptlink::MOVING_AT.store(t0, Relaxed);
                            BLOCK_STATS[7].fetch_add(1, Relaxed);
                        }
                        let mut data = Vec::with_capacity(n << 12);
                        let (ok, t) = unlocked(|| UFS.with(|| command(None, Some(&mut data))));
                        // The caller found again: the process table may have moved meanwhile.
                        if let (true, Some(p)) = (ok, proc_of(cur)) {
                            p.write_from(buf, &data);
                        }
                        BLOCK_STATS[6].fetch_add(t - t0, Relaxed);
                        (ok, t)
                    };
                    let t1 = crate::platform::cntpct();
                    let s = if write { 3 } else { 0 };
                    BLOCK_STATS[s].fetch_add(1, Relaxed);
                    BLOCK_STATS[s + 1].fetch_add(count, Relaxed);
                    BLOCK_STATS[s + 2].fetch_add(t1 - t0, Relaxed);
                    if timed {
                        f.x[1] = t_cmd - t0;
                        f.x[2] = t1 - t_cmd;
                    }
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
            f.x[0] = if ok && op == 3 {
                // Op 3: this vault process answers gets of up to `len` bytes (its reply buffer), held to the kernel's own bound (WHY the min: the length is the vault process's, and SYS_REPLY copies at most STORE_GET_MAX). Forgotten when another process serves.
                unsafe { VAULT_GET = (p.epoch, len.min(STORE_GET_MAX)) };
                0
            } else if !ok || len > STORE_MAX {
                ERR
            } else {
                // Each op's vault call made with the kernel lock let go (`unlocked`, under the vault's own lock), the caller found again after it: at a boot the listing waited 46 ms for the vault's lock behind the AoC's firmware (2026-11-18, `diag lock`). The domain keys and the read's cache stay under the kernel lock.
                let (skip, buf) = (f.x[2] as usize, f.x[4]);
                match op {
                    0 => {
                        let names = unlocked(|| unsafe { STORE_KEYS }.map(|keys| keys()));
                        match (names, proc_of(cur)) {
                            (Some(Ok(names)), Some(p)) => migrate_next(p, names, skip, buf, len),
                            _ => ERR,
                        }
                    }
                    1 => {
                        let name = name();
                        let miss = scope_of(&name).is_some() && unsafe { (*(&raw const MIGRATE_CACHE)).as_ref() }.is_none_or(|(k, _)| k != name.as_bytes());
                        let got = if miss { unlocked(|| unsafe { STORE }.map(|store| store(&name, None))) } else { None };
                        match proc_of(cur) {
                            Some(p) => migrate_read(p, &name, offset, buf, len, got),
                            None => ERR,
                        }
                    }
                    2 => {
                        let name = name();
                        match scope_of(&name) {
                            Some(sc) => {
                                unsafe { *(&raw mut MIGRATE_CACHE) = None };
                                // Said apart from the removal's own line ("store remove ... (process)", which a process's delete also writes): this entry lives on in the vault process, under a scope a kernel without it will not look in.
                                crate::flashlog::line(&alloc::format!("store moved {name:?} into the vault process, scope {}", sc.name));
                                match unlocked(|| unsafe { STORE_REMOVE }.map(|rm| rm(&name))) {
                                    Some(Ok(true)) => 0,
                                    Some(Ok(false)) => NOTHING,
                                    _ => ERR,
                                }
                            }
                            None => ERR,
                        }
                    }
                    _ => ERR,
                }
            };
            0
        }
        SYS_STORE if f.x[1] & STORE_ITEMS != 0 => match vault_route() {
            Some(Ok(ep)) => items_forward(f, cur, ep),
            _ => {
                f.x[0] = ERR;
                0
            }
        },
        SYS_STORE if names_stow(p, f) => match kist_store_route(p, f) {
            Ok((ep, d)) => kist_domain_forward(f, cur, ep, &d),
            Err(()) => {
                f.x[0] = ERR;
                0
            }
        },
        SYS_STORE if store_route(p, f).is_some() => match store_route(p, f).unwrap() {
            Ok((ep, sc)) => store_forward(f, cur, ep, sc),
            Err(()) => {
                f.x[0] = ERR;
                0
            }
        },
        // An extent lives only in the vault process: the kernel's own vault has none.
        SYS_STORE if f.x[1] & STORE_EXTENT != 0 => {
            f.x[0] = ERR;
            0
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
                    // The `camera` switch takes which module in bits 8-15 (camera_power.rs `select`, ferros_sys `power_arg`): bit 0 alone is on or off for it, as x1 == 1 is for every other switch.
                    let on = if POWERS[i].name == "camera" {
                        crate::camera_power::select((f.x[1] >> 8) as u8);
                        f.x[1] & 1 == 1
                    } else {
                        f.x[1] == 1
                    };
                    // An audited switch runs with the kernel lock let go (2026-11-16, `diag lock`: the camera's on held it 42 to 66 ms, Bluetooth's for pcie1 19.5 ms twice, and a camera stream 400 us from its deadline died behind them): a switch is hardware sequences with their waits.
                    // Why that is safe: every switch runs on the kernel's core only (this syscall is refused anywhere else; the service loop's `power_offs_apply` is on the same core), so two never overlap; the switches keep their state in their own statics and touch none of the lock's (no `process` state; the flash log takes its own, `LOG`); the caller's thread stays on this core, so its process is not freed meanwhile, and `p` is not used again after the lock is taken back (a spawn on another core may move the process table).
                    // Only the switches in `SWITCH_UNLOCKED` (whose hardware nothing under the lock touches from another core; the camera's fence has its own lock since 2026-11-18); the rest keep the lock.
                    let free = SWITCH_UNLOCKED.contains(&POWERS[i].name);
                    let epoch = p.epoch;
                    // A switch going off: every line this process armed disarmed first, so no armed action touches a block whose clocks or power go (irqarm.rs).
                    if !on {
                        crate::irqarm::disarm_epoch(epoch);
                    }
                    // Interrupts open meanwhile too (`unlocked`): the kernel's core takes every granted line by default, and a switch's waits with them masked held a device's interrupt off as long.
                    let ok = if free { unlocked(|| (POWERS[i].set)(on)) } else { (POWERS[i].set)(on) };
                    if ok {
                        power_owned(i, if on { epoch } else { 0 });
                        // Its process ended on another core meanwhile (`kill_locked` found its thread here and left it `dying`): what it switched on is turned off as for any other ending.
                        if k().procs.iter().flatten().any(|q| q.epoch == epoch && q.dying) {
                            power_orphaned(epoch);
                        }
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
        SYS_UNLOCK if f.x[1] == 5 && export_route(p, f).is_some() => match export_route(p, f).unwrap() {
            Ok(ep) => export_forward(f, cur, ep),
            Err(()) => {
                f.x[0] = ERR;
                0
            }
        },
        // Android on this device (owner, 2026-11-21; docs/BOOT.md "Android only by the identity's choice"), op 13 for a Settings holder (the Settings page), x2 the request: 0 start Android now, 1 Android off on this device (and the setting back to ferros), 2 Android on again for a person who accepted the disclaimer before, 3 the state (bit 0 this device allows Android, bit 1 the person present accepted the disclaimer, bit 2 it starts into Android), 4 start into ferros, 5 start into Android (only while a person is present on a device that allows Android), 6 Android's recovery request waiting on the person, 7 yes to it, 8 no to it (docs/BOOT.md "Recovery requests from Android"). 0 to 2 only while a person is present; the vault's half of 0 to 3 is service.rs `store_unlock` (its ops 13, 15, 16, 17); 4 and 5 are the kernel's own setting (settings.rs `START_ANDROID`), kept in the device document. → 1 done, 0 refused, ERR without the right or a vault.
        SYS_UNLOCK if f.x[1] == 13 => {
            let token = p.read(f.x[0], 32);
            let held = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0);
            let android = || crate::settings::START_ANDROID.load(core::sync::atomic::Ordering::Relaxed);
            let start_into = |on: bool| {
                if android() != on {
                    crate::settings::START_ANDROID.store(on, core::sync::atomic::Ordering::Relaxed);
                    keep_settings();
                }
            };
            f.x[0] = match (held, unsafe { UNLOCK }) {
                (true, Some(_)) if f.x[2] == 4 => {
                    start_into(false);
                    1
                }
                (true, Some(_)) if f.x[2] == 5 => {
                    let ok = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) && crate::service::ANDROID_ALLOWED.load(core::sync::atomic::Ordering::Relaxed) == 2;
                    if ok {
                        start_into(true);
                    }
                    ok as u64
                }
                // 6: Android's recovery request waiting on the person (service.rs `RECOVERY_ASK`, ferros_hal::bcb `ASK_*`), 0 while nobody is present: the question the Android page shows.
                (true, Some(_)) if f.x[2] == 6 => {
                    let ask = crate::service::RECOVERY_ASK.load(core::sync::atomic::Ordering::Acquire);
                    if crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) { ask as u64 } else { 0 }
                }
                // 7 yes, allow it once (only a wipe or a rescue, the person's alone to allow; owner, 2026-11-02), 8 no (any request): taken by the service loop, which checks the person again and misc again (service.rs `recovery_yes`, `recovery_no`).
                (true, Some(_)) if f.x[2] == 7 || f.x[2] == 8 => {
                    let ask = crate::service::RECOVERY_ASK.load(core::sync::atomic::Ordering::Acquire);
                    let yes = f.x[2] == 7;
                    let ok = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) && ask != ferros_hal::bcb::ASK_NONE && (!yes || crate::service::recovery_allowable(ask));
                    if ok {
                        crate::service::RECOVERY_ANSWER.store(if yes { 1 } else { 2 }, core::sync::atomic::Ordering::Release);
                    }
                    crate::flashlog::line(&alloc::format!("recovery: the Android page answered {} ({})", if yes { "yes" } else { "no" }, if ok { "taken" } else { "refused" }));
                    ok as u64
                }
                // WHY the bound: x2 is the caller's; only the four requests below exist.
                (true, Some(unlock)) if f.x[2] < 4 => match unlock([13, 15, 16, 17][f.x[2] as usize], None) {
                    // Android off: what this device starts into goes back to ferros with it.
                    Ok(1) if f.x[2] == 1 => {
                        start_into(false);
                        1
                    }
                    Ok(s) if f.x[2] == 3 => s | (android() as u64) << 2,
                    r => r.unwrap_or(ERR),
                },
                _ => ERR,
            };
            0
        }
        // Op 15, remove this device from its identity (AUTH.md "Remove this device"; owner, 2026-11-22: press remove, type the handle, unbind; the fleet told at once, then the identity's data erased and the killswitch's power off). Only the identity service (`fgtw` by name, never a kist), which tells the fleet before it asks.
        // x2 a buffer in the caller's arc: in, [flags u8: bit 0 a dry run][the typed handle's seed 32][each kist the person has, (developer 32, option 32)]; x3 its length; x4 its room. Out, written over it: a dry run's account (lines of text, `remove_dry_text`), else nothing.
        // A removal: the seed must be the person present's (service.rs op 18, compared in registers); then each kist's own domain is stamped for a drop in the vault process while the person's keys still exist, and op 19 releases (op 4's release: every scope's domain of the person's dropped, the kernel's identity class, tags, bound mark and kept seed deleted, the device-only secret renewed, the lock) and powers the device off once the drops are sent. → 1 removed, 0 the seed is not the person present's (nothing changed), ERR nobody present, no right, or a buffer of another shape. A dry run → the account's whole length (what fits is written); nothing changed, no seed needed.
        SYS_UNLOCK if f.x[1] == 15 => {
            let token = p.read(f.x[0], 32);
            let held = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Unlock, r)) if r & RIGHT_SEND != 0);
            let (at, len, room) = (f.x[2], f.x[3] as usize, f.x[4] as usize);
            // WHY the bounds: the lengths are the caller's: the flags and a seed, then whole kist identities, at most 2^8 of them (each costs a derivation here, under the kernel lock), and room for at least what was sent.
            let fits = len >= 33 && (len - 33) % 64 == 0 && (len - 33) >> 6 <= 1 << 8 && room >= len;
            f.x[0] = match (held && fits && !p.kist && p.name.as_slice() == b"fgtw", unsafe { UNLOCK }) {
                (true, Some(unlock)) => {
                    let mut msg = p.read(at, len);
                    let r = if msg[0] & 1 == 1 {
                        let t = remove_dry_text(&msg[33..]);
                        let b = t.as_bytes();
                        // WHY the min: `room` is the caller's buffer; only what it holds is written, and the whole length is returned so it can ask again with more.
                        p.write(at, &b[..b.len().min(room)]);
                        b.len() as u64
                    } else {
                        let mut seed = [0u8; 32];
                        seed.copy_from_slice(&msg[1..33]);
                        let m = unlock(18, Some(&seed));
                        crate::keys::wipe_to_dram(&mut seed);
                        match m {
                            Ok(1) => {
                                // Each kist's own domain of the person's, stamped while their keys still exist (the release locks them).
                                let mut drops: Vec<Vec<u8>> = kist_domains(&msg[33..]).iter().filter_map(|d| stamp(VOP_DROP, &[KIST_DOMAIN_PREFIX, &d[..]].concat(), &[true], &[], &[])).collect();
                                match unlock(19, None) {
                                    Ok(_) => {
                                        crate::service::release_drops_add(drops);
                                        1
                                    }
                                    Err(()) => {
                                        for d in drops.iter_mut() {
                                            crate::keys::wipe_to_dram(d);
                                        }
                                        ERR
                                    }
                                }
                            }
                            Ok(_) => {
                                crate::flashlog::line("identity remove refused: the handle typed is not the person present's; nothing changed");
                                0
                            }
                            Err(()) => ERR,
                        }
                    };
                    crate::keys::wipe_to_dram(&mut msg);
                    r
                }
                _ => ERR,
            };
            // A removal locked the identity.
            identity_bell();
            0
        }
        // Attest with a tag (ops 6-9): the Unlock right, or Settings for keeping, forgetting and listing (`tag_syscall`).
        SYS_UNLOCK if (6..=9).contains(&f.x[1]) => {
            f.x[0] = tag_syscall(p, f);
            // An unlock by tag locks whatever was open first.
            identity_bell();
            0
        }
        SYS_UNLOCK => {
            let token = p.read(f.x[0], 32);
            let op = f.x[1];
            f.x[0] = match (p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), unsafe { UNLOCK }) {
                (Some((Object::Unlock, r)), Some(unlock)) if r & RIGHT_SEND != 0 => match op {
                    0 => unlock(0, None).unwrap_or(ERR),
                    1 => {
                        let mut seed = [0u8; 32];
                        // Read straight into the array: `read` would leave a heap copy of the seed behind.
                        p.read_into(f.x[2], &mut seed);
                        let r = unlock(1, Some(&seed));
                        crate::keys::wipe_to_dram(&mut seed);
                        r.unwrap_or(ERR)
                    }
                    2..=4 => unlock(op as u8, None).unwrap_or(ERR),
                    // Op 11: the identity's doorbell, x2 a word in the caller's arc (8-aligned, as SYS_WAIT takes one).
                    // The state as op 0 answers it, with a change count above bit 8 (`ID_WORD`), is written there now and at every change, waking whatever of the process waits on it (`identity_ring`, one of the bells).
                    // The state is read live first, so an unlock at boot (before any bell existed) is in the first word.
                    // One word a process; a second registration moves it. → 0, ERR with no vault.
                    11 => match unlock(0, None) {
                        Ok(s) => {
                            identity_ring(s);
                            bell_register(p, pi, BELL_ID_STATE, f.x[2], ID_WORD.load(core::sync::atomic::Ordering::Acquire))
                        }
                        Err(()) => ERR,
                    },
                    // Op 10: the person key from the identity service (FACETS.md D1), 32 bytes at x2, held beside the identity's keys until lock; ERR while nobody is present. This copy wiped at once.
                    10 => {
                        let mut key = [0u8; 32];
                        p.read_into(f.x[2], &mut key);
                        let r = unlock(10, Some(&key));
                        crate::keys::wipe_to_dram(&mut key);
                        r.unwrap_or(ERR)
                    }
                    // Op 12, the retype gate (ahau_core::retype; docs/AUTH.md "Unlock at boot"): turn attest at boot on with the seed of the handle typed again, 32 bytes at x2; the reply document into x3, x4 its length.
                    // Only a seed that is the person present's turns it on (`retype_keep`, compared in the kernel), and the kernel keeps that seed for unlock at boot at once, so the next boot unlocks with no attest typed in between; anything else changes nothing. No tag reaches this: a tag unlock (ops 6 to 9) never keeps the seed nor sets the setting (owner, 2026-11-18: "no tags"). SYS_SET_PREF refuses to turn it on, so this is the one way a process does.
                    // Returns the document's whole length (what fits of it is written), or ERR when no document could be made.
                    // Only the attest screen's keyboard (ahau, by name and never a kist, as PREF_DND is fgtw's): the identity service holds the Unlock right and the seed all session, and must not be able to switch it on without the handle typed.
                    12 if p.kist || p.name.as_slice() != b"ahau" => ERR,
                    12 => {
                        let mut seed = [0u8; 32];
                        p.read_into(f.x[2], &mut seed);
                        let r = unlock(12, Some(&seed));
                        crate::keys::wipe_to_dram(&mut seed);
                        use ahau_core::retype::{reply, Reply};
                        let (answer, why) = match r {
                            Ok(1) if pref(PREF_UNLOCK_AT_BOOT) == 1 => (Reply { error: false, present: true, matched: true, on: true }, "on already"),
                            Ok(1) if keep_settings() => {
                                set_pref_locked(PREF_UNLOCK_AT_BOOT, 1);
                                (Reply { error: false, present: true, matched: true, on: true }, "on")
                            }
                            Ok(1) => (Reply { error: true, present: true, matched: true, on: false }, "the setting was not kept"),
                            Ok(2) => (Reply { error: true, present: true, matched: true, on: pref(PREF_UNLOCK_AT_BOOT) == 1 }, "the seed could not be kept"),
                            Ok(_) => (Reply { error: true, present: true, matched: false, on: pref(PREF_UNLOCK_AT_BOOT) == 1 }, "they do not match"),
                            Err(()) => (Reply { error: true, present: false, matched: false, on: pref(PREF_UNLOCK_AT_BOOT) == 1 }, "no seed held: nobody present"),
                        };
                        match reply(answer, why, crate::settings::eagle_now()) {
                            Some(doc) => {
                                // WHY the min: x4 is the caller's buffer length; only what it holds is written.
                                p.write(f.x[3], &doc[..doc.len().min(f.x[4] as usize)]);
                                doc.len() as u64
                            }
                            None => ERR,
                        }
                    }
                    // Op 14, the disclaimer's typed confirmation (owner, 2026-11-01 and 2026-11-21; docs/BOOT.md "Android only by the identity's choice"): Android allowed on this device once the person present has read the disclaimer in full and typed their own handle again, 32 bytes of its seed at x2; the reply document (ahau_core::retype, `on` = Android allowed now) into x3, x4 its length. Only the attest screen's keyboard (ahau, by name and never a kist), as op 12.
                    14 if p.kist || p.name.as_slice() != b"ahau" => ERR,
                    14 => {
                        let mut seed = [0u8; 32];
                        p.read_into(f.x[2], &mut seed);
                        let r = unlock(14, Some(&seed));
                        crate::keys::wipe_to_dram(&mut seed);
                        use ahau_core::retype::{reply, Reply};
                        let (answer, why) = match r {
                            Ok(1) => (Reply { error: false, present: true, matched: true, on: true }, "Android allowed on this device"),
                            Ok(2) => (Reply { error: true, present: true, matched: true, on: false }, "the acceptance could not be kept"),
                            Ok(_) => (Reply { error: true, present: true, matched: false, on: false }, "they do not match"),
                            Err(()) => (Reply { error: true, present: false, matched: false, on: false }, "nobody present"),
                        };
                        match reply(answer, why, crate::settings::eagle_now()) {
                            Some(doc) => {
                                // WHY the min: x4 is the caller's buffer length; only what it holds is written.
                                p.write(f.x[3], &doc[..doc.len().min(f.x[4] as usize)]);
                                doc.len() as u64
                            }
                            None => ERR,
                        }
                    }
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
            // The lock op, an unlock (it locks what was open first) and a release move the lock epoch: the identity service hears it at once.
            identity_bell();
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
            // A kist never speaks for a link, whatever its name, here, in SYS_DETAIL, SYS_ZONE or SYS_LANG (2026-11-10): its name is its developer's, and with GPS started only on a claim a kist named `gps` could launch while the driver is down and speak as it.
            let slot = [b"modem" as &[u8], b"pcie1", b"bt", b"nfc", b"gps", b"usb"].iter().position(|n| !p.kist && *n == p.name.as_slice());
            f.x[0] = match slot {
                // The slot comes from the process: bounded so a bad one is refused, not a write past the radio's eight.
                // A word other than the one it held rings the link's bell (SYS_BELL): the page showing the link draws again, and a driver setting the same word again rings nothing.
                Some(i) if f.x[0] < 8 => {
                    if STATUS[i][f.x[0] as usize].swap(f.x[1], core::sync::atomic::Ordering::Relaxed) != f.x[1] {
                        bell_count(i);
                    }
                    0
                }
                // Slots 8 and up: the link's tracks, bounded as the measures are.
                Some(i) if f.x[0] < (8 + TRACKS) as u64 => {
                    if TRACK[i][f.x[0] as usize - 8].swap(f.x[1], core::sync::atomic::Ordering::Relaxed) != f.x[1] {
                        bell_count(i);
                    }
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
                    if keep_settings() {
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
                if kept && !keep_settings() {
                    CONTROL[link][slot].store(was, Relaxed);
                    ERR
                } else {
                    CONTROL_GEN[link].fetch_add(1, Relaxed);
                    0
                }
            };
            // A knob set: the drivers asleep on a word look at their blocks now (2026-11-13: the WiFi driver waits on its chip's interrupt and no longer looks every quarter second), as a preference's change wakes them.
            if f.x[0] == 0 {
                wake_word_waiters();
            }
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
            let named = |n: &[u8]| !p.kist && p.name.as_slice() == n;
            let settings = |at: u64| {
                let token = p.read(at, 32);
                matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0)
            };
            let keep = || keep_settings();
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
                // The GPS driver: the zone in effect's principal place, its first aid when the person has no last fix (owner, 2026-11-11, "can't we figure that out from the orbital data anyway?": the chip needs to know roughly where it is to know which sky to look at). Into x1, 8 bytes: latitude, longitude, i32 arc-seconds.
                7 if named(b"gps") => match crate::zone::place() {
                    Some((lat, lon)) => {
                        let mut b = [0u8; 8];
                        b[..4].copy_from_slice(&lat.to_le_bytes());
                        b[4..].copy_from_slice(&lon.to_le_bytes());
                        p.write(f.x[1], &b);
                        8
                    }
                    None => ERR,
                },
                _ => ERR,
            };
            0
        }
        // The GPS driver asked for a Settings holder that holds a GPS claim (the map's GPS page): forwarded as the caller's own call, CALL_LIVE (the caller passes it in x4, so its registers come back as it set them), so a driver still replaying its session answers ERR at once instead of holding the page.
        SYS_CLAIM if f.x[0] == 3 => {
            let token = p.read(f.x[2], 32);
            let settings = matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0);
            let claimed = unsafe { (*(&raw const CLAIMS)).iter().any(|c| c.epoch == p.epoch && right_is_gps(c.right)) };
            let ep = endpoint(b"gps");
            // Only the driver's reading ops pass: FIX and SATS, CALL_LIVE; and WATCH (3, x1 bits 8-39 the change count last seen), the driver's doorbell, held by it until its fixes or fix messages move. WATCH is not CALL_LIVE (the caller passes the bare length): a driver still replaying its session, or not yet started for the claim, takes it when it first receives, so the page learns the driver came up without asking again. STOP (1) is the kernel's to give, by the claims.
            let op = f.x[1] & 0xFF;
            if settings && claimed && matches!(op, 0 | 2) && f.x[4] & CALL_LIVE != 0 && has_receiver(ep) {
                call_endpoint(f, cur, ep, alloc::vec![op as u8], f.x[3], f.x[4] & !CALL_LIVE)
            } else if settings && claimed && op == 3 && f.x[4] & CALL_LIVE == 0 {
                let mut msg = alloc::vec![3u8];
                msg.extend_from_slice(&((f.x[1] >> 8) as u32).to_le_bytes());
                call_endpoint(f, cur, ep, msg, f.x[3], f.x[4])
            } else {
                f.x[0] = ERR;
                0
            }
        }
        SYS_CLAIM => {
            // WHY the bound: the right is the caller's, a code into ferros_kist::RIGHTS (`right_is_gps` reads it with `get`); past a byte it names nothing.
            let gps = f.x[1] < 1 << 8 && right_is_gps(f.x[1] as u8);
            let held = |at: u64| {
                let token = p.read(at, 32);
                p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)).filter(|o| o.1 & RIGHT_SEND != 0).map(|o| o.0)
            };
            let claims = unsafe { &mut *(&raw mut CLAIMS) };
            f.x[0] = match f.x[0] {
                0 if gps => {
                    // The proof: Settings (the settings page), or the `gps` endpoint's send right (a kist the person granted GPS; its slot is empty, never this, when they said no).
                    let proved = match held(f.x[2]) {
                        Some(Object::Settings) => true,
                        Some(Object::Endpoint(e)) => kk.endpoints[e].name == b"gps",
                        // A GPS mailbox's view (2026-11-19): held only thru one of the two above (SYS_MAILBOX's proof) or granted by name.
                        Some(Object::Dma(i)) => DMA[i].name == ferros_mailbox::VIEW_NAMES[ferros_mailbox::GPS],
                        _ => false,
                    };
                    // GPS is locked until a person attests (radios at attest), and a GPS switched off refuses every claim (preference 15, bit 4).
                    let person = crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire);
                    let allowed = pref(PREF_RADIOS_OFF) >> 4 & 1 == 0;
                    if !(proved && person && allowed) {
                        ERR
                    } else {
                        if !claims.iter().any(|c| c.epoch == p.epoch && c.right == f.x[1] as u8) {
                            claims.push(Claim { epoch: p.epoch, right: f.x[1] as u8, capsule: p.capsule });
                            claims_publish();
                        }
                        0
                    }
                }
                1 => {
                    let before = claims.len();
                    claims.retain(|c| !(c.epoch == p.epoch && c.right as u64 == f.x[1]));
                    if claims.len() != before {
                        claims_publish();
                    }
                    0
                }
                2 if matches!(held(f.x[2]), Some(Object::Settings)) => {
                    let mine: Vec<&Claim> = claims.iter().filter(|c| c.right as u64 == f.x[1]).collect();
                    // WHY the bound: the buffer is the caller's; whole records only, as many as fit, and the count says how many there are.
                    let fit = (f.x[4] as usize / CLAIM_REC).min(mine.len());
                    let mut out = Vec::with_capacity(fit * CLAIM_REC);
                    for c in &mine[..fit] {
                        out.push(c.capsule.is_some() as u8);
                        out.extend_from_slice(&c.capsule.unwrap_or([0; 32]));
                    }
                    p.write(f.x[3], &out);
                    mine.len() as u64
                }
                // The claims' doorbell for a Settings holder (the map's permissions and GPS pages): a word in its arc (x1) the kernel writes GPS_CLAIMS into now and at every change, waking whatever of it waits there (`claims_publish`, one of the bells, SYS_BELL). One word a process; a second registration moves it.
                4 if matches!(held(f.x[2]), Some(Object::Settings)) => {
                    bell_register(p, pi, BELL_CLAIMS, f.x[1], GPS_CLAIMS.load(core::sync::atomic::Ordering::Acquire))
                }
                _ => ERR,
            };
            0
        }
        SYS_BELL => {
            // WHY the bound: the source is the caller's, an index into the counts; the claims' bell is SYS_CLAIM op 4's, behind Settings.
            let source = f.x[0] as usize;
            f.x[0] = if source <= BELL_FACTS {
                bell_register(p, pi, source, f.x[1], BELL_COUNT[source].load(core::sync::atomic::Ordering::Acquire))
            } else if source == BELL_SURFACES && holds_surfaces(p) {
                bell_register(p, pi, source, f.x[1], SURFACES_LIVE.load(core::sync::atomic::Ordering::Acquire))
            } else if source == BELL_SLOTS && holds_surfaces(p) {
                bell_register(p, pi, source, f.x[1], SLOTS_LIVE.load(core::sync::atomic::Ordering::Acquire))
            } else if source == BELL_IDENTITY {
                bell_register(p, pi, source, f.x[1], LOCK_EPOCH.load(core::sync::atomic::Ordering::Acquire) as u64)
            } else if source == BELL_PUCKS && holds_surfaces(p) {
                bell_register(p, pi, source, f.x[1], PUCKS_LIVE.load(core::sync::atomic::Ordering::Acquire))
            } else if source == BELL_GPU_CAMERA && p.cspace.iter().any(|c| matches!(c.object, Object::Dma(i) if DMA[i].name == "arena")) {
                bell_register(p, pi, source, f.x[1], BELL_COUNT[source].load(core::sync::atomic::Ordering::Acquire))
            } else if source == BELL_FRONT && holds_surfaces(p) {
                // The display's own word, read by the kernel and never written (`BELL_FRONT`): kept with the bells' words for its process's life and for `held_in`, nothing written into it now.
                match p.word(f.x[1]) {
                    Some(addr) => {
                        let words = unsafe { &mut *(&raw mut BELL_WORDS) };
                        words.retain(|w| !(w.0 == pi && w.3 == source));
                        words.push((pi, p.epoch, addr, source));
                        0
                    }
                    None => {
                        p.bad_buf(f.x[1], 8, true);
                        ERR
                    }
                }
            } else {
                ERR
            };
            0
        }
        SYS_NOT_STARTED => {
            let settings = |at: u64| {
                let token = p.read(at, 32);
                matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0)
            };
            f.x[0] = match f.x[0] {
                0 if settings(f.x[1]) => {
                    let list = unsafe { &*(&raw const NOT_STARTED) };
                    // WHY the bound: the buffer is the caller's; whole records only, as many as fit, and the count says how many there are.
                    let fit = (f.x[3] as usize / NOT_STARTED_REC).min(list.len());
                    let mut out = Vec::with_capacity(fit * NOT_STARTED_REC);
                    for r in &list[..fit] {
                        out.push(r.1);
                        out.extend_from_slice(&r.0);
                        out.extend_from_slice(&r.2.to_le_bytes());
                    }
                    p.write(f.x[2], &out);
                    list.len() as u64
                }
                1 if settings(f.x[2]) => {
                    bell_register(p, pi, BELL_KISTS, f.x[1], BELL_COUNT[BELL_KISTS].load(core::sync::atomic::Ordering::Acquire))
                }
                2 if settings(f.x[1]) => {
                    let live = kists_live();
                    // WHY the bound: the buffer is the caller's; whole addresses only, as many as fit, and the count says how many there are.
                    let fit = (f.x[3] as usize / 32).min(live.len());
                    p.write(f.x[2], live[..fit].as_flattened());
                    live.len() as u64
                }
                _ => ERR,
            };
            0
        }
        SYS_OFFER => {
            let stow = |at: u64| {
                let token = p.read(at, 32);
                matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Stow, r)) if r & RIGHT_SEND != 0)
            };
            let offer = unsafe { &mut *(&raw mut OFFER) };
            f.x[0] = match f.x[0] {
                0 if stow(f.x[1]) => bell_register(p, pi, BELL_OFFERS, f.x[2], BELL_COUNT[BELL_OFFERS].load(core::sync::atomic::Ordering::Acquire)),
                // WHY the length: the buffer is the caller's; the answer is 40 bytes, written whole or not at all.
                1 if stow(f.x[1]) && f.x[3] >= 40 => match offer.as_mut().filter(|o| !ferros_kist::offer::finished(o.state)) {
                    Some(o) => {
                        if o.state == ferros_kist::offer::OFFERED {
                            o.state = ferros_kist::offer::CHECKING;
                            crate::flashlog::line("kist offer: the stower took it");
                        }
                        let mut out = [0u8; 40];
                        out[..32].copy_from_slice(&o.capsule);
                        out[32..].copy_from_slice(&o.len.to_le_bytes());
                        p.write(f.x[2], &out);
                        0
                    }
                    None => NOTHING,
                },
                // WHY the length: the buffer is the caller's; the state is 35 bytes, read whole.
                2 if stow(f.x[1]) && f.x[3] >= 35 => {
                    let b = p.read(f.x[2], 35);
                    match offer.as_mut().filter(|o| o.capsule[..] == b[..32] && !ferros_kist::offer::finished(o.state)) {
                        Some(o) => {
                            if (o.state, o.why, o.flags) != (b[32], b[33], b[34]) {
                                crate::flashlog::line(&alloc::format!("kist offer: the stower says {} (why A#{}, flags A#{})", ferros_kist::offer::state_name(b[32]), b[33], b[34]));
                            }
                            o.state = b[32];
                            o.why = b[33];
                            o.flags = b[34];
                            // The service loop answers a host's held read now, not at its next event.
                            unsafe { core::arch::asm!("sev") };
                            0
                        }
                        None => NOTHING,
                    }
                }
                _ => ERR,
            };
            0
        }
        SYS_MAILBOX => {
            f.x[0] = match f.x[0] {
                0 => mbox_open(p, f.x[1], f.x[2], f.x[3]),
                _ => ERR,
            };
            0
        }
        SYS_NOTIFY => {
            notify::syscall(f, p, pi);
            0
        }
        SYS_CALLER => {
            let token = p.read(f.x[0], 32);
            let behind = match p.lookup(&token).map(|i| p.cspace[i].object) {
                Some(Object::Reply(caller)) if caller != KERNEL_CALLER => proc_of(caller).map(|q| (caller_holds(q), q.epoch)),
                _ => None,
            };
            f.x[0] = match behind {
                None => NOTHING,
                // x1: its end watched, when the word is the server's (`end_watch`); a word outside its arc resolves or traps (`bad_buf`), and the answer is ERR.
                Some((holds, epoch)) if f.x[1] != 0 => match end_watch(p, pi, f.x[1], epoch) {
                    true => holds | CALLER_WATCHED,
                    false => ERR,
                },
                Some((holds, _)) => holds,
            };
            0
        }
        SYS_LANG => {
            let settings = |at: u64| {
                let token = p.read(at, 32);
                matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Settings, r)) if r & RIGHT_SEND != 0)
            };
            let keep = || keep_settings();
            // WHY `valid`: the code is the caller's, and is kept in the vault and handed to every process as two bytes; one that is no packed three letters is refused, not cut.
            f.x[0] = match f.x[0] {
                0 if settings(f.x[1]) && crate::lang::valid(f.x[2]) => {
                    if crate::lang::set_device(f.x[2] as u16, keep) { 0 } else { ERR }
                }
                1 if settings(f.x[1]) => {
                    if crate::lang::set_own(f.x[2] == 1, keep) { 0 } else { ERR }
                }
                2 if !p.kist && p.name.as_slice() == b"fgtw" && crate::lang::valid(f.x[1]) => {
                    if crate::lang::set_identity(f.x[1] as u16, keep) { 0 } else { ERR }
                }
                _ => ERR,
            };
            0
        }
        SYS_DETAIL => {
            let own = LINK_NAMES.iter().position(|n| !p.kist && *n == p.name.as_slice());
            f.x[0] = match f.x[0] {
                // A driver's own document: its length bounded so it fits the link's buffer.
                0 => match own {
                    Some(i) if (f.x[2] as usize) <= DETAIL_MAX => {
                        let n = f.x[2] as usize;
                        let doc = p.read(f.x[1], n);
                        // A document other than the one kept rings the link's bell (SYS_BELL), as a measure does.
                        let changed = unsafe { (*(&raw const DETAIL_LEN))[i] != n || (&(*(&raw const DETAIL))[i])[..n] != doc[..] };
                        unsafe {
                            (&mut *(&raw mut DETAIL))[i][..n].copy_from_slice(&doc);
                            (*(&raw mut DETAIL_LEN))[i] = n;
                        }
                        if changed {
                            bell_count(i);
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
                Some((Object::Sensors, r)) if r & RIGHT_SEND != 0 && op == 0 && len >= crate::sensorlink::BYTES && off.checked_add(len).is_some_and(|e| e <= p.arc.size) && !p.in_hole(off, len) => {
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
                // Bit 2 of x4 (2026-11-14): this power session's key for the calling system process, bound to its name, from the wairua (fresh each power session, never stored): the same for every call in one boot, unlinkable to any other boot. The WiFi address while nobody is present (AUTH.md rule 2: no stable identifier on the air while locked), so a driver restarted within a boot rejoins as it was and takes no new lease. The caller passes no domain (x2 = 0): every kernel from before this answers that with ERR, so no caller is ever handed a lasting key where it asked for a passing one. Not to a kist.
                Some((Object::Ira, r)) if r & RIGHT_SEND != 0 && f.x[4] & 4 != 0 => match (!p.kist).then(|| crate::wairua::derive(SESSION_ROOT_CONTEXT)).flatten() {
                    Some(mut root) => {
                        let mut h = blake3::Hasher::new_derive_key(SESSION_KEY_CONTEXT);
                        h.update(&root);
                        h.update(&p.name);
                        root.fill(0);
                        let mut key = *h.finalize().as_bytes();
                        p.write(f.x[3], &key);
                        key.fill(0);
                        32
                    }
                    None => ERR,
                },
                // Bit 1 of x4: the kist's alias (FACETS.md "The alias"; D1, 2026-11-12: the kernel derives): a seed for this kist alone, the same on every device of the person's, derived from the person key and the gate identity the kernel stamped at launch (the developer key and option), never from anything the caller names. Only to a kist the kist job launched, only while the person is present and their identity service has handed in the person key; else ERR. x1 and x2 are not read.
                Some((Object::Ira, r)) if r & RIGHT_SEND != 0 && f.x[4] & 2 != 0 => {
                    let alias = match p.gate {
                        Some((d, o)) if p.kist => crate::service::alias(&d, &o),
                        _ => None,
                    };
                    // The log names the kist and the outcome, never a byte of the alias.
                    let who = alloc::string::String::from_utf8_lossy(&p.name).into_owned();
                    match alias {
                        Some(mut a) => {
                            p.write(f.x[3], &a);
                            crate::keys::wipe_to_dram(&mut a);
                            ALIASES_GIVEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                            // Which kists the person uses is theirs: a personal entry, sealed under their key (LEDGER.md "Classes").
                            crate::flashlog::personal(&alloc::format!("alias given to the kist {who}"));
                            32
                        }
                        None => {
                            // A refusal is the system's to diagnose, often with nobody present: a system entry, the kist's name left out (which kists the person has is theirs), a process that is no kist named.
                            if p.gate.is_none() || !p.kist {
                                crate::flashlog::line(&alloc::format!("alias refused to {who}: not a kist launched by the kist job"));
                            } else {
                                crate::flashlog::line("alias refused to a kist: nobody present, or no person key handed in yet");
                            }
                            ERR
                        }
                    }
                }
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
            // Do not disturb is the identity's (PREF_DND): only its service writes it (as SYS_LANG op 2 the language), by name and never a kist, no Settings needed; a Settings holder never does (a device setting is never written into an identity's preference, SETTINGS.md).
            let held = if index == PREF_DND { !p.kist && p.name.as_slice() == b"fgtw" } else { held };
            // WHY: the person's app scale is any app's to change (owner, 2026-11-20: "settings pages should modify that, same with apps"; every kist follows it, ferros_fluor), so a pinch inside a kist sets it for every kist: for this preference alone the display grant, which every kist holds (the implicit set), is the right.
            let held = held || (index == PREF_SCALE && matches!(p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Endpoint(e), r)) if e == endpoint(b"display") && r & RIGHT_SEND != 0));
            // Attest at boot turns on only thru the retype gate (SYS_UNLOCK op 12: the handle typed again, compared with the person present's in the kernel); off is always safe and stays a plain set (owner, 2026-11-17: "my toddler could do it without me").
            let held = held && !(index == PREF_UNLOCK_AT_BOOT && value != 0);
            // A kept preference is one the settings document carries (settings.rs `FIELDS`), with a vault to keep it in; it is applied now and the document written at the trap's end, the kernel lock let go (`keep_settings`).
            let kept = |index: usize| crate::settings::FIELDS.iter().any(|&(_, i)| i == index) && unsafe { SETTINGS_WRITE }.is_some();
            f.x[0] = if held && f.x[1] >> 9 == 0 && pref_valid(index, value) && (live || kept(index)) {
                set_pref_locked(index, value);
                if !live {
                    keep_settings();
                }
                0
            } else {
                ERR
            };
            0
        }
        // Stow mode (x2 bit 3): a kist the stower stored, read, checked and sealed by the service loop (`stow_request`).
        SYS_SPAWN if f.x[2] & 8 != 0 => {
            f.x[0] = stow_request(p, pi, &p.read(f.x[0], (f.x[1] as usize).min(4096)), f.x[2] & 16 != 0);
            0
        }
        SYS_SPAWN if f.x[2] & 2 != 0 => {
            f.x[0] = kist_request(p, &p.read(f.x[0], (f.x[1] as usize).min(4096)), f.x[2] & 4 != 0, f.x[2] & 32 != 0);
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
            // Where the whole surfaces region was passed: (place in `objects`, its index in `DMA`).
            let mut whole: Vec<(usize, usize)> = Vec::new();
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
                    // The whole surfaces region passes as one surface of it, the child's own (but to the display server, below).
                    Some((Object::Dma(i), r)) if DMA[i].name == "surfaces" => {
                        whole.push((objects.len(), i));
                        objects.push((Object::NewSurface, r))
                    }
                    // The whole blocks' window passes as a 32 MiB block of it, the child's own (GPU-KISTS.md plan step 1, 2026-11-12): init hands the attest screen its region this way.
                    Some((Object::Dma(i), r)) if DMA[i].name == "arena" => objects.push((Object::NewRegion(BLOCK_LEAST_LOG), r)),
                    // A GPU block is its owner's alone: freed when that owner ends (`regions_free`), it could not stay a child's. The camera's view likewise: its holder's, taken back at the camera's close.
                    Some((Object::Dma(i), _)) if DMA[i].name == "gpu-block" || DMA[i].name == "camera-view" => {
                        bad = true;
                        bad_at = bad_at.min(j);
                    }
                    Some(o) => objects.push(o),
                }
            }
            // The display server keeps the whole window (2026-11-12): it composes every surface and watches their owners (`BELL_SURFACES`), and a slot of its own was a surface it never drew in that counted against everyone else's (with four, the attest screen's two and one app's left Photon none on the Pro). The server of `display` is that process, whatever its name.
            if objects.iter().any(|o| matches!(o, (Object::Endpoint(e), r) if *e == endpoint(b"display") && r & RIGHT_RECV != 0)) {
                for &(at, i) in &whole {
                    objects[at].0 = Object::Dma(i);
                }
            }
            // A child handed the Settings capability (the map: the quick row's torch, QUICK.md) gets the torch's send right too, as the slot after its last: init's grants page is full (108 slots), so init cannot hold one to pass on, and the torch's server is the kernel's to start anyway (`ON_DEMAND`, docs/TORCH.md). Who may set the device's settings may light its torch.
            if !bad && objects.iter().any(|o| matches!(o.0, Object::Settings)) {
                objects.push((Object::Endpoint(endpoint(b"torch")), RIGHT_SEND));
            }
            // The server of `fgtw`, the identity service, gets the receive right on `facets` the same way, as the slot after its last (docs/FACETS.md, 2026-11-13): init's page is full, and the facets are its to serve, to every kist the person granted be you or people.
            if !bad && objects.iter().any(|o| matches!(o, (Object::Endpoint(e), r) if *e == endpoint(b"fgtw") && r & RIGHT_RECV != 0)) {
                objects.push((Object::Endpoint(endpoint(b"facets")), RIGHT_RECV));
            }
            // x2 bit 0: only if no live process has this name — init's keepers start a driver again this way (NOTHING: it is running), never a second beside one that is alive but not serving yet.
            let shown = if name.is_empty() { &key } else { &name };
            let once = f.x[2] & 1 != 0;
            if once && named_live(shown) {
                f.x[0] = NOTHING;
            } else {
                // `spawn_queued` lets the kernel lock go for the capsule's read, its checks and its code's copy: `p` is not used after it.
                let held = p.cspace.len();
                let r = if bad { Err("spawn: a token the caller does not hold (or a reply/console)") } else { spawn_queued(shown, &key, &objects, once) };
                let kk = k();
                match r {
                    Ok(Some(_)) => f.x[0] = 0,
                    // Started meanwhile by another spawn of the same name, while the lock was let go.
                    Ok(None) => f.x[0] = NOTHING,
                    Err(e) => {
                        kk.console.extend_from_slice(b"spawn refused: ");
                        kk.console.extend_from_slice(e.as_bytes());
                        if bad {
                            kk.console.extend_from_slice(alloc::format!(" (the request's token {bad_at} of {n}; the caller holds {held})").as_bytes());
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
            if kk.endpoints.iter().any(|e| e.poller == Some(cur) && !e.queue.is_empty()) {
                // A call already waiting on an endpoint this thread polls: back at once with 1, as `wake_server` would have woken it, and as `irq_wait` answers 2 (2026-11-11). A call that came between the server's last poll and this sleep waited out the whole sleep (up to 1 ms an exchange for the WiFi driver).
                f.x[0] = 1;
                0
            } else if f.x[0] <= now {
                // A deadline already passed is a yield: anyone runnable goes first, this process comes back round. A server whose wake time had fallen into the past spun its whole slice on sleep/recv and held every client until the slice timer (the display, 2026-10-23); now it gives the core up on each pass instead.
                // Nothing queued that this core may run (a thread pinned to the X3 does not count): back at once, as with an empty queue, rather than queueing itself only to be picked again.
                f.x[0] = 0;
                let old = OLD.load(core::sync::atomic::Ordering::Relaxed);
                let nothing = if old { kk.runnable.is_empty() } else { !kk.runnable.iter().any(|&r| may_pick(r)) };
                if nothing {
                    YIELDS[1].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    0
                } else {
                    t.state = State::Runnable;
                    t.frame = *f;
                    if old {
                        enqueue_back(cur);
                    } else {
                        queue_back(cur);
                    }
                    tr(TR_YIELD, cur, 0);
                    let r = if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 };
                    YIELDS[2 + (cur_get() == Some(cur)) as usize].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    r
                }
            } else {
                t.state = State::Sleeping(f.x[0]);
                t.frame = *f;
                t.frame.x[0] = 0;
                NEXT_WAKE.fetch_min(f.x[0], core::sync::atomic::Ordering::AcqRel);
                tr(TR_SLEEP, cur, ((f.x[0] - now) * 1000 / crate::platform::cntfrq()).min(65535));
                if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
            }
        }
        SYS_CALL => {
            let token = p.read(f.x[0], 32);
            // An on-demand server called while none runs (`ON_DEMAND`: the torch): the service loop starts it. The call waits for it, a live one too (since 2026-11-14), and is served by the server once it receives.
            if let Some((Object::Endpoint(ep), r)) = p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                if r & RIGHT_SEND != 0 {
                    on_demand_note(ep);
                }
            }
            match p.lookup(&token).map(|i| (p.cspace[i].object, p.cspace[i].rights)) {
                Some((Object::Console, r)) if r & RIGHT_SEND != 0 => {
                    // The text read where it lies in the process's memory, with no copy on the heap: a console write never allocates, never fails and never waits (2026-11-14).
                    let len = (f.x[2] as usize).min(4096);
                    let msg: &[u8] = match p.view(f.x[1], len) {
                        // SAFETY: `view` resolved all `len` bytes inside this process's arc or one of its DMA regions, identity-mapped and the kernel's to read; the slice lives only for this write, under the kernel lock, while the process's record (its line state) is updated beside it.
                        Some(m) => unsafe { core::slice::from_raw_parts(m.as_ptr(), m.len()) },
                        None => &[],
                    };
                    let o = kk.console.owner(&p.name, p.con);
                    p.con = o;
                    let at = kk.console.pos();
                    // Each line stamped with the kernel's clock in ms, as the flash log's lines are ("t<ms> "), so a timeline across processes reads straight off the console (2026-11-12, measuring attest to the deck).
                    let ms = crate::platform::cntpct() / (crate::platform::cntfrq() / 1000);
                    for line in msg.split_inclusive(|&c| c == b'\n') {
                        if p.line_start {
                            kk.console.put(o, b"t");
                            kk.console.dec(o, ms);
                            kk.console.put(o, b" ");
                            kk.console.put(o, &p.name);
                            kk.console.put(o, b": ");
                            if line[0] != b'\n' {
                                p.words.clear();
                            }
                        }
                        kk.console.put(o, line);
                        for &c in line {
                            // WHY the bound: the line is the process's, any length; the flash log keeps its first WORDS_MAX bytes.
                            if c != b'\n' && p.words.len() < WORDS_MAX {
                                p.words.push(c);
                            }
                        }
                        p.line_start = line[line.len() - 1] == b'\n';
                    }
                    pstore_since(at);
                    // CONSOLE_FLASH in the reply length: the text goes to the flash log too, the line that survives a reset and a decayed DRAM (2026-11-16: a camera open died after its switch with nothing of its later steps kept). For a driver's few lines a session; each costs a flash-log write under this lock (up to 0.66 ms, `diag lock`).
                    if f.x[4] & CONSOLE_FLASH != 0 {
                        // The process is the entry's writer, its words the entry's words.
                        let text = alloc::string::String::from_utf8_lossy(msg);
                        crate::flashlog::line_from(&alloc::string::String::from_utf8_lossy(&p.name), text.trim_end());
                    }
                    let n = 2usize.min((f.x[4] & !(CONSOLE_FLASH | CALL_LIVE)) as usize);
                    p.write(f.x[3], &b"ok"[..n]);
                    f.x[0] = n as u64;
                    0
                }
                // Files on a partition, read only (partfs.rs `part_call`), answered at once.
                // Read with the kernel lock let go (2026-11-18): a 1 MiB chunk of the modem's firmware is UFS commands and an ext4 walk, milliseconds every other core waited out while the modem driver read its 98 MB.
                // The open file is the partition reader's own lock's (`PART`); the request was copied out before and the reply goes into the caller's memory only once the kernel lock is back (`unlocked`).
                Some((Object::Part(i), r)) if r & RIGHT_SEND != 0 => {
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    let cap = (f.x[4] & !CALL_LIVE) as usize;
                    let reply = unlocked(|| PART.with(|| crate::partfs::part_call(PARTS[i], &msg, cap)));
                    let n = reply.len().min(cap);
                    // The caller found again: the process table may have moved meanwhile.
                    if let Some(p) = proc_of(cur) {
                        p.write(f.x[3], &reply[..n]);
                    }
                    f.x[0] = n as u64;
                    0
                }
                // CALL_LIVE (bit 63 of the reply length) and no server receiving on the endpoint — none holds the right, or the one that does has not started receiving yet: ERR now, not a wait for a server that may never come (the IP process at a boot where the WiFi driver failed, 2026-10-29; and while the driver brings its device up).
                // Not for an endpoint whose server the kernel starts on demand (`ON_DEMAND`, 2026-11-14 for the camera): its server is coming, so the call waits for it as a plain one does, and is answered ERR if it cannot be started (`on_demand_refuse_locked`). Lumis's first call on `camera` found the driver not yet started and said so.
                Some((Object::Endpoint(ep), r)) if r & RIGHT_SEND != 0 && f.x[4] & CALL_LIVE != 0 && !has_receiver(ep) && kk.endpoints[ep].on_demand == 0 => {
                    f.x[0] = ERR;
                    0
                }
                // The GPU service neither running nor coming (`gpu_absent`): ERR now, a plain call too, never a wait for a service nobody will start.
                Some((Object::Endpoint(ep), r)) if r & RIGHT_SEND != 0 && gpu_absent(ep) => {
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
                    // The GPU service asks for its next request only once its last job is over (done, or its group ended): what an ended client's job could still have written is free to give out again.
                    if kk.endpoints[ep].name == b"gpu" {
                        gpu_drained();
                    }
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
                        deliver(cur, ep, true);
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
            // A hardware grant: offset (x1) must be 4-byte aligned and inside the grant; the access goes straight to the register, or thru the EL3 monitor for blocks only it can reach (the PMU). Here only when `mmio_fast` could not answer it.
            MMIO_PATHS[1].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
                        // A PCIe link's window noted as on the fast path (pcie_guard.rs): here the lock is held too, so an access never answered would hold it until the service loop ends it.
                        let link = crate::pcie_guard::begin(core(), hw.base + off);
                        let v = unsafe { core::ptr::read_volatile((hw.base + off) as *const u32) as u64 };
                        if link {
                            crate::pcie_guard::end(core());
                        }
                        v
                    } else {
                        if hw.via_el3 {
                            crate::platform::priv_reg((hw.base + off) as u64, true, f.x[2] as u32);
                        } else {
                            let link = crate::pcie_guard::begin(core(), hw.base + off);
                            unsafe { core::ptr::write_volatile((hw.base + off) as *mut u32, f.x[2] as u32) };
                            if link {
                                crate::pcie_guard::end(core());
                            }
                        }
                        note_write(hw.base + off);
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
                // x1 = 1: the region's size in bytes instead (2026-11-11: the GPU service maps the layer arena by it, which grew from 64 to 128 MiB; a kernel from before answers the base to every call, which a caller tells by its being far past any size).
                Some(Object::Dma(i)) if region(i).1 > 0 && f.x[1] == 1 => region(i).1 as u64,
                // x1 = 2 (2026-11-13): the caller's GPU block grown to 2^x2 bytes, answered its size after (`block_ask`); ERR, said on the console, when it is not the caller's block, past the ceiling, or more than the pool can give.
                Some(Object::Dma(i)) if DMA[i].name == "gpu-block" && f.x[1] == 2 => block_ask(p, cur, i, f.x[2]),
                // x1 = 10 (2026-11-20, docs/GPU-KISTS.md "Layers"): on a surface the caller holds, another surface of its own, its token written at arc offset x2 (`surface_ask`).
                Some(Object::Dma(i)) if DMA[i].name == "surface" && f.x[1] == 10 => surface_ask(p, f.x[2]),
                // x1 = 6 (2026-11-18, docs/PUCKS.md): the caller's puck square resized to hold x2 bytes past its porch (`puck_ask`), answered the bytes it holds after; ERR, said on the console, when it is not the caller's, past the slot, or more than the pool can give.
                Some(Object::Dma(i)) if DMA[i].name == "puck" && f.x[1] == 6 => puck_ask(p, i, f.x[2]),
                // x1 = 3 (2026-11-14): the camera driver's run of x2 frames side by side (`cam_ask`; x3 bit 0: under 4 GiB), answered its physical start, which its job's addresses are moved to; the region's base (x1 = 0) is where the driver reads it.
                Some(Object::Dma(i)) if DMA[i].name == "camera" && f.x[1] == 3 => cam_ask(p, cur, f.x[2], f.x[3] & 1 == 1),
                // x1 = 4 (2026-11-15, docs/CAMERA.md "The view"): the driver gives the caller of the OPEN it is answering (its reply token at arc offset x2) a read-only view of x3's frames of its run (low half the first, high half how many), the holder's token written at arc offset x4; answered the view's base (`cam_view_give`). x1 = 5: the view taken back, at the driver's close (`cam_view_drop`) → 0.
                // x3 bit 63 (2026-11-18, docs/CAMERA.md "A cacheable view"): the view mapped write-back (a kernel from before refuses the bit as frames past the run, and the driver asks again without it).
                Some(Object::Dma(i)) if DMA[i].name == "camera" && f.x[1] == 4 => cam_view_give(p, f.x[2], f.x[3] & 0xFFFF_FFFF, (f.x[3] >> 32) & 0x7FFF_FFFF, f.x[4], f.x[3] >> 63 == 1),
                // x1 = 6 (2026-11-18): bytes x2 to x2 + x3 of the driver's run (on `camera`) or of the view (on `camera-view`, its holder) cleaned and invalidated, with the lock let go; answered the ticks it took (`cam_lines`).
                Some(Object::Dma(i)) if (DMA[i].name == "camera" || DMA[i].name == "camera-view") && f.x[1] == 6 => cam_lines(p.epoch, DMA[i].name == "camera-view", f.x[2], f.x[3]),
                Some(Object::Dma(i)) if DMA[i].name == "camera" && f.x[1] == 5 && unsafe { (*(&raw const CAM_VIEW)).driver } == p.epoch => {
                    cam_view_drop("the driver closed");
                    0
                }
                // x1 = 7 (2026-11-20, docs/GPU-KISTS.md "The queue"): the GPU service holds epoch x2's GPU work (x3 = 1; x4 its queue's `SUBMIT` word, 0 none), or lets it go (x3 = 0) once that client's group has ended (`gpu_hold`). x1 = 8: whether epoch x2 is alive (`gpu_alive`).
                Some(Object::Dma(i)) if DMA[i].name == "arena" && f.x[1] == 7 => gpu_hold(f.x[2], f.x[3] == 1, f.x[4]),
                Some(Object::Dma(i)) if DMA[i].name == "arena" && f.x[1] == 8 => gpu_alive(f.x[2]),
                // x1 = 9 (2026-11-20, docs/GPU-KISTS.md "The camera ring on the GPU"): x2 0, the epoch whose GPU space may still map a camera view taken back; x2 that epoch, the service has stopped its jobs and unmapped the view (`gpu_cam_settled`).
                Some(Object::Dma(i)) if DMA[i].name == "arena" && f.x[1] == 9 => gpu_cam_settled(f.x[2]),
                // x1 = 10 (2026-11-21, GPU-COMPOSE.md 2.2): the service's installed tables for the display came from its stamp x2 and map the surface slots in x3 (`dv_settled`).
                Some(Object::Dma(i)) if DMA[i].name == "arena" && f.x[1] == 10 => dv_settled(f.x[2], f.x[3]),
                // A mailbox's region (2026-11-19): its frame taken and mapped at the first ask, the writer's region making the caller its writer (`mbox_dma`).
                Some(Object::Dma(i)) if f.x[1] == 0 && mbox_of(i).is_some() => mbox_dma(p, i),
                // A surface is answered past its porch (cachemap SURFACE_PORCH); every other region at its base.
                Some(Object::Dma(i)) if region(i).1 > 0 => (region(i).0 + if DMA[i].name == "surface" || DMA[i].name == "puck" { crate::cachemap::SURFACE_PORCH } else { 0 }) as u64,
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
                    // One transfer, and a forwarded store reply's 8-byte value ahead of it (`store_forward`); a plain caller still gets at most what it asked for. WHY the min: the length is the server's own.
                    let len = (f.x[2] as usize).min(STORE_MAX + 8);
                    let mut handed = false;
                    if caller == KERNEL_CALLER {
                        // The kernel's own call (`vault_call`): kept for the service loop; the server runs on. A get's reply up to STORE_GET_MAX (a kist's capsule, `kernel_get_max`); every other call of the kernel's asks no more than STORE_MAX. WHY the min: the length is the server's own.
                        kernel_answered(Ok(p.read(f.x[1], (f.x[2] as usize).min(STORE_GET_MAX + 8))));
                    } else if let (Some(ct), Some(cp)) = (th(caller), proc_of(caller)) {
                        if let State::Calling(roff, rcap) = ct.state {
                            if rcap & STORE_REPLY != 0 {
                                // A forwarded store call (`store_forward`): the vault's reply is the syscall's value, then the bytes for the caller's buffer, as many as it asked for (a get up to STORE_GET_MAX), copied from the vault process's buffer straight into the caller's: until 2026-11-11 each quarter MiB went thru a zeroed heap vector first, three passes over every byte of a 24 MB read under the kernel lock. WHY the checks: the reply is the vault process's, its length its own; shorter than the value it carries nothing, and the caller is refused.
                                let len = (f.x[2] as usize).min(STORE_GET_MAX + 8);
                                let t0 = crate::platform::cntpct();
                                let mut n = 0;
                                match p.view(f.x[1], len) {
                                    Some(msg) if msg.len() >= 8 => {
                                        n = (msg.len() - 8).min((rcap & !STORE_REPLY) as usize);
                                        cp.write(roff, &msg[8..8 + n]);
                                        ct.frame.x[0] = u64::from_le_bytes(msg[..8].try_into().unwrap());
                                    }
                                    _ => ct.frame.x[0] = ERR,
                                }
                                use core::sync::atomic::Ordering::Relaxed;
                                let waited = t0 - ct.called;
                                STORE_GETS[0].fetch_add(1, Relaxed);
                                STORE_GETS[1].fetch_add(n as u64, Relaxed);
                                STORE_GETS[2].fetch_add(waited, Relaxed);
                                if STORE_GETS[3].fetch_max(waited, Relaxed) < waited {
                                    STORE_LONGEST[0].store(pin_key(&cp.name), Relaxed);
                                    STORE_LONGEST[1].store(ct.what, Relaxed);
                                }
                                STORE_BY_CLOCK[crate::service::clock_state() as usize & 3].fetch_add(1, Relaxed);
                                STORE_GETS[4].fetch_add(crate::platform::cntpct() - t0, Relaxed);
                            } else if (f.x[2] as usize) > len {
                                // A reply past a store transfer (2026-11-14: the camera's preview frame, 6 MiB at bin 2; docs/CAMERA.md "The endpoint"): copied from the server's memory straight into the caller's, never thru the kernel's heap, as the forwarded store replies are, up to `REPLY_MAX` and what the caller asked for. WHY the min: both lengths are the processes' own.
                                let n = (f.x[2] as usize).min(REPLY_MAX).min(rcap as usize);
                                match p.view(f.x[1], n) {
                                    Some(msg) => {
                                        cp.write(roff, msg);
                                        ct.frame.x[0] = n as u64;
                                    }
                                    None => ct.frame.x[0] = ERR,
                                }
                            } else {
                                let msg = p.read(f.x[1], len);
                                let n = msg.len().min(rcap as usize);
                                cp.write(roff, &msg[..n]);
                                ct.frame.x[0] = n as u64;
                            }
                            ct.state = State::Runnable;
                            kk.ipc_round_trips += 1;
                            tr(TR_REPLY, caller, cur as u64);
                            handed = true;
                        }
                    }
                    f.x[0] = 0;
                    if handed {
                        // The caller runs now, here: it waited (KERNEL.md "Threads, interrupts and cores": direct hand-off both ways). The server goes on after it, from the front while it has front credit (`enqueue_woken`): until 2026-11-11 it went to the back, and a polling driver waited a slice-long round of the queue (~9.7 ms with Mahere's threads busy) to get back to its loop after every reply. Queued first, so the caller lands ahead of it.
                        t.state = State::Runnable;
                        t.frame = *f;
                        enqueue_woken(cur, WAKE_AFTER_REPLY);
                        handoff(caller, WAKE_REPLY);
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
    // A hold on this thread's process (ARCS.md step 3): it does not go back to EL0 but parks runnable (picked again once the hold ends), and the last of the process's threads off a core does the held work (`hold_try`), here when that is this one.
    let r = if kk.procs.get(pi).and_then(|x| x.as_ref()).is_some_and(|p| p.hold.is_some()) {
        if r == 0 && cur_get() == Some(cur) {
            if let Some(t) = th(cur) {
                t.frame = *f;
                t.state = State::Runnable;
            }
            enqueue_back(cur);
            cur_set(None);
            hold_try(pi);
            if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
        } else {
            hold_try(pi);
            r
        }
    } else {
        r
    };
    // Leaving: the number, the process the trap entered from (bits 8-15), the process it returns into or FF when it leaves EL0 (bits 16-23).
    crate::platform::crumb(0x70 + nr.min(SYS_LAST) | (cur as u64) << 8 | cur_get().map(|c| c as u64).unwrap_or(0xFF) << 16);
    // No `deadlines_sync` here (2026-11-11): it walked every thread, under the lock, at the end of every syscall on every core (a hundred threads with Mahere up). A deadline is added only by `sleep`, `irq_wait` and `wait`, which lower NEXT_WAKE to it on the spot; one that goes away (a wake before it) leaves NEXT_WAKE early, which costs an idle core one look, and that look (`wake_sleepers`) makes it exact again. Never late, so no deadline is missed.
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
/// Bit 62 of a console write's reply length: the line goes to the flash log as well (ferros_sys `console_flash`).
pub const CONSOLE_FLASH: u64 = 1 << 62;

/// Whether the process that last received on endpoint `ep` is alive (not ending).
fn has_receiver(ep: usize) -> bool {
    let s = k().endpoints[ep].server;
    s != 0 && k().procs.iter().flatten().any(|p| !p.dying && p.epoch == s)
}

/// Whether `name` is up (glass.rs `boot_watch`): an endpoint of that name has a live process that has received on it (the vault, the display); with no such endpoint (usb serves none: it hands PT to the kernel), a live process of that name.
/// Under the kernel lock.
pub fn up_locked(name: &[u8]) -> bool {
    let kk = k();
    match kk.endpoints.iter().position(|e| e.name == name) {
        Some(ep) => has_receiver(ep),
        None => kk.procs.iter().flatten().any(|p| p.name == name && !p.dying),
    }
}

/// After a process ends: each CALL_LIVE caller still queued on an endpoint nobody can serve any more gets ERR.
fn fail_orphaned_calls() {
    let kk = k();
    for ep in 0..kk.endpoints.len() {
        // The vault, alive and still opening, has not received yet, or ended having served and is being started again (`VAULT_ON`): the store calls queued for it wait.
        if has_receiver(ep) || vault_ep() == Some(ep) || (vault_state() == VAULT_ON && k().endpoints[ep].name == VAULT_EP) {
            continue;
        }
        // No server left: one started again must receive before CALL_LIVE counts it.
        kk.endpoints[ep].server = 0;
        if kk.endpoints[ep].queue.is_empty() {
            continue;
        }
        // The GPU service ended and is not started again (killed, or its restarts stopped: `crash_answer` marks one it restarts before this runs): every call queued on it, plain ones too, gets ERR (`gpu_absent`).
        if gpu_absent(ep) {
            on_demand_refuse_locked(GPU_EP);
            continue;
        }
        // An on-demand server that ended with calls still queued (one came between its last look and its end): another is started for them.
        on_demand_note(ep);
        let queued: Vec<usize> = kk.endpoints[ep].queue.iter().map(|q| q.0).collect();
        for caller in queued {
            if caller == KERNEL_CALLER {
                // The kernel's own call, queued for a vault that ended: its stamp (domain keys) wiped, the service loop told.
                for q in k().endpoints[ep].queue.iter_mut().filter(|q| q.0 == KERNEL_CALLER) {
                    crate::keys::wipe_to_dram(&mut q.1);
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

/// One right in use: the process holding it, by its epoch (the kernel's own name for it, never one the process chose), and for a kist the capsule it was launched from (the person's list names a kist by it, so the permissions page can say which is using what).
struct Claim {
    epoch: u64,
    right: u8,
    capsule: Option<[u8; 32]>,
}
/// The claims held now (`SYS_CLAIM`). A claim lasts until its process releases it or ends: both end paths drop it (`claims_orphaned`), so a kist that dies, is killed or is relaunched without the right holds nothing. Per right, so another right can say "using now" the same way once something marks its use; only GPS is claimed today. Under the kernel lock.
static mut CLAIMS: Vec<Claim> = Vec::new();
/// GPS's claims as the service loop and the pages see them (SYS_FACTS 85): bits 0-15 how many, bits 16 and up a count of every change, so a reader knows the set moved without asking for it.
pub static GPS_CLAIMS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Whether right `r` (ferros_kist::RIGHTS) is GPS.
fn right_is_gps(r: u8) -> bool {
    ferros_kist::RIGHTS.get(r as usize) == Some(&"gps")
}

/// The bells' sources (SYS_BELL): 0-5 the links in the facts' order, then the facts a settings page shows, then GPS's claims (SYS_CLAIM op 4, for a Settings holder; its word is GPS_CLAIMS itself, which carries its own change count), then the kists (`BELL_KISTS`, SYS_NOT_STARTED op 1, for a Settings holder: a launch that did not start, a kist starting or ending), then the windows' owners (`BELL_SURFACES`, for the holder of the `surfaces` region; its word is `SURFACES_LIVE`, the generations with their own change count).
pub const BELL_FACTS: usize = 6;
const BELL_CLAIMS: usize = 7;
const BELL_KISTS: usize = 8;
const BELL_SURFACES: usize = 9;
/// The offers' (SYS_OFFER op 0, for the Stow holder): a host offered a kist to stow.
const BELL_OFFERS: usize = 10;
/// The notes' (SYS_NOTIFY op 4, for a Settings holder): a note posted, revised, withdrawn, seen, past its deadline, or the person's loaded or gone (notify.rs).
const BELL_NOTES: usize = 11;
/// The surface slots' (2026-11-12), for the holder of the `surfaces` region: its word is `SLOTS_LIVE`, four bits a slot, no count. `BELL_SURFACES` stays in its old form (slots 0 to 3) for a display from before.
const BELL_SLOTS: usize = 12;
/// The identity's lock (2026-11-17; owner: "once we lock it there's ZERO way someone can get into those keys"): its word is `LOCK_EPOCH`, moved at every lock of the identity (kvault `lock`: the lock op, the person leaving, an unlock that locks what was open first, a release), rung by `identity_bell`.
/// The identity service wipes the roots, the person key and the fleet key it holds when it moves (payloads/fgtw `locked_out`).
/// No capability: whether a person is present is fact 83, anyone's to read, and the epoch says no more than when that changed.
pub const BELL_IDENTITY: usize = 13;
/// The pucks' squares' (2026-11-18, docs/PUCKS.md), for the holder of the `surfaces` region (the display): its word is `PUCKS_LIVE`, four bits a slot, no count.
const BELL_PUCKS: usize = 14;
/// The camera on the GPU (2026-11-20, docs/GPU-KISTS.md "The camera ring on the GPU"), for the GPU service (the holder of `arena`): counted each time a camera view its holder's GPU calls carried is taken back, so the service unmaps it from that client's address space and says so (`gpu_cam_settled`).
const BELL_GPU_CAMERA: usize = 15;
/// The identity's state (SYS_UNLOCK op 11, for an Unlock holder; 2026-11-16, owner: "Yuppers" to the identity doorbell): its word is `ID_WORD`, the state with its own change count. Beside `BELL_IDENTITY` (the lock epoch, anyone's by SYS_BELL), which came to forge first with number 13 (merged 2026-11-20).
const BELL_ID_STATE: usize = 16;
/// The app in front (2026-11-18, `stop_front`, WINDOWS.md "Stop"), for the holder of the `surfaces` region (the display): not a bell the kernel rings but a word the display keeps and the kernel reads, the app window in front as the display last composed it, its surface slot in bits 0-7, its generation in 8-15, bit 16 set (0 none). Read at power + volume down, so the stop needs nothing of the display at that moment.
const BELL_FRONT: usize = 17;
const BELLS: usize = 18;
/// The lock epoch last rung into the identity's bell (`identity_bell`).
static IDENTITY_RUNG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The identity's bell word as last rung (`identity_ring`): bits 0 to 7 SYS_UNLOCK op 0's answer (bit 0 an identity unlocked, bit 1 the device bound, bit 2 the person key held), the change count above bit 8.
static ID_WORD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// `ID_WORD`'s state bits.
const ID_STATE: u64 = (1 << 8) - 1;

/// The identity's state is `state` (SYS_UNLOCK op 0's answer) after an unlock, a lock, a bind, a release or the person key handed in (service.rs `store_unlock`, `store_tag`), or as a waiter registers (op 11).
/// When it differs from the state last rung, the count moves on by one and every identity bell rings (`bell_ring`): a process asleep on the word (init's `wait_person`) wakes and reads it, never asking on a timer.
/// Under the kernel lock.
pub fn identity_ring(state: u64) {
    let was = ID_WORD.load(core::sync::atomic::Ordering::Acquire);
    if was & ID_STATE == state & ID_STATE {
        return;
    }
    let v = ((was >> 8) + 1) << 8 | state & ID_STATE;
    ID_WORD.store(v, core::sync::atomic::Ordering::Release);
    bell_ring(BELL_ID_STATE, v);
}
/// Each source's change count (the claims' is GPS_CLAIMS'), so a word registered late starts from the count now and a reader tells a change by inequality.
static BELL_COUNT: [core::sync::atomic::AtomicU64; BELLS] = [const { core::sync::atomic::AtomicU64::new(0) }; BELLS];
/// The bells' words (SYS_BELL, SYS_CLAIM op 4, SYS_NOT_STARTED op 1): (the process's slot, its epoch, the word's address in its arc, the source). At most one a source a process. Under the kernel lock.
static mut BELL_WORDS: Vec<(usize, u64, usize, usize)> = Vec::new();

/// SYS_ARC (ARCS.md step 2): the caller's arc grown, shrunk or read; x0 and x1 as the call's doc says, the reason in x1 and on the console when refused. Under the kernel lock, in the caller's trap: `f` is its live frame. Returns the trap's result: a masked arc's grow or shrink with another of its threads at EL0 elsewhere holds the process (`hold`, step 3) and the caller waits for it (`State::Held`).
fn arc_call(f: &mut Frame, cur: usize, pi: usize) -> u64 {
    // A grow's frames made clean first with the kernel lock let go (`frames::prepare`, 2026-11-22, kernlock), when the pool has fewer known clean than it asks: the grow under the lock then zeroes none (about 90 us a frame, a 448 MiB growth 20 ms), and a camera streaming meanwhile never waits behind it.
    if f.x[0] == ARC_GROW {
        let Some(p) = k().procs.get(pi).and_then(|x| x.as_ref()) else { return 0 };
        // WHY the min: n is the process's; a grow past its ceiling is refused (`arc_op`), so frames made clean past it would be zeroed for nothing. The ceiling is never under the size (`arc_alloc_room`, `arc_op`), so the subtraction cannot wrap.
        let want = (f.x[1].min((p.arc.ceiling - p.arc.size) as u64) as usize).div_ceil(crate::frames::FRAME);
        let epoch = p.epoch;
        if want > crate::frames::clean_free() {
            unlocked(|| crate::frames::prepare(want));
            // Ended on another core meanwhile: its threads are drained at their traps, nothing to grow.
            if !proc_of(cur).is_some_and(|q| q.epoch == epoch && !q.dying) {
                return 0;
            }
        }
    }
    let kk = k();
    let Some(p) = kk.procs.get(pi).and_then(|x| x.as_ref()) else { return 0 };
    let (op, n) = (f.x[0], f.x[1]);
    // A masked arc's mask (ring) or limit (exact) changes in every thread at once: one of its threads at EL0 on another core now would go on with the old one. It is held until they have all left; a native thread never reads x27 after its start, so a native arc changes under its running threads.
    let elsewhere = kk.threads.iter().enumerate().any(|(i, x)| i != cur && matches!(x, Some(x) if x.proc == pi) && on_a_core(i));
    if p.arc.guard != 0 && elsewhere && (op == ARC_GROW || op == ARC_SHRINK) {
        if let Some(t) = th(cur) {
            t.frame = *f;
            t.state = State::Held;
        }
        cur_set(None);
        hold(pi, Some((cur, op, n)), false);
        return if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 };
    }
    // Frames given back inside the arc (step 4): no mask or limit changes, so no hold. x0 the bytes given back, x1 the arc's top.
    if op == ARC_RELEASE {
        let (off, len) = (f.x[1] as usize, f.x[2] as usize);
        let p = kk.procs.get_mut(pi).and_then(|x| x.as_mut()).unwrap();
        let top = if p.arc.limit != 0 { p.arc.limit } else { p.arc.size };
        // WHY: the range is the process's; an end that wraps or passes the arc names nothing to give back past it.
        let end = match off.checked_add(len) {
            Some(e) if e <= top => e,
            _ => top,
        };
        let r = arc_release(pi, p, off, end).map(|n| (n as u64, top as u64));
        if let Ok((bytes, _)) = r {
            let line = alloc::format!("{}: arc gave back A#{} MiB inside it ({} frames free)\n", core::str::from_utf8(&p.name).unwrap_or("?"), bytes >> 20, crate::frames::free());
            kk.console.extend_from_slice(line.as_bytes());
        }
        match r {
            Ok((bytes, top)) => {
                f.x[0] = bytes;
                f.x[1] = top;
            }
            Err(why) => arc_answer(pi, op, n, Err(why), f),
        }
        return 0;
    }
    // A page of its own made kernel-only, or made its own again (2026-11-14: Photon's alias thread ran its 256 KiB heap stack 110 KiB past its end, over whatever lay below, and a thread died later on a return address it had never written). No mask or limit changes, so no hold.
    if op == ARC_GUARD || op == ARC_UNGUARD {
        let off = f.x[1] as usize;
        let p = kk.procs.get_mut(pi).and_then(|x| x.as_mut()).unwrap();
        let top = if p.arc.limit != 0 { p.arc.limit } else { p.arc.size };
        let held = p.guards.iter().position(|&(g, _)| g == off);
        // WHY: the offset is the process's; one off a page boundary or with its page past the arc's top names no page of it (top - off cannot wrap once off < top).
        let page = off % 4096 == 0 && off < top && top - off >= 4096;
        let ok = match (op == ARC_GUARD, held) {
            (true, None) if page => crate::cachemap::page_guard(p.arc.base + off).map(|attrs| p.guards.push((off, attrs))).is_some(),
            (false, Some(i)) => crate::cachemap::page_unguard(p.arc.base + off, p.guards[i].1) && {
                p.guards.swap_remove(i);
                true
            },
            _ => false,
        };
        if !ok {
            arc_answer(pi, op, n, Err(ARC_WHY_PAGE), f);
            return 0;
        }
        f.x[0] = 0;
        f.x[1] = top as u64;
        return 0;
    }
    let r = arc_op(pi, op, n);
    // The caller's own x27 is live in `f` (`arc_op` set every thread off a core).
    if let (Ok(_), Some(p)) = (&r, kk.procs.get(pi).and_then(|x| x.as_ref())) {
        f.x[27] = p.arc.x27() as u64;
    }
    arc_answer(pi, op, n, r, f);
    0
}

/// SYS_ARC's answer into a frame (the caller's live one, or a held caller's saved one), and its console line: each grow and shrink, each refusal with its reason.
fn arc_answer(pi: usize, op: u64, n: u64, r: Result<(u64, u64), u64>, f: &mut Frame) {
    let kk = k();
    let Some(p) = kk.procs.get_mut(pi).and_then(|x| x.as_mut()) else { return };
    let name = core::str::from_utf8(&p.name).unwrap_or("?");
    match r {
        Ok((top, was)) => {
            // A grow or a shrink that changed the arc; ARC_SIZE answers (top, ceiling) and is no change at all (it was logged as a shrink from the ceiling until 2026-11-11: "Mahere: arc shrank G#2B400000 to G#3800000" on the Pro, ferros_rt asking the size before its first grow).
            if (op == ARC_GROW || op == ARC_SHRINK) && top != was {
                if op == ARC_GROW { p.grows += 1 } else { p.shrinks += 1 }
                let line = alloc::format!("{name}: arc {} G#{was:X} to G#{top:X} ({} frames free)\n", if op == ARC_GROW { "grew" } else { "shrank" }, crate::frames::free());
                kk.console.extend_from_slice(line.as_bytes());
            }
            f.x[0] = top;
            f.x[1] = was;
        }
        Err(why) => {
            let reason = ["", "past its ceiling", "the pool has too few free frames", "another of its threads runs on another core", "under what it was spawned with", "a word the kernel holds lies past the new end", "no such op", "not a page it may guard or unguard"][why as usize];
            let line = alloc::format!("{name}: arc op {op} of G#{n:X} refused: {reason}\n");
            kk.console.extend_from_slice(line.as_bytes());
            f.x[0] = ERR;
            f.x[1] = why;
        }
    }
}

/// SYS_ARC's work on process `pi`'s arc, every thread of it that is off a core given the new x27 (the caller's live frame is the caller's to set): Ok((the new top, the old)), or Err(the reason). Under the kernel lock.
fn arc_op(pi: usize, op: u64, n: u64) -> Result<(u64, u64), u64> {
    use crate::frames::FRAME;
    let kk = k();
    let Some(p) = kk.procs.get_mut(pi).and_then(|x| x.as_mut()) else { return Err(ARC_WHY_OP) };
    let (masked, exact) = (p.arc.guard != 0, p.arc.limit != 0);
    // The top a mode reaches: exact mode's limit, the arc's size otherwise.
    let old = if exact { p.arc.limit } else { p.arc.size };
    if op == ARC_SIZE {
        return Ok((old as u64, p.arc.ceiling as u64));
    }
    if op != ARC_GROW && op != ARC_SHRINK {
        return Err(ARC_WHY_OP);
    }
    let n = n as usize;
    // WHY the bound: n is the process's; a grow past the 64 GiB no window reaches is refused before the sum could wrap.
    if op == ARC_GROW && n > 1 << 36 {
        return Err(ARC_WHY_CEILING);
    }
    let want = if op == ARC_GROW { old + n } else { n };
    // The new top in the mode's shape: whole frames native, a power of two ring (its mask), 16 bytes exact (its stack pointer's alignment).
    let top = if !masked { want.next_multiple_of(FRAME) } else if exact { want.next_multiple_of(16) } else { arc_shape(want, true) };
    if (op == ARC_GROW && top <= old) || (op == ARC_SHRINK && top >= old) {
        return Ok((old as u64, old as u64));
    }
    if top > p.arc.ceiling {
        return Err(ARC_WHY_CEILING);
    }
    if top < p.floor {
        return Err(ARC_WHY_FLOOR);
    }
    // Exact mode maps whole pages up to its limit (a grow inside what it maps already maps nothing); the others are their top.
    let size = if !exact { top } else if op == ARC_GROW && top <= p.arc.size { p.arc.size } else { top.next_multiple_of(1 << 12) };
    if size < p.arc.size && held_in(pi, p.epoch, p.arc.base + size, usize::MAX) {
        return Err(ARC_WHY_HELD);
    }
    // A grow over frames given back (exact mode's limit coming back over a released range) maps them again first.
    if op == ARC_GROW {
        arc_refill(p, old, top)?;
    }
    arc_resize(&mut p.arc, size)?;
    if exact {
        p.arc.limit = top;
    }
    // A shrink takes the holes past the new end with it.
    let mapped = p.arc.mapped;
    p.holes.retain(|&(a, _)| a < mapped);
    for h in p.holes.iter_mut() {
        // WHY: interval arithmetic, the part of the hole the arc still holds.
        if h.1 > mapped {
            h.1 = mapped;
        }
    }
    // And the guards past it: their pages are no longer the process's.
    p.guards.retain(|&(g, _)| g + 4096 <= size);
    let x27 = p.arc.x27() as u64;
    for (i, x) in kk.threads.iter_mut().enumerate() {
        if let Some(x) = x {
            if x.proc == pi && !on_a_core(i) {
                x.frame.x[27] = x27;
            }
        }
    }
    Ok((top as u64, old as u64))
}

// ---------------------------------------------------------------- holds and moves (ARCS.md step 3)

/// What a held process waits for, both at once when both were asked: SYS_ARC from a thread on a masked arc (the thread, the op, the bytes), and a move of its arc to other frames; and when the hold began (the pause, measured).
#[derive(Clone, Copy)]
struct Hold {
    arc: Option<(usize, u64, u64)>,
    mv: bool,
    t0: u64,
}

/// Hold process `pi` off the cores for a SYS_ARC (`arc`) or a move (`mv`) (owner, 2026-11-11: "a move can happen at any moment"): none of its threads is picked to run until it is done, every core running one now is asked to park it (SGI 3, `.Lirq_hold`: as a slice's end), and the last of them to leave does it (`hold_try`), at once when none is on a core. A hold already pending takes the new ask too. Under the kernel lock.
fn hold(pi: usize, arc: Option<(usize, u64, u64)>, mv: bool) {
    let kk = k();
    let Some(p) = kk.procs.get_mut(pi).and_then(|x| x.as_mut()) else { return };
    let h = p.hold.get_or_insert(Hold { arc: None, mv: false, t0: crate::platform::cntpct() });
    // A held SYS_ARC cannot be asked twice (its caller is held), so `arc` fills an empty place.
    if arc.is_some() {
        h.arc = arc;
    }
    h.mv |= mv;
    let me = core();
    for (c, t) in unsafe { *(&raw const CURRENT) }.iter().enumerate() {
        if c != me && t.is_some_and(|t| kk.threads.get(t).and_then(|x| x.as_ref()).is_some_and(|x| x.proc == pi)) {
            // SGI 3 to core c (ICC_SGI1R_EL1: INTID bits 24-27, Aff1 bits 16-23, target list bit 0 = Aff0 0), as `diag core` sends SGI 2.
            unsafe { core::arch::asm!("dsb sy", "msr S3_0_C12_C11_5, {v}", "isb", v = in(reg) ((c as u64) << 16) | (3u64 << 24) | 1) };
        }
    }
    hold_try(pi);
}

/// Process `pi`'s hold done, if none of its threads is on a core any more: the held SYS_ARC answered into its caller's saved frame (the caller runnable again), then the move made; then its threads may run. Called wherever a thread of it leaves a core: the end of its trap, a slice's end or the hold SGI (`ferros_el0_preempt`). Under the kernel lock.
fn hold_try(pi: usize) {
    let kk = k();
    let Some(h) = kk.procs.get(pi).and_then(|x| x.as_ref()).and_then(|p| p.hold) else { return };
    if kk.threads.iter().enumerate().any(|(i, x)| matches!(x, Some(x) if x.proc == pi) && on_a_core(i)) {
        return;
    }
    if let Some((caller, op, n)) = h.arc {
        let r = arc_op(pi, op, n);
        if let Some(t) = th(caller) {
            let mut f = t.frame;
            arc_answer(pi, op, n, r, &mut f);
            let t = th(caller).unwrap();
            t.frame = f;
            t.state = State::Runnable;
            enqueue_back(caller);
        }
    }
    if h.mv {
        arc_move(pi, crate::platform::cntpct().wrapping_sub(h.t0));
    }
    if let Some(p) = kk.procs.get_mut(pi).and_then(|x| x.as_mut()) {
        p.hold = None;
    }
    // Its threads queued meanwhile may run now: every core told.
    kick(0xFFFF);
}

/// Process `pi`'s arc moved to other frames, while none of its threads is on a core: each frame copied by its own address (the kernel's: cachemap frame_block) to a frame `frames::take` gives (the first free in the map's order), the copy's level-3 table written from the old one with every page's permissions kept (W^X, the grants page, BTI's guarded pages, the guards), the window's slot pointed at it, every core's TLB told, the old frames wiped back into the pool. The window, so every address the process holds, x28, x27 and every kernel record of the arc (bells, waits, the stower's word, the sensors' block), stay as they were: nothing is rewritten. DMA regions are not in an arc and never move. `held` is how long the process waited off the cores for it; the copy and the whole pause go on the console. Under the kernel lock.
fn arc_move(pi: usize, held: u64) {
    use crate::frames::FRAME;
    let kk = k();
    let Some(p) = kk.procs.get_mut(pi).and_then(|x| x.as_mut()) else { return };
    let name = core::str::from_utf8(&p.name).unwrap_or("?");
    let slots = p.arc.mapped / FRAME;
    if crate::frames::free() < slots {
        let line = alloc::format!("{name}: arc not moved: the pool has A#{} free frames, the arc A#{slots}\n", crate::frames::free());
        kk.console.extend_from_slice(line.as_bytes());
        return;
    }
    let t0 = crate::platform::cntpct();
    let mut old = Vec::with_capacity(slots);
    for k in 0..slots {
        let va = p.arc.base + k * FRAME;
        let Some(t) = crate::cachemap::slot_table(va) else { continue };
        let from = crate::frames::of_table(t);
        // Counted free above, under the same lock: every take succeeds. Not zeroed first: the copy fills it whole.
        let to = crate::frames::take_for_copy().unwrap();
        unsafe { core::ptr::copy_nonoverlapping(crate::frames::pa(from) as *const u8, crate::frames::pa(to) as *mut u8, FRAME) };
        crate::cachemap::retable(t, crate::frames::table(to), crate::frames::pa(to));
        crate::cachemap::set_slot(va, crate::frames::table(to));
        old.push(from);
    }
    let copied = crate::platform::cntpct().wrapping_sub(t0);
    // Every core forgets the old frames before they are wiped; the code's new copies reach the instruction side.
    crate::cachemap::unmapped(p.arc.base, p.arc.mapped);
    crate::cachemap::sync_exec(p.arc.base, p.arc.size);
    for f in old {
        crate::frames::give(f);
    }
    p.moves += 1;
    let f = crate::platform::cntfrq();
    let total = crate::platform::cntpct().wrapping_sub(t0) + held;
    let line = alloc::format!("{name}: arc moved, A#{} MiB copied in A#{} us, held A#{} us in all (waiting A#{} us for its threads to leave)\n", p.arc.mapped >> 20, copied * 1_000_000 / f, total * 1_000_000 / f, held * 1_000_000 / f);
    crate::platform::pstore(line.as_bytes());
    kk.console.extend_from_slice(line.as_bytes());
}

/// `diag move <name>`: the process's arc moved to other frames, now if none of its threads is on a core, else as the last of them leaves (its console line says when, how long the copy took and how long it was held). False when no process has that name.
pub fn move_named(name: &[u8]) -> bool {
    lock();
    let kk = k();
    let found = kk.procs.iter().position(|p| matches!(p, Some(p) if p.name == name && !p.dying));
    if let Some(pi) = found {
        hold(pi, None, true);
    }
    unlock();
    found.is_some()
}

/// Whether the kernel holds an address in process `pi`'s arc inside [lo, hi) (a bell's word, the sensors' block): a shrink or a release there would leave it pointing at an unmapped slot, the kernel's own fault when it next writes or reads there.
fn held_in(pi: usize, epoch: u64, lo: usize, hi: usize) -> bool {
    let words = unsafe { &*(&raw const BELL_WORDS) };
    let (se, sa) = crate::sensorlink::block();
    words.iter().any(|w| w.0 == pi && w.1 == epoch && w.2 + 8 > lo && w.2 < hi) || (se == epoch && sa + crate::sensorlink::BYTES > lo && sa < hi)
}

/// Give back the frames wholly inside arc offsets [lo, hi) of process `pi` while its arc keeps its size (ARCS.md step 4): unmapped, every core's TLB told, wiped into the pool, the range a hole (`Process::holes`) until a grow over it fills it again. Never the first frame (the grants page, which the kernel writes into, and the code). Returns the bytes given back; Err(ARC_WHY_HELD) when the kernel holds a word there, Err(ARC_WHY_FLOOR) for the first frame. Under the kernel lock.
fn arc_release(pi: usize, p: &mut Process, lo: usize, hi: usize) -> Result<usize, u64> {
    use crate::frames::FRAME;
    // Whole frames inside the range, and inside what is mapped.
    let lo = lo.next_multiple_of(FRAME);
    // WHY the bound: the range is the process's (or the kernel's own sum at spawn); past the frames mapped there is nothing to give back.
    let hi = if hi / FRAME * FRAME > p.arc.mapped { p.arc.mapped } else { hi / FRAME * FRAME };
    if hi <= lo {
        return Ok(0);
    }
    if lo < FRAME {
        return Err(ARC_WHY_FLOOR);
    }
    if held_in(pi, p.epoch, p.arc.base + lo, p.arc.base + hi) {
        return Err(ARC_WHY_HELD);
    }
    let mut gone = Vec::with_capacity((hi - lo) / FRAME);
    let mut at = lo;
    while at < hi {
        if let Some(t) = crate::cachemap::slot_table(p.arc.base + at) {
            gone.push(crate::frames::of_table(t));
            crate::cachemap::set_slot(p.arc.base + at, 0);
        }
        at += FRAME;
    }
    crate::cachemap::unmapped(p.arc.base + lo, hi - lo);
    let n = gone.len() * FRAME;
    for f in gone {
        crate::frames::give(f);
    }
    p.holes.push((lo, hi));
    // A guard in what went is gone with its frame.
    p.guards.retain(|&(g, _)| g < lo || g >= hi);
    Ok(n)
}

/// A grow over process `pi`'s holes inside arc offsets [lo, hi): fresh zeroed frames mapped again where they were given back, and the holes trimmed to what is left outside the range (exact mode's limit can grow back over a range it released). Err(ARC_WHY_MEMORY) with nothing changed when the pool has too few frames. Under the kernel lock.
fn arc_refill(p: &mut Process, lo: usize, hi: usize) -> Result<(), u64> {
    use crate::frames::FRAME;
    let (lo, hi) = (lo / FRAME * FRAME, hi.next_multiple_of(FRAME));
    let mut slots = Vec::new();
    for &(a, b) in &p.holes {
        // WHY the choices here and below: interval arithmetic, each hole's overlap with the range (and what is left of it outside), not a bound on anyone's value.
        let mut at = if a > lo { a } else { lo };
        while at < b && at < hi && at < p.arc.mapped {
            // Only a slot still unmapped (a hole is, but a shrink and a grow since may have mapped it whole).
            if crate::cachemap::slot_table(p.arc.base + at).is_none() {
                slots.push(at);
            }
            at += FRAME;
        }
    }
    if slots.is_empty() {
        return Ok(());
    }
    if crate::frames::free() < slots.len() {
        return Err(ARC_WHY_MEMORY);
    }
    for &at in &slots {
        // Counted free above, under the same lock: every take succeeds.
        let f = crate::frames::take().unwrap();
        // The bytes of this frame that are the process's: all of it but past the arc's size in its last frame.
        let usable = if p.arc.size - at < FRAME { p.arc.size - at } else { FRAME };
        crate::cachemap::map_slot(p.arc.base + at, crate::frames::pa(f), crate::frames::table(f), usable);
    }
    crate::cachemap::mapped();
    // What is left of each hole outside [lo, hi).
    let mut kept = Vec::new();
    for &(a, b) in &p.holes {
        if a < lo {
            kept.push((a, if b < lo { b } else { lo }));
        }
        if b > hi {
            kept.push((if a > hi { a } else { hi }, b));
        }
    }
    p.holes = kept;
    Ok(())
}

/// The ends watched (SYS_CALLER x1): (the watcher's slot, its epoch, the word's address in its arc, the epoch of the process it watches). One a watcher, so the table holds at most one a process. Under the kernel lock.
static mut END_WATCHES: Vec<(usize, u64, usize, u64)> = Vec::new();

/// Process `p` (slot `pi`) watches the end of the process with `watched` (the one behind a call it holds the reply token of): the word at arc offset `at` (8-aligned, inside the arc, as SYS_WAIT takes one) gets bit 63 when that process ends (`end_watch_ring`). A watch it held before is dropped. The word is the process's own and not written now: the server clears the bit itself, after this (a watch moved here can no longer ring), and an end between the two shows as its reply to that call failing. False for a word outside its arc. Under the kernel lock.
fn end_watch(p: &Process, pi: usize, at: u64, watched: u64) -> bool {
    // A word outside the arc: the syscall resolves or traps (`bad_buf`).
    let Some(addr) = p.word(at) else {
        p.bad_buf(at, 8, true);
        return false;
    };
    let w = unsafe { &mut *(&raw mut END_WATCHES) };
    w.retain(|e| e.0 != pi);
    w.push((pi, p.epoch, addr, watched));
    true
}

/// The process with `epoch` ended: every word watching it (`end_watch`) gets bit 63, by an atomic or, and its process's threads waiting on that word are woken (0, as a wake), so a server held in a wait for its own device (a frame 14 years long) hears its caller's end in that same wait. A watcher that has ended since (its slot empty, dying, or a successor's), or a word its arc no longer holds (shrunk, or given back as a hole), is never written. The ended process's own watches go with it. Under the kernel lock.
fn end_watch_ring(epoch: u64) {
    let kk = k();
    let w = unsafe { &mut *(&raw mut END_WATCHES) };
    for &(pi, we, addr, watched) in w.iter() {
        if watched != epoch {
            continue;
        }
        // WHY the checks: the word was resolved at the registration; the watcher may have ended, or shrunk its arc or given that frame back since, and a write there would land in another process's memory or an unmapped slot.
        if !kk.procs.get(pi).and_then(|q| q.as_ref()).is_some_and(|q| q.epoch == we && !q.dying && addr + 8 <= q.arc.base + q.arc.size && !q.in_hole(addr - q.arc.base, 8)) {
            continue;
        }
        // SAFETY: `addr` is an 8-aligned word inside the watcher's live arc (checked just above, under the lock that every arc change takes), identity-mapped Normal memory the kernel reads and writes as `bell_ring` does.
        unsafe { (*(addr as *const core::sync::atomic::AtomicU64)).fetch_or(1 << 63, core::sync::atomic::Ordering::SeqCst) };
        for (t, x) in kk.threads.iter_mut().enumerate() {
            if let Some(x) = x {
                if x.proc == pi && matches!(x.state, State::Waiting(a, _) if a == addr) {
                    x.state = State::Runnable;
                    x.frame.x[0] = 0;
                    enqueue_back(t);
                }
            }
        }
    }
    w.retain(|&(_, we, _, watched)| watched != epoch && we != epoch);
}

/// A word in the caller's arc (`at`, 8-aligned and inside the arc as SYS_WAIT takes one) rung for `source` from now, `value` written there at once; one the process registered for the source before is dropped. Under the kernel lock.
fn bell_register(p: &Process, pi: usize, source: usize, at: u64, value: u64) -> u64 {
    // A word outside the arc: the syscall resolves or traps (`bad_buf`).
    let Some(addr) = p.word(at) else {
        p.bad_buf(at, 8, true);
        return ERR;
    };
    let words = unsafe { &mut *(&raw mut BELL_WORDS) };
    words.retain(|w| !(w.0 == pi && w.3 == source));
    words.push((pi, p.epoch, addr, source));
    unsafe { core::ptr::write_volatile(addr as *mut u64, value) };
    0
}

/// `source` changed: `value` written into every word registered for it and the threads of its process waiting there woken, so a page asleep on it, or a loop that reads it, draws again (owner, 2026-11-10: "Needs a doorbell"). A word whose process has ended (its slot empty, dying, or a successor's) is dropped, never written. Under the kernel lock.
fn bell_ring(source: usize, value: u64) {
    let kk = k();
    let words = unsafe { &mut *(&raw mut BELL_WORDS) };
    words.retain(|&(pi, epoch, _, _)| kk.procs.get(pi).and_then(|q| q.as_ref()).is_some_and(|q| q.epoch == epoch && !q.dying));
    for &(pi, _, addr, s) in words.iter() {
        if s != source {
            continue;
        }
        unsafe { core::ptr::write_volatile(addr as *mut u64, value) };
        for (t, x) in kk.threads.iter_mut().enumerate() {
            if let Some(x) = x {
                if x.proc == pi && matches!(x.state, State::Waiting(a, _) if a == addr) {
                    x.state = State::Runnable;
                    x.frame.x[0] = 0;
                    enqueue_back(t);
                }
            }
        }
    }
}

/// The identity's bell rung when its lock epoch moved since it last rang (`BELL_IDENTITY`): after every SYS_UNLOCK call, tag ops included, the only syscalls that lock.
/// Under the kernel lock.
/// A lock the kernel makes on its own way to a reset (service.rs `lock_before_reset`) rings nothing: nothing runs after it.
fn identity_bell() {
    let e = LOCK_EPOCH.load(core::sync::atomic::Ordering::Acquire);
    if IDENTITY_RUNG.swap(e, core::sync::atomic::Ordering::AcqRel) != e {
        bell_ring(BELL_IDENTITY, e as u64);
    }
}

/// Power + volume down (service.rs, chord.rs; owner, 2026-11-18: "power plus vol down kills", then: Stop ends the app in front immediately, no close message, no grace period): the app in front ended now, one shot dead, as init's OP_STOP ends a kist from the deck's Stop, and no record kept (the person's own act). ferros apps keep every change as it happens, so there is nothing to shut down; the camera driver closes its stream the moment its holder ends.
/// The app in front is the display's, read from the word it registered (`BELL_FRONT`) and keeps current as it composes: nothing of the display is asked now, so a display or touch the app wedged still lets the chord thru.
/// Nothing on the deck with no app in front, and never a system process (one holding Settings that is not a kist: the map, the attest screen). Returns what happened, for the flash log.
pub fn stop_front() -> alloc::string::String {
    with_lock(|| {
        let words = unsafe { &*(&raw const BELL_WORDS) };
        let kk = k();
        // The table never shrinks (procs only grows), so a registered slot is always there to look at.
        let Some(&(_, _, addr, _)) = words.iter().find(|w| w.3 == BELL_FRONT && kk.procs[w.0].as_ref().is_some_and(|q| q.epoch == w.1 && !q.dying)) else {
            return alloc::string::String::from("stop: no display has said what is in front; nothing stopped");
        };
        // SAFETY: the word was checked inside the display's arc at its registration (`Process::word`), and the arc stays mapped while the process lives (checked just above, under the lock that ends it); a shrink or a release over it is refused (`held_in`).
        let front = unsafe { core::ptr::read_volatile(addr as *const u64) };
        if front >> 16 & 1 == 0 {
            return alloc::string::String::from("stop: no app in front; nothing stopped");
        }
        let (slot, generation) = ((front & 0xFF) as usize, (front >> 8 & 0xFF) as u8);
        let slots = unsafe { &*(&raw const SLOTS) };
        // WHY the bound: the slot is the display's word, read as it stands.
        let Some(s) = slots.get(slot).filter(|s| s.generation == generation && s.owner != 0) else {
            return alloc::format!("stop: the window in front (surface {slot}, generation {generation}) has no owner now; nothing stopped");
        };
        let Some(pi) = kk.procs.iter().position(|q| matches!(q, Some(q) if q.epoch == s.owner && !q.dying)) else {
            return alloc::format!("stop: the window in front (surface {slot}) belongs to a process already ending");
        };
        let p = kk.procs[pi].as_ref().unwrap();
        let shown = alloc::string::String::from_utf8_lossy(&p.name).into_owned();
        if !p.kist && caller_holds(p) & CALLER_SETTINGS != 0 {
            return alloc::format!("stop: {shown} is the system's; nothing stopped");
        }
        kill_at(pi);
        alloc::format!("stop: {shown} (surface {slot}) ended")
    })
}

/// `source` (a link, or `BELL_FACTS`) changed: its count moved on by one and rung. Under the kernel lock: from a syscall, or the service loop thru `with_lock` (service.rs `bell_facts`).
pub fn bell_count(source: usize) {
    let v = BELL_COUNT[source].fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
    bell_ring(source, v);
}

/// The claims changed: GPS's count published with its change counted, rung into every claims bell (`bell_ring`), and the service loop woken (`sev`) to start or stop the driver, never on a timer.
fn claims_publish() {
    let n = unsafe { (*(&raw const CLAIMS)).iter().filter(|c| right_is_gps(c.right)).count() } as u64;
    let g = GPS_CLAIMS.load(core::sync::atomic::Ordering::Relaxed) >> 16;
    let v = (g + 1) << 16 | n;
    GPS_CLAIMS.store(v, core::sync::atomic::Ordering::Release);
    bell_ring(BELL_CLAIMS, v);
    unsafe { core::arch::asm!("sev") };
}

/// A process ended (exit, fault, kill): its claims go with it.
fn claims_orphaned(epoch: u64) {
    let c = unsafe { &mut *(&raw mut CLAIMS) };
    let before = c.len();
    c.retain(|x| x.epoch != epoch);
    if c.len() != before {
        claims_publish();
    }
}

/// Every GPS claim dropped (the switch turned off, or nobody present: the service loop, under the lock). → how many there were.
pub fn gps_claims_drop() -> usize {
    let c = unsafe { &mut *(&raw mut CLAIMS) };
    let before = c.len();
    c.retain(|x| !right_is_gps(x.right));
    let n = before - c.len();
    if n != 0 {
        claims_publish();
    }
    n
}

/// GPS's claims now (`GPS_CLAIMS`' count).
pub fn gps_claims() -> u64 {
    GPS_CLAIMS.load(core::sync::atomic::Ordering::Acquire) & 0xFFFF
}

/// A link's measures, tracks and detail emptied as its driver ends (one shot dead, it empties nothing itself): until 2026-11-10 a stopped driver's last words stayed in the facts, and the console drew a stopped GPS's satellites as if it still heard them. Under the kernel lock.
fn link_emptied(name: &[u8]) {
    let Some(i) = LINK_NAMES.iter().position(|n| *n == name) else { return };
    for w in &STATUS[i] {
        w.store(u64::MAX, core::sync::atomic::Ordering::Relaxed);
    }
    for w in &TRACK[i] {
        w.store(u64::MAX, core::sync::atomic::Ordering::Relaxed);
    }
    unsafe { (*(&raw mut DETAIL_LEN))[i] = 0 };
    // The page showing the link draws it emptied (SYS_BELL).
    bell_count(i);
}

/// Bumped whenever a process is made, ends or is killed: what the service loop watches to act on the process table changing (the GPU's clock, service.rs), with no poll.
pub static PROCS_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// The fault a process is ending by, whole, from the EL0 trap's fault branch to `end_process` in the same call: ESR, FAR, and the PC and link register as offsets into its arc, the call chain from the faulting frame (`frames`) and the thread.
/// Under the kernel lock.
static mut LAST_FAULT: Option<Fault> = None;

struct Fault {
    esr: u64,
    far: u64,
    pc: u64,
    lr: u64,
    frames: Vec<u64>,
    thread: Vec<u8>,
}

/// The frames as the console shows them: `+G#…` offsets into the arc, innermost first.
fn frames_text(frames: &[u64]) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for f in frames {
        s.push_str(&alloc::format!(" +G#{f:X}"));
    }
    s
}

/// A process's end, whole, in the flash log (category `kernel.process`, its fields typed: ferros_ledger death), for every end (2026-11-12, the 8a: the vault process died during a stow, and its panic and its exit were on the console alone, which nobody read before it drained).
/// A death by a fault or a panic is also one line on the console, and on the glass when no display serves to show anything else (the display itself the one that died, or never up), written there straight (glass.rs `notice`; owner, 2026-11-17: when things go south, say so on the glass): the message and where for a panic, ESR, FAR and the PC for a fault; the frames on a second console line, for `ferros-bridge deaths` to name.
/// While a display serves, the glass is its own: a line written under it would stay in the phases its compose-once marks say hold only the background.
/// Under the kernel lock (the flash log's entry is written once this core lets it go, flashlog.rs `deferred`); a death is rare, never the hot path.
fn death_line(name: &[u8], value: u64, last: &[u8], panicked: Option<alloc::boxed::Box<Panicked>>) {
    use ferros_ledger::death::{Death, END_EXIT, END_FAULT, END_PANIC};
    let who = core::str::from_utf8(name).unwrap_or("?");
    let text = |b: &[u8]| alloc::string::String::from_utf8_lossy(b).trim_end().into();
    let last_words: alloc::string::String = text(last);
    let fault = unsafe { (*(&raw mut LAST_FAULT)).take() };
    let mut d = Death { end: END_EXIT, exit: Some(value), code: Some(CODE as u64), last_words: (!last_words.is_empty()).then(|| last_words.clone()), ..Death::default() };
    // The line said once, its numbers typed (glass.rs `Note`): the console and the flash log get it in `A#`/`G#`, the glass in the device's base.
    use ferros_glass::{Num, Say};
    let mut note = crate::glass::Note::words(who);
    let chain = if value >> 56 == FAULT >> 56 {
        d.end = END_FAULT;
        match fault {
            Some(fa) => {
                let t: alloc::string::String = text(&fa.thread);
                note.s(": fault, ESR ");
                note.n(Num::Bits(fa.esr));
                note.s(", FAR ");
                note.n(Num::Bits(fa.far));
                note.s(", PC +");
                note.n(Num::Bits(fa.pc));
                note.s(" LR +");
                note.n(Num::Bits(fa.lr));
                note.s(" in its arc, thread ");
                note.s(&t);
                (d.esr, d.far, d.pc, d.lr, d.thread) = (Some(fa.esr), Some(fa.far), Some(fa.pc), Some(fa.lr), Some(t));
                d.frames = fa.frames;
                true
            }
            None => {
                note.s(": fault, exit ");
                note.n(Num::Word(value));
                false
            }
        }
    } else if value == 0xDEAD || value == 101 {
        d.end = END_PANIC;
        match panicked {
            Some(p) => {
                let (m, file, t): (alloc::string::String, alloc::string::String, alloc::string::String) = (text(&p.message), text(&p.file), text(&p.thread));
                // The message is the process's own words (its numbers written by its own formatting); where it is, the kernel's, typed.
                note.s(": panicked at ");
                note.s(&file);
                note.s(":");
                note.n(Num::Line(p.line as u64));
                note.s(":");
                note.n(Num::Line(p.column as u64));
                note.s(": ");
                note.s(&m);
                note.s(" (thread ");
                note.s(&t);
                note.s(")");
                (d.message, d.file, d.line, d.column, d.thread) = (Some(m), Some(file), Some(p.line as u64), Some(p.column as u64), Some(t));
                d.frames = p.frames;
                true
            }
            // A panic handler from before SYS_PANIC (or a kernel's exit with no report): its last line is all there is.
            None => {
                note.s(": panicked, its last line: ");
                note.s(&last_words);
                false
            }
        }
    } else {
        let line = alloc::format!("{who}: ended, exit G#{value:016X}; its last line: {last_words}");
        // The USB process ends after its tries with nothing plugged in and is started again, every 2^4 s: kept once, then counted.
        if name == b"usb" {
            crate::flashlog::death_once(crate::flashlog::once::USB_END, who, &line, d);
        } else {
            crate::flashlog::death(who, &line, d);
        }
        return;
    };
    let line = note.dev();
    let kk = k();
    let at = kk.console.pos();
    kk.console.extend_from_slice(line.as_bytes());
    kk.console.push(b'\n');
    if chain && !d.frames.is_empty() {
        kk.console.extend_from_slice(alloc::format!("{who}: frames{} (ferros-bridge deaths names them)\n", frames_text(&d.frames)).as_bytes());
    }
    pstore_since(at);
    crate::flashlog::death(who, &line, d);
    let display_serving = name != b"display" && up_locked(b"display");
    crate::glass::notice(note, !display_serving);
}

fn end_process(pi: usize, cur: usize, value: u64) {
    PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    // akita bring-up: a process ending (the usb process is the only one there) resets plainly in a FERROS_PROBE=8 build, so it reads apart from the power-up probes 81-85 (board.rs).
    crate::board::probe_log(86);
    let kk = k();
    // Looked at before its threads' calls are dropped below: a job of its on the GPU keeps its blocks and surfaces from the next owner until it is over (`gpu_drain`).
    let gpu_busy = gpu_job_in_flight(pi);
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
        mmio_quiesce(&p.fast);
        // Say how it ended on the console: a process that outlives its first slice (spawn already returned) has no one else to hear its exit value — a fault or a panic would otherwise vanish.
        if !p.line_start {
            kk.console.push(b'\n');
        }
        let name = p.name.clone();
        // A kist ended by a fault or a panic: kept for the map's circle (`kist_stopped`).
        kist_stopped(p, value);
        gpu_ending(p.epoch, gpu_busy);
        surfaces_free(p.epoch);
        regions_free(p.epoch);
        pucks_free(p.epoch);
        crate::irqarm::disarm_epoch(p.epoch);
        cam_free(p.epoch);
        extents_free(p.epoch);
        end_watch_ring(p.epoch);
        crate::sensorlink::ended(p.epoch);
        power_orphaned(p.epoch);
        claims_orphaned(p.epoch);
        link_emptied(&name);
        fail_callers(&p.cspace);
        p.cspace.retain(|c| !matches!(c.object, Object::Reply(_)));
        let at = kk.console.pos();
        kk.console.extend_from_slice(&name);
        kk.console.extend_from_slice(b": exit G#");
        for i in (0..16).rev() {
            kk.console.push(b"0123456789ABCDEF"[((value >> (4 * i)) & 0xF) as usize]);
        }
        kk.console.push(b'\n');
        pstore_since(at);
        // The end in the flash log, whole (`death_line`): its exit value and last line, and a panic's or a fault's report as typed fields. Said under the kernel lock, written once this core lets it go (flashlog.rs `deferred`).
        let last = p.words.clone();
        let panicked = p.panicked.take();
        vault_ended(p, value);
        death_line(&name, value, &last, panicked);
        // A kept process dead by a panic or a fault: started again (`crash_answer`), after its death is said above.
        crash_answer(&name, value);
        if elsewhere {
            kk.procs[pi].as_mut().unwrap().dying = true;
        } else if let Some(p) = kk.procs[pi].take() {
            // Its mailboxes marked dead only now, with no thread of it left to publish over the mark.
            mailboxes_gone(p.epoch);
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
    // The name the thread gave itself (SYS_THREAD_NAME), when it gave one.
    if let Some(t) = kk.threads.get(cur).and_then(|x| x.as_ref()) {
        let n = t.label.iter().position(|&c| c == 0).unwrap_or(16);
        if n != 0 {
            o.extend_from_slice(b" (");
            o.extend_from_slice(&t.label[..n]);
            o.push(b')');
        }
    }
    o.extend_from_slice(b": FAULT esr ");
    hex(&mut o, f.esr);
    o.extend_from_slice(b" elr+");
    hex(&mut o, f.elr.wrapping_sub(base));
    o.extend_from_slice(b" far ");
    hex(&mut o, f.far);
    o.extend_from_slice(b" sp+");
    hex(&mut o, f.sp_el0.wrapping_sub(base));
    o.push(b'\n');
    // A data abort in a page the process guarded itself (ARC_GUARD): the thread's stack ran past its end.
    if (f.esr >> 26) & 0x3F == 0x24 && proc_of(cur).is_some_and(|p| p.guards.iter().any(|&(g, _)| g == f.far.wrapping_sub(base) as usize & !4095)) {
        o.extend_from_slice(&name);
        o.extend_from_slice(b": the fault is in the guard page under a thread's stack: the stack ran past its end\n");
    }
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
    let mut held = None;
    // A yield asked of this core for a device thread (devsched `first`, `make_room`), not the slice's end (the tick handler stops the slice timer): the thread keeps its place at the front, behind the urgent thread `pick_runnable` takes first, rather than waiting a queue's turn for a device's microseconds. Any other preemption (the slice's end, an interrupt taken at EL0, a hold) goes to the back as before: a thread put at the front for every interrupt would take a fresh slice ahead of the queue each time.
    let ctl: u64;
    unsafe { core::arch::asm!("mrs {}, cnthp_ctl_el2", out(reg) ctl) };
    let asked = YIELD[core()].swap(0, core::sync::atomic::Ordering::Relaxed) != 0;
    // Asked by `make_room` (devsched `first` on) or by `make_way` (a light thread woken with every worker busy, 2026-11-17).
    let old = OLD.load(core::sync::atomic::Ordering::Relaxed);
    let keep = asked && ctl & 1 != 0 && (!old || crate::devsched::FIRST.load(core::sync::atomic::Ordering::Relaxed));
    // A device thread in its run for its own interrupt (`on_irq`, inside its 2^10 us budget), parked unasked, by an interrupt taken at EL0 for another line or by the slice's end: it keeps its place just behind the thread that interrupt woke, and is urgent again, so the core takes it back next.
    // Measured 2026-11-18 (slit-robust, the Pro, Lumis in Slitscan): the camera's frame thread, preempted mid-answer on c6 by the sensors' line, went behind Lumis's threads for 4.4 ms with its line masked; once its line followed it, the same thread parked 2 ms between its last two syscalls of an answer (the slice it was handed on a busy core ending: a slice is armed once a `run_slice`, so a thread woken onto a core can inherit one nearly spent). Either way the next frame's descriptor went unreleased and the slit stream died on LATE_CONFIG.
    let irq_run = !asked && !old && cur_get().and_then(|c| th(c).map(|y| (c, y.on_irq))).is_some_and(|(c, on)| on && th(c).is_some_and(|y| !over_budget(c, y)));
    if let Some(cur) = cur_get() {
        if !drain_dying(cur) {
            if let Some(p) = th(cur) {
                p.frame = *f;
                p.state = State::Runnable;
                held = Some(p.proc);
                if keep && old {
                    enqueue_front(cur);
                } else if keep {
                    enqueue_second(cur);
                } else if irq_run {
                    p.urgent = true;
                    let line = p.run_line;
                    PREEMPT_KEPT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    enqueue_second(cur);
                    // And a core for it now if none is idle (`make_room`): behind the thread its core was taken for (another device's, which may run a while), it would otherwise wait for that thread (2026-11-18: the camera's frame thread parked 1.36 ms between two syscalls of one answer with every A715 busy, its line masked meanwhile).
                    if line != 0 && crate::devsched::FIRST.load(core::sync::atomic::Ordering::Relaxed) {
                        make_room(cur, line as usize - 1, core());
                    }
                } else {
                    enqueue_back(cur);
                }
            }
            tr(TR_PREEMPT, cur, 0);
        }
    }
    cur_set(None);
    // A slice's end or the hold SGI (`.Lirq_hold`): if its process is held, this may have been the last of its threads on a core.
    if let Some(pi) = held {
        hold_try(pi);
    }
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
        crate::killswitch::enable_sgi(3);
    }
    let ticks = slice_ticks();
    unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", t = in(reg) ticks, c = out(reg) _) };
}

fn disarm_slice() {
    unsafe { core::arch::asm!("msr cnthp_ctl_el2, xzr", "isb") };
}

/// Give the next runnable process one time slice (called from the service loop). Returns false if nothing was runnable. Its console output collects until `take_console`.
/// A sleeper or a timed waiter at its deadline `d`: queued as any other wake (`enqueue_woken`: to the front while its front credit holds, a busy worker asked to make way when none is idle; at the back always until 2026-11-17, when the map's frame and the display's receive thread waited out rayon's slices), its wait measured from the deadline, so the time to notice it counts.
fn timer_woken(t: usize, d: u64) {
    if OLD.load(core::sync::atomic::Ordering::Relaxed) {
        woken(t, WAKE_TIMER);
        enqueue_back(t);
    } else {
        enqueue_woken(t, WAKE_TIMER);
    }
    if let Some(x) = th(t) {
        x.woke = d;
    }
}

/// Sleepers whose deadline has passed become runnable (called before every slice, and by any trap once the earliest deadline has passed).
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
                    timer_woken(i, d);
                    tr(TR_WAKE, i, ((now - d) * 1000 / crate::platform::cntfrq()).min(65535));
                }
            } else if let State::Waiting(a, d) = p.state {
                // An interrupt the thread waits on has fired: it runs first (the device is waiting on it), while it has front credit (`enqueue_woken`: a driver whose device keeps it busy past a slice waits its turn like any other thread, where before nothing bounded it).
                if a < 1024 && irq_pending_take(a as u32) {
                    p.state = State::Runnable;
                    p.frame.x[0] = 0;
                    irq_stamp(p, a);
                    let (line, landed) = (p.irq_line as usize, p.irq_core as usize);
                    tr(TR_IRQ, i, (landed as u64) << 8 | (a as u64 & 0xFF));
                    let spent = credit(p, now);
                    let front = enqueue_woken(i, WAKE_IRQ);
                    // Woken behind the queue, its front credit spent: counted per line with the credit it had (devsched `spent`, `diag dev`).
                    if !front && line != 0 {
                        crate::devsched::spent(line - 1, spent);
                    }
                    tr(TR_WAKE, i, a as u64);
                    // With front credit left it is urgent (devsched `first`): picked first, and a core made free for it when none is.
                    if front && line != 0 {
                        if let Some(x) = th(i) {
                            x.urgent = true;
                        }
                        if crate::devsched::FIRST.load(core::sync::atomic::Ordering::Relaxed) {
                            make_room(i, line - 1, landed);
                        }
                    }
                    continue;
                }
                // A timed wait whose deadline passed: back with 2.
                if d != 0 && d <= now {
                    p.state = State::Runnable;
                    p.frame.x[0] = 2;
                    timer_woken(i, d);
                    tr(TR_WAKE, i, 0xFFFE);
                }
            }
        }
    }
    frame_mailbox_flush();
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

/// Append the kernel's own text to the process console (kept for `diag console`).
pub fn push_console(bytes: &[u8]) {
    lock();
    k().console.extend_from_slice(bytes);
    crate::platform::pstore(bytes);
    unlock();
    console_flashlog(bytes);
}

/// akita: the RAM log does not survive its resets, so the kernel's console lines and a boot-time spawn's first slice go to the flash log too (in the ferros partition, board.rs).
pub fn console_flashlog(bytes: &[u8]) {
    if crate::board::akita() {
        let text = alloc::string::String::from_utf8_lossy(bytes);
        let text = text.trim_end();
        if !text.is_empty() {
            crate::flashlog::line(text);
        }
    }
}

/// The ring's bytes from position `at` to now into the RAM log (`pstore`), both parts when they wrap. Under the kernel lock.
fn pstore_since(at: u64) {
    let (a, b) = k().console.since(at);
    crate::platform::pstore(a);
    if !b.is_empty() {
        crate::platform::pstore(b);
    }
}

/// Drain the console: a line for each writer whose text was overwritten before this read could have it (`… A#<n> bytes dropped from <name>`), then everything written since the last drain.
/// The ring keeps what was drained too, for `diag "console kept"`: a reader that drains takes the lines from everyone else (2026-11-12: several tools on one device, and a timeline read off the console lost its middle to another's drain).
pub fn take_console() -> Vec<u8> {
    lock();
    let v = k().console.take();
    unlock();
    v
}

/// The console as kept, without draining it: the last `max` bytes the ring holds, drained or not (`diag "console kept [KiB]"`).
pub fn console_kept(max: usize) -> Vec<u8> {
    lock();
    let v = k().console.kept(max);
    unlock();
    v
}

/// End a process by name from outside (the service loop, `diag restart`), whatever it is doing: its arc freed, its calls still queued on any endpoint dropped, and every reply capability pointing at it removed from every other process — so a late reply can never reach whatever process takes its slot next. A server's queued callers stay queued on the endpoint for the next server. Returns whether a process of that name was found. Never call it for the process that is running (only from the kernel's own loop, where none is).
pub fn kill(name: &[u8]) -> bool {
    lock();
    // A kist ended from outside (the bridge's `diag kill`): kept for the map's circle, as a fault is. The person's own relaunch kills by `kill_locked` alone and keeps nothing.
    if let Some(c) = k().procs.iter().flatten().find(|p| p.name == name && p.kist && !p.dying).and_then(|p| p.capsule) {
        kist_record(&c, ferros_kist::launch::why::KILLED);
    }
    let r = kill_locked(name);
    unlock();
    r
}
fn kill_locked(name: &[u8]) -> bool {
    let kk = k();
    // A live process of that name, never one already ending: a process left `dying` (a thread of it still on a core) ahead of its relaunch in the table was found first, killed again, and the live one ran on (as `kill`'s record and `kist_request` already skip one).
    let Some(i) = kk.procs.iter().position(|p| matches!(p, Some(p) if p.name == name && !p.dying)) else { return false };
    kill_at(i);
    true
}

/// End process `i` (alive, not ending) from outside, as `kill_locked` does by name: the stop (`stop_front`) finds its process as its window's owner, by epoch, never by a name. Under the lock.
fn kill_at(i: usize) {
    PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    let kk = k();
    let name = kk.procs[i].as_ref().unwrap().name.clone();
    let name = &name[..];
    // The vault process killed (`diag restart vault` starts its successor at once; a bare `diag kill vault` leaves that to the service loop): the store calls queued for it wait for the next one, which the loop starts if none is alive by then.
    if name == VAULT_EP && vault_state() == VAULT_ON {
        VAULT_WANTED.store(true, core::sync::atomic::Ordering::Release);
    }
    // Its register accesses done before anything it drove is switched off or handed on (`mmio_quiesce`).
    mmio_quiesce(&kk.procs[i].as_ref().unwrap().fast);
    // A kist no longer running: the kists' bell rung, so the map's circle stops glowing as running (`kists_live`). Under the lock, so a reader woken by it finds the process gone or ending.
    if kk.procs[i].as_ref().unwrap().kist {
        bell_count(BELL_KISTS);
    }
    gpu_ending(kk.procs[i].as_ref().unwrap().epoch, gpu_job_in_flight(i));
    surfaces_free(kk.procs[i].as_ref().unwrap().epoch);
    regions_free(kk.procs[i].as_ref().unwrap().epoch);
    pucks_free(kk.procs[i].as_ref().unwrap().epoch);
    crate::irqarm::disarm_epoch(kk.procs[i].as_ref().unwrap().epoch);
    cam_free(kk.procs[i].as_ref().unwrap().epoch);
    extents_free(kk.procs[i].as_ref().unwrap().epoch);
    end_watch_ring(kk.procs[i].as_ref().unwrap().epoch);
    crate::sensorlink::ended(kk.procs[i].as_ref().unwrap().epoch);
    power_orphaned(kk.procs[i].as_ref().unwrap().epoch);
    claims_orphaned(kk.procs[i].as_ref().unwrap().epoch);
    link_emptied(name);
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
        // Its mailboxes marked dead only now, with no thread of it left to publish over the mark.
        mailboxes_gone(p.epoch);
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
    /// A copy of what was written to the console while this spawn ran (anyone's, tagged with the sender's name); the text stays in the ring, so a caller never puts it back (`console_flashlog` for akita's flash log).
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
    /// The right to stow kists (`Object::Stow`): the stower's (payloads/kists).
    Stow,
    /// The hold, any kind (`Object::Items`; a development spawn's `--items`): a kist's comes from its capsule's declaration instead.
    Items,
}

/// Where a process finds its *ihi* (the oracle's output for its capsule): the last 32 bytes of the grants page.
pub const IHI: usize = 0xFE0;

/// Seals found only in the image form at a launch, each (old, new) to be moved to the address form by the service loop (`seal_moves`, service.rs `move_seals`). Under the kernel lock.
static mut SEAL_MOVES: Vec<([u8; 32], [u8; 32])> = Vec::new();

/// A masked image's seal checked against `sealed` (SANDBOX.md "Seal"; owner, 2026-11-17): by its address, `address`, which the caller computed from the bytes it is about to run on this same launch, so nothing runs on an address seal whose address was not checked then; else by its image (the old full pass over `code`), which then runs once more, and its move to the address form is queued. Takes the kernel lock for the queue unless held.
pub fn seal_ok(sealed: &[u8], developer: &[u8; 32], hp: &[u8; 32], m: &crate::oracle::Modes, address: &[u8; 32], code: &[u8]) -> Result<(), &'static str> {
    let key = crate::oracle::seal_key().ok_or("no system secret: cannot check a seal")?;
    match ferros_kist::seal::check(sealed, &key, developer, hp, true, m, address, || ferros_kist::seal::by_image(&key, developer, hp, true, m, code)) {
        ferros_kist::seal::Verdict::ByAddress => Ok(()),
        ferros_kist::seal::Verdict::ByImage(old, new) => {
            with_lock_held_or(|| {
                let q = unsafe { &mut *(&raw mut SEAL_MOVES) };
                if !q.iter().any(|e| e.0 == old) {
                    q.push((old, new));
                    QUEUED.fetch_or(QUEUED_SEALS, core::sync::atomic::Ordering::Relaxed);
                }
            });
            Ok(())
        }
        ferros_kist::seal::Verdict::No => Err("gate: masked capsule not installed (no seal)"),
    }
}

/// The seal moves queued since the last look (`seal_ok`), for the service loop to write. Takes the kernel lock.
pub fn seal_moves() -> Vec<([u8; 32], [u8; 32])> {
    if QUEUED.load(core::sync::atomic::Ordering::Relaxed) & QUEUED_SEALS == 0 {
        return Vec::new();
    }
    with_lock(|| {
        QUEUED.fetch_and(!QUEUED_SEALS, core::sync::atomic::Ordering::Relaxed);
        core::mem::take(unsafe { &mut *(&raw mut SEAL_MOVES) })
    })
}

/// Spawn a signed capsule: it must pass the gate (oracle.rs); the oracle's ihi is written into the process's arc at `IHI` and the kernel keeps no copy. Then as `spawn`.
pub fn spawn_capsule(name: &[u8], capsule: &[u8], input: &[u8], grants: &[Grant], sealed: &[u8]) -> Result<Outcome, &'static str> {
    spawn_capsule_as(name, capsule, input, grants, sealed, false)
}

/// `spawn_capsule`, and with `once` only while no live process has that name, checked under the kernel lock as the process is made (`spawn_as`): the service loop's restart of a kept process at its death (docs/KERNEL.md "Restarts"), so it never starts a second beside one init's keeper started meanwhile.
pub fn spawn_capsule_as(name: &[u8], capsule: &[u8], input: &[u8], grants: &[Grant], sealed: &[u8], once: bool) -> Result<Outcome, &'static str> {
    let g = crate::oracle::gate(capsule)?;
    // Masked (sandboxed) code runs only if it was installed — its seal is in the vault's `sealed` list (`sealed`: 32-byte entries) — and then without re-checking: the checker ran once at install (SANDBOX.md, run and gun). Native dev code spawns unchecked here; a production build refuses it entirely.
    if g.masked {
        // The address from the very bytes the process is built from.
        seal_ok(sealed, &g.developer_pubkey, &g.hp, &crate::oracle::modes_of(&g), blake3::hash(capsule).as_bytes(), &g.code)?;
    } else if cfg!(ferros_production) {
        return Err("gate: production refuses unmasked native code");
    }
    let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
    spawn_as(name, &g.code, input, grants, Some(ihi), g.masked, g.text_len, g.fixed | (g.bti as u32) << 8 | (g.exact as u32) << 9, once)
}

/// `spawn_capsule` over a capsule held outside the kernel heap (service.rs `restart`: a staging arc the vault's pieces were read into), its code never copied into the heap: the gate where it lies (`gate_located`: the signature over the whole file, the code found by its section's entry, `locate_code`, and held to be framed exactly so), masked code's modes read in place (`masked_modes`) and its seal found in `sealed`, then the process built from the code where it lies (`spawn` copies it into the process's own arc). The same checks as `spawn_capsule`'s gate, whose parse copied the code out of the capsule and then copied it again (2026-11-12: with the capsule itself, three blocks of 3 MB in the heap at once for the display's).
pub fn spawn_capsule_in_place(name: &[u8], capsule: &mut [u8], input: &[u8], grants: &[Grant], sealed: &[u8], once: bool) -> Result<Outcome, &'static str> {
    let (co, cl, masked) = locate_code(capsule, capsule.len())?;
    let g = crate::oracle::gate_located(capsule, co, cl)?;
    let (exec_len, mode) = if masked {
        let m = crate::oracle::masked_modes(capsule, co, cl)?;
        // The address from the bytes in the staging arc, which the process is built from.
        let address = *blake3::hash(capsule).as_bytes();
        seal_ok(sealed, &g.developer_pubkey, &g.hp, &m, &address, &capsule[co..co + cl])?;
        (m.text_len, m.fixed | (m.bti as u32) << 8 | (m.exact as u32) << 9)
    } else {
        if cfg!(ferros_production) {
            return Err("gate: production refuses unmasked native code");
        }
        (cl, 0)
    };
    let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
    spawn_as(name, &capsule[co..co + cl], input, grants, Some(ihi), masked, exec_len, mode, once)
}

/// Install a masked capsule: the gate (signature), the sandbox checker over its instruction span — once — then its seal. Returns the seal for the caller to add to the vault's `sealed` list.
pub fn install_capsule(capsule: &[u8]) -> Result<[u8; 32], &'static str> {
    let g = crate::oracle::gate(capsule)?;
    if !g.masked {
        return Err("install: only masked capsules are installed");
    }
    let mode = if g.exact { ferros_sandbox::Mode::Exact } else if g.fixed != 0 { ferros_sandbox::Mode::Fixed(g.fixed) } else { ferros_sandbox::Mode::Ring };
    ferros_sandbox::check_image_with(&g.code[..g.text_len], mode, g.bti).map_err(|_| "install: masked code failed the sandbox checker")?;
    // The address form (2026-11-17): the address of the bytes just gated and checked.
    crate::oracle::seal_address(&g.developer_pubkey, &g.hp, true, &crate::oracle::modes_of(&g), blake3::hash(capsule).as_bytes()).ok_or("no system secret: cannot seal")
}

/// Spawn `code` (position-independent, entry at offset 0) as a process holding a console capability (grant slot 0) plus `grants` (slots 1..), and run until nothing is runnable: it exits, or it blocks (a server in recv stays parked for later callers). `input` is copied into the arc after the code; x0 = grants offset, x1 = input offset, x2 = input length.
pub fn spawn(name: &[u8], code: &[u8], input: &[u8], grants: &[Grant], ihi: Option<[u8; 32]>, masked: bool, exec_len: usize, fixed: u32) -> Result<Outcome, &'static str> {
    spawn_as(name, code, input, grants, ihi, masked, exec_len, fixed, false)
}

/// `spawn`, and with `once` refused ("running already") when a live process has that name as the process is made, under the kernel lock, so no second starts beside one made meanwhile (as `spawn_build`'s `once`).
pub fn spawn_as(name: &[u8], code: &[u8], input: &[u8], grants: &[Grant], ihi: Option<[u8; 32]>, masked: bool, exec_len: usize, fixed: u32, once: bool) -> Result<Outcome, &'static str> {
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
    // The arc taken under the lock, the code copied into it with the lock let go (2026-11-18: a capsule's code is up to megabytes; the arc is no process's until `build_from`, so nothing else reaches it), then the process made under the lock again.
    if cap_key().is_none() {
        unlock();
        return Err("no wairua: refusing to mint capabilities");
    }
    let arc = match arc_for(code, input.len(), masked, fixed) {
        Ok(a) => a,
        Err(e) => {
            unlock();
            return Err(e);
        }
    };
    unlock();
    // SAFETY: the arc was just taken for this image (`arc_for` sized it past CODE plus the code's length) and is no process's yet.
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), (arc.base + CODE) as *mut u8, code.len()) };
    spawn_step(9);
    lock();
    // A process of that name made meanwhile (init's keeper, a restart by hand): nothing made here, the arc back to the pool.
    if once && named_live(name) {
        arc_free(arc);
        unlock();
        return Err("running already");
    }
    // A refused build made nothing that holds the arc: it goes back to the pool.
    let built = build_from(name, arc, code.len(), input, &objects, ihi, masked, exec_len, fixed >> 8 & 1 == 1).inspect_err(|_| arc_free(arc));
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
/// Whether a launch, a stow or a seal move waits (bits `QUEUED_LAUNCH`, `QUEUED_STOW`, `QUEUED_SEALS`): set under the kernel lock with the queue, read without it by the service loop, which takes the lock only when its bit is set (2026-11-19: the three takes held the lock three times a pass, at up to 4,800 passes a second under a Lumis save).
pub static QUEUED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
const QUEUED_LAUNCH: u32 = 1 << 0;
const QUEUED_STOW: u32 = 1 << 1;
const QUEUED_SEALS: u32 = 1 << 2;
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
        "audio" => (Object::Endpoint(endpoint(b"audio")), RIGHT_SEND),
        // The cameras (docs/CAMERA.md): a send right on the camera driver's endpoint. Until a driver serves it, a call that fails at once when nobody receives (CALL_LIVE) tells the kist so, and Lumis says there is no camera driver; never a fake camera.
        "camera" => (Object::Endpoint(endpoint(b"camera")), RIGHT_SEND),
        // Be you and the person's people (docs/FACETS.md, 2026-11-13): a send right on the identity service's `facets` endpoint, either right the same one. The kernel stamps every call on it with the caller's kist identity and which of the two it was granted (`facets_stamp`), so the service answers each op by the grant and never by the caller's word.
        "be_you" | "people" => (Object::Endpoint(endpoint(b"facets")), RIGHT_SEND),
        // A puck's circle (docs/PUCKS.md, 2026-11-17): a send right on the display's `puck` endpoint, every call on it stamped with the caller's kist identity (`puck_stamp`), so the display draws a puck's list only into the circle the deck gave that identity and no puck can name another's.
        "puck" => (Object::Endpoint(endpoint(b"puck")), RIGHT_SEND),
        // The person's things (docs/STORE.md, 2026-11-19): the hold, its kinds the capsule's `items` section (set on the process once it is built, `spawn_kist`).
        "items" => (Object::Items, RIGHT_SEND),
        _ => return None,
    })
}

/// The GPU, as every kist is handed it (owner, 2026-11-12: "I feel like GPU should be implicit. If I'm running it on my machine..."; GPU-KISTS.md 1.6): asked of no one, like its CPU time, three slots right after its wants (ferros_sys::gpu `kist_slots`; `ferros_kist::slots` lays them out: after the leading implicit rights and the asked wants, or after a capsule's declared wants): the GPU service (`use gpu`, every call stamped with the kist's own memory, `gpu_stamp`), a GPU block of its own (`NewRegion`: 32 MiB of frames at launch since 2026-11-13, grown to what it asks for by SYS_DMA x1 = 2 up to `GPU_BLOCK_MOST`, so a kist that never draws on the GPU holds little and one that does is not held to what the kists started first left), and a second surface for a mailbox (`SpareSurface`). Each an empty slot when there is none to give. A production build gives three empty slots: a kist's GPU programs run unchecked, so they run on development builds only until the checked kernel form exists (GPU-KISTS.md plan steps 3-6), as its native code does; the kist then draws on the CPU, and its slots keep their places.
fn kist_gpu() -> [(Object, u32); 3] {
    if cfg!(ferros_production) {
        return [(Object::Empty, 0); 3];
    }
    [(Object::Endpoint(endpoint(b"gpu")), RIGHT_SEND), (Object::NewRegion(BLOCK_LEAST_LOG), RIGHT_SEND | RIGHT_RECV), (Object::SpareSurface, RIGHT_SEND | RIGHT_RECV)]
}

/// `SYS_SPAWN` in kist mode (x2 bit 1; bit 2, a relaunch: a live process of that name killed first, NOTHING with none alive; bit 5, a stop: a live process of that name killed and nothing launched, 0 or NOTHING with none alive): [name length][name][the capsule's 32-byte vault address][the caller's Kists token, 32 bytes][n][n right codes, one a declared want in its order: the right, or `EMPTY` for an asked want said no to; for an implicit want what is sent does not matter, the kernel hands it anyway]. Queued for the service loop, which streams the capsule from the vault and checks every slot against the capsule's own declaration before anything runs (`spawn_kist`). 0 queued, NOTHING already running or starting, ERR refused (the reason on the console): no Kists right, a code outside the vocabulary, nobody present (a kist's data and rights are the person's), or another launch still streaming.
fn kist_request(p: &Process, req: &[u8], replace: bool, stop: bool) -> u64 {
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
    if !crate::kvault::PERSON.load(core::sync::atomic::Ordering::Acquire) {
        return refuse("nobody is present: a kist runs only for an attested person");
    }
    // A stop (the person's Stop on the kist's card, owner 2026-11-12; docs/KISTS.md "The deck and the card"): the running kist killed, one shot dead, and nothing launched. No record is kept, as for a relaunch's kill: the person's own act, so its circle goes to not running, not stopped. Ahead of the launch slot, so a launch still streaming never holds a stop back.
    // Only the process `kill_locked` would find by that name, and only when it is that kist (by its capsule): a kist named as a system process stops nothing of the system's.
    if stop {
        let theirs = kk.procs.iter().flatten().find(|q| q.name == name && !q.dying).is_some_and(|q| q.kist && q.capsule.as_ref().map(|c| &c[..]) == Some(&capsule[..]));
        if !theirs {
            return NOTHING;
        }
        kill_locked(&name);
        return 0;
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
    } else if alive {
        // Running: the surface it was given and that surface's generation, so the launcher can bring its window to the front (display op 59; owner, 2026-11-11: a running kist's circle tapped raises it, never a second copy). A kist with no surface (none was free, or it asked for none): NOTHING, as before.
        let slots = unsafe { &*(&raw const SLOTS) };
        let surface = kk.procs.iter().flatten().find(|q| q.name == name && !q.dying).and_then(|q| slots.iter().position(|s| s.owner == q.epoch));
        return match surface {
            Some(s) => RUNNING_ON | (s as u64) << 8 | slots[s].generation as u64,
            None => NOTHING,
        };
    } else if unsafe { &*(&raw const KIST_STARTING) }.as_slice() == name.as_slice() {
        return NOTHING;
    }
    *slot = Some(KistLaunch { name, capsule: capsule[..].try_into().unwrap(), codes });
    QUEUED.fetch_or(QUEUED_LAUNCH, core::sync::atomic::Ordering::Relaxed);
    unsafe { core::arch::asm!("sev") };
    0
}

/// Whether a process is ending but not yet gone (a thread of it still on a core: its arc returns at that thread's next trap).
pub fn any_dying() -> bool {
    with_lock(|| k().procs.iter().flatten().any(|p| p.dying))
}

/// The service loop takes the waiting launch, marking its name as starting until `kist_done`.
pub fn kist_take() -> Option<KistLaunch> {
    if QUEUED.load(core::sync::atomic::Ordering::Relaxed) & QUEUED_LAUNCH == 0 {
        return None;
    }
    with_lock(|| {
        QUEUED.fetch_and(!QUEUED_LAUNCH, core::sync::atomic::Ordering::Relaxed);
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
pub fn kist_need(head: &[u8], total: usize) -> Result<(usize, usize), &'static str> {
    let (co, cl, _) = locate_code(head, total)?;
    if co + 56 > head.len() {
        return Err("kist: the code's header is past the first bytes read");
    }
    if !has_image_header(&head[co..co + 56]) {
        return Err("kist: the image has no header (ferros_sys::entry!), so its .bss is unknown");
    }
    let tail = image_tail(&head[co..co + 56]);
    // And the most it may grow to (SYS_ARC): the code with its whole declared heap.
    let ceiling = image_tail_ceiling(&head[co..co + 56]);
    Ok((CODE + total.max(cl + tail) + STACK + (1 << 16), CODE + total.max(cl + ceiling) + STACK + (1 << 16)))
}

/// A streamed spawn's arc (service.rs, the bridge's spawn Write pieces: `bridge spawn` of a capsule past 1 MiB) as (its size at spawn, its ceiling), from the bytes the host asked for and the image's first 56 bytes. The host counts the image's whole declared heap; a growing image starts with `image_tail` and keeps the rest as its ceiling (SYS_ARC), as a kist's arc does (`kist_need`). Until 2026-11-12 the arc was the host's sum, never growing: Mahere's 640 MiB heap asked for 640 MiB at spawn, which akita's 336 MiB pool refused outright ("Piece at 0 not taken").
pub fn stream_room(need: usize, code: &[u8]) -> (usize, usize) {
    if !has_image_header(code) {
        return (need, need);
    }
    // The host counts the declared heap as the image states it (ferros-bridge: header words 16 and 48, unclamped), so the heap the arc does not start with is taken off as the image states it too.
    // Until 2026-11-22 this took off `image_tail_ceiling - image_tail`, where both are clamped at 1 GiB: Lumis declares a 4 GiB heap (std/lumis/build.rs), so 3 GiB of it stayed in the arc at spawn, about 1550 frames taken, zeroed and mapped under the kernel lock (the Pro, b1129: 138 ms at service.rs's staging, then 64 ms in `build_from`'s W^X pass over the same 3 GiB), and the ultrawide's stream, open in the stored Lumis, died at that moment.
    let grow = u64::from_le_bytes(code[48..56].try_into().unwrap()) as usize - image_heap_start(code);
    // WHY the check: `need` is the host's; one that leaves the heap out (a bridge from before the header) has nothing to give back, and keeps its sum as it is.
    // The stack's guard page on top (`build_from` wants it above the image; the host's sum leaves it out, which only the arc's rounding to a frame covered, and the 3 GiB above for Lumis).
    match need.checked_sub(grow) {
        Some(size) => (size + STACK_GUARD, need),
        None => (need, need),
    }
}

/// Whether a kist's capsule carries masked code, from its first bytes (its header's table names the section): its arc is then taken masked, with the guard above it (SANDBOX.md "Placement and guards").
pub fn kist_masked(head: &[u8], total: usize) -> bool {
    locate_code(head, total).is_ok_and(|c| c.2)
}

/// A masked kist's code where it lies in its staged capsule, gated, its modes read and its seal found in `sealed` (32-byte entries): SANDBOX.md's "spawn runs only sealed images", one BLAKE3 over the image and no checker. Returns the developer's key and identity for the ihi, and the modes.
/// `address` is the capsule's, already checked against its bytes in this arc on this launch (`spawn_kist`), so the seal is found by address with no pass over the image; a kist sealed before 2026-11-17 is checked by its image once more and its seal moved (`seal_ok`).
fn kist_sealed(capsule: &mut [u8], co: usize, cl: usize, sealed: &[u8], pre: Option<&KistHashes>, address: &[u8; 32]) -> Result<(crate::oracle::Located, crate::oracle::Modes), KistFault> {
    use ferros_kist::launch::why;
    let t = crate::platform::cntpct();
    let g = crate::oracle::gate_located_with(capsule, co, cl, pre.map(|p| (&p.file, p.slot))).map_err(|e| (why::CHECKS, e))?;
    kist_time(KT_GATE, t);
    let t = crate::platform::cntpct();
    let m = crate::oracle::masked_modes(capsule, co, cl).map_err(|e| (why::CHECKS, e))?;
    let r = seal_ok(sealed, &g.developer_pubkey, &g.hp, &m, address, &capsule[co..co + cl]);
    kist_time(KT_SEAL, t);
    match r {
        Ok(()) => Ok((g, m)),
        Err(e) if crate::oracle::seal_key().is_none() => Err((why::OTHER, e)),
        Err(_) => Err((why::NOT_HERE, "gate: masked kist not stowed on this device (no seal)")),
    }
}

/// A kist launch that did not start: why (`ferros_kist::launch::why`, or `KIST_ALREADY`), and the job's words for the console.
pub type KistFault = (u8, &'static str);
/// Not a fault: a process of the kist's name started while its capsule was read, so it runs and nothing is kept for the map.
pub const KIST_ALREADY: u8 = u8::MAX;

/// A capsule's two hashes, taken in one pass as it was read into its arc (`ferros_kist::twin`, service.rs `KistJob`): its address, the hash its signature covers, and where its header puts the signature's slot. The arc is the kernel's alone from the read to the build (no process maps it), so what was hashed is what is checked and run.
pub struct KistHashes {
    pub address: [u8; 32],
    pub file: [u8; 32],
    pub slot: (usize, usize),
}

/// The last kist launch's checks, each in counter ticks, for the service loop's "started from the vault" line (2026-11-17, owner: "it takes TWO SECONDS to open Photon"): the address, the gate, the seal, the build.
pub const KT_ADDRESS: usize = 0;
pub const KT_GATE: usize = 1;
pub const KT_SEAL: usize = 2;
pub const KT_BUILD: usize = 3;
pub static KIST_TICKS: [core::sync::atomic::AtomicU64; 4] = [const { core::sync::atomic::AtomicU64::new(0) }; 4];

/// The ticks since `t` kept as check `k` of the last launch.
fn kist_time(k: usize, t: u64) {
    KIST_TICKS[k].store(crate::platform::cntpct() - t, core::sync::atomic::Ordering::Relaxed);
}

/// A kist whose capsule the service loop streamed from the vault into `arc` at the code offset (`total` bytes): it must be the capsule named (its blake3 is its vault address), pass the gate where it lies, and declare every asked right handed to it in the slot it is handed in (`ferros_kist::want_codes`: nothing undeclared can be asked for), the implicit set handed to it without a question in the slots `ferros_kist::slots` gives them. Masked code runs only with its seal in `sealed` (the stower's, `stow_kist`), the checker never run again; native code only in a development build (the bridge's `kist stow`). Then the code moves down, the rest is zeroed, and the process is built with exactly the layout's objects, queued to run, and what was kept of its last launch not starting dropped (`kist_started`). On any refusal the arc goes back to the pool, and the reason comes back as the map shows it (`ferros_kist::launch::why`).
/// `pre`: the address and the signature's hash as the read took them (`KistHashes`), checked here in place of hashing the capsule again; None hashes it here.
pub fn spawn_kist(l: &KistLaunch, arc: Arc, total: usize, sealed: &[u8], pre: Option<&KistHashes>) -> Result<(), KistFault> {
    use ferros_kist::launch::why;
    for t in &KIST_TICKS {
        t.store(0, core::sync::atomic::Ordering::Relaxed);
    }
    let mut arc = arc;
    let r = (|| {
        if CODE + total + STACK > arc.size {
            return Err((why::OTHER, "kist: the capsule does not fit its arc"));
        }
        let capsule = unsafe { core::slice::from_raw_parts_mut((arc.base + CODE) as *mut u8, total) };
        // The address taken as the bytes came in, where the job could (a capsule with a signature slot in its first bytes); else hashed here.
        let t = crate::platform::cntpct();
        let address = match pre {
            Some(p) => p.address,
            None => *blake3::hash(capsule).as_bytes(),
        };
        kist_time(KT_ADDRESS, t);
        if address != l.capsule {
            return Err((why::READ, "kist: the vault's capsule is not the one named (its blake3)"));
        }
        let wants = ferros_kist::want_codes(capsule).map_err(|e| (why::READ, e))?;
        if wants.len() != l.codes.len() || wants.iter().zip(&l.codes).any(|(&w, &c)| c != ferros_kist::EMPTY && c != w) {
            return Err((why::CHECKS, "kist: a right the capsule does not declare in that slot"));
        }
        // The kinds of the hold it declares (STORE.md part 1), read before its code is moved over the capsule; they matter only where the person granted `items`.
        let items = ferros_kist::items_of(capsule).map_err(|e| (why::READ, e))?;
        let (co, cl, masked) = locate_code(capsule, total).map_err(|e| (why::READ, e))?;
        let (g, modes) = if masked {
            let (g, m) = kist_sealed(capsule, co, cl, sealed, pre, &address)?;
            (g, Some(m))
        } else {
            if cfg!(ferros_production) {
                return Err((why::CHECKS, "gate: production refuses unmasked native code"));
            }
            let t = crate::platform::cntpct();
            let g = crate::oracle::gate_located_with(capsule, co, cl, pre.map(|p| (&p.file, p.slot))).map_err(|e| (why::CHECKS, e))?;
            kist_time(KT_GATE, t);
            (g, None)
        };
        let t_build = crate::platform::cntpct();
        // A masked arc is the one its code was checked for: fixed mode's exactly 2^K, which a staged capsule's arc is not (it holds the whole capsule first).
        if modes.as_ref().is_some_and(|m| m.fixed != 0 && arc.size != 1 << m.fixed) {
            return Err((why::OTHER, "kist: a fixed-mode kist is not launched from the vault yet (its arc must be exactly the sealed size)"));
        }
        if masked != (arc.guard != 0) {
            return Err((why::OTHER, "kist: the staged arc's kind is not the code's (masked or native)"));
        }
        let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or((why::OTHER, "no ira: no system secret, no ihi"))?;
        unsafe {
            core::ptr::copy((arc.base + CODE + co) as *const u8, (arc.base + CODE) as *mut u8, cl);
            core::ptr::write_bytes((arc.base + CODE + cl) as *mut u8, 0, total - cl);
        }
        Ok((ihi, cl, modes, wants, (g.developer_pubkey, g.hp), t_build, items))
    })();
    let (ihi, cl, modes, wants, gate, t_build, items) = match r {
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
            return Err((KIST_ALREADY, "kist: a process of that name started meanwhile"));
        }
        // Every slot's object made here, laid out from the capsule's own wants (`ferros_kist::slots`; KISTS.md "The implicit set"): an implicit right's object always, whatever init sent for it (owner, 2026-11-12: "Yes on the implicit set"), an asked want's as init sent it (its right or an empty slot, checked against the declaration above), the GPU's (`kist_gpu`). Every code is in the vocabulary (checked at the request), so `kist_object` makes each.
        // Which facets the person granted (an asked want answered yes, sent as its own code): stamped on the kist's calls on `facets`.
        let facets = wants.iter().zip(l.codes.iter()).fold(0u8, |m, (&w, &c)| m | ((w == c && c == ferros_kist::BE_YOU) as u8) | (((w == c && c == ferros_kist::PEOPLE) as u8) << 1));
        let gpu = kist_gpu();
        // A puck (its capsule declares `puck`; docs/PUCKS.md) draws its square and never shows a window: in its surface's place a square of the pucks' window (`NewPuck`, frames only as it asks), and its GPU slots empty, so a puck holds no surface slot of the sixteen, none of a surface's 32 MiB, and no GPU block.
        let puck = wants.contains(&ferros_kist::PUCK);
        let objects: Vec<(Object, u32)> = ferros_kist::slots(&wants)
            .iter()
            .filter_map(|s| match *s {
                ferros_kist::Slot::Implicit(ferros_kist::SURFACE) if puck => Some((Object::NewPuck, RIGHT_SEND | RIGHT_RECV)),
                ferros_kist::Slot::Gpu(_) if puck => Some((Object::Empty, 0)),
                ferros_kist::Slot::Implicit(c) => kist_object(c),
                ferros_kist::Slot::Asked(i) => kist_object(l.codes[i]),
                ferros_kist::Slot::Gpu(g) => Some(gpu[g as usize]),
            })
            .collect();
        match build_from(&l.name, arc, cl, b"", &objects, Some(ihi), masked, exec_len, bti) {
            Ok((me, _, _)) => {
                if let Some(q) = th(me).map(|t| t.proc).and_then(|pi| kk.procs.get_mut(pi)).and_then(|q| q.as_mut()) {
                    q.kist = true;
                    q.capsule = Some(l.capsule);
                    q.gate = Some(gate);
                    q.facets = facets;
                    // The hold's kinds as its capsule declares them, where the person granted `items` (its slot holds the right, not an empty one); none otherwise.
                    let granted = wants.iter().zip(l.codes.iter()).any(|(&w, &c)| w == c && c == ferros_kist::ITEMS);
                    q.items = granted.then(|| alloc::boxed::Box::new(ItemRight { any: false, makes: items.makes.iter().map(|k| k.0).collect(), opens: items.opens.iter().map(|k| k.0).collect() }));
                }
                enqueue_back(me);
                kist_started(&l.capsule);
                kist_time(KT_BUILD, t_build);
                Ok(())
            }
            Err(e) => {
                arc_free(arc);
                Err((why::OTHER, e))
            }
        }
    })
}

/// The kist launches that did not start (owner, 2026-11-11: Mahere's circle tapped, "nothing opened", the console alone saying the arc pool had no room), and the kists that started and then stopped abnormally (`kist_stopped`, the `kill` from outside): one record a capsule, its reason (`ferros_kist::launch::why`) and the kists' bell count it rang, kept until that kist next starts. A record a kist the person has, at most: a capsule launched again replaces its own. Read by the Settings holder alone (SYS_NOT_STARTED op 0), as the claims' users list. Under the kernel lock.
static mut NOT_STARTED: Vec<([u8; 32], u8, u64)> = Vec::new();
/// A record of SYS_NOT_STARTED op 0: the reason, the capsule's address, the kists' bell count it rang (u64 LE).
pub const NOT_STARTED_REC: usize = 41;

/// The service loop's kist job ended without starting `capsule`'s kist, for `why`: kept (`NOT_STARTED`), and the kists' bell rung, so the map, which asked init to launch it and was answered "started", learns it did not and says why on its circle. Rung even when the record was the same, since each launch the person asks for is told. Takes the kernel lock.
pub fn kist_not_started(capsule: &[u8; 32], why: u8) {
    with_lock(|| kist_record(capsule, why));
}

/// `capsule`'s kist kept with `why` (a start that did not happen, or a stop after one: `kist_stopped`), replacing what was kept for it, and the kists' bell rung. Under the kernel lock.
fn kist_record(capsule: &[u8; 32], why: u8) {
    let v = BELL_COUNT[BELL_KISTS].load(core::sync::atomic::Ordering::Acquire) + 1;
    let list = unsafe { &mut *(&raw mut NOT_STARTED) };
    list.retain(|r| r.0 != *capsule);
    list.push((*capsule, why, v));
    bell_count(BELL_KISTS);
}

/// A process ended with `value` (its exit value, or the kernel's FAULT word): a kist that ended abnormally is kept with why, as a launch that did not start is (owner, 2026-11-11: Mahere started, faulted at once, "no toast or anything, just no indication at all"), so the map can say so on its circle. A fault (`FAULT` in the top byte, a syscall's bad buffer among them) or a panic (G#DEAD, ferros_sys::panic_exit and std's abort; 101, std's main panicking); any other exit value is the kist's own and keeps nothing. Every kist's end rings the kists' bell, kept or not, so its circle stops glowing as running (`kists_live`). Under the kernel lock.
fn kist_stopped(p: &Process, value: u64) {
    use ferros_kist::launch::why;
    let Some(capsule) = p.capsule.filter(|_| p.kist) else { return };
    let r = if value >> 56 == FAULT >> 56 {
        why::FAULT
    } else if value == 0xDEAD || value == 101 {
        why::PANIC
    } else {
        bell_count(BELL_KISTS);
        return;
    };
    kist_record(&capsule, r);
}

/// `capsule`'s kist started: what was kept of a launch of it that did not is dropped, and the kists' bell rung, so its circle glows as running (`kists_live`) and the map's note on it goes. Under the kernel lock.
fn kist_started(capsule: &[u8; 32]) {
    let list = unsafe { &mut *(&raw mut NOT_STARTED) };
    list.retain(|r| r.0 != *capsule);
    bell_count(BELL_KISTS);
}

/// The kists alive now (SYS_NOT_STARTED op 2): each process the kist job started (by its capsule), not ending. Under the kernel lock.
fn kists_live() -> Vec<[u8; 32]> {
    k().procs.iter().flatten().filter(|q| q.kist && !q.dying).filter_map(|q| q.capsule).collect()
}

/// A stow the stower asked for (`stow_request`): the capsule by its vault address, and where to tell it each stage: the process (its slot and epoch, so a word is never written into a successor's arc) and the word's address in its arc.
pub struct KistStow {
    pub capsule: [u8; 32],
    /// A stow's check (`SYS_SPAWN` stow mode with x2 bit 4, ferros_sys `stow_check`): native code on a development build is taken unchecked (`STOW_DONE` with `STOW_NATIVE`) instead of refused.
    pub native_ok: bool,
    proc: usize,
    epoch: u64,
    word: usize,
}

/// The kist a host offered over USB to stow (PT `ferros.kist.offer`, service.rs; docs/KISTS.md "Stowing", ferros_kist::offer): its capsule's address in `capsules.` and its length, its state as the stower last told it (SYS_OFFER op 2), why it was refused, and its flags. One at a time; kept after it ends until the next replaces it, so the host reads its end. Under the kernel lock.
pub struct Offer {
    pub capsule: [u8; 32],
    pub len: u64,
    pub state: u8,
    pub why: u8,
    pub flags: u8,
}
static mut OFFER: Option<Offer> = None;

/// A host offers the capsule it carried in whole: kept as OFFERED and the offers' bell rung (the stower takes it). Err while another offer has not ended (the person has not answered it yet).
pub fn offer_put(capsule: [u8; 32], len: u64) -> Result<(), &'static str> {
    with_lock(|| {
        let o = unsafe { &mut *(&raw mut OFFER) };
        if o.as_ref().is_some_and(|o| !ferros_kist::offer::finished(o.state)) {
            return Err("another kist is offered and waiting on the person");
        }
        *o = Some(Offer { capsule, len, state: ferros_kist::offer::OFFERED, why: 0, flags: 0 });
        bell_count(BELL_OFFERS);
        Ok(())
    })
}

/// Whether an offer has not ended yet (no part of another capsule is taken meanwhile).
pub fn offer_open() -> bool {
    with_lock(|| unsafe { (*(&raw const OFFER)).as_ref() }.is_some_and(|o| !ferros_kist::offer::finished(o.state)))
}

/// The offer's state for a host's read of `capsule`: [state, why, flags], GONE when it is not the offer.
pub fn offer_state(capsule: &[u8; 32]) -> [u8; 3] {
    with_lock(|| match unsafe { (*(&raw const OFFER)).as_ref() } {
        Some(o) if o.capsule == *capsule => [o.state, o.why, o.flags],
        _ => [ferros_kist::offer::GONE, 0, 0],
    })
}

/// Nobody is present (the person left, or never came): an offer not ended is gone, so it is never shown to whoever attests next. Its bytes stay in `capsules.`, named by no list (inert: a launch reads only what a person's list names).
pub fn offer_drop() {
    with_lock(|| {
        if let Some(o) = unsafe { (*(&raw mut OFFER)).as_mut() }.filter(|o| !ferros_kist::offer::finished(o.state)) {
            crate::flashlog::line(&alloc::format!("kist offer: gone, the person left while it was {}", ferros_kist::offer::state_name(o.state)));
            o.state = ferros_kist::offer::GONE;
        }
    })
}
/// One stow at a time, beside one launch: the service loop streams one capsule through the kernel's channel at once (launches first). Under the kernel lock.
static mut KIST_STOW: Option<KistStow> = None;

/// The stages a stow is told in (bits 56-63 of the word; ferros_sys `stow` mirrors them): reading the capsule from the vault (the bytes read so far below), checking its code, sealing it, done (the word's low bits 0), refused (the low byte the reason, `STOW_WHY_*`, said on the console too).
pub const STOW_READING: u64 = 0;
pub const STOW_CHECKING: u64 = 1;
pub const STOW_SEALING: u64 = 2;
pub const STOW_DONE: u64 = 3;
pub const STOW_REFUSED: u64 = 4;
/// Why a stow was refused: the vault's capsule is not the one named, its sections do not read, it is native code (a development build: never stowed from the network, owner 2026-11-10), its signature does not verify, its masked fields do not read, the checker refused its code, no system secret to seal with, the vault would not keep the seal, the vault could not read it, no arc for it.
pub const STOW_WHY_NOT_NAMED: u64 = 1;
pub const STOW_WHY_SECTIONS: u64 = 2;
pub const STOW_WHY_NATIVE: u64 = 3;
pub const STOW_WHY_SIGNATURE: u64 = 4;
pub const STOW_WHY_MODES: u64 = 5;
pub const STOW_WHY_CHECKER: u64 = 6;
pub const STOW_WHY_SECRET: u64 = 7;
pub const STOW_WHY_KEPT: u64 = 8;
pub const STOW_WHY_READ: u64 = 9;
pub const STOW_WHY_ROOM: u64 = 10;
/// `STOW_DONE`'s value for a stow's check that found native code on a development build (`KistStow::native_ok`): taken unchecked, nothing sealed.
pub const STOW_NATIVE: u64 = 1;

/// `SYS_SPAWN` in stow mode (x2 bit 3; bit 4 a stow's check, `native_ok`): [the capsule's 32-byte address in `capsules.`][the caller's Stow token, 32 bytes][the word's offset in its arc, u64 LE]. Queued for the service loop (`stow_take`), which reads the capsule from the vault and tells the word each stage, waking whoever waits on it (`stow_tell`). 0 queued, ERR refused (the reason on the console): no Stow right, a malformed request, another stow still under way.
fn stow_request(p: &Process, pi: usize, req: &[u8], native_ok: bool) -> u64 {
    let refuse = |why: &str| {
        let kk = k();
        kk.console.extend_from_slice(b"kist stow refused: ");
        kk.console.extend_from_slice(why.as_bytes());
        kk.console.push(b'\n');
        ERR
    };
    if req.len() < 72 {
        return refuse("a malformed request");
    }
    if !matches!(p.lookup(&req[32..64]).map(|i| (p.cspace[i].object, p.cspace[i].rights)), Some((Object::Stow, r)) if r & RIGHT_SEND != 0) {
        return refuse("the caller holds no Stow right");
    }
    let slot = unsafe { &mut *(&raw mut KIST_STOW) };
    if slot.is_some() {
        return refuse("another stow is still under way");
    }
    let off = u64::from_le_bytes(req[64..72].try_into().unwrap());
    // The word inside the caller's own arc, as `SYS_WAIT` finds it: aligned, and refused outside the arc.
    let Some(word) = p.word(off) else { return refuse("the word is outside the caller's arc") };
    *slot = Some(KistStow { capsule: req[..32].try_into().unwrap(), native_ok, proc: pi, epoch: p.epoch, word });
    QUEUED.fetch_or(QUEUED_STOW, core::sync::atomic::Ordering::Relaxed);
    unsafe { core::arch::asm!("sev") };
    0
}

/// The service loop takes the waiting stow (after any launch).
pub fn stow_take() -> Option<KistStow> {
    if QUEUED.load(core::sync::atomic::Ordering::Relaxed) & QUEUED_STOW == 0 {
        return None;
    }
    with_lock(|| {
        QUEUED.fetch_and(!QUEUED_STOW, core::sync::atomic::Ordering::Relaxed);
        unsafe { (*(&raw mut KIST_STOW)).take() }
    })
}

/// The stower told a stage: `stage` in bits 56-63, `value` below (the bytes read, or the reason), written into its word and every thread of it waiting there woken. Nothing when the process has ended (its slot empty or a successor's).
pub fn stow_tell(i: &KistStow, stage: u64, value: u64) {
    with_lock(|| {
        let kk = k();
        // WHY the arc's end and its holes: the stower may have shrunk its arc or given frames back (SYS_ARC) since it named the word; one there is not written (it would be an unmapped slot, the kernel's own fault).
        if !kk.procs.get(i.proc).and_then(|q| q.as_ref()).is_some_and(|q| q.epoch == i.epoch && !q.dying && i.word + 8 <= q.arc.base + q.arc.size && !q.in_hole(i.word - q.arc.base, 8)) {
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

/// A stow's capsule, read from the vault into `arc` at the code offset (`total` bytes), checked as SANDBOX.md's lifecycle says, once: it is the capsule named (its blake3), its code is masked (native code is a development build, never stowed from the network), its signature verifies where it lies (`gate_located`, the Ed25519 egg; the stower checked every egg before it stored it), and the sandbox checker passes its instruction span in its mode. Returns the code's span in the arc, for `stow_seal`. The arc is the caller's to free.
pub fn stow_check(arc: &Arc, total: usize, addr: &[u8; 32]) -> Result<(crate::oracle::Located, crate::oracle::Modes, usize, usize), u64> {
    let capsule = unsafe { core::slice::from_raw_parts_mut((arc.base + CODE) as *mut u8, total) };
    let say = |why: &str| push_console(alloc::format!("kist stow: {why}\n").as_bytes());
    if blake3::hash(capsule).as_bytes() != addr {
        say("the vault's capsule is not the one named (its blake3)");
        return Err(STOW_WHY_NOT_NAMED);
    }
    let (co, cl, masked) = locate_code(capsule, total).map_err(|e| {
        say(e);
        STOW_WHY_SECTIONS
    })?;
    if !masked {
        say("native code: a development build is not stowed from the network");
        return Err(STOW_WHY_NATIVE);
    }
    let g = crate::oracle::gate_located(capsule, co, cl).map_err(|e| {
        say(e);
        STOW_WHY_SIGNATURE
    })?;
    let m = crate::oracle::masked_modes(capsule, co, cl).map_err(|e| {
        say(e);
        STOW_WHY_MODES
    })?;
    let mode = if m.exact { ferros_sandbox::Mode::Exact } else if m.fixed != 0 { ferros_sandbox::Mode::Fixed(m.fixed) } else { ferros_sandbox::Mode::Ring };
    if let Err(v) = ferros_sandbox::check_image_with(&capsule[co..co + m.text_len], mode, m.bti) {
        say(&alloc::format!("the checker refused its code at +G#{:X} (instruction G#{:08X})", v.offset, v.insn));
        return Err(STOW_WHY_CHECKER);
    }
    Ok((g, m, co, cl))
}

/// The seal of a stow that passed `stow_check`: keyed by this device's system secret, so it is good on this device alone.
/// The address form (2026-11-17): `addr` is the capsule's, which `stow_check` held to the bytes in the arc it checked, and nothing has written the arc since.
pub fn stow_seal(g: &crate::oracle::Located, m: &crate::oracle::Modes, addr: &[u8; 32]) -> Result<[u8; 32], u64> {
    crate::oracle::seal_address(&g.developer_pubkey, &g.hp, true, m, addr).ok_or_else(|| {
        push_console(b"kist stow: no system secret: cannot seal\n");
        STOW_WHY_SECRET
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
    let at = kk.console.pos();
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
    let (a, b) = kk.console.since(at);
    let out = Outcome { exit, console: [a, b].concat(), round_trips: kk.ipc_round_trips - trips0 };
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
            Grant::Items => (Object::Items, RIGHT_SEND),
            Grant::Kists => (Object::Kists, RIGHT_SEND),
            Grant::Stow => (Object::Stow, RIGHT_SEND),
            Grant::Hardware(n) => match HARDWARE.iter().position(|h| h.name.as_bytes() == *n) {
                Some(i) => (Object::Hardware(i), RIGHT_SEND | RIGHT_RECV),
                None => return Err("no such hardware block"),
            },
            Grant::Entropy => (Object::Entropy, RIGHT_SEND),
            Grant::Dma(b"surface") => (Object::NewSurface, RIGHT_SEND | RIGHT_RECV),
            // A GPU block of its own (GPU-KISTS.md plan step 1, 2026-11-12; frames of the pool since 2026-11-13): `layers` 128 MiB (Mahere's development spawn; the largest block of the old arena), `layers-32` 32 MiB (wallgen's, gputest's), each grown by its holder's ask (SYS_DMA x1 = 2); a block is never granted by name, only given.
            Grant::Dma(b"layers") => (Object::NewRegion(27), RIGHT_SEND | RIGHT_RECV),
            Grant::Dma(b"layers-32") => (Object::NewRegion(BLOCK_LEAST_LOG), RIGHT_SEND | RIGHT_RECV),
            Grant::Dma(b"gpu-block") => return Err("a GPU block is given (`layers`, `layers-32`), never named"),
            Grant::Dma(b"camera-view") => return Err("the camera's view is given by the camera driver at an open, never named"),
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

/// The heap a growing image starts with (ARCS.md step 2): 4 MiB, two frames, enough for ferros_rt's 1 MiB main stack, the TLS blocks and the first allocations; everything past it is asked for (SYS_ARC) as the heap runs out, up to the heap the build declared.
const HEAP_START: usize = 4 << 20;

/// Whether the image's heap grows on demand: bit 0 of its header's word at offset 4 (ferros_rt's `_entry` sets it since 2026-11-11; the pad word was 0 before, and `ferros_sys::entry!` leaves it 0). Its declared heap (`__ferros_heap`) is then a ceiling, not a reservation.
fn image_grows(code: &[u8]) -> bool {
    has_image_header(code) && code[4] & 1 == 1
}

/// The zeroed tail the kernel maps after an image at spawn: .bss plus 16 and the heap, or, for a growing image, the heap it starts with (`HEAP_START`, or less when it declared less).
fn image_tail(code: &[u8]) -> usize {
    if !has_image_header(code) {
        return 0;
    }
    let w = |at: usize| u64::from_le_bytes(code[at..at + 8].try_into().unwrap()) as usize;
    // Two words, summed here: .bss plus 16 (the linker script's own arithmetic) and the heap (a --defsym the script only forwards; summing them in the script took the default heap once, 2026-10-27).
    // WHY the min: both words are the image's, a gigabyte is past any arc the pool would give (an image declaring more is refused for room, not sized past the windows).
    (w(16) + image_heap_start(code)).min(1 << 30)
}

/// The heap an image starts with: its whole declared heap (header word 48), or for a growing image `HEAP_START` when it declared more. Never more than it declared, so the declared heap less this cannot wrap.
fn image_heap_start(code: &[u8]) -> usize {
    let declared = u64::from_le_bytes(code[48..56].try_into().unwrap()) as usize;
    // WHY the choice: a growing image starts with HEAP_START, never with more than its build declared.
    if image_grows(code) && declared > HEAP_START { HEAP_START } else { declared }
}

/// The zeroed tail at its most: .bss plus 16 and the whole declared heap (the arc's ceiling), up to the board's whole pool.
/// Until 2026-11-22 it was capped at 1 GiB, a cap carried over from `image_tail` (d2f14c2e, 2026-10-27, when an arc was one contiguous block of a boot-time pool and the whole tail was mapped at spawn) into the ceiling (50963897, 2026-11-11, ARCS step 2). For a growing image the ceiling maps nothing: it sizes the arc's window of addresses (`arc_alloc_room`) and bounds SYS_ARC's grows, every frame still asked for and refused when the pool has none. So the cap kept Lumis (a 4 GiB heap declared, std/lumis/build.rs) at about 1 GiB from the vault while a dev run had 4 GiB: its full-resolution integrator's 1.07 GB allocation failed on the Pro (2026-11-22, 15:14:43).
/// What the cap protected is kept: the sum of the image's two words cannot wrap (taken in 128 bits), and no window is sized past what the pool could ever give (`frames::map_bytes`), so an image declaring more is given a window it can never fill no larger than the pool, and its grows past the free frames are refused as they come (ARC_WHY_MEMORY).
fn image_tail_ceiling(code: &[u8]) -> usize {
    if !has_image_header(code) {
        return 0;
    }
    let w = |at: usize| u64::from_le_bytes(code[at..at + 8].try_into().unwrap()) as u128;
    // WHY the min: both words are the image's; past the pool's bytes no arc could hold it, and the window it sizes (a power of two of the 64 GiB arc space) would only spend addresses other arcs need. PROOF it is needed: without it an image declaring a heap near 2^64 makes `arc_alloc_room`'s `(ceiling + guard).next_power_of_two()` overflow, which the release kernel (no overflow checks) wraps to 0, so the window is one frame while the ceiling lets SYS_ARC grow the arc over the windows of the arcs after it; and one declaring a few hundred GiB takes a window no power of two of the arc space holds, so it is refused for room though the pool could run it.
    (w(16) + w(48)).min(crate::frames::map_bytes() as u128) as usize
}

/// A process's arc (its making is `arc_for`, the code's copy, then `build_from`; the copy with the kernel lock let go since 2026-11-18, `spawn`, `spawn_build`): the image's header read, the arc sized for its code, its zeroed tail, `inline` bytes of input, the stack's guard and the stack, taken from the pool (fixed mode's exact 2^K, exact mode's limit set). Under the kernel lock (the arc pool). The caller copies the code in at CODE.
fn arc_for(code: &[u8], inline: usize, masked: bool, fixed: u32) -> Result<Arc, &'static str> {
    // `inline`: the whole input lies in the arc (a bridge: pcie1 still reads input from its arc); sandboxed apps carry no input, so their arcs stay small either way. INLINE_INPUT becomes the cap once pcie1 streams firmware via SYS_INPUT (option 1).
    // No header, no spawn (owner, 2026-11-09): without it the .bss is unknown, and the arc sized from the file alone put the stack on the process's own statics (the display and bt, 2026-11-08). Every payload carries it (ferros_sys::entry!, tools/payload-check.sh).
    if !has_image_header(code) {
        return Err("spawn: the image has no header (ferros_sys::entry!), so its .bss is unknown");
    }
    let tail = image_tail(code);
    // `fixed` carries the mode word: bits 0-7 fixed mode's K (0: not fixed), bit 8 BTI (guarded code pages, `build_from`'s), bit 9 exact mode.
    let (fixed, exact) = (fixed & 0xFF, fixed >> 9 & 1 == 1);
    let need = CODE + code.len() + tail + inline + STACK_GUARD + STACK;
    // The most it may grow to (SYS_ARC): its whole declared heap; the same as `need` for an image whose heap does not grow.
    let ceiling = need - tail + image_tail_ceiling(code);
    // Fixed mode (SANDBOX.md, "Modes"): the arc is exactly the sealed 2^K, the size every `bfi` in the code assumes, and never grows.
    if fixed != 0 && need > 1 << fixed {
        return Err("spawn: the image does not fit its fixed arc");
    }
    spawn_step(7);
    let mut arc = if fixed != 0 { arc_alloc(1 << fixed, masked) } else { arc_alloc_room(need, ceiling, masked) }.ok_or("arc pool exhausted")?;
    spawn_step(8);
    // Exact mode: the limit is what the image needs, rounded to the 16 bytes the stack pointer must keep; the arc around it is the power of two the pool hands out, its rest the room the limit can grow into.
    if exact {
        arc.limit = (need + 15) & !15;
    }
    Ok(arc)
}

/// The process in an arc that already holds its code at the code offset (`spawn` and `spawn_build` copied it, with the kernel lock let go; a streamed spawn staged it there): the capability table, the grants page, the ihi, the preferences, the input past the code's zeroed tail, W^X, and the first thread.
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
    // The display server (the receiver of `display`) gets the GPU service's send right and a GPU block of its own after the slots it was given (GPU-COMPOSE.md 1.2, 2026-11-21: the compose on the GPU, a client with its own space), at init's spawn and at a keeper's restart alike: init's grants page is full, so it cannot hold them to pass on. The block is the least (32 MiB); the display grows it when it turns the GPU compose on.
    // With no GPU service (the GPU did not come to its reset at boot, pd.rs `gpu_fresh`; before 2026-11-22 none after any jump) both slots are empty, so a call of the display's never waits on a server that will not come, and the display composes on NEON alone.
    // Not init: it holds the receive right on `display` too (to hand on) and its page is full; with the two slots added its spawn was refused for more grants than the page holds, so a kernel with this rule started no init, no USB and no shell (the Pro, 2026-11-21, a RAM reload of 8f04f065: silent until a hand reset). So only a receiver of `display` that does not hold the blocks' window whole (init does, to hand blocks on; the display never), and only when the page has room for both, as every slot added after the given ones (`LINES_WITH`): a full page is skipped, never refused.
    let display = objects.iter().any(|o| matches!(o, (Object::Endpoint(e), r) if *e == endpoint(b"display") && r & RIGHT_RECV != 0)) && !objects.iter().any(|o| matches!(o.0, Object::Dma(i) if DMA[i].name == "arena")) && TOKENS + 4 + 32 * (objects.len() + 3) <= PREFS_HI;
    let gpu = endpoint(b"gpu");
    let served = k().procs.iter().flatten().any(|q| !q.dying && q.cspace.iter().any(|c| matches!(c.object, Object::Endpoint(e) if e == gpu) && c.rights & RIGHT_RECV != 0));
    let extra = if served { [(Object::Endpoint(gpu), RIGHT_SEND), (Object::NewRegion(BLOCK_LEAST_LOG), RIGHT_SEND | RIGHT_RECV)] } else { [(Object::Empty, 0), (Object::Empty, 0)] };
    for &(object, rights) in objects.iter().chain(&extra[..if display { 2 } else { 0 }]) {
        let object = match object {
            Object::NewSurface => match surface_take(epoch) {
                Ok(s) => Object::Dma(surface0() + s),
                // A spawn refused after a surface was given (a second surface, or the grants page full below) gives it back: no process of that epoch will ever end to free it, and a surface held by nobody is lost until a reboot.
                Err(why) => {
                    surfaces_free(epoch);
                    regions_free(epoch);
                    pucks_free(epoch);
                    return Err(why);
                }
            },
            // A GPU block: 2^n bytes, or the largest power of two down to 32 MiB the pool can give, or an empty slot (the client then has no GPU memory and draws on the CPU, as without the GPU service).
            Object::NewRegion(n) => {
                let mut got = Err("no size asked");
                for log in (BLOCK_LEAST_LOG.min(n)..=n).rev() {
                    got = block_take(epoch, 1 << log);
                    if got.is_ok() {
                        break;
                    }
                }
                match got {
                    Ok(j) => Object::Dma(gpub0() + j),
                    Err(why) => {
                        let line = alloc::format!("spawn: no GPU block ({why}; A#{} MiB of the pool free): an empty slot for the region\n", (crate::frames::free() * crate::frames::FRAME) >> 20);
                        k().console.extend_from_slice(line.as_bytes());
                        Object::Empty
                    }
                }
            }
            // A puck's square: a free slot of the pucks' window, no frames until it asks. None free: the spawn is refused, as for a surface.
            Object::NewPuck => match puck_take(epoch) {
                Ok(j) => Object::Dma(puck0() + j),
                Err(why) => {
                    surfaces_free(epoch);
                    regions_free(epoch);
                    pucks_free(epoch);
                    return Err(why);
                }
            },
            // The GPU mailbox's second surface: only beside a surface of its own already given, and only while a slot and the frames for another stay free for the next app's window.
            Object::SpareSurface => {
                let slots = unsafe { &*(&raw const SLOTS) };
                let free = slots.iter().filter(|s| s.owner == 0 && !s.held).count();
                let has = cspace.iter().any(|c| matches!(c.object, Object::Dma(i) if DMA[i].name == "surface"));
                match (has && free >= 2 && crate::frames::free() >= 2 * SURFACE_FRAMES).then(|| surface_take(epoch).ok()).flatten() {
                    Some(s) => Object::Dma(surface0() + s),
                    None => Object::Empty,
                }
            }
            o => o,
        };
        cspace.push(CapEntry { token: mint(object, rights, epoch), object, rights });
    }
    // PCIe channel 1's interrupt goes with its controller (2026-11-11): a process granted the controller's registers (pcie1-elbi) gets the line as the slot after its last, as a child handed Settings gets the torch (SYS_SPAWN). init holds every grant to hand on and its grants page is full, so it cannot hold the line to pass on; and the keepers' restart list (service.rs `shell_grants`) must give a restarted driver what a booted one has. Only when the page has room: init, full, is skipped rather than refused.
    // Since 2026-11-13 the same for every line in `LINES_WITH`: the modem's MSI and wakeup with pcie0-elbi, NFC's with hsi2c0, Bluetooth's UART with its UART.
    for (hw, lines) in LINES_WITH {
        if !cspace.iter().any(|c| matches!(c.object, Object::Hardware(h) if HARDWARE[h].name == *hw)) {
            continue;
        }
        for line in lines.iter() {
            if !cspace.iter().any(|c| matches!(c.object, Object::Irq(i) if IRQS[i].name == *line)) && TOKENS + 4 + 32 * (cspace.len() + 1) <= PREFS_HI {
                if let Some(i) = IRQS.iter().position(|l| l.name == *line) {
                    let object = Object::Irq(i);
                    cspace.push(CapEntry { token: mint(object, RIGHT_SEND, epoch), object, rights: RIGHT_SEND });
                }
            }
        }
    }
    // The AoC's a32 mailbox line goes with the Sensors right the same way (2026-11-12): the sensors process blocks on it until com.google.usf rings, instead of waking every 2 ms to look (payloads/sensors). The slot after its last; init, full, is skipped.
    if cspace.iter().any(|c| c.object == Object::Sensors) && !cspace.iter().any(|c| matches!(c.object, Object::Irq(i) if IRQS[i].name == "aoc-mbox-a32")) && TOKENS + 4 + 32 * (cspace.len() + 1) <= PREFS_HI {
        if let Some(i) = IRQS.iter().position(|l| l.name == "aoc-mbox-a32") {
            let object = Object::Irq(i);
            cspace.push(CapEntry { token: mint(object, RIGHT_SEND, epoch), object, rights: RIGHT_SEND });
        }
    }
    // The motion mailbox's writer's region goes with the Sensors right too, the slot after the line (2026-11-19, docs/SENSORS.md "The motion mailbox"): the sensors process publishes gravity and the field there for any number of readers, and init's page has no slot left to hand it.
    let motion = mbox0() + ferros_mailbox::MOTION;
    if cspace.iter().any(|c| c.object == Object::Sensors) && !cspace.iter().any(|c| c.object == Object::Dma(motion)) && TOKENS + 4 + 32 * (cspace.len() + 1) <= PREFS_HI {
        let object = Object::Dma(motion);
        cspace.push(CapEntry { token: mint(object, RIGHT_SEND | RIGHT_RECV, epoch), object, rights: RIGHT_SEND | RIGHT_RECV });
    }
    // The thermometer's mailbox's writer's region the same way, the slot after motion's (2026-11-20, docs/SENSORS.md "The thermo mailbox"): the sensors process publishes the MLX90632's temperatures there.
    let thermo = mbox0() + ferros_mailbox::THERMO;
    if cspace.iter().any(|c| c.object == Object::Sensors) && !cspace.iter().any(|c| c.object == Object::Dma(thermo)) && TOKENS + 4 + 32 * (cspace.len() + 1) <= PREFS_HI {
        let object = Object::Dma(thermo);
        cspace.push(CapEntry { token: mint(object, RIGHT_SEND | RIGHT_RECV, epoch), object, rights: RIGHT_SEND | RIGHT_RECV });
    }
    // The tokens end at PREFS_HI (G#FB0, the second preferences block, then the first at G#FC0 and the ihi at G#FE0): a 109th slot's token was once written over the preferences and overwritten by the next preferences write, so init's spawn of the GPU service was refused for a token it held (2026-11-05). Refused here, with the reason, rather than corrupted.
    if TOKENS + 4 + 32 * cspace.len() > PREFS_HI {
        surfaces_free(epoch);
        regions_free(epoch);
        pucks_free(epoch);
        return Err("spawn: more grants than the grants page holds (108 slots and the console)");
    }
    // A line that stormed under the process before is this one's to arm again (`irq_ack_guarded`).
    for c in cspace.iter() {
        if let Object::Irq(i) = c.object {
            irq_guard_reset(i);
        }
    }
    let zero = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0, v: [0; 32], fpcr: 0, fpsr: 0, tpidr: 0, _pad: 0 };
    // An empty slot's token is zeros: a want said no to reads as no grant at all.
    for c in cspace.iter_mut().filter(|c| c.object == Object::Empty) {
        c.token = [0; 32];
    }
    let domain = ihi.map(|i| blake3::derive_key(KIST_DOMAIN_CONTEXT, &i));
    // The register grants by slot, for `mmio_fast`: HARDWARE has fewer than 255 blocks, so an index plus one fits a byte.
    let hw = cspace.iter().map(|c| match c.object { Object::Hardware(g) => g as u8 + 1, _ => 0 }).collect();
    let fast = alloc::boxed::Box::new(Grants { hw, ended: core::sync::atomic::AtomicBool::new(false) });
    // The hold's right as a development spawn has it (`--items`: any kind); a kist's is set from its capsule's declaration once it is built (`spawn_kist`).
    let items = cspace.iter().any(|c| c.object == Object::Items).then(|| alloc::boxed::Box::new(ItemRight { any: true, makes: Vec::new(), opens: Vec::new() }));
    let p = Process { arc, cspace, epoch, dying: false, name: name.to_vec(), line_start: true, con: ferros_console::KERNEL, words: Vec::with_capacity(WORDS_MAX), input: input.to_vec(), born: crate::platform::cntpct(), pets: 0, last_pet: 0, gap_max: 0, gap_at: 0, start: 0, pet_thread: usize::MAX, paused_at: 0, paused: 0, domain, kist: false, gate: None, facets: 0, capsule: None, items, floor: arc.top() - arc.base, grows: 0, shrinks: 0, hold: None, moves: 0, holes: Vec::new(), guards: Vec::new(), fast, text_end: CODE + exec_len, panicked: None };
    PROCS_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    vault_spawned(objects);
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
    crate::cachemap::set_arc_wx(arc.base, arc.size, arc.base + CODE, arc.base + CODE + exec_len, !masked, bti);
    // The grants page read-only (the handle table, the tokens, the preferences, the ihi): nothing in it is the process's to write.
    crate::cachemap::grants_page(arc.base, false);
    // The stack's guard: the page under it (`floor`, checked above to lie past the image, its tail and the input).
    crate::cachemap::guard_pages(floor, STACK_GUARD);
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
    // Its surface's owner alive from here: the display's bell says so before the process can show a window on it.
    surfaces_ring();
    // And its square's, a puck's (2026-11-18, the 8a: `puck_take` rang the squares' bell before the process was in the table, so the newest puck's slot read as nobody's and the display drew nothing for it until another square came or went: only basecalc showed, the clock did not).
    pucks_ring();
    // A masked arc's power-of-two slack (ARCS.md step 4): the frames wholly between its image (code, zeroed tail, input) and its first stack's guard page hold nothing it was built with, and masked code reaches only what it was linked with (its statics, its static heaps, its stacks), so they go back to the pool at once (ahau: 20 MiB of its 128). Not exact mode's, whose limit may grow into it; a native arc is whole frames already.
    // Not given back at spawn (2026-11-11, the Pro's first ARCS boot: ahau's thread 7 wrote at arc offset G#760_0000, inside the 18 MiB released here, and faulted, so the device had no attest screen): a masked no_std process's allocator grows into exactly this span, so it is not slack. A process gives back what it knows it no longer needs itself (SYS_ARC op 3).
    let _ = (masked, floor);
    let me = add_thread(Thread { proc: pi, frame, state: State::Runnable, mask: MASK_WORKERS, called: 0, what: 0, ran: 0, ran_at: 0, woke: 0, wkind: 0, irq_at: 0, irq_line: 0, irq_core: 0, irq_noticed: 0, device: false, urgent: false, on_irq: false, run_line: 0, irq_run_at: 0, last_core: 0, wait_call: false, label: [0; 16] });
    Ok((me, arc, in_off))
}

/// `SYS_SPAWN`'s work: the capsule by vault key thru the gate (and, masked, its seal), then a process holding `objects`, queued to run. Ok(None): `once`, and a live process of that name started meanwhile.
/// Called with the kernel lock held (a syscall's), which it lets go for all that is not the tables (2026-11-18, `unlocked`): the capsule's and the seals' reads (under the vault's lock: 24 to 148 ms for a capsule, `diag lock`), the gate, the seal and the ihi (signatures and hashes over the capsule, about 7 ms), and the code's copy into its arc. It holds the kernel lock again for the arc's taking and the process's making (`arc_for`, `build_from`): the arc pool and the tables.
/// The caller's `p` and `t` are used by neither it nor the caller after it.
fn spawn_queued(name: &[u8], key: &[u8], objects: &[(Object, u32)], once: bool) -> Result<Option<usize>, &'static str> {
    let read = unsafe { CAPSULES }.ok_or("spawn: no vault to read capsules from")?;
    let t0 = crate::platform::cntpct();
    // Read and checked with the lock let go: nothing here touches the kernel's tables (the vault has its own lock, the gate and the seal work on the heap's copies, and a seal still to move is queued under the kernel lock taken for that alone, `seal_ok`).
    let checked = unlocked(|| -> Result<(Vec<u8>, crate::oracle::Gated, [u8; 32], u64), &'static str> {
        spawn_step(1);
        let capsule = read(key).ok_or("spawn: no such capsule in the vault")?;
        spawn_step(2);
        // The seals list, a `seals` document since 2026-11-17 (service.rs `seals_read`): one that does not read is none here, and a masked image is refused as not installed (the flash log says why since the vault's open).
        let sealed = read(crate::kvault::SEALED.as_bytes()).and_then(|d| ferros_kist::seal::list_of(&d)).unwrap_or_default();
        let t_read = crate::platform::cntpct();
        spawn_step(3);
        let g = crate::oracle::gate(&capsule)?;
        spawn_step(4);
        let ihi = spawn_checked(&g, &capsule, &sealed)?;
        Ok((capsule, g, ihi, t_read))
    });
    let r = match &checked {
        Ok((_, g, ihi, _)) => spawn_build(name, g, *ihi, objects, once),
        Err(e) => Err(*e),
    };
    // What a spawn costs, by part, on the console (2026-11-12, measuring attest to the deck).
    let (len, t_read) = match &checked {
        Ok((c, _, _, t)) => (c.len(), *t),
        Err(_) => (0, crate::platform::cntpct()),
    };
    let us = |a: u64, b: u64| b.wrapping_sub(a) * 1_000_000 / crate::platform::cntfrq();
    let t_end = crate::platform::cntpct();
    let kk = k();
    // On a line of its own: a process's line still open (no newline yet) is broken here rather than run into.
    if kk.console.last().is_some_and(|c| c != b'\n') {
        kk.console.push(b'\n');
    }
    kk.console.extend_from_slice(b"t");
    push_dec(&mut kk.console, t_end / (crate::platform::cntfrq() / 1000));
    kk.console.extend_from_slice(b" kernel: spawned ");
    kk.console.extend_from_slice(name);
    kk.console.extend_from_slice(b": capsule A#");
    push_dec(&mut kk.console, len as u64);
    kk.console.extend_from_slice(b" bytes read in A#");
    push_dec(&mut kk.console, us(t0, t_read));
    kk.console.extend_from_slice(b" us, gate and build A#");
    push_dec(&mut kk.console, us(t_read, t_end));
    kk.console.extend_from_slice(if r.is_ok() { b" us\n".as_slice() } else { b" us (refused)\n" });
    r
}

/// The decimal digits of `v` onto the console as the kernel's, for lines built under the lock without the formatter.
fn push_dec(out: &mut ferros_console::ConRing, v: u64) {
    out.dec(ferros_console::KERNEL, v);
}

/// A spawn's step into this core's crumb (platform `crumb`, kept in the report page across a warm reset): SYS_SPAWN's entering crumb G#9A with the step in bits 24-31 (1 the capsule's read, 2 read, 3 the seal list read, 4 gated, 5 sealed, 6 ihi, 7 the arc asked, 8 the arc given, 9 the code copied, 10 built, 11 queued), so a spawn that hangs names its step at the next boot as PREV_EXIT and the core's PREV_NOTE (2026-11-16: kernel #3 hung inside a spawn with the lock held, right after the person attested).
/// And while the kernel heap's largest free block is under 2^22 bytes, each step goes to the flash log too, with that block: a spawn short of heap names itself whatever kind of hang follows.
fn spawn_step(step: u64) {
    crate::platform::crumb(0x90 + SYS_SPAWN | step << 24);
    let largest = crate::heap_largest_free();
    if largest < 1 << 22 {
        crate::flashlog::line(&alloc::format!("spawn step A#{step}: the kernel heap's largest free block A#{largest} bytes"));
    }
}

/// `spawn_queued` past the gate, with the kernel lock let go: the seal of a masked capsule, then the ihi.
fn spawn_checked(g: &crate::oracle::Gated, capsule: &[u8], sealed: &[u8]) -> Result<[u8; 32], &'static str> {
    if g.masked {
        // The address of the capsule the gate just copied the code out of, in the kernel heap where nothing else writes.
        seal_ok(sealed, &g.developer_pubkey, &g.hp, &crate::oracle::modes_of(g), blake3::hash(capsule).as_bytes(), &g.code)?;
    } else if cfg!(ferros_production) {
        return Err("gate: production refuses unmasked native code");
    }
    spawn_step(5);
    let ihi = crate::oracle::ihi(&g.developer_pubkey, &g.hp).ok_or("no ira: no system secret, no ihi")?;
    spawn_step(6);
    Ok(ihi)
}

/// Whether a live process (not ending) has this name. Under the kernel lock.
fn named_live(name: &[u8]) -> bool {
    k().procs.iter().flatten().any(|q| q.name == *name && !q.dying)
}

/// `spawn_queued`'s process from its checked image, the kernel lock held on entry and on return: the arc taken (`arc_for`), the code copied into it with the lock let go (the arc is no process's yet, so nothing else reaches it), then the process made in it and queued (`build_from`). Ok(None) when `once` and a process of that name came alive meanwhile.
fn spawn_build(name: &[u8], g: &crate::oracle::Gated, ihi: [u8; 32], objects: &[(Object, u32)], once: bool) -> Result<Option<usize>, &'static str> {
    if once && named_live(name) {
        return Ok(None);
    }
    cap_key().ok_or("no wairua: refusing to mint capabilities")?;
    let arc = arc_for(&g.code, 0, g.masked, g.fixed | (g.exact as u32) << 9)?;
    unlocked(|| {
        // SAFETY: the arc was just taken for this image (`arc_for` sized it past CODE plus the code's length) and is no process's until `build_from` below, so nothing else reads or writes it meanwhile.
        unsafe { core::ptr::copy_nonoverlapping(g.code.as_ptr(), (arc.base + CODE) as *mut u8, g.code.len()) };
        spawn_step(9);
    });
    // A spawn of the same name may have run its whole course while the copy did: still one at a time under `once`.
    if once && named_live(name) {
        arc_free(arc);
        return Ok(None);
    }
    // A refused build made nothing that holds the arc (its refusals all come before the process is in the table): it goes back to the pool.
    let (me, _, _) = build_from(name, arc, g.code.len(), b"", objects, Some(ihi), g.masked, g.text_len, g.bti).inspect_err(|_| arc_free(arc))?;
    spawn_step(10);
    enqueue_back(me);
    spawn_step(11);
    Ok(Some(me))
}

/// A granted region as a handle (SANDBOX.md, "Handles"): its base and its mask, for masked code to reach it as `base + (offset & mask)`. A region is maskable when it is a power of two in size and aligned to it; an MMIO block of any other size (20 KiB, 52 KiB, 3.5 MiB) gets no handle and is reached through the kernel's mmio calls instead, never rounded up over its neighbours. A grant that is not a region, or a region that cannot be masked, is (0, 0): native code reaches it as it always has, masked code cannot.
fn handle_of(object: Object) -> (u64, u64) {
    let (base, size) = match object {
        Object::Dma(i) => region(i),
        Object::Hardware(h) if !HARDWARE[h].via_el3 => (HARDWARE[h].base, HARDWARE[h].size),
        _ => return (0, 0),
    };
    // Exact or nothing (ISOMEM Claim 5: a grant reaches its own bytes and no neighbour's): a region the mask can bound exactly is a power of two at a base aligned to it. Rounding a 20 KiB block up to 32 KiB would hand the process the next block's registers too; such a block is reached through the kernel's mmio calls, which check each offset.
    if size == 0 || !size.is_power_of_two() || base % size != 0 {
        return (0, 0);
    }
    (base as u64, (size - 1) as u64)
}

/// `diag arcs`: every process's arc, under the lock: name, base (its window's address), size, frames mapped, window; then every surface held; and the frame pool's state after them.
pub fn arcs_report() -> alloc::string::String {
    lock();
    let mut s = alloc::string::String::new();
    for p in k().procs.iter().flatten() {
        let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{} base G#{:X} size G#{:X} frames A#{} ceiling G#{:X} window G#{:X} {} grew A#{} shrank A#{} moved A#{} gave back A#{} MiB inside\n", core::str::from_utf8(&p.name).unwrap_or("?"), p.arc.base, p.arc.size, p.arc.mapped / crate::frames::FRAME, p.arc.ceiling, p.arc.window, if p.arc.guard != 0 { "masked" } else { "native" }, p.grows, p.shrinks, p.moves, p.holes.iter().map(|h| h.1 - h.0).sum::<usize>() >> 20));
    }
    surfaces_report(&mut s);
    blocks_report(&mut s);
    pucks_report(&mut s);
    mailboxes_report(&mut s);
    s.push_str(&crate::frames::pool_report());
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

/// Zero `count` blocks of the ferros partition from block `at` of it, a run at a time through the run bounce, the watchdogs fed at every run: the owner's fresh start (service.rs `vault fresh`).
/// Whether every run was written; it stops at the first refusal.
/// Under the UFS lock (`UFS`) for the whole of it, so no other UFS command runs between the runs or writes the bounce they share.
pub fn zero_blocks(at: u64, count: u64) -> bool {
    UFS.with(|| zero_blocks_held(at, count))
}
fn zero_blocks_held(at: u64, count: u64) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let Some((first, blocks)) = crate::ufs_device::part() else { return false };
    // WHY the bound: the regions are the kernel's own constants, but the partition is the GPT's; a region past its end would zero whatever follows the partition.
    // PROOF of no overflow: at and count are below 2^32 (block numbers of a partition the GPT gives as u32).
    if at + count > blocks as u64 {
        return false;
    }
    let u = ferros_hal::ufs::UfsController::new(0x1320_0000);
    let run = (&raw mut UFS_RUN) as usize;
    if ferros_hal::ufs::PRDT_STRIDE.load(Relaxed) == 0 {
        u.probe_prdt_stride(0, (first as u64 + at) as u32, run);
    }
    // After the probe, which uses the bounce as its scratch.
    unsafe { core::ptr::write_bytes(run as *mut u8, 0, ferros_hal::ufs::RUN_MAX << 12) };
    let mut k = 0u64;
    while k < count {
        // WHY the smaller: the last run is what is left of the region, the others BLOCK_MAX.
        let n = if count - k < BLOCK_MAX { count - k } else { BLOCK_MAX };
        crate::platform::pet_watchdogs();
        if !u.write_run_lun(0, (first as u64 + at + k) as u32, n as usize, run) {
            return false;
        }
        k += n;
    }
    true
}
