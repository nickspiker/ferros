//! Processes: code at EL0 in an arc, talking to the world only through capabilities (KERNEL.md: Execution Model, Modules 2-4; ORACLE.md spawn).
//!
//! A process is one arc — a power-of-two, aligned span of the arc pool, zeroed when allocated — and a capability space. The kernel enters it with `eret` to EL0 holding the arc in two registers the code may not write (x28 = base, x27 = mask); compiled code reaches memory only as `base + (offset & mask)`. EL0 itself takes away the system controls: a process can't mask the killswitch, issue SMCs or touch MMIO, DMA or kernel memory (the static map gives EL0 the pool only).
//!
//! The syscall boundary is `svc #0` with the call number in x8:
//! - 0 `exit(code)` — the process ends; its arc and capabilities cease to exist.
//! - 1 `call(cap, msg, msg_len, reply, reply_cap)` — present a 32-byte capability token (at arc offset `cap`) and send `msg`; the reply is written at `reply` (at most `reply_cap` bytes) and its length returned in x0. All offsets are arc offsets, masked by the kernel.
//! There is no "print", no PID and no ambient authority: what a process may reach is exactly the tokens it was granted at spawn (written at arc offset 0: u32 count, then the 32-byte tokens). An object you hold no token for does not exist for you — an unknown token gets the same answer as a wrong one.
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

/// An arc: `size` bytes at `base`, both powers of two, base aligned to size.
#[derive(Clone, Copy)]
pub struct Arc {
    pub base: usize,
    pub size: usize,
}

impl Arc {
    pub fn mask(&self) -> usize {
        self.size - 1
    }
}

/// Allocate the smallest arc of at least `bytes` (buddy split from the pool), zeroed: the slack beyond what was asked for is zero and belongs to this arc alone.
pub fn arc_alloc(bytes: usize) -> Option<Arc> {
    let want = bytes.max(1 << ARC_MIN_LOG2).next_power_of_two();
    let k = want.trailing_zeros() as usize;
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
    Some(Arc { base, size: want })
}

/// Return an arc to the pool, merging buddies.
pub fn arc_free(arc: Arc) {
    let a = arcs();
    let (p0, p1) = crate::cachemap::POOL;
    let mut base = arc.base;
    let mut k = arc.size.trailing_zeros() as usize;
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

/// Kernel objects a capability can name. The console is served by the kernel itself: what a process sends it is collected for whoever spawned the process.
#[derive(Clone, Copy, PartialEq)]
pub enum Object {
    Console,
}

pub const RIGHT_SEND: u32 = 1;

struct CapEntry {
    token: [u8; 32],
    object: Object,
    rights: u32,
}

const CAP_DOMAIN: &[u8] = b"ferros.cap.v1";
const CAP_KEY_CONTEXT: &str = "ferros.cap-key.v1";

static EPOCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The per-session key every token is MACed under (derived from the wairua, so spawning is the first thing that needs KOHI). None = no wairua: refuse to spawn.
fn cap_key() -> Option<[u8; 32]> {
    crate::wairua::draw().map(|w| blake3::derive_key(CAP_KEY_CONTEXT, &w))
}

fn mint(key: &[u8; 32], object: Object, rights: u32, nonce: u64, epoch: u64) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(CAP_DOMAIN);
    h.update(&[object as u8]);
    h.update(&rights.to_le_bytes());
    h.update(&nonce.to_le_bytes());
    h.update(&epoch.to_le_bytes());
    *h.finalize().as_bytes()
}

// ---------------------------------------------------------------- processes

struct Process {
    arc: Arc,
    cspace: Vec<CapEntry>,
    console: Vec<u8>,
}

static CURRENT: Global<Option<Process>> = Global(UnsafeCell::new(None));

fn current() -> Option<&'static mut Process> {
    unsafe { (*CURRENT.0.get()).as_mut() }
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
    /// Find the capability for a presented token: compare against every entry without an early exit, so the time taken says nothing about which (if any) matched.
    fn lookup(&self, token: &[u8]) -> Option<(Object, u32)> {
        let mut hit: Option<(Object, u32)> = None;
        for e in &self.cspace {
            let mut diff = 0u8;
            for (a, b) in e.token.iter().zip(token.iter()) {
                diff |= a ^ b;
            }
            if diff == 0 && token.len() == 32 {
                hit = Some((e.object, e.rights));
            }
        }
        hit
    }
}

/// Saved process registers on exception entry from EL0 (built by `.Lel0_sync`).
#[repr(C)]
pub struct Frame {
    pub x: [u64; 31],
    pub sp_el0: u64,
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
    pub far: u64,
}

const SYS_EXIT: u64 = 0;
const SYS_CALL: u64 = 1;
const ERR: u64 = u64::MAX;

/// Called from `.Lel0_sync` for every synchronous exception taken from EL0. Returns 0 to resume the process, 1 to end it (the exit value is then in x0).
#[unsafe(no_mangle)]
pub extern "C" fn ferros_el0_trap(f: &mut Frame) -> u64 {
    let ec = (f.esr >> 26) & 0x3F;
    let Some(p) = current() else {
        f.x[0] = ERR;
        return 1;
    };
    if ec != 0x15 {
        // Not a syscall: the process faulted. It ends; the kernel carries on.
        f.x[0] = FAULT | ec << 40 | (f.elr.wrapping_sub(p.arc.base as u64) & 0xFF_FFFF) << 16 | (f.far.wrapping_sub(p.arc.base as u64) & 0xFFFF);
        return 1;
    }
    match f.x[8] {
        SYS_EXIT => 1,
        SYS_CALL => {
            let token = p.read(f.x[0], 32);
            f.x[0] = match p.lookup(&token) {
                Some((Object::Console, r)) if r & RIGHT_SEND != 0 => {
                    let msg = p.read(f.x[1], (f.x[2] as usize).min(4096));
                    p.console.extend_from_slice(&msg);
                    let reply = b"ok";
                    let n = reply.len().min(f.x[4] as usize);
                    p.write(f.x[3], &reply[..n]);
                    n as u64
                }
                _ => ERR,
            };
            0
        }
        _ => {
            f.x[0] = ERR;
            0
        }
    }
}

/// Exit value marking a fault: FAULT | EC << 40 | faulting PC (arc offset) << 16 | fault address (arc offset, low 16 bits).
pub const FAULT: u64 = 0xFA << 56;

unsafe extern "C" {
    /// main.rs: enter EL0 at `entry` with SP_EL0 = `sp`, x28 = arc base, x27 = arc mask, x0..x2 = args; returns the exit value when the process ends.
    fn ferros_el0_enter(entry: usize, sp: usize, base: usize, mask: usize, a0: u64, a1: u64, a2: u64) -> u64;
}

/// What a finished process left behind.
pub struct Outcome {
    pub exit: u64,
    pub console: Vec<u8>,
    pub arc: Arc,
}

/// Arc layout: grants page, then code.
const GRANTS: usize = 0;
const CODE: usize = 0x1000;
const STACK: usize = 64 * 1024;

/// Spawn `code` (position-independent, entry at its offset 0) as a process with a console capability, run it to exit, and return what it left. `input` is copied into the arc after the code; x0 = grants offset, x1 = input offset, x2 = input length.
pub fn spawn(code: &[u8], input: &[u8]) -> Result<Outcome, &'static str> {
    if !crate::cachemap::vhe() {
        return Err("no VHE: processes need the EL2&0 regime");
    }
    let key = cap_key().ok_or("no wairua: refusing to mint capabilities")?;
    let arc = arc_alloc(CODE + code.len() + input.len() + STACK).ok_or("arc pool exhausted")?;
    let epoch = EPOCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    let console = mint(&key, Object::Console, RIGHT_SEND, 0, epoch);
    let p = Process { arc, cspace: alloc::vec![CapEntry { token: console, object: Object::Console, rights: RIGHT_SEND }], console: Vec::new() };
    // Grants page: count, then the tokens (slot 0 = console). Then the code, then the input.
    p.write(GRANTS as u64, &1u32.to_le_bytes());
    p.write((GRANTS + 4) as u64, &console);
    p.write(CODE as u64, code);
    let in_off = CODE + code.len();
    p.write(in_off as u64, input);
    crate::cachemap::sync_code(arc.base + CODE, code.len());
    unsafe { *CURRENT.0.get() = Some(p) };
    let exit = unsafe { ferros_el0_enter(arc.base + CODE, arc.base + arc.size - 16, arc.base, arc.mask(), GRANTS as u64, in_off as u64, input.len() as u64) };
    let p = unsafe { (*CURRENT.0.get()).take() }.ok_or("process vanished")?;
    // Its tokens die with its epoch; its arc goes back to the pool (zeroed again on the next allocation).
    arc_free(p.arc);
    Ok(Outcome { exit, console: p.console, arc: p.arc })
}
