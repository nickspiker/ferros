//! The vault process (docs/VAULT-OUT.md): the vault out of the kernel. It owns its disk (a window of blocks the kernel moves for it, `ferros_sys::block_read`/`block_write`), the layout (manifestus: the rotating ring, the tract, the HAMT, two mirrors), and the values of any size, sealed in pieces (`ferros_store`); the kernel only moves blocks and, from step 3, issues each request's domain keys.
//!
//! Step 1 (2026-11-07): this process runs beside the kernel's own vault, on a new, empty region, and serves test domains whose keys are fixed and not secret (`test_domain`), so its disk path and its store can be proven on the device before anything depends on it. Nothing of a person's goes here until the kernel issues real keys.
//!
//! The mirrors: A from block 0 of the window, B from half way (the window is 2 × 8 GiB, process.rs WINDOWS); each starts at MIRROR blocks (512 MiB) and grows into its half later (manifestus `grow`), because a blank vault is read whole before its first commit and 8 GiB would be four million reads.
//!
//! Requests (≤ 4 KiB, op-byte messages as the display's): [op][domain][name length][name][rest]; the reply's first byte is the status (0 done, 1 absent, 2 failed, 3 a bad request).
//! - 0 set: rest [offset u64 LE][more u8][bytes]: the value's bytes from `offset`, in order from 0; the piece without `more` finishes it.
//! - 1 get: rest [offset u64 LE]: reply [status][whole length u64 LE][the bytes from offset, as many as fit].
//! - 2 delete: reply [status][whether it was there].
//! - 3 names: rest [index u32 LE]: reply [status][count u32 LE][from that index, each name as its length then its bytes, as many as fit].
//! - 4 sweep: reply [status][pieces removed u64 LE].
//!
//! Grant slots: 0 console, 1 serve `vault`, 2 the `vault` block window, 3 entropy.
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

/// Step 1's test domains: keys fixed by the domain's number and not secret, for proving the disk path and the store on the device. Gone once the kernel issues real keys (VAULT-OUT.md, step 3).
fn test_domain(n: u8) -> Domain {
    Domain { addr: blake3::derive_key("ferros vault process step 1 test domain addresses", &[n]), seal: blake3::derive_key("ferros vault process step 1 test domain seal", &[n]) }
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
    let mut msg = alloc::vec![0u8; 4096];
    let mut out = alloc::vec![0u8; 4096];
    let mut token = [0u8; 32];
    // Values being written a piece at a time: (domain, name, the next offset expected, the writer).
    let mut writers: Vec<(u8, Vec<u8>, u64, Writer)> = Vec::new();
    loop {
        let n = ferros_sys::recv(cap(grants(), S_SERVE), &mut msg, &mut token, false);
        if n == ERR {
            continue;
        }
        let len = serve(&mut store, &mut writers, &msg[..n as usize], &mut out);
        ferros_sys::reply(&token, &out[..len]);
    }
}

/// One request into `out`; returns the reply's length.
fn serve(store: &mut Store<Half, Half, Entropy>, writers: &mut Vec<(u8, Vec<u8>, u64, Writer)>, m: &[u8], out: &mut [u8]) -> usize {
    // A message is the caller's: every length in it is checked against what it holds.
    if m.len() < 3 || m.len() < 3 + m[2] as usize {
        out[0] = 3;
        return 1;
    }
    let (op, dn, name, rest) = (m[0], m[1], &m[3..3 + m[2] as usize], &m[3 + m[2] as usize..]);
    let d = test_domain(dn);
    let u64_at = |b: &[u8]| -> Option<u64> { b.get(..8).map(|s| u64::from_le_bytes(s.try_into().unwrap_or([0; 8]))) };
    match op {
        0 if !name.is_empty() && rest.len() >= 9 => {
            let (offset, more, bytes) = (u64_at(rest).unwrap_or(0), rest[8] != 0, &rest[9..]);
            let i = match writers.iter().position(|w| w.0 == dn && w.1 == name) {
                Some(i) if offset == writers[i].2 => i,
                // A value starts at offset 0; one restarted from 0 starts afresh (its earlier pieces are left for the sweep).
                Some(i) if offset == 0 => {
                    writers.swap_remove(i);
                    writers.push((dn, name.to_vec(), 0, store.begin(&d, name)));
                    writers.len() - 1
                }
                None if offset == 0 => {
                    writers.push((dn, name.to_vec(), 0, store.begin(&d, name)));
                    writers.len() - 1
                }
                _ => {
                    out[0] = 3;
                    return 1;
                }
            };
            let ok = store.write(&d, &mut writers[i].3, bytes).is_ok();
            writers[i].2 += bytes.len() as u64;
            out[0] = if !ok {
                writers.swap_remove(i);
                2
            } else if !more {
                let w = writers.swap_remove(i).3;
                if store.finish(&d, w).is_ok() { 0 } else { 2 }
            } else {
                0
            };
            1
        }
        1 => {
            let offset = u64_at(rest).unwrap_or(0);
            let room = out.len() - 9;
            match store.read(&d, name, offset, &mut out[9..]) {
                Ok(Some(whole)) => {
                    out[0] = 0;
                    out[1..9].copy_from_slice(&whole.to_le_bytes());
                    // WHY: the offset is the caller's and may lie past the end (nothing to send then: the subtraction stops at 0), and a reply holds what fits in 4 KiB after its header.
                    9 + (whole.saturating_sub(offset) as usize).min(room)
                }
                Ok(None) => {
                    out[0] = 1;
                    1
                }
                Err(e) => {
                    say!("vault: read failed: {:?}", e);
                    out[0] = 2;
                    1
                }
            }
        }
        2 => match store.delete(&d, name) {
            Ok(was) => {
                out[0] = 0;
                out[1] = was as u8;
                2
            }
            Err(_) => {
                out[0] = 2;
                1
            }
        },
        3 => {
            let from = rest.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4]))).unwrap_or(0) as usize;
            let Ok(names) = store.names(&d) else {
                out[0] = 2;
                return 1;
            };
            out[0] = 0;
            let mut at = 5;
            let mut count = 0u32;
            for nm in names.iter().skip(from) {
                if at + 1 + nm.len() > out.len() {
                    break;
                }
                out[at] = nm.len() as u8;
                out[at + 1..at + 1 + nm.len()].copy_from_slice(nm);
                at += 1 + nm.len();
                count += 1;
            }
            out[1..5].copy_from_slice(&count.to_le_bytes());
            at
        }
        4 => match store.sweep(&d) {
            Ok(n) => {
                out[0] = 0;
                out[1..9].copy_from_slice(&n.to_le_bytes());
                9
            }
            Err(_) => {
                out[0] = 2;
                1
            }
        },
        _ => {
            out[0] = 3;
            1
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("vault: panic: {}", info);
    ferros_sys::exit(1)
}
