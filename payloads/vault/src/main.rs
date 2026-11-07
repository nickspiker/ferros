//! The vault process (docs/VAULT-OUT.md): the vault out of the kernel. It owns its disk (a window of blocks the kernel moves for it, `ferros_sys::block_read`/`block_write`), the layout (manifestus: the rotating ring, the tract, the HAMT, two mirrors), and the values of any size, sealed in pieces (`ferros_store`). The kernel moves blocks and issues each request's domain keys; it holds, parses and seals no value.
//!
//! The mirrors: A from block 0 of the window, B from half way (the window is 2 × 8 GiB, process.rs WINDOWS); each starts at MIRROR blocks (512 MiB) and grows into its half later (manifestus `grow`), because a blank vault is read whole before its first commit and 8 GiB would be four million reads.
//!
//! Requests come only from the kernel (process.rs `store_forward`): each is a process's `SYS_STORE` call, stamped with the domain keys of its scope, one transfer at most. [op: 0 set, 1 get, 2 delete, 3 list][flags: bit 0 old entries wait in the kernel's vault][lock epoch u32 LE][domains][per domain: class (1 the person's), address key, sealing key][name length][name: within the scope, or 1 and a 32-byte address][rest]. Rest: set [offset u64][more u8][bytes]; get [offset u64][length u32]; list [offset u64][length u32][addresses u8]. The reply: [the syscall's value u64 LE][the bytes for the caller's buffer].
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
use ferros_store::{Domain, Nonces, Store, Writer};
use ferros_sys::{cap, console, freq, now, ERR};
use manifestus::{Block, BlockDev, DeletePolicy, Mirror, Vault};

/// The heap: manifestus's working set (its index walks, the reap's windows) and a piece being opened or sealed.
const HEAP_BYTES: usize = 8 << 20;
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

/// Each mirror's size to start with, in 4 KiB blocks (512 MiB), and where B starts in the window (its second half).
const MIRROR: u64 = 1 << 17;
const HALF: u64 = 1 << 21;
/// The ring's size, as the kernel's vault has it (manifestus's host default).
const RING_LOG2: u8 = manifestus::HOST_RING_LOG2;
/// Blocks moved by one kernel call at most (process.rs BLOCK_MAX).
const RUN: usize = 1 << 4;

static mut GRANTS: u64 = 0;

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
        if ferros_sys::block_read(cap(grants(), S_BLOCKS), self.base + lba, buf) != 0 {
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
    /// Up to RUN blocks a kernel call.
    fn read_run(&mut self, lba: u64, out: &mut [Block]) -> manifestus::Result<()> {
        self.io(lba, out.len() as u64)?;
        for (k, chunk) in out.chunks_mut(RUN).enumerate() {
            let bytes = unsafe { core::slice::from_raw_parts_mut(chunk.as_mut_ptr() as *mut u8, chunk.len() << 12) };
            if ferros_sys::block_read(cap(grants(), S_BLOCKS), self.base + lba + (k * RUN) as u64, bytes) != 0 {
                return Err(manifestus::Error::Corrupt(alloc::format!("block read failed at {lba}")));
            }
        }
        Ok(())
    }
    fn write_run(&mut self, lba: u64, blocks: &[&Block]) -> manifestus::Result<()> {
        self.io(lba, blocks.len() as u64)?;
        let mut buf = alloc::vec![0u8; RUN << 12];
        for (k, chunk) in blocks.chunks(RUN).enumerate() {
            for (j, b) in chunk.iter().enumerate() {
                buf[j << 12..(j + 1) << 12].copy_from_slice(&b[..]);
            }
            if ferros_sys::block_write(cap(grants(), S_BLOCKS), self.base + lba + (k * RUN) as u64, &buf[..chunk.len() << 12]) != 0 {
                return Err(manifestus::Error::Corrupt(alloc::format!("block write failed at {lba}")));
            }
        }
        Ok(())
    }
    /// Each block is on the flash when the kernel's call returns (the HAL polls the command to completion).
    fn flush(&mut self) -> manifestus::Result<()> {
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
pub extern "C" fn vault_main(g: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
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

type St = Store<Half, Half, Entropy>;

extern "C" fn work(_: u64) -> ! {
    let t0 = now();
    let mirror = Mirror::new(Half { base: 0, blocks: MIRROR }, Half { base: HALF, blocks: MIRROR });
    let vault = match Vault::open(mirror, RING_LOG2, seconds(), DeletePolicy::UnlinkOnly) {
        Ok(v) => v,
        Err(e) => {
            say!("vault: the region did not open: {:?}", e);
            ferros_sys::exit(2);
        }
    };
    let mut store = Store::new(vault, Entropy { pool: [0; 4096], at: 4096 }, seconds);
    say!("vault: open in {} ms, serving", (now() - t0) * 1000 / freq());
    // One transfer and its stamp each way, with room to spare.
    let mut msg = alloc::vec![0u8; 8 << 10];
    let mut out = alloc::vec![0u8; 8 << 10];
    let mut token = [0u8; 32];
    let mut writers: Vec<Pending> = Vec::new();
    let mut epoch: Option<u32> = None;
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
        }
        epoch = Some(e);
        // Old entries wait in the kernel's vault: they move before this request is served, so it never finds a name between the two vaults (gone from the kernel's, not yet here).
        if m[1] & 1 != 0 {
            migrate(&mut store);
        }
        let len = serve(&mut store, &mut writers, m, &mut out);
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
fn serve(store: &mut St, writers: &mut Vec<Pending>, m: &[u8], out: &mut [u8]) -> usize {
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
                let ok = store.write(&d, &mut writers[i].w, bytes).is_ok();
                writers[i].next += bytes.len() as u64;
                if !ok {
                    writers.swap_remove(i);
                    ERR
                } else if !more {
                    let w = writers.swap_remove(i).w;
                    if store.finish(&d, w).is_ok() { 0 } else { ERR }
                } else {
                    0
                }
            }
            1 => {
                let (offset, len) = (u64_at(rest), u32::from_le_bytes(rest[8..12].try_into().unwrap()) as usize);
                match store.read(&d, name, offset, &mut out[8..8 + len]) {
                    Ok(Some(whole)) => {
                        // WHY: the offset is the caller's and may lie past the end (nothing to send then: the subtraction stops at 0).
                        data = (whole.saturating_sub(offset) as usize).min(len);
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
