//! The raw machine: the `svc` trap, the arc registers, the counter, the image's relocations (ferros_sys says the same things for a no_std payload; this copy keeps std's dependency at `core` alone).

use core::arch::asm;
use core::sync::atomic::{Ordering, compiler_fence};

/// A syscall failed: unknown or wrong token, out-of-range offset, no such object.
pub const ERR: u64 = u64::MAX;

/// Syscall numbers (x8), KERNEL.md.
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const CALL: u64 = 1;
    pub const RECV: u64 = 2;
    pub const REPLY: u64 = 3;
    pub const ENTROPY: u64 = 6;
    pub const INPUT: u64 = 8;
    pub const SLEEP: u64 = 9;
    pub const FACTS: u64 = 12;
    pub const STORE: u64 = 13;
    pub const THREAD_SPAWN: u64 = 14;
    pub const THREAD_EXIT: u64 = 15;
    pub const WAIT: u64 = 16;
    pub const WAKE: u64 = 17;
    pub const PIN: u64 = 20;
}

/// The trap: number in x8, arguments in x0..x4, result in x0. The kernel may write into any buffer the arguments name, so nothing is assumed cached across it.
#[inline(always)]
pub fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    compiler_fence(Ordering::SeqCst);
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    compiler_fence(Ordering::SeqCst);
    r
}

/// The process's arc base (x28, set by the kernel at entry and never written by compiled code: the target reserves it).
#[inline(always)]
pub fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

/// The arc's mask (x27): its size − 1.
#[inline(always)]
pub fn arc_mask() -> u64 {
    let m: u64;
    unsafe { asm!("mov {}, x27", out(reg) m, options(nomem, nostack, preserves_flags)) };
    m
}

/// A pointer as the arc offset the kernel wants.
#[inline(always)]
pub fn off<T: ?Sized>(p: *const T) -> u64 {
    (p as *const u8 as u64).wrapping_sub(arc_base())
}

/// The generic timer count (EL0 may read it: CNTHCTL_EL2.EL0VCTEN).
#[inline(always)]
pub fn now() -> u64 {
    let t: u64;
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) t, options(nomem, nostack, preserves_flags)) };
    t
}

/// The counter's rate, Hz.
#[inline(always)]
pub fn freq() -> u64 {
    let f: u64;
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    f
}

unsafe extern "C" {
    static __image_start: u8;
    static __rela_start: u8;
    static __rela_end: u8;
}

/// The address of a linker-script symbol by PC-relative `adrp`, never the GOT: under `relocation-model = pic` rustc reaches an extern static through a GOT entry, and the GOT entries are exactly what `relocate()` has yet to fix (the first std hello faulted there, 2026-10-27).
macro_rules! pc_rel {
    ($sym:path) => {{
        let a: usize;
        unsafe { asm!("adrp {a}, {s}", "add {a}, {a}, :lo12:{s}", a = out(reg) a, s = sym $sym, options(nomem, nostack, preserves_flags)) };
        a
    }};
}

/// The image's load address.
#[inline(always)]
pub fn image_base() -> usize {
    pc_rel!(__image_start)
}

/// Apply the image's own relocations (ferros.ld links `-pie`: every absolute address in data is an R_AARCH64_RELATIVE entry in `.rela.dyn`, and this adds the load address to each). The first thing the entry does; the loop uses only PC-relative addressing.
///
/// # Safety
/// Exactly once, before any other code of the image.
#[inline(never)]
pub unsafe fn relocate() {
    const R_AARCH64_RELATIVE: u64 = 1027;
    unsafe {
        let base = image_base() as u64;
        let mut p = pc_rel!(__rela_start) as *const u64;
        let end = pc_rel!(__rela_end) as *const u64;
        while p < end {
            let (offset, info, addend) = (p.read(), p.add(1).read(), p.add(2).read());
            if info & 0xFFFF_FFFF == R_AARCH64_RELATIVE {
                ((base + offset) as *mut u64).write(base.wrapping_add(addend));
            }
            p = p.add(3);
        }
        asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}
