//! The resident service (GPU.md "The resident service"; GPU-KISTS.md plan step 1, 2026-11-12, and "The queue", 2026-11-20): every client's jobs in a group and an address space of its own, which map only the memory the kernel's stamp says the client holds (its GPU block at `ARENA_VA`, its surfaces at their place from `SURFACES_VA`) and the service's few pages for that client (its command stream and descriptors read-only, its code region read-only and the only executable pages, its sequence page).
//! No address a client names reaches another client's memory or the service's: it is in no table of the client's space, the GPU MMU faults the access there, and the kernel's fence (s2mpu::fence_gpu) holds every space inside gpu-mem, the blocks and the surfaces.
//!
//! Two ways in, one path: a call (op 1, a job run and answered, as since 2026-11-05) and a queue (ops 2 to 4, ferros_gpuq: a client's jobs submitted from its own thread into a ring in its own block, no call a job).
//! Both write the same command-stream words (ferros_gpuq `job_words`) into the client's own stream, end with the same sequence word, and are ended by the same thread at the GPU's job interrupt (`events`).
//!
//! The threads:
//! - the main thread takes the calls (`serve`): a run, a kernel loaded, a queue opened or closed.
//! - one thread a slot (`slot_thread`) waits on its client's queue's `SUBMIT` and writes each new job into the client's stream.
//! - `events` waits on the GPU's job interrupt: each slot's jobs ended in order, their results written, the client's `DONE` woken; a fault, a command-stream fault or a guard passed ends that client's group and its queue, no other.
//! - `clock` turns the `gpu-clock` switch on the kernel's core: up when a job is queued, down when none is (the owner, 2026-11-11: "the GPU powers up when work is queued and gates when its queue empties, as events, never a timer").
//! - `mmu_watch` (main.rs) as before: a client's page fault noted and its space let go.
//!
//! Requests on `gpu`: the kernel's stamp (ferros_sys::gpu `Stamp`), then
//! - `[1][preload u16, bit 15 = 64 registers a thread][code at an offset into the client's own block u32][code words u16][groups x u32][groups y u32][uniforms n u8, at most 64][n × u64]`, then optionally `[n u8][the client's name][n u8][the kernel's name]` (`label`): a run. The reply `[status][µs u32]`: 0 done (then `[the GPU's own µs u32]` when it stamped the job), 1 a request it cannot take (no stamp, no block, its code outside its block, no slot free), 2 the job faulted or did not finish in its guard, 3 its code starts with a zero word, 4 (`OUTSIDE`) it reached outside the client's memory, then `[the GPU address u64][AS_FAULTSTATUS u32]`, 5 (`UNCHECKED`) a kist's job on a production build.
//! - `[2][preload u16][code at an offset into its block u32][code words u16]`: a kernel loaded for its queue, `[0][handle u8]`, or `[1]` (no room: 16 kernels, 2^14 words a client).
//! - `[3][queue at an offset into its block u32][depth, log2 u8]`: its queue opened (ferros_gpuq), `[0]`, or `[1]`.
//! - `[4]`: its queue closed, its jobs still in it stopped and lost, `[0]`.

use super::*;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::mem::rd64;
use ferros_gpuq as q;
use ferros_sys::gpu::{is_block, is_camera, Key, Stamp, CAMERA_VA, OUTSIDE, SURFACES_PA, UNCHECKED};
use ferros_sys::BELL_GPU_CAMERA;

/// Each slot's command stream: 4 pages, room for the deepest queue's jobs and a call's (ferros_gpuq `JOB_WORDS_MOST` words a job, padded to 64 bytes: 17 jobs in flight at most, 4352 bytes).
const RING_PAGES: usize = 4;
/// Each slot's loaded kernels (op 2): their code after the calls' 32 pages, 32 pages more, and their shader program descriptors in the descriptor page from `SPD_AT`, 32 bytes each.
const KERNEL_PAGES: usize = 32;
const KERNELS_MOST: usize = 1 << 4;
const SPD_AT: usize = 0x400;
/// Each loaded kernel's thread storage descriptor (genxml v11 "Local Storage", 32 bytes, 64-aligned) in the descriptor page: its workgroup storage's place and size (2026-11-21), or none.
const TSD_AT: usize = 0x800;
/// In a slot's sequence page (mapped at `OUT_VA`): the word each job's stream sets to its sequence at its end, and the line a call's stamps land in (its start at + 8, its end at + 16).
const SEQ_AT: usize = 0x240;
const CALL_STAMPS: usize = 0x278;
/// A level-2 block of 2 MiB, read-only (BLOCK_RW with AP's read-only bit, as PAGE_RO is PAGE_RW's): a camera view's frames (2026-11-20).
const BLOCK_RO: u64 = 0x4C1;

/// What the threads share, fixed before they start.
struct Svc {
    g: Gpu,
    sh: Shared,
    cores: u64,
    max_threads: u32,
    grants: u64,
    irq: u64,
    surfaces_ok: bool,
    /// Whether the GPU stamps a job's start and end (`STORE_STATE`: its stream has the registers, and the GPU's timestamp runs), and its timestamp's lead on the CPU's counter.
    stamps: bool,
    stamp_offset: u64,
}

/// A job in a slot's stream, oldest first: its sequence, the client's job number (`CALL` for a call), whether it was refused (no words written: it ends as it reaches the head), and when its doorbell rang.
#[derive(Clone, Copy, Default)]
struct Flight {
    seq: u32,
    job: u64,
    refused: bool,
    submitted: u64,
}
const CALL: u64 = u64::MAX;
/// The most a slot holds in flight: a queue's deepest and a call, rounded up.
const FLIGHT: usize = 1 << 5;

/// A client's queue as the service holds it: the window address of its start (its `SUBMIT` word), its GPU address in the client's space, its depth, and the next job number to take.
#[derive(Clone, Copy)]
struct Queue {
    cpu: usize,
    gpu: u64,
    depth_log2: u32,
    taken: u64,
}

impl Queue {
    fn ring(&self) -> q::Ring<'static> {
        // SAFETY: the queue lies in a block the kernel keeps held while the service holds its client's epoch (process.rs `gpu_hold`): mapped at `cpu` in the blocks' window for this process, `q::bytes(depth_log2)` long (checked at the open), 8-aligned; every word an atomic.
        let words = unsafe { core::slice::from_raw_parts(self.cpu as *const AtomicU64, q::bytes(self.depth_log2) / 8) };
        q::Ring::new(words, self.depth_log2).unwrap()
    }
}

/// A client's slot: its own group, address space, stream, descriptors, sequence page, code region and page-table pages, all in gpu-mem; and the client bound to it, by the kernel's stamp.
struct Slot {
    mem: SlotMem,
    ring: usize,
    desc: usize,
    flag: usize,
    code: usize,
    /// Its two pools of table pages (`tables`), and which holds the tables in use.
    pools: [usize; 2],
    cur: usize,
    /// What its client's last stamp named (`bind`), kept to make its tables again: the regions, and the frames of those in a window, in order.
    regions: [(u64, u64); ferros_sys::gpu::REGIONS],
    nregions: usize,
    frames: [u64; ferros_sys::gpu::FRAMES],
    nframes: usize,
    /// Its client's camera view is mapped in its space; and mapped cacheable on the GPU, every job ending with the GPU's caches invalidated (`q::END_FLUSH`; the default since 2026-11-20; op 5's byte 0 maps it non-cacheable, the measure).
    camera: bool,
    camera_cached: bool,
    /// Its address space's root while a client is bound.
    root: usize,
    insert: usize,
    client: Option<Key>,
    /// Its client's block (its window base and bytes when bound).
    block: (u64, u64),
    kist: bool,
    /// When it last took a job (a count of jobs), for choosing the slot used longest ago.
    used: u64,
    /// Its group is running (started, not ended since).
    running: bool,
    /// Kernels loaded for its client (op 2) and the code words they take.
    kernels: usize,
    code_used: usize,
    /// Each loaded kernel's workgroup (2^n invocations, 4 a warp) and whether it has workgroup storage (no merging then); its thread storage descriptor at `TSD_AT` + 64 a kernel.
    kern_wg: [u8; KERNELS_MOST],
    kern_wls: [bool; KERNELS_MOST],
    queue: Option<Queue>,
    /// The next sequence a job of this slot's ends with, and its jobs in flight, `head` the oldest.
    seq: u32,
    flight: [Flight; FLIGHT],
    head: usize,
    len: usize,
    /// When the oldest in flight became the oldest: its guard counts from here.
    head_since: u64,
}

impl Slot {
    const EMPTY: Slot = Slot { mem: SlotMem { susp_va: 0, uio_va: 0, uio: 0 }, ring: 0, desc: 0, flag: 0, code: 0, pools: [0; 2], cur: 0, regions: [(0, 0); ferros_sys::gpu::REGIONS], nregions: 0, frames: [0; ferros_sys::gpu::FRAMES], nframes: 0, camera: false, camera_cached: true, root: 0, insert: 0, client: None, block: (0, 0), kist: false, used: 0, running: false, kernels: 0, code_used: 0, kern_wg: [4; KERNELS_MOST], kern_wls: [false; KERNELS_MOST], queue: None, seq: 1, flight: [Flight { seq: 0, job: 0, refused: false, submitted: 0 }; FLIGHT], head: 0, len: 0, head_since: 0 };
}

struct State {
    slots: [Slot; SLOTS],
    jobs: u64,
    /// Jobs in flight in every slot: the clock's want (`WANT`).
    inflight: usize,
    /// A call's job ended (`events` to the main thread): its status, the service's µs, the GPU's µs, the fault's address and status.
    call: (u8, u32, u32, u64, u32),
}

static mut SVC: Option<Svc> = None;
/// Filled in place by `serve` before any thread starts (a slot is about 6 KiB: seven built on the main thread's stack and moved would be 40 KiB of it).
static mut ST: State = State { slots: [const { Slot::EMPTY }; SLOTS], jobs: 0, inflight: 0, call: (0, 0, 0, 0, 0) };
/// The lock over `ST` and the firmware's shared interface: three states, Drepper's futex mutex (0 free, 1 held, 2 held with waiters).
static LOCK: AtomicU64 = AtomicU64::new(0);
/// Each slot thread's doorbell while its slot has no queue: moved when one opens there.
static SLOT_BELL: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// The main thread's doorbell for its call's job: moved by `events` when the job ends.
static CALL_BELL: AtomicU64 = AtomicU64::new(0);
/// The clock's want, 1 while any job is in flight: `clock` turns the switch to it.
static WANT: AtomicU64 = AtomicU64::new(0);
/// Jobs put in flight since the service began (`fly`): the clock's look at each display frame tells "nothing since the last frame" by it not moving.
static ACTIVITY: AtomicU64 = AtomicU64::new(0);
/// The clock held up whatever the queues hold (op 6, a development measure: the frame's cost of the turns, docs/GPU-KISTS.md "Power and clock by events").
static CLOCK_HOLD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

static mut EVENTS_STACK: [u8; 1 << 14] = [0; 1 << 14];
static mut CLOCK_STACK: [u8; 1 << 13] = [0; 1 << 13];
static mut CAMERA_STACK: [u8; 1 << 14] = [0; 1 << 14];
/// The camera's bell word (`camera_watch`): the kernel writes its count here and wakes it.
static CAMERA_BELL: AtomicU64 = AtomicU64::new(0);
static mut SLOT_STACKS: [[u8; 1 << 14]; SLOTS] = [[0; 1 << 14]; SLOTS];

fn svc() -> &'static Svc {
    // SAFETY: set once by `serve` before any thread that reads it starts, never written again.
    unsafe { (*(&raw const SVC)).as_ref().unwrap() }
}

fn lock() {
    if LOCK.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        return;
    }
    while LOCK.swap(2, Ordering::Acquire) != 0 {
        ferros_sys::wait(&LOCK, 2, 0);
    }
}

fn unlock() {
    if LOCK.swap(0, Ordering::Release) == 2 {
        ferros_sys::wake(&LOCK, 1);
    }
}

/// `f` over the state with the lock held.
fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    lock();
    // SAFETY: `ST` is set before any thread starts and touched only here, under the lock.
    let r = f(unsafe { &mut *(&raw mut ST) });
    unlock();
    r
}

/// Slot `k`'s group's blocks in the shared interface: CSG input and output, stream 0's input and output (GPU.md, ferros-csf-spec.md: group n at G#0400_1000 + n · G#A0).
fn blocks(sh: &Shared, k: usize) -> (u32, u32, u32, u32) {
    let gctl = 0x0400_1000u32 + 0xA0 * k as u32;
    (sh.rd(gctl + 4), sh.rd(gctl + 8), sh.rd(gctl + 0x40 + 4), sh.rd(gctl + 0x40 + 8))
}

/// GLB_DB_REQ's bit `bit` toggled against GLB_DB_ACK, then doorbell 0: the firmware looks at that group's request.
fn glb_doorbell(g: &Gpu, sh: &Shared, bit: u32) {
    let (gin, gout) = (sh.rd(0x0400_0008), sh.rd(0x0400_000C));
    sh.wr(gin + 0x08, (sh.rd(gin + 0x08) & !bit) | ((sh.rd(gout + 0x08) ^ bit) & bit));
    g.doorbell(0);
}

/// The command-stream group of slot `k` (group k, address space k + 1, doorbell k + 1), started as kbase starts one (program_cs, program_csg_slot): its stream's ring (`RING_PAGES`) and user-IO pages, every ack interrupt, START; the group's cores, endpoints and `priority`, its address space, its suspend buffer, START. False if the firmware did not acknowledge it in 100 ms. Under the lock.
fn group_start(g: &Gpu, sh: &Shared, k: usize, sl: &mut Slot, cores: u64, priority: u32) -> bool {
    let (cin, cout, sin, sout) = blocks(sh, k);
    let doorbell = k as u32 + 1;
    // A fresh queue: its user-IO pages zeroed (insert and extract at 0), as kbase gives a new queue.
    for w in 0..2 * PAGE / 8 {
        wr64(sl.mem.uio + 8 * w, 0);
    }
    sl.insert = 0;
    sh.wr(sin + 0x10, RING_VA as u32);
    sh.wr(sin + 0x14, (RING_VA >> 32) as u32);
    sh.wr(sin + 0x18, (RING_PAGES * PAGE) as u32);
    sh.wr(sin + 0x30, sl.mem.uio_va as u32);
    sh.wr(sin + 0x34, (sl.mem.uio_va >> 32) as u32);
    sh.wr(sin + 0x38, (sl.mem.uio_va + PAGE as u64) as u32);
    sh.wr(sin + 0x3C, ((sl.mem.uio_va + PAGE as u64) >> 32) as u32);
    sh.wr(sin + 0x04, doorbell << 8);
    sh.wr(sin + 0x0C, 0xFFFF_FFFF);
    let cs_ack = sh.rd(sout);
    let keep = 1 << 27 | 1 << 30 | 1 << 31;
    sh.wr(sin, (cs_ack & keep) | 1 << 10 | 1 << 8 | 1 << 12 | 1);
    unsafe { core::arch::asm!("dmb osh") };
    sh.wr(cin + 0x08, (sh.rd(cin + 0x08) & !1) | ((sh.rd(cout + 0x08) ^ 1) & 1));
    sh.wr(cin + 0x20, cores as u32);
    sh.wr(cin + 0x24, (cores >> 32) as u32);
    sh.wr(cin + 0x28, cores as u32);
    sh.wr(cin + 0x2C, (cores >> 32) as u32);
    sh.wr(cin + 0x30, g.rd64(TILER_PRESENT) as u32);
    sh.wr(cin + 0x34, cores.count_ones() | cores.count_ones() << 8 | 1 << 16 | priority << 28);
    sh.wr(cin + 0x38, 0);
    sh.wr(cin + 0x50, k as u32 + 1);
    sh.wr(cin + 0x40, sl.mem.susp_va as u32);
    sh.wr(cin + 0x44, (sl.mem.susp_va >> 32) as u32);
    sh.wr(cin + 0x48, 0);
    sh.wr(cin + 0x4C, 0);
    sh.wr(cin + 0x04, 0xFFFF_FFFF);
    sh.wr(cin, (sh.rd(cin) & !7) | 1);
    unsafe { core::arch::asm!("dmb osh") };
    glb_doorbell(g, sh, 1 << k);
    g.wait(100, |_| sh.rd(cout) & 7 == 1 && sh.rd(sout) & 7 == 1)
}

/// Slot `k`'s group ended (CSG_REQ's state TERMINATE, 0): whatever job it runs stops, kbase's way of ending a context's groups. False if the firmware did not acknowledge it in 100 ms. Under the lock.
fn group_end(g: &Gpu, sh: &Shared, k: usize) -> bool {
    let (cin, cout, _, _) = blocks(sh, k);
    sh.wr(cin, sh.rd(cin) & !7);
    unsafe { core::arch::asm!("dmb osh") };
    glb_doorbell(g, sh, 1 << k);
    g.wait(100, |_| sh.rd(cout) & 7 == 0)
}

/// The GPU's L2 and its cores' load-store caches cleaned and invalidated, the rest invalidated (GPU_COMMAND FLUSH_CACHES: panthor `GPU_FLUSH_CACHES(CACHE_CLEAN | CACHE_INV, CACHE_CLEAN | CACHE_INV, CACHE_INV)`, G#2_3304), CLEAN_CACHES_COMPLETED (GPU_IRQ bit 17) waited for; false when it did not complete in 64 ms.
/// Found on the 8a (2026-11-20): a job's group ended mid-job (a CLOSE with the camera's job in flight) left its command stream's timestamp store dirty in the L2, with no end-of-job flush to write it out; the next client's job's flush wrote it after the client's memory had gone back to the pool, and the GPU's fence refused the write (G#8_9240_1A40, a permission fault, a write). Had those frames been another client's by then, the write would have landed in that client's memory.
/// So every group ended with a job in it is followed by this before its memory may go (`queue_end`, `reap`), as kbase flushes the GPU's caches when it takes a context's address space down.
fn caches_flush(g: &Gpu) -> bool {
    const CLEAN_CACHES_COMPLETED: u32 = 1 << 17;
    g.wr(GPU_IRQ_CLEAR, CLEAN_CACHES_COMPLETED);
    g.wr(GPU_COMMAND, 4 | 0x233 << 8);
    let ok = g.wait(64, |g| g.rd(GPU_IRQ_RAWSTAT) & CLEAN_CACHES_COMPLETED != 0);
    g.wr(GPU_IRQ_CLEAR, CLEAN_CACHES_COMPLETED);
    ok
}

/// Address space `n` given `root`: the walks and lines of whatever tables it had dropped first (LOCKADDR over the whole 48-bit space, LOCK, FLUSH_PT, which cleans and invalidates the L2 and the MMU's cached walks with it: kbase's sequence for changed mappings on arch 11, ferros-csf-spec.md), then TRANSCFG, TRANSTAB, MEMATTR and UPDATE (`Gpu::install`).
fn space_install(g: &Gpu, n: usize, root: usize) -> bool {
    let idle = |g: &Gpu| g.rd(Gpu::as_reg(n, AS_STATUS)) & 1 == 0;
    if !g.wait(64, idle) {
        return false;
    }
    // LOCKADDR: base 0, SIZE = log2(2^48) - 1.
    g.wr64(Gpu::as_reg(n, AS_LOCKADDR), 47);
    g.wr(Gpu::as_reg(n, AS_COMMAND), 2);
    if !g.wait(64, idle) {
        return false;
    }
    g.wr(Gpu::as_reg(n, AS_COMMAND), 4);
    if !g.wait(64, idle) {
        return false;
    }
    g.install(n, root)
}

/// A group that would not end: its job may still run in its client's address space, and that client may have ended and its memory be another's soon. The service ends, one shot: the GPU's next service soft-resets it before it serves (every job on it stopped), and the kernel holds every block and surface an ended client's job could reach until that service takes its first request (process.rs `gpu_drain`; the holds of queues go with this process, `gpu_ending`). Clients fall back as they do without a GPU service until it is started again (`diag "restart gpu"`).
fn stuck(grants: u64, k: usize) -> ! {
    say!(grants, "gpu: slot {k}'s group did not end in 100 ms: the service ends, so the GPU is reset before it runs anything more\n");
    exit(1)
}

/// The guard of a slot's job, in counter ticks: a system client's 2^10 ms, a kist's 2^8 ms.
fn guard(s: &Svc, kist: bool) -> u64 {
    s.g.f / 1000 * if kist { GUARD_KIST_MS } else { GUARD_SYSTEM_MS }
}

/// When slot `sl`'s oldest job's guard passes, as a wait's deadline: 0 (none) while nothing of it is in flight. Under the lock.
fn guard_at(s: &Svc, sl: &Slot) -> u64 {
    if sl.len == 0 { 0 } else { sl.head_since + guard(s, sl.kist) }
}

/// Slot `k` looked at after a wait that may have reached its guard: a late oldest job ends with its group (`reap`, which also lands any that ended and answers a call's), the `DONE` word to wake returned. The main thread's call bell is moved here when a call's job ended, as `events` moves it.
fn guard_check(k: usize) -> Option<usize> {
    let s = svc();
    let (w, call) = with(|st| {
        let at = guard_at(s, &st.slots[k]);
        if at == 0 || ferros_sys::now() < at {
            return (None, false);
        }
        let mut w = None;
        let call = reap(st, k, ferros_sys::now(), &mut w);
        (w, call)
    });
    if call {
        CALL_BELL.fetch_add(1, Ordering::Release);
        ferros_sys::wake(&CALL_BELL, 1);
    }
    w
}

/// SYS_DMA on the blocks' window (`arena`): x1 = 7 hold or let go of an epoch's GPU work with its queue word, 8 whether an epoch lives (process.rs `gpu_hold`, `gpu_alive`).
fn hold(epoch: u64, on: bool, word: usize) -> bool {
    ferros_sys::svc(ferros_sys::nr::DMA, cap(svc().grants, S_ARENA), 7, epoch, on as u64, word as u64) == 0
}
fn alive(epoch: u64) -> bool {
    ferros_sys::svc(ferros_sys::nr::DMA, cap(svc().grants, S_ARENA), 8, epoch, 0, 0) == 1
}

/// Slot `k`'s group running, its client's space installed: started afresh when it is not. False when the firmware did not start it. Under the lock.
fn ensure_running(s: &Svc, sl: &mut Slot, k: usize) -> bool {
    if sl.running {
        return true;
    }
    MMU_FAULTS[k + 1].store(0, Ordering::Relaxed);
    let ok = space_install(&s.g, k + 1, sl.root) && group_start(&s.g, &s.sh, k, sl, s.cores, if sl.kist { PRIORITY_KIST } else { PRIORITY_SYSTEM });
    sl.running = ok;
    ok
}

/// `words` appended to slot `k`'s stream (padded with NOPs to the next 64 bytes: the front end fetches the ring a line at a time and keeps the line, 2026-11-04), its insert offset published, its doorbell rung. The ring is `RING_PAGES` and the stream reads it at the insert offset modulo that: a slot holds at most `FLIGHT` jobs of at most `JOB_WORDS_MOST` words, less than the ring, so no word the stream has not read is written over. Under the lock.
fn append(s: &Svc, sl: &mut Slot, k: usize, words: &[u64]) {
    let size = RING_PAGES * PAGE;
    let at = sl.insert;
    for (j, &w) in words.iter().enumerate() {
        wr64(sl.ring + (at + 8 * j) % size, w);
    }
    let end = (at + 8 * words.len()).next_multiple_of(64);
    for p in (at + 8 * words.len()..end).step_by(8) {
        wr64(sl.ring + p % size, 0);
    }
    unsafe { core::arch::asm!("dmb osh") };
    sl.insert = end;
    wr64(sl.mem.uio, end as u64);
    s.g.doorbell(k + 1);
}

/// A job put in flight on slot `k`: its sequence, and the clock's want. Under the lock.
fn fly(st: &mut State, k: usize, f: Flight) {
    let sl = &mut st.slots[k];
    if sl.len == 0 {
        sl.head_since = f.submitted;
    }
    sl.flight[(sl.head + sl.len) % FLIGHT] = f;
    sl.len += 1;
    st.inflight += 1;
    ACTIVITY.fetch_add(1, Ordering::Relaxed);
    if st.inflight == 1 {
        WANT.store(1, Ordering::Release);
        ferros_sys::wake(&WANT, 1);
    }
}

/// The oldest job of slot `k` ended: out of flight, and the clock's want. Under the lock.
fn land(st: &mut State, k: usize, now: u64) -> Flight {
    let sl = &mut st.slots[k];
    let f = sl.flight[sl.head];
    sl.head = (sl.head + 1) % FLIGHT;
    sl.len -= 1;
    sl.head_since = now;
    st.inflight -= 1;
    if st.inflight == 0 && !CLOCK_HOLD.load(Ordering::Relaxed) {
        WANT.store(0, Ordering::Release);
        ferros_sys::wake(&WANT, 1);
    }
    f
}

/// Slot `k`'s queue ended (`why`) and let go: every job still in flight lost, `DONE` caught up with what was taken, its group ended, the epoch's hold let go, the slot unbound when its client is gone (`unbind`). Under the lock; the caller wakes `DONE` (returned) and `SUBMIT`.
fn queue_end(st: &mut State, k: usize, why: u64, unbind: bool) -> Option<usize> {
    let s = svc();
    let now = ferros_sys::now();
    if st.slots[k].running && !group_end(&s.g, &s.sh, k) {
        stuck(s.grants, k);
    }
    // A job stopped mid-way leaves its writes in the GPU's caches: written out now, while its memory is still its client's (`caches_flush`).
    if st.slots[k].len > 0 && !caches_flush(&s.g) {
        stuck(s.grants, k);
    }
    st.slots[k].running = false;
    let qu = st.slots[k].queue.take();
    while st.slots[k].len > 0 {
        let f = land(st, k, now);
        if f.job == CALL {
            // A call's job ended with the group: its caller answered 2 (the main thread waits on the bell).
            st.call = (2, ((now - f.submitted) * 1_000_000 / s.g.f) as u32, 0, 0, 0);
            CALL_BELL.fetch_add(1, Ordering::Release);
            ferros_sys::wake(&CALL_BELL, 1);
        } else if let Some(qu) = qu {
            qu.ring().end(f.job, q::S_LOST, f.submitted, now, (0, 0), 0);
        }
    }
    let epoch = st.slots[k].client.map_or(0, |c| c.epoch);
    if let Some(qu) = qu {
        let r = qu.ring();
        r.finish(qu.taken, why);
        r.mark_closed();
    }
    hold(epoch, false, 0);
    if unbind {
        st.slots[k].client = None;
        st.slots[k].kernels = 0;
        st.slots[k].code_used = 0;
    }
    qu.map(|qu| qu.cpu)
}

/// What a slot's queue's `DONE` is now: its oldest job still in flight (every one before it ended), or every job taken when none is.
fn queue_done(sl: &Slot) -> u64 {
    let taken = sl.queue.map_or(0, |qu| qu.taken);
    (0..sl.len).map(|i| sl.flight[(sl.head + i) % FLIGHT]).find(|f| f.job != CALL).map_or(taken, |f| f.job)
}

/// Slot `k`'s jobs that ended, in order, their results written; then whether its oldest failed (a fault in its space, a command-stream fault, its guard passed), which ends its group and its queue, no other. Under the lock. Fills `wake` with the `DONE` word to wake, and says whether a call's job ended.
fn reap(st: &mut State, k: usize, now: u64, wake: &mut Option<usize>) -> bool {
    let s = svc();
    let mut call = false;
    let last = rd32(st.slots[k].flag + SEQ_AT);
    let qu = st.slots[k].queue;
    let mut landed = false;
    while st.slots[k].len > 0 {
        let h = st.slots[k].flight[st.slots[k].head];
        // A refused job ends as it reaches the head; a run one once its sequence is set (the slot's sequences grow by one a job, so the wrapping difference orders them).
        if !h.refused && (last.wrapping_sub(h.seq) as i32) < 0 {
            break;
        }
        let f = land(st, k, now);
        landed = true;
        if f.job == CALL {
            let sl = &st.slots[k];
            let (t0, t1) = (rd64(sl.flag + CALL_STAMPS + 8), rd64(sl.flag + CALL_STAMPS + 16));
            let gpu_us = if s.stamps && t0 != 0 && t1 >= t0 { ((t1 - t0) * 1_000_000 / s.g.f) as u32 } else { 0 };
            st.call = (0, ((now - f.submitted) * 1_000_000 / s.g.f) as u32, gpu_us, 0, 0);
            call = true;
        } else if let Some(qu) = qu {
            qu.ring().end(f.job, if f.refused { q::S_REFUSED } else { q::S_DONE }, f.submitted, now, (0, 0), s.stamp_offset);
        }
    }
    if landed {
        if let Some(qu) = st.slots[k].queue {
            qu.ring().finish(queue_done(&st.slots[k]), 0);
            *wake = Some(qu.cpu + q::DONE * 8);
        }
    }
    if st.slots[k].len == 0 {
        return call;
    }
    let n_as = k + 1;
    let (_, _, sin, sout) = blocks(&s.sh, k);
    let fault = MMU_FAULTS[n_as].load(Ordering::Acquire);
    let cs = (s.sh.rd(sin) ^ s.sh.rd(sout)) & 1 << 31 != 0;
    let late = now >= st.slots[k].head_since + guard(s, st.slots[k].kist);
    if fault == 0 && !cs && !late {
        return call;
    }
    MMU_FAULTS[n_as].store(0, Ordering::Relaxed);
    let addr = MMU_ADDRS[n_as].load(Ordering::Relaxed);
    let (cf, info, ptr) = (s.sh.rd(sout + 0x80), s.sh.rd(sout + 0x88) as u64 | (s.sh.rd(sout + 0x8C) as u64) << 32, s.sh.rd(sout + 0x40) as u64 | (s.sh.rd(sout + 0x44) as u64) << 32);
    let h = st.slots[k].flight[st.slots[k].head];
    let epoch = st.slots[k].client.map_or(0, |c| c.epoch);
    if fault != 0 {
        say!(s.grants, "gpu: slot {k} (epoch A#{epoch}): a job reached outside its memory: GPU address G#{addr:X}, AS_FAULTSTATUS G#{:X} ({}) in its own address space {n_as}; its group ended\n", fault as u32, exception(fault as u32));
    } else {
        say!(s.grants, "gpu: slot {k} (epoch A#{epoch}): a job {}: CS_FAULT G#{cf:X} ({}), CS_FAULT_INFO G#{info:X}, CS_STATUS_CMD_PTR G#{ptr:X}; its group ended\n", if cs { "faulted" } else { "did not finish in its guard" }, exception(cf));
    }
    // The job ended with its group, whatever state it is in (past its guard, stalled, faulted): no other client's job waits behind it, and its group starts afresh at its client's next job or queue.
    if !group_end(&s.g, &s.sh, k) || !caches_flush(&s.g) {
        stuck(s.grants, k);
    }
    st.slots[k].running = false;
    let status = if fault != 0 { q::S_OUTSIDE } else { q::S_FAILED };
    let mut first = true;
    while st.slots[k].len > 0 {
        let f = land(st, k, now);
        if f.job == CALL {
            st.call = (if fault != 0 { OUTSIDE } else { 2 }, ((now - f.submitted) * 1_000_000 / s.g.f) as u32, 0, addr, fault as u32);
            call = true;
        } else if let Some(qu) = qu {
            let (st_, fa) = if first && h.job == f.job { (status, if fault != 0 { (addr, fault as u32 as u64) } else { (0, 0) }) } else { (q::S_LOST, (0, 0)) };
            qu.ring().end(f.job, st_, f.submitted, now, fa, s.stamp_offset);
        }
        first = false;
    }
    if let Some(qu) = qu {
        let r = qu.ring();
        r.finish(qu.taken, q::E_FAULT);
        *wake = Some(qu.cpu + q::DONE * 8);
    }
    call
}

/// The GPU's job interrupt, on a thread of its own: each group's events acknowledged as kbase does (CSG_ACK's IDLE, SYNC_UPDATE and progress bits into CSG_REQ, CSG_IRQ_ACK = CSG_IRQ_REQ), then every slot's jobs that ended landed (`reap`), its client's `DONE` woken, a call's main thread woken.
/// No deadline here: a job's guard is its waiter's (`guard_check`), the slot's thread for a queue's jobs and the main thread for a call's, each asleep until its own slot's oldest job's guard at most, so a job put in flight while this thread sleeps is guarded all the same.
extern "C" fn events(_: u64) -> ! {
    let s = svc();
    loop {
        let r = ferros_sys::irq_wait(s.irq, 0);
        let now = ferros_sys::now();
        let mut wakes = [None; SLOTS];
        let call = with(|st| {
            if r == 0 {
                let raw = s.g.rd(JOB_IRQ_RAWSTAT);
                s.g.wr(JOB_IRQ_CLEAR, raw);
                for j in 0..SLOTS {
                    if raw & 1 << j != 0 {
                        let (ji, jo, _, _) = blocks(&s.sh, j);
                        let mask = 1 << 28 | 1 << 29 | 1 << 31;
                        s.sh.wr(ji, (s.sh.rd(ji) & !mask) | (s.sh.rd(jo) & mask));
                        s.sh.wr(ji + 0x0C, s.sh.rd(jo + 0x0C));
                    }
                }
                ferros_sys::irq_ack(s.irq);
            }
            let mut call = false;
            for (k, w) in wakes.iter_mut().enumerate() {
                call |= reap(st, k, now, w);
            }
            call
        });
        for w in wakes.into_iter().flatten() {
            // SAFETY: a `DONE` word of a queue still held (its block held while the service holds its epoch: it is let go only under the lock, after its last wake here was decided), in the blocks' window.
            ferros_sys::wake(unsafe { &*(w as *const AtomicU64) }, u64::MAX);
        }
        if call {
            CALL_BELL.fetch_add(1, Ordering::Release);
            ferros_sys::wake(&CALL_BELL, 1);
        }
    }
}

/// The GPU's clock (the `gpu-clock` switch, grant 9) turned to `WANT` on the kernel's core, where every switch runs: this thread lives there (one move at its start, as ferros_sys `power` makes), so each turn is one syscall.
/// The coordinator's rule (2026-11-20, after the 8a's measure: a turn up and one down a frame cost about 7 % of the kernel's core):
/// - up at once when a job is put in flight with the clock down (`WANT`);
/// - down only at a display frame (the frame mailbox, ferros_mailbox `FRAME`) that finds nothing in flight and nothing put in flight since the frame before (`ACTIVITY`), two such looks in a row (a whole frame quiet), never at a job's end;
/// - while up, the firmware's shader-core power-off delay (`GLB_PWROFF_TIMER`, panthor_fw.h `poweroff_timer`) just over one display frame (the shortest frame spacing seen since the clock went up, and a quarter more), so a client working once a frame finds the cores on; back to the default 800 us when the clock goes down.
/// While the display starts no frame (the Pro's panel holds a still picture) the clock stays up until the next frame or the dark profile (the kernel's `gpu_clock` drops it there): no timer brings it down.
/// Without the frame mailbox (a kernel from before it) the clock goes down when the queues empty, as before.
extern "C" fn clock(token: u64) -> ! {
    ferros_sys::pin(ferros_sys::CORES_BIG);
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
    let turn = |on: bool| ferros_sys::svc(ferros_sys::nr::POWER, token, on as u64, 0, 0, 0);
    // The service ready with nothing queued: the floor, whatever the kernel's rule from before the switch left (the top while this process lives).
    turn(false);
    let s = svc();
    let mut tok = [0u8; 32];
    let frames = ferros_sys::mailbox::Reader::open(ferros_sys::mailbox::FRAME, 0, &mut tok);
    let Some(frames) = frames else {
        say!(s.grants, "gpu: no frame mailbox: the clock goes down when the queues empty\n");
        let mut applied = 0;
        loop {
            let w = WANT.load(Ordering::Acquire);
            if w == applied {
                ferros_sys::wait(&WANT, applied, 0);
                continue;
            }
            turn(w != 0);
            applied = w;
        }
    };
    let mut out = [0u64; ferros_sys::mailbox::frame::WORDS];
    loop {
        // Down: asleep until a job is put in flight.
        while WANT.load(Ordering::Acquire) == 0 && !CLOCK_HOLD.load(Ordering::Relaxed) {
            ferros_sys::wait(&WANT, 0, 0);
        }
        turn(true);
        let mut seen = frames.seq();
        let mut last_act = ACTIVITY.load(Ordering::Relaxed);
        // Quiet looks in a row: down at the second. A job put in flight right after a frame's publish races the look at that frame, so one look may find nothing new while the next finds two; two quiet looks in a row hold a whole frame with nothing put in flight (measured on the 8a: one quiet look turned the clock 60 times in 128 frames at a job a frame).
        let mut quiet = 0u32;
        let (mut last_at, mut shortest, mut delay) = (0u64, u64::MAX, PWROFF_DEFAULT);
        // Up: a look at each display frame.
        loop {
            seen = frames.wait(seen, 0);
            let at = match frames.read(&mut out) {
                ferros_sys::mailbox::Got::Fresh { at, seq, .. } => {
                    seen = seq;
                    at
                }
                _ => continue,
            };
            let act = ACTIVITY.load(Ordering::Relaxed);
            let busy = act != last_act || WANT.load(Ordering::Acquire) != 0 || CLOCK_HOLD.load(Ordering::Relaxed);
            last_act = act;
            quiet = if busy { 0 } else { quiet + 1 };
            if quiet == 2 {
                break;
            }
            if last_at != 0 {
                shortest = shortest.min(at.wrapping_sub(last_at));
            }
            last_at = at;
            // Just over one display frame, from the second frame of the run on (the first has no spacing yet).
            let want = if shortest == u64::MAX { PWROFF_DEFAULT } else { shortest + shortest / 4 };
            if want != delay {
                with(|_| pwroff_set(s, want));
                delay = want;
            }
        }
        turn(false);
        if delay != PWROFF_DEFAULT {
            with(|_| pwroff_set(s, PWROFF_DEFAULT));
        }
        CLOCK_FRAMES_DOWN.fetch_add(1, Ordering::Relaxed);
    }
}

/// The firmware's shader-core power-off delay at global init (main.rs: G#4CCC ticks of the 24.576 MHz system counter, 800 us).
const PWROFF_DEFAULT: u64 = 0x4CCC;
/// Times the clock went down at a frame (`clock`).
static CLOCK_FRAMES_DOWN: AtomicU64 = AtomicU64::new(0);

/// The firmware's shader-core power-off delay set to `ticks` of the system counter (GLB_PWROFF_TIMER at the global input's +G#14, its source bit 31 clear: the system timestamp; then GLB_REQ's CFG_POWEROFF_TIMER, bit 3, toggled against GLB_ACK and doorbell 0, as panthor_fw.c sets it at init); false when the firmware did not acknowledge it in 16 ms. Under the lock (the shared interface).
fn pwroff_set(s: &Svc, ticks: u64) -> bool {
    let (gin, gout) = (s.sh.rd(0x0400_0008), s.sh.rd(0x0400_000C));
    // WHY the mask: the field is 31 bits (GLB_TIMER_VAL); a frame spacing past it (over a minute) is no display frame, and the top bit would pick the GPU's own counter as the source.
    s.sh.wr(gin + 0x14, (ticks & 0x7FFF_FFFF) as u32);
    let (req, ack) = (s.sh.rd(gin), s.sh.rd(gout));
    s.sh.wr(gin, (req & !8) | ((ack ^ 8) & 8));
    s.g.doorbell(0);
    s.g.wait(16, |_| (s.sh.rd(gin) ^ s.sh.rd(gout)) & 8 == 0)
}

/// Slot `k`'s thread: while its slot has a queue, waits on the queue's `SUBMIT` (the client's doorbell, SYS_WAKE on its own block's word) and writes each new job into the client's stream; a `GONE` there is the client's end (the kernel's mark) or the queue's close (the service's).
extern "C" fn slot_thread(k: u64) -> ! {
    let k = k as usize;
    let s = svc();
    loop {
        let mut wake_done = None;
        let waiting = with(|st| {
            let Some(qu) = st.slots[k].queue else {
                return (&SLOT_BELL[k] as *const AtomicU64, SLOT_BELL[k].load(Ordering::Acquire));
            };
            let r = qu.ring();
            let raw = r.submit_raw();
            if raw & q::GONE != 0 {
                let epoch = st.slots[k].client.map_or(0, |c| c.epoch);
                if !alive(epoch) {
                    wake_done = queue_end(st, k, q::E_CLOSED, true).map(|a| a + q::DONE * 8);
                    say!(s.grants, "gpu: slot {k}'s client (epoch A#{epoch}) ended with its queue open: its group ended, its GPU work let go\n");
                }
                return (&SLOT_BELL[k] as *const AtomicU64, SLOT_BELL[k].load(Ordering::Acquire));
            }
            let now = ferros_sys::now();
            while st.slots[k].queue.unwrap().taken < r.submitted() {
                let n = st.slots[k].queue.unwrap().taken;
                // WHY: the client wrote `SUBMIT`; one past its depth means it wrote over a record whose job had not ended, so nothing in the ring can be trusted: the queue ends.
                if n.wrapping_sub(r.done()) >= 1 << qu.depth_log2 || st.slots[k].len >= FLIGHT - 1 {
                    wake_done = queue_end(st, k, q::E_OVERRUN, false).map(|a| a + q::DONE * 8);
                    say!(s.grants, "gpu: slot {k}'s client submitted past its queue's depth: the queue ended\n");
                    return (&SLOT_BELL[k] as *const AtomicU64, SLOT_BELL[k].load(Ordering::Acquire));
                }
                let (kern, nu, groups) = r.request(n);
                let ok = q::check(kern, nu, groups, st.slots[k].kernels as u64) && ensure_running(s, &mut st.slots[k], k);
                let seq = st.slots[k].seq;
                if ok {
                    let rec = qu.gpu + r.record_offset(n) as u64;
                    let d = q::Dispatch { spd: DESC_VA + (SPD_AT + 32 * kern as usize) as u64, tsd: DESC_VA + (TSD_AT + 64 * kern as usize) as u64, fau: rec + (q::UNIFORMS * 8) as u64, uniforms: nu as u32, groups, max_threads: s.max_threads, stamps: s.stamps.then_some(rec + q::RESULT_BYTES as u64), sync: OUT_VA + SEQ_AT as u64, value: seq, wg_log2: st.slots[k].kern_wg[kern as usize] as u32, merge: !st.slots[k].kern_wls[kern as usize] && st.slots[k].kern_wg[kern as usize] == 4 };
                    let mut words = [0u64; q::JOB_WORDS_MOST];
                    let len = q::job_words(&d, &mut words);
                    append(s, &mut st.slots[k], k, &words[..len]);
                    st.slots[k].seq = seq.wrapping_add(1);
                }
                fly(st, k, Flight { seq, job: n, refused: !ok, submitted: now });
                st.slots[k].queue.as_mut().unwrap().taken = n + 1;
                st.jobs += 1;
                st.slots[k].used = st.jobs;
            }
            (r.submit_word() as *const AtomicU64, st.slots[k].queue.unwrap().taken)
        });
        if let Some(w) = wake_done {
            // SAFETY: as in `events`: decided under the lock before the hold was let go; a word of the blocks' window, mapped while any block is held there (a let-go block maps the blank frame, cachemap `gpub_map`).
            ferros_sys::wake(unsafe { &*(w as *const AtomicU64) }, u64::MAX);
        }
        // A refused job at the head ends at once: the events thread lands it at its next look, so ring it here when nothing else will.
        with(|st| {
            if st.slots[k].len > 0 && st.slots[k].flight[st.slots[k].head].refused {
                let mut w = None;
                reap(st, k, ferros_sys::now(), &mut w);
                wake_done = w;
            }
        });
        if let Some(w) = wake_done.take() {
            // SAFETY: as above.
            ferros_sys::wake(unsafe { &*(w as *const AtomicU64) }, u64::MAX);
        }
        // Asleep until the next job, the client's end or the close (`GONE`), or the oldest job's guard (`guard_at`: none while nothing is in flight).
        let deadline = with(|st| guard_at(s, &st.slots[k]));
        // SAFETY: the word is `SLOT_BELL[k]` or the queue's `SUBMIT`, in a block held while the queue is open; a queue closed or ended after this look is marked `GONE` there first, so the wait returns at once.
        ferros_sys::wait(unsafe { &*waiting.0 }, waiting.1, deadline);
        if let Some(w) = guard_check(k) {
            // SAFETY: as in `events`.
            ferros_sys::wake(unsafe { &*(w as *const AtomicU64) }, u64::MAX);
        }
    }
}

/// Slot `k` bound to the client of `stamp`: its address space from the stamp alone (its block's frames in 2 MiB entries from `ARENA_VA`, each surface frame by frame at its place, and the slot's own pages). Under the lock; the slot's group has ended.
fn bind(s: &Svc, sl: &mut Slot, stamp: &Stamp, bbase: u64, bsize: u64) {
    // The stamp's regions and their frames kept with the slot, so its tables can be made again without the client (a camera view taken back, `camera_watch`).
    sl.nregions = stamp.n;
    sl.regions[..stamp.n].copy_from_slice(stamp.regions());
    sl.nframes = 0;
    for i in 0..stamp.n {
        for pa in stamp.frames_of(i) {
            sl.frames[sl.nframes] = pa;
            sl.nframes += 1;
        }
    }
    sl.client = Some(stamp.key());
    sl.block = (bbase, bsize);
    sl.kist = stamp.kist;
    tables(s, sl, true);
}

/// A slot's client's address space made from what its bind kept, in the slot's other pool of table pages, then installed (`space_install`: the old walks and lines dropped first) when its group runs, else at its next start: the slot's own pages (its stream and descriptors read-only, its sequence page, its code read-only and the only executable pages), its block's frames read-write at `ARENA_VA`, each surface frame by frame at its place from `SURFACES_VA`, and (`camera`) its camera view read-only at `CAMERA_VA`, cacheable on the GPU, every job ending with the GPU's caches invalidated (`q::END_FLUSH`: the front end writes the ring with no snooping, and a line the GPU's L2 kept from a lap before would be the last lap's), non-cacheable when the client asked (op 5's byte 0).
/// Two pools a slot, so tables the GPU may be walking are never written: a running group's space moves to the new ones at once (2026-11-20: a block grown or a camera view given while a queue runs). Under the lock.
fn tables(s: &Svc, sl: &mut Slot, camera: bool) {
    let pool = sl.pools[sl.cur ^ 1];
    let mut tm = Mem::new(pool);
    let t = Table::new(&mut tm);
    t.map(&mut tm, RING_VA, sl.ring, RING_PAGES, PAGE_RO | NC | NX, TABLE);
    t.map(&mut tm, DESC_VA, sl.desc, 1, PAGE_RO | NC | NX, TABLE);
    t.map(&mut tm, OUT_VA, sl.flag, 1, PAGE_RW | NC | NX, TABLE);
    t.map(&mut tm, CODE_BIG_VA, sl.code, CODE_PAGES + KERNEL_PAGES, PAGE_RO | NC, TABLE);
    let (bbase, bsize) = sl.block;
    if !is_block(bbase, bsize) {
        // A kernel from before 2026-11-13: a block of the fixed arena, at its physical address.
        t.map(&mut tm, ARENA_VA, bbase as usize, bsize as usize / PAGE, PAGE_RW | 6 << 2 | 2 << 8 | NX, TABLE);
    }
    let mut at = 0;
    sl.camera = false;
    for i in 0..sl.nregions {
        let (b, sz) = sl.regions[i];
        if !ferros_sys::gpu::in_window(b, sz) {
            continue;
        }
        let n = (sz / ferros_sys::gpu::FRAME) as usize;
        for j in 0..n.min(sl.nframes - at) {
            let pa = sl.frames[at + j] as usize;
            let off = j as u64 * ferros_sys::gpu::FRAME;
            if is_block(b, sz) {
                // The block's frames in 2 MiB blocks from `ARENA_VA` (2026-11-13: frames of the pool, wherever it had them), in the stamp's order, which is the CPU's from the block's base.
                if (b, sz) == (bbase, bsize) {
                    t.map_block(&mut tm, ARENA_VA + off, pa, BLOCK_RW | 6 << 2 | 2 << 8 | NX, TABLE);
                }
            } else if is_camera(b, sz) {
                if camera {
                    t.map_block(&mut tm, CAMERA_VA + off, pa, BLOCK_RO | if sl.camera_cached { 0 } else { NC } | NX, TABLE);
                    sl.camera = true;
                }
            } else if s.surfaces_ok {
                t.map(&mut tm, SURFACES_VA + (b - SURFACES_PA) + off, pa, ferros_sys::gpu::FRAME as usize / PAGE, PAGE_RW | 6 << 2 | 2 << 8 | NX, TABLE);
            }
        }
        at += n;
    }
    // The pool's size is the bound on any stamp the kernel writes (a block's and a camera view's 2 MiB entries in level-2 tables, at most the four surfaces in pages): past it is a bug here, loud.
    assert!(tm.used() <= TABLE_PAGES * PAGE, "gpu: a client's tables outgrew their slot");
    sl.root = t.root;
    sl.cur ^= 1;
}

/// The slot of the client of `stamp`: its own if bound (its tables made again in place when its memory changed: a block grown, a surface or a camera view given or taken), else a free one, else the one used longest ago that holds no queue and no job (its group ended, its space given the new client's tables). None when every slot is held so. Under the lock.
fn slot_for(st: &mut State, stamp: &Stamp, bbase: u64, bsize: u64) -> Option<usize> {
    let s = svc();
    if let Some(k) = st.slots.iter().position(|sl| sl.client.is_some_and(|c| c.epoch == stamp.epoch)) {
        // A camera view in the stamp that its tables lack: taken back and given again since its bind (`camera_watch` made them without it), the same frames, so the same key.
        let camera_back = !st.slots[k].camera && stamp.regions().iter().any(|&(b, sz)| is_camera(b, sz));
        if st.slots[k].client != Some(stamp.key()) || camera_back {
            bind(s, &mut st.slots[k], stamp, bbase, bsize);
            if st.slots[k].running && !space_install(&s.g, k + 1, st.slots[k].root) {
                say!(s.grants, "gpu: slot {k}'s address space did not take its new tables\n");
            }
        }
        return Some(k);
    }
    let k = (0..SLOTS).filter(|&k| st.slots[k].queue.is_none() && st.slots[k].len == 0).min_by_key(|&k| (st.slots[k].client.is_some(), st.slots[k].used))?;
    if st.slots[k].running && !group_end(&s.g, &s.sh, k) {
        stuck(s.grants, k);
    }
    st.slots[k].running = false;
    st.slots[k].kernels = 0;
    st.slots[k].code_used = 0;
    bind(s, &mut st.slots[k], stamp, bbase, bsize);
    // The thread storage descriptor every job of the slot names: no local storage (G#8000_0000 packed as log2), as since 2026-11-04.
    for j in 0..8 {
        wr32(st.slots[k].desc + 0x40 + 4 * j, if j == 1 { 0x1F } else { 0 });
    }
    Some(k)
}

/// The camera's bell (the kernel's `BELL_GPU_CAMERA`): a camera view whose holder's GPU calls carried it was taken back (its CLOSE, its holder's end, a view given anew). That client's jobs are stopped (its group ended; its queue ends `E_CAMERA`), the view unmapped from its space with the GPU's TLB and walk caches flushed (`space_install`), and the kernel told (SYS_DMA x1 = 9), which only then shuts the frames out of the fence and lets them serve anyone else.
/// Also at the service's start: a stale epoch left by a service before this one, whose GPU this one has reset.
extern "C" fn camera_watch(_: u64) -> ! {
    let s = svc();
    let arena = cap(s.grants, S_ARENA);
    let registered = ferros_sys::bell(BELL_GPU_CAMERA, &CAMERA_BELL);
    loop {
        let seen = CAMERA_BELL.load(Ordering::Acquire);
        let stale = ferros_sys::svc(ferros_sys::nr::DMA, arena, 9, 0, 0, 0);
        if stale != 0 && stale != ferros_sys::ERR {
            let mut wakes = [None; SLOTS];
            with(|st| {
                for k in 0..SLOTS {
                    // Its client the stale one, or gone (a slot let go keeps its last tables until it is bound again).
                    if !(st.slots[k].camera && st.slots[k].client.is_none_or(|c| c.epoch == stale)) {
                        continue;
                    }
                    // A job still running is stopped before the unmap (the owner's condition): its queue ends, every job of it lost.
                    if st.slots[k].len > 0 {
                        wakes[k] = queue_end(st, k, q::E_CAMERA, false).map(|a| a + q::DONE * 8);
                        say!(s.grants, "gpu: slot {k}: the camera view went with a job in flight: its group ended, its queue ended\n");
                    }
                    let sl = &mut st.slots[k];
                    tables(s, sl, false);
                    if !space_install(&s.g, k + 1, sl.root) {
                        say!(s.grants, "gpu: slot {k}'s address space did not take its tables without the camera\n");
                    }
                }
            });
            for w in wakes.into_iter().flatten() {
                // SAFETY: as in `events`.
                ferros_sys::wake(unsafe { &*(w as *const AtomicU64) }, u64::MAX);
            }
            ferros_sys::svc(ferros_sys::nr::DMA, arena, 9, stale, 0, 0);
            continue;
        }
        if !registered {
            // A kernel without the bell has no camera on the GPU to settle: nothing more to watch.
            ferros_sys::thread_exit(0);
        }
        ferros_sys::wait(&CAMERA_BELL, seen, 0);
    }
}

/// The resident service: the threads started, then the calls taken one at a time.
/// A call's job is waited for before the next request is taken (the kernel's `gpu_drained`: what an ended caller's job could reach stays drained until then); a queue's jobs run with no call, under the epoch's hold instead (`hold`).
pub fn serve(g: &Gpu, sh: &Shared, m: &mut Mem, slot_mem: [SlotMem; SLOTS], cores: u64, grants: u64) -> ! {
    let arena = ferros_sys::dma(cap(grants, S_ARENA)) as usize;
    let arena_bytes = ferros_sys::dma_size(cap(grants, S_ARENA), ARENA_MOST as u64).map_or(ARENA_BEFORE, |n| n as usize);
    let arena_ok = arena != 0 && arena != usize::MAX;
    // A client's surfaces are mapped for it only when the kernel fences them in for the GPU (granted here, the whole region, s2mpu::fence_gpu).
    let surfaces = ferros_sys::dma(cap(grants, S_SURFACES));
    let surfaces_ok = surfaces == ferros_sys::SURFACES_WINDOW && ferros_sys::dma_size(cap(grants, S_SURFACES), SURFACES_BYTES as u64) == Some(SURFACES_BYTES as u64);
    // SAFETY: no other thread runs yet; the slots are filled in place.
    let slots = unsafe { &mut (*(&raw mut ST)).slots };
    for (k, sl) in slots.iter_mut().enumerate() {
        sl.mem = slot_mem[k];
        sl.ring = m.alloc(RING_PAGES);
        sl.desc = m.alloc(1);
        sl.flag = m.alloc(1);
        sl.code = m.alloc(CODE_PAGES + KERNEL_PAGES);
        sl.pools = [m.alloc(TABLE_PAGES), m.alloc(TABLE_PAGES)];
    }
    let mmu = cap(grants, S_MMU);
    // The grants page's count (G#200, ferros_sys::cap) says whether slot 8 was given: the kernel that gives it is the one that stamps every call on `gpu` (2026-11-12). A kernel from before stamps nothing, so a request's first bytes would be the client's own, naming any memory: the service then serves nothing, each request answered 1 at once (its clients draw on the CPU, as with no GPU service), until the kernel is the stamping one.
    let count = unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) } as u64;
    if count <= S_MMU {
        say!(grants, "gpu: this kernel does not stamp calls on `gpu` (no slot 8): serving nothing\n");
        let mut msg = [0u8; 64];
        let mut token = [0u8; 32];
        loop {
            if ferros_sys::recv(cap(grants, S_GPU), &mut msg, &mut token, false) != ferros_sys::NOTHING {
                ferros_sys::reply(&token, &[1, 0, 0, 0, 0]);
            }
        }
    }
    g.wr(MMU_IRQ_CLEAR, 0xFFFF_FFFF);
    g.wr(MMU_IRQ_MASK, 0xFFFF);
    MMU_REGS.store(g.regs, Ordering::Release);
    ferros_sys::thread_spawn(mmu_watch, unsafe { &mut *(&raw mut MMU_STACK) }, mmu);
    // The stamps: stream 0's registers (CS_FEATURES bits 7:0 plus one, panthor CS_FEATURES_WORK_REGS) hold the pair the stamps use, and the GPU's timestamp runs (GPU_TIMESTAMP, G#98, panthor_gpu_regs.h); its lead on the CPU's counter read once, between two reads of the counter.
    let regs = (sh.rd(0x0400_1000 + 0x40) & 0xFF) + 1;
    // The high word read on each side of the low (panthor `gpu_read64_counter`): a carry between the two halves would put the low word's wrap in the high one.
    let read_ts = || loop {
        let hi = g.rd(GPU_TIMESTAMP + 4);
        let lo = g.rd(GPU_TIMESTAMP);
        if g.rd(GPU_TIMESTAMP + 4) == hi {
            return (hi as u64) << 32 | lo as u64;
        }
    };
    let t0 = ferros_sys::now();
    let ts = read_ts();
    let t1 = ferros_sys::now();
    let stamps = regs >= q::REGS_USED && ts != 0;
    let stamp_offset = ts.wrapping_sub(t0 + (t1 - t0) / 2);
    say!(grants, "gpu: the stream's registers A#{regs}; the GPU's timestamp G#{ts:X}, A#{} ticks ahead of the CPU's counter (read in A#{} ticks); job stamps {}\n", stamp_offset as i64, t1 - t0, if stamps { "on" } else { "off" });
    // WHY the count: the `gpu-clock` switch is grant 9 from a kernel of 2026-11-20 on; before it the kernel keeps the GPU at its top while this process lives, and nothing here turns it.
    let clock_ok = count > S_CLOCK;
    unsafe {
        *(&raw mut SVC) = Some(Svc { g: Gpu { regs: g.regs, grants: g.grants, f: g.f }, sh: Shared { pa: sh.pa }, cores, max_threads: g.rd(THREAD_MAX_THREADS), grants, irq: cap(grants, S_IRQ), surfaces_ok, stamps, stamp_offset });
    }
    let s = svc();
    ferros_sys::thread_spawn(events, unsafe { &mut *(&raw mut EVENTS_STACK) }, 0);
    for k in 0..SLOTS {
        ferros_sys::thread_spawn(slot_thread, unsafe { &mut (*(&raw mut SLOT_STACKS))[k] }, k as u64);
    }
    if clock_ok {
        ferros_sys::thread_spawn(clock, unsafe { &mut *(&raw mut CLOCK_STACK) }, cap(grants, S_CLOCK));
    }
    ferros_sys::thread_spawn(camera_watch, unsafe { &mut *(&raw mut CAMERA_STACK) }, 0);
    say!(grants, "gpu: serving `gpu`, a group and an address space a client ({} at once), calls and queues: the arena {} ({} MiB), the surfaces {}, the clock {}\n", SLOTS, if arena_ok { "granted" } else { "not granted" }, arena_bytes >> 20, if surfaces_ok { "fenced in" } else { "not granted" }, if clock_ok { "by the queues (gpu-clock)" } else { "the kernel's (no gpu-clock grant)" });
    let mut msg = [0u8; 1 << 13];
    let mut token = [0u8; 32];
    loop {
        let n = ferros_sys::recv(cap(grants, S_GPU), &mut msg, &mut token, false);
        if n == ferros_sys::NOTHING || n == 0 {
            continue;
        }
        let whole = &msg[..(n as usize).min(msg.len())];
        let mut reply = [1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut reply_len = 5;
        // The kernel's stamp: who calls and what memory it holds. None only from a kernel before the stamp, whose callers' memory the service cannot know: refused.
        let Some((stamp, m)) = Stamp::read(whole) else {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        };
        if stamp.kist && stamp.production {
            reply[0] = UNCHECKED;
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        // The client's block: the first region inside the arena (a client holds one; a holder of the whole arena, a development spawn's `--dma arena`, gets the whole).
        let block = stamp.regions().iter().copied().find(|&(b, sz)| arena_ok && b as usize >= arena && b as usize + sz as usize <= arena + arena_bytes);
        let Some((bbase, bsize)) = block.filter(|_| !m.is_empty() && (1..=6).contains(&m[0])) else {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        };
        let u16_at = |i: usize| u16::from_le_bytes([m[i], m[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([m[i], m[i + 1], m[i + 2], m[i + 3]]);
        match m[0] {
            // A kernel loaded for the client's queue: its words copied from its block after the calls' code, its descriptor written.
            2 if m.len() >= 9 => {
                let (preload, code_off, words) = (u16_at(1) as u32, u32_at(3) as usize, u16_at(7) as usize);
                // The workgroup (2026-11-21, valhall::ssa `workgroup`), from a request of 16 bytes: [9] 2^n invocations along x (4 to 8), [10] the bytes of workgroup storage each workgroup needs, log 2 (0 none), [11..15] where that storage goes in the client's block, [15] the bytes the client set aside there, log 2. A 9-byte request is a warp with none, as before.
                let (wg, wls_log2, wls_at, room_log2) = if m.len() >= 16 { (m[9] as u32, m[10] as u32, u32_at(11) as usize, m[15] as u32) } else { (4, 0, 0, 0) };
                // The storage each workgroup takes: a power of two of at least 128 bytes (Mesa's pan_wls_adjust_size), `instances` of them a core (as many workgroups as a core's threads hold, Mesa's pan_calc_wls_instances; fewer and the GPU holds workgroups back until one is free), for every core id up to the last present (pan_query_core_id_range): as many as the room the client set aside holds.
                let core_range = 64 - s.cores.leading_zeros();
                let size_log2 = wls_log2.max(7);
                let want_log2 = (s.max_threads >> wg.min(8)).max(1).trailing_zeros();
                let inst_log2 = (0..=want_log2).rev().find(|&i| ((core_range as u64) << (size_log2 + i)) <= 1u64 << room_log2.min(40));
                // WHY: every field is the client's; its storage must lie in its own block, page aligned (pan_emit_tls's assert), and a kernel asking for storage must have room for at least one workgroup a core. Outside that its own space faults anyway; refused here, it hears why at once.
                let wls_ok = wls_log2 == 0 || (wls_log2 <= 16 && wls_at % PAGE == 0 && room_log2 <= 30 && wls_at + (1usize << room_log2) <= bsize as usize && inst_log2.is_some());
                let fits = words > 0 && code_off % 8 == 0 && code_off + 8 * words <= bsize as usize && (4..=8).contains(&wg) && wls_ok;
                let r = with(|st| {
                    let k = slot_for(st, &stamp, bbase, bsize)?;
                    let sl = &mut st.slots[k];
                    let at = sl.code_used.next_multiple_of(16);
                    if !fits || sl.kernels >= KERNELS_MOST || at + words + 16 > KERNEL_PAGES * PAGE / 8 {
                        return None;
                    }
                    let code = unsafe { core::slice::from_raw_parts((bbase as usize + code_off) as *const u64, words) };
                    // WHY: as a call's, a kernel whose first word is zero never reached the block (2026-11-11).
                    if code[0] == 0 {
                        return None;
                    }
                    let base = sl.code + CODE_PAGES * PAGE;
                    for (j, &w) in code.iter().enumerate() {
                        wr64(base + 8 * (at + j), w);
                    }
                    for j in words..words + 16 {
                        wr64(base + 8 * (at + j), 0);
                    }
                    let va = CODE_BIG_VA + (CODE_PAGES * PAGE + 8 * at) as u64;
                    // Bit 15 of the preload word, the service's own: 64 registers a thread (as a call's).
                    let (spd0, pre) = if preload & 0x8000 != 0 { (0x0000_0018u32, preload & 0x7FFF) } else { (0x8000_0018, preload) };
                    let h = sl.kernels;
                    for (j, w) in [spd0, pre, va as u32, (va >> 32) as u32, 0, 0, 0, 0].iter().enumerate() {
                        wr32(sl.desc + SPD_AT + 32 * h + 4 * j, *w);
                    }
                    // Its thread storage descriptor (genxml v11 "Local Storage"; Mesa's pan_emit_tls): no thread storage; the workgroup storage's instances a core (log 2, bits 0-4) and its size scale (log 2 of the size plus 1, bits 8-12) with its base, or G#8000_0000 packed as log 2 (G#1F) for none.
                    let (w1, base) = match inst_log2.filter(|_| wls_log2 != 0) {
                        Some(i) => (i | (size_log2 + 1) << 8, ARENA_VA + wls_at as u64),
                        None => (0x1F, 0),
                    };
                    for (j, w) in [0, w1, 0, 0, base as u32, (base >> 32) as u32, 0, 0].iter().enumerate() {
                        wr32(sl.desc + TSD_AT + 64 * h + 4 * j, *w);
                    }
                    sl.kern_wg[h] = wg as u8;
                    sl.kern_wls[h] = wls_log2 != 0;
                    sl.kernels += 1;
                    sl.code_used = at + words + 16;
                    Some(h as u8)
                });
                match r {
                    Some(h) => ferros_sys::reply(&token, &[0, h, if wls_log2 != 0 { inst_log2.unwrap_or(0) as u8 } else { 0 }]),
                    None => ferros_sys::reply(&token, &[1]),
                };
                continue;
            }
            // A queue opened: at an offset of its block, its depth; the client's epoch held with the queue's word (the kernel marks it at the client's end), its group started.
            3 if m.len() >= 6 => {
                let (at, depth) = (u32_at(1) as usize, m[5] as u32);
                // WHY: the client names where; the queue must lie whole in its own block, page aligned, no deeper than the ring allows.
                let fits = at % q::PAGE == 0 && depth <= q::DEPTH_MOST_LOG2 && at + q::bytes(depth) <= bsize as usize;
                let ok = fits
                    && with(|st| {
                        let Some(k) = slot_for(st, &stamp, bbase, bsize) else { return false };
                        let cpu = bbase as usize + at;
                        if !hold(stamp.epoch, true, cpu) {
                            return false;
                        }
                        let mut qu = Queue { cpu, gpu: ARENA_VA + at as u64, depth_log2: depth, taken: 0 };
                        qu.taken = qu.ring().open();
                        st.slots[k].queue = Some(qu);
                        if !ensure_running(s, &mut st.slots[k], k) {
                            queue_end(st, k, q::E_SERVICE, false);
                            return false;
                        }
                        SLOT_BELL[k].fetch_add(1, Ordering::Release);
                        ferros_sys::wake(&SLOT_BELL[k], 1);
                        true
                    });
                ferros_sys::reply(&token, &[!ok as u8]);
                continue;
            }
            // Its memory looked at again (2026-11-20): its address space made from this call's stamp when it changed (a camera view given since: mapped read-only at `CAMERA_VA`), `[0][the camera mapped u8]`.
            5 => {
                // The camera mapped cacheable on the GPU (the coordinator, 2026-11-20, after the 8a's measure: 1.4 to 1.7 times faster), every job ending with the GPU's L2 and load-store caches invalidated (`q::END_FLUSH`, since 2026-11-21; a start-of-job invalidate for the camera's holder before), so a frame landed since the last job is read from DRAM: `[5]` or `[5][1]`; `[5][0]` non-cacheable (the measure). A change remakes its tables.
                let cached = !(m.len() > 1 && m[1] == 0);
                let cam = with(|st| {
                    let k = slot_for(st, &stamp, bbase, bsize)?;
                    if st.slots[k].camera_cached != cached {
                        st.slots[k].camera_cached = cached;
                        tables(s, &mut st.slots[k], true);
                        if st.slots[k].running && !space_install(&s.g, k + 1, st.slots[k].root) {
                            say!(s.grants, "gpu: slot {k}'s address space did not take its new tables\n");
                        }
                    }
                    Some(st.slots[k].camera)
                });
                match cam {
                    Some(c) => ferros_sys::reply(&token, &[0, c as u8]),
                    None => ferros_sys::reply(&token, &[1, 0]),
                };
                continue;
            }
            // `[6][1]` the GPU's clock held up whatever is queued, `[6][0]` let go (a development measure: what the turns cost a frame), `[0]`.
            6 => {
                let on = m.len() > 1 && m[1] == 1;
                with(|st| {
                    CLOCK_HOLD.store(on, Ordering::Relaxed);
                    WANT.store((on || st.inflight > 0) as u64, Ordering::Release);
                });
                ferros_sys::wake(&WANT, 1);
                ferros_sys::reply(&token, &[0]);
                continue;
            }
            // Its queue closed: what runs stopped, what waits lost.
            4 => {
                let w = with(|st| {
                    let k = st.slots.iter().position(|sl| sl.client == Some(stamp.key()) && sl.queue.is_some())?;
                    queue_end(st, k, q::E_CLOSED, false)
                });
                if let Some(a) = w {
                    // SAFETY: the caller's own block, held by it (it is in this call): its `SUBMIT` and `DONE` words.
                    ferros_sys::wake(unsafe { &*(a as *const AtomicU64) }, u64::MAX);
                    ferros_sys::wake(unsafe { &*((a + q::DONE * 8) as *const AtomicU64) }, u64::MAX);
                }
                ferros_sys::reply(&token, &[0]);
                continue;
            }
            _ => {}
        }
        // A run (op 1).
        if m.len() < 18 || m[0] != 1 {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        let (preload, code_off, words) = (u16_at(1) as u32, u32_at(3) as usize, u16_at(7) as usize);
        let groups = (u32_at(9), u32_at(13));
        let nu = m[17] as usize;
        // The request's own lengths checked against what was sent, the code against the client's own block: they come from another process.
        let fits = m.len() >= 18 + 8 * nu && nu <= UNIFORMS && words > 0 && words + 16 <= CODE_PAGES * PAGE / 8 && code_off % 8 == 0 && code_off + 8 * words <= bsize as usize && groups.0 > 0 && groups.1 > 0;
        if !fits {
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        let code = unsafe { core::slice::from_raw_parts((bbase as usize + code_off) as *const u64, words) };
        let (client, rest) = label(&m[18 + 8 * nu..]);
        let (kernel, _) = label(rest);
        // WHY: a kernel's first word is an instruction, and the zero word is none a client emits (opcode 0 is NOP, which the valhall builder has no form for, and even Mesa's NOP sets bits 47:46: NOP.end is G#7800_C000_0000_0000, assembler-cases.txt). Zeros there mean the client's words never reached the arena (2026-11-11: masked ahau's pointer stores landed in its own arc).
        if code[0] == 0 {
            say!(grants, "gpu: refused {client}'s {kernel} kernel: its code at G#{code_off:X} in its block ({words} words) starts with a zero word, so its words never reached the arena\n");
            reply[0] = 3;
            ferros_sys::reply(&token, &reply[..reply_len]);
            continue;
        }
        let bell = CALL_BELL.load(Ordering::Acquire);
        let put = with(|st| {
            let k = slot_for(st, &stamp, bbase, bsize)?;
            if !ensure_running(s, &mut st.slots[k], k) {
                say!(grants, "gpu: slot {k} (address space {}) did not start for {client}\n", k + 1);
                return Some(None);
            }
            let sl = &mut st.slots[k];
            // The call's code at the start of the slot's code region, its descriptor and uniforms in the descriptor page (one call at a time: the main thread waits for it).
            for (j, w) in code.iter().enumerate() {
                wr64(sl.code + 8 * j, *w);
            }
            for j in words..(words + 16).max(PAGE / 8) {
                wr64(sl.code + 8 * j, 0);
            }
            let (spd0, pre) = if preload & 0x8000 != 0 { (0x0000_0018u32, preload & 0x7FFF) } else { (0x8000_0018, preload) };
            for (j, w) in [spd0, pre, CODE_BIG_VA as u32, (CODE_BIG_VA >> 32) as u32, 0, 0, 0, 0].iter().enumerate() {
                wr32(sl.desc + 4 * j, *w);
            }
            for j in 0..nu {
                wr64(sl.desc + 0x80 + 8 * j, u64::from_le_bytes(m[18 + 8 * j..26 + 8 * j].try_into().unwrap()));
            }
            wr64(sl.flag + CALL_STAMPS + 8, 0);
            wr64(sl.flag + CALL_STAMPS + 16, 0);
            let seq = sl.seq;
            let d = q::Dispatch { spd: DESC_VA, tsd: DESC_VA + 0x40, fau: DESC_VA + 0x80, uniforms: nu as u32, groups, max_threads: s.max_threads, stamps: s.stamps.then_some(OUT_VA + CALL_STAMPS as u64), sync: OUT_VA + SEQ_AT as u64, value: seq, wg_log2: 4, merge: true };
            let mut jw = [0u64; q::JOB_WORDS_MOST];
            let len = q::job_words(&d, &mut jw);
            append(s, sl, k, &jw[..len]);
            sl.seq = seq.wrapping_add(1);
            st.jobs += 1;
            st.slots[k].used = st.jobs;
            fly(st, k, Flight { seq, job: CALL, refused: false, submitted: ferros_sys::now() });
            Some(Some(k))
        });
        let k = match put {
            None => {
                ferros_sys::reply(&token, &reply[..reply_len]);
                continue;
            }
            Some(None) => {
                reply[0] = 2;
                ferros_sys::reply(&token, &reply[..reply_len]);
                continue;
            }
            Some(Some(k)) => k,
        };
        // The call's job waited for (`events` moves the bell when it ends, done or failed): the next request is taken only after it.
        // Its guard is this thread's: asleep until the job ends or its guard passes, then the slot looked at (`guard_check`), which ends a late job with its group and answers the call.
        while CALL_BELL.load(Ordering::Acquire) == bell {
            let deadline = with(|st| guard_at(s, &st.slots[k]));
            ferros_sys::wait(&CALL_BELL, bell, deadline);
            if let Some(w) = guard_check(k) {
                // SAFETY: as in `events`.
                ferros_sys::wake(unsafe { &*(w as *const AtomicU64) }, u64::MAX);
            }
        }
        let (status, us, gpu_us, addr, fstatus) = with(|st| st.call);
        reply[0] = status;
        reply[1..5].copy_from_slice(&us.to_le_bytes());
        if status == 0 {
            if gpu_us != 0 {
                reply[5..9].copy_from_slice(&gpu_us.to_le_bytes());
                reply_len = 9;
            }
        } else if status == OUTSIDE {
            reply[5..13].copy_from_slice(&addr.to_le_bytes());
            reply[13..17].copy_from_slice(&fstatus.to_le_bytes());
            reply_len = 17;
            say!(grants, "gpu: {client}'s {kernel} kernel reached outside its memory: GPU address G#{addr:X}, AS_FAULTSTATUS G#{fstatus:X} ({}) in its own address space {}; its job ended\n", exception(fstatus), k + 1);
        } else {
            let sl_code = with(|st| st.slots[k].code);
            let ran = |w: usize| unsafe { core::ptr::read_volatile((sl_code + 8 * w) as *const u64) };
            say!(grants, "gpu: {client}'s {kernel} kernel failed: {words} words from G#{code_off:X} in its block, first G#{:X} G#{:X}, last G#{:X}; {} x {} groups, preload G#{preload:X}, {nu} uniforms; {us} us\n", ran(0), ran(1), ran(words - 1), groups.0, groups.1);
        }
        ferros_sys::reply(&token, &reply[..reply_len]);
    }
}
