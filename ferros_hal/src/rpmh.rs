//! Qualcomm RPMh (Resource Power Manager hardened) driver.
//!
//! RPMh manages power resources (LDOs, regulators, clocks) on Qualcomm SoCs.
//! The APPS processor communicates with RPMh via TCS (Trigger Command Sets)
//! through the APPS RSC (Resource State Coordinator) MMIO registers.
//!
//! ## cmd-db
//!
//! A firmware-provided database at a fixed DRAM address maps resource names
//! (like "ldoc6") to VRM (Voltage Regulator Module) addresses used in TCS
//! commands. The cmd-db is populated by TF-A before the kernel boots.
//!
//! ## TCS Protocol
//!
//! 1. Look up resource address in cmd-db
//! 2. Write CMD_ADDR, CMD_DATA, CMD_MSGID to a free TCS slot
//! 3. Set CMD_ENABLE bits
//! 4. Trigger via CONTROL register
//! 5. Poll IRQ_STATUS for completion
//!
//! ## QCM6490 Layout
//!
//! ```text
//! APPS RSC:  0x1822_0000
//! TCS base:  RSC + 0xD00
//! cmd-db:    0x8086_0000 (DRAM, read-only)
//! ```

use crate::mmio;

// ---------------------------------------------------------------------------
// cmd-db structures and parser
// ---------------------------------------------------------------------------

/// cmd-db base address in DRAM (populated by TF-A).
const CMD_DB_BASE: usize = 0x8086_0000;

/// Expected magic bytes at offset 0x04.
const CMD_DB_MAGIC: [u8; 4] = [0xDB, 0x30, 0x03, 0x0C];

/// Resource type IDs in rsc_hdr.slv_id.
const SLV_ID_VRM: u16 = 4;

/// Parse cmd-db and find the VRM address for a named resource.
///
/// `name` must be <= 8 bytes (zero-padded internally).
/// Returns the 32-bit VRM address on success.
pub fn cmd_db_lookup(name: &[u8]) -> Option<u32> {
    let base = CMD_DB_BASE;

    // Verify magic at offset 0x04
    let magic_word = unsafe { mmio::read32(base + 0x04) };
    let magic_bytes = magic_word.to_le_bytes();
    if magic_bytes != CMD_DB_MAGIC {
        return None;
    }

    // Scan up to 8 rsc_hdr entries starting at offset 0x08 (16 bytes each)
    for i in 0..8u32 {
        let hdr_off = 0x08 + (i as usize) * 16;
        let w0 = unsafe { mmio::read32(base + hdr_off) };
        let slv_id = (w0 & 0xFFFF) as u16;
        let header_offset = ((w0 >> 16) & 0xFFFF) as u16;
        let w1 = unsafe { mmio::read32(base + hdr_off + 4) };
        let _data_offset = (w1 & 0xFFFF) as u16;
        let cnt = ((w1 >> 16) & 0xFFFF) as u16;

        if slv_id == 0 && cnt == 0 {
            break; // Empty slot, end of headers
        }

        if slv_id != SLV_ID_VRM {
            continue;
        }

        // Found VRM header — scan entry_headers (24 bytes each)
        // entry_headers start at data[] which is at base + 0x90 + header_offset
        let entries_base = base + 0x90 + header_offset as usize;

        for j in 0..cnt as usize {
            let entry_off = entries_base + j * 24;

            // Read 8-byte resource ID
            let id_w0 = unsafe { mmio::read32(entry_off) };
            let id_w1 = unsafe { mmio::read32(entry_off + 4) };
            let mut id = [0u8; 8];
            id[..4].copy_from_slice(&id_w0.to_le_bytes());
            id[4..].copy_from_slice(&id_w1.to_le_bytes());

            // Compare with requested name (zero-padded)
            let mut padded = [0u8; 8];
            let len = name.len().min(8);
            padded[..len].copy_from_slice(&name[..len]);

            if id == padded {
                // addr is at offset 16 within entry_header
                let addr = unsafe { mmio::read32(entry_off + 16) };
                return Some(addr);
            }
        }
    }

    None
}

/// Dump all VRM entries from cmd-db. Calls `cb(name, addr)` for each entry.
pub fn cmd_db_dump_vrm<F: FnMut(&[u8; 8], u32)>(mut cb: F) {
    let base = CMD_DB_BASE;

    let magic_word = unsafe { mmio::read32(base + 0x04) };
    if magic_word.to_le_bytes() != CMD_DB_MAGIC {
        return;
    }

    for i in 0..8u32 {
        let hdr_off = 0x08 + (i as usize) * 16;
        let w0 = unsafe { mmio::read32(base + hdr_off) };
        let slv_id = (w0 & 0xFFFF) as u16;
        let header_offset = ((w0 >> 16) & 0xFFFF) as u16;
        let w1 = unsafe { mmio::read32(base + hdr_off + 4) };
        let cnt = ((w1 >> 16) & 0xFFFF) as u16;

        if slv_id == 0 && cnt == 0 { break; }
        if slv_id != SLV_ID_VRM { continue; }

        let entries_base = base + 0x90 + header_offset as usize;
        for j in 0..cnt as usize {
            let entry_off = entries_base + j * 24;
            let id_w0 = unsafe { mmio::read32(entry_off) };
            let id_w1 = unsafe { mmio::read32(entry_off + 4) };
            let mut id = [0u8; 8];
            id[..4].copy_from_slice(&id_w0.to_le_bytes());
            id[4..].copy_from_slice(&id_w1.to_le_bytes());
            let addr = unsafe { mmio::read32(entry_off + 16) };
            cb(&id, addr);
        }
    }
}

// ---------------------------------------------------------------------------
// APPS RSC / TCS registers
// ---------------------------------------------------------------------------

/// APPS RSC DRV2 base address (QCM6490).
const RSC_BASE: usize = 0x1822_0000;

/// TCS base offset within DRV (from DT qcom,tcs-offset).
const TCS_DRV_OFFSET: usize = 0xD00;

/// TCS register stride — v2.7: 0x2A0 per TCS block.
#[allow(dead_code)] const TCS_STRIDE: usize = 0x2A0;

/// Command register stride within a TCS — v2.7: 0x14 per command.
#[allow(dead_code)] const CMD_STRIDE: usize = 0x14;

// TCS-level register offsets (relative to tcs_base + tcs_id * TCS_STRIDE)
#[allow(dead_code)] const TCS_IRQ_ENABLE: usize = 0x00;
const TCS_IRQ_STATUS: usize = 0x04;
const TCS_IRQ_CLEAR: usize = 0x08;
const TCS_CMD_WAIT_FOR_CMPL: usize = 0x10;
const TCS_CONTROL: usize = 0x14;
#[allow(dead_code)] const TCS_STATUS: usize = 0x18;
const TCS_CMD_ENABLE: usize = 0x1C;

// Per-command register offsets (+ cmd_id * CMD_STRIDE)
const TCS_CMD_MSGID: usize = 0x30;
const TCS_CMD_ADDR: usize = 0x34;
const TCS_CMD_DATA: usize = 0x38;
const TCS_CMD_STATUS: usize = 0x3C;

// CONTROL register bits
const TCS_AMC_MODE_ENABLE: u32 = 1 << 16;
const TCS_AMC_MODE_TRIGGER: u32 = 1 << 24;

// CMD_MSGID bits
const CMD_MSGID_LEN: u32 = 8;          // payload length in bytes [7:0]
const CMD_MSGID_RESP_REQ: u32 = 1 << 8; // request completion response
const CMD_MSGID_WRITE: u32 = 1 << 16;  // write request (bit 16)

// CMD_STATUS bits
#[allow(dead_code)] const CMD_STATUS_COMPL: u32 = 1 << 16;

/// VRM register offsets within a resource address.
pub const VRM_VOLTAGE: u32 = 0x0;   // voltage in mV
pub const VRM_ENABLE: u32 = 0x4;    // 0=off, 1=on
pub const VRM_MODE: u32 = 0x8;      // regulator mode

/// Send a single RPMh write command via TCS.
///
/// Uses TCS slot 0 (active-only, intended for immediate requests).
/// `addr` is the VRM address from cmd-db + register offset.
/// `data` is the value to write.
///
/// Returns `true` on success.
pub fn tcs_write(addr: u32, data: u32) -> bool {
    let tcs_base = RSC_BASE + TCS_DRV_OFFSET;
    // Use TCS index 0, command slot 0
    let tcs = tcs_base; // TCS 0

    unsafe {
        // 1. Clear any pending IRQ for TCS 0
        mmio::write32(tcs + TCS_IRQ_CLEAR, 1);

        // 2. Set up command 0: write request with completion response
        let msgid = CMD_MSGID_LEN | CMD_MSGID_WRITE | CMD_MSGID_RESP_REQ;
        mmio::write32(tcs + TCS_CMD_MSGID, msgid);
        mmio::write32(tcs + TCS_CMD_ADDR, addr);
        mmio::write32(tcs + TCS_CMD_DATA, data);

        // 3. Enable command 0, wait for its completion
        mmio::write32(tcs + TCS_CMD_ENABLE, 1);
        mmio::write32(tcs + TCS_CMD_WAIT_FOR_CMPL, 1);

        // 4. Trigger sequence: clear trigger → clear enable → set enable → set trigger
        let ctrl = mmio::read32(tcs + TCS_CONTROL);
        mmio::write32(tcs + TCS_CONTROL, ctrl & !TCS_AMC_MODE_TRIGGER);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        mmio::write32(tcs + TCS_CONTROL, ctrl & !(TCS_AMC_MODE_TRIGGER | TCS_AMC_MODE_ENABLE));
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        mmio::write32(tcs + TCS_CONTROL, TCS_AMC_MODE_ENABLE);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        mmio::write32(tcs + TCS_CONTROL, TCS_AMC_MODE_ENABLE | TCS_AMC_MODE_TRIGGER);

        // 5. Poll IRQ_STATUS for TCS 0 completion (bit 0)
        for _ in 0..100_000u32 {
            let irq = mmio::read32(tcs + TCS_IRQ_STATUS);
            if irq & 1 != 0 {
                // Clear the IRQ
                mmio::write32(tcs + TCS_IRQ_CLEAR, 1);
                return true;
            }
        }
    }

    false // Timeout
}

/// Read TCS IRQ_STATUS for diagnostics.
pub fn tcs0_irq_status() -> u32 {
    let tcs_base = RSC_BASE + TCS_DRV_OFFSET;
    unsafe { mmio::read32(tcs_base + TCS_IRQ_STATUS) }
}

/// Read TCS CONTROL register for diagnostics.
pub fn tcs0_control() -> u32 {
    let tcs_base = RSC_BASE + TCS_DRV_OFFSET;
    unsafe { mmio::read32(tcs_base + TCS_CONTROL) }
}

/// Read TCS CMD_STATUS for cmd 0 (diagnostics).
pub fn tcs0_cmd0_status() -> u32 {
    let tcs_base = RSC_BASE + TCS_DRV_OFFSET;
    unsafe { mmio::read32(tcs_base + TCS_CMD_STATUS) }
}

/// Enable a VRM regulator via RPMh TCS.
///
/// `vrm_addr` is the base address from cmd-db for the resource.
/// Sends enable=1 command.
pub fn vrm_enable(vrm_addr: u32) -> bool {
    tcs_write(vrm_addr + VRM_ENABLE, 1)
}

/// Set VRM regulator voltage via RPMh TCS.
///
/// `vrm_addr` is the base address from cmd-db.
/// `mv` is voltage in millivolts.
pub fn vrm_set_voltage(vrm_addr: u32, mv: u32) -> bool {
    tcs_write(vrm_addr + VRM_VOLTAGE, mv)
}

/// Enable SD card power rails via RPMh.
///
/// Looks up "ldoc6" (vqmmc, I/O voltage) and "ldoc9" (vmmc, card power)
/// in cmd-db, then sends TCS commands to enable them.
///
/// Returns (ldoc6_ok, ldoc9_ok).
pub fn sd_power_enable_rpmh() -> (bool, bool) {
    let ldoc6_addr = cmd_db_lookup(b"ldoc6");
    let ldoc9_addr = cmd_db_lookup(b"ldoc9");

    let c6_ok = match ldoc6_addr {
        Some(addr) => {
            // vqmmc — 1.8V I/O voltage
            vrm_set_voltage(addr, 1800) && vrm_enable(addr)
        }
        None => false,
    };

    let c9_ok = match ldoc9_addr {
        Some(addr) => {
            // vmmc — 2.95V card power
            vrm_set_voltage(addr, 2950) && vrm_enable(addr)
        }
        None => false,
    };

    (c6_ok, c9_ok)
}
