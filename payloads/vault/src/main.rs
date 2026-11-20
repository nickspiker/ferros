//! The vault process (docs/VAULT-OUT.md): the vault out of the kernel. It owns its disk (a window of blocks the kernel moves for it, `ferros_sys::block_read`/`block_write`), the layout (manifestus: the rotating ring, the tract, the HAMT, two mirrors), and the values of any size, sealed in pieces (`ferros_store`). The kernel moves blocks and issues each request's domain keys; it holds, parses and seals no value.
//!
//! The mirrors: A from block 0 of the window, B from half way (the window is 2 × 8 GiB, process.rs WINDOWS). A blank region starts at MIRROR blocks (512 MiB), because a blank vault is read whole before its first commit and 8 GiB would be four million reads; once a vault is there each mirror opens bounded by its half and grows into it (manifestus `grow`, `Half::grow` zeroing from the committed extent).
//!
//! Requests come only from the kernel: each is a process's `SYS_STORE` call (process.rs `store_forward`) or the kernel's own call (`vault_call`: the bridge's store commands, and op 4, drop, at release), or a process's export (`SYS_UNLOCK` op 5, forwarded: op 5 here), stamped with the domain keys of its scope, one transfer at most. [op: 0 set, 1 get, 2 delete, 3 list][flags: bit 0 old entries wait in the kernel's vault][lock epoch u32 LE][domains][per domain: class (1 the person's), address key, sealing key][name length][name: within the scope, or 1 and a 32-byte address][rest]. Rest: set [offset u64][more u8][bytes]; get [offset u64][length u32]; list [offset u64][length u32][addresses u8]. The reply: [the syscall's value u64 LE][the bytes for the caller's buffer].
//!
//! Keys live for the request they came with: zeroed after it, and the store's cache knows a key only by its hash. When the lock epoch moves (the person locked), the cache's plaintext and any write of the person's in progress go.
//!
//! Migration (VAULT-OUT.md): whenever a stamp says old entries wait (from boot, after a bridge `store put`, at unlock), the vault takes each old entry of the kernel's vault that belongs here (`SYS_VAULT`): its domain keys and its value, written into its domain, then removed from the kernel's vault once committed.
//!
//! Grant slots: 0 console, 1 serve `vault`, 2 the `vault` block window, 3 entropy, 4 the vault right.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst};
use core::sync::atomic::{AtomicU64, AtomicUsize};
use ferros_store::{Domain, Nonces, Opener, Sealed, Store, Writer};
use ferros_sys::{cap, console, freq, now, ERR, NOTHING};
use manifestus::{Block, BlockDev, DeletePolicy, Mirror, Vault};

/// The heap: manifestus's working set (its index walks, the reap's windows) and a piece being opened or sealed. 24 MiB from 2026-11-08: a write grows a buffer of manifestus's with the vault's entries (about 926 bytes an entry, doubling: 671,760 bytes writing a 47 MB kist capsule into an empty store, 1,343,504 writing it again, measured on the host, ferros_store `rewrite_peak`), and at 8 MiB the vault process died rewriting Mahere's capsule. 56 MiB since 2026-11-12 (the arc 64 MiB): on the 8a, after Mahere's cells, three Mahere capsules and an offer of Photon cut off at 64 MiB, the open itself ran 24 MiB out ("memory allocation of 69768 bytes failed" before "open in"), every boot, and the GPU, WiFi and sensors processes, which read their firmware from this vault, ended with it. The open's high water is on the console ("open in"). 2026-11-12, the 8a: it ran out again inside manifestus's reap while a 91 MB capsule was dropped (a reap window is a sixty-fourth of the tract, 2 GiB then 4 GiB there, and its survivors were held twice); manifestus `bounded-reap` writes them out 256 blocks at a time, and the same run of offers that ended it twice then left it alive (docs/VAULT-OUT.md "Its end").
const HEAP_BYTES: usize = 56 << 20;
const STACK_BYTES: usize = 1 << 18;

#[repr(C, align(16))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_BYTES> = Region(UnsafeCell::new([0; HEAP_BYTES]));
static STACK: Region<STACK_BYTES> = Region(UnsafeCell::new([0; STACK_BYTES]));

core::arch::global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym vault_main
);

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

const S_SERVE: u64 = 1;
const S_BLOCKS: u64 = 2;
const S_ENTROPY: u64 = 3;
const S_VAULT: u64 = 4;

/// Past the four a `SYS_STORE` call makes (process.rs `VOP_*`): drop every value of each stamped domain (the kernel's own call, at release), and the export.
const OP_DROP: u8 = 4;
/// The person's export as one VSF document (a process's `SYS_UNLOCK` op 5, forwarded), a transfer at a time.
const OP_EXPORT: u8 = 5;
/// How many values each stamped domain holds and their bytes in all (`ferros_store` `size`, from the heads, no value read): a kist's own domain on its card's unstow sheet (docs/KISTS.md "The unstow sheet"), stamped by the kernel for the stower (process.rs `VOP_SIZE`). The value: the bytes; the reply's bytes: the count of values, u64 LE.
const OP_SIZE: u8 = 6;
/// A single-copy extent (process.rs `VOP_EXTENT`, `extent_forward`): rest [extent op][block u64][length u32][bytes]; see `extent`.
const OP_EXTENT: u8 = 7;
/// The extent ops (process.rs `EOP_*`).
const EOP_OPEN: u8 = 0;
const EOP_WRITE: u8 = 1;
const EOP_SEAL: u8 = 2;
const EOP_READ: u8 = 3;
const EOP_INFO: u8 = 4;
const EOP_DELETE: u8 = 5;
const EOP_SPACE: u8 = 6;
/// The kernel's own channel only (the bridge's `store extent-list` / `extent-drop`; process.rs `extent_forward` refuses both from a process): the area's extents as records, and an extent dropped by its first block, its domain's alone. An extent's name is a derived key, never stored, so a clip whose name is lost (a writer that died before saying it) is found and dropped by these.
const EOP_LIST: u8 = 7;
const EOP_DROP_AT: u8 = 8;
/// The bridge's `store extent-drop-orphan` (the kernel's own channel, development builds): a sealed extent at a first block dropped whatever domain owns it, for one whose owning build no longer exists.
const EOP_DROP_ORPHAN: u8 = 9;
/// One record of EOP_LIST: first, blocks, bytes (u64 LE each), sealed (u8), at (i64 LE), owner (32).
const LIST_ROW: usize = 8 + 8 + 8 + 1 + 8 + 32;
/// Grant slot 5: the `clips` block window, the extents' area (process.rs WINDOWS; a kernel from before grants none, and every extent op is refused).
const S_CLIPS: u64 = 5;
/// The area's blocks: process.rs CLIPS_BLOCKS (48 GiB); the kernel bounds every call to the window whatever this says.
const CLIPS_AREA: u64 = 12 << 20;
/// The largest extent transfer (process.rs EXTENT_MAX): one UFS command.
const EXTENT_MAX: usize = 1 << 20;

/// Each mirror's size to start with, in 4 KiB blocks (512 MiB), and where B starts in the window (its second half).
const MIRROR: u64 = 1 << 17;
const HALF: u64 = 1 << 21;
/// The ring's size, as the kernel's vault has it (manifestus's host default).
const RING_LOG2: u8 = manifestus::HOST_RING_LOG2;
/// Blocks moved by one kernel call at most (process.rs BLOCK_MAX).
const RUN: usize = 1 << 4;

static mut GRANTS: u64 = 0;
/// Spawned to start the region afresh (input bit 0, by hand).
static mut FRESH: bool = false;

fn grants() -> u64 {
    unsafe { GRANTS }
}

/// One mirror: `blocks` blocks of the window from `base`.
struct Half {
    base: u64,
    blocks: u64,
}

impl Half {
    fn io(&self, lba: u64, n: u64) -> manifestus::Result<()> {
        if lba + n > self.blocks {
            return Err(manifestus::Error::Bounds(lba));
        }
        Ok(())
    }
}

impl BlockDev for Half {
    fn block_count(&self) -> u64 {
        self.blocks
    }
    fn read(&mut self, lba: u64, buf: &mut Block) -> manifestus::Result<()> {
        self.io(lba, 1)?;
        // A block the read-ahead holds (a piece's leaf, just past its furrows) is a copy, from the window or the next one read beside it (which then becomes the window); any other is one block, never a fill: the index walk's nodes lie anywhere.
        let at = self.base + lba;
        let next = unsafe { AHEAD_ON && NEXT_POSTED && at >= NEXT_AT.0 && at < NEXT_AT.0 + NEXT_AT.1 as u64 };
        if !ahead_copy(at, buf) && !(next && ahead_run(at, buf, self.base + self.blocks)) && !blocks_read(at, buf, &IO_TICKS) {
            return Err(manifestus::Error::Corrupt(alloc::format!("block read failed at {lba}")));
        }
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &Block) -> manifestus::Result<()> {
        self.io(lba, 1)?;
        ahead_drop();
        if ferros_sys::block_write(cap(grants(), S_BLOCKS), self.base + lba, buf) != 0 {
            return Err(manifestus::Error::Corrupt(alloc::format!("block write failed at {lba}")));
        }
        Ok(())
    }
    /// Up to `RUN_NOW` blocks a kernel call (one UFS command each on a kernel from 2026-11-08 on). While a large value is read in bulk (`AHEAD_ON`), a run comes thru the read-ahead (`ahead_run`): a copy from the window, or from the next window read beside it, or a fresh fill from the run's first block.
    fn read_run(&mut self, lba: u64, out: &mut [Block]) -> manifestus::Result<()> {
        self.io(lba, out.len() as u64)?;
        let bytes = unsafe { core::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, out.len() << 12) };
        let at = self.base + lba;
        let ok = ahead_copy(at, bytes) || (unsafe { AHEAD_ON } && out.len() <= AHEAD && ahead_run(at, bytes, self.base + self.blocks)) || blocks_read(at, bytes, &IO_TICKS);
        if !ok {
            return Err(manifestus::Error::Corrupt(alloc::format!("block read failed at {lba}")));
        }
        Ok(())
    }
    fn write_run(&mut self, lba: u64, blocks: &[&Block]) -> manifestus::Result<()> {
        self.io(lba, blocks.len() as u64)?;
        ahead_drop();
        let mut buf = alloc::vec![0u8; RUN_MAX << 12];
        for (k, chunk) in blocks.chunks(RUN_MAX).enumerate() {
            for (j, b) in chunk.iter().enumerate() {
                buf[j << 12..(j + 1) << 12].copy_from_slice(&b[..]);
            }
            if !blocks_write(self.base + lba + (k * RUN_MAX) as u64, &buf[..chunk.len() << 12]) {
                return Err(manifestus::Error::Corrupt(alloc::format!("block write failed at {lba}")));
            }
        }
        Ok(())
    }
    /// Each block is on the flash when the kernel's call returns (the HAL polls the command to completion).
    fn flush(&mut self) -> manifestus::Result<()> {
        Ok(())
    }
    /// A grow without the old extent: refused. This device is a window larger than the vault's extent, so only the engine knows where the old blocks end; manifestus's tract grows call `grow_from` with it.
    fn grow(&mut self, new_blocks: u64) -> manifestus::Result<()> {
        Err(manifestus::Error::Corrupt(alloc::format!("a grow to {new_blocks} blocks without the old extent: refused (the window zeroes only from a known extent)")))
    }
    /// The tract grown from `old_blocks` to `new_blocks` (the device's whole extent before and after, manifestus's space-first rule): exactly the new blocks zeroed, nothing below. 2026-11-08: this zeroed from a mark of its own that a grow during the vault's open found still unset, and wiped both mirrors from block 0 (the vault had filled at its first 512 MiB, where the default refused to grow).
    fn grow_from(&mut self, old_blocks: u64, new_blocks: u64) -> manifestus::Result<()> {
        // WHY: a half of the window is all one mirror has; past it the vault is full, and a write must fail rather than reach the other mirror.
        if new_blocks > self.blocks {
            return Err(manifestus::Error::Corrupt(alloc::format!("the vault's window is full: {new_blocks} blocks wanted, {} in a mirror", self.blocks)));
        }
        ahead_drop();
        let zero = alloc::vec![0u8; RUN << 12];
        let mut at = old_blocks;
        while at < new_blocks {
            // A run of up to RUN blocks a call, the last one shorter.
            let n = (new_blocks - at).min(RUN as u64) as usize;
            if ferros_sys::block_write(cap(grants(), S_BLOCKS), self.base + at, &zero[..n << 12]) != 0 {
                return Err(manifestus::Error::Corrupt(alloc::format!("zeroing the grown tract failed at {at}")));
            }
            at += n as u64;
        }
        Ok(())
    }
}

/// Nonces from the kernel's entropy (the session's wairua), drawn 4 KiB at a time.
struct Entropy {
    pool: [u8; 4096],
    at: usize,
}

impl Nonces for Entropy {
    fn nonce(&mut self) -> [u8; 24] {
        if self.at + 24 > self.pool.len() {
            if ferros_sys::entropy(cap(grants(), S_ENTROPY), &mut self.pool) == ERR {
                panic!("vault: no entropy for a nonce");
            }
            self.at = 0;
        }
        let mut n = [0u8; 24];
        n.copy_from_slice(&self.pool[self.at..self.at + 24]);
        self.at += 24;
        n
    }
}


struct Log {
    b: [u8; 512],
    n: usize,
}

impl core::fmt::Write for Log {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}

macro_rules! say {
    ($($a:tt)*) => {{
        let mut l = Log { b: [0; 512], n: 0 };
        let _ = core::fmt::Write::write_fmt(&mut l, format_args!($($a)*));
        let _ = core::fmt::Write::write_str(&mut l, "\n");
        console(grants(), &l.b[..l.n]);
    }};
}

#[unsafe(no_mangle)]
pub extern "C" fn vault_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    // Input bit 0, by hand over the bridge only (the kernel spawns with no input): start the region afresh (`FRESH`).
    unsafe { FRESH = input_len >= 1 && core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) & 1 == 1 };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = g };
    let stack = unsafe { &mut *STACK.0.get() };
    ferros_sys::thread_spawn(work, stack, 0);
    ferros_sys::thread_exit(0)
}

/// A value being written a piece at a time: its domain (by a hash of the address key: no key outlives its request), whether it is the person's, its name, the next offset expected, the writer.
struct Pending {
    domain: [u8; 32],
    person: bool,
    name: Vec<u8>,
    next: u64,
    w: Writer,
}

/// The most blocks one kernel call moves on a kernel from 2026-11-11 on (process.rs BLOCK_MAX: one UFS command, 1 MiB), on one from 2026-11-08 (32: 128 KiB, a 64 KiB piece's 17 blocks a call) and on one from before (16, each block its own command): each refusal of a larger run drops to the next smaller for good.
const RUN_MAX: usize = 1 << 8;
const RUN_MID: usize = 1 << 5;
static RUN_NOW: AtomicUsize = AtomicUsize::new(RUN_MAX);

/// The run after a refused one (`RUN_NOW`), or None when even the smallest was refused (the read fails).
fn run_down(m: usize) -> Option<usize> {
    if m > RUN_MID {
        Some(RUN_MID)
    } else if m > RUN {
        Some(RUN)
    } else {
        None
    }
}

/// Block reads, measured for the read report (`Meter`): the serving thread's time in them (`IO_TICKS`, with its waits for the next window read beside it), the reader thread's (`AHEAD_TICKS`, beside the fetch), every call's own time, blocks, calls, and the kernel's own split of the calls (SYS_BLOCK timed: ticks in the UFS commands, ticks copying into this process), and blocks the read-ahead gave without a call.
static IO_TICKS: AtomicU64 = AtomicU64::new(0);
static AHEAD_TICKS: AtomicU64 = AtomicU64::new(0);
static IO_CALL_TICKS: AtomicU64 = AtomicU64::new(0);
static IO_BLOCKS: AtomicU64 = AtomicU64::new(0);
static IO_CALLS: AtomicU64 = AtomicU64::new(0);
static IO_CMD: AtomicU64 = AtomicU64::new(0);
static IO_COPY: AtomicU64 = AtomicU64::new(0);
static AHEAD_HITS: AtomicU64 = AtomicU64::new(0);

/// `buf.len() / 4096` blocks of the window from `at` into `buf`, `RUN_NOW` blocks a call; the time it took is added to `spent` (the thread's own counter).
fn blocks_read(at: u64, buf: &mut [u8], spent: &AtomicU64) -> bool {
    let t = now();
    let n = buf.len() >> 12;
    let mut k = 0;
    while k < n {
        let m = (n - k).min(RUN_NOW.load(Relaxed));
        let tc = now();
        let (r, split) = ferros_sys::block_read_timed(cap(grants(), S_BLOCKS), at + k as u64, &mut buf[k << 12..(k + m) << 12]);
        IO_CALL_TICKS.fetch_add(now() - tc, Relaxed);
        IO_CALLS.fetch_add(1, Relaxed);
        if r != 0 {
            // A kernel from before refuses a larger run than its own: once, then the next smaller.
            match run_down(m) {
                Some(s) => {
                    RUN_NOW.store(s, Relaxed);
                    continue;
                }
                None => return false,
            }
        }
        if let Some((cmd, copy)) = split {
            IO_CMD.fetch_add(cmd, Relaxed);
            IO_COPY.fetch_add(copy, Relaxed);
        }
        k += m;
    }
    spent.fetch_add(now() - t, Relaxed);
    IO_BLOCKS.fetch_add(n as u64, Relaxed);
    true
}

/// `blocks_read`'s other half.
fn blocks_write(at: u64, buf: &[u8]) -> bool {
    let n = buf.len() >> 12;
    let mut k = 0;
    while k < n {
        let m = (n - k).min(RUN_NOW.load(Relaxed));
        if ferros_sys::block_write(cap(grants(), S_BLOCKS), at + k as u64, &buf[k << 12..(k + m) << 12]) != 0 {
            match run_down(m) {
                Some(s) => {
                    RUN_NOW.store(s, Relaxed);
                    continue;
                }
                None => return false,
            }
        }
        k += m;
    }
    true
}

/// The read-ahead (owner, 2026-11-11: "2.9s for a 24MB read is embarassing"): a large value's blocks lie in one stretch of the tract, each piece's furrows then its leaf, the next piece's after, so while one is read in bulk (`AHEAD_ON`, a get of a transfer or more) a furrow run the window does not hold fills it with AHEAD blocks from the run's first (one UFS command of 1 MiB: about fifteen pieces, leaves and all), and the leaves and runs after it are copies. It was two commands a piece: the leaf's block (when not in the index cache) and the 17-block run, 370 pieces' worth for a 24 MB value, each command's latency paid at queue depth one.
/// Each fill posts the window after it to the reader thread (`ahead_reader`), which reads it into the second buffer while this thread hashes and decodes the blocks of the first; a run in the second makes it the window (the buffers swap) and posts the one after. Anything else (a run neither holds: another value, a piece out of order) fills afresh from the run.
/// Nothing checks less: every block still meets its BLAKE3 guard in manifestus and every piece its seal. The windows hold only what the disk holds: the serving thread alone writes blocks, and every write first waits out a read beside it and drops both (`ahead_drop`); manifestus reads blocks it has staged and not yet written from its own staging, never from this device.
const AHEAD: usize = RUN_MAX;
/// Off since 2026-11-17: ferros_store gathers a get's furrows and reads exactly their runs, consecutive positions across pieces in one command of up to 512 KiB, where the windows read 197 MB of blocks for Photon's 92 MB capsule (half of what they read was used, the rest read again run by run).
/// Kept, switched off, for one measurement of both on the device; it goes once that is made.
const READ_AHEAD: bool = false;
/// The window: its first block (of the vault's window, both mirrors' numbering), how many it holds; the bytes. The serving thread's alone.
static mut AHEAD_AT: (u64, usize) = (0, 0);
static mut AHEAD_BUF: Vec<u8> = Vec::new();
/// The next window, read beside: its first block and length, its bytes (the reader thread's from the post until its done word says so), and whether one is posted and not yet taken.
static mut NEXT_AT: (u64, usize) = (0, 0);
static mut NEXT_BUF: Vec<u8> = Vec::new();
static mut NEXT_POSTED: bool = false;
/// The reader thread's job (a sequence number; its first block, length and buffer in `NEXT_JOB`) and its done word (the number shifted up one, whether every block came in bit 0); whether it started.
static NEXT_REQ: AtomicU64 = AtomicU64::new(0);
static NEXT_DONE: AtomicU64 = AtomicU64::new(0);
static mut NEXT_JOB: (u64, usize, usize) = (0, 0, 0);
static READER_UP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static READER_STACK: Region<{ 1 << 16 }> = Region(UnsafeCell::new([0; 1 << 16]));
/// A large value is being read in bulk: furrow runs may fill the window. Set around a bulk get (`serve`).
static mut AHEAD_ON: bool = false;
/// The value being read: blocks its windows read and blocks they gave, and whether they gave too little to go on (`ahead_run`); cleared by a get from offset 0 (`serve`), the start of a value.
/// A value written in pieces among other writes lies scattered over the tract, not in one stretch: Mahere's map cells, written 4 KiB a call.
/// Measured on the 8a (2026-11-12, a 1.6 MB value in three gets): the windows read 21 MB for 1.9 MB of hits, the read 74 to 83 ms; read-ahead off, 2.1 MB in 112 calls, 26 ms; with this rule, 34 to 35 ms.
static mut AHEAD_READ: u64 = 0;
static mut AHEAD_GOT: u64 = 0;
static mut AHEAD_POOR: bool = false;

/// `buf.len() / 4096` blocks from `at` out of the window, if it holds them all.
fn ahead_copy(at: u64, buf: &mut [u8]) -> bool {
    // SAFETY: the window is the serving thread's alone (the reader thread writes only the next one).
    let ((first, held), win) = unsafe { (AHEAD_AT, &*(&raw const AHEAD_BUF)) };
    let n = (buf.len() >> 12) as u64;
    // Whether the window holds them all (the copy below reads only inside what it holds).
    if at < first || at + n > first + held as u64 {
        return false;
    }
    let from = ((at - first) << 12) as usize;
    buf.copy_from_slice(&win[from..from + buf.len()]);
    AHEAD_HITS.fetch_add(n, Relaxed);
    unsafe { AHEAD_GOT += n };
    true
}

/// A furrow run of a bulk read (`buf.len() / 4096` blocks from `at`, no more than AHEAD) the window does not hold, from blocks below `limit` (the mirror's end): out of the next window once its read is done, else from a fresh fill; then the window after posted. Whether it came.
fn ahead_run(at: u64, buf: &mut [u8], limit: u64) -> bool {
    let n = (buf.len() >> 12) as u64;
    let (next, posted) = unsafe { (NEXT_AT, NEXT_POSTED) };
    if posted && at >= next.0 && at + n <= next.0 + next.1 as u64 {
        let ok = ahead_settle();
        if ok {
            // SAFETY: the reader thread is done with the next buffer (`ahead_settle`), and both buffers are otherwise the serving thread's.
            unsafe {
                core::mem::swap(&mut *(&raw mut AHEAD_BUF), &mut *(&raw mut NEXT_BUF));
                AHEAD_AT = next;
            }
            ahead_post_after(next.0, next.1, limit);
            return ahead_copy(at, buf);
        }
    }
    // No fresh fill once two windows' worth gave under half of what they read: the value does not lie in a stretch, and its runs are read as they are asked for (the caller's `blocks_read`).
    // SAFETY: the counters are the serving thread's alone.
    unsafe {
        if AHEAD_POOR || (AHEAD_READ >= (AHEAD << 1) as u64 && AHEAD_GOT << 1 < AHEAD_READ) {
            AHEAD_POOR = true;
            return false;
        }
    }
    // A fresh fill: the next buffer's read (if one runs) is let finish first, as it is reused for the one after.
    ahead_settle();
    // WHY the min: the fill is AHEAD blocks unless this mirror ends sooner, and past its end lie the other mirror's blocks (or the window's end).
    let m = (limit - at).min(AHEAD as u64) as usize;
    // SAFETY: as `ahead_copy`.
    let win = unsafe { &mut *(&raw mut AHEAD_BUF) };
    if win.len() < AHEAD << 12 {
        win.resize(AHEAD << 12, 0);
    }
    let ok = blocks_read(at, &mut win[..m << 12], &IO_TICKS);
    unsafe { AHEAD_AT = (at, if ok { m } else { 0 }) };
    unsafe { AHEAD_READ += m as u64 };
    if ok {
        ahead_post_after(at, m, limit);
    }
    ok && ahead_copy(at, buf)
}

/// How far the next window reaches back into the one before it: past a piece's 17 furrows and its leaf. Without it a piece whose blocks crossed a window's end (most windows' last piece: 256 blocks are not a whole number of 18-block pieces) lay in neither, so the serving thread waited out the read beside, threw it away and filled afresh from that piece, a 1 MiB command on its own time at nearly every window (2026-11-11).
const OVERLAP: u64 = 1 << 5;

/// The window after one of `held` blocks from `first`, posted: it begins OVERLAP blocks before that one ends. Nothing after a window no longer than the overlap: it is the mirror's last few blocks.
fn ahead_post_after(first: u64, held: usize, limit: u64) {
    if held as u64 > OVERLAP {
        ahead_post(first + held as u64 - OVERLAP, limit);
    }
}

/// The window from `at` (below `limit`) posted to the reader thread; nothing when it is not up or the mirror ends.
fn ahead_post(at: u64, limit: u64) {
    if !READER_UP.load(Relaxed) || at >= limit {
        return;
    }
    // WHY the min: as in `ahead_run`.
    let m = (limit - at).min(AHEAD as u64) as usize;
    // SAFETY: no read is posted (`ahead_settle` ran before every post), so the next buffer is the serving thread's.
    let next = unsafe { &mut *(&raw mut NEXT_BUF) };
    if next.len() < AHEAD << 12 {
        next.resize(AHEAD << 12, 0);
    }
    let seq = NEXT_REQ.load(Relaxed) + 1;
    unsafe {
        NEXT_JOB = (at, m, next.as_mut_ptr() as usize);
        NEXT_AT = (at, m);
        NEXT_POSTED = true;
        AHEAD_READ += m as u64;
    }
    NEXT_REQ.store(seq, Release);
    ferros_sys::wake(&NEXT_REQ, 1);
}

/// The posted read, if any, waited out and taken (no longer posted): whether every block came. The wait counts as the serving thread's block reading.
fn ahead_settle() -> bool {
    if !unsafe { NEXT_POSTED } {
        return false;
    }
    unsafe { NEXT_POSTED = false };
    let (t, seq) = (now(), NEXT_REQ.load(Relaxed));
    let ok = loop {
        let d = NEXT_DONE.load(Acquire);
        if d >> 1 == seq {
            break d & 1 != 0;
        }
        ferros_sys::wait(&NEXT_DONE, d, 0);
    };
    IO_TICKS.fetch_add(now() - t, Relaxed);
    ok
}

/// The reader thread: each posted window read into the next buffer, then said done.
extern "C" fn ahead_reader(_: u64) -> ! {
    let mut seen = 0;
    loop {
        let r = NEXT_REQ.load(Acquire);
        if r == seen {
            ferros_sys::wait(&NEXT_REQ, r, 0);
            continue;
        }
        seen = r;
        let (at, m, buf) = unsafe { NEXT_JOB };
        // SAFETY: the serving thread posted `m` blocks of its next buffer and touches it no more until this job's done word says so (`ahead_settle`).
        let ok = blocks_read(at, unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, m << 12) }, &AHEAD_TICKS);
        NEXT_DONE.store(r << 1 | ok as u64, Release);
        ferros_sys::wake(&NEXT_DONE, 1);
    }
}

/// A write is coming: a read beside is let finish, and neither window may still be what the disk holds.
fn ahead_drop() {
    ahead_settle();
    unsafe { AHEAD_AT = (0, 0) };
}

/// The helpers that open a read's pieces beside the serving thread (owner, 2026-11-08: "24MB from the vault should be imperceptibly fast"): a 1 MiB get (ferros_sys::STORE_GET_MAX, since 2026-11-11) is sixteen 64 KiB pieces, claimed one at a time by whichever core is free, so the X3 the serving thread runs on takes its larger share; a 256 KiB one was four, one a core, each batch waiting on its slowest worker. They hold nothing between requests: the sealing key is on the board only while a request's pieces open, and wiped after.
const HELPERS: usize = 3;
const HELPER_STACK: usize = 1 << 16;
static HELPER_STACKS: [Region<HELPER_STACK>; HELPERS] = [const { Region(UnsafeCell::new([0; HELPER_STACK])) }; HELPERS];

/// The board: the batch (bits 32-63, moved for each), its piece count (bits 16-31) and the next piece to claim (bits 0-15). A claim is a compare-and-swap within one batch, so a helper late to a batch claims nothing of the next: a claim it wins is of a batch whose server still waits, whose pieces and key are still on the board.
static BOARD_NEXT: AtomicU64 = AtomicU64::new(0);
/// Pieces of the batch opened.
static BOARD_DONE: AtomicU64 = AtomicU64::new(0);
/// The batch's pieces (their address) and the sealing key.
static mut BOARD: (usize, [u8; 32]) = (0, [0; 32]);
/// The batch's piece count, all of which its server will hand over (`Opener::ready`); bits 16-31 of BOARD_NEXT count those handed over so far.
static BOARD_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Helpers waiting on BOARD_NEXT: a piece handed over wakes one only when one waits (each wake is a syscall under the kernel lock).
static BOARD_WAITING: AtomicU64 = AtomicU64::new(0);

/// Claim and open the board's pieces until none handed over is left to claim.
fn work_board() {
    loop {
        let c = BOARD_NEXT.load(Acquire);
        let (n, next) = ((c >> 16) & 0xFFFF, c & 0xFFFF);
        if next >= n {
            return;
        }
        if BOARD_NEXT.compare_exchange(c, c + 1, AcqRel, Relaxed).is_err() {
            continue;
        }
        let (at, key) = unsafe { (BOARD.0, &*(&raw const BOARD.1)) };
        // SAFETY: piece `next` < n was handed over in the batch this claim is of (the claim compares the batch too): its server waits for every piece of the batch (BOARD_DONE) before the slots or the key leave the board, it no longer touches a slot once handed over, and no other thread claims the same index.
        ferros_store::open_piece(key, unsafe { &mut *(at as *mut Sealed).add(next as usize) });
        if BOARD_DONE.fetch_add(1, AcqRel) + 1 == BOARD_TOTAL.load(Acquire) {
            ferros_sys::wake(&BOARD_DONE, 1);
        }
    }
}

extern "C" fn helper(_: u64) -> ! {
    loop {
        let c = BOARD_NEXT.load(Acquire);
        work_board();
        // Counted as waiting before the word is looked at again (sequentially consistent, against the server's hand-over then its look at the count): either the server sees this helper waiting and wakes it, or this helper sees the piece and does not sleep.
        BOARD_WAITING.fetch_add(1, SeqCst);
        if BOARD_NEXT.load(SeqCst) == c {
            ferros_sys::wait(&BOARD_NEXT, c, 0);
        }
        BOARD_WAITING.fetch_sub(1, SeqCst);
    }
}

/// The serving thread's opener (streamed, `ferros_store::Opener`): each piece goes on the board as it is fetched, and the helpers open it while the serving thread fetches the next (the fetch, with every block's BLAKE3 guard, is the engine's and runs on this thread alone; before 2026-11-11 the helpers sat idle thru it and the fetch thru the opening); at the end this thread opens what is still unclaimed, waits for the rest and wipes the key. A batch of one piece is opened here at `finish`: the board's hand-offs would cost more than the piece.
#[derive(Default)]
struct Helpers {
    /// The batch is one piece, kept off the board.
    solo: bool,
    /// When the batch began, for the opening's measure.
    t0: u64,
}

impl Opener for Helpers {
    fn open_all(&mut self, seal: &[u8; 32], pieces: &mut [Sealed]) {
        let n = pieces.len();
        let at = pieces.as_mut_ptr();
        // SAFETY: the slice stays borrowed here until `finish` returns, and nothing else touches it.
        unsafe { self.start(seal, at, n) };
        for i in 0..n {
            self.ready(i);
        }
        unsafe { self.finish(seal, at, n) };
    }
    unsafe fn start(&mut self, seal: &[u8; 32], pieces: *mut Sealed, n: usize) {
        self.t0 = now();
        self.solo = n == 1;
        if self.solo {
            return;
        }
        unsafe { BOARD = (pieces as usize, *seal) };
        BOARD_DONE.store(0, Release);
        BOARD_TOTAL.store(n as u64, Release);
        let batch = (BOARD_NEXT.load(Acquire) >> 32) + 1;
        BOARD_NEXT.store(batch << 32, SeqCst);
    }
    fn ready(&mut self, _i: usize) {
        if self.solo {
            return;
        }
        // Handed over in order: slot i is the (i + 1)-th.
        BOARD_NEXT.fetch_add(1 << 16, SeqCst);
        if BOARD_WAITING.load(SeqCst) != 0 {
            ferros_sys::wake(&BOARD_NEXT, 1);
        }
    }
    unsafe fn finish(&mut self, seal: &[u8; 32], pieces: *mut Sealed, n: usize) {
        let t = now();
        if self.solo {
            ferros_store::open_piece(seal, unsafe { &mut *pieces });
        } else {
            work_board();
            loop {
                let d = BOARD_DONE.load(Acquire);
                if d >= n as u64 {
                    break;
                }
                ferros_sys::wait(&BOARD_DONE, d, 0);
            }
            unsafe {
                (*(&raw mut BOARD.1)).fill(0);
                BOARD.0 = 0;
            }
        }
        let opened: u64 = (0..n).map(|i| unsafe { (*pieces.add(i)).bytes.len() as u64 }).sum();
        OPEN_TICKS.fetch_add(now() - t, Relaxed);
        OPEN_SPAN.fetch_add(now() - self.t0, Relaxed);
        OPEN_BYTES.fetch_add(opened, Relaxed);
    }
}

/// Whether the serving thread is on the X3 for a large value's bulk read (the get request).
static mut ON_BIG: bool = false;

/// Opening, measured for the read report: the wall time of each request's opening, and the sealed bytes opened.
static OPEN_TICKS: AtomicU64 = AtomicU64::new(0);
static OPEN_BYTES: AtomicU64 = AtomicU64::new(0);
/// Opening streamed beside the fetch (2026-11-11): `OPEN_TICKS` is the part after the fetch (the serving thread opening what is left and waiting for the helpers), this the whole span from the batch's first piece to its last opened.
static OPEN_SPAN: AtomicU64 = AtomicU64::new(0);

/// The counters a read report takes the difference of: block reads (their calls' time, blocks, calls; the kernel's own split of the calls, in the UFS commands and copying into this process), blocks the read-ahead gave, opening (time, sealed bytes), and the store's fetch and copy (`ferros_store::ReadTicks`).
#[derive(Clone, Copy)]
struct Counts {
    io: u64,
    ahead: u64,
    call: u64,
    blocks: u64,
    calls: u64,
    cmd: u64,
    kcopy: u64,
    hits: u64,
    open: u64,
    span: u64,
    opened: u64,
    fetch: u64,
    copy: u64,
    walk: u64,
}

impl Counts {
    const ZERO: Counts = Counts { io: 0, ahead: 0, call: 0, blocks: 0, calls: 0, cmd: 0, kcopy: 0, hits: 0, open: 0, span: 0, opened: 0, fetch: 0, copy: 0, walk: 0 };
    fn now(st: ferros_store::ReadTicks) -> Counts {
        let l = |a: &AtomicU64| a.load(Relaxed);
        Counts { io: l(&IO_TICKS), ahead: l(&AHEAD_TICKS), call: l(&IO_CALL_TICKS), blocks: l(&IO_BLOCKS), calls: l(&IO_CALLS), cmd: l(&IO_CMD), kcopy: l(&IO_COPY), hits: l(&AHEAD_HITS), open: l(&OPEN_TICKS), span: l(&OPEN_SPAN), opened: l(&OPEN_BYTES), fetch: st.fetch, copy: st.copy, walk: st.walk }
    }
    /// What happened between `self` and `then`, counter by counter (each only grows).
    fn since(&self, then: &Counts) -> Counts {
        Counts { io: self.io - then.io, ahead: self.ahead - then.ahead, call: self.call - then.call, blocks: self.blocks - then.blocks, calls: self.calls - then.calls, cmd: self.cmd - then.cmd, kcopy: self.kcopy - then.kcopy, hits: self.hits - then.hits, open: self.open - then.open, span: self.span - then.span, opened: self.opened - then.opened, fetch: self.fetch - then.fetch, copy: self.copy - then.copy, walk: self.walk - then.walk }
    }
}

/// A large value read in one stretch, measured stage by stage (owner, 2026-11-08: "Report MB/s per stage"; 2026-11-11: "2.9s for a 24MB read is embarassing", so one device run says where every millisecond goes). A run is the requests that follow on from each other in the value (each starting where the last ended); it ends at the value's end, or at a request that does not follow on (reported with the counts as they stood before it). A run of at least METER_MIN bytes says two console lines; shorter ones (a document's opening, a probe) say nothing. Until 2026-11-11 a report began only at offset 0 and ended at the first read to reach the value's end: the display's wallpaper load opens the document at offset 0, and vsf's walk of the plane's section reads its last bytes, so the 27 requests of the opening were reported (19 ms) and the plane's 48, starting at its data offset, never were.
/// - `vault: read …`: the run's bytes, where it started in the value, the requests, first to last; serving split into the fetch (manifestus's index walk to each piece, "walk", then its furrows copied out of the read-ahead unchecked, and the pieces handed to the helpers: `ReadTicks::fetch` less the block reads inside it), the block reads, the checks and opening left after them (each furrow's BLAKE3 guard and decode, the furrows joined, and XChaCha20-Poly1305, on this thread and the helpers, since 2026-11-11 with manifestus `locate` and `check_furrows`; the helpers open beside the fetch, and "all" is each batch from its first piece handed over to its last opened), the copy out (into the reply, the opened pieces wiped) and the rest (the head, the bookkeeping); and the time between requests (from a reply to the next request's start: the kernel's hand-back, the reader's own work, the stamp and hand-off of the next call, and the wait for a core this thread may run on; while it is pinned to the X3 that is the X3's turn after the render thread and the kernel's service pass).
///   The block reads are the serving thread's own and its waits for the next window; "beside it" is the reader thread's reading of the next windows while the serving thread fetched.
/// - `vault: read blocks …`: the block reads' bytes, kernel calls (both threads') and their time, the kernel's split of those calls (in the UFS commands, copying into this process, the rest of the syscalls: entry, the kernel lock's wait), and the blocks the read-ahead gave without a call.
struct Meter {
    on: bool,
    t0: u64,
    requests: u64,
    serving: u64,
    /// The run's bytes, where it began and where the next request must begin to follow on; the value's length.
    bytes: u64,
    from: u64,
    next: u64,
    whole: u64,
    /// Requests served while this thread was pinned to the X3.
    pinned: u64,
    at: Counts,
}

/// Runs at least this large get a read report (and only values at least this large are followed).
const METER_MIN: u64 = 1 << 20;

impl Meter {
    /// A read of `data` bytes from `offset` of a value `whole` bytes long, served in `ticks` from `t_start` (on the X3 if `big`), the counters as they stood before it (`before`) and the store's stage time `st` after it.
    fn note(&mut self, offset: u64, data: u64, whole: u64, ticks: u64, t_start: u64, big: bool, before: Counts, st: ferros_store::ReadTicks) {
        if self.on && (offset != self.next || whole != self.whole) {
            self.say(&before, t_start);
            self.on = false;
        }
        if !self.on {
            if whole < METER_MIN {
                return;
            }
            *self = Meter { on: true, t0: t_start, requests: 0, serving: 0, bytes: 0, from: offset, next: offset, whole, pinned: 0, at: before };
        }
        self.requests += 1;
        self.serving += ticks;
        self.bytes += data;
        self.next = offset + data;
        self.pinned += big as u64;
        if self.next >= whole {
            self.say(&Counts::now(st), now());
            self.on = false;
        }
    }

    /// The run's two lines, the counters at its end being `end` and its end `t_end`; nothing for a short run.
    fn say(&self, end: &Counts, t_end: u64) {
        if self.bytes < METER_MIN {
            return;
        }
        let c = end.since(&self.at);
        let f = freq();
        let us = |t: u64| t * 1_000_000 / f;
        let ms = |t: u64| us(t) / 1000;
        // WHY the max: a stage that took no time (every piece already open, no block read) would divide by zero, and the report is not worth the process.
        let rate = |bytes: u64, t: u64| bytes / us(t).max(1);
        let wall = t_end - self.t0;
        // Signed: block reads made by a migration in the same requests count in `io` too, outside the fetch.
        let fetch = ms(c.fetch) as i64 - ms(c.io) as i64;
        let rest = ms(self.serving) as i64 - ms(c.fetch) as i64 - ms(c.open) as i64 - ms(c.copy) as i64;
        say!(
            "vault: read {} B from {} of {} in {} requests ({} on the X3), {} ms first to last ({} MB/s): serving {} ms = fetch {} (walk {}) + block reads {} (beside it {}) + checks and opening after them {} (all {} ms, {} cores) + copy out {} + rest {}; between requests {} ms",
            self.bytes,
            self.from,
            self.whole,
            self.requests,
            self.pinned,
            ms(wall),
            rate(self.bytes, wall),
            ms(self.serving),
            fetch,
            ms(c.walk),
            ms(c.io),
            ms(c.ahead),
            ms(c.open),
            ms(c.span),
            HELPERS + 1,
            ms(c.copy),
            rest,
            ms(wall - self.serving)
        );
        // Signed: a kernel that does not time leaves the split at 0.
        let sys = ms(c.call) as i64 - ms(c.cmd) as i64 - ms(c.kcopy) as i64;
        say!(
            "vault: read blocks {} B in {} calls, {} ms in them ({} MB/s): in the UFS commands {} ms ({} MB/s), the kernel's copies {} ms, the rest of the calls {} ms; read-ahead gave {} blocks",
            c.blocks << 12,
            c.calls,
            ms(c.call),
            rate(c.blocks << 12, c.call),
            ms(c.cmd),
            rate(c.blocks << 12, c.cmd),
            ms(c.kcopy),
            sys,
            c.hits
        );
    }
}

static mut METER: Meter = Meter { on: false, t0: 0, requests: 0, serving: 0, bytes: 0, from: 0, next: 0, whole: 0, pinned: 0, at: Counts::ZERO };

type St = Store<Half, Half, Entropy>;

extern "C" fn work(_: u64) -> ! {
    let t0 = now();
    // Each mirror bounded by its half of the window (8 GiB), so a grown tract opens again; what is in use is the committed extent, claimed below. A blank region (block 0, the first root slot, all zeros) opens at MIRROR instead: a blank vault is read whole before its first commit, and over 8 GiB that would be four million reads. The next open finds it and takes the half.
    // Afresh: the first MIRROR blocks of each half zeroed, so the open below finds a blank region and formats it. Only by hand (input bit 0): it destroys whatever the region held (2026-11-08, the owner: "I do not care about literally any data on this device", after the wipe left orphaned blocks no spine reached).
    if unsafe { FRESH } {
        let zero = alloc::vec![0u8; RUN << 12];
        for base in [0, HALF] {
            let mut at = 0;
            while at < MIRROR {
                if ferros_sys::block_write(cap(grants(), S_BLOCKS), base + at, &zero) != 0 {
                    say!("vault: zeroing for a fresh start failed at {}", base + at);
                    ferros_sys::exit(2);
                }
                at += RUN as u64;
            }
        }
        say!("vault: the region zeroed for a fresh start ({} blocks a mirror)", MIRROR);
    }
    let mut probe = [0u8; 4096];
    let blank = ferros_sys::block_read(cap(grants(), S_BLOCKS), 0, &mut probe) == 0 && probe.iter().all(|&b| b == 0);
    let bound = if blank { MIRROR } else { HALF };
    say!("vault: block 0 blank {}, opening bounded at {} blocks a mirror", blank, bound);
    let mirror = Mirror::new(Half { base: 0, blocks: bound }, Half { base: HALF, blocks: bound });
    // Lazily (manifestus `open_lazy`): reads are served from the head's index at once, and the walk that finds what is live is stepped between requests (`settle_step`, below); every write finishes it first, so writes are as after an eager open. The eager open walked the whole index before serving anything: 10.7 s on the 8a with 462,713 live blocks (2026-11-12), and every reader waited behind it, the person's space at attest among them (the deck's kists list 4.9 s, the GPU's firmware, the wallpaper), which an attest in a boot's first seconds met as "four seconds to the deck".
    let vault = match Vault::open_lazy(mirror, RING_LOG2, eagle(), DeletePolicy::UnlinkOnly) {
        Ok(v) => v,
        Err(e) => {
            say!("vault: the region did not open: {:?}", e);
            ferros_sys::exit(2);
        }
    };
    let st = vault.engine_stats();
    say!("vault: tract {} blocks from {}, the window {} a mirror; generation {}, {} live blocks, plow {} reap {}{}", st.tract_blocks, vault.tract_base(), HALF, st.generation, st.live_blocks, st.plow, st.reap, if vault.settled() { "" } else { " (not settled: the walk that counts them follows, between requests)" });
    let mut store = Store::new(vault, Entropy { pool: [0; 4096], at: 4096 }, eagle);
    store.set_clock(now);
    say!("vault: open in {} ms, serving; the heap's high water {} KiB of {} KiB, {} KiB in use", (now() - t0) * 1000 / freq(), ferros_heap::PEAK.load(Relaxed) >> 10, HEAP_BYTES >> 10, ferros_heap::IN_USE.load(Relaxed) >> 10);
    // The pieces' helpers, idle until a read hands them a batch (`Helpers`).
    for s in HELPER_STACKS.iter() {
        if ferros_sys::thread_spawn(helper, unsafe { &mut *s.0.get() }, 0) == ERR {
            say!("vault: a helper did not start; reads open their pieces on fewer cores");
        }
    }
    // The read-ahead's reader (`ahead_reader`), idle until a bulk read posts the next window; without it each window is read when it is reached.
    if ferros_sys::thread_spawn(ahead_reader, unsafe { &mut *READER_STACK.0.get() }, 0) == ERR {
        say!("vault: the read-ahead's reader did not start; each window is read when reached");
    } else {
        READER_UP.store(true, Relaxed);
    }
    if ferros_sys::thread_spawn(recoverer, unsafe { &mut *RECOVER_STACK_MEM.0.get() }, 0) == ERR {
        say!("vault: the recoverer did not start; clips left open are closed inside the extent requests");
    } else {
        RECOVERER_UP.store(true, Relaxed);
    }
    // One transfer and its stamp each way, with room to spare: in, a set's piece (ferros_sys::STORE_MAX, 256 KiB); out, a get's (ferros_sys::STORE_GET_MAX, 1 MiB since 2026-11-11, the kernel's bound on a forwarded get).
    // In, a set's piece (ferros_sys::STORE_MAX) or an extent's write (EXTENT_MAX, a UFS command's 1 MiB), the larger.
    let mut msg = alloc::vec![0u8; ferros_sys::STORE_MAX.max(EXTENT_MAX) + (1 << 12)];
    let mut out = alloc::vec![0u8; ferros_sys::STORE_GET_MAX + (1 << 12)];
    // The kernel forwards gets larger than one STORE_MAX transfer only once this process says it answers them (a vault from before would index past its reply buffer); a kernel from before refuses the call and its gets stay at STORE_MAX.
    ferros_sys::vault_gets(cap(grants(), S_VAULT), ferros_sys::STORE_GET_MAX);
    let mut token = [0u8; 32];
    let mut writers: Vec<Pending> = Vec::new();
    let mut epoch: Option<u32> = None;
    // The person's export being read (op 5): built at offset 0, kept until read to its end or the person locks.
    let mut export: Option<Vec<u8>> = None;
    // The lazy open's walk (above): stepped while no request waits, a step at a time (WALK_STEP pointers, a few ms), so a request waits for at most one step. Done once settled; a step that fails stops the walk (the eager open's refusal, met later): reads go on from the index, and every write fails with the walk's error, said here once.
    let mut walking = !store.engine().settled();
    let t_walk = now();
    // Clips the last boot's writers left open (the power went, a reset): to the recoverer at once, so they are closed before anyone records again.
    recover_kick(&mut store);

    loop {
        let n = if walking {
            let n = ferros_sys::recv(cap(grants(), S_SERVE), &mut msg, &mut token, true);
            if n == NOTHING {
                match store.engine().settle_step() {
                    Ok(false) => {}
                    Ok(true) => {
                        walking = false;
                        let st = store.engine().engine_stats();
                        live_note(store.engine());
                        let (hashed, tree, bytes) = unsafe { LIVE };
                        say!("vault: settled {} ms after serving began: {} live blocks ({} hashed, {} by position, the live set {} KiB), plow {} reap {}; the heap {} KiB in use, high water {} KiB", (now() - t_walk) * 1000 / freq(), st.live_blocks, hashed, tree, bytes >> 10, st.plow, st.reap, ferros_heap::IN_USE.load(Relaxed) >> 10, ferros_heap::PEAK.load(Relaxed) >> 10);
                    }
                    Err(e) => {
                        walking = false;
                        say!("vault: the open's walk refused: {:?}; serving reads only, every write will fail", e);
                    }
                }
                continue;
            }
            n
        } else {
            ferros_sys::recv(cap(grants(), S_SERVE), &mut msg, &mut token, false)
        };
        if n == ERR {
            continue;
        }
        let m = &mut msg[..n as usize];
        let e = u32::from_le_bytes(m[2..6].try_into().unwrap());
        if epoch.is_some_and(|x| x != e) {
            // The person locked (or unlocked) since the last request: what this process holds of theirs goes.
            store.forget();
            writers.retain(|p| !p.person);
            if let Some(mut x) = export.take() {
                x.fill(0);
            }
        }
        epoch = Some(e);
        // A clip the recoverer finished goes into the table before any request is served (an atomic's look when none did).
        recover_commit(&mut store, false);
        // Old entries wait in the kernel's vault: they move before this request is served, so it never finds a name between the two vaults (gone from the kernel's, not yet here).
        if m[1] & 1 != 0 {
            migrate(&mut store);
        }
        let len = serve(&mut store, &mut writers, &mut export, m, &mut out, &token);
        live_note(store.engine());
        // The stamp's keys and the name wiped; an extent's bytes past them are the clip's own, not a secret of the vault's, and zeroing a MiB a write cost the clip's write path (2026-11-19).
        let wipe_to = if m[0] == OP_EXTENT { m.len().min(1 << 9) } else { m.len() };
        m[..wipe_to].fill(0);
        ferros_sys::reply(&token, &out[..len]);
    }
}

/// The domain at `at` of a stamp: (the person's?, the domain).
fn domain_at(m: &[u8], at: usize) -> (bool, Domain) {
    let mut d = Domain { addr: [0; 32], seal: [0; 32] };
    d.addr.copy_from_slice(&m[at + 1..at + 33]);
    d.seal.copy_from_slice(&m[at + 33..at + 65]);
    (m[at] != 0, d)
}

fn wipe(d: &mut Domain) {
    d.addr.fill(0);
    d.seal.fill(0);
    core::hint::black_box(d);
}

/// One stamped request into `out` (the reply: the syscall's value, then the caller's bytes); returns the reply's length.
fn serve(store: &mut St, writers: &mut Vec<Pending>, export: &mut Option<Vec<u8>>, m: &[u8], out: &mut [u8], token: &[u8; 32]) -> usize {
    const NOTHING: u64 = ferros_sys::NOTHING;
    let (op, count) = (m[0], m[6] as usize);
    let at_name = 7 + count * 65;
    let name = &m[at_name + 1..at_name + 1 + m[at_name] as usize];
    let rest = &m[at_name + 1 + name.len()..];
    let u64_at = |b: &[u8]| u64::from_le_bytes(b[..8].try_into().unwrap());
    // For the death report: a set's offset and bytes, a read's (get, list, export) offset and length; a delete or a drop has neither.
    unsafe {
        SERVING = match op {
            0 => (op, u64_at(rest), (rest.len() - 9) as u64, name.len()),
            1 | 3 | OP_EXPORT => (op, u64_at(rest), u32::from_le_bytes(rest[8..12].try_into().unwrap()) as u64, name.len()),
            _ => (op, 0, 0, name.len()),
        }
    };
    let mut data = 0;
    let ret = if op == 3 {
        // A listing over every domain stamped: names one per line, or with the address flag the 32-byte addresses.
        let (offset, len, raw) = (u64_at(rest) as usize, u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize, rest[12] != 0);
        let mut all = Vec::new();
        let mut ok = true;
        for i in 0..count {
            let (_, mut d) = domain_at(m, 7 + i * 65);
            match store.names(&d) {
                Ok(names) => {
                    for nm in names {
                        match (raw, nm.first()) {
                            (true, Some(1)) if nm.len() == 33 => all.extend_from_slice(&nm[1..]),
                            (false, Some(&c)) if c != 1 => {
                                all.extend_from_slice(&nm);
                                all.push(b'\n');
                            }
                            _ => {}
                        }
                    }
                }
                Err(_) => ok = false,
            }
            wipe(&mut d);
        }
        if ok {
            // WHY: the offset is the caller's and may lie past the end (nothing to send then).
            let from = offset.min(all.len());
            data = (all.len() - from).min(len);
            out[8..8 + data].copy_from_slice(&all[from..from + data]);
            all.len() as u64
        } else {
            ERR
        }
    } else if op == OP_EXTENT {
        let (_, mut d) = domain_at(m, 7);
        let r = extent(store, &d, name, rest, out, &mut data, token);
        wipe(&mut d);
        r
    } else if op == OP_DROP {
        // Release (the kernel's own call): each stamped domain dropped whole (ferros_store `drop_domain`), any write of it in progress with it. How many values went.
        let (mut n, mut ok) = (0u64, true);
        for i in 0..count {
            let (_, mut d) = domain_at(m, 7 + i * 65);
            let id = *blake3::hash(&d.addr).as_bytes();
            writers.retain(|p| p.domain != id);
            // Its extents go with it (manifestus `remove_owner`): a clip is the domain's as its values are, once the recoverer is out of any of them.
            recover_commit(store, true);
            if let Some(t) = extents(store) {
                let mut next = t.clone();
                if next.remove_owner(&id) > 0 {
                    match next.store(store.engine(), eagle()) {
                        Ok(()) => *t = next,
                        Err(_) => ok = false,
                    }
                }
            }
            match store.drop_domain(&d) {
                Ok(k) => n += k,
                Err(_) => ok = false,
            }
            wipe(&mut d);
        }
        say!("vault: dropped {} domains ({} values){}", count, n, if ok { "" } else { ", some failed" });
        if ok { n } else { ERR }
    } else if op == OP_SIZE {
        // A kist's own domain sized for its unstow sheet: its values and their bytes, over the domains stamped (one).
        let (mut values, mut bytes, mut ok) = (0u64, 0u64, true);
        for i in 0..count {
            let (_, mut d) = domain_at(m, 7 + i * 65);
            match store.size(&d) {
                Ok((v, b)) => {
                    values += v;
                    bytes += b;
                }
                Err(_) => ok = false,
            }
            wipe(&mut d);
        }
        if ok {
            out[8..16].copy_from_slice(&values.to_le_bytes());
            data = 8;
            bytes
        } else {
            ERR
        }
    } else if op == OP_EXPORT {
        // The person's export (a process's SYS_UNLOCK op 5, forwarded): built at offset 0 from the stamped domains, each with its scope's prefix after the offset and length; handed out a transfer at a time; wiped once read to its end.
        let (offset, len) = (u64_at(rest) as usize, u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize);
        if offset == 0 {
            if let Some(mut x) = export.take() {
                x.fill(0);
            }
            let mut doms = Vec::with_capacity(count);
            let mut at = 12;
            for i in 0..count {
                let p = &rest[at + 1..at + 1 + rest[at] as usize];
                at += 1 + p.len();
                doms.push((p, domain_at(m, 7 + i * 65).1));
            }
            *export = export_document(store, &doms);
            for (_, d) in doms.iter_mut() {
                wipe(d);
            }
        }
        match export.as_mut() {
            Some(doc) => {
                // WHY: the offset is the caller's and may lie past the end (nothing to send then).
                let from = offset.min(doc.len());
                data = (doc.len() - from).min(len);
                out[8..8 + data].copy_from_slice(&doc[from..from + data]);
                let whole = doc.len() as u64;
                if from + data == doc.len() {
                    doc.fill(0);
                    *export = None;
                }
                whole
            }
            None => ERR,
        }
    } else {
        let (person, mut d) = domain_at(m, 7);
        let id = *blake3::hash(&d.addr).as_bytes();
        let r = match op {
            0 => {
                let (offset, more, bytes) = (u64_at(rest), rest[8] != 0, &rest[9..]);
                let i = match writers.iter().position(|p| p.domain == id && p.name == name) {
                    Some(i) if offset == writers[i].next => Some(i),
                    // A value starts at offset 0; one restarted from 0 starts afresh (its earlier pieces are left for the sweep).
                    Some(i) if offset == 0 => {
                        writers.swap_remove(i);
                        None
                    }
                    None if offset == 0 => None,
                    _ => {
                        wipe(&mut d);
                        return reply(out, ERR, 0);
                    }
                };
                let i = i.unwrap_or_else(|| {
                    writers.push(Pending { domain: id, person, name: name.to_vec(), next: 0, w: store.begin(&d, name) });
                    writers.len() - 1
                });
                // A refused write or finish says why on the console (2026-11-08: every write failed with "vault error" and no word of the cause).
                let written = store.write(&d, &mut writers[i].w, bytes);
                writers[i].next += bytes.len() as u64;
                if let Err(e) = written {
                    say!("vault: write refused at {}: {:?}", offset, e);
                    writers.swap_remove(i);
                    ERR
                } else if !more {
                    let p = writers.swap_remove(i);
                    let whole = p.next;
                    match store.finish(&d, p.w) {
                        Ok(()) => {
                            // A large value's end says what the live set and the heap hold after it (2026-11-13: the live set's growth ended the process twice on the 8a).
                            if whole >= METER_MIN {
                                let (hashed, tree) = store.engine().live_counts();
                                say!("vault: wrote {} B; the live set {} hashed, {} by position, {} KiB; the heap {} KiB in use, high water {} of {} KiB", whole, hashed, tree, store.engine().live_bytes() >> 10, ferros_heap::IN_USE.load(Relaxed) >> 10, ferros_heap::PEAK.load(Relaxed) >> 10, HEAP_BYTES >> 10);
                            }
                            0
                        }
                        Err(e) => {
                            say!("vault: finish refused: {:?}", e);
                            ERR
                        }
                    }
                } else {
                    0
                }
            }
            1 => {
                let (offset, len) = (u64_at(rest), u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize);
                let t = now();
                // A large value read in bulk transfers is served on the X3 (owner, 2026-11-09: "why the wallpaper still takes about 1s to show"): the index walk, the blocks' guards and the copies ran three times as fast there as on a worker, and a 24 MB value's read went from 1256 to 637 ms of serving. Pinned at its first bulk transfer and back on the workers when it reaches its end or a request that is not one comes (a move a transfer cost most of the gain: 1026 ms), so the render thread's core is only lent while someone reads a large value.
                let bulk = len >= 1 << 16;
                if bulk && unsafe { !ON_BIG } {
                    ferros_sys::pin(ferros_sys::CORES_BIG);
                    unsafe { ON_BIG = true };
                }
                let before = Counts::now(store.read_ticks());
                // A value's start: its windows' yield counted afresh (`AHEAD_POOR`).
                if offset == 0 {
                    unsafe { (AHEAD_READ, AHEAD_GOT, AHEAD_POOR) = (0, 0, false) };
                }
                unsafe { AHEAD_ON = bulk && READ_AHEAD };
                let r = store.read_with(&d, name, offset, &mut out[8..8 + len], &mut Helpers::default());
                unsafe { AHEAD_ON = false };
                let big = unsafe { ON_BIG };
                let done = !matches!(r, Ok(Some(whole)) if bulk && offset + (len as u64) < whole);
                if done && unsafe { ON_BIG } {
                    ferros_sys::pin(0);
                    unsafe { ON_BIG = false };
                }
                match r {
                    Ok(Some(whole)) => {
                        // WHY: the offset is the caller's and may lie past the end (nothing to send then: the subtraction stops at 0).
                        data = (whole.saturating_sub(offset) as usize).min(len);
                        unsafe { (*(&raw mut METER)).note(offset, data as u64, whole, now() - t, t, big, before, store.read_ticks()) };
                        whole
                    }
                    Ok(None) => NOTHING,
                    Err(e) => {
                        say!("vault: read failed: {:?}", e);
                        ERR
                    }
                }
            }
            2 => {
                writers.retain(|p| !(p.domain == id && p.name == name));
                match store.delete(&d, name) {
                    Ok(true) => 0,
                    Ok(false) => NOTHING,
                    Err(_) => ERR,
                }
            }
            _ => ERR,
        };
        wipe(&mut d);
        r
    };
    reply(out, ret, data)
}

/// The extents' table (manifestus `extent`), read from the vault at its first use and kept: this process alone changes it, and every change is committed before its reply.
/// None when it does not read (a table over another area): every extent op is refused, said once.
static mut EXTENTS: Option<manifestus::Extents> = None;
static mut EXTENTS_READ: bool = false;
/// The extent writes since the last seal: (calls, bytes, ticks in the block calls, the first one's tick).
static mut EXT_METER: (u64, u64, u64, u64) = (0, 0, 0, 0);

fn extents(store: &mut St) -> Option<&'static mut manifestus::Extents> {
    // SAFETY: the serving thread's alone, as SERVING is.
    unsafe {
        if !EXTENTS_READ {
            EXTENTS_READ = true;
            match manifestus::Extents::load(store.engine(), CLIPS_AREA) {
                Ok(t) => {
                    say!("vault: extents: {} in the clips area, {} of {} blocks taken", t.list().len(), t.taken(), CLIPS_AREA);
                    EXTENTS = Some(t);
                }
                Err(e) => say!("vault: extents: the table did not read ({:?}); every extent op is refused", e),
            }
        }
        (*(&raw mut EXTENTS)).as_mut()
    }
}

/// An extent's name in the table: the domain's address key and the caller's name, never stored.
fn extent_name(d: &Domain, name: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("ferros vault 2026-11-19 extent name");
    h.update(&d.addr);
    h.update(name);
    *h.finalize().as_bytes()
}

/// The kernel's SYS_BLOCK x1 bit 3 on the clips window (process.rs `extent_give`): the extent `first`, `blocks` given to the caller the reply token names, its capability back (None: refused, or a kernel from before); with `take_back` that caller's extents taken back.
fn extent_give(token: &[u8; 32], first: u64, blocks: u64, take_back: bool) -> Option<[u8; 32]> {
    let mut io = [0u8; 64];
    io[..32].copy_from_slice(token);
    let r = ferros_sys::svc(ferros_sys::nr::BLOCK, cap(grants(), S_CLIPS), 8 | (take_back as u64) << 4, first, blocks, ferros_sys::off(io.as_ptr()));
    io[..32].fill(0);
    (r == 0 && !take_back).then(|| io[32..].try_into().unwrap())
}

/// A clip's extent as a VSF byte source (vsf `container::ByteSource`): its blocks of the clips window, each miss reading only the blocks the request covers (a UFS command's run at most) and keeping them, since the clip reader asks for small pieces next to each other (a frame's header, its closing bytes) and for a checked frame's samples whole.
/// A miss read a whole run before (2026-11-19): a frame's header and its closing bytes cost two MiB, and a recovery's walk read the clip twice over.
struct ExtentBytes {
    first: u64,
    blocks: u64,
    /// The blocks kept: the first (u64::MAX: none) and how many.
    at: core::cell::Cell<u64>,
    n: core::cell::Cell<u64>,
    buf: core::cell::RefCell<Vec<u8>>,
    /// The block reads: calls, bytes, ticks in them.
    meter: core::cell::Cell<(u64, u64, u64)>,
}

impl ExtentBytes {
    fn new(first: u64, blocks: u64) -> ExtentBytes {
        ExtentBytes { first, blocks, at: core::cell::Cell::new(u64::MAX), n: core::cell::Cell::new(0), buf: core::cell::RefCell::new(alloc::vec![0u8; RUN_MAX << 12]), meter: core::cell::Cell::new((0, 0, 0)) }
    }
}

impl vsf::container::ByteSource for ExtentBytes {
    fn len(&self) -> u64 {
        self.blocks << 12
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), vsf::container::SourceError> {
        // WHY: the offset and length are the clip reader's, from the clip's own bytes; past the extent is outside the source.
        if offset.checked_add(out.len() as u64).is_none_or(|e| e > self.len()) {
            return Err(vsf::container::SourceError::OutOfRange { offset, len: out.len() as u64, size: self.len() });
        }
        let end = offset + out.len() as u64;
        let mut done = 0usize;
        while done < out.len() {
            let at = offset + done as u64;
            let blk = at >> 12;
            let mut buf = self.buf.borrow_mut();
            if self.at.get() == u64::MAX || blk < self.at.get() || blk >= self.at.get() + self.n.get() {
                // The blocks the rest of the request covers, a run at most; at least one, since `blk` is inside the extent (the range check above).
                let n = (end.div_ceil(4096) - blk).min(RUN_MAX as u64).min(self.blocks - blk);
                let t0 = ferros_sys::now();
                if ferros_sys::block_read(cap(grants(), S_CLIPS), self.first + blk, &mut buf[..(n as usize) << 12]) != 0 {
                    self.at.set(u64::MAX);
                    return Err(vsf::container::SourceError::Io("the clips window refused a read".into()));
                }
                let (c, b, t) = self.meter.get();
                self.meter.set((c + 1, b + (n << 12), t + ferros_sys::now() - t0));
                self.at.set(blk);
                self.n.set(n);
            }
            let from = (at - (self.at.get() << 12)) as usize;
            let k = (out.len() - done).min(((self.n.get() as usize) << 12) - from);
            out[done..done + k].copy_from_slice(&buf[from..from + k]);
            done += k;
        }
        Ok(())
    }
}

/// Clips left open by a writer that is gone (it died, the power went) are closed by the recoverer, a thread of this process on an A510, never inside a request: each clip read to its last whole frame, its closing generation written right after that frame, the extent sealed at that length and its tail given back.
/// Never dropped: they are the person's footage. An extent whose clip does not read (its writer died before its first bytes) is sealed at no bytes, its first block kept.
/// Until 2026-11-19 this ran inside the next OPEN on the serving thread, hashing every frame's samples: a Lumis clip killed at 2,525 frames (18 GB) held that OPEN and every other vault request for 89 s, the camera's thread waiting in it until its driver closed, the Pro at 89 C.
/// Now the walk reads the frames' headers alone (vsf `Reader::open_headers`) and the samples of the last `RECOVER_CHECKED` frames are checked (`check_tail`): a recorder writes its frames in order, so a torn write tears the tail.
const RECOVER_CHECKED: usize = 1 << 3;
const RECOVER_STACK: usize = 1 << 17;
static RECOVER_STACK_MEM: Region<RECOVER_STACK> = Region(UnsafeCell::new([0; RECOVER_STACK]));
/// The jobs posted and finished (a sequence each; equal: the recoverer idle).
static RECOVER_REQ: AtomicU64 = AtomicU64::new(0);
static RECOVER_DONE: AtomicU64 = AtomicU64::new(0);
/// The job (its extent's first block and blocks), written by the serving thread before RECOVER_REQ moves, read by the recoverer after; its answer, written by the recoverer before RECOVER_DONE moves.
static mut RECOVER_JOB: (u64, u64) = (0, 0);
static mut RECOVER_OUT: Option<Recovered> = None;
/// The extent being recovered (serving thread only): its name, first block and blocks, matched again at the commit.
static mut RECOVERING: Option<([u8; 32], u64, u64)> = None;
static RECOVERER_UP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Extents opened without their blocks given to the caller (a kernel that refused `extent_give`): written thru this process, held by no writer the kernel knows of, so never taken for left open.
static mut THROUGH: Vec<[u8; 32]> = Vec::new();

/// What a recovery found and what it cost.
struct Recovered {
    len: u64,
    hash: [u8; 32],
    frames: usize,
    /// Frames whose samples were checked.
    checked: usize,
    /// Ticks: the headers' walk, the tail's check, the closing generation's write.
    walk: u64,
    check: u64,
    write: u64,
    /// The block reads: calls, bytes, ticks.
    reads: (u64, u64, u64),
}

extern "C" fn recoverer(_: u64) -> ! {
    // An A510: the recovery goes at the vault's own pace, never at the big cores' heat (the old one ran on the X3 for 89 s).
    ferros_sys::pin(ferros_sys::CORES_LITTLE);
    let mut seen = 0;
    loop {
        let r = RECOVER_REQ.load(Acquire);
        if r == seen {
            ferros_sys::wait(&RECOVER_REQ, r, 0);
            continue;
        }
        seen = r;
        let (first, blocks) = unsafe { RECOVER_JOB };
        let out = recover_clip(first, blocks);
        // SAFETY: the serving thread reads RECOVER_OUT only once RECOVER_DONE says this job is done.
        unsafe { *(&raw mut RECOVER_OUT) = Some(out) };
        RECOVER_DONE.store(r, Release);
        ferros_sys::wake(&RECOVER_DONE, 1);
    }
}

/// Hand the recoverer the next open extent no live writer holds (the kernel says, SYS_BLOCK x1 bits 3 and 5), if it is idle. From the serving thread: at the start, at every extent request and after each commit.
fn recover_kick(store: &mut St) {
    if unsafe { (*(&raw const RECOVERING)).is_some() } {
        return;
    }
    let Some(t) = extents(store) else { return };
    let through = unsafe { &*(&raw const THROUGH) };
    let next = t.list().iter().filter(|e| !e.sealed && !through.contains(&e.name)).find(|e| ferros_sys::svc(ferros_sys::nr::BLOCK, cap(grants(), S_CLIPS), 8 | 32, e.first, 0, 0) == 0).copied();
    let Some(e) = next else { return };
    if !RECOVERER_UP.load(Relaxed) {
        // No thread (it did not start): here, as before, rather than never.
        let out = recover_clip(e.first, e.blocks);
        recover_seal(store, e.name, e.first, e.blocks, out);
        return;
    }
    unsafe {
        RECOVER_JOB = (e.first, e.blocks);
        RECOVERING = Some((e.name, e.first, e.blocks));
    }
    say!("vault: extent at {}: its writer is gone; its clip is closed beside the requests, on an A510", e.first);
    RECOVER_REQ.fetch_add(1, Release);
    ferros_sys::wake(&RECOVER_REQ, 1);
}

/// A finished recovery sealed into the table (every request looks first); `wait` waits for one in flight (an extent about to be removed or replaced: its blocks are not given while the recoverer may still write in them).
fn recover_commit(store: &mut St, wait: bool) {
    let Some((name, first, blocks)) = (unsafe { *(&raw const RECOVERING) }) else { return };
    loop {
        let d = RECOVER_DONE.load(Acquire);
        if d == RECOVER_REQ.load(Acquire) {
            break;
        }
        if !wait {
            return;
        }
        ferros_sys::wait(&RECOVER_DONE, d, 0);
    }
    unsafe { RECOVERING = None };
    let Some(out) = (unsafe { (*(&raw mut RECOVER_OUT)).take() }) else { return };
    recover_seal(store, name, first, blocks, out);
    recover_kick(store);
}

fn recover_seal(store: &mut St, name: [u8; 32], first: u64, blocks: u64, out: Recovered) {
    let Some(t) = extents(store) else { return };
    // The extent may have gone meanwhile (its domain dropped): nothing to seal.
    if !t.list().iter().any(|e| e.name == name && e.first == first && !e.sealed) {
        return;
    }
    let mut next = t.clone();
    let ms = |k: u64| k * 1000 / freq();
    match next.seal(&name, out.len, out.hash, eagle()).and_then(|_| next.store(store.engine(), eagle())) {
        Ok(()) => {
            *t = next;
            say!(
                "vault: extent at {} recovered: its writer was gone; {} frames, sealed at {} bytes, {} blocks given back; the headers' walk {} ms, the last {} frames' samples checked in {} ms, the closing written in {} ms; block reads {} of {} MiB in {} ms",
                first,
                out.frames,
                out.len,
                blocks - out.len.div_ceil(4096).max(1),
                ms(out.walk),
                out.checked,
                ms(out.check),
                ms(out.write),
                out.reads.0,
                out.reads.1 >> 20,
                ms(out.reads.2)
            );
        }
        Err(err) => say!("vault: extent at {} could not be sealed after its recovery: {:?}", first, err),
    }
}

/// The clip in an open extent closed at its last whole frame; length 0 when it does not read as a clip. On the recoverer.
fn recover_clip(first: u64, blocks: u64) -> Recovered {
    let mut o = Recovered { len: 0, hash: [0; 32], frames: 0, checked: 0, walk: 0, check: 0, write: 0, reads: (0, 0, 0) };
    let t0 = ferros_sys::now();
    let mut r = match vsf::visual::clip::Reader::open_headers(ExtentBytes::new(first, blocks)) {
        Ok(r) => r,
        Err(err) => {
            say!("vault: extent at {}: no clip in it ({}); sealed empty", first, err);
            return o;
        }
    };
    let t1 = ferros_sys::now();
    o.walk = t1 - t0;
    if r.recovered() {
        o.checked = r.check_tail(RECOVER_CHECKED);
    }
    let t2 = ferros_sys::now();
    o.check = t2 - t1;
    o.reads = r.source().meter.get();
    let end = r.end();
    (o.len, o.frames) = (end, r.frames().len());
    let closing = match r.repair("ferros vault") {
        Ok(c) => c,
        Err(err) => {
            say!("vault: extent at {}: its closing generation was not made ({}); sealed at its frames' end", first, err);
            return o;
        }
    };
    // The closing generation from `end`: the block holding `end` read first, its bytes before `end` kept.
    let b0 = end >> 12;
    let lead = (end & 4095) as usize;
    let total = (lead + closing.len()).div_ceil(4096);
    if b0 + total as u64 > blocks {
        say!("vault: extent at {}: no room for its closing generation; sealed at its frames' end", first);
        return o;
    }
    let mut out = alloc::vec![0u8; total << 12];
    if lead != 0 && ferros_sys::block_read(cap(grants(), S_CLIPS), first + b0, &mut out[..4096]) != 0 {
        return o;
    }
    out[lead..lead + closing.len()].copy_from_slice(&closing);
    for (k, chunk) in out.chunks(RUN_MAX << 12).enumerate() {
        if ferros_sys::block_write(cap(grants(), S_CLIPS), first + b0 + (k * RUN_MAX) as u64, chunk) != 0 {
            say!("vault: extent at {}: its closing generation's write was refused; sealed at its frames' end", first);
            return o;
        }
    }
    o.write = ferros_sys::now() - t2;
    o.len = end + closing.len() as u64;
    o.hash = *blake3::hash(&closing).as_bytes();
    o
}

/// One extent request (`OP_EXTENT`): its value, with the reply's bytes in `out[8..]` (their count in `data`).
fn extent(store: &mut St, d: &Domain, name: &[u8], rest: &[u8], out: &mut [u8], data: &mut usize, token: &[u8; 32]) -> u64 {
    const NOTHING: u64 = ferros_sys::NOTHING;
    let (eop, block, len) = (rest[0], u64::from_le_bytes(rest[1..9].try_into().unwrap()), u32::from_le_bytes(rest[9..13].try_into().unwrap()) as usize);
    let bytes = &rest[13..];
    let t_in = ferros_sys::now();
    let key = extent_name(d, name);
    let owner = *blake3::hash(&d.addr).as_bytes();
    // Clips left open by a writer that is gone are handed to the recoverer (`recover_kick`): an OPEN takes a free stretch at once and never waits for them.
    recover_kick(store);
    // But an extent this request removes or replaces is not one the recoverer may still be writing in: its recovery is let finish first (a few hundred ms since 2026-11-19).
    if (matches!(eop, EOP_OPEN | EOP_DELETE) && unsafe { *(&raw const RECOVERING) }.is_some_and(|r| r.0 == key)) || (matches!(eop, EOP_DROP_AT | EOP_DROP_ORPHAN) && unsafe { *(&raw const RECOVERING) }.is_some_and(|r| r.1 == block)) {
        recover_commit(store, true);
    }
    let Some(t) = extents(store) else { return ERR };
    let now = eagle();
    match eop {
        EOP_OPEN => {
            let blocks = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            // A name recorded again starts afresh: its old extent, open or sealed, goes in the same commit; the table changes only once that commit is made.
            let mut next = t.clone();
            let had = next.remove(&key).is_some();
            match next.reserve(key, owner, blocks, now).and_then(|e| next.store(store.engine(), now).map(|()| e)) {
                Ok(e) => {
                    *t = next;
                    // The extent's blocks given to the caller for its own writes (a kernel from 2026-11-19 on): the clip's bytes then go straight to the flash, not thru this process. Refused, the caller writes thru the vault (EOP_WRITE) as before.
                    let given = extent_give(token, e.first, e.blocks, false);
                    let through = unsafe { &mut *(&raw mut THROUGH) };
                    through.retain(|n| *n != key);
                    if let Some(tok) = given {
                        out[8..40].copy_from_slice(&tok);
                        *data = 32;
                    } else {
                        through.push(key);
                    }
                    let f = freq();
                    say!("vault: extent opened: {} blocks at {} of the clips area{}{}; {} free at most; served in {} us{}", e.blocks, e.first, if had { ", over the name's last" } else { "" }, if given.is_some() { ", given to the caller" } else { "" }, t.largest_free(), (ferros_sys::now() - t_in) * 1_000_000 / f, if unsafe { (*(&raw const RECOVERING)).is_some() } { ", a recovery running beside it" } else { "" });
                    e.blocks
                }
                Err(err) => {
                    say!("vault: extent open of {} blocks refused: {:?}; {} free at most", blocks, err, t.largest_free());
                    ERR
                }
            }
        }
        EOP_WRITE | EOP_READ => {
            let n = (len >> 12) as u64;
            // WHY: the block and the length are the caller's; an extent's own blocks are all it may touch (the kernel bounds the call to the clips window besides).
            let Some(e) = t.find(&key).copied().filter(|e| e.holds(block, n)) else { return ERR };
            if eop == EOP_WRITE {
                if e.sealed {
                    return ERR;
                }
                // One UFS command: the kernel took at most EXTENT_MAX (BLOCK_MAX blocks) for an extent write.
                let t0 = ferros_sys::now();
                let r = ferros_sys::block_write(cap(grants(), S_CLIPS), e.first + block, &bytes[..len]);
                // The writes' own measure, said at the seal: calls, bytes, time in the block calls, and from the first write.
                unsafe {
                    let m = &mut *(&raw mut EXT_METER);
                    if m.0 == 0 {
                        m.3 = t0;
                    }
                    m.0 += 1;
                    m.1 += len as u64;
                    m.2 += ferros_sys::now() - t0;
                }
                if r == 0 { 0 } else { ERR }
            } else if ferros_sys::block_read(cap(grants(), S_CLIPS), e.first + block, &mut out[8..8 + len]) == 0 {
                *data = len;
                len as u64
            } else {
                ERR
            }
        }
        EOP_SEAL => {
            let length = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            let hash: [u8; 32] = bytes[8..40].try_into().unwrap();
            let mut next = t.clone();
            match next.seal(&key, length, hash, now).and_then(|_| next.store(store.engine(), now)) {
                Ok(()) => {
                    *t = next;
                    // Sealed: the caller's own writes end (it holds no extent any more).
                    let _ = extent_give(token, 0, 0, true);
                    let m = unsafe { core::mem::take(&mut *(&raw mut EXT_METER)) };
                    let f = freq();
                    say!("vault: extent sealed, {} bytes: {} writes of {} MiB in all, {} ms in the block calls ({} MB/s), {} ms from the first write", length, m.0, m.1 >> 20, m.2 * 1000 / f, m.1 * f / m.2.max(1) / 1_000_000, (ferros_sys::now() - m.3) * 1000 / f);
                    0
                }
                Err(err) => {
                    say!("vault: extent seal of {} bytes refused: {:?}", length, err);
                    ERR
                }
            }
        }
        EOP_INFO => match t.find(&key) {
            Some(e) => {
                let o = &mut out[8..8 + 65];
                o[..8].copy_from_slice(&e.blocks.to_le_bytes());
                o[8..16].copy_from_slice(&e.bytes.to_le_bytes());
                o[16] = e.sealed as u8;
                o[17..49].copy_from_slice(&e.hash);
                o[49..57].copy_from_slice(&e.at.to_le_bytes());
                o[57..65].copy_from_slice(&e.first.to_le_bytes());
                *data = 65;
                e.blocks
            }
            None => NOTHING,
        },
        EOP_SPACE => {
            // Read only: the area, what every extent takes, the longest free stretch, this domain's own extents' blocks and count (Lumis's recording space).
            let (mine, n) = t.list().iter().filter(|e| e.owner == owner).fold((0u64, 0u64), |(b, c), e| (b + e.blocks, c + 1));
            let free = t.area - t.taken();
            for (k, v) in [t.area, t.taken(), t.largest_free(), mine, n].iter().enumerate() {
                out[8 + k * 8..16 + k * 8].copy_from_slice(&v.to_le_bytes());
            }
            *data = 40;
            free
        }
        EOP_DELETE => {
            let mut next = t.clone();
            match next.remove(&key) {
                Some(_) => match next.store(store.engine(), now) {
                    Ok(()) => {
                        *t = next;
                        0
                    }
                    Err(_) => ERR,
                },
                None => NOTHING,
            }
        }
        EOP_LIST => {
            // The caller's own domain's owner first (how a host tells its extents from others'), then a record an extent, as many as the buffer holds; the value: how many there are.
            out[8..40].copy_from_slice(&owner);
            let rows = len.saturating_sub(32) / LIST_ROW; // WHY/PROOF: the length is the kernel's stamp of the host's ask; one under 32 lists no record rather than wrapping to a huge count
            let mut o = 40;
            for e in t.list().iter().take(rows) {
                out[o..o + 8].copy_from_slice(&e.first.to_le_bytes());
                out[o + 8..o + 16].copy_from_slice(&e.blocks.to_le_bytes());
                out[o + 16..o + 24].copy_from_slice(&e.bytes.to_le_bytes());
                out[o + 24] = e.sealed as u8;
                out[o + 25..o + 33].copy_from_slice(&e.at.to_le_bytes());
                out[o + 33..o + 65].copy_from_slice(&e.owner);
                o += LIST_ROW;
            }
            *data = o - 8;
            t.list().len() as u64
        }
        EOP_DROP_AT => {
            // Only the caller's own domain's extent at that first block: no domain drops another's.
            let Some(e) = t.list().iter().find(|e| e.first == block && e.owner == owner).copied() else { return NOTHING };
            let mut next = t.clone();
            next.remove(&e.name);
            match next.store(store.engine(), now) {
                Ok(()) => {
                    *t = next;
                    say!("vault: extent at {} dropped by its first block ({} blocks, {} bytes, sealed {}); {} free at most", e.first, e.blocks, e.bytes, e.sealed, t.largest_free());
                    0
                }
                Err(_) => ERR,
            }
        }
        EOP_DROP_ORPHAN => {
            // Any domain's, but only a sealed one: an open extent may still have a writer, or is the recoverer's to close.
            let Some(e) = t.list().iter().find(|e| e.first == block && e.sealed).copied() else { return NOTHING };
            let mut next = t.clone();
            next.remove(&e.name);
            match next.store(store.engine(), now) {
                Ok(()) => {
                    *t = next;
                    say!("vault: orphan extent at {} dropped by its first block, owner {:02X}{:02X}{:02X}{:02X}.. ({} blocks, {} bytes); {} free at most", e.first, e.owner[0], e.owner[1], e.owner[2], e.owner[3], e.blocks, e.bytes, t.largest_free());
                    0
                }
                Err(_) => ERR,
            }
        }
        _ => ERR,
    }
}

/// The largest value an export carries, and the largest export: the document is built whole in this process's heap (HEAP_BYTES). A value past the first is left out and named on the console (the person's made wallpaper, `map.wallpaper.made`, is 24 MB and belongs to this device's screen alone); an export past the second is refused.
const EXPORT_VALUE_MAX: u64 = 1 << 20;
const EXPORT_MAX: u64 = 1 << 22;

/// The person's export as the kernel's own export builds it (service.rs `store_export`): section `identity_export`, one `entry` field per name, its values the full name (`d`) and the stored bytes (`v'r'`). None when a domain cannot be read or it would pass EXPORT_MAX.
fn export_document(store: &mut St, doms: &[(&[u8], Domain)]) -> Option<Vec<u8>> {
    let parts: Vec<(&[u8], &Domain)> = doms.iter().map(|(p, d)| (*p, d)).collect();
    let x = match store.export(&parts, EXPORT_VALUE_MAX, EXPORT_MAX) {
        Ok(x) => x,
        Err(e) => {
            say!("vault: export refused: {:?}", e);
            return None;
        }
    };
    for (name, n) in &x.left_out {
        say!("vault: export leaves out {} ({} bytes)", core::str::from_utf8(name).unwrap_or("?"), n);
    }
    let mut section = vsf::VsfSection::new("identity_export");
    // A name is a `d`, ASCII: one that is not cannot be carried, and the export fails whole rather than leaving it behind unsaid.
    let mut unnamed = 0usize;
    for (name, value) in &x.entries {
        match vsf::Ascii::from_bytes(name) {
            Some(n) => section.add_field_multi("entry", alloc::vec![vsf::VsfType::d(n), vsf::VsfType::v(b'r', value.clone())]),
            None => {
                say!("vault: export: a name that is not ASCII ({} bytes) cannot be carried", name.len());
                unnamed += 1;
            }
        }
    }
    say!("vault: export: {} entries", x.entries.len());
    let doc = if unnamed == 0 { vsf::VsfBuilder::new().creation_time_oscillations(eagle()).provenance_only().add_section_direct(section).build().ok() } else { None };
    for (_, mut v) in x.entries {
        v.fill(0);
    }
    doc
}

/// Eagle time now, in oscillations (the kernel's settings.rs `eagle_now`, from the wall clock the kernel publishes): 0 until the kernel has read the PMIC's clock.
/// What every manifestus commit and every head and list document of the store is stamped with (docs/VSF-AUDIT.md P14, 2026-11-17; seconds since the boot until then, stamped as if they were Eagle time).
/// The kernel reads the clock before the vault opens and publishes it at the service's start, before this process is spawned, so 0 here means the clock could not be read: the Eagle epoch, every reader's "no clock", never a made-up time. manifestus records the time and never compares it.
fn eagle() -> i64 {
    // The Eagle epoch (1969-07-20 20:17:40 UTC) in the wall clock's seconds.
    const EPOCH_UNIX: i128 = -14_182_940;
    let Some((secs, ticks, hz)) = ferros_sys::utc_time() else { return 0 };
    let ops = vsf::OSCILLATIONS_PER_SECOND as i128;
    ((secs as i128 - EPOCH_UNIX) * ops + ticks as i128 * ops / hz as i128) as i64
}

fn reply(out: &mut [u8], ret: u64, data: usize) -> usize {
    out[..8].copy_from_slice(&ret.to_le_bytes());
    8 + data
}

/// Every old entry of the kernel's vault that belongs here, written into its domain and then removed from the kernel's vault. One that will not move is skipped (and left where it is).
fn migrate(store: &mut St) {
    let c = cap(grants(), S_VAULT);
    let (mut skip, mut moved, mut bytes) = (0u64, 0u32, 0u64);
    let t0 = now();
    let mut head = [0u8; 512];
    let mut piece = alloc::vec![0u8; 4096];
    loop {
        let n = ferros_sys::vault_next(c, skip, &mut head);
        if n == ferros_sys::NOTHING || n == ERR {
            break;
        }
        let n = n as usize;
        let (_, mut d) = domain_at(&head, 0);
        unsafe { SERVING = (OP_MIGRATE, 0, 0, n - 66) };
        let full = head[66..n].to_vec();
        let rel = &full[head[65] as usize..];
        head.fill(0);
        let mut w = store.begin(&d, rel);
        let mut offset = 0u64;
        let ok = loop {
            let whole = ferros_sys::vault_read(c, &full, offset, &mut piece);
            if whole == ERR || whole == ferros_sys::NOTHING {
                break false;
            }
            let take = ((whole - offset) as usize).min(piece.len());
            if store.write(&d, &mut w, &piece[..take]).is_err() {
                break false;
            }
            offset += take as u64;
            if offset == whole {
                break store.finish(&d, w).is_ok();
            }
        };
        wipe(&mut d);
        piece.fill(0);
        if ok && ferros_sys::vault_done(c, &full) == 0 {
            moved += 1;
            bytes += offset;
        } else {
            say!("vault: {} did not move; left in the kernel's vault", core::str::from_utf8(&full).unwrap_or("?"));
            skip += 1;
        }
    }
    if moved > 0 || skip > 0 {
        say!("vault: moved {} entries ({} bytes) from the kernel's vault in {} ms, {} left", moved, bytes, (now() - t0) * 1000 / freq(), skip);
    }
}

/// The request being served, for the death report (`panic`): its op (`OP_*`, the four of `serve` below them; G#FF migrating), the value's offset and the transfer's length, the name's length; set at each request's start. The serving thread's alone (the helpers and the reader only open and read for it).
static mut SERVING: (u8, u64, u64, usize) = (u8::MAX, 0, 0, 0);
/// While migrating (`migrate`): no request of a process, the kernel's old entries moving.
const OP_MIGRATE: u8 = u8::MAX;

/// The engine's live set as it stood after the last request (`live_note`), for the death report: blocks with a recorded hash, blocks live by position alone, and the bytes it holds.
/// 2026-11-13, the 8a: "memory allocation of 21495816 bytes failed serving a set" was this set's hashbrown table doubling to 2^19 buckets (2^19 × 41 + 8 bytes), and 2026-11-12's 10,747,912 "serving a delete" the doubling before it; manifestus `live-shards` makes it a direct index by position in pages, whose largest allocation is 32 KiB.
static mut LIVE: (usize, usize, usize) = (0, 0, 0);

/// The live set's counts and bytes noted (`LIVE`). The serving thread's alone.
fn live_note(v: &Vault<Half, Half>) {
    let (hashed, tree) = v.live_counts();
    unsafe { LIVE = (hashed, tree, v.live_bytes()) };
}

/// What the death report calls an op.
fn op_name(op: u8) -> &'static str {
    match op {
        0 => "a set",
        1 => "a get",
        2 => "a delete",
        3 => "a list",
        OP_DROP => "a drop",
        OP_EXPORT => "an export",
        OP_SIZE => "a size",
        OP_MIGRATE => "the migration",
        _ => "no request",
    }
}

/// The vault process's own death report (2026-11-12, the 8a: it died during a Photon re-stow and nothing said why, the console lines lost): the panic (a refused allocation says "memory allocation of N bytes failed"), the request it was serving, and the heap's state (in use, its high water, free and in how many blocks, the largest of them: a first-fit heap with room left but no block large enough is fragmented, not full). One line on the console; the kernel keeps a process's last line and writes it to the flash log when the process ends (process.rs `end_process`), so it survives the reset that may follow.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let (op, offset, len, name) = unsafe { SERVING };
    // The heap's lock is not taken while a panic says so (try_lock): the panic may have come from inside the allocator, and a spin on its lock here would hang the report.
    let (free, blocks, largest) = match ALLOC.try_lock() {
        Some(h) => (h.free_bytes() >> 10, h.free_blocks(), h.largest_free() >> 10),
        None => (0, 0, 0),
    };
    let (hashed, tree, live_bytes) = unsafe { LIVE };
    say!(
        "vault: panic: {} serving {} (offset {}, {} bytes, a name of {} bytes); the live set {} hashed, {} by position, {} KiB; the heap: {} KiB in use, high water {} of {} KiB, {} KiB free in {} blocks, the largest {} KiB",
        info,
        op_name(op),
        offset,
        len,
        name,
        hashed,
        tree,
        live_bytes >> 10,
        ferros_heap::IN_USE.load(Relaxed) >> 10,
        ferros_heap::PEAK.load(Relaxed) >> 10,
        HEAP_BYTES >> 10,
        free,
        blocks,
        largest
    );
    ferros_sys::exit(1)
}
