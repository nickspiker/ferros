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

use ferros_ledger::device::{Device, DeviceError, DeviceId, DeviceInfo, DeviceIoKind};

/// Standard SDHCI register offsets (from SD Host Controller Spec v3.00).
#[allow(dead_code)]
mod regs {
    pub const SDMA_ADDR: usize = 0x00;
    pub const BLOCK_SIZE: usize = 0x04;
    pub const BLOCK_COUNT: usize = 0x06;
    pub const ARGUMENT: usize = 0x08;
    pub const TRANSFER_MODE: usize = 0x0C;
    pub const COMMAND: usize = 0x0E;
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
    pub const CAPABILITIES: usize = 0x40;

    // Present State bits
    pub const CMD_INHIBIT: u32 = 1 << 0;
    pub const DAT_INHIBIT: u32 = 1 << 1;
    pub const CARD_INSERTED: u32 = 1 << 16;

    // Normal interrupt status bits
    pub const CMD_COMPLETE: u16 = 1 << 0;
    pub const XFER_COMPLETE: u16 = 1 << 1;
    pub const BUF_READ_READY: u16 = 1 << 5;
    pub const BUF_WRITE_READY: u16 = 1 << 4;
}

/// SD/MMC command indices.
#[allow(dead_code)]
mod cmd {
    pub const GO_IDLE: u16 = 0;
    pub const ALL_SEND_CID: u16 = 2;
    pub const SEND_RELATIVE_ADDR: u16 = 3;
    pub const SELECT_CARD: u16 = 7;
    pub const SEND_IF_COND: u16 = 8;
    pub const SET_BLOCKLEN: u16 = 16;
    pub const READ_SINGLE_BLOCK: u16 = 17;
    pub const WRITE_SINGLE_BLOCK: u16 = 24;
    pub const APP_CMD: u16 = 55;
    pub const SD_SEND_OP_COND: u16 = 41; // ACMD41
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
        // Step 1: Check card presence
        let state = unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) };
        if state & regs::CARD_INSERTED == 0 {
            return Ok(false);
        }

        // Step 2: Software reset
        unsafe { crate::mmio::write8(self.base + regs::SW_RESET, 0x01) }; // reset all
        self.wait_reset()?;

        // Step 3: Set clock to 400KHz for identification
        self.set_clock(400_000)?;

        // Step 4: Power on (3.3V)
        unsafe { crate::mmio::write8(self.base + regs::POWER_CTRL, 0x0F) }; // SD bus power on, 3.3V

        // Step 5: Card identification sequence
        self.send_cmd(cmd::GO_IDLE, 0)?;
        self.send_cmd(cmd::SEND_IF_COND, 0x000001AA)?; // voltage 2.7-3.6V, check pattern

        // Step 6: ACMD41 loop — wait for card to be ready
        for _ in 0..1000 {
            self.send_cmd(cmd::APP_CMD, 0)?;
            self.send_cmd(cmd::SD_SEND_OP_COND, 0x40FF8000)?; // HCS=1, voltage window
            let resp = unsafe { crate::mmio::read32(self.base + regs::RESPONSE) };
            if resp & (1 << 31) != 0 {
                // Card is ready
                break;
            }
        }

        // Step 7: Get CID
        self.send_cmd(cmd::ALL_SEND_CID, 0)?;
        self.parse_cid();

        // Step 8: Get RCA
        self.send_cmd(cmd::SEND_RELATIVE_ADDR, 0)?;
        self.rca = (unsafe { crate::mmio::read32(self.base + regs::RESPONSE) } >> 16) as u16;

        // Step 9: Select card
        self.send_cmd(cmd::SELECT_CARD, (self.rca as u32) << 16)?;

        // Step 10: Set block length to 512
        self.send_cmd(cmd::SET_BLOCKLEN, 512)?;

        // Step 11: Switch to 25MHz for data transfer
        self.set_clock(25_000_000)?;

        self.device_info = Some(DeviceInfo {
            id: self.device_id,
            capacity: self.capacity,
            vendor: Vec::new(), // TODO: extract from CID
            model: Vec::new(),  // TODO: extract from CID
            atomic_write_size: Some(512),
            has_volatile_cache: false,
        });

        self.initialized = true;
        Ok(true)
    }

    fn wait_reset(&self) -> Result<(), DeviceError> {
        for _ in 0..10_000 {
            let val = unsafe { crate::mmio::read8(self.base + regs::SW_RESET) };
            if val & 0x01 == 0 {
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    fn set_clock(&self, _hz: u32) -> Result<(), DeviceError> {
        // TODO: Calculate divisor from base clock capability register.
        // For now, just enable internal clock and SD clock.
        unsafe {
            crate::mmio::write32(self.base + regs::CLOCK_CTRL,
                (1 << 0) |  // internal clock enable
                (1 << 2)     // SD clock enable
            );
        }
        Ok(())
    }

    fn send_cmd(&self, cmd_idx: u16, arg: u32) -> Result<(), DeviceError> {
        // Wait for CMD line to be free
        while unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::CMD_INHIBIT != 0 {}

        unsafe {
            crate::mmio::write32(self.base + regs::ARGUMENT, arg);
            // Command register: index in bits [13:8], response type in [1:0]
            let cmd_reg = (cmd_idx << 8) | 0x1A; // R1 response, CRC check, index check
            crate::mmio::write32(self.base + regs::COMMAND, cmd_reg as u32);
        }

        // Wait for command complete
        for _ in 0..100_000 {
            let status = unsafe { crate::mmio::read32(self.base + regs::NORMAL_INT_STATUS) };
            if status as u16 & regs::CMD_COMPLETE != 0 {
                // Clear the flag
                unsafe { crate::mmio::write32(self.base + regs::NORMAL_INT_STATUS, regs::CMD_COMPLETE as u32) };
                return Ok(());
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
    fn read_block(&self, block_addr: u32, buf: &mut [u8; 512]) -> Result<(), DeviceError> {
        if !self.initialized {
            return Err(DeviceError::NotReady);
        }

        // Wait for DAT line free
        while unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::DAT_INHIBIT != 0 {}

        // Set up for single block read
        unsafe {
            crate::mmio::write32(self.base + regs::BLOCK_SIZE, 512);
            crate::mmio::write32(self.base + regs::BLOCK_COUNT, 1);
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);
            let cmd_reg = (cmd::READ_SINGLE_BLOCK << 8) | 0x3A; // R1, data read
            crate::mmio::write32(self.base + regs::COMMAND, cmd_reg as u32);
        }

        // Wait for buffer read ready
        for _ in 0..1_000_000 {
            let status = unsafe { crate::mmio::read32(self.base + regs::NORMAL_INT_STATUS) } as u16;
            if status & regs::BUF_READ_READY != 0 {
                unsafe { crate::mmio::write32(self.base + regs::NORMAL_INT_STATUS, regs::BUF_READ_READY as u32) };
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
        for _ in 0..100_000 {
            let status = unsafe { crate::mmio::read32(self.base + regs::NORMAL_INT_STATUS) } as u16;
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write32(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE as u32) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
    }

    /// Write a 512-byte block to the card.
    fn write_block(&mut self, block_addr: u32, data: &[u8; 512]) -> Result<(), DeviceError> {
        if !self.initialized {
            return Err(DeviceError::NotReady);
        }

        while unsafe { crate::mmio::read32(self.base + regs::PRESENT_STATE) } & regs::DAT_INHIBIT != 0 {}

        unsafe {
            crate::mmio::write32(self.base + regs::BLOCK_SIZE, 512);
            crate::mmio::write32(self.base + regs::BLOCK_COUNT, 1);
            crate::mmio::write32(self.base + regs::ARGUMENT, block_addr);
            let cmd_reg = (cmd::WRITE_SINGLE_BLOCK << 8) | 0x3A; // R1, data write
            crate::mmio::write32(self.base + regs::COMMAND, cmd_reg as u32);
        }

        // Wait for buffer write ready
        for _ in 0..1_000_000 {
            let status = unsafe { crate::mmio::read32(self.base + regs::NORMAL_INT_STATUS) } as u16;
            if status & regs::BUF_WRITE_READY != 0 {
                unsafe { crate::mmio::write32(self.base + regs::NORMAL_INT_STATUS, regs::BUF_WRITE_READY as u32) };
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
        for _ in 0..100_000 {
            let status = unsafe { crate::mmio::read32(self.base + regs::NORMAL_INT_STATUS) } as u16;
            if status & regs::XFER_COMPLETE != 0 {
                unsafe { crate::mmio::write32(self.base + regs::NORMAL_INT_STATUS, regs::XFER_COMPLETE as u32) };
                return Ok(());
            }
        }
        Err(DeviceError::IoError(DeviceIoKind::Timeout))
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
