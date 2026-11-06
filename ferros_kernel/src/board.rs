//! Which board ferros runs on (owner, 2026-11-05: "Board split, yes plz!"): husky (Pixel 8 Pro) or akita (Pixel 8a), both Tensor G3 (zuma), told apart by the root `model` of the device tree ABL hands over ("ZUMA AKITA MP board based on ZUMA" on the 8a; docs/BOARDS.md). Read at entry, before anything board-specific runs.
//!
//! What differs and what it changes:
//! - DRAM: husky 12 GB, top at G#B_0000_0000; akita 8 GB, top at G#A_0000_0000. The hv's high RAM (its gate mark at G#A_FFFF_FF10, the jump's mailbox and entropy pool just under it) exists on husky only: on akita those reads would land past the end of DRAM.
//! - akita boots storage-off until its per-unit partitions (persist, efs, efs_backup, devinfo, mfg_data) are backed up: no UFS power-up, no flash log, no vault, no slot marks, no init (every driver process assumes husky's panel, touch and rails); its USB process starts straight from the kernel's embedded rescue capsule.
//! - The same on both (from akita's own device tree, 2026-11-05): the keys (vol down gpa4-1, vol up gpa6-2, power gpa4-0), the UFS VCC enable (gpp0-1), the eUSB repeater (hsi2c, address G#3E), the low 2 GB's carve-outs.
use core::ptr::read_volatile;
use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Board {
    Husky = 0,
    Akita = 1,
}

static BOARD: AtomicU8 = AtomicU8::new(Board::Husky as u8);

/// The board, as `detect` found it (husky until then, and on a boot with no device tree).
pub fn get() -> Board {
    if BOARD.load(Ordering::Relaxed) == Board::Akita as u8 { Board::Akita } else { Board::Husky }
}

pub fn akita() -> bool {
    get() == Board::Akita
}

/// Where the side keys sit, in panel rows (top and bottom, inclusive), measured with `display ruler`: the row beside each key's edge read off the binary ruler. The gate's marks are made from these (ready.rs).
#[derive(Clone, Copy)]
pub struct Keys {
    pub power: (usize, usize),
    /// The volume rocker: up is its upper half, down its lower half.
    pub rocker: (usize, usize),
}

pub fn keys() -> Keys {
    match get() {
        // Measured by the owner, 2026-11-06.
        Board::Akita => Keys { power: (629, 797), rocker: (932, 1271) },
        // Measured by the owner, 2026-11-06 (the hv's gate.rs carries the same).
        Board::Husky => Keys { power: (933, 1124), rocker: (1279, 1660) },
    }
}

/// Whether the hv's high RAM is there (husky's top, where its gate mark, the jump's mailbox and its entropy pool live).
pub fn hv_ram() -> bool {
    get() == Board::Husky
}

/// The board from the device tree at `dtb` (ABL's x0): akita when the root `model` says AKITA. A RELOAD's tag (an odd x0) or no tree keeps what is set.
/// MMU off, the tree in Device memory: aligned 32-bit and single-byte volatile loads only (an ordinary slice compare can become a wide unaligned load, and that faults on Device memory), and only the root node's own properties are looked at, which come before its first child.
pub fn detect(dtb: u64) {
    if dtb == 0 || dtb & 3 != 0 {
        return;
    }
    let base = dtb as usize;
    let be = |o: usize| u32::from_be(unsafe { read_volatile((base + o) as *const u32) });
    let byte = |a: usize| unsafe { read_volatile(a as *const u8) };
    // WHY the checks: x0 is whatever started us; a wrong pointer must leave the board as it is, not walk random memory.
    if be(0) != 0xD00D_FEED {
        return;
    }
    let total = be(4) as usize;
    let (st, ss) = (be(8) as usize, be(12) as usize);
    if total < 40 || total > 16 << 20 || st >= total || ss >= total {
        return;
    }
    // The root: FDT_BEGIN_NODE, its empty name padded to four bytes, then its properties.
    let mut p = st;
    if be(p) != 1 {
        return;
    }
    p += 8;
    while p + 12 <= total {
        match be(p) {
            // A property: its length, its name's offset, its value.
            3 => {
                let (len, name) = (be(p + 4) as usize, be(p + 8) as usize);
                let v = base + p + 12;
                let is_model = ss + name + 6 <= total && (0..6).all(|k| byte(base + ss + name + k) == b"model\0"[k]);
                if is_model {
                    let akita = len >= 5 && (0..=len - 5).any(|k| (0..5).all(|j| byte(v + k + j) == b"AKITA"[j]));
                    if akita {
                        BOARD.store(Board::Akita as u8, Ordering::Relaxed);
                    }
                    return;
                }
                p += 12 + ((len + 3) & !3);
            }
            // FDT_NOP: skip.
            4 => p += 4,
            // The root's first child, or anything else: its own properties are over.
            _ => return,
        }
    }
}

/// akita bring-up, `FERROS_PROBE=<n>` builds only: at checkpoint `n`, back into the bootloader (platform::reboot_to_bootloader: the mode stamped through EL3, then the cluster watchdogs fired; fastboot about 20 s later). Until akita's USB comes up there is no other way out of it, and the host sees the one bit as fastboot coming back or not: the boot reached `n`, and (on akita only) the board was told right. A build without the variable never takes it.
pub fn probe(n: u32) {
    let want = option_env!("FERROS_PROBE").and_then(|s| s.parse::<u32>().ok());
    // A probe also fires for its sub-probes (71 and 72 in a FERROS_PROBE=7 build): every way out of a step answers.
    if akita() && want.is_some_and(|w| w == n || (n >= 10 && w == n / 10)) {
        crate::platform::pstore(b"ferros: akita probe reached, to the bootloader\n");
        crate::platform::reboot_to_bootloader();
    }
}

/// Whether the kernel moves onto the Cortex-X3 (x3.rs): every build unless `FERROS_X3=0`, husky since 2026-10-22 and akita since 2026-11-06. akita stayed on CPU0 that morning on a misreading: the "never came back from the hop" was read from probe 5, which a successful hop never reaches (CPU0 parks), while three other faults could stop its boot; its X3 probe looked dead too, but only because the probe's completion word never matched (x3.rs). With those fixed, the hop works there: the service on the X3 at ~2.9 GHz, the vault, the store and `reboot fastboot` checked.
pub fn x3() -> bool {
    option_env!("FERROS_X3") != Some("0")
}

/// akita bring-up, `FERROS_PROBE=<n>` builds only: at log checkpoint `n`, a plain reset (PSCI), not the bootloader. ABL boots slot a again and counts its retries down, then falls back to slot b's Android, which shows the RAM log of the last ferros session (/sys/fs/pstore; tools/husky-pstore.sh): the flash log's lines and the processes' consoles up to here, read without hands.
pub fn probe_log(n: u32) {
    let want = option_env!("FERROS_PROBE").and_then(|s| s.parse::<u32>().ok());
    if akita() && want.is_some_and(|w| w == n || (n >= 10 && w == n / 10)) {
        crate::platform::pstore(alloc::format!("ferros: akita log probe {n}, reset\n").as_bytes());
        crate::platform::reboot();
    }
}

/// akita bring-up, `FERROS_PROBE=<n>` builds only: at checkpoint `n` the watchdogs stop being petted and the boot goes on as it would. Whatever follows writes its lines to the RAM log; when the watchdog runs out the chip resets warm, ABL counts slot a's retries down and falls back to slot b's Android, and the log of the last session is there to read (/sys/fs/pstore). For the steps that hang rather than fail: no clock here, the hardware watchdog ends it.
pub fn probe_starve(n: u32) {
    if akita() && option_env!("FERROS_PROBE").and_then(|s| s.parse::<u32>().ok()) == Some(n) {
        crate::platform::pstore(alloc::format!("ferros: akita probe {n}: the watchdogs left unpetted from here\n").as_bytes());
        crate::platform::starve_watchdogs();
    }
}
