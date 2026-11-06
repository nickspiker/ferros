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
