//! The sensors' protocol in a process (KERNEL-OUT.md move 4, phase 1): the USF conversation with the AoC — opening the channel, the registry, the subscriptions, the samples, the light scope — runs in `payloads/sensors`. The kernel keeps the AoC's start (its rails, the firmware through GSA and Trusty: secure-world calls, phase 2) and reads what the process publishes.
//!
//! The `sensors` power switch hands the conversation over: on, the kernel stops polling USF and skips the steps after the AoC is online; off, or the process gone, it takes the conversation back (re-opens the channel and subscribes, as after a warm reload).
//!
//! The process publishes one block in its own memory (`SYS_SENSORS` op 0): the samples the kernel answers `SYS_FACTS` and `SYS_LIGHT` with, and proximity for the power key's wake rule. The kernel reads it in place, under the kernel lock, and forgets it the moment the process ends (`end_process`). The layout is `ferros_sys::sensor_block`'s; the indices are repeated here.
//!
//! Every function here runs with the kernel lock held unless it says otherwise.

use core::sync::atomic::{AtomicBool, Ordering};

/// The process holds the conversation (between `power sensors on` and `off`, or its end).
pub static PROCESS: AtomicBool = AtomicBool::new(false);
/// Set when the conversation comes back to the kernel: the service loop opens the channel again and subscribes.
pub static TAKE_BACK: AtomicBool = AtomicBool::new(false);

/// The block's words (ferros_sys `sensor_block`): a magic first, then the gravity vector, its last two samples with times, the spectral sensor, proximity, the light scope's format, sample count and channel mask, and from word 32 the scope's ring of i32 samples.
pub const MAGIC: u64 = 0x5345_4E53_4F52_5301;
pub const GRAVITY_XY: usize = 1;
pub const GRAVITY_Z: usize = 2;
pub const PAIR: usize = 3;
pub const SPECTRAL: usize = 9;
pub const PROX: usize = 16;
pub const SCOPE_FMT: usize = 19;
pub const SCOPE_N: usize = 20;
pub const SCOPE_MASK: usize = 21;
pub const SCOPE: usize = 32;
pub const SCOPE_LEN: usize = 65536;
/// The block's size in bytes: the header words and the ring.
pub const BYTES: usize = SCOPE * 8 + SCOPE_LEN * 4;

/// The published block: the process's epoch and the block's address (0: none).
static mut BLOCK: (u64, usize) = (0, 0);

/// `SYS_SENSORS` op 0: the block at `addr` (already checked to lie in the caller's arc) is the sensors' from now on.
pub fn publish(epoch: u64, addr: usize) {
    unsafe { BLOCK = (epoch, addr) };
}

/// The process with `epoch` has ended: its block is gone with it, and the conversation comes back to the kernel.
pub fn ended(epoch: u64) {
    unsafe {
        if BLOCK.0 == epoch && BLOCK.1 != 0 {
            BLOCK = (0, 0);
            if PROCESS.load(Ordering::Relaxed) {
                PROCESS.store(false, Ordering::Relaxed);
                TAKE_BACK.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Word `i` of the block, None while no block is published or its magic is not yet written.
pub fn word(i: usize) -> Option<u64> {
    let (_, a) = unsafe { BLOCK };
    if a == 0 {
        return None;
    }
    let w = |k: usize| unsafe { core::ptr::read_volatile((a + 8 * k) as *const u64) };
    (w(0) == MAGIC).then(|| w(i))
}

/// Whether a block is published and live.
pub fn live() -> bool {
    word(0).is_some()
}

/// The light scope's `n` samples before sample number `end`, oldest first, little-endian i32, from the process's ring (as usf.rs `scope_copy`).
pub fn scope_copy(end: u64, n: usize, out: &mut alloc::vec::Vec<u8>) {
    let (_, a) = unsafe { BLOCK };
    let ring = a + 8 * SCOPE;
    for i in 0..n as u64 {
        let k = (end - n as u64 + i) % SCOPE_LEN as u64;
        out.extend_from_slice(&unsafe { core::ptr::read_volatile((ring + 4 * k as usize) as *const i32) }.to_le_bytes());
    }
}

/// Proximity near (Android's distance below its maximum), from the process's block; None before its first sample. Takes the kernel lock: the power key asks from the service loop.
pub fn prox_near() -> Option<bool> {
    crate::process::with_lock(|| {
        if word(PROX + 2)? >> 32 == 0 {
            return None;
        }
        Some(f32::from_bits(word(PROX)? as u32) < 5.0)
    })
}

/// The `sensors` power switch (process.rs POWERS). On: the process holds the conversation from now on; the kernel stops polling USF. Off: the kernel takes it back on the service loop's next pass.
pub fn power(on: bool) -> bool {
    if on {
        PROCESS.store(true, Ordering::Relaxed);
        crate::flashlog::line("sensors: the USF conversation goes to the sensors process");
    } else if PROCESS.load(Ordering::Relaxed) {
        PROCESS.store(false, Ordering::Relaxed);
        TAKE_BACK.store(true, Ordering::Relaxed);
        crate::flashlog::line("sensors: the USF conversation comes back to the kernel");
    }
    true
}

/// Forget the block outright: the service loop's check found no sensors process running, whatever path ended it, so the block is never read again (its memory may already be another process's).
pub fn forget() {
    unsafe { BLOCK = (0, 0) };
}
