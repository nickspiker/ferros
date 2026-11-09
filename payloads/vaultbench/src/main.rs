//! The vault's read path, stage by stage, on this device's cores (owner, 2026-11-08: "24MB from the vault should be imperceptibly fast"): XChaCha20-Poly1305 opening a 64 KiB piece, BLAKE3 over 4 KiB blocks (manifestus's block guard), the keyed BLAKE3 of an address, and a 64 KiB copy, each in MB/s (or µs a call), on the X3 and on a worker. Built as the vault process builds (chacha20poly1305 0.10, blake3 `pure`), so a cfg such as `chacha20_force_neon` shows what it changes.
//!
//! `ferros-bridge spawn vaultbench.bin --name vaultbench`. Grant slots: 0 console.

#![no_std]
#![no_main]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{Key, Tag, XChaCha20Poly1305, XNonce};

/// Nothing here allocates; the allocator is for the crates' `alloc` feature only.
struct NoHeap;
unsafe impl GlobalAlloc for NoHeap {
    unsafe fn alloc(&self, _: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}
#[global_allocator]
static ALLOC: NoHeap = NoHeap;

const STACK_BYTES: usize = 1 << 16;
#[repr(C, align(16))]
struct Stack([u8; STACK_BYTES]);
#[unsafe(link_section = ".data.stack")]
static mut STACK: Stack = Stack([1; STACK_BYTES]);

/// A piece, as ferros_store seals one (64 KiB), and a second for the copy.
const PIECE: usize = 1 << 16;
#[repr(C, align(64))]
struct Buf(UnsafeCell<[u8; PIECE]>);
unsafe impl Sync for Buf {}
#[unsafe(link_section = ".data.buf")]
static A: Buf = Buf(UnsafeCell::new([1; PIECE]));
#[unsafe(link_section = ".data.buf")]
static B: Buf = Buf(UnsafeCell::new([2; PIECE]));

/// Each stage moves 2^ROUNDS_LOG2 pieces' worth (16 MiB).
const ROUNDS_LOG2: u32 = 8;

struct Log {
    grants: u64,
    buf: [u8; 1024],
    n: usize,
}

impl Log {
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn dec(&mut self, mut v: u64) {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let d = b;
        self.put(&d[i..]);
    }
    /// `bytes` over `us` as MB/s.
    fn rate(&mut self, what: &[u8], bytes: u64, us: u64) {
        self.put(what);
        self.put(b" ");
        self.dec(bytes / us.max(1));
        self.put(b" MB/s;");
    }
    fn flush(&mut self) {
        let mut r = [0u8; 16];
        ferros_sys::call(ferros_sys::cap(self.grants, 0), &self.buf[..self.n], &mut r);
        self.n = 0;
    }
}

static mut GRANTS: u64 = 0;

fn stages(l: &mut Log, core: &[u8]) {
    let (a, b) = unsafe { (&mut *A.0.get(), &mut *B.0.get()) };
    let rounds = 1u64 << ROUNDS_LOG2;
    let bytes = rounds * PIECE as u64;
    let key = [7u8; 32];
    let nonce = [9u8; 24];
    let at = [3u8; 32];
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let tag: Tag = cipher.encrypt_in_place_detached(XNonce::from_slice(&nonce), &at, a).unwrap();
    // Opening: the tag checked, then the keystream applied; sealed again each round so the next opening has a whole ciphertext to check (the pair is reported, halved).
    let t = ferros_sys::now();
    for _ in 0..rounds {
        cipher.decrypt_in_place_detached(XNonce::from_slice(&nonce), &at, a, &tag).unwrap();
        let _ = cipher.encrypt_in_place_detached(XNonce::from_slice(&nonce), &at, a).unwrap();
    }
    let us = ferros_sys::us_since(t);
    l.put(b"vaultbench: ");
    l.put(core);
    l.put(b": ");
    // Each round opened and sealed once: the open's rate is twice the pair's.
    l.rate(b"xchacha20poly1305 open (pair/2)", bytes * 2, us);
    let t = ferros_sys::now();
    let mut acc = 0u8;
    for _ in 0..rounds {
        for blk in a.chunks(1 << 12) {
            acc ^= blake3::hash(blk).as_bytes()[0];
        }
    }
    let us = ferros_sys::us_since(t);
    l.rate(b" blake3 4 KiB blocks", bytes, us);
    let t = ferros_sys::now();
    let n = 1u64 << 14;
    for i in 0..n {
        let mut h = blake3::Hasher::new_keyed(&key);
        h.update(b"piece\0");
        h.update(&at);
        h.update(&i.to_le_bytes());
        acc ^= h.finalize().as_bytes()[0];
    }
    let us = ferros_sys::us_since(t);
    l.put(b" keyed address hash ");
    l.dec(us * 1000 / n);
    l.put(b" ns;");
    let t = ferros_sys::now();
    for _ in 0..rounds {
        b.copy_from_slice(a);
        core::hint::black_box(&mut *b);
    }
    let us = ferros_sys::us_since(t);
    l.rate(b" copy 64 KiB", bytes, us);
    // The two halves of the open on their own: the XChaCha20 keystream applied, and Poly1305 over the piece.
    {
        use chacha20::cipher::{KeyIvInit, StreamCipher};
        let t = ferros_sys::now();
        for _ in 0..rounds {
            let mut c = chacha20::XChaCha20::new(Key::from_slice(&key), XNonce::from_slice(&nonce));
            c.apply_keystream(a);
        }
        let us = ferros_sys::us_since(t);
        l.rate(b" xchacha20", bytes, us);
        use poly1305::universal_hash::{KeyInit as _, UniversalHash};
        let t = ferros_sys::now();
        for _ in 0..rounds {
            let mut p = poly1305::Poly1305::new(poly1305::Key::from_slice(&key));
            p.update_padded(a);
            acc ^= p.finalize()[0];
        }
        let us = ferros_sys::us_since(t);
        l.rate(b" poly1305", bytes, us);
    }
    l.put(if acc == 0 { b"\n" } else { b" \n" });
    l.flush();
}

extern "C" fn work(_: u64) -> ! {
    let mut l = Log { grants: unsafe { GRANTS }, buf: [0; 1024], n: 0 };
    ferros_sys::pin(ferros_sys::CORES_BIG);
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
    stages(&mut l, b"X3");
    ferros_sys::pin(ferros_sys::CORES_WORKERS);
    ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
    stages(&mut l, b"A715");
    ferros_sys::exit(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { GRANTS = grants };
    let stack = unsafe { &mut (*(&raw mut STACK)).0 };
    ferros_sys::thread_spawn(work, stack, 0);
    ferros_sys::thread_exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::exit(9)
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
