//! UFS (Universal Flash Storage) host controller driver.
//!
//! Talks to the UFSHCI v3.0 controller on Pixel 8 (zuma, base G#1320_0000).
//! ABL initializes the controller, but its handoff state does not process host transfer requests (UFS.md) — `full_init()` re-initializes the controller from HCE reset: vendor config, M-PHY/UNIPRO calibration (see `ufs_cal`), DME_LINKSTARTUP, device init, and a power-mode change to HS-G4.
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
    pub const UTMRLBA: usize = 0x70;    // Task Mgmt Request List Base (low)
    pub const UTMRLBAU: usize = 0x74;   // Task Mgmt Request List Base (high)
    pub const UTMRLRSR: usize = 0x80;   // Task Mgmt Request List Run-Stop
    pub const UICCMD: usize = 0x90;     // UIC Command
    pub const UCMDARG1: usize = 0x94;
    pub const UCMDARG2: usize = 0x98;
    pub const UCMDARG3: usize = 0x9C;

    // IS bit definitions
    pub const IS_UTRCS: u32 = 1 << 0;   // Transfer Request Completion
    pub const IS_UPMS: u32 = 1 << 4;    // UIC Power Mode Status
    pub const IS_UHXS: u32 = 1 << 5;    // UIC Hibernate Exit Status
    pub const IS_UHES: u32 = 1 << 6;    // UIC Hibernate Enter Status
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

/// Exynos vendor-specified HCI register offsets (from `ufs_cal::base::HCI` = G#1320_1100). Subset used by `full_init`; full map in the reference `ufs-vs-regs.h`.
#[allow(dead_code)]
mod vs {
    pub const TXPRDT_ENTRY_SIZE: usize = 0x00;
    pub const RXPRDT_ENTRY_SIZE: usize = 0x04;
    pub const US_TO_CNT_VAL: usize = 0x0C;
    pub const UTRL_NEXUS_TYPE: usize = 0x40;
    pub const UTMRL_NEXUS_TYPE: usize = 0x44;
    pub const SW_RST: usize = 0x50;
    pub const DATA_REORDER: usize = 0x60;
    pub const AXIDMA_RWDATA_BURST_LEN: usize = 0x6C;
    pub const GPIO_OUT: usize = 0x70;         // bit 0 = UFS device reset_n line
    pub const ERROR_EN_DL_LAYER: usize = 0x7C;
    pub const ERROR_EN_N_LAYER: usize = 0x80;
    pub const ERROR_EN_T_LAYER: usize = 0x84;
    pub const V2P1_CTRL: usize = 0x8C;        // bit 16 = IA_TICK_SEL
    pub const CLKSTOP_CTRL: usize = 0xB0;     // forced clock stops
    pub const FORCE_HCS: usize = 0xB4;        // AUTO clock-stop enables — the M-PHY APB hang gate
    pub const IOP_ACG_DISABLE: usize = 0x100;

    pub const SW_RST_MASK: u32 = 0b11;        // UNIPRO | LINK
    pub const PRDT_PREFETCH_EN: u32 = 1 << 31;
    pub const WLU_EN: u32 = 1 << 31;
    /// All auto clock-stop enable bits (CLK_STOP_CTRL_EN_ALL + core clk). We keep them OFF for the whole session: we poll, power is irrelevant during bring-up, and the auto-gate is what bus-hung the AP on every past PMA/vendor access.
    pub const FORCE_HCS_ALL_EN: u32 = 0xFF0;
    pub const CLK_STOP_ALL: u32 = 0x1F;
}

/// UNIPRO register offsets (from `ufs_cal::base::UNIPRO`) read by `full_init`.
mod unip {
    pub const PA_AVAILRXDATALANES: usize = 0x3100;
    pub const PA_CONNECTEDRXDATALANES: usize = 0x3204;
    pub const PA_ACTIVERXDATALANES: usize = 0x3200;
    pub const PA_DBG_OPTION_SUITE_1: usize = 0x39A8;
    pub const PA_DBG_OPTION_SUITE_2: usize = 0x39B4;
    // gs-kernel runtime values — the proven Linux re-link path (which, like us, re-links from a fresh HCE reset rather than reusing ABL's link). Toggling these to ABL's captured values (G#98913C1C/G#E01C195F) made no difference to link startup, so match the reference.
    pub const DBG_SUITE1_ENABLE: u32 = 0x90913C1C;
    pub const DBG_SUITE2_ENABLE: u32 = 0xE01C115F;
}

/// UIC command opcodes.
mod uic {
    pub const DME_GET: u32 = 0x01;
    pub const DME_SET: u32 = 0x02;
    pub const DME_LINKSTARTUP: u32 = 0x16;
    pub const DME_HIBERN8_ENTER: u32 = 0x17;
    pub const DME_HIBERN8_EXIT: u32 = 0x18;
}

/// UniPro PA-layer MIB attribute IDs for the power-mode change.
mod pa {
    pub const ACTIVETXDATALANES: u32 = 0x1560;
    pub const TXGEAR: u32 = 0x1568;
    pub const TXTERMINATION: u32 = 0x1569;
    pub const HSSERIES: u32 = 0x156A;
    pub const PWRMODE: u32 = 0x1571;
    pub const ACTIVERXDATALANES: u32 = 0x1580;
    pub const RXGEAR: u32 = 0x1583;
    pub const RXTERMINATION: u32 = 0x1584;
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

/// UTP Task Management Request List — 1 slot (80-byte UTMRD). Never ringed; exists so HCS.UTMRLRDY has a valid base and the run-stop bit can be set, matching the standard `make_hba_operational` flow.
#[repr(C, align(1024))]
struct UtmrlBuf {
    utmrd: [u32; 32],
}

static mut UTMRL_BUF: UtmrlBuf = UtmrlBuf { utmrd: [0; 32] };

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
        crate::ufs_cal::trace_write32(self.base + offset, val)
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

    /// Physical address of the UTRD (== the value written to UTRLBA). No MMU on husky, so the CPU-visible address IS the physical address the UFS DMA master must reach.
    pub fn utrd_phys(&self) -> u64 {
        unsafe { &raw const UFS_BUF.utrd as usize as u64 }
    }

    /// Run one NOP OUT on the CURRENT link state without touching HCE/PHY/config — the pristine-ABL-link transfer test. Returns (ocs, rsp_transaction_code).
    pub fn live_nop(&self) -> (u8, u8) {
        let (ocs, rsp, _) = self.send_nop();
        (ocs, rsp)
    }

    /// Exit UFS hibernate on the live link. ABL parks UFS in HIBERN8 before handoff (its last UIC command reads DME_HIBERN8_ENTER=G#17): HCS reads link-up/ready but the transfer manager sits idle (never fetches the descriptor, no error, no timeout) until DME_HIBERN8_EXIT. Hibernate-exit completion is signalled by IS.UHXS (bit 5), NOT IS.UPMS (bit 4, which is for power-MODE changes) — waiting on UPMS reports a false failure and can trip UPMCRS fatal. MUST be the first UFS operation on the pristine link, before any doorbell ring or IS clear perturbs the hibernating state. Returns [is_before, uiccmd_last, uic_result, uhxs_completed, hcs_after, is_after]. uhxs_completed 1 = the link actually left hibernate.
    pub fn live_hibern8_exit(&self) -> [u32; 6] {
        let mut out = [0u32; 6];
        out[0] = self.read_reg(regs::IS);          // IS as ABL left it (UCCS from its HIBERN8_ENTER)
        out[1] = self.read_reg(regs::UICCMD);       // last UIC command (G#17 = HIBERN8_ENTER confirms hibernate)
        // Exynos hibern8_notify EXIT/PRE_CHANGE: ungate the internal clock, then run the pre-h8-exit PMA cal, BEFORE the DME command — else the exit hits an untuned PHY and UPMCRS goes fatal.
        self.unlock_clocks();
        let _ = crate::ufs_cal::pre_h8_exit(crate::ufs_cal::CalParams {
            available_lane: 2,
            connected_rx_lane: 2,
            active_rx_lane: 2,
        });
        self.write_reg(regs::IS, 0xFFFF_FFFF);      // clear ABL's stale UCCS/UHES before issuing exit
        out[2] = match self.uic_cmd(uic::DME_HIBERN8_EXIT, 0, 0, 0) {
            Ok(code) => code,
            Err(()) => 0xFFFF_FFFF,
        };
        let mut ok = false;
        for _ in 0..1_000_000u32 {
            if self.read_reg(regs::IS) & regs::IS_UHXS != 0 {
                self.write_reg(regs::IS, regs::IS_UHXS);
                ok = true;
                break;
            }
        }
        out[3] = ok as u32;
        out[4] = self.read_reg(regs::HCS);
        out[5] = self.read_reg(regs::IS);
        out
    }

    /// Snapshot 12 PMA/PA/UNIPRO registers for a working-vs-failed link register-diff (the concrete next lead in UFS.md). Clears the FORCE_HCS auto clock-stop gates first — a PMA read with the gate on bus-hangs the AP. Order: PMA COMN 0x000/0x140/0x150/0x19C/0x1A0/0xC74, PMA TRSV lane0 0x9F0/0x9F4/0xA00, UNIPRO PA_CTRLSTATE 0x15C / PA_TX_STATE 0x160 / MAXRXHSGEAR 0x321C.
    pub fn snapshot_pma(&self) -> [u32; 12] {
        self.hci_w(vs::FORCE_HCS, self.hci(vs::FORCE_HCS) & !vs::FORCE_HCS_ALL_EN);
        self.hci_w(vs::CLKSTOP_CTRL, self.hci(vs::CLKSTOP_CTRL) & !vs::CLK_STOP_ALL);
        let pma = |off: usize| unsafe { crate::mmio::read32(crate::ufs_cal::base::PMA + off) };
        let uni = |off: usize| unsafe { crate::mmio::read32(crate::ufs_cal::base::UNIPRO + off) };
        [
            pma(0x000), pma(0x140), pma(0x150), pma(0x19C),
            pma(0x1A0), pma(0xC74), pma(0x9F0), pma(0x9F4),
            pma(0xA00), uni(0x15C), uni(0x160), uni(0x321C),
        ]
    }
}

/// Step numbers for `InitReport::fail_step` (0 = no failure). Each is also the bit index set in `steps` on success.
pub mod step {
    pub const CLOCKS: u32 = 1;
    pub const HCE_OFF: u32 = 2;
    pub const SW_RST: u32 = 3;
    pub const CONFIG_HOST: u32 = 4;
    pub const DEV_RESET: u32 = 5;
    pub const HCE_ON: u32 = 6;
    pub const LANES: u32 = 7;
    pub const PRE_LINK: u32 = 8;
    pub const LINKSTARTUP: u32 = 9;
    pub const DEVICE_PRESENT: u32 = 10;
    pub const POST_LINK: u32 = 11;
    pub const LISTS: u32 = 12;
    pub const NOP: u32 = 13;
    pub const FDEVICEINIT: u32 = 14;
    pub const PMC_CAL: u32 = 15;
    pub const PMC: u32 = 16;
}

/// Everything `full_init` measured, for DIAG reporting. All fields are raw u32 so the kernel can dump them without formatting logic.
#[derive(Default)]
pub struct InitReport {
    /// Bit N set = step N completed (see `step`).
    pub steps: u32,
    /// First step that failed, 0 if the whole sequence succeeded. HS gear failure (PMC) leaves the link usable at PWM-G1 — check `steps` bit PMC.
    pub fail_step: u32,
    pub avail_rx: u32,
    pub conn_rx: u32,
    pub active_rx: u32,
    /// EmbCalWait timeouts: pre_link | post_link<<8 | pre_pmc<<16.
    pub cal_timeouts: u32,
    pub linkstartup_tries: u32,
    /// DME_LINKSTARTUP UCMDARG2 result of the last try (0 = success).
    pub linkstartup_res: u32,
    pub hcs_after_link: u32,
    pub nop_tries: u32,
    pub nop_ocs: u32,
    pub fdev_ocs: u32,
    pub fdev_polls: u32,
    /// Which DME_SET failed during PMC (1-based index) <<8 | its result code. 0 = all fine.
    pub pmc_set_fail: u32,
    /// HCS.UPMCRS after the power-mode change (1 = PWR_LOCAL = success).
    pub upmcrs: u32,
    pub hcs_final: u32,
    /// DME_LINKSTARTUP_CNF_RESULT (UNIPRO G#7854) from the last attempt.
    pub ls_cnf: u32,
    /// DME_INTR_ERROR_CODE (UNIPRO G#7B20) from the last attempt.
    pub dme_err: u32,
    /// DBG_PA_CTRLSTATE (UNIPRO G#15C) from the last attempt.
    pub pa_state: u32,
    /// UECPA after the last attempt (clear-on-read; cleared before each attempt).
    pub uecpa: u32,
    /// gph5 DAT (G#1306_0004) after the GPIO_OUT device-reset pulse — bit1 should track the reset_n line if the pad is muxed to UFS function and GPIO_OUT reaches it.
    pub gph5_dat: u32,
    /// DBG_PA_TX_STATE (UNIPRO G#160) after link startup.
    pub pa_tx_state: u32,
    /// UEC family after link startup: UECDL G#3C | UECN G#40<<8 wouldn't fit; packed as UECDL[7:0]|UECN[15:8]|UECT[23:16]|UECDME[31:24] low bytes.
    pub uec_pack: u32,
    /// Clock/refclk state captured right before DME_LINKSTARTUP (the device needs its reference clock to respond). HCI_CLKSTOP_CTRL G#B0 (bit4 REFCLKOUT_STOP must be 0 = refclk running to device).
    pub clkstop_ctrl: u32,
    /// HCI_FORCE_HCS G#B4 (auto clock-stop enables; all should be clear after unlock).
    pub force_hcs: u32,
    /// HCI_MPHY_REFCLK_SEL G#108 (bit0).
    pub mphy_refclk_sel: u32,
    /// CMU_HSI2 QCH_CON_UFS_EMBD (G#1300_30C4) — UFS aclk Q-channel gate.
    pub cmu_qch: u32,
    /// CMU_HSI2 UFS UNIPRO clock gate (G#1300_2110).
    pub cmu_unipro_gate: u32,
    /// PCS cal read-back after pre_link (AUX-window path): RX-lane G#2094 (expect G#F6) | G#20BC<<8 (expect G#79) | TX-lane G#22A4<<16 (expect G#02). If these DON'T match, the AUX-window PCS writes aren't landing = the bug.
    pub pcs_readback: u32,
    /// GPH5CON (G#1306_0000) as full_init found it — pins 0/1 nibbles are the ufs_refclk_out / ufs_rst_n pinmux. Function 2 = both nibbles G#2. If gph5-0 (bits[3:0]) is NOT 2, ABL de-routed REFCLKOUT and the device gets no reference clock (Linux re-routes via pinctrl; we didn't).
    pub gph5_con_before: u32,
    /// GPH5CON after full_init routes both pins to function 2.
    pub gph5_con_after: u32,
}

impl UfsController {
    fn hci(&self, off: usize) -> u32 {
        unsafe { crate::mmio::read32(crate::ufs_cal::base::HCI + off) }
    }
    fn hci_w(&self, off: usize, v: u32) {
        crate::ufs_cal::trace_write32(crate::ufs_cal::base::HCI + off, v)
    }
    fn unipro(&self, off: usize) -> u32 {
        unsafe { crate::mmio::read32(crate::ufs_cal::base::UNIPRO + off) }
    }
    fn unipro_w(&self, off: usize, v: u32) {
        crate::ufs_cal::trace_write32(crate::ufs_cal::base::UNIPRO + off, v)
    }

    /// Set the M-PHY clock gating to match Linux's working bring-up exactly. A ferros-vs-Linux FWTRACE diff showed HCI_FORCE_HCS (G#B4) was the ONLY register ferros programmed differently across the whole init: ferros zeroed it (all auto clock-stop enables OFF), Linux holds it at G#9C0 during cal and at DME_LINKSTARTUP. Zeroing it left the M-PHY clocked differently than the device expects, so the peer stayed silent at link startup. G#9C0 keeps REFCLKOUT_STOP_EN | UFSP_DRCG_EN | REFCLK_STOP_EN | UNIPRO_PCLK_STOP_EN enabled (auto-gate only when idle — harmless during active bring-up) while clearing MPHY_APBCLK_STOP_EN (bit10) and UNIPRO_MCLK_STOP_EN (bit5) so PMA/mclk stay accessible — which is exactly why we cleared FORCE_HCS in the first place (a PMA access with bit10 set bus-hangs the AP). CLKSTOP_CTRL (forced stops) still fully cleared, as Linux's gate_clk(false) does.
    fn unlock_clocks(&self) {
        self.hci_w(vs::FORCE_HCS, 0x9C0);
        self.hci_w(vs::CLKSTOP_CTRL, self.hci(vs::CLKSTOP_CTRL) & !vs::CLK_STOP_ALL);
    }

    /// Issue a UIC command and wait for completion. Returns the UCMDARG2 result code (0 = success) or Err on timeout / UCRDY never ready.
    pub fn uic_cmd(&self, cmd: u32, arg1: u32, arg2: u32, arg3: u32) -> Result<u32, ()> {
        let mut ready = false;
        for _ in 0..1_000_000u32 {
            if self.read_reg(regs::HCS) & regs::HCS_UCRDY != 0 {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(());
        }
        self.write_reg(regs::IS, regs::IS_UCCS);
        self.write_reg(regs::UCMDARG1, arg1);
        self.write_reg(regs::UCMDARG2, arg2);
        self.write_reg(regs::UCMDARG3, arg3);
        self.write_reg(regs::UICCMD, cmd & 0xFF);
        for _ in 0..1_000_000u32 {
            if self.read_reg(regs::IS) & regs::IS_UCCS != 0 {
                self.write_reg(regs::IS, regs::IS_UCCS);
                return Ok(self.read_reg(regs::UCMDARG2) & 0xFF);
            }
        }
        Err(())
    }

    /// DME_SET of a PA MIB attribute (selector = lane, 0 for link-global).
    pub fn dme_set(&self, attr: u32, selector: u32, val: u32) -> Result<(), u32> {
        match self.uic_cmd(uic::DME_SET, (attr << 16) | selector, 0, val) {
            Ok(0) => Ok(()),
            Ok(code) => Err(code),
            Err(()) => Err(0xFFFF_FFFF),
        }
    }

    /// DME_GET of a MIB attribute — returns UCMDARG3 (the value).
    pub fn dme_get(&self, attr: u32, selector: u32) -> Result<u32, u32> {
        match self.uic_cmd(uic::DME_GET, (attr << 16) | selector, 0, 0) {
            Ok(0) => Ok(self.read_reg(regs::UCMDARG3)),
            Ok(code) => Err(code),
            Err(()) => Err(0xFFFF_FFFF),
        }
    }

    /// Send a Query flag request (set / read). Returns (OCS, flag value from the response TSF).
    fn query_flag(&self, query_fn: u8, opcode: u8, idn: u8) -> (u8, u32) {
        unsafe {
            let buf = &raw mut UFS_BUF;
            core::ptr::write_bytes(&raw mut (*buf).ucd as *mut u8, 0, core::mem::size_of::<Ucd>());
            (*buf).ucd.cmd_upiu[0] = upiu::QUERY_REQ;
            (*buf).ucd.cmd_upiu[3] = 0; // task tag = doorbell slot
            (*buf).ucd.cmd_upiu[5] = query_fn;
            (*buf).ucd.cmd_upiu[12] = opcode;
            (*buf).ucd.cmd_upiu[13] = idn;
        }
        let ocs = self.send_command(1, 0, 0);
        let value = unsafe {
            let rsp = &(*(&raw const UFS_BUF)).ucd.rsp_upiu;
            ((rsp[20] as u32) << 24) | ((rsp[21] as u32) << 16) | ((rsp[22] as u32) << 8) | rsp[23] as u32
        };
        (ocs, value)
    }

    /// Full Exynos UFSHCI re-initialization from HCE reset, replaying what ABL's LK driver does: vendor config at enable time, M-PHY/UNIPRO cal, DME_LINKSTARTUP, device init, HS-G4 power mode.
    /// Leaves all UFS clocks forced on (FORCE_HCS = 0) — see `vs::FORCE_HCS_ALL_EN`.
    pub fn full_init(&self) -> InitReport {
        use crate::ufs_cal::{self, udelay};
        let mut r = InitReport::default();
        ufs_cal::trace_reset(); // record every MMIO write for a ferros-side FWTRACE vs Linux
        let mut done = |r: &mut InitReport, s: u32| r.steps |= 1 << s;
        macro_rules! fail {
            ($r:expr, $s:expr) => {{
                $r.fail_step = $s;
                return $r;
            }};
        }

        // 1. Clocks: kill every auto clock-stop and forced stop so nothing gates mid-sequence.
        self.unlock_clocks();
        done(&mut r, step::CLOCKS);

        // 2..9. Host enable + link startup, retried as a UNIT: on a failed DME_LINKSTARTUP, ufshcd re-runs the whole hba_enable (SW_RST, config, device reset, HCE cycle, cal) before trying again — a bare command retry on the same enable never recovers. We mirror that.
        let mut link_ok = false;
        let mut cal = ufs_cal::CalParams { available_lane: 2, connected_rx_lane: 1, active_rx_lane: 1 };
        for attempt in 1..=4u32 {
            r.linkstartup_tries = attempt;

            // HCE off.
            self.write_reg(regs::HCE, 0);
            let mut ok = false;
            for _ in 0..1_000_000u32 {
                if self.read_reg(regs::HCE) & 1 == 0 {
                    ok = true;
                    break;
                }
            }
            if !ok {
                fail!(r, step::HCE_OFF);
            }
            done(&mut r, step::HCE_OFF);

            // Vendor SW reset of link + UNIPRO logic.
            self.hci_w(vs::SW_RST, vs::SW_RST_MASK);
            let mut ok = false;
            for _ in 0..1_000_000u32 {
                if self.hci(vs::SW_RST) & vs::SW_RST_MASK == 0 {
                    ok = true;
                    break;
                }
            }
            if !ok {
                fail!(r, step::SW_RST);
            }
            done(&mut r, step::SW_RST);

            // config_host — the vendor block ABL applies before enable. NEXUS_TYPE here, BEFORE HCE 0->1, is the load-bearing line: it marks every tag as a nexus transfer at enable time.
            self.unlock_clocks();
            // IA_TICK_SEL via read-modify-write, matching exynos_ufs_fit_aggr_timeout. SW_RST resets this register to G#1, so Linux's working value is G#10001 — forcing ABL's captured G#40010000 here wrote a stale pre-reset value.
            self.hci_w(vs::V2P1_CTRL, self.hci(vs::V2P1_CTRL) | 1 << 16);
            self.hci_w(vs::US_TO_CNT_VAL, 0xB2); // ABL's aggregation-timer count (ACLK MHz); unused by our polling driver
            self.hci_w(vs::DATA_REORDER, 0xA);
            self.hci_w(vs::TXPRDT_ENTRY_SIZE, vs::PRDT_PREFETCH_EN | 12);
            self.hci_w(vs::RXPRDT_ENTRY_SIZE, 12);
            self.hci_w(vs::UTRL_NEXUS_TYPE, 0xFFFF_FFFF);
            self.hci_w(vs::UTMRL_NEXUS_TYPE, 0xFFFF_FFFF);
            self.hci_w(vs::AXIDMA_RWDATA_BURST_LEN, vs::WLU_EN | (3 << 27) | 3); // WLU_EN | BURST_LEN(3), matching the reference re-link path
            self.hci_w(vs::IOP_ACG_DISABLE, self.hci(vs::IOP_ACG_DISABLE) & !1);
            self.unipro_w(unip::PA_DBG_OPTION_SUITE_1, unip::DBG_SUITE1_ENABLE);
            self.unipro_w(unip::PA_DBG_OPTION_SUITE_2, unip::DBG_SUITE2_ENABLE);
            done(&mut r, step::CONFIG_HOST);

            // Route the UFS pinmux like the pinctrl framework does (pinctrl-0 = ufs_rst_n + ufs_refclk_out, both function 2). ABL may hand off with gph5-0 (REFCLKOUT) de-routed — the device then gets no reference clock and stays silent at link startup. gph5-0 = bits[3:0], gph5-1 = bits[7:4]; function 2 = nibble G#2.
            const GPH5CON: usize = 0x1306_0000;
            let con = unsafe { crate::mmio::read32(GPH5CON) };
            if r.gph5_con_before == 0 {
                r.gph5_con_before = con;
            }
            crate::ufs_cal::trace_write32(GPH5CON, (con & !0xFF) | 0x22);
            unsafe { core::arch::asm!("dsb sy") };
            r.gph5_con_after = unsafe { crate::mmio::read32(GPH5CON) };
            udelay(1_000); // let REFCLKOUT reach the device before reset/link

            // Hardware-reset the UFS device via the dedicated reset_n line, then give it time to boot before asking for a link.
            self.hci_w(vs::GPIO_OUT, 0);
            udelay(5);
            self.hci_w(vs::GPIO_OUT, 1);
            r.gph5_dat = unsafe { crate::mmio::read32(0x1306_0004) }; // confirm the reset_n pad tracks GPIO_OUT
            udelay(2_000);
            done(&mut r, step::DEV_RESET);

            // HCE on.
            self.write_reg(regs::HCE, 1);
            let mut ok = false;
            for _ in 0..1_000_000u32 {
                if self.read_reg(regs::HCE) & 1 == 1 {
                    ok = true;
                    break;
                }
                udelay(1);
            }
            if !ok {
                fail!(r, step::HCE_ON);
            }
            done(&mut r, step::HCE_ON);

            // Available lanes from UNIPRO. HCE 0->1 restores the FORCE_HCS defaults — unlock again or this read hangs the bus.
            self.unlock_clocks();
            r.avail_rx = self.unipro(unip::PA_AVAILRXDATALANES);
            cal.available_lane = if r.avail_rx == 2 { 2 } else { 1 };
            done(&mut r, step::LANES);

            // Linux's link_startup_notify PRE block (from the FWTRACE of a working bring-up): DFES layer error enables, then RE-write the PA debug option suites. The HCE 0->1 cycle resets the option suites, and the reference rewrites them here "to keep phy context ... for unipro v1.8" — writing them only in config_host (pre-HCE, as we did) leaves reset defaults in place at DME_LINKSTARTUP.
            self.hci_w(vs::ERROR_EN_DL_LAYER, 0x8000_2020);
            self.hci_w(vs::ERROR_EN_N_LAYER, 0x8000_0007);
            self.hci_w(vs::ERROR_EN_T_LAYER, 0x8000_0017);
            self.unipro_w(unip::PA_DBG_OPTION_SUITE_1, unip::DBG_SUITE1_ENABLE);
            self.unipro_w(unip::PA_DBG_OPTION_SUITE_2, unip::DBG_SUITE2_ENABLE);

            // Pre-link cal, then clear stale UIC error state so this attempt's codes are its own.
            r.cal_timeouts = (r.cal_timeouts & !0xFF) | (ufs_cal::pre_link(cal) & 0xFF);
            done(&mut r, step::PRE_LINK);
            // Capture the clock/refclk state the device depends on to respond to link startup.
            r.clkstop_ctrl = self.hci(vs::CLKSTOP_CTRL);
            r.force_hcs = self.hci(vs::FORCE_HCS);
            r.mphy_refclk_sel = self.hci(0x108);
            r.cmu_qch = unsafe { crate::mmio::read32(0x1300_30C4) };
            r.cmu_unipro_gate = unsafe { crate::mmio::read32(0x1300_2110) };
            // Verify the AUX-window PCS cal writes actually landed (PMA writes already confirmed via register-diff; PCS is the unverified path).
            let pcs_2094 = ufs_cal::read_pcs(ufs_cal::RX_LANE0, 0x2094) & 0xFF; // expect G#F6
            let pcs_20bc = ufs_cal::read_pcs(ufs_cal::RX_LANE0, 0x20BC) & 0xFF; // expect G#79
            let pcs_22a4 = ufs_cal::read_pcs(ufs_cal::TX_LANE0, 0x22A4) & 0xFF; // expect G#02
            r.pcs_readback = pcs_2094 | (pcs_20bc << 8) | (pcs_22a4 << 16);
            let _ = self.read_reg(0x38); // UECPA is clear-on-read
            self.write_reg(regs::IS, 0xFFFF_FFFF);

            match self.uic_cmd(uic::DME_LINKSTARTUP, 0, 0, 0) {
                Ok(0) => {
                    r.linkstartup_res = 0;
                    link_ok = true;
                }
                Ok(code) => r.linkstartup_res = code,
                Err(()) => r.linkstartup_res = 0xFFFF_FFFF,
            }
            // UNIPRO-level forensics for the last attempt (success or failure).
            r.ls_cnf = self.unipro(0x7854); // DME_LINKSTARTUP_CNF_RESULT
            r.dme_err = self.unipro(0x7B20); // DME_INTR_ERROR_CODE
            r.pa_state = self.unipro(0x15C); // DBG_PA_CTRLSTATE
            r.pa_tx_state = self.unipro(0x160); // DBG_PA_TX_STATE
            r.uecpa = self.read_reg(0x38);
            // UEC family (each clear-on-read): DL G#3C, N G#40, T G#44, DME G#48. Pack the low bytes.
            let uecdl = self.read_reg(0x3C) & 0xFF;
            let uecn = self.read_reg(0x40) & 0xFF;
            let uect = self.read_reg(0x44) & 0xFF;
            let uecdme = self.read_reg(0x48) & 0xFF;
            r.uec_pack = uecdl | (uecn << 8) | (uect << 16) | (uecdme << 24);
            if link_ok {
                break;
            }
            udelay(100_000);
        }
        if !link_ok {
            fail!(r, step::LINKSTARTUP);
        }
        done(&mut r, step::LINKSTARTUP);

        // 10. Device present.
        let mut ok = false;
        for _ in 0..1_000_000u32 {
            if self.read_reg(regs::HCS) & regs::HCS_DP != 0 {
                ok = true;
                break;
            }
        }
        r.hcs_after_link = self.read_reg(regs::HCS);
        if !ok {
            fail!(r, step::DEVICE_PRESENT);
        }
        done(&mut r, step::DEVICE_PRESENT);

        // 11. Post-link cal with the lane picture the link negotiated.
        self.unlock_clocks();
        r.conn_rx = self.unipro(unip::PA_CONNECTEDRXDATALANES);
        r.active_rx = self.unipro(unip::PA_ACTIVERXDATALANES);
        cal.connected_rx_lane = r.conn_rx;
        cal.active_rx_lane = r.active_rx;
        r.cal_timeouts |= (ufs_cal::post_link(cal) & 0xFF) << 8;
        done(&mut r, step::POST_LINK);

        // 12. Transfer + task-management lists (standard make_hba_operational).
        let utmrl = &raw const UTMRL_BUF as usize as u64;
        self.write_reg(regs::UTMRLBA, utmrl as u32);
        self.write_reg(regs::UTMRLBAU, (utmrl >> 32) as u32);
        self.write_reg(regs::UTMRLRSR, 1);
        self.init_transfer_list();
        done(&mut r, step::LISTS);

        // 13. NOP OUT — the device just came out of hardware reset, so give it retries.
        let mut nop_ocs = 0xFFu8;
        for attempt in 1..=8u32 {
            r.nop_tries = attempt;
            let (ocs, rsp, _) = self.send_nop();
            nop_ocs = ocs;
            if ocs == ocs::SUCCESS && rsp == upiu::NOP_IN {
                break;
            }
            udelay(20_000);
        }
        r.nop_ocs = nop_ocs as u32;
        if nop_ocs != ocs::SUCCESS {
            fail!(r, step::NOP);
        }
        done(&mut r, step::NOP);

        // 14. fDeviceInit: set the flag, poll until the device clears it (boot LUN scan etc).
        let (set_ocs, _) = self.query_flag(0x81, 0x06, 0x01);
        r.fdev_ocs = set_ocs as u32;
        if set_ocs != ocs::SUCCESS {
            fail!(r, step::FDEVICEINIT);
        }
        let mut ok = false;
        for poll in 1..=512u32 {
            r.fdev_polls = poll;
            let (ocs, val) = self.query_flag(0x01, 0x05, 0x01);
            if ocs == ocs::SUCCESS && val & 1 == 0 {
                ok = true;
                break;
            }
            udelay(4_000);
        }
        if !ok {
            fail!(r, step::FDEVICEINIT);
        }
        done(&mut r, step::FDEVICEINIT);

        // 15+16. Power-mode change to FAST HS-G4 x2, series B. Failure past this point is non-fatal: the link still works at PWM-G1.
        self.unlock_clocks();
        r.cal_timeouts |= (ufs_cal::pre_pmc_hs_b(cal) & 0xFF) << 16;
        done(&mut r, step::PMC_CAL);
        let lanes = if r.conn_rx == 2 { 2 } else { 1 };
        let sets: [(u32, u32); 8] = [
            (pa::ACTIVETXDATALANES, lanes),
            (pa::ACTIVERXDATALANES, lanes),
            (pa::TXGEAR, 4),
            (pa::RXGEAR, 4),
            (pa::TXTERMINATION, 1),
            (pa::RXTERMINATION, 1),
            (pa::HSSERIES, 2),
            (pa::PWRMODE, 0x11), // FAST_MODE rx | tx — must be last, triggers the change
        ];
        self.write_reg(regs::IS, regs::IS_UPMS);
        for (i, &(attr, val)) in sets.iter().enumerate() {
            if let Err(code) = self.dme_set(attr, 0, val) {
                r.pmc_set_fail = (((i + 1) as u32) << 8) | (code & 0xFF);
                r.fail_step = step::PMC;
                r.hcs_final = self.read_reg(regs::HCS);
                return r;
            }
        }
        let mut ok = false;
        for _ in 0..1_000_000u32 {
            if self.read_reg(regs::IS) & regs::IS_UPMS != 0 {
                self.write_reg(regs::IS, regs::IS_UPMS);
                ok = true;
                break;
            }
        }
        r.hcs_final = self.read_reg(regs::HCS);
        r.upmcrs = (r.hcs_final >> 8) & 0x7;
        if !ok || r.upmcrs != 1 {
            fail!(r, step::PMC);
        }
        done(&mut r, step::PMC);
        // Zuma post_pmc tables are empty with AH8 cal off — nothing to do.

        r
    }
}
