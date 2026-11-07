//! The vault process's proof on the device (VAULT-OUT.md, step 1): a client of `vault` that, in test domain 1, writes a small value and a 1 MiB one in 4 KiB messages, reads both back (the large one checked by its BLAKE3), lists the domain's names, deletes one, sees that domain 2 finds nothing, and sweeps; each step's result and time on the console. With input 01 it only reads the large value back, for after the vault process is restarted.
//!
//! Grant slots: 0 console, 1 use `vault`.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use ferros_sys::{call, cap, console, freq, now, ERR};

const HEAP_BYTES: usize = 4 << 20;

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

const S_VAULT: u64 = 1;
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

/// One request to the vault: [op][domain][name length][name][rest]; returns the reply's length into `r`.
fn ask(op: u8, dom: u8, name: &[u8], rest: &[u8], r: &mut [u8]) -> usize {
    let mut m = Vec::with_capacity(3 + name.len() + rest.len());
    m.extend_from_slice(&[op, dom, name.len() as u8]);
    m.extend_from_slice(name);
    m.extend_from_slice(rest);
    let n = call(cap(unsafe { GRANTS }, S_VAULT), &m, r);
    if n == ERR { 0 } else { n as usize }
}

/// The whole value written in messages of up to `chunk` bytes; whether every one was taken.
fn put(dom: u8, name: &[u8], v: &[u8], chunk: usize) -> bool {
    let mut r = [0u8; 16];
    let mut off = 0;
    loop {
        let end = (off + chunk).min(v.len());
        let more = end < v.len();
        let mut rest = Vec::with_capacity(9 + end - off);
        rest.extend_from_slice(&(off as u64).to_le_bytes());
        rest.push(more as u8);
        rest.extend_from_slice(&v[off..end]);
        if ask(0, dom, name, &rest, &mut r) != 1 || r[0] != 0 {
            return false;
        }
        off = end;
        if !more {
            return true;
        }
    }
}

/// The whole value read back; None if it is absent or a read failed.
fn get(dom: u8, name: &[u8]) -> Option<Vec<u8>> {
    let mut r = alloc::vec![0u8; 4096];
    let mut v = Vec::new();
    loop {
        let n = ask(1, dom, name, &(v.len() as u64).to_le_bytes(), &mut r);
        if n < 9 || r[0] != 0 {
            return None;
        }
        let whole = u64::from_le_bytes(r[1..9].try_into().ok()?) as usize;
        if v.is_empty() {
            v.reserve_exact(whole);
        }
        v.extend_from_slice(&r[9..n]);
        if v.len() >= whole {
            return Some(v);
        }
        if n == 9 {
            return None;
        }
    }
}

fn names(dom: u8) -> Vec<Vec<u8>> {
    let mut r = alloc::vec![0u8; 4096];
    let n = ask(3, dom, b"", &0u32.to_le_bytes(), &mut r);
    let mut out = Vec::new();
    if n < 5 || r[0] != 0 {
        return out;
    }
    let mut at = 5;
    while at < n {
        let l = r[at] as usize;
        out.push(r[at + 1..at + 1 + l].to_vec());
        at += 1 + l;
    }
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
    let big: Vec<u8> = (0..1u32 << 20).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let want = blake3::hash(&big);
    // Input 01: only look, after the vault process was restarted: the value the last run left is still there, whole.
    if input_len > 0 && unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } == 1 {
        let t = now();
        let back = get(1, b"big");
        let same = back.as_ref().is_some_and(|b| blake3::hash(b) == want);
        let listed = names(1) == [b"big".to_vec()];
        say!("vaulttest: after a restart, 1 MiB read back {} in {} ms, the same by BLAKE3: {}, names {}", back.is_some(), ms(t), same, listed);
        ferros_sys::exit(if same && listed { 0 } else { 1 })
    }
    let t = now();
    let small_ok = put(1, b"small", b"hello vault", 4000) && get(1, b"small").as_deref() == Some(&b"hello vault"[..]);
    say!("vaulttest: small value written and read back: {} ({} ms)", small_ok, ms(t));
    let t = now();
    let wrote = put(1, b"big", &big, 4000);
    let tw = ms(t);
    let t = now();
    let back = get(1, b"big");
    let tr = ms(t);
    let same = back.as_ref().is_some_and(|b| blake3::hash(b) == want);
    say!("vaulttest: 1 MiB written {} in {} ms, read back {} in {} ms, the same by BLAKE3: {}", wrote, tw, back.is_some(), tr, same);
    let mut n = names(1);
    n.sort();
    let listed = n == [b"big".to_vec(), b"small".to_vec()];
    let mut r = [0u8; 16];
    let deleted = ask(2, 1, b"small", b"", &mut r) == 2 && r[0] == 0 && r[1] == 1;
    let after = names(1) == [b"big".to_vec()];
    say!("vaulttest: names listed {}, small deleted {}, names after {}", listed, deleted, after);
    let other = ask(1, 2, b"big", &0u64.to_le_bytes(), &mut r) == 1 && r[0] == 1;
    let mut s = [0u8; 16];
    let swept = ask(4, 1, b"", b"", &mut s) == 9 && s[0] == 0;
    say!("vaulttest: domain 2 finds nothing {}, sweep ran {} ({} pieces)", other, swept, u64::from_le_bytes(s[1..9].try_into().unwrap_or([0; 8])));
    let all = small_ok && wrote && same && listed && deleted && after && other && swept;
    say!("vaulttest: {}", if all { "all passed" } else { "FAILED" });
    ferros_sys::exit(if all { 0 } else { 1 })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("vaulttest: panic: {}", info);
    ferros_sys::exit(1)
}
