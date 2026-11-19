//! The kernel, as this process reaches it (KERNEL.md, the process ABI): the grants page, a call on an endpoint, an ira key.
//! Inlined as Photon's, Mahere's and kete's are: ferros_sys is no_std and not a published crate.

use core::arch::asm;

/// The kernel's refusal (ferros_sys::ERR).
pub const ERR: u64 = u64::MAX;
const CALL: u64 = 1;
const FACTS: u64 = 12;
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

const INPUT: u64 = 8;

const PIN: u64 = 20;

/// The cores this thread may run on from now (husky: the A510s G#000F, the A715 workers G#00F0, the X3 G#0100); threads it spawns later inherit it. A mask whose cores are not up falls back to any core.
pub fn pin(mask: u64) -> u64 {
    svc(PIN, mask, 0, 0, 0, 0)
}

/// The spawn input's bytes from `start` into `buf` (ferros_sys `input`): how many were copied, 0 past the end or for a process spawned with none (a kist opened from the map).
pub fn input(start: u64, buf: &mut [u8]) -> usize {
    svc(INPUT, start, off(buf.as_mut_ptr() as *const u8), buf.len() as u64, 0, 0) as usize
}

/// The gravity vector the AoC last reported (ferros_sys `Facts::gravity`: facts 12 and 13, the x and y halves of one word, then z), m/s² in the panel's frame, x right, y up, z out of the glass, as Android's gravity sensor gives it; None until the sensors are up, or from a kernel that publishes no gravity (an unknown fact reads all ones). The facts need no grant: the kernel answers them from the sensors process's block.
pub fn gravity() -> Option<[f32; 3]> {
    // The facts up to gravity's, all ones (unknown) where the kernel writes none.
    let mut b = [0xFFu8; 14 * 8];
    // Thru a mutable pointer: the kernel writes the buffer.
    svc(FACTS, off(b.as_mut_ptr()), b.len() as u64, 0, 0, 0);
    let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    let (xy, z) = (word(12), word(13));
    if xy == u64::MAX || z == u64::MAX {
        return None;
    }
    Some([f32::from_bits(xy as u32), f32::from_bits((xy >> 32) as u32), f32::from_bits(z as u32)])
}

/// Which board this is (fact 84, as ferros_sys `Facts::board`): 1 husky (the Pixel 8 Pro), 2 akita (the Pixel 8a); None from a kernel that does not say.
pub fn board() -> Option<u64> {
    let mut b = [0xFFu8; 85 * 8];
    // Thru a mutable pointer: the kernel writes the buffer.
    svc(FACTS, off(b.as_mut_ptr()), b.len() as u64, 0, 0, 0);
    let v = u64::from_le_bytes(b[84 * 8..85 * 8].try_into().unwrap());
    (v != u64::MAX).then_some(v)
}
