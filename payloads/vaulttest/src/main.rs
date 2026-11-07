//! The vault process's proof on the device (VAULT-OUT.md): a process with the `test` store scope, using the store calls every process uses, which the kernel forwards to the vault process. It writes a small value and a 6 MiB one (past the kernel's old 3 MiB limit) in 4 KiB transfers, reads both back (the large one checked by its BLAKE3), lists the names, deletes one and finds it gone. With input 01 it only reads the large value back, for after the vault process is restarted.
//!
//! Grant slots: 0 console, 1 store test.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use ferros_sys::{cap, console, freq, now};

const HEAP_BYTES: usize = 16 << 20;

#[repr(C, align(16))]
struct Region<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Region<N> {}
static HEAP: Region<HEAP_BYTES> = Region(UnsafeCell::new([0; HEAP_BYTES]));

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
    main = sym test_main
);

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

const S_STORE: u64 = 1;
/// The large value's size: 6 MiB.
const BIG: usize = 6 << 20;
static mut GRANTS: u64 = 0;

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
        console(unsafe { GRANTS }, &l.b[..l.n]);
    }};
}

fn store() -> u64 {
    cap(unsafe { GRANTS }, S_STORE)
}

/// The whole value written in pieces of up to `chunk` bytes, as any process writes a large one.
fn put(name: &[u8], v: &[u8], chunk: usize) -> bool {
    if v.len() <= chunk {
        return ferros_sys::store_set(store(), name, v);
    }
    let mut off = 0;
    while off < v.len() {
        let end = (off + chunk).min(v.len());
        if !ferros_sys::store_set_at(store(), name, off, &v[off..end], end < v.len()) {
            return false;
        }
        off = end;
    }
    true
}

/// The whole value read back in 4 KiB transfers; None if it is absent or a read failed.
fn get(name: &[u8]) -> Option<Vec<u8>> {
    let mut r = alloc::vec![0u8; 4096];
    let whole = ferros_sys::store_get_at(store(), name, 0, &mut r)?;
    let mut v = Vec::with_capacity(whole);
    v.extend_from_slice(&r[..whole.min(r.len())]);
    while v.len() < whole {
        let n = ferros_sys::store_get_at(store(), name, v.len(), &mut r)?;
        if n != whole {
            return None;
        }
        let take = (whole - v.len()).min(r.len());
        v.extend_from_slice(&r[..take]);
    }
    Some(v)
}

/// The scope's names, sorted.
fn names() -> Vec<Vec<u8>> {
    let mut r = alloc::vec![0u8; 4096];
    let Some(n) = ferros_sys::store_keys(store(), 0, &mut r) else { return Vec::new() };
    let mut out: Vec<Vec<u8>> = r[..n.min(r.len())].split(|&b| b == b'\n').filter(|s| !s.is_empty()).map(|s| s.to_vec()).collect();
    out.sort();
    out
}

fn ms(t: u64) -> u64 {
    (now() - t) * 1000 / freq()
}

#[unsafe(no_mangle)]
pub extern "C" fn test_main(g: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = g };
    // Past the kernel's old 3 MiB limit on a value (service.rs STORE_VALUE_MAX).
    let big: Vec<u8> = (0..BIG as u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let want = blake3::hash(&big);
    // Input 01: only look, after the vault process was restarted: the value the last run left is still there, whole.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 1 {
        let t = now();
        let back = get(b"big");
        let same = back.as_ref().is_some_and(|b| blake3::hash(b) == want);
        let listed = names() == [b"big".to_vec()];
        say!("vaulttest: after a restart, {} MiB read back {} in {} ms, the same by BLAKE3: {}, names {}", BIG >> 20, back.is_some(), ms(t), same, listed);
        ferros_sys::exit(if same && listed { 0 } else { 1 })
    }
    let t = now();
    let small_ok = put(b"small", b"hello vault", 4096) && get(b"small").as_deref() == Some(&b"hello vault"[..]);
    say!("vaulttest: small value written and read back: {} ({} ms)", small_ok, ms(t));
    let t = now();
    let wrote = put(b"big", &big, 4096);
    let tw = ms(t);
    let t = now();
    let back = get(b"big");
    let tr = ms(t);
    let same = back.as_ref().is_some_and(|b| blake3::hash(b) == want);
    say!("vaulttest: {} MiB written {} in {} ms, read back {} in {} ms, the same by BLAKE3: {}", BIG >> 20, wrote, tw, back.is_some(), tr, same);
    let listed = names() == [b"big".to_vec(), b"small".to_vec()];
    let deleted = ferros_sys::store_delete(store(), b"small") == Some(true);
    let again = ferros_sys::store_delete(store(), b"small") == Some(false);
    let after = names() == [b"big".to_vec()];
    let absent = get(b"small").is_none();
    say!("vaulttest: names listed {}, small deleted {} (and gone: {} {}), names after {}", listed, deleted, again, absent, after);
    let all = small_ok && wrote && same && listed && deleted && again && after && absent;
    say!("vaulttest: {}", if all { "all passed" } else { "FAILED" });
    ferros_sys::exit(if all { 0 } else { 1 })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("vaulttest: panic: {}", info);
    ferros_sys::exit(1)
}
