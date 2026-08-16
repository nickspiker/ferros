//! UFS (Universal Flash Storage) host controller driver.
//!
//! Talks to the UFSHCI v3.0 controller on QCM6490 (base 0x1D84000). ABL initializes the controller and brings up the UniPro link before handing off to us. We skip link startup and just issue SCSI commands thru the existing link.
//!
//! ## Architecture
//!
//! UFSHCI uses a Transfer Request List (array of UTRDs in DRAM). Each UTRD points to a UCD (Command Descriptor) containing:
//!   - Command UPIU (SCSI CDB or Query request)
//!   - Response UPIU (filled by device)
//!   - PRDT (scatter-gather for data)
//!
//! To send a command: fill UTRD+UCD, ring doorbell, poll for completion.

/// UFSHCI register offsets from base address.
#[allow(dead_code)]
mod regs {
    pub const CAP: usize = 0x00;        // Controller Capabilities
    pub const VER: usize = 0x08;        // Version
    pub const IS: usize = 0x20;         // Interrupt Status (W1C)
    pub const IE: usize = 0x24;         // Interrupt Enable
    pub const HCS: usize = 0x30;        // Host Controller Status
    pub const HCE: usize = 0x34;        // Host Controller Enable
    pub const UTRLBA: usize = 0x50;     // Transfer Request List Base (low)
    pub const UTRLBAU: usize = 0x54;    // Transfer Request List Base (high)
    pub const UTRLDBR: usize = 0x58;    // Transfer Request List Doorbell
    pub const UTRLCLR: usize = 0x5C;    // Transfer Request List Clear
    pub const UTRLRSR: usize = 0x60;    // Transfer Request List Run-Stop

    // IS bit definitions
    pub const IS_UTRCS: u32 = 1 << 0;   // Transfer Request Completion
    pub const IS_UCCS: u32 = 1 << 10;   // UIC Command Completion

    // HCS bit definitions
    pub const HCS_DP: u32 = 1 << 0;     // Device Present
    pub const HCS_UTRLRDY: u32 = 1 << 1; // Transfer List Ready
    pub const HCS_UTMRLRDY: u32 = 1 << 2; // Task Mgmt List Ready
    pub const HCS_UCRDY: u32 = 1 << 3;  // UIC Command Ready
    pub const HCS_READY_MASK: u32 = 0xF; // All four ready bits
}

/// UPIU Transaction Codes
#[allow(dead_code)]
mod upiu {
    pub const NOP_OUT: u8 = 0x00;
    pub const COMMAND: u8 = 0x01;
    pub const QUERY_REQ: u8 = 0x16;
    pub const NOP_IN: u8 = 0x20;
    pub const RESPONSE: u8 = 0x21;
    pub const QUERY_RSP: u8 = 0x36;

    // Command flags
    pub const FLAG_READ: u8 = 0x40;
    pub const FLAG_WRITE: u8 = 0x20;

    // Query function
    pub const QUERY_FN_READ: u8 = 0x01;

    // Query opcodes
    pub const QUERY_OP_READ_DESC: u8 = 0x01;

    // Descriptor IDNs
    pub const DESC_DEVICE: u8 = 0x00;
    pub const DESC_UNIT: u8 = 0x02;
    pub const DESC_GEOMETRY: u8 = 0x07;
}

/// SCSI opcodes
mod scsi {
    pub const READ_10: u8 = 0x28;
    pub const WRITE_10: u8 = 0x2A;
#[allow(dead_code)] pub const TEST_UNIT_READY: u8 = 0x00;
}

/// Overall Command Status values (UTRD DW2)
#[allow(dead_code)]
mod ocs {
    pub const SUCCESS: u8 = 0x00;
    pub const INVALID_CMD_TABLE: u8 = 0x01;
    pub const INVALID_PRDT: u8 = 0x02;
    pub const MISMATCH_DATA_BUF: u8 = 0x03;
    pub const MISMATCH_RESP: u8 = 0x04;
    pub const PEER_COMM_FAIL: u8 = 0x05;
    pub const ABORTED: u8 = 0x06;
    pub const FATAL_ERROR: u8 = 0x07;
    pub const INVALID: u8 = 0x0F;  // Not yet completed
}

/// UTP Transfer Request Descriptor (32 bytes, in DRAM).
#[repr(C, align(32))]
struct Utrd {
    dw: [u32; 8],
}

/// UTP Command Descriptor — Command UPIU + Response UPIU + PRDT. Must be 128-byte aligned. Fixed layout: [0x000..0x200) Command UPIU (512 bytes) [0x200..0x400) Response UPIU (512 bytes) [0x400..0x410) PRDT entry 0 (16 bytes)
#[repr(C, align(128))]
struct Ucd {
    cmd_upiu: [u8; 512],
    rsp_upiu: [u8; 512],
    prdt: [PrdtEntry; 1],
}

/// Physical Region Description Table entry (16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct PrdtEntry {
    base_addr_lo: u32,
    base_addr_hi: u32,
    reserved: u32,
    size: u32, // byte count - 1
}

/// Static DMA buffers for UFS transfers. Single slot — we do one command at a time.
#[repr(C, align(1024))]
struct UfsBuffers {
    utrd: Utrd,
    _pad: [u8; 992], // pad to 1024 for UTRD list alignment
    ucd: Ucd,
    data: [u8; 4096], // one block data buffer
}

static mut UFS_BUF: UfsBuffers = UfsBuffers {
    utrd: Utrd { dw: [0; 8] },
    _pad: [0; 992],
    ucd: Ucd {
        cmd_upiu: [0; 512],
        rsp_upiu: [0; 512],
        prdt: [PrdtEntry {
            base_addr_lo: 0,
            base_addr_hi: 0,
            reserved: 0,
            size: 0,
        }; 1],
    },
    data: [0; 4096],
};

/// Probe result from UFS controller.
#[derive(Default)]
pub struct UfsProbe {
    pub hce: u32,
    pub hcs: u32,
    pub cap: u32,
    pub ver: u32,
    pub link_up: bool,
    pub nop_ok: bool,
    pub nop_ocs: u8,
    pub nop_rsp: u8,
    pub nop_rsp_raw: [u8; 4], // first 4 bytes of response UPIU
    /// Raw descriptor bytes for debugging
    pub geo_raw: [u8; 32],
    pub unit0_raw: [u8; 32],
    pub geo_len: usize,
    pub unit0_len: usize,
    /// Geometry descriptor fields
    pub geo_ok: bool,
    pub total_raw_capacity_sectors: u64,  // in 512-byte units
    pub segment_size: u32,                // in 512-byte units
    pub min_block_size_exp: u8,           // block = 2^exp * 512
    pub num_lun: u8,
    /// Unit 0 descriptor fields
    pub unit0_ok: bool,
    pub unit0_block_size_exp: u8,         // block = 2^exp * 512
    pub unit0_block_count: u64,
    pub unit0_erase_block_size: u32,      // in allocation units
}

pub struct UfsController {
    base: usize,
}

impl UfsController {
    pub fn new(base: usize) -> Self {
        Self { base }
    }

    /// Resume from ABL's initialized state at the default QCM6490 base. ABL leaves HCE=1 and link up. We just set up our transfer list.
    pub fn resume() -> Self {
        let ctrl = Self::new(ferros_layout::UFS_BASE);
        ctrl.init_transfer_list();
        ctrl
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { crate::mmio::read32(self.base + offset) }
    }

    fn write_reg(&self, offset: usize, val: u32) {
        unsafe { crate::mmio::write32(self.base + offset, val) }
    }

    /// Check if the controller is enabled and the link is up.
    pub fn link_is_up(&self) -> bool {
        let hce = self.read_reg(regs::HCE);
        let hcs = self.read_reg(regs::HCS);
        (hce & 1) != 0 && (hcs & regs::HCS_READY_MASK) == regs::HCS_READY_MASK
    }

    /// Set up our own transfer request list and start it.
    /// Stops the list first: UTRLBA is only sampled while UTRLRSR=0, so rebasing a running list (e.g. a RUN payload attaching with its own UFS_BUF after the kernel already started one) is silently ignored and every command then completes into the OLD descriptor — the payload sees its UTRD stuck at OCS=G#F.
    pub fn init_transfer_list(&self) {
        unsafe {
            let utrd_addr = &raw const UFS_BUF.utrd as usize as u64;
            self.write_reg(regs::UTRLRSR, 0);
            self.write_reg(regs::UTRLBA, utrd_addr as u32);
            self.write_reg(regs::UTRLBAU, (utrd_addr >> 32) as u32);
            // Clear any pending interrupts
            self.write_reg(regs::IS, 0xFFFF_FFFF);
            // Start the transfer request list
            self.write_reg(regs::UTRLRSR, 1);
        }
    }

    /// Build and send a NOP OUT to verify device responsiveness.
    fn send_nop(&self) -> (u8, u8, [u8; 4]) {
        unsafe {
            let buf = &raw mut UFS_BUF;

            // Zero out UCD
            core::ptr::write_bytes(&raw mut (*buf).ucd as *mut u8, 0, core::mem::size_of::<Ucd>());

            // NOP OUT UPIU: transaction_code = 0x00, rest zero
            (*buf).ucd.cmd_upiu[0] = upiu::NOP_OUT;
            (*buf).ucd.cmd_upiu[3] = 0; // task tag 0

            // Build UTRD
            let ucd_addr = &raw const (*buf).ucd as usize as u64;
            (*buf).utrd.dw[0] = 0;
            // DW0 byte 3: interrupt=1, direction=0, cmd_type=1 (native UFS)
            (*buf).utrd.dw[0] = (1 << 24) // interrupt
                | (0 << 25)               // no data direction
                | (1 << 28);              // command_type = native UFS
            (*buf).utrd.dw[1] = 0;
            (*buf).utrd.dw[2] = 0x0F00_0000; // OCS = INVALID (byte 8 in LE = DW2 bits [7:0])
            // Actually OCS is at byte offset 8 of the UTRD = DW2. On LE: DW2 byte 0 (lowest) = OCS. So 0x0F in bits [7:0].
            (*buf).utrd.dw[2] = 0x0000_000F; // OCS = INVALID
            (*buf).utrd.dw[3] = 0;
            (*buf).utrd.dw[4] = ucd_addr as u32;
            (*buf).utrd.dw[5] = (ucd_addr >> 32) as u32;
            // Response UPIU: offset=0x80 DWORDs (0x200 bytes), length=0x80 DWORDs
            (*buf).utrd.dw[6] = (0x0080 << 16) | 0x0080;
            // PRDT: offset=0x100 DWORDs (0x400 bytes), length=0 entries
            (*buf).utrd.dw[7] = (0x0100 << 16) | 0x0000;

            // Cache clean
            let utrd_addr_phys = &raw const (*buf).utrd as usize;
            let ucd_addr_phys = &raw const (*buf).ucd as usize;
            crate::mmio::cache_clean(utrd_addr_phys, 32);
            crate::mmio::cache_clean(ucd_addr_phys, 1024);
            core::arch::asm!("dsb sy");

            // Ring doorbell (slot 0)
            self.write_reg(regs::UTRLDBR, 1);

            // Poll for completion
            for _ in 0..1_000_000u32 {
                let is = self.read_reg(regs::IS);
                if is & regs::IS_UTRCS != 0 {
                    self.write_reg(regs::IS, regs::IS_UTRCS);
                    break;
                }
            }

            // Read results
            crate::mmio::cache_invalidate(utrd_addr_phys, 32);
            crate::mmio::cache_invalidate(ucd_addr_phys, 1024);
            let ocs = ((*buf).utrd.dw[2] & 0xFF) as u8;
            let rsp_code = (*buf).ucd.rsp_upiu[0]; // transaction code
            let mut rsp_raw = [0u8; 4];
            rsp_raw.copy_from_slice(&(&(*buf).ucd.rsp_upiu)[..4]);
            (ocs, rsp_code, rsp_raw)
        }
    }

    /// Send a Query Request to read a descriptor. Returns the descriptor bytes (up to 255) or empty on failure.
    fn query_read_descriptor(&self, idn: u8, index: u8) -> ([u8; 256], usize, u8) {
        let mut desc = [0u8; 256];
        unsafe {
            let buf = &raw mut UFS_BUF;

            // Zero UCD
            core::ptr::write_bytes(&raw mut (*buf).ucd as *mut u8, 0, core::mem::size_of::<Ucd>());

            // Query Request UPIU
            (*buf).ucd.cmd_upiu[0] = upiu::QUERY_REQ;
            (*buf).ucd.cmd_upiu[1] = 0; // flags
            (*buf).ucd.cmd_upiu[3] = 0; // task tag — MUST match the doorbell slot (we always ring slot 0)
            (*buf).ucd.cmd_upiu[5] = upiu::QUERY_FN_READ; // query function
            // Data segment length (big-endian u16 at bytes 10-11)
            (*buf).ucd.cmd_upiu[10] = 0;
            (*buf).ucd.cmd_upiu[11] = 0xFF; // max 255 bytes

            // TSF (Transaction Specific Fields) at bytes 12-31
            (*buf).ucd.cmd_upiu[12] = upiu::QUERY_OP_READ_DESC; // opcode
            (*buf).ucd.cmd_upiu[13] = idn;     // descriptor IDN
            (*buf).ucd.cmd_upiu[14] = index;    // index
            (*buf).ucd.cmd_upiu[15] = 0;        // selector
            // Length at bytes 18-19 (big-endian)
            (*buf).ucd.cmd_upiu[18] = 0;
            (*buf).ucd.cmd_upiu[19] = 0xFF;

            // Build UTRD
            let ucd_addr = &raw const (*buf).ucd as usize as u64;
            (*buf).utrd.dw[0] = (1 << 24)  // interrupt
                | (0 << 25)                 // no data direction (descriptor comes in response UPIU)
                | (1 << 28);               // command_type = native UFS (device mgmt)
            (*buf).utrd.dw[1] = 0;
            (*buf).utrd.dw[2] = 0x0000_000F; // OCS = INVALID
            (*buf).utrd.dw[3] = 0;
            (*buf).utrd.dw[4] = ucd_addr as u32;
            (*buf).utrd.dw[5] = (ucd_addr >> 32) as u32;
            (*buf).utrd.dw[6] = (0x0080 << 16) | 0x0080;
            (*buf).utrd.dw[7] = (0x0100 << 16) | 0x0000;

            // Cache clean + doorbell
            let utrd_phys = &raw const (*buf).utrd as usize;
            let ucd_phys = &raw const (*buf).ucd as usize;
            crate::mmio::cache_clean(utrd_phys, 32);
            crate::mmio::cache_clean(ucd_phys, 1024);
            core::arch::asm!("dsb sy");

            self.write_reg(regs::UTRLDBR, 1);

            // Poll
            for _ in 0..1_000_000u32 {
                let is = self.read_reg(regs::IS);
                if is & regs::IS_UTRCS != 0 {
                    self.write_reg(regs::IS, regs::IS_UTRCS);
                    break;
                }
            }

            // Read results
            crate::mmio::cache_invalidate(utrd_phys, 32);
            crate::mmio::cache_invalidate(ucd_phys, 1024);
            let ocs = ((*buf).utrd.dw[2] & 0xFF) as u8;

            if ocs == ocs::SUCCESS {
                // Response UPIU: descriptor data starts at byte 32
                let rsp_data_len = (((*buf).ucd.rsp_upiu[10] as usize) << 8)
                    | ((*buf).ucd.rsp_upiu[11] as usize);
                let copy_len = rsp_data_len.min(256);
                desc[..copy_len].copy_from_slice(&(&(*buf).ucd.rsp_upiu)[32..32 + copy_len]);
                (desc, copy_len, ocs)
            } else {
                (desc, 0, ocs)
            }
        }
    }

    /// Probe the UFS controller and device.
    pub fn probe(&self) -> UfsProbe {
        let mut p = UfsProbe::default();

        p.hce = self.read_reg(regs::HCE);
        p.hcs = self.read_reg(regs::HCS);
        p.cap = self.read_reg(regs::CAP);
        p.ver = self.read_reg(regs::VER);
        p.link_up = self.link_is_up();

        if !p.link_up {
            return p;
        }

        // Set up our transfer list
        self.init_transfer_list();

        // NOP OUT to verify device
        let (nop_ocs, nop_rsp, nop_raw) = self.send_nop();
        p.nop_ocs = nop_ocs;
        p.nop_rsp = nop_rsp;
        p.nop_rsp_raw = nop_raw;
        p.nop_ok = nop_ocs == ocs::SUCCESS && nop_rsp == upiu::NOP_IN;

        if !p.nop_ok {
            return p;
        }

        // Read Geometry Descriptor (IDN 0x07)
        let (geo, geo_len, geo_ocs) = self.query_read_descriptor(upiu::DESC_GEOMETRY, 0);
        p.geo_len = geo_len;
        p.geo_raw[..32.min(geo_len)].copy_from_slice(&geo[..32.min(geo_len)]);
        if geo_ocs == ocs::SUCCESS && geo_len >= 0x20 {
            p.geo_ok = true;
            // qTotalRawDeviceCapacity at offset 0x04, 8 bytes big-endian
            p.total_raw_capacity_sectors =
                ((geo[0x04] as u64) << 56) | ((geo[0x05] as u64) << 48)
                | ((geo[0x06] as u64) << 40) | ((geo[0x07] as u64) << 32)
                | ((geo[0x08] as u64) << 24) | ((geo[0x09] as u64) << 16)
                | ((geo[0x0A] as u64) << 8) | (geo[0x0B] as u64);
            // dSegmentSize at offset 0x0D, 4 bytes big-endian
            p.segment_size =
                ((geo[0x0D] as u32) << 24) | ((geo[0x0E] as u32) << 16)
                | ((geo[0x0F] as u32) << 8) | (geo[0x10] as u32);
            p.min_block_size_exp = geo[0x12];
            p.num_lun = geo[0x0C];
        }

        // Read Unit Descriptor for LUN 0 (IDN 0x02, index 0)
        let (unit, unit_len, unit_ocs) = self.query_read_descriptor(upiu::DESC_UNIT, 0);
        p.unit0_len = unit_len;
        p.unit0_raw[..32.min(unit_len)].copy_from_slice(&unit[..32.min(unit_len)]);
        if unit_ocs == ocs::SUCCESS && unit_len >= 0x20 {
            p.unit0_ok = true;
            p.unit0_block_size_exp = unit[0x0A];
            // qLogicalBlockCount at offset 0x0B, 8 bytes big-endian
            p.unit0_block_count =
                ((unit[0x0B] as u64) << 56) | ((unit[0x0C] as u64) << 48)
                | ((unit[0x0D] as u64) << 40) | ((unit[0x0E] as u64) << 32)
                | ((unit[0x0F] as u64) << 24) | ((unit[0x10] as u64) << 16)
                | ((unit[0x11] as u64) << 8) | (unit[0x12] as u64);
            // dEraseBlockSize at offset 0x13, 4 bytes big-endian
            p.unit0_erase_block_size =
                ((unit[0x13] as u32) << 24) | ((unit[0x14] as u32) << 16)
                | ((unit[0x15] as u32) << 8) | (unit[0x16] as u32);
        }

        p
    }

    /// Common UTRD setup, doorbell, poll, return OCS. Caller must have already filled ucd.cmd_upiu and prdt.
    fn send_command(&self, cmd_type: u32, direction: u32, prdt_count: u16) -> u8 {
        unsafe {
            let buf = &raw mut UFS_BUF;
            let ucd_addr = &raw const (*buf).ucd as usize as u64;

            // Build UTRD
            (*buf).utrd.dw[0] = (1 << 24)          // interrupt
                | (direction << 25)                  // data direction
                | (cmd_type << 28);                  // command type
            (*buf).utrd.dw[1] = 0;
            (*buf).utrd.dw[2] = 0x0000_000F;        // OCS = INVALID
            (*buf).utrd.dw[3] = 0;
            (*buf).utrd.dw[4] = ucd_addr as u32;
            (*buf).utrd.dw[5] = (ucd_addr >> 32) as u32;
            (*buf).utrd.dw[6] = (0x0080 << 16) | 0x0080; // response: offset=0x80 DW, len=0x80 DW
            (*buf).utrd.dw[7] = (0x0100 << 16) | (prdt_count as u32); // prdt: offset=0x100 DW

            // Cache clean UTRD + UCD + data buffer
            let utrd_phys = &raw const (*buf).utrd as usize;
            let ucd_phys = &raw const (*buf).ucd as usize;
            let data_phys = &raw const (*buf).data as usize;
            crate::mmio::cache_clean(utrd_phys, 32);
            crate::mmio::cache_clean(ucd_phys, 1040); // 1024 + 16 for PRDT
            crate::mmio::cache_clean(data_phys, 4096);
            core::arch::asm!("dsb sy");

            // Ring doorbell
            self.write_reg(regs::UTRLDBR, 1);

            // Poll for completion
            for _ in 0..10_000_000u32 {
                let is = self.read_reg(regs::IS);
                if is & regs::IS_UTRCS != 0 {
                    self.write_reg(regs::IS, regs::IS_UTRCS);
                    break;
                }
            }

            // Invalidate caches to see DMA results
            crate::mmio::cache_invalidate(utrd_phys, 32);
            crate::mmio::cache_invalidate(ucd_phys, 1040);
            crate::mmio::cache_invalidate(data_phys, 4096);

            ((*buf).utrd.dw[2] & 0xFF) as u8
        }
    }

    /// Build a SCSI Command UPIU in the UCD.
    fn build_scsi_upiu(&self, lun: u8, tag: u8, flags: u8, xfer_len: u32, cdb: &[u8]) {
        unsafe {
            let buf = &raw mut UFS_BUF;
            // Zero command UPIU
            core::ptr::write_bytes((&raw mut (*buf).ucd.cmd_upiu) as *mut u8, 0, 512);

            (*buf).ucd.cmd_upiu[0] = upiu::COMMAND;   // transaction code
            (*buf).ucd.cmd_upiu[1] = flags;            // R/W flags
            (*buf).ucd.cmd_upiu[2] = lun;              // LUN
            (*buf).ucd.cmd_upiu[3] = tag;              // task tag
            (*buf).ucd.cmd_upiu[4] = 0;                // command set type = SCSI

            // Expected data transfer length (big-endian u32, bytes 12-15)
            (*buf).ucd.cmd_upiu[12] = (xfer_len >> 24) as u8;
            (*buf).ucd.cmd_upiu[13] = (xfer_len >> 16) as u8;
            (*buf).ucd.cmd_upiu[14] = (xfer_len >> 8) as u8;
            (*buf).ucd.cmd_upiu[15] = xfer_len as u8;

            // CDB at bytes 16-31 (zero-padded)
            let copy_len = cdb.len().min(16);
            for i in 0..copy_len {
                (*buf).ucd.cmd_upiu[16 + i] = cdb[i];
            }

            // Zero response UPIU
            core::ptr::write_bytes((&raw mut (*buf).ucd.rsp_upiu) as *mut u8, 0, 512);
        }
    }

    /// Set up PRDT entry 0 pointing to the data buffer.
    fn setup_prdt(&self, byte_count: u32) {
        unsafe {
            let buf = &raw mut UFS_BUF;
            let data_addr = &raw const (*buf).data as usize as u64;
            (*buf).ucd.prdt[0].base_addr_lo = data_addr as u32;
            (*buf).ucd.prdt[0].base_addr_hi = (data_addr >> 32) as u32;
            (*buf).ucd.prdt[0].reserved = 0;
            (*buf).ucd.prdt[0].size = byte_count - 1; // byte count minus 1
        }
    }

    /// Read one 4KB block from UFS LUN 0. Returns OCS (0 = success). Data is in the internal buffer.
    pub fn read_block(&self, lba: u32) -> u8 {
        // SCSI READ(10) CDB: opcode=0x28, LBA (big-endian), transfer length=1 block
        let cdb = [
            scsi::READ_10,
            0x00,                        // flags
            (lba >> 24) as u8,           // LBA [31:24]
            (lba >> 16) as u8,           // LBA [23:16]
            (lba >> 8) as u8,            // LBA [15:8]
            lba as u8,                   // LBA [7:0]
            0x00,                        // group
            0x00, 0x01,                  // transfer length = 1 block
            0x00,                        // control
        ];

        self.build_scsi_upiu(0, 0, upiu::FLAG_READ, 4096, &cdb);
        self.setup_prdt(4096);

        // direction=2 (device→host), cmd_type=0 (SCSI), 1 PRDT entry
        self.send_command(0, 2, 1)
    }

    /// Write one 4KB block to UFS LUN 0. Caller must fill data buffer first via `data_buffer_mut()`. Returns OCS (0 = success).
    pub fn write_block(&self, lba: u32) -> u8 {
        // SCSI WRITE(10) CDB
        let cdb = [
            scsi::WRITE_10,
            0x00,                        // flags
            (lba >> 24) as u8,
            (lba >> 16) as u8,
            (lba >> 8) as u8,
            lba as u8,
            0x00,
            0x00, 0x01,                  // transfer length = 1 block
            0x00,
        ];

        self.build_scsi_upiu(0, 0, upiu::FLAG_WRITE, 4096, &cdb);
        self.setup_prdt(4096);

        // direction=1 (host→device), cmd_type=0 (SCSI), 1 PRDT entry
        self.send_command(0, 1, 1)
    }

    /// Get a reference to the data buffer (for reading results after read_block).
    pub fn data_buffer(&self) -> &[u8; 4096] {
        unsafe { &(*(&raw const UFS_BUF)).data }
    }

    /// Get a mutable reference to the data buffer (for filling before write_block).
    pub fn data_buffer_mut(&self) -> &mut [u8; 4096] {
        unsafe { &mut (*(&raw mut UFS_BUF)).data }
    }

    /// Get the SCSI response status from the last command.
    pub fn last_response_status(&self) -> u8 {
        unsafe { (*(&raw const UFS_BUF)).ucd.rsp_upiu[7] }
    }

    /// First 16 bytes of the last Response UPIU (transaction code, flags, LUN, tag, response, status, ...). All zero = the controller never wrote a response (it never DMA'd the descriptors — points upstream of the device, at DMA/SysMMU/controller, not the device itself).
    pub fn response_upiu_head(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        unsafe {
            let rsp = &(*(&raw const UFS_BUF)).ucd.rsp_upiu;
            out.copy_from_slice(&rsp[..16]);
        }
        out
    }

    /// Raw UTRD OCS field (dw[2] & 0xFF) from the last command — 0xF is our pre-armed "INVALID" sentinel; if it's still 0xF the controller never wrote the descriptor back.
    pub fn last_ocs(&self) -> u8 {
        unsafe { ((*(&raw const UFS_BUF)).utrd.dw[2] & 0xFF) as u8 }
    }
}
