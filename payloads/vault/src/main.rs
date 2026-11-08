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
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU64, AtomicUsize};
use ferros_store::{Domain, Nonces, Opener, Sealed, Store, Writer};
use ferros_sys::{cap, console, freq, now, ERR};
use manifestus::{Block, BlockDev, DeletePolicy, Mirror, Vault};

/// The heap: manifestus's working set (its index walks, the reap's windows) and a piece being opened or sealed. 24 MiB since 2026-11-08: a write grows a buffer of manifestus's with the vault's entries (about 926 bytes an entry, doubling: 671,760 bytes writing a 47 MB kist capsule into an empty store, 1,343,504 writing it again, measured on the host, ferros_store `rewrite_peak`), and at 8 MiB the vault process died rewriting Mahere's capsule. The growth itself is manifestus's to bound.
const HEAP_BYTES: usize = 24 << 20;
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
        if !blocks_read(self.base + lba, buf) {
            return Err(manifestus::Error::Corrupt(alloc::format!("block read failed at {lba}")));
        }
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &Block) -> manifestus::Result<()> {
        self.io(lba, 1)?;
        if ferros_sys::block_write(cap(grants(), S_BLOCKS), self.base + lba, buf) != 0 {
            return Err(manifestus::Error::Corrupt(alloc::format!("block write failed at {lba}")));
        }
        Ok(())
    }
    /// Up to `RUN_NOW` blocks a kernel call (one UFS command each on a kernel from 2026-11-08 on).
    fn read_run(&mut self, lba: u64, out: &mut [Block]) -> manifestus::Result<()> {
        self.io(lba, out.len() as u64)?;
        let bytes = unsafe { core::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, out.len() << 12) };
        if !blocks_read(self.base + lba, bytes) {
            return Err(manifestus::Error::Corrupt(alloc::format!("block read failed at {lba}")));
        }
        Ok(())
    }
    fn write_run(&mut self, lba: u64, blocks: &[&Block]) -> manifestus::Result<()> {
        self.io(lba, blocks.len() as u64)?;
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

/// manifestus needs a recorded time, not a clock: seconds since this boot, as the kernel's vault records.
fn seconds() -> i64 {
    (now() / freq()) as i64
}

struct Log {
    b: [u8; 256],
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
        let mut l = Log { b: [0; 256], n: 0 };
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

/// The most blocks one kernel call moves on a kernel from 2026-11-08 on (process.rs BLOCK_MAX: one UFS command, 128 KiB, so a 64 KiB piece's 17 blocks are one call), and on one from before (16, each block its own command): the first refusal of a larger run drops to the smaller for good.
const RUN_MAX: usize = 1 << 5;
static RUN_NOW: AtomicUsize = AtomicUsize::new(RUN_MAX);

/// Block reads, measured for the read report (`Meter`): calls' time in timer ticks, and blocks.
static IO_TICKS: AtomicU64 = AtomicU64::new(0);
static IO_BLOCKS: AtomicU64 = AtomicU64::new(0);

/// `buf.len() / 4096` blocks of the window from `at` into `buf`, `RUN_NOW` blocks a call.
fn blocks_read(at: u64, buf: &mut [u8]) -> bool {
    let t = now();
    let n = buf.len() >> 12;
    let mut k = 0;
    while k < n {
        let m = (n - k).min(RUN_NOW.load(Relaxed));
        if ferros_sys::block_read(cap(grants(), S_BLOCKS), at + k as u64, &mut buf[k << 12..(k + m) << 12]) != 0 {
            // A kernel from before 2026-11-08 refuses more than RUN blocks a call: once, then the smaller run.
            if m > RUN {
                RUN_NOW.store(RUN, Relaxed);
                continue;
            }
            return false;
        }
        k += m;
    }
    IO_TICKS.fetch_add(now() - t, Relaxed);
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
            if m > RUN {
                RUN_NOW.store(RUN, Relaxed);
                continue;
            }
            return false;
        }
        k += m;
    }
    true
}

/// The helpers that open a read's pieces beside the serving thread (owner, 2026-11-08: "24MB from the vault should be imperceptibly fast"): a 256 KiB transfer (ferros_sys::STORE_MAX) is four 64 KiB pieces, one a core. They hold nothing between requests: the sealing key is on the board only while a request's pieces open, and wiped after.
const HELPERS: usize = 3;
const HELPER_STACK: usize = 1 << 16;
static HELPER_STACKS: [Region<HELPER_STACK>; HELPERS] = [const { Region(UnsafeCell::new([0; HELPER_STACK])) }; HELPERS];

/// The board: the batch (bits 32-63, moved for each), its piece count (bits 16-31) and the next piece to claim (bits 0-15). A claim is a compare-and-swap within one batch, so a helper late to a batch claims nothing of the next: a claim it wins is of a batch whose server still waits, whose pieces and key are still on the board.
static BOARD_NEXT: AtomicU64 = AtomicU64::new(0);
/// Pieces of the batch opened.
static BOARD_DONE: AtomicU64 = AtomicU64::new(0);
/// The batch's pieces (their address) and the sealing key.
static mut BOARD: (usize, [u8; 32]) = (0, [0; 32]);

/// Claim and open the board's pieces until none is left to claim.
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
        // SAFETY: piece `next` < n of the batch this claim is of: its server waits for every claimed piece (BOARD_DONE) before the slice or the key leave the board, and no other thread claims the same index.
        ferros_store::open_piece(key, unsafe { &mut *(at as *mut Sealed).add(next as usize) });
        if BOARD_DONE.fetch_add(1, AcqRel) + 1 == n {
            ferros_sys::wake(&BOARD_DONE, 1);
        }
    }
}

extern "C" fn helper(_: u64) -> ! {
    loop {
        let c = BOARD_NEXT.load(Acquire);
        work_board();
        ferros_sys::wait(&BOARD_NEXT, c, 0);
    }
}

/// The serving thread's opener: a lone piece here, more on the board with the helpers (the server opens its share too), waited for, the key wiped.
struct Helpers;

impl Opener for Helpers {
    fn open_all(&mut self, seal: &[u8; 32], pieces: &mut [Sealed]) {
        let t = now();
        let n = pieces.len() as u64;
        if n == 1 {
            ferros_store::open_piece(seal, &mut pieces[0]);
        } else {
            unsafe { BOARD = (pieces.as_mut_ptr() as usize, *seal) };
            BOARD_DONE.store(0, Release);
            let batch = (BOARD_NEXT.load(Acquire) >> 32) + 1;
            BOARD_NEXT.store(batch << 32 | n << 16, Release);
            ferros_sys::wake(&BOARD_NEXT, HELPERS as u64);
            work_board();
            loop {
                let d = BOARD_DONE.load(Acquire);
                if d >= n {
                    break;
                }
                ferros_sys::wait(&BOARD_DONE, d, 0);
            }
            unsafe {
                (*(&raw mut BOARD.1)).fill(0);
                BOARD.0 = 0;
            }
        }
        OPEN_TICKS.fetch_add(now() - t, Relaxed);
        OPEN_BYTES.fetch_add(pieces.iter().map(|p| p.bytes.len() as u64).sum(), Relaxed);
    }
}

/// Opening, measured for the read report: the wall time of each request's opening, and the sealed bytes opened.
static OPEN_TICKS: AtomicU64 = AtomicU64::new(0);
static OPEN_BYTES: AtomicU64 = AtomicU64::new(0);

/// A large value read from its first byte to its last, measured stage by stage (owner, 2026-11-08: "Report MB/s per stage"): the block reads (the kernel's UFS commands and copies), the opening (XChaCha20-Poly1305, on the helpers), and the rest of serving (manifestus's index walk and each block's BLAKE3 guard and decode, the copies); from the first request to the last, the requests' own time and the time between them (the kernel's hand-offs and the reader's own work).
struct Meter {
    on: bool,
    t0: u64,
    requests: u64,
    serving: u64,
    io: (u64, u64),
    open: (u64, u64),
}

/// Values at least this large get a read report.
const METER_MIN: u64 = 1 << 20;

impl Meter {
    /// A read of `len` bytes from `offset` of a value `whole` bytes long, served in `ticks`: starts at offset 0, said when it reaches the end.
    fn note(&mut self, offset: u64, data: u64, whole: u64, ticks: u64, t_start: u64) {
        if offset == 0 && whole >= METER_MIN {
            *self = Meter { on: true, t0: t_start, requests: 0, serving: 0, io: (IO_TICKS.load(Relaxed), IO_BLOCKS.load(Relaxed)), open: (OPEN_TICKS.load(Relaxed), OPEN_BYTES.load(Relaxed)) };
        }
        if !self.on {
            return;
        }
        self.requests += 1;
        self.serving += ticks;
        if offset + data < whole {
            return;
        }
        self.on = false;
        let f = freq();
        let us = |t: u64| t * 1_000_000 / f;
        // WHY the max: a stage that took no time (every piece already open, no block read) would divide by zero, and the report is not worth the process.
        let rate = |bytes: u64, t: u64| bytes / us(t).max(1);
        let (io_t, io_b) = (IO_TICKS.load(Relaxed) - self.io.0, (IO_BLOCKS.load(Relaxed) - self.io.1) << 12);
        let (op_t, op_b) = (OPEN_TICKS.load(Relaxed) - self.open.0, OPEN_BYTES.load(Relaxed) - self.open.1);
        let wall = now() - self.t0;
        // Signed: block reads made by a migration in the same requests count in `io` too.
        let rest = us(self.serving) as i64 - us(io_t) as i64 - us(op_t) as i64;
        say!(
            "vault: read {} B in {} requests, {} ms first to last ({} MB/s): serving {} ms; blocks {} B in {} ms ({} MB/s); opening {} B in {} ms ({} MB/s, {} cores); index, block guards and copies {} ms; between requests {} ms",
            whole,
            self.requests,
            us(wall) / 1000,
            rate(whole, wall),
            us(self.serving) / 1000,
            io_b,
            us(io_t) / 1000,
            rate(io_b, io_t),
            op_b,
            us(op_t) / 1000,
            rate(op_b, op_t),
            HELPERS + 1,
            rest / 1000,
            us(wall - self.serving) / 1000
        );
    }
}

static mut METER: Meter = Meter { on: false, t0: 0, requests: 0, serving: 0, io: (0, 0), open: (0, 0) };

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
    let vault = match Vault::open(mirror, RING_LOG2, seconds(), DeletePolicy::UnlinkOnly) {
        Ok(v) => v,
        Err(e) => {
            say!("vault: the region did not open: {:?}", e);
            ferros_sys::exit(2);
        }
    };
    say!("vault: tract {} blocks from {}, the window {} a mirror", vault.tract_blocks(), vault.tract_base(), HALF);
    let mut store = Store::new(vault, Entropy { pool: [0; 4096], at: 4096 }, seconds);
    say!("vault: open in {} ms, serving", (now() - t0) * 1000 / freq());
    // The pieces' helpers, idle until a read hands them a batch (`Helpers`).
    for s in HELPER_STACKS.iter() {
        if ferros_sys::thread_spawn(helper, unsafe { &mut *s.0.get() }, 0) == ERR {
            say!("vault: a helper did not start; reads open their pieces on fewer cores");
        }
    }
    // One transfer (ferros_sys::STORE_MAX, 256 KiB) and its stamp each way, with room to spare.
    let mut msg = alloc::vec![0u8; ferros_sys::STORE_MAX + (1 << 12)];
    let mut out = alloc::vec![0u8; ferros_sys::STORE_MAX + (1 << 12)];
    let mut token = [0u8; 32];
    let mut writers: Vec<Pending> = Vec::new();
    let mut epoch: Option<u32> = None;
    // The person's export being read (op 5): built at offset 0, kept until read to its end or the person locks.
    let mut export: Option<Vec<u8>> = None;
    loop {
        let n = ferros_sys::recv(cap(grants(), S_SERVE), &mut msg, &mut token, false);
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
        // Old entries wait in the kernel's vault: they move before this request is served, so it never finds a name between the two vaults (gone from the kernel's, not yet here).
        if m[1] & 1 != 0 {
            migrate(&mut store);
        }
        let len = serve(&mut store, &mut writers, &mut export, m, &mut out);
        m.fill(0);
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
fn serve(store: &mut St, writers: &mut Vec<Pending>, export: &mut Option<Vec<u8>>, m: &[u8], out: &mut [u8]) -> usize {
    const NOTHING: u64 = ferros_sys::NOTHING;
    let (op, count) = (m[0], m[6] as usize);
    let at_name = 7 + count * 65;
    let name = &m[at_name + 1..at_name + 1 + m[at_name] as usize];
    let rest = &m[at_name + 1 + name.len()..];
    let u64_at = |b: &[u8]| u64::from_le_bytes(b[..8].try_into().unwrap());
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
    } else if op == OP_DROP {
        // Release (the kernel's own call): each stamped domain dropped whole (ferros_store `drop_domain`), any write of it in progress with it. How many values went.
        let (mut n, mut ok) = (0u64, true);
        for i in 0..count {
            let (_, mut d) = domain_at(m, 7 + i * 65);
            let id = *blake3::hash(&d.addr).as_bytes();
            writers.retain(|p| p.domain != id);
            match store.drop_domain(&d) {
                Ok(k) => n += k,
                Err(_) => ok = false,
            }
            wipe(&mut d);
        }
        say!("vault: dropped {} domains ({} values){}", count, n, if ok { "" } else { ", some failed" });
        if ok { n } else { ERR }
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
                    let w = writers.swap_remove(i).w;
                    match store.finish(&d, w) {
                        Ok(()) => 0,
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
                // The pieces opened on the helpers as well as here (`Helpers`).
                match store.read_with(&d, name, offset, &mut out[8..8 + len], &mut Helpers) {
                    Ok(Some(whole)) => {
                        // WHY: the offset is the caller's and may lie past the end (nothing to send then: the subtraction stops at 0).
                        data = (whole.saturating_sub(offset) as usize).min(len);
                        unsafe { (*(&raw mut METER)).note(offset, data as u64, whole, now() - t, t) };
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
    for (name, value) in &x.entries {
        section.add_field_multi("entry", alloc::vec![vsf::VsfType::d(alloc::string::String::from_utf8_lossy(name).into_owned()), vsf::VsfType::v(b'r', value.clone())]);
    }
    say!("vault: export: {} entries", x.entries.len());
    let doc = vsf::VsfBuilder::new().creation_time_oscillations(eagle()).provenance_only().add_section_direct(section).build().ok();
    for (_, mut v) in x.entries {
        v.fill(0);
    }
    doc
}

/// Eagle time now, in oscillations (the kernel's settings.rs `eagle_now`, from the wall clock the kernel publishes): 0 until the kernel has read the PMIC's clock.
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

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("vault: panic: {}", info);
    ferros_sys::exit(1)
}
