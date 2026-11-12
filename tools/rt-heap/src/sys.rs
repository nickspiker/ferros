//! A fake arc for alloc.rs on the host: one range reserved unreadable, SYS_ARC's grow (readable, zeroed frames at the top), shrink (frames past the new top unreadable again, their bytes dropped) and size, in 2 MiB frames, with the kernel's floor and ceiling. A heap that touches memory it gave back faults the test.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const ERR: u64 = u64::MAX;

pub mod nr {
    pub const ARC: u64 = 40;
}

pub const FRAME: usize = 1 << 21;

static BASE: AtomicUsize = AtomicUsize::new(0);
/// The kernel's lock, and the arc: (top, floor, ceiling).
static ARC: Mutex<(usize, usize, usize)> = Mutex::new((0, 0, 0));
pub static GROWS: AtomicUsize = AtomicUsize::new(0);
pub static SHRINKS: AtomicUsize = AtomicUsize::new(0);

unsafe fn syscall(n: usize, a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize) -> isize {
    let r: isize;
    unsafe {
        core::arch::asm!("syscall", inlateout("rax") n as isize => r, in("rdi") a0, in("rsi") a1, in("rdx") a2, in("r10") a3, in("r8") a4, in("r9") a5, lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    r
}

const MMAP: usize = 9;
const MPROTECT: usize = 10;
const MADVISE: usize = 28;
const PROT_NONE: usize = 0;
const PROT_RW: usize = 3;
const MAP_PRIVATE_ANON_NORESERVE: usize = 0x02 | 0x20 | 0x4000;
const MADV_DONTNEED: usize = 4;

fn protect(at: usize, len: usize, prot: usize) {
    let r = unsafe { syscall(MPROTECT, at, len, prot, 0, 0, 0) };
    assert_eq!(r, 0, "mprotect");
}

/// Reserve an arc of `ceiling` bytes, `floor` of them mapped (what the process was spawned with). Returns its base. Once a process.
pub fn init(floor: usize, ceiling: usize) -> usize {
    let len = ceiling + FRAME;
    let r = unsafe { syscall(MMAP, 0, len, PROT_NONE, MAP_PRIVATE_ANON_NORESERVE, usize::MAX, 0) };
    assert!(r > 0, "mmap");
    let base = (r as usize).next_multiple_of(FRAME);
    protect(base, floor, PROT_RW);
    BASE.store(base, Ordering::Relaxed);
    *ARC.lock().unwrap() = (floor, floor, ceiling);
    base
}

pub fn arc_base() -> u64 {
    BASE.load(Ordering::Relaxed) as u64
}

/// The arc's top now (an offset).
pub fn top() -> usize {
    ARC.lock().unwrap().0
}

/// SYS_ARC: op 0 grow, 1 shrink, 2 size, as the kernel answers them (process.rs `arc_op`, native mode: whole frames).
pub fn svc2(n: u64, op: u64, x: u64) -> (u64, u64) {
    assert_eq!(n, nr::ARC);
    let base = BASE.load(Ordering::Relaxed);
    let mut arc = ARC.lock().unwrap();
    let (top, floor, ceiling) = *arc;
    match op {
        0 => {
            let new = (top + x as usize).next_multiple_of(FRAME);
            if new > ceiling {
                return (ERR, 1);
            }
            protect(base + top, new - top, PROT_RW);
            arc.0 = new;
            GROWS.fetch_add(1, Ordering::Relaxed);
            (new as u64, top as u64)
        }
        1 => {
            let keep = (x as usize).next_multiple_of(FRAME);
            if keep < floor {
                return (ERR, 4);
            }
            if keep >= top {
                return (top as u64, top as u64);
            }
            let r = unsafe { syscall(MADVISE, base + keep, top - keep, MADV_DONTNEED, 0, 0, 0) };
            assert_eq!(r, 0, "madvise");
            protect(base + keep, top - keep, PROT_NONE);
            arc.0 = keep;
            SHRINKS.fetch_add(1, Ordering::Relaxed);
            (keep as u64, top as u64)
        }
        2 => (top as u64, ceiling as u64),
        _ => (ERR, 6),
    }
}
