//! SD/MMC controller driver — microSD card access.
//!
//! This is the storage driver that lets the ledger read/write the
//! microSD card. On QCM6490, the SD/MMC controller is a Qualcomm
//! SDHCI-compatible block at a known MMIO base.
//!
//! ## Boot Sequence
//!
//! ABL (or XBL) typically initializes the SDHCI controller during
//! boot to check for SD card presence. We may inherit an already-
//! initialized controller, or we may need to do full init:
//!
//! ```text
//! 1. Reset controller
//! 2. Set clock to 400KHz (identification mode)
//! 3. Send CMD0 (GO_IDLE)
//! 4. Send CMD8 (SEND_IF_COND) — voltage check
//! 5. ACMD41 loop (SD_SEND_OP_COND) — wait for card ready
//! 6. CMD2 (ALL_SEND_CID) — get card identity (vendor, serial)
//! 7. CMD3 (SEND_RELATIVE_ADDR) — get RCA
//! 8. CMD7 (SELECT_CARD) — select for data transfer
//! 9. Set clock to 25/50MHz (data transfer mode)
//! 10. CMD16 (SET_BLOCKLEN) — set to 512 bytes
//! ```
//!
//! ## Implementation Status
//!
//! This is a skeleton. The register definitions are from the SD Host
//! Controller Simplified Spec v3.00 (SDHCI). QCM6490 uses a Qualcomm
//! variant with vendor-specific registers in the 0x200+ range.

use alloc::vec::Vec;

use ferros_vault::device::{Device, DeviceError, DeviceId, DeviceInfo, DeviceIoKind};

/// Standard SDHCI register offsets (from SD Host Controller Spec v3.00).
#[allow(dead_code)]
mod regs {
    pub const BLOCK_SIZE: usize = 0x04;
    pub const BLOCK_COUNT: usize = 0x06;
    pub const ARGUMENT: usize = 0x08;
    pub const TRANSFER_MODE: usize = 0x0C;
    pub const RESPONSE: usize = 0x10; // 0x10-0x1F (128 bits)
    pub const BUFFER_DATA: usize = 0x20;
    pub const PRESENT_STATE: usize = 0x24;
    pub const HOST_CTRL1: usize = 0x28;
    pub const POWER_CTRL: usize = 0x29;
    pub const CLOCK_CTRL: usize = 0x2C;
    pub const TIMEOUT_CTRL: usize = 0x2E;
    pub const SW_RESET: usize = 0x2F;
    pub const NORMAL_INT_STATUS: usize = 0x30;
    pub const ERROR_INT_STATUS: usize = 0x32;
    pub const NORMAL_INT_STATUS_EN: usize = 0x34;
    pub const ERROR_INT_STATUS_EN: usize = 0x36;
    pub const CAPABILITIES: usize = 0x40;

    // Present State bits
    pub const CMD_INHIBIT: u32 = 1 << 0;
    pub const DAT_INHIBIT: u32 = 1 << 1;

    // HOST_CTRL1 bits
    pub const HOST_4BIT: u8 = 1 << 1;  // 4-bit data transfer width

    // Qualcomm vendor-specific registers (sdhci-msm v5)
    pub const VENDOR_SPEC: usize = 0x20C;
    pub const PWRCTL_STATUS: usize = 0x240;
    pub const PWRCTL_MASK: usize = 0x244;
    pub const PWRCTL_CLEAR: usize = 0x248;
    pub const PWRCTL_CTL: usize = 0x24C;

    // VENDOR_SPEC POR (power-on-reset) value from sdhci-msm driver
    pub const VENDOR_SPEC_POR_VAL: u32 = 0x0A0C;

    // PWRCTL_CTL success bits
    pub const PWRCTL_BUS_SUCCESS: u32 = 1 << 0;
    pub const PWRCTL_IO_SUCCESS: u32 = 1 << 2;

    // Normal interrupt status bits
    pub const CMD_COMPLETE: u16 = 1 << 0;
    pub const XFER_COMPLETE: u16 = 1 << 1;
    pub const BUF_WRITE_READY: u16 = 1 << 4;
    pub const BUF_READ_READY: u16 = 1 << 5;
    pub const ERR_INTERRUPT: u16 = 1 << 15;

    // SDHCI command register response type encoding (bits [1:0])
    pub const RESP_NONE: u16 = 0x00;    // no response
    pub const RESP_136: u16 = 0x01;     // R2 (136-bit)
    pub const RESP_48: u16 = 0x02;      // R1, R3, R6, R7 (48-bit)
    pub const RESP_48_BUSY: u16 = 0x03; // R1b (48-bit + busy)

    // Command register flag bits
    pub const CMD_CRC_CHECK: u16 = 1 << 3;
    pub const CMD_INDEX_CHECK: u16 = 1 << 4;
    pub const CMD_DATA_PRESENT: u16 = 1 << 5;
}

/// SDHCI command response types.
#[derive(Clone, Copy)]
enum RespType {
    None,    // CMD0
    R1,      // most commands
    R1b,     // CMD7, CMD12
    R2,      // CMD2, CMD9 (136-bit CID/CSD)
    R3,      // ACMD41 (no CRC)
    R6,      // CMD3 (RCA)
    R7,      // CMD8 (interface condition)
}

impl RespType {
    fn to_cmd_flags(self) -> u16 {
        match self {
            RespType::None => regs::RESP_NONE,
            RespType::R1 => regs::RESP_48 | regs::CMD_CRC_CHECK | regs::CMD_INDEX_CHECK,
            RespType::R1b => regs::RESP_48_BUSY | regs::CMD_CRC_CHECK | regs::CMD_INDEX_CHECK,
            RespType::R2 => regs::RESP_136 | regs::CMD_CRC_CHECK,
            RespType::R3 => regs::RESP_48, // no CRC/index check for OCR
            RespType::R6 => regs::RESP_48 | regs::CMD_CRC_CHECK | regs::CMD_INDEX_CHECK,
            RespType::R7 => regs::RESP_48 | regs::CMD_CRC_CHECK | regs::CMD_INDEX_CHECK,
        }
    }
}

/// SD/MMC command indices.
#[allow(dead_code)]
mod cmd {
    pub const GO_IDLE: u16 = 0;
    pub const ALL_SEND_CID: u16 = 2;
    pub const SEND_RELATIVE_ADDR: u16 = 3;
    pub const SELECT_CARD: u16 = 7;
    pub const SEND_IF_COND: u16 = 8;
    pub const SEND_CSD: u16 = 9;
    pub const SET_BLOCKLEN: u16 = 16;
    pub const READ_SINGLE_BLOCK: u16 = 17;
    pub const READ_MULTIPLE_BLOCK: u16 = 18;
    pub const SET_BLOCK_COUNT: u16 = 23;   // CMD23 — pre-defined multi-block count
    pub const WRITE_SINGLE_BLOCK: u16 = 24;
    pub const WRITE_MULTIPLE_BLOCK: u16 = 25;
    pub const APP_CMD: u16 = 55;
    pub const SD_SEND_OP_COND: u16 = 41;  // ACMD41
    pub const SET_BUS_WIDTH: u16 = 6;     // ACMD6
}

/// Decoded CSD register (Card-Specific Data).
#[derive(Default, Clone)]
pub struct CsdInfo {
    /// CSD structure version (0=v1, 1=v2 for SDHC/SDXC)
    pub csd_ver: u8,
    /// Max transfer speed (TRAN_SPEED raw byte)
    pub tran_speed: u8,
    /// Card capacity in 512-byte blocks
    pub capacity_blocks: u64,
    /// Card capacity in bytes
    pub capacity_bytes: u64,
    /// Max read block length (READ_BL_LEN, log2)
    pub read_bl_len: u8,
    /// Erase block enabled (1 = erase in units of 512 bytes)
    pub erase_blk_en: bool,
    /// Erase sector size (in write blocks)
    pub sector_size: u8,
    /// Command classes supported
    pub ccc: u16,
}

impl CsdInfo {
    /// Decode CSD from the 4 response words (as read from SDHCI RESPONSE regs).
    /// SDHCI shifts R2 responses right by 8 bits — the MSB of CSD is in resp[3] bits [23:0].
    pub fn from_response(resp: &[u32; 4]) -> Self {
        // SDHCI shifts R2 responses right by 8 bits. Reconstruct as u128.
        // CSD spec bit X → u128 bit (X - 8).
        let csd: u128 = (resp[3] as u128) << 96
            | (resp[2] as u128) << 64
            | (resp[1] as u128) << 32
            | (resp[0] as u128);

        let csd_ver = ((csd >> 118) & 0x3) as u8;       // [127:126]
        let tran_speed = ((csd >> 88) & 0xFF) as u8;     // [103:96]
        let ccc = ((csd >> 76) & 0xFFF) as u16;          // [95:84]
        let read_bl_len = ((csd >> 72) & 0xF) as u8;     // [83:80]

        let (capacity_blocks, capacity_bytes) = if csd_ver == 1 {
            // CSD v2 (SDHC/SDXC): C_SIZE is 22 bits at [69:48]
            let c_size = ((csd >> 40) & 0x3FFFFF) as u64;
            let blocks = (c_size + 1) * 1024; // in 512-byte sectors
            let bytes = blocks * 512;
            (blocks, bytes)
        } else {
            (0u64, 0u64)
        };

        let erase_blk_en = ((csd >> 38) & 1) == 1;       // [46]
        let sector_size = ((csd >> 31) & 0x7F) as u8;     // [45:39]

        Self {
            csd_ver,
            tran_speed,
            capacity_blocks,
            capacity_bytes,
            read_bl_len,
            erase_blk_en,
            sector_size,
            ccc,
        }
    }

    /// Decode TRAN_SPEED into MHz.
    pub fn max_freq_mhz(&self) -> u32 {
        let unit = match self.tran_speed & 0x7 {
            0 => 100,   // 100 KHz
            1 => 1000,  // 1 MHz
            2 => 10000, // 10 MHz
            3 => 100000,// 100 MHz
            _ => 0,
        };
        let mult = match (self.tran_speed >> 3) & 0xF {
            1 => 10,
            2 => 12,
            3 => 13,
            4 => 15,
            5 => 20,
            6 => 25, // 0x32 = unit=10MHz, mult=2.5 → 25MHz
            7 => 30,
            8 => 35,
            9 => 40,
            10 => 45,
            11 => 50,
            12 => 55,
            13 => 60,
            14 => 70,
            15 => 80,
            _ => 0,
        };
        (unit * mult) / 10000 // result in MHz
    }
}

/// Diagnostic probe results — each step logged individually.
#[derive(Default)]
pub struct ProbeResult {
    pub reset_ok: bool,
    pub present_state: u32,
    pub clock_ctrl: u16,
    pub caps: u32,
    pub cmd0_ok: bool,
    pub cmd8_ok: bool,
    pub cmd8_resp: u32,
    pub cmd8_err: u16,  // ERROR_INT_STATUS when CMD8 fails
    pub acmd41_ok: bool,
    pub acmd41_tries: u32,
    pub ocr: u32,
    pub cmd2_ok: bool,
    pub cmd2_err: u16,
    pub cid: [u32; 4],
    pub cmd3_ok: bool,
    pub cmd3_resp: u32,
    pub cmd3_err: u16,
    pub cmd7_ok: bool,
    pub cmd7_err: u16,
    pub cmd9_ok: bool,
    pub csd: [u32; 4],
    pub cmd16_ok: bool,
    pub bus4_ok: bool,
    pub clk25_ok: bool,
    pub read_ok: bool,
    pub read_err: u16,
    pub block0_head: [u8; 16],  // first 16 bytes of block 0
    pub block0_sig: [u8; 2],   // bytes 510-511 (MBR signature)
    pub write_ok: bool,
    pub write_err: u16,
    pub verify_ok: bool,
    pub verify_err: u16,
    pub pre_cmd2_present: u32,
    pub pre_cmd2_int: u16,
    pub pre_cmd2_pwrctl: u32,
    pub pwrctl_status_pre: u32,
    pub pwrctl_status_post: u32,
    pub pwrctl_ack_ok: bool,
}

/// Card identity from CID register (CMD2 response).
#[derive(Clone, Debug)]
pub struct CardIdentity {
    /// Manufacturer ID (MID).
    pub manufacturer_id: u8,
    /// OEM/Application ID.
    pub oem_id: [u8; 2],
    /// Product name (5 bytes ASCII).
    pub product_name: [u8; 5],
    /// Product revision.
    pub product_rev: u8,
    /// Serial number.
    pub serial: u32,
}

/// An SDHCI controller instance.
pub struct SdmmcController {
    /// MMIO base address of the SDHCI register block.
    base: usize,
    /// Whether the controller has been initialized and a card is present.
    initialized: bool,
    /// Card identity (populated after init).
    card_id: Option<CardIdentity>,
    /// Card capacity in bytes.
    capacity: u64,
    /// Relative Card Address (from CMD3).
    rca: u16,
    /// Device ID derived from card serial.
    device_id: DeviceId,
    /// Cached device info (populated during init).
    device_info: Option<DeviceInfo>,
}

impl SdmmcController {
    /// Create a new controller handle.
    ///
    /// Does NOT initialize the hardware — call `init()` first.
    pub fn new(base: usize) -> Self {
        Self {
            base,
            initialized: false,
            card_id: None,
            capacity: 0,
            rca: 0,
            device_id: DeviceId([0; 16]),
            device_info: None,
        }
    }

    /// Initialize the controller and probe for a card.
    ///
    /// Returns Ok(true) if a card is found, Ok(false) if slot is empty.
    pub fn init(&mut self) -> Result<bool, DeviceError> {
        // Step 0: GCC block reset + clock re-init for hot-reload recovery.
        crate::gcc::sdc2_block_reset();
        crate::gcc::sdc2_set_400khz();
        Self::delay(50_000);

        // Step 0.5: Clear any pending Qualcomm PWRCTL interrupts — SDHCI
        // SW_RESET may not complete while PWRCTL is pending.
        unsafe {
            let status = crate::mmio::read32(self.base + regs::PWRCTL_STATUS);
            if status != 0 {
                crate::mmio::write32(self.base + regs::PWRCTL_CLEAR, status);
                Self::delay(1000);
                crate::mmio::write32(self.base + regs::PWRCTL_CTL,
                    regs::PWRCTL_BUS_SUCCESS | regs::PWRCTL_IO_SUCCESS);
                Self::delay(1000);
            }
        }

        // Step 1: Software reset
        unsafe { crate::mmio::write8(self.base + regs::SW_RESET, 0x01) };
        // Wait for reset with PWRCTL polling — the Qualcomm wrapper generates
        // PWRCTL interrupts during reset that must be acknowledged.
        for _ in 0..100_000u32 {
            let val = unsafe { crate::mmio::read8(self.base + regs::SW_RESET) };
            if val & 0x01 == 0 {
                break;
            }
            // Check for PWRCTL during reset
            let pwr = unsafe { crate::mmio::read32(self.base + regs::PWRCTL_STATUS) };
            if pwr != 0 {
                unsafe {
                    crate::mmio::write32(self.base + regs::PWRCTL_CLEAR, pwr);
                    crate::mmio::write32(self.base + regs::PWRCTL_CTL,
                        regs::PWRCTL_BUS_SUCCESS | regs::PWRCTL_IO_SUCCESS);
                }
            }
        }

        // Step 2: Enable all interrupt status signals
        unsafe {
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS_EN, 0x7FFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS_EN, 0xFFFF);
        }

        // Step 3: Set timeout to max
        unsafe { crate::mmio::write8(self.base + regs::TIMEOUT_CTRL, 0x0E) };

        // Step 4: Power on (3.3V)
        unsafe { crate::mmio::write8(self.base + regs::POWER_CTRL, 0x0F) };

        // Step 5: Set clock to 400KHz for identification
        self.set_clock_raw(400);

        // Small delay for power + clock stabilization
        Self::delay(100_000);

        // Step 6: Card identification sequence
        self.send_cmd_raw(cmd::GO_IDLE, 0, RespType::None)?;

        // CMD8: voltage check
        let cmd8_ok = self.send_cmd_raw(cmd::SEND_IF_COND, 0x000001AA, RespType::R7);
        let _sdhc = cmd8_ok.is_ok(); // SD v2+ if CMD8 succeeds

        // Step 7: ACMD41 loop — wait for card to be ready (up to 1s)
        let mut ocr = 0u32;
        for _ in 0..1000 {
            // CMD55 (APP_CMD) with RCA=0 during init
            self.send_cmd_raw(cmd::APP_CMD, 0, RespType::R1)?;
            // ACMD41: HCS=1 (bit 30), voltage window 2.7-3.6V
            self.send_cmd_raw(cmd::SD_SEND_OP_COND, 0x40FF8000, RespType::R3)?;
            ocr = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
            if ocr & (1 << 31) != 0 {
                break;
            }
            Self::delay(500_000);
        }
        if ocr & (1 << 31) == 0 {
            return Ok(false); // Card never became ready
        }

        // Step 8: Get CID
        self.send_cmd_raw(cmd::ALL_SEND_CID, 0, RespType::R2)?;
        self.parse_cid();

        // Step 9: Get RCA
        self.send_cmd_raw(cmd::SEND_RELATIVE_ADDR, 0, RespType::R6)?;
        self.rca = (unsafe { crate::mmio::read32(self.base + regs::RESPONSE) } >> 16) as u16;

        // Step 10: Select card
        self.send_cmd_raw(cmd::SELECT_CARD, (self.rca as u32) << 16, RespType::R1b)?;

        // Step 11: Set block length to 512
        self.send_cmd_raw(cmd::SET_BLOCKLEN, 512, RespType::R1)?;

        // Step 12: Switch to 25MHz for data transfer
        self.set_clock_raw(25_000);

        self.device_info = Some(DeviceInfo {
            id: self.device_id,
            capacity: self.capacity,
            vendor: Vec::new(),
            model: Vec::new(),
            atomic_write_size: Some(512),
            has_volatile_cache: false,
        });

        self.initialized = true;
        Ok(true)
    }

    /// Diagnostic probe: try to talk to the card, returning step-by-step
    /// results as (step_name, ok, extra_data).
    ///
    /// This doesn't modify `self` state — it's a read-only probe for
    /// logging to the boot console.
    pub fn probe_card(&mut self) -> ProbeResult {
        let mut r = ProbeResult::default();

        // Software reset
        unsafe { crate::mmio::write8(self.base + regs::SW_RESET, 0x01) };
        for _ in 0..10_000u32 {
            if unsafe { crate::mmio::read8(self.base + regs::SW_RESET) } & 0x01 == 0 {
                r.reset_ok = true;
                break;
            }
        }
        if !r.reset_ok { return r; }

        // Qualcomm vendor init: write POR value to VENDOR_SPEC
        unsafe { crate::mmio::write32(self.base + regs::VENDOR_SPEC, regs::VENDOR_SPEC_POR_VAL) };

        // Clear any pending power control IRQ from reset
        r.pwrctl_status_pre = unsafe { crate::mmio::read32(self.base + regs::PWRCTL_STATUS) };
        self.pwrctl_clear_pending();

        // Enable power control IRQ mask (all bits)
        unsafe { crate::mmio::write32(self.base + regs::PWRCTL_MASK, 0x0F) };

        // Enable status signals
        unsafe {
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS_EN, 0x7FFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS_EN, 0xFFFF);
            crate::mmio::write8(self.base + regs::TIMEOUT_CTRL, 0x0E);
        }

        // Power on 3.3V — this triggers a Qualcomm power control IRQ
        unsafe { crate::mmio::write8(self.base + regs::POWER_CTRL, 0x0F) };

        // ACK the power control IRQ
        Self::delay(1_000);
        r.pwrctl_status_post = unsafe { crate::mmio::read32(self.base + regs::PWRCTL_STATUS) };
        r.pwrctl_ack_ok = self.pwrctl_clear_pending();

        // Clock 400KHz
        self.set_clock_raw(400);

        Self::delay(100_000);

        r.present_state = unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) };
        r.clock_ctrl = unsafe { crate::mmio::read16(self.base + regs::CLOCK_CTRL) };
        r.caps = unsafe { crate::mmio::read32(self.base + regs::CAPABILITIES) };

        // CMD0 — GO_IDLE (no response expected)
        r.cmd0_ok = self.send_cmd_raw(cmd::GO_IDLE, 0, RespType::None).is_ok();
        if !r.cmd0_ok { return r; }

        // CMD8 — SEND_IF_COND
        match self.send_cmd_raw(cmd::SEND_IF_COND, 0x1AA, RespType::R7) {
            Ok(resp) => {
                r.cmd8_ok = true;
                r.cmd8_resp = resp;
            }
            Err(_) => {
                r.cmd8_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
            }
        }

        // ACMD41 loop — card can take up to 1s to power up.
        // SD spec requires >= 1ms between retries.
        for i in 0..1000u32 {
            let c55 = self.send_cmd_raw(cmd::APP_CMD, 0, RespType::R1);
            if c55.is_err() { break; }
            // HCS (bit 30) = SDHC/SDXC support
            // Voltage window 2.7-3.6V (bits 20:15)
            let c41 = self.send_cmd_raw(cmd::SD_SEND_OP_COND, 0x40FF8000, RespType::R3);
            if c41.is_err() { break; }
            r.ocr = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
            r.acmd41_tries = i + 1;
            if r.ocr & (1 << 31) != 0 {
                r.acmd41_ok = true;
                break;
            }
            // ~1ms delay (conservative — 500K nops at ~1GHz ≈ 0.5ms)
            Self::delay(500_000);
        }
        if !r.acmd41_ok { return r; }

        // Check for pending power IRQ after voltage negotiation
        self.pwrctl_clear_pending();

        // CMD-line reset to clear command engine state after ACMD41 loop.
        // This only resets the command state machine, not the card.
        unsafe {
            crate::mmio::write8(self.base + regs::SW_RESET, 0x02);
            for _ in 0..10_000u32 {
                if crate::mmio::read8(self.base + regs::SW_RESET) & 0x02 == 0 { break; }
            }
            // Re-enable interrupt status after CMD reset
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS_EN, 0x7FFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS_EN, 0xFFFF);
        }

        // Capture state before CMD2
        r.pre_cmd2_present = unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) };
        r.pre_cmd2_int = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
        r.pre_cmd2_pwrctl = unsafe { crate::mmio::read32(self.base + regs::PWRCTL_STATUS) };

        // CMD2 — ALL_SEND_CID
        match self.send_cmd_raw(cmd::ALL_SEND_CID, 0, RespType::R2) {
            Ok(_) => {
                r.cid[0] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
                r.cid[1] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 4) };
                r.cid[2] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 8) };
                r.cid[3] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 12) };
                r.cmd2_ok = true;
            }
            Err(_) => {
                r.cmd2_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
            }
        }

        // CMD3 — SEND_RELATIVE_ADDR
        match self.send_cmd_raw(cmd::SEND_RELATIVE_ADDR, 0, RespType::R6) {
            Ok(_) => {
                r.cmd3_resp = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
                r.cmd3_ok = true;
            }
            Err(_) => {
                r.cmd3_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
                return r;
            }
        }

        let rca = r.cmd3_resp & 0xFFFF0000; // RCA in upper 16 bits

        // CMD9 — SEND_CSD (card speed/voltage/capacity data, R2 response)
        // Must be sent while card is in standby state (before CMD7), OR
        // can be sent to addressed card. We send with RCA.
        match self.send_cmd_raw(cmd::SEND_CSD, rca, RespType::R2) {
            Ok(_) => {
                r.cmd9_ok = true;
                r.csd[0] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
                r.csd[1] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 4) };
                r.csd[2] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 8) };
                r.csd[3] = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 12) };
            }
            Err(_) => {}
        }

        // CMD7 — SELECT_CARD (transitions card to transfer state)
        match self.send_cmd_raw(cmd::SELECT_CARD, rca, RespType::R1b) {
            Ok(_) => r.cmd7_ok = true,
            Err(_) => {
                r.cmd7_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
                return r;
            }
        }

        // CMD16 — SET_BLOCKLEN to 512 (required for standard capacity, good practice)
        r.cmd16_ok = self.send_cmd_raw(cmd::SET_BLOCKLEN, 512, RespType::R1).is_ok();

        // Switch to 4-bit bus width: ACMD6 (SET_BUS_WIDTH) arg=2
        if self.send_cmd_raw(cmd::APP_CMD, rca, RespType::R1).is_ok() {
            if self.send_cmd_raw(cmd::SET_BUS_WIDTH, 2, RespType::R1).is_ok() {
                // Tell the host controller to use 4-bit mode
                unsafe {
                    let hc = crate::mmio::read8(self.base + regs::HOST_CTRL1);
                    crate::mmio::write8(self.base + regs::HOST_CTRL1, hc | regs::HOST_4BIT);
                }
                r.bus4_ok = true;
            }
        }

        // Stay at 400KHz for now — 25MHz needs DLL tuning investigation
        // Just test 4-bit bus width at the current clock rate
        r.clk25_ok = false; // not switching yet

        // Read block 0 — MBR/GPT signature area
        // For SDHC/SDXC (CCS=1), block address is in 512-byte units
        self.pwrctl_clear_pending();
        match self.read_block_raw(0) {
            Ok(buf) => {
                r.read_ok = true;
                r.block0_head[..16].copy_from_slice(&buf[..16]);
                r.block0_sig = [buf[510], buf[511]];
            }
            Err(_) => {
                r.read_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
            }
        }

        // Write-read-verify test on block 1 (avoid block 0 MBR)
        // Write a recognizable pattern, read it back, verify
        if r.read_ok {
            match self.write_block_raw(1, &Self::test_pattern()) {
                Ok(()) => {
                    r.write_ok = true;
                    // Read it back
                    match self.read_block_raw(1) {
                        Ok(buf) => {
                            let pattern = Self::test_pattern();
                            r.verify_ok = buf[..] == pattern[..];
                            if !r.verify_ok {
                                r.verify_err = 0xFFFF; // mismatch marker
                            }
                        }
                        Err(_) => {
                            r.verify_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
                        }
                    }
                }
                Err(_) => {
                    r.write_err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
                }
            }
        }

        // Mark as initialized if card was successfully selected
        if r.cmd7_ok {
            self.initialized = true;
        }

        r
    }

    /// Raw block read for probe — doesn't require `self.initialized`.
    fn read_block_raw(&self, block_addr: u32) -> Result<[u8; 512], DeviceError> {
        // Wait for DAT line free
        for _ in 0..100_000u32 {
            if unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::DAT_INHIBIT == 0 { break; }
        }

        unsafe {
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, 0xFFFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS, 0xFFFF);

            // Block size=512, count=1
            crate::mmio::write32(self.base + regs::BLOCK_SIZE, 512 | (1 << 16));

            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);

            // CMD17 (READ_SINGLE_BLOCK) with DATA_PRESENT, R1 response
            // Transfer mode: read (bit 4), single block
            let xfer_mode: u16 = 1 << 4; // data direction = read
            let cmd_reg: u16 = (cmd::READ_SINGLE_BLOCK << 8)
                | regs::CMD_DATA_PRESENT
                | RespType::R1.to_cmd_flags();
            // Combined 32-bit write: transfer_mode | (command << 16)
            crate::mmio::write32(
                self.base + regs::TRANSFER_MODE,
                (cmd_reg as u32) << 16 | xfer_mode as u32,
            );
        }

        // Wait for command complete
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::ERR_INTERRUPT != 0 {
                return Err(DeviceError::IoError(DeviceIoKind::HardwareFailure));
            }
            if status & regs::CMD_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::CMD_COMPLETE) };
                break;
            }
        }

        // Wait for buffer read ready
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::ERR_INTERRUPT != 0 {
                return Err(DeviceError::IoError(DeviceIoKind::HardwareFailure));
            }
            if status & regs::BUF_READ_READY != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::BUF_READ_READY) };
                break;
            }
        }

        // Read 512 bytes from buffer data port
        let mut buf = [0u8; 512];
        for i in 0..128 {
            let word = unsafe { crate::mmio::read32(self.base + regs::BUFFER_DATA) };
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
        }

        // Wait for transfer complete
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE) };
                return Ok(buf);
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Raw block write for probe — doesn't require `self.initialized`.
    fn write_block_raw(&self, block_addr: u32, data: &[u8; 512]) -> Result<(), DeviceError> {
        for _ in 0..100_000u32 {
            if unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::DAT_INHIBIT == 0 { break; }
        }

        unsafe {
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, 0xFFFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS, 0xFFFF);

            crate::mmio::write32(self.base + regs::BLOCK_SIZE, 512 | (1 << 16));
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);

            // CMD24 (WRITE_SINGLE_BLOCK) with DATA_PRESENT, R1 response
            // Transfer mode: write (bit 4 = 0), single block
            let xfer_mode: u16 = 0; // data direction = write
            let cmd_reg: u16 = (cmd::WRITE_SINGLE_BLOCK << 8)
                | regs::CMD_DATA_PRESENT
                | RespType::R1.to_cmd_flags();
            crate::mmio::write32(
                self.base + regs::TRANSFER_MODE,
                (cmd_reg as u32) << 16 | xfer_mode as u32,
            );
        }

        // Wait for command complete
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::ERR_INTERRUPT != 0 {
                return Err(DeviceError::IoError(DeviceIoKind::HardwareFailure));
            }
            if status & regs::CMD_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::CMD_COMPLETE) };
                break;
            }
        }

        // Wait for buffer write ready
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::ERR_INTERRUPT != 0 {
                return Err(DeviceError::IoError(DeviceIoKind::HardwareFailure));
            }
            if status & regs::BUF_WRITE_READY != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::BUF_WRITE_READY) };
                break;
            }
        }

        // Write 512 bytes
        for i in 0..128 {
            let off = i * 4;
            let word = u32::from_le_bytes([
                data[off], data[off + 1], data[off + 2], data[off + 3],
            ]);
            unsafe { crate::mmio::write32(self.base + regs::BUFFER_DATA, word) };
        }

        // Wait for transfer complete
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Generate a recognizable 512-byte test pattern.
    fn test_pattern() -> [u8; 512] {
        let mut buf = [0u8; 512];
        // "FERROS" magic + incrementing bytes
        buf[0..6].copy_from_slice(b"FERROS");
        for i in 6..512 {
            buf[i] = (i & 0xFF) as u8;
        }
        buf
    }

    /// Clear pending Qualcomm power control IRQ and ACK success.
    /// Returns true if there was a pending IRQ that was cleared.
    fn pwrctl_clear_pending(&self) -> bool {
        let status = unsafe { crate::mmio::read32(self.base + regs::PWRCTL_STATUS) };
        if status == 0 {
            return false;
        }
        // Clear the pending bits
        unsafe { crate::mmio::write32(self.base + regs::PWRCTL_CLEAR, status) };
        // Poll until cleared
        for _ in 0..10_000u32 {
            if unsafe { crate::mmio::read32(self.base + regs::PWRCTL_STATUS) } == 0 {
                break;
            }
        }
        // ACK success (BUS_SUCCESS | IO_SUCCESS)
        unsafe {
            crate::mmio::write32(
                self.base + regs::PWRCTL_CTL,
                regs::PWRCTL_BUS_SUCCESS | regs::PWRCTL_IO_SUCCESS,
            );
        }
        true
    }

    fn delay(iters: u32) {
        for _ in 0..iters {
            unsafe { core::arch::asm!("nop") };
        }
    }

    /// Enable SDHCI clock output — Qualcomm bypass mode (no internal divider).
    ///
    /// On sdhci-msm, the clock frequency is set entirely by the GCC RCG.
    /// The SDHCI clock divider is unused — we just enable INT_EN + CARD_EN
    /// with divider=0 so the GCC clock passes straight through.
    fn set_clock_raw(&self, _khz: u32) {
        unsafe {
            // 1. Disable everything
            crate::mmio::write16(self.base + regs::CLOCK_CTRL, 0);

            // 2. Enable internal clock (bit 0), divider=0
            crate::mmio::write16(self.base + regs::CLOCK_CTRL, 1 << 0);

            // 3. Wait for internal clock stable (bit 1)
            for _ in 0..500_000u32 {
                if crate::mmio::read16(self.base + regs::CLOCK_CTRL) & (1 << 1) != 0 {
                    break;
                }
            }

            // 4. Enable SD clock output (bit 2)
            let clk = crate::mmio::read16(self.base + regs::CLOCK_CTRL);
            crate::mmio::write16(self.base + regs::CLOCK_CTRL, clk | (1 << 2));
        }
    }

    fn send_cmd_raw(&self, cmd_idx: u16, arg: u32, resp: RespType) -> Result<u32, DeviceError> {
        // Wait for CMD line to be free
        for _ in 0..100_000u32 {
            if unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::CMD_INHIBIT == 0 {
                break;
            }
        }

        unsafe {
            // Clear any pending interrupt status
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, 0xFFFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS, 0xFFFF);

            // Set argument
            crate::mmio::write32(self.base + regs::ARGUMENT, arg);

            // Build command register value
            // [13:8] = command index, [5:0] = flags (response type, CRC, index check)
            let cmd_reg: u16 = (cmd_idx << 8) | resp.to_cmd_flags();

            // Linux sdhci.c always writes COMMAND + TRANSFER_MODE as a combined
            // 32-bit write to offset 0x0C. Some controllers require this.
            // Low 16 bits = TRANSFER_MODE (0 for non-data), high 16 bits = COMMAND.
            crate::mmio::write32(self.base + regs::TRANSFER_MODE, (cmd_reg as u32) << 16);
        }

        // Wait for command complete or error.
        // At 400KHz, a 136-bit R2 response takes ~0.5ms. With CPU loop
        // overhead (~3-5 cycles/iter at 1GHz), we need a generous timeout.
        for _ in 0..5_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::ERR_INTERRUPT != 0 {
                let _err = unsafe { crate::mmio::read16(self.base + regs::ERROR_INT_STATUS) };
                // Clear errors
                unsafe {
                    crate::mmio::write16(self.base + regs::ERROR_INT_STATUS, 0xFFFF);
                    crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, 0xFFFF);
                    // CMD line reset
                    crate::mmio::write8(self.base + regs::SW_RESET, 0x02);
                }
                for _ in 0..10_000u32 {
                    if unsafe { crate::mmio::read8(self.base + regs::SW_RESET) } & 0x02 == 0 { break; }
                }
                return Err(DeviceError::IoError(DeviceIoKind::HardwareFailure));
            }
            if status & regs::CMD_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::CMD_COMPLETE) };
                let resp0 = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
                return Ok(resp0);
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    fn parse_cid(&mut self) {
        // CID is 128 bits in RESPONSE registers [0x10..0x1F]
        let r0 = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
        let r1 = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 4) };
        let r2 = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 8) };
        let r3 = unsafe { crate::mmio::read32(self.base + regs::RESPONSE + 12) };

        let serial = r0;
        let mid = ((r3 >> 24) & 0xFF) as u8;

        // Build DeviceId from card serial + manufacturer
        let mut did = [0u8; 16];
        did[0..4].copy_from_slice(&serial.to_le_bytes());
        did[4] = mid;
        self.device_id = DeviceId(did);

        self.card_id = Some(CardIdentity {
            manufacturer_id: mid,
            oem_id: [((r3 >> 16) & 0xFF) as u8, ((r3 >> 8) & 0xFF) as u8],
            product_name: [
                (r3 & 0xFF) as u8,
                ((r2 >> 24) & 0xFF) as u8,
                ((r2 >> 16) & 0xFF) as u8,
                ((r2 >> 8) & 0xFF) as u8,
                (r2 & 0xFF) as u8,
            ],
            product_rev: ((r1 >> 24) & 0xFF) as u8,
            serial,
        });
    }

    /// Read a 512-byte block from the card.
    pub fn read_block(&self, block_addr: u32, buf: &mut [u8; 512]) -> Result<(), DeviceError> {
        if !self.initialized {
            return Err(DeviceError::NotReady);
        }

        // Wait for both CMD and DAT lines free
        for _ in 0..100_000u32 {
            let ps = unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) };
            if ps & (regs::CMD_INHIBIT | regs::DAT_INHIBIT) == 0 { break; }
        }

        unsafe {
            // Clear stale interrupt status
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, 0xFFFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS, 0xFFFF);

            crate::mmio::write16(self.base + regs::BLOCK_SIZE, 512);
            crate::mmio::write16(self.base + regs::BLOCK_COUNT, 1);
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);
            let xfer_mode: u16 = 1 << 4; // read direction
            let cmd_reg: u16 = (cmd::READ_SINGLE_BLOCK << 8) | regs::CMD_DATA_PRESENT | RespType::R1.to_cmd_flags();
            crate::mmio::write32(self.base + regs::TRANSFER_MODE, (cmd_reg as u32) << 16 | xfer_mode as u32);
        }

        // Wait for buffer read ready
        for _ in 0..1_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::BUF_READ_READY != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::BUF_READ_READY) };
                break;
            }
        }

        // Read 512 bytes from buffer data port (128 x 32-bit reads)
        for i in 0..128 {
            let word = unsafe { crate::mmio::read32(self.base + regs::BUFFER_DATA) };
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
        }

        // Wait for transfer complete
        for _ in 0..100_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Write a 512-byte block to the card.
    pub fn write_block(&mut self, block_addr: u32, data: &[u8; 512]) -> Result<(), DeviceError> {
        if !self.initialized {
            return Err(DeviceError::NotReady);
        }

        // Wait for both CMD and DAT lines free
        for _ in 0..100_000u32 {
            let ps = unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) };
            if ps & (regs::CMD_INHIBIT | regs::DAT_INHIBIT) == 0 { break; }
        }

        unsafe {
            // Clear stale interrupt status
            crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, 0xFFFF);
            crate::mmio::write16(self.base + regs::ERROR_INT_STATUS, 0xFFFF);

            crate::mmio::write16(self.base + regs::BLOCK_SIZE, 512);
            crate::mmio::write16(self.base + regs::BLOCK_COUNT, 1);
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);
            let xfer_mode: u16 = 0; // write direction
            let cmd_reg: u16 = (cmd::WRITE_SINGLE_BLOCK << 8) | regs::CMD_DATA_PRESENT | RespType::R1.to_cmd_flags();
            crate::mmio::write32(self.base + regs::TRANSFER_MODE, (cmd_reg as u32) << 16 | xfer_mode as u32);
        }

        // Wait for buffer write ready
        for _ in 0..1_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::BUF_WRITE_READY != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::BUF_WRITE_READY) };
                break;
            }
        }

        // Write 512 bytes
        for i in 0..128 {
            let off = i * 4;
            let word = u32::from_le_bytes([
                data[off], data[off + 1], data[off + 2], data[off + 3],
            ]);
            unsafe { crate::mmio::write32(self.base + regs::BUFFER_DATA, word) };
        }

        // Wait for transfer complete
        for _ in 0..100_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Read multiple 512-byte blocks using CMD23 + CMD18.
    /// `buf` must be exactly `count * 512` bytes.
    pub fn read_blocks(&self, block_addr: u32, buf: &mut [u8], count: u16) -> Result<(), DeviceError> {
        if !self.initialized { return Err(DeviceError::NotReady); }
        if buf.len() < count as usize * 512 { return Err(DeviceError::IoError(DeviceIoKind::InvalidParam)); }

        // CMD23: SET_BLOCK_COUNT (optional, skip if unsupported)
        let _ = self.send_cmd_raw(cmd::SET_BLOCK_COUNT, count as u32, RespType::R1);

        // Wait for DAT line free
        for _ in 0..100_000u32 {
            if unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::DAT_INHIBIT == 0 { break; }
        }

        unsafe {
            crate::mmio::write16(self.base + regs::BLOCK_SIZE, 512);
            crate::mmio::write16(self.base + regs::BLOCK_COUNT, count);
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);
            // Combined 32-bit write: TRANSFER_MODE (low 16) + COMMAND (high 16)
            let xfer_mode: u16 = (1 << 5) | (1 << 4) | (1 << 1); // multi, read, blk_cnt_en
            let cmd_reg: u16 = (cmd::READ_MULTIPLE_BLOCK << 8) | regs::CMD_DATA_PRESENT | RespType::R1.to_cmd_flags();
            crate::mmio::write32(self.base + regs::TRANSFER_MODE, (cmd_reg as u32) << 16 | xfer_mode as u32);
        }

        // Read blocks one at a time from the buffer data port
        for blk in 0..count as usize {
            // Wait for buffer read ready
            for _ in 0..1_000_000u32 {
                let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
                if status & regs::ERR_INTERRUPT != 0 {
                    return Err(DeviceError::IoError(DeviceIoKind::ReadError));
                }
                if status & regs::BUF_READ_READY != 0 {
                    unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::BUF_READ_READY) };
                    break;
                }
            }
            // Read 512 bytes
            let base_off = blk * 512;
            for i in 0..128 {
                let word = unsafe { crate::mmio::read32(self.base + regs::BUFFER_DATA) };
                let off = base_off + i * 4;
                buf[off..off + 4].copy_from_slice(&word.to_le_bytes());
            }
        }

        // Wait for transfer complete
        for _ in 0..1_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Write multiple 512-byte blocks using CMD23 + CMD25.
    /// `data` must be exactly `count * 512` bytes.
    pub fn write_blocks(&mut self, block_addr: u32, data: &[u8], count: u16) -> Result<(), DeviceError> {
        if !self.initialized { return Err(DeviceError::NotReady); }
        if data.len() < count as usize * 512 { return Err(DeviceError::IoError(DeviceIoKind::InvalidParam)); }

        // CMD23: SET_BLOCK_COUNT (optional, skip if unsupported)
        let _ = self.send_cmd_raw(cmd::SET_BLOCK_COUNT, count as u32, RespType::R1);

        for _ in 0..100_000u32 {
            if unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::DAT_INHIBIT == 0 { break; }
        }

        unsafe {
            crate::mmio::write16(self.base + regs::BLOCK_SIZE, 512);
            crate::mmio::write16(self.base + regs::BLOCK_COUNT, count);
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);
            // Combined 32-bit write: TRANSFER_MODE (low 16) + COMMAND (high 16)
            let xfer_mode: u16 = (1 << 5) | (1 << 1); // multi, write, blk_cnt_en
            let cmd_reg: u16 = (cmd::WRITE_MULTIPLE_BLOCK << 8) | regs::CMD_DATA_PRESENT | RespType::R1.to_cmd_flags();
            crate::mmio::write32(self.base + regs::TRANSFER_MODE, (cmd_reg as u32) << 16 | xfer_mode as u32);
        }

        // Write blocks one at a time
        for blk in 0..count as usize {
            for _ in 0..1_000_000u32 {
                let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
                if status & regs::ERR_INTERRUPT != 0 {
                    return Err(DeviceError::IoError(DeviceIoKind::WriteError));
                }
                if status & regs::BUF_WRITE_READY != 0 {
                    unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::BUF_WRITE_READY) };
                    break;
                }
            }
            let base_off = blk * 512;
            for i in 0..128 {
                let off = base_off + i * 4;
                let word = u32::from_le_bytes([
                    data[off], data[off + 1], data[off + 2], data[off + 3],
                ]);
                unsafe { crate::mmio::write32(self.base + regs::BUFFER_DATA, word) };
            }
        }

        for _ in 0..1_000_000u32 {
            let status = unsafe { crate::mmio::read16(self.base + regs::NORMAL_INT_STATUS) };
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write16(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Decode the CSD register from the probe's raw response.
    pub fn decode_csd(&self, csd_raw: &[u32; 4]) -> CsdInfo {
        CsdInfo::from_response(csd_raw)
    }
}

/// Implement the Ledger's Device trait for the SD/MMC controller.
///
/// The Device trait wants arbitrary byte-range access. The SD card
/// works in 512-byte blocks. We translate: read the block(s) containing
/// the requested range, copy out the relevant bytes.
impl Device for SdmmcController {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        if offset + buf.len() as u64 > self.capacity {
            return Err(DeviceError::OutOfBounds {
                offset,
                len: buf.len() as u64,
                capacity: self.capacity,
            });
        }

        let mut remaining = buf.len();
        let mut buf_pos = 0;
        let mut byte_offset = offset;

        while remaining > 0 {
            let block_addr = (byte_offset / 512) as u32;
            let block_offset = (byte_offset % 512) as usize;
            let copy_len = core::cmp::min(remaining, 512 - block_offset);

            let mut block_buf = [0u8; 512];
            self.read_block(block_addr, &mut block_buf)?;

            buf[buf_pos..buf_pos + copy_len]
                .copy_from_slice(&block_buf[block_offset..block_offset + copy_len]);

            buf_pos += copy_len;
            byte_offset += copy_len as u64;
            remaining -= copy_len;
        }

        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), DeviceError> {
        if offset + data.len() as u64 > self.capacity {
            return Err(DeviceError::OutOfBounds {
                offset,
                len: data.len() as u64,
                capacity: self.capacity,
            });
        }

        let mut remaining = data.len();
        let mut data_pos = 0;
        let mut byte_offset = offset;

        while remaining > 0 {
            let block_addr = (byte_offset / 512) as u32;
            let block_offset = (byte_offset % 512) as usize;
            let copy_len = core::cmp::min(remaining, 512 - block_offset);

            let mut block_buf = [0u8; 512];

            // Read-modify-write if partial block
            if block_offset != 0 || copy_len < 512 {
                self.read_block(block_addr, &mut block_buf)?;
            }

            block_buf[block_offset..block_offset + copy_len]
                .copy_from_slice(&data[data_pos..data_pos + copy_len]);

            self.write_block(block_addr, &block_buf)?;

            data_pos += copy_len;
            byte_offset += copy_len as u64;
            remaining -= copy_len;
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<(), DeviceError> {
        // SD cards don't have a volatile write cache in the SDHCI sense.
        // The card controller handles write completion internally.
        Ok(())
    }

    fn info(&self) -> &DeviceInfo {
        self.device_info.as_ref().expect("SdmmcController::init() must be called before info()")
    }
}
