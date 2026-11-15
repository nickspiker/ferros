//! The kernel, as this process reaches it (KERNEL.md, the process ABI): the grants page, a call on an endpoint, an ira key.
//! Inlined as Photon's, Mahere's and kete's are: ferros_sys is no_std and not a published crate.

use core::arch::asm;

/// The kernel's refusal (ferros_sys::ERR).
pub const ERR: u64 = u64::MAX;
const CALL: u64 = 1;
const IRA: u64 = 23;
/// Bit 63 of a call's reply length: fail at once when no server is receiving on the endpoint, instead of waiting for one that may never come; an on-demand endpoint (`camera`) waits for the server the kernel is starting (KERNEL.md "CALL_LIVE").
const CALL_LIVE: u64 = 1 << 63;

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

/// Whether `slot` holds a grant: a slot past the count, or an empty one (a right the person said no to: its token is zeros, process.rs `Object::Empty`), holds none.
pub fn held(slot: u64) -> bool {
    if slot >= grant_count() {
        return false;
    }
    let t = unsafe { core::slice::from_raw_parts((arc_base() + cap(slot)) as *const u8, 32) };
    t.iter().any(|&b| b != 0)
}

/// A call on the endpoint in `slot` that fails at once when nobody serves it: the reply's length, or ERR.
pub fn call_live(slot: u64, msg: &[u8], reply: &mut [u8]) -> u64 {
    svc(CALL, cap(slot), off(msg.as_ptr()), msg.len() as u64, off(reply.as_mut_ptr()), reply.len() as u64 | CALL_LIVE)
}

/// A 32-byte key for `domain` rooted in the device's ira (ferros_sys `ira_key`): the same on every boot, never the ira itself, and for a kist bound to its gate identity, so no other kist derives it (KISTS.md, "A kist's ira keys are its own"). None without the ira this boot.
pub fn ira_key(slot: u64, domain: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let r = svc(IRA, cap(slot), off(domain.as_ptr()), domain.len() as u64, off(out.as_mut_ptr()), 0);
    (r == 32).then_some(out)
}

const DMA: u64 = 7;

/// The base of a region this process holds a token for, the token in the process's own memory (the camera's view: docs/CAMERA.md "The view"); ERR once it is taken back.
pub fn dma(token: &[u8; 32]) -> u64 {
    svc(DMA, off(token.as_ptr()), 0, 0, 0, 0)
}
