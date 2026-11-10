//! The vault process's proof on the device (VAULT-OUT.md): a process with the `test` store scope, using the store calls every process uses, which the kernel forwards to the vault process. It writes a small value and a 6 MiB one (past the kernel's old 3 MiB limit) in 4 KiB transfers, reads both back (the large one checked by its BLAKE3), lists the names, deletes one and finds it gone. With input 01 it only reads the large value back, for after the vault process is restarted. With input 02 it streams one 576 MiB value in and back, compared byte for byte, (the tract's grow past a blank vault's first 512 MiB) and deletes it.
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
    // The mode byte first: `arc_base` reads x28, which this function's own code may take for itself further down (the grow mode never ran after a rebuild, 2026-11-08).
    let mode = if input_len > 0 { unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + input) as *const u8) } } else { 0 };
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init(HEAP.0.get() as usize, HEAP_BYTES) };
    unsafe { GRANTS = g };
    // Input 02: the grow (manifestus grow_from, VAULT-OUT.md): one value larger than a blank vault's first 512 MiB a mirror, streamed in, read back and compared byte for byte (the pattern regenerated a transfer at a time; blake3's incremental Hasher faulted in this payload, 2026-11-08) and deleted, so the tract must grow into its half of the window.
    if mode == 2 {
        grow();
    }
    // Input 03: the wallpaper's read, timed (owner, 2026-11-09: "why the wallpaper still takes about 1s to show"): a value the wallpaper's size written and read back in the display's transfers.
    if mode == 3 || mode == 4 {
        wall(mode == 4);
    }
    // Past the kernel's old 3 MiB limit on a value (service.rs STORE_VALUE_MAX).
    let big: Vec<u8> = (0..BIG as u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let want = blake3::hash(&big);
    // Input 01: only look, after the vault process was restarted: the value the last run left is still there, whole.
    if mode == 1 {
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

/// The grow mode's value: 576 MiB, past a blank vault's first 512 MiB a mirror (and its 9216 pieces' entries within the vault's 24 MiB heap as manifestus doubles its buffer of them).
/// On akita (2026-11-08, a blank vault): the vault process ran out of its 24 MiB heap at 384 MiB written (an allocation of 10.7 MB failed: manifestus's buffer of the vault's entries, doubling), so today this mode stops there and the vault restarts; it passes once the vault bounds that buffer.
const GROW: usize = 576 << 20;

/// The grow mode's byte at `i`, the large value's pattern (generated a transfer at a time: the value does not fit the heap).
fn grow_fill(off: usize, buf: &mut [u8]) {
    for (k, b) in buf.iter_mut().enumerate() {
        *b = (((off + k) as u32).wrapping_mul(2654435761) >> 13) as u8;
    }
}

fn grow() -> ! {
    let mut buf = alloc::vec![0u8; 4096];
    let mut want = alloc::vec![0u8; 4096];
    let t = now();
    let mut off = 0;
    let mut wrote = true;
    while off < GROW {
        grow_fill(off, &mut buf);
        if !ferros_sys::store_set_at(store(), b"grow", off, &buf, off + buf.len() < GROW) {
            wrote = false;
            break;
        }
        off += buf.len();
    }
    say!("vaulttest: grow: {} MiB written {} in {} ms (stopped at {} MiB)", GROW >> 20, wrote, ms(t), off >> 20);
    let t = now();
    let mut off = 0;
    let mut read = true;
    let mut same = true;
    while read && off < GROW {
        match ferros_sys::store_get_at(store(), b"grow", off, &mut buf) {
            Some(whole) if whole == GROW => {
                grow_fill(off, &mut want);
                same &= buf == want;
                off += buf.len();
            }
            _ => read = false,
        }
    }
    let same = read && same;
    say!("vaulttest: grow: read back {} in {} ms, every byte as written: {}", read, ms(t), same);
    let deleted = ferros_sys::store_delete(store(), b"grow") == Some(true);
    say!("vaulttest: grow: deleted {}; {}", deleted, if wrote && same && deleted { "passed" } else { "FAILED" });
    ferros_sys::exit(if wrote && same && deleted { 0 } else { 1 })
}

/// The wallpaper read's size: the Pro's screen, three planes of 16-bit samples (24,127,488 bytes; the made wallpaper's document is 24,128,145).
const WALL: usize = 6 * 1344 * 2992;

/// The value the size of a wallpaper written in the display's transfers (ferros_sys::STORE_MAX), then read back the way the display reads it twice, timed: the first read after the write and a second, so what a cache holds shows. The vault process says each read's stages (its read report).
/// Input 04 reads the value input 03 left (and deletes it), so the reads' own block calls can be counted apart from the write's.
fn wall(read_only: bool) -> ! {
    let mut buf = alloc::vec![0u8; ferros_sys::STORE_MAX];
    let mut want = alloc::vec![0u8; ferros_sys::STORE_MAX];
    let t = now();
    let mut off = if read_only { WALL } else { 0 };
    let mut wrote = true;
    while off < WALL {
        let n = (WALL - off).min(buf.len());
        grow_fill(off, &mut buf[..n]);
        if !ferros_sys::store_set_at(store(), b"wall", off, &buf[..n], off + n < WALL) {
            wrote = false;
            break;
        }
        off += n;
    }
    say!("vaulttest: wall: {} B written {} in {} ms", WALL, wrote, ms(t));
    let mut same = wrote;
    for pass in 0..2 {
        let t = now();
        let mut off = 0;
        while same && off < WALL {
            let n = (WALL - off).min(buf.len());
            match ferros_sys::store_get_at(store(), b"wall", off, &mut buf[..n]) {
                Some(whole) if whole == WALL => {
                    grow_fill(off, &mut want[..n]);
                    same &= buf[..n] == want[..n];
                    off += n;
                }
                _ => same = false,
            }
        }
        let us = (now() - t) * 1_000_000 / freq();
        say!("vaulttest: wall: read {} in {} ms ({} MB/s), pass {}, as written: {}", WALL, us / 1000, WALL as u64 / us, pass, same);
    }
    if !read_only {
        say!("vaulttest: wall: kept for input 04");
        ferros_sys::exit(if same { 0 } else { 1 })
    }
    let deleted = ferros_sys::store_delete(store(), b"wall") == Some(true);
    say!("vaulttest: wall: deleted {}; {}", deleted, if same && deleted { "passed" } else { "FAILED" });
    ferros_sys::exit(if same && deleted { 0 } else { 1 })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("vaulttest: panic: {}", info);
    ferros_sys::exit(1)
}
