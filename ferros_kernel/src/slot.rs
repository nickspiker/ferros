//! A/B slot marking on husky: tell the bootloader that the slot ferros booted from is good, so ABL stops counting down its retries and the slot stays ferros across power cycles — what Android's boot-control HAL does once boot completes.
//!
//! The record is the `devinfo` partition (UFS LUN 3, 4 KiB LBA 6), measured on hardware (2026-10-14): magic "DEVI", u16 major = 3, then at G#30 one 4-byte word per slot (a, b): byte 0 = retries left, byte 1 = flags (b0 unbootable, b1 successful, b2 active, b3 de-activated by a switch — not booted). `fastboot --set-active` sets retries to 3, every ABL boot of an unmarked slot takes one, a marked slot keeps its count. Nothing else in the partition changed across slot switches and boots (no checksum, no mirror). The rest of the block is factory identity data (serials, MACs, IMEIs): it is rewritten byte-identical and verified, never altered.
//!
//! Which slot booted is taken from two independent sources that must agree: the last `androidboot.slot_suffix` in the bootconfig ABL appends to the initrd (it carries two, `_a` then the real one — `:=` overrides, last wins), and devinfo's active bit. Any disagreement, unknown version or unknown flag bit → nothing is written.

use ferros_hal::ufs::UfsController;

const UFS_BASE: usize = 0x1320_0000;
const DEVINFO_LUN: u8 = 3;
const DEVINFO_LBA: u32 = 6;
const SLOT_WORDS: usize = 0x30;
const FLAG_SUCCESSFUL: u8 = 1 << 1;
const FLAG_ACTIVE: u8 = 1 << 2;
/// Bit 3: set on the slot that was de-activated by a switch (`fastboot --set-active` leaves it on the old slot and clears it on the new; measured 2026-10-17). A slot carrying it is not booted. Earlier guessed as "fastboot-ok".
const FLAG_DEACTIVATED: u8 = 1 << 3;
const FLAG_KNOWN: u8 = 0x0F;

/// Outcome of `mark_booted_slot`, reported in DIAG as SLOT_MARK.
pub const MARKED: u32 = 0;
pub const ALREADY: u32 = 1;
pub const NO_INITRD: u32 = 0x10;
pub const NO_BOOTCONFIG: u32 = 0x11;
pub const NO_SLOT_SUFFIX: u32 = 0x12;
pub const READ_FAILED: u32 = 0x20;
pub const BAD_FORMAT: u32 = 0x21;
pub const UNKNOWN_FLAGS: u32 = 0x22;
pub const NOT_ACTIVE: u32 = 0x23;
pub const WRITE_FAILED: u32 = 0x30;
pub const VERIFY_FAILED: u32 = 0x31;
/// The devinfo write took but the UFS bBootLunEn attribute could not be switched (or did not read back): the bootloader would boot the old slot and re-sync devinfo to it.
pub const BOOTLUN_FAILED: u32 = 0x32;

pub struct SlotMark {
    /// 0 = a, 1 = b, G#FF = unknown.
    pub slot: u32,
    /// The booted slot's devinfo word before and after (LE: retries | flags << 8).
    pub before: u32,
    pub after: u32,
    pub result: u32,
}

/// The booted slot from ABL's bootconfig: the trailer "#BOOTCONFIG\n" ends the initrd, preceded by the text size and checksum (u32 LE each).
fn booted_slot(dtb_addr: u64) -> Result<usize, u32> {
    let dtb = unsafe { ferros_hal::dtb::Dtb::from_ptr(dtb_addr as *const u8) }.ok_or(NO_INITRD)?;
    let prop = dtb.find_node_prop(b"chosen", b"linux,initrd-end").ok_or(NO_INITRD)?;
    let end = if prop.data.len() >= 8 { prop.as_u64() } else { prop.as_u32().map(|v| v as u64) }.ok_or(NO_INITRD)? as usize;
    if end < 20 {
        return Err(NO_BOOTCONFIG);
    }
    let tail = unsafe { core::slice::from_raw_parts((end - 20) as *const u8, 20) };
    if &tail[8..] != b"#BOOTCONFIG\n" {
        return Err(NO_BOOTCONFIG);
    }
    let size = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
    if size == 0 || size > 64 * 1024 || size > end - 20 {
        return Err(NO_BOOTCONFIG);
    }
    let text = unsafe { core::slice::from_raw_parts((end - 20 - size) as *const u8, size) };
    let key = b"androidboot.slot_suffix";
    let at = text.windows(key.len()).rposition(|w| w == key).ok_or(NO_SLOT_SUFFIX)?;
    let line = &text[at + key.len()..];
    let line = &line[..line.iter().position(|&c| c == b'\n').unwrap_or(line.len())];
    let eq = line.iter().position(|&c| c == b'=').ok_or(NO_SLOT_SUFFIX)?;
    match line[eq + 1..].iter().filter(|&&c| c != b' ' && c != b'"').copied().collect::<alloc::vec::Vec<u8>>().as_slice() {
        b"_a" => Ok(0),
        b"_b" => Ok(1),
        _ => Err(NO_SLOT_SUFFIX),
    }
}

fn read_devinfo(ufs: &UfsController) -> Option<alloc::vec::Vec<u8>> {
    // A LUN's first command after power-on can report UNIT ATTENTION; it succeeds on retry.
    for _ in 0..4 {
        if ufs.read_block_lun(DEVINFO_LUN, DEVINFO_LBA) == 0 && ufs.last_response_status() == 0 {
            return Some(ufs.data_buffer().to_vec());
        }
    }
    None
}

/// Mark the slot this boot came from as successful. `dtb_addr` must be ABL's DTB pointer (a real boot, not a RELOAD); the UFS link must be up.
pub fn mark_booted_slot(dtb_addr: u64) -> SlotMark {
    let mut m = SlotMark { slot: 0xFF, before: 0, after: 0, result: 0 };
    let slot = match booted_slot(dtb_addr) {
        Ok(s) => s,
        Err(e) => {
            m.result = e;
            return m;
        }
    };
    m.slot = slot as u32;
    let ufs = UfsController::new(UFS_BASE);
    let Some(block) = read_devinfo(&ufs) else {
        m.result = READ_FAILED;
        return m;
    };
    let word = |b: &[u8], s: usize| u32::from_le_bytes([b[SLOT_WORDS + 4 * s], b[SLOT_WORDS + 4 * s + 1], b[SLOT_WORDS + 4 * s + 2], b[SLOT_WORDS + 4 * s + 3]]);
    m.before = word(&block, slot);
    m.after = m.before;
    if &block[0..4] != b"DEVI" || u16::from_le_bytes([block[4], block[5]]) != 3 {
        m.result = BAD_FORMAT;
        return m;
    }
    let flags = [block[SLOT_WORDS + 1], block[SLOT_WORDS + 5]];
    if flags.iter().any(|f| f & !FLAG_KNOWN != 0) {
        m.result = UNKNOWN_FLAGS;
        return m;
    }
    if flags[slot] & FLAG_ACTIVE == 0 || flags[1 - slot] & FLAG_ACTIVE != 0 {
        m.result = NOT_ACTIVE;
        return m;
    }
    // The other slot stays a spare (owner, 2026-10-28: "if a fails 7 times it goes to b"). A switch leaves it de-activated, and ABL's fallback when this slot's retries run out is proven only onto a slot without that flag (BOOT.md, the A/B fallback test). So once it has booted successfully before, the flag is cleared here, on every boot of this slot, and the spare stays armed.
    let rearm = flags[1 - slot] & FLAG_SUCCESSFUL != 0 && flags[1 - slot] & FLAG_DEACTIVATED != 0;
    if flags[slot] & FLAG_SUCCESSFUL != 0 && !rearm {
        m.result = ALREADY;
        return m;
    }
    let mut want = block.clone();
    want[SLOT_WORDS + 4 * slot + 1] |= FLAG_SUCCESSFUL;
    if rearm {
        want[SLOT_WORDS + 4 * (1 - slot) + 1] &= !FLAG_DEACTIVATED;
    }
    ufs.data_buffer_mut().copy_from_slice(&want);
    if ufs.write_block_lun(DEVINFO_LUN, DEVINFO_LBA) != 0 || ufs.last_response_status() != 0 {
        m.result = WRITE_FAILED;
        return m;
    }
    let Some(back) = read_devinfo(&ufs) else {
        m.result = VERIFY_FAILED;
        return m;
    };
    m.after = word(&back, slot);
    m.result = if back == want { MARKED } else { VERIFY_FAILED };
    m
}

/// Set one slot's word as ABL reads it (`diag slot word`, the A/B fallback test): retries left and flags, nothing else in devinfo touched. Refuses unknown flags; read back after writing. Development: what Android's boot control and ferros's marking do in steps, done in one.
pub fn set_word(slot: usize, retries: u8, flags: u8) -> u32 {
    if slot > 1 || flags & !FLAG_KNOWN != 0 {
        return BAD_FORMAT;
    }
    let ufs = UfsController::new(UFS_BASE);
    let Some(block) = read_devinfo(&ufs) else { return READ_FAILED };
    if &block[0..4] != b"DEVI" || u16::from_le_bytes([block[4], block[5]]) != 3 {
        return BAD_FORMAT;
    }
    let mut want = block.clone();
    want[SLOT_WORDS + 4 * slot] = retries;
    want[SLOT_WORDS + 4 * slot + 1] = flags;
    ufs.data_buffer_mut().copy_from_slice(&want);
    if ufs.write_block_lun(DEVINFO_LUN, DEVINFO_LBA) != 0 || ufs.last_response_status() != 0 {
        return WRITE_FAILED;
    }
    if !matches!(read_devinfo(&ufs), Some(back) if back == want) {
        return VERIFY_FAILED;
    }
    MARKED
}

/// Both slots' words as they are now (retries | flags << 8), for `diag slot`.
pub fn words() -> Option<[u32; 2]> {
    let ufs = UfsController::new(UFS_BASE);
    let b = read_devinfo(&ufs)?;
    Some([b[SLOT_WORDS] as u32 | (b[SLOT_WORDS + 1] as u32) << 8, b[SLOT_WORDS + 4] as u32 | (b[SLOT_WORDS + 5] as u32) << 8])
}

/// Make `slot` (0 = a, 1 = b) the one ABL boots next: its active flag set with 3 retries (what `fastboot --set-active` writes), the other slot's active flag cleared, everything else byte for byte; verified by reading back. The gate's Android choice (ready.rs) uses it for slot a before a reset. Returns a result code as `mark_booted_slot` does.
pub fn set_active(slot: usize) -> u32 {
    if slot > 1 {
        return BAD_FORMAT;
    }
    let ufs = UfsController::new(UFS_BASE);
    let Some(block) = read_devinfo(&ufs) else { return READ_FAILED };
    if &block[0..4] != b"DEVI" || u16::from_le_bytes([block[4], block[5]]) != 3 {
        return BAD_FORMAT;
    }
    let flags = [block[SLOT_WORDS + 1], block[SLOT_WORDS + 5]];
    if flags.iter().any(|f| f & !FLAG_KNOWN != 0) {
        return UNKNOWN_FLAGS;
    }
    // Exactly what `fastboot --set-active` leaves (measured 2026-10-17 from Android after a switch to a: a = retries 2/3, flags G#06; b = flags G#0A): the new slot gets 3 retries, the active bit, and bit 3 CLEARED; the old slot loses the active bit and GAINS bit 3. Bit 3 marks the de-activated slot; ABL would not boot a slot still carrying it (our first write left it on a: ABL moved active back to b without even trying a).
    let mut want = block.clone();
    want[SLOT_WORDS + 4 * slot] = 3;
    want[SLOT_WORDS + 4 * slot + 1] = (want[SLOT_WORDS + 4 * slot + 1] | FLAG_ACTIVE) & !FLAG_DEACTIVATED;
    want[SLOT_WORDS + 4 * (1 - slot) + 1] = (want[SLOT_WORDS + 4 * (1 - slot) + 1] & !FLAG_ACTIVE) | FLAG_DEACTIVATED;
    ufs.data_buffer_mut().copy_from_slice(&want);
    if ufs.write_block_lun(DEVINFO_LUN, DEVINFO_LBA) != 0 || ufs.last_response_status() != 0 {
        return WRITE_FAILED;
    }
    if !matches!(read_devinfo(&ufs), Some(back) if back == want) {
        return VERIFY_FAILED;
    }
    // The part devinfo alone cannot do (measured 2026-10-17, bootloader log): the ROM boots the bootloader chain from the Boot LU named by the UFS attribute bBootLunEn (1 = A, 2 = B), and the bootloader boots the Android slot of that letter — its log prints "boot slot: b" before it reads devinfo, and then writes the active flag back onto that slot ("AB Decision: active slot boot ok"). Three devinfo-only writes were undone that way. So the hand-off switches the Boot LU as well, as `fastboot --set-active` does.
    let lun_en = slot as u32 + 1;
    if ufs.read_attribute(0x00, 0) != Some(lun_en) && (!ufs.write_attribute(0x00, 0, lun_en) || ufs.read_attribute(0x00, 0) != Some(lun_en)) {
        return BOOTLUN_FAILED;
    }
    MARKED
}
