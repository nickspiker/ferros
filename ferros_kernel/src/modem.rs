//! The cellular modem's one kernel duty: the handover block, assembled at a real boot while ABL's device tree is still there. Everything else — the cold bring-up over PCIe ch0, the boot-channel download, the online service, the data call and its packets on `rmnet` — is the modem driver process (payloads/modem, since 2026-10-29: the owner's drivers-to-userspace plan), which reaches the CP through hardware, DMA and partition grants. docs/oracle/MODEM-BOOT-RECON.md has the protocol.

use alloc::vec::Vec;

/// The IPC region the CP and its driver share (zuma-rmem.dtsi), EL0-accessible for the driver (cachemap.rs `MODEM_WINDOWS`).
pub const IPC: usize = 0xEA40_0000;

fn rd(a: usize) -> u32 {
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
fn wr8(a: usize, v: u8) {
    unsafe { core::ptr::write_volatile(a as *mut u8, v) }
}
fn rd8(a: usize) -> u8 {
    unsafe { core::ptr::read_volatile(a as *const u8) }
}

fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}

// ---------------------------------------------------------------------------
// The handover block: `struct t_handover_block_info` (modem_prj.h 179-196), G#A1 bytes at ap2cp_handover_block_info (IPC+G#82C), which cbd fills before the boot (`update_handover_block_info`, decoded from cbd's own symbols 2026-10-29; both captures show its memcpy first of all). It tells the CP which board it is in, so it can pick its RF configuration. Layout and sources, for a normal boot of a user build:
//   G#00 version 1 · G#04 project_id = cdt[0] (platform) · G#08 revision = `hwinfo` from hardware_config.json · G#0C major_id = cdt[3] · G#10 minor_id = cdt[4] · G#14 modem_sku = cdt[6] · G#18 modem_hw = cdt[7] · G#1C cpinfo0 = 0 (set only for factory boot modes) · G#20 cpinfo1 = 0 (set only after an EFS clear) · G#24 cpinfo2 = /chosen/config/modem_flag · G#28 rf_sub = cdt[8] · G#2C rf_config = `rfid` from hardware_config.json, else /chosen/plat/rfid · G#30, G#34, G#38 reserved = cdt[1], cdt[2], cdt[5] · G#3C reserved = 0 (user build) · G#40, G#50 cpid = /chosen/config/imei1, imei2 (16 bytes each) · G#60 cpsig = /mnt/vendor/persist/modem/cpsha (64 bytes), then a NUL.
// cdt[] is the bootconfig's `androidboot.cdt_hwid`, read by cbd as "0x%04x%02x%02x%04x%02x%02x%02x%02x%04x%08x". husky's radio build carries no hardware_config.json (its /images/default holds modem.bin, uecap, pw_token_db and confpack), so cbd's lookup finds nothing: revision stays 0 and rf_config comes from the device tree, as here.
// The block is assembled in place at a real boot (`read_board`), while ABL's tree is still there: a RELOAD has no tree, and the IPC page keeps the block across reloads. cpsig is added by the driver's ROM stage, which reads persist. The two IMEIs are this phone's identity: they go from ABL's device tree to the CP's own memory and are never printed, logged or stored.

const HANDOVER: usize = IPC + 0x82C;
const HANDOVER_LEN: usize = 0xA1;

/// The bootconfig ABL appends to the initrd (slot.rs: the text, then its size and checksum, then "#BOOTCONFIG\n", ending at /chosen/linux,initrd-end).
fn bootconfig(dtb: &ferros_hal::dtb::Dtb) -> Option<&'static [u8]> {
    let prop = dtb.find_node_prop(b"chosen", b"linux,initrd-end")?;
    let end = if prop.data.len() >= 8 { prop.as_u64() } else { prop.as_u32().map(|v| v as u64) }? as usize;
    if end < 20 {
        return None;
    }
    let tail = unsafe { core::slice::from_raw_parts((end - 20) as *const u8, 20) };
    if &tail[8..] != b"#BOOTCONFIG\n" {
        return None;
    }
    let size = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
    // External data (ABL's): a size past the initrd's own end would read below it.
    if size == 0 || size > 64 * 1024 || size > end - 20 {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts((end - 20 - size) as *const u8, size) })
}

/// The last `key = value` in the bootconfig (`:=` overrides, last wins), quotes and spaces dropped.
fn bootconfig_value(text: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    let at = text.windows(key.len()).rposition(|w| w == key)?;
    let line = &text[at + key.len()..];
    let line = &line[..line.iter().position(|&c| c == b'\n').unwrap_or(line.len())];
    let eq = line.iter().position(|&c| c == b'=')?;
    Some(line[eq + 1..].iter().filter(|&&c| c != b' ' && c != b'"').copied().collect())
}

fn hex_digit(c: u8) -> Option<u32> {
    (c as char).to_digit(16)
}

/// Assemble the handover block at IPC+G#82C from ABL's device tree and bootconfig (a real boot only). Every field cbd leaves at 0 on a normal boot is 0; cpsig is zero until the ROM stage fills it.
pub fn read_board(dtb_addr: u64) {
    let Some(dtb) = (unsafe { ferros_hal::dtb::Dtb::from_ptr(dtb_addr as *const u8) }) else {
        return;
    };
    let mut b = [0u8; HANDOVER_LEN];
    let mut put = |off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put(0x00, 1);
    if let Some(v) = bootconfig(&dtb).and_then(|t| bootconfig_value(t, b"androidboot.cdt_hwid")) {
        // "0x" then the ten fields at their fixed widths (cbd's sscanf format): 4 2 2 4 2 2 2 2 4 8 hex digits.
        let digits = v.strip_prefix(b"0x").unwrap_or(&v);
        let widths = [4usize, 2, 2, 4, 2, 2, 2, 2, 4, 8];
        if digits.len() == widths.iter().sum::<usize>() && digits.iter().all(|&c| hex_digit(c).is_some()) {
            let mut cdt = [0u32; 10];
            let mut at = 0;
            for (k, w) in widths.iter().enumerate() {
                cdt[k] = digits[at..at + w].iter().fold(0u32, |a, &c| a << 4 | hex_digit(c).unwrap());
                at += w;
            }
            for (off, k) in [(0x04, 0), (0x0C, 3), (0x10, 4), (0x14, 6), (0x18, 7), (0x28, 8), (0x30, 1), (0x34, 2), (0x38, 5)] {
                put(off, cdt[k]);
            }
        }
    }
    if let Some(v) = dtb.find_node_prop(b"plat", b"rfid").and_then(|p| p.as_u32()) {
        put(0x2C, v);
    }
    if let Some(p) = dtb.find_node_prop(b"config", b"modem_flag") {
        // fscanf "%x": optional spaces, an optional 0x, then hex digits up to the first other byte.
        let s = p.data.iter().skip_while(|&&c| c == b' ').copied().collect::<Vec<u8>>();
        let s = s.strip_prefix(b"0x").unwrap_or(&s);
        let n = s.iter().take_while(|&&c| hex_digit(c).is_some()).count();
        put(0x24, s[..n].iter().fold(0u32, |a, &c| a << 4 | hex_digit(c).unwrap()));
    }
    for (off, name) in [(0x40, b"imei1"), (0x50, b"imei2")] {
        if let Some(p) = dtb.find_node_prop(b"config", name) {
            // The property's length is ABL's; the block field is 16 bytes and cbd reads exactly 16, so a longer property is cut there and a shorter one zero-filled.
            let n = p.data.len().min(16);
            b[off..off + n].copy_from_slice(&p.data[..n]);
        }
    }
    for (i, &x) in b.iter().enumerate() {
        wr8(HANDOVER + i, x);
    }
    unsafe { core::arch::asm!("dsb sy") };
}

/// `diag modem hwcfg`: the handover block as it stands at IPC+G#82C — its board words in full, the IMEI and cpsig fields only as set or zero.
pub fn hwcfg(rep: &mut Vec<u8>) {
    let names = ["version", "project_id", "revision", "major_id", "minor_id", "modem_sku", "modem_hw", "cpinfo0", "cpinfo1", "cpinfo2", "rf_sub", "rf_config", "reserved0", "reserved1", "reserved2", "reserved3"];
    for (k, n) in names.iter().enumerate() {
        line(rep, alloc::format!("{n}: G#{:X}\n", rd(HANDOVER + 4 * k)));
    }
    let set = |off: usize, len: usize| (0..len).any(|i| rd8(HANDOVER + off + i) != 0);
    line(rep, alloc::format!("cpid 0, cpid 1, cpsig: {}, {}, {} (not shown)\n", ["zero", "set"][set(0x40, 16) as usize], ["zero", "set"][set(0x50, 16) as usize], ["zero", "set"][set(0x60, 64) as usize]));
}
