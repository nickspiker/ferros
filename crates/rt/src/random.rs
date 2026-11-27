//! Entropy for std (`HashMap`'s seeds and whatever asks `std::random`): the kernel's ENTROPY syscall through an Entropy capability when the process was spawned with one (`--entropy`; the entry finds the slot by trying each grant), else a counter-seeded mixer that is NOT cryptographic — a process that needs real entropy is spawned with the capability, and a crypto library on ferros asks the kernel directly.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::sys::{self, ERR, nr, off};

static SLOT: AtomicU64 = AtomicU64::new(u64::MAX);
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Find the Entropy grant among the process's `count` grants (the kernel answers ERR to the syscall on any other kind).
pub fn detect(count: u64) {
    let mut probe = [0u8; 8];
    for slot in 1..count.min(64) {
        if sys::svc(nr::ENTROPY, crate::cap(slot), off(probe.as_mut_ptr()), 8, 0, 0) != ERR {
            SLOT.store(slot, Ordering::Relaxed);
            return;
        }
    }
}

/// Whether the kernel's entropy is behind `fill_bytes`.
pub fn is_hardware() -> bool {
    SLOT.load(Ordering::Relaxed) != u64::MAX
}

pub fn fill_bytes(bytes: &mut [u8]) {
    let slot = SLOT.load(Ordering::Relaxed);
    if slot != u64::MAX {
        let mut ok = true;
        for chunk in bytes.chunks_mut(4096) {
            if sys::svc(nr::ENTROPY, crate::cap(slot), off(chunk.as_mut_ptr()), chunk.len() as u64, 0, 0) == ERR {
                ok = false;
                break;
            }
        }
        if ok {
            return;
        }
    }
    // splitmix64 over the counter, the call count and the buffer's address: unpredictable enough for a hash seed, nothing more.
    let mut x = sys::now() ^ (bytes.as_ptr() as u64).rotate_left(32) ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for chunk in bytes.chunks_mut(8) {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
}
