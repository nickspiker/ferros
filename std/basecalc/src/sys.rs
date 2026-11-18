//! The kernel, as this process reaches it (KERNEL.md, the process ABI): the grants page and an ira key.
//! Inlined as Lumis's, Photon's and Mahere's are: ferros_sys is no_std and not a published crate.

use core::arch::asm;

const IRA: u64 = 23;

#[inline(always)]
fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

/// The arc base (x28, the kernel's, never written by compiled code on this target).
#[inline(always)]
fn arc_base() -> u64 {
    let b: u64;
    unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
    b
}

#[inline(always)]
fn off<T: ?Sized>(p: *const T) -> u64 {
    (p as *const u8 as u64).wrapping_sub(arc_base())
}

/// The grants page is arc offset 0: the handle table in its first 512 bytes, the u32 count at G#200, the 32-byte tokens after it (SANDBOX.md, "Handles").
pub fn grant_count() -> u64 {
    unsafe { ((arc_base() + 0x200) as *const u32).read_volatile() as u64 }
}

fn cap(slot: u64) -> u64 {
    0x204 + 32 * slot
}

/// A 32-byte key for `domain` rooted in the device's ira (ferros_sys `ira_key`): the same on every boot, never the ira itself, and for a kist bound to its gate identity, so no other kist derives it (KISTS.md, "A kist's ira keys are its own"). None without the ira this boot.
pub fn ira_key(slot: u64, domain: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(IRA, cap(slot), off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
    (r == 32).then_some(out)
}
