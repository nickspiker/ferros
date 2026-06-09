//! Qualcomm SPMI PMIC Arbiter v7 driver.
//!
//! SPMI (System Power Management Interface) connects the SoC to PMICs. The arbiter translates SPMI read/write commands to MMIO register accesses.
//!
//! ## QCM6490 SPMI layout
//!
//! ```text
//! Core:    0x0C44_0000 Chnls:   0x0C60_0000  (write channels, +0x1000 per APID) Obsrvr:  0x0E60_0000  (read channels) APID map: Core + 0x2000 (+4 per entry)
//! ```
//!
//! ## Usage
//!
//! 1. Find the APID for a PMIC peripheral: `find_apid(ppid)`
//! 2. Write register: `write_reg(apid, reg_offset, value)`

use crate::mmio;

// ---------------------------------------------------------------------------
// Base addresses (QCM6490 / SC7280 SPMI arbiter v7)
// ---------------------------------------------------------------------------

const CORE_BASE: usize = 0x0C44_0000;
const CHNLS_BASE: usize = 0x0C60_0000;
const OBSRVR_BASE: usize = 0x0E60_0000;
const APID_MAP_BASE: usize = CORE_BASE + 0x2000;

// Channel register offsets
const PMIC_ARB_CMD: usize = 0x00;
const PMIC_ARB_STATUS: usize = 0x08;
const PMIC_ARB_WDATA0: usize = 0x10;

// Status bits
const STATUS_DONE: u32 = 1 << 0;
const STATUS_FAILURE: u32 = 1 << 1;
const STATUS_DENIED: u32 = 1 << 2;
const STATUS_DROPPED: u32 = 1 << 3;

// Arbiter opcodes
const OP_EXT_WRITEL: u32 = 0; // Extended Write Long (up to 8 bytes)

// Max APIDs to scan
const MAX_APIDS: usize = 1024;

// Max polling iterations before giving up
const POLL_MAX: u32 = 100_000;

// ---------------------------------------------------------------------------
// PPID construction
// ---------------------------------------------------------------------------

/// Construct a PPID (Peripheral Port ID) from SID and PID. PPID = (SID << 8) | PID
pub const fn ppid(sid: u8, pid: u8) -> u16 {
    (sid as u16) << 8 | pid as u16
}

// ---------------------------------------------------------------------------
// PMIC SID assignments for FP5 (QCM6490)
// ---------------------------------------------------------------------------

/// PM8350C SID
pub const SID_PM8350C: u8 = 2;

/// PM8350C Flash LED peripheral ID
pub const PID_FLASH: u8 = 0xEE;

/// PM8350C LPG channel 0 (Red LED if wired)
pub const PID_LPG_CH0: u8 = 0xE8;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Find the APID for a given PPID by scanning the arbiter's APID map.
///
/// Returns `None` if the PPID is not found (peripheral not mapped).
pub fn find_apid(target_ppid: u16) -> Option<u16> {
    for n in 0..MAX_APIDS {
        let entry = unsafe { mmio::read32(APID_MAP_BASE + 4 * n) };
        let entry_ppid = ((entry >> 8) & 0xFFF) as u16;
        if entry_ppid == target_ppid {
            return Some(n as u16);
        }
    }
    None
}

/// Write a single byte to a PMIC register via SPMI.
///
/// - `apid`: the arbiter peripheral ID (from `find_apid`)
/// - `reg_offset`: register offset within the peripheral (0x00-0xFF)
/// - `value`: byte to write
///
/// Returns `true` on success, `false` on timeout or error.
pub fn write_byte(apid: u16, reg_offset: u8, value: u8) -> bool {
    write_byte_status(apid, reg_offset, value).0
}

/// Write a byte and return (success, raw_status) for diagnostics.
pub fn write_byte_status(apid: u16, reg_offset: u8, value: u8) -> (bool, u32) {
    let ch = CHNLS_BASE + (apid as usize) * 0x1000;

    unsafe {
        // Write data first
        mmio::write32(ch + PMIC_ARB_WDATA0, value as u32);

        // Construct and issue command EXT_WRITEL opcode=0, reg_offset in bits[11:4], byte_count-1 in bits[3:0]
        let cmd = (OP_EXT_WRITEL << 27)
            | ((reg_offset as u32 & 0xFF) << 4)
            | 0; // 1 byte - 1 = 0
        mmio::write32(ch + PMIC_ARB_CMD, cmd);

        // Poll for completion
        for _ in 0..POLL_MAX {
            let status = mmio::read32(ch + PMIC_ARB_STATUS);
            if status & STATUS_DONE != 0 {
                let ok = (status & (STATUS_FAILURE | STATUS_DENIED | STATUS_DROPPED)) == 0;
                return (ok, status);
            }
        }
    }
    (false, 0xDEAD) // Timeout
}

/// Read a single byte from a PMIC register via SPMI (observer channel).
///
/// - `apid`: the arbiter peripheral ID
/// - `reg_offset`: register offset within the peripheral (0x00-0xFF)
///
/// Returns `Some(value)` on success, `None` on error.
pub fn read_byte(apid: u16, reg_offset: u8) -> Option<u8> {
    // v5 observer offset: 0x10000 * ee + 0x80 * apid (EE=0 for HLOS)
    let ch = OBSRVR_BASE + 0x80 * (apid as usize);

    // Observer channel register offsets (same layout as write channel)
    const OBS_CMD: usize = 0x00;
    const OBS_STATUS: usize = 0x08;
    const OBS_RDATA0: usize = 0x18;

    // EXT_READL opcode = 1
    const OP_EXT_READL: u32 = 1;

    unsafe {
        let cmd = (OP_EXT_READL << 27)
            | ((reg_offset as u32 & 0xFF) << 4)
            | 0; // 1 byte - 1 = 0
        mmio::write32(ch + OBS_CMD, cmd);

        for _ in 0..POLL_MAX {
            let status = mmio::read32(ch + OBS_STATUS);
            if status & STATUS_DONE != 0 {
                if (status & (STATUS_FAILURE | STATUS_DENIED | STATUS_DROPPED)) != 0 {
                    return None;
                }
                let data = mmio::read32(ch + OBS_RDATA0);
                return Some(data as u8);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// PON (Power-On) button status
// ---------------------------------------------------------------------------

/// PM7325 SID (primary PMIC on QCM6490)
pub const SID_PM7325: u8 = 0;

/// PON peripheral ID
pub const PID_PON: u8 = 0x08;

/// PON RT_STS register offset — real-time button state.
const PON_RT_STS: u8 = 0x10;

/// Bit masks in PON RT_STS (active low — 0 = pressed)
const KPDPWR_N: u8 = 1 << 0; // power button
const RESIN_N: u8 = 1 << 1;  // volume down ("reset in")

/// Read PON real-time status. Returns None if SPMI fails.
pub fn pon_rt_sts() -> Option<u8> {
    let pon_ppid = ppid(SID_PM7325, PID_PON);
    let apid = find_apid(pon_ppid)?;
    read_byte(apid, PON_RT_STS)
}

/// Check if volume-down is currently pressed.
pub fn vol_down_pressed() -> bool {
    match pon_rt_sts() {
        Some(sts) => (sts & RESIN_N) == 0, // active low
        None => false,
    }
}

/// Check if power button is currently pressed.
pub fn power_pressed() -> bool {
    match pon_rt_sts() {
        Some(sts) => (sts & KPDPWR_N) == 0, // active low
        None => false,
    }
}

// ---------------------------------------------------------------------------
// PMIC LDO regulator control
// ---------------------------------------------------------------------------

/// LDO control register offsets (within each LDO peripheral)
const LDO_EN_CTL: u8 = 0x46;     // bit 7 = VREG_EN
#[allow(dead_code)] const LDO_VSET_LB: u8 = 0x40;    // voltage set low byte (some PMICs use 0x44)
const LDO_STATUS1: u8 = 0x08;    // regulator status

/// Enable an LDO regulator via SPMI. `sid`: PMIC slave ID, `ldo_pid`: peripheral ID of the LDO. Returns true on success.
pub fn ldo_enable(sid: u8, ldo_pid: u8) -> bool {
    let ldo_ppid = ppid(sid, ldo_pid);
    let apid = match find_apid(ldo_ppid) {
        Some(a) => a,
        None => return false,
    };
    // Read current EN_CTL, set bit 7 (VREG_EN)
    let cur = read_byte(apid, LDO_EN_CTL).unwrap_or(0);
    write_byte(apid, LDO_EN_CTL, cur | 0x80)
}

/// Read LDO STATUS1 register for diagnostics.
pub fn ldo_status(sid: u8, ldo_pid: u8) -> Option<u8> {
    let ldo_ppid = ppid(sid, ldo_pid);
    let apid = find_apid(ldo_ppid)?;
    read_byte(apid, LDO_STATUS1)
}

/// Read LDO EN_CTL register (bit 7 = enabled).
pub fn ldo_en_ctl(sid: u8, ldo_pid: u8) -> Option<u8> {
    let ldo_ppid = ppid(sid, ldo_pid);
    let apid = find_apid(ldo_ppid)?;
    read_byte(apid, LDO_EN_CTL)
}

// PM8350C LDO peripheral IDs (LDO1=0x9B, step 3 per LDO)
/// PM8350C LDO6 PID (vqmmc — SD I/O voltage)
pub const PID_LDO6_PM8350C: u8 = 0xAA;
/// PM8350C LDO9 PID (vmmc — SD card power 2.95V)
pub const PID_LDO9_PM8350C: u8 = 0xB3;

/// Enable SD card power rails (vmmc + vqmmc) on Fairphone 5. Returns (vqmmc_ok, vmmc_ok).
pub fn sd_power_enable() -> (bool, bool) {
    let vqmmc = ldo_enable(SID_PM8350C, PID_LDO6_PM8350C);
    let vmmc = ldo_enable(SID_PM8350C, PID_LDO9_PM8350C);
    (vqmmc, vmmc)
}

// ---------------------------------------------------------------------------
// Flash LED convenience functions
// ---------------------------------------------------------------------------

/// Fire the PM8350C flash LED for a brief burst (~130ms).
///
/// This is the simplest proof-of-life output — a bright white flash from the camera flash LED, requiring only SPMI writes.
///
/// Returns `true` if all SPMI writes succeeded.
pub fn flash_led_fire() -> bool {
    let flash_ppid = ppid(SID_PM8350C, PID_FLASH);
    let apid = match find_apid(flash_ppid) {
        Some(a) => a,
        None => return false,
    };

    // Disable strobe and channels first
    if !write_byte(apid, 0x4A, 0x00) { return false; } // CHAN_STROBE: off
    if !write_byte(apid, 0x4E, 0x00) { return false; } // CHAN_EN: all off

    // Disable module
    if !write_byte(apid, 0x46, 0x00) { return false; } // MODULE_EN: off

    // Set current for channel 0 (source 1): 50mA = (50/12.5)-1 = 3
    if !write_byte(apid, 0x42, 0x03) { return false; } // ITARGET ch0

    // Set timeout: ~130ms (bit7=enable, value=13)
    if !write_byte(apid, 0x3E, 0x8D) { return false; } // SAFETY_TIMER

    // Enable module
    if !write_byte(apid, 0x46, 0x80) { return false; } // MODULE_EN: on

    // Activate channel 0 with SW strobe
    if !write_byte(apid, 0x4A, 0x01) { return false; } // SW strobe active
    if !write_byte(apid, 0x4E, 0x01) { return false; } // CHAN_EN: ch0

    true
}

/// Fire flash LED repeatedly (count times with delay between).
pub fn flash_led_burst(count: u32) {
    for _ in 0..count {
        flash_led_fire();
        // Delay ~200ms between flashes
        for _ in 0..2_000_000u32 {
            unsafe { core::arch::asm!("nop") };
        }
    }
}
