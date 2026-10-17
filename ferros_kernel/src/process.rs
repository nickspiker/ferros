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
// Single core, never touched from the IRQ handler.
unsafe impl<T> Sync for Global<T> {}

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
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Running,
    Runnable,
    /// In recv: (endpoint, buf, buf_cap, reply-token offset).
    Receiving(usize, u64, u64, u64),
    /// Called and waiting for the reply: (reply offset, reply cap).
    Calling(u64, u64),
}

struct Process {
    arc: Arc,
    cspace: Vec<CapEntry>,
    frame: Frame,
    state: State,
    epoch: u64,
    name: Vec<u8>,
    /// The process's console output is at the start of a line (so the next byte gets the name tag).
    line_start: bool,
    /// The spawn input, kept in kernel memory and pulled in by the `input` syscall (so a driver's arc stays small while large one-shot data — firmware — streams through a chunk buffer). Inlined into the arc too only when it is small.
    input: Vec<u8>,
}

struct Endpoint {
    name: Vec<u8>,
    /// Calls waiting for the server: (caller, message).
    queue: alloc::collections::VecDeque<(usize, Vec<u8>)>,
}

struct Kernel {
    procs: Vec<Option<Process>>,
    endpoints: Vec<Endpoint>,
    runnable: alloc::collections::VecDeque<usize>,
    current: Option<usize>,
    /// Everything any process sent its console during the current kernel entry, each line tagged with the sender's name.
    console: Vec<u8>,
    /// Processes that ended during the current entry: (process, exit value).
    exited: Vec<(usize, u64)>,
    ipc_round_trips: u32,
}

static KERNEL: Global<Kernel> = Global(UnsafeCell::new(Kernel {
    procs: Vec::new(),
    endpoints: Vec::new(),
    runnable: alloc::collections::VecDeque::new(),
    current: None,
    console: Vec::new(),
    exited: Vec::new(),
    ipc_round_trips: 0,
}));

fn k() -> &'static mut Kernel {
    unsafe { &mut *KERNEL.0.get() }
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
    kk.endpoints.push(Endpoint { name: name.to_vec(), queue: alloc::collections::VecDeque::new() });
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
];
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
    let Some(sp) = kk.procs[server].as_mut() else { return };
    let State::Receiving(_, buf, cap, tok_off) = sp.state else { return };
    let n = msg.len().min(cap as usize);
    sp.write(buf, &msg[..n]);
    let token = mint(Object::Reply(caller), RIGHT_SEND, sp.epoch);
    sp.cspace.push(CapEntry { token, object: Object::Reply(caller), rights: RIGHT_SEND });
    sp.write(tok_off, &token);
    sp.frame.x[0] = n as u64;
    sp.state = State::Runnable;
    kk.runnable.push_back(server);
}

/// Take the next runnable process (if any) and make it the one the trap returns into.
fn switch_to_next(f: &mut Frame) -> bool {
    let kk = k();
    while let Some(next) = kk.runnable.pop_front() {
        if let Some(np) = kk.procs[next].as_mut() {
            if np.state == State::Runnable {
                np.state = State::Running;
                *f = np.frame;
                kk.current = Some(next);
                return true;
            }
        }
    }
    kk.current = None;
    false
}

/// Called from `.Lel0_sync` for every synchronous exception taken from EL0, with the running process's live frame. Returns 0 to eret through `f` (which may now be another process's frame), 1 to leave EL0 (value in f.x[0]).
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_trap(f: &mut Frame) -> u64 {
    let kk = k();
    let Some(cur) = kk.current else {
        f.x[0] = ERR;
        return 1;
    };
    let ec = (f.esr >> 26) & 0x3F;
    let base = kk.procs[cur].as_ref().map(|p| p.arc.base as u64).unwrap_or(0);
    let end = |f: &mut Frame, value: u64| -> u64 {
        let kk = k();
        if let Some(p) = kk.procs[cur].take() {
            // Say how it ended on the console: a process that outlives its first slice (spawn already returned) has no one else to hear its exit value — a fault or a panic would otherwise vanish.
            if !p.line_start {
                kk.console.push(b'\n');
            }
            kk.console.extend_from_slice(&p.name);
            kk.console.extend_from_slice(b": exit G#");
            for i in (0..16).rev() {
                kk.console.push(b"0123456789ABCDEF"[((value >> (4 * i)) & 0xF) as usize]);
            }
            kk.console.push(b'\n');
            arc_free(p.arc);
        }
        kk.exited.push((cur, value));
        if switch_to_next(f) { 0 } else { f.x[0] = PARKED; 1 }
    };
    if ec != 0x15 {
        // Not a syscall: the process faulted. It ends; the kernel carries on. Its full state goes to the console first (ESR, ELR/FAR/SP as arc offsets, every register), so a fault is debuggable from the console alone.
        let v = FAULT | ec << 40 | (f.elr.wrapping_sub(base) & 0xFF_FFFF) << 16 | (f.far.wrapping_sub(base) & 0xFFFF);
        fault_report(f, base);
        return end(f, v);
    }
    let p = kk.procs[cur].as_mut().unwrap();
    match f.x[8] {
        SYS_EXIT => end(f, f.x[0]),
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
                    p.state = State::Calling(f.x[3], f.x[4]);
                    p.frame = *f;
                    kk.endpoints[ep].queue.push_back((cur, msg));
                    // A server already waiting on this endpoint takes it now.
                    if let Some(server) = kk.procs.iter().position(|q| matches!(q, Some(q) if matches!(q.state, State::Receiving(e, ..) if e == ep))) {
                        deliver(server, ep);
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
                    f.x[0] = NOTHING;
                    0
                }
                Some((Object::Endpoint(ep), r)) if r & RIGHT_RECV != 0 => {
                    p.state = State::Receiving(ep, f.x[1], f.x[2], f.x[3]);
                    p.frame = *f;
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
                    if let Some(cp) = kk.procs[caller].as_mut() {
                        if let State::Calling(roff, rcap) = cp.state {
                            let n = msg.len().min(rcap as usize);
                            cp.write(roff, &msg[..n]);
                            cp.frame.x[0] = n as u64;
                            cp.state = State::Runnable;
                            kk.runnable.push_back(caller);
                            kk.ipc_round_trips += 1;
                        }
                    }
                    f.x[0] = 0;
                    0
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
    }
}

/// A faulting process's state on the console: `name: FAULT esr … elr+… far+… sp+…` then x0..x30, eight per line (G# hex; the three addresses as offsets into its arc).
fn fault_report(f: &Frame, base: u64) {
    let kk = k();
    let Some(cur) = kk.current else { return };
    let name = kk.procs[cur].as_ref().map(|p| p.name.clone()).unwrap_or_default();
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
    let kk = k();
    if let Some(cur) = kk.current {
        if let Some(p) = kk.procs[cur].as_mut() {
            p.frame = *f;
            p.state = State::Runnable;
            kk.runnable.push_back(cur);
        }
    }
    kk.current = None;
    f.x[0] = PREEMPTED;
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
pub fn run_slice() -> bool {
    let kk = k();
    let Some(next) = kk.runnable.pop_front() else { return false };
    let Some(p) = kk.procs[next].as_mut() else { return true };
    if p.state != State::Runnable {
        return true;
    }
    p.state = State::Running;
    let frame = p.frame;
    kk.current = Some(next);
    arm_slice();
    unsafe { ferros_el0_resume(&frame) };
    disarm_slice();
    k().current = None;
    true
}

/// Append to the kernel's process console (e.g. a boot-time spawn's output, kept for `diag console`).
pub fn push_console(bytes: &[u8]) {
    k().console.extend_from_slice(bytes);
}

/// Drain what processes have sent their consoles since the last drain.
pub fn take_console() -> Vec<u8> {
    core::mem::take(&mut k().console)
}

/// How many processes are alive (running, runnable or blocked).
pub fn alive() -> usize {
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
pub enum Grant<'a> {
    Serve(&'a [u8]),
    Use(&'a [u8]),
    /// A hardware grant from the registry, by name.
    Hardware(&'a [u8]),
    /// The entropy right (syscall 6).
    Entropy,
    /// A DMA region from the registry, by name (syscall 7).
    Dma(&'a [u8]),
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
    cap_key().ok_or("no wairua: refusing to mint capabilities")?;
    let inline = input.len(); // bridge: full inline (pcie1 still reads input from its arc); sandboxed apps carry no input, so their arcs stay small either way. INLINE_INPUT becomes the cap once pcie1 streams firmware via SYS_INPUT (option 1).
    let arc = arc_alloc(CODE + code.len() + inline + STACK, guard).ok_or("arc pool exhausted")?;
    let epoch = EPOCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    let mut cspace = alloc::vec![CapEntry { token: mint(Object::Console, RIGHT_SEND, epoch), object: Object::Console, rights: RIGHT_SEND }];
    for g in grants {
        let (object, rights) = match g {
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
        };
        cspace.push(CapEntry { token: mint(object, rights, epoch), object, rights });
    }
    let zero = Frame { x: [0; 31], sp_el0: 0, elr: 0, spsr: 0, esr: 0, far: 0 };
    let p = Process { arc, cspace, frame: zero, state: State::Running, epoch, name: name.to_vec(), line_start: true, input: input.to_vec() };
    // Grants page: count, then the tokens in slot order. Then the code, then the input.
    p.write(GRANTS as u64, &(p.cspace.len() as u32).to_le_bytes());
    for (i, c) in p.cspace.iter().enumerate() {
        p.write((GRANTS + 4 + 32 * i) as u64, &c.token);
    }
    if let Some(i) = ihi {
        p.write(IHI as u64, &i);
    }
    p.write(CODE as u64, code);
    let in_off = CODE + code.len();
    p.write(in_off as u64, input);
    crate::cachemap::sync_code(arc.base + CODE, code.len());
    // W^X (SANDBOX.md): a masked process may execute only its checked .text ([CODE, CODE+exec_len)), read-only, and may not write it; everything else is non-executable. A native dev process gets the whole arc RWX. The arc returns to writable data on free (arc_free → arc_reset).
    crate::cachemap::set_arc_wx(arc.base, arc.block, arc.base + CODE, arc.base + CODE + exec_len, !guard);
    let kk = k();
    kk.exited.clear();
    let trips0 = kk.ipc_round_trips;
    let me = match kk.procs.iter().position(|q| q.is_none()) {
        Some(i) => {
            kk.procs[i] = Some(p);
            i
        }
        None => {
            kk.procs.push(Some(p));
            kk.procs.len() - 1
        }
    };
    kk.current = Some(me);
    arm_slice();
    unsafe { ferros_el0_enter(arc.base + CODE, arc.base + arc.size - 16, arc.base, arc.mask(), GRANTS as u64, in_off as u64, input.len() as u64) };
    disarm_slice();
    let kk = k();
    kk.current = None;
    let exit = match kk.exited.iter().find(|&&(i, _)| i == me) {
        Some(&(_, v)) => v,
        None if matches!(kk.procs[me].as_ref().map(|p| p.state), Some(State::Runnable)) => PREEMPTED,
        None => PARKED,
    };
    Ok(Outcome { exit, console: core::mem::take(&mut kk.console), round_trips: kk.ipc_round_trips - trips0 })
}
