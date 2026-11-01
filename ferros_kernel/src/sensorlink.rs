//! The sensors in a process (KERNEL-OUT.md move 4): the USF conversation with the AoC — opening the channel, the registry, the subscriptions, the samples, the light scope — is `payloads/sensors`'s, and the kernel has none of its own since 2026-11-01 (usf.rs gone). The kernel keeps the AoC's start (its rails, the firmware through GSA and Trusty: secure-world calls), behind the `sensors` power switch: on asks for it, and the service loop runs it a step a pass (service.rs `SensorBoot`) while the process waits for the AoC to come online (decision 2, owner, 2026-11-01: no FF-A door for EL0).
//!
//! The process publishes one block in its own memory (`SYS_SENSORS` op 0): the samples the kernel answers `SYS_FACTS` and `SYS_LIGHT` with, and proximity for the power key's wake rule. The kernel reads it in place, under the kernel lock, and forgets it the moment the process ends (`end_process`, `kill`, and the service loop's check). With no block every answer is "unknown" and the wake rule fails open. The layout is `ferros_sys::sensor_block`'s; the indices are repeated here.
//!
//! Also the board's identity from ABL's device tree (`read_board`), which the registry scripts filter on (`SYS_SENSORS` op 1).
//!
//! Every function here runs with the kernel lock held unless it says otherwise.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The `sensors` switch asked for the AoC: the service loop starts it (once the boot's first seconds have passed) unless it is up already.
pub static START: AtomicBool = AtomicBool::new(false);

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

/// The process with `epoch` has ended: its block is gone with it.
pub fn ended(epoch: u64) {
    unsafe {
        if BLOCK.0 == epoch {
            BLOCK = (0, 0);
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

/// The light scope's `n` samples before sample number `end`, oldest first, little-endian i32, from the process's ring.
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

/// The `sensors` power switch (process.rs POWERS). On: the AoC is started if it is not up (asked here, run by the service loop: its firmware takes some 1.4 s out of the vault, too long for a syscall); the process waits for it online. Off: nothing to undo — the AoC keeps running, as it did when the kernel held the conversation.
pub fn power(on: bool) -> bool {
    if on {
        START.store(true, Ordering::Relaxed);
    }
    true
}

/// Forget the block outright: the service loop's check found no sensors process running, whatever path ended it, so the block is never read again (its memory may already be another process's).
pub fn forget() {
    unsafe { BLOCK = (0, 0) };
}

// ── The board's identity for the registry scripts ──
/// The board's CDT (product << 16 | stage << 12 | major << 8 | minor << 4 | variant) as ABL's device tree gives it in /chosen/plat, read at entry (`read_board`); 0 when absent. The registry scripts filter their sections on it (`?+0x20000:0xFFFF0000`, husky_dvt excludes proto and EVT stages); with 0 the AoC skips husky_common and husky_dvt and no sensor appears.
pub static CDT: AtomicU32 = AtomicU32::new(0);
/// The board's MLB number from /chosen/config/pcbcfg ("MLB-<n>"); 0 when absent.
pub static MLB: AtomicU32 = AtomicU32::new(0);

/// Read /chosen/plat {product, stage, major, minor, variant} (big-endian cells) and /chosen/config pcbcfg out of the FDT at `addr`, as libusf's UsfRegistryStoreFile::GetDevCdtInfo does from /sys/firmware/devicetree. A walk of the structure block tracking the node path.
pub fn read_board(addr: u64) {
    let be = |p: usize| unsafe { u32::from_be(core::ptr::read_unaligned(p as *const u32)) };
    let base = addr as usize;
    if be(base) != 0xD00D_FEED {
        return;
    }
    let size = be(base + 4) as usize;
    let (st, strs) = (base + be(base + 8) as usize, base + be(base + 12) as usize);
    let end = base + size.min(4 << 20);
    let name_at = |p: usize| -> &'static [u8] {
        let mut n = 0;
        while p + n < end && unsafe { *((p + n) as *const u8) } != 0 {
            n += 1;
        }
        unsafe { core::slice::from_raw_parts(p as *const u8, n) }
    };
    // The path as node names, depth 0 = root.
    let mut path: [&[u8]; 8] = [b""; 8];
    let mut depth = 0usize;
    let mut cells = [0u32; 5];
    let mut pos = st;
    while pos + 4 <= end {
        let tok = be(pos);
        pos += 4;
        match tok {
            1 => {
                let n = name_at(pos);
                if depth < 8 {
                    path[depth] = n;
                }
                depth += 1;
                pos = (pos + n.len() + 1 + 3) & !3;
            }
            2 => depth = depth.saturating_sub(1),
            3 => {
                let (len, off) = (be(pos) as usize, be(pos + 4) as usize);
                let data = pos + 8;
                let pname = name_at(strs + off);
                if depth == 3 && path[1] == b"chosen" && path[2] == b"plat" && len == 4 {
                    let k = [b"product".as_slice(), b"stage", b"major", b"minor", b"variant"].iter().position(|&k| k == pname);
                    if let Some(k) = k {
                        cells[k] = be(data);
                    }
                }
                if depth == 3 && path[1] == b"chosen" && path[2] == b"config" && pname == b"pcbcfg" {
                    let v = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
                    if let Some(rest) = v.strip_prefix(b"MLB-") {
                        let n = rest.iter().take_while(|c| c.is_ascii_digit()).fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
                        MLB.store(n, Ordering::Relaxed);
                    }
                }
                pos = (data + len + 3) & !3;
            }
            4 => {}
            _ => break,
        }
    }
    let [product, stage, major, minor, variant] = cells;
    CDT.store(product << 16 | (stage & 15) << 12 | (major & 15) << 8 | (minor & 15) << 4 | (variant & 15), Ordering::Relaxed);
}
