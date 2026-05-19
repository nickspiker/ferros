//! DWC3 USB device-mode driver for Apple M1.
//!
//! Synopsys DWC3 with Apple ATCPHY, PipeHandler, and DART IOMMU.
//! Implements the `UsbBulk` trait for Photon Transport.
//!
//! Register base addresses must be read from the Apple Device Tree (ADT)
//! and passed to `M1Usb::init()`. Use `tools/m1n1-boot.py` to dump them.

use ferros_hal::mmio;
use ferros_hal::{UsbBulk, UsbEvent};
use crate::dart::Dart;

// ---------------------------------------------------------------------------
// DWC3 register offsets (Synopsys IP, same on all platforms)
// ---------------------------------------------------------------------------

const GCTL: usize = 0xC110;
const GSTS: usize = 0xC118;
const GSNPSID: usize = 0xC120;
const GHWPARAMS3: usize = 0xC14C;
const GUSB2PHYCFG: usize = 0xC200;
const GUSB3PIPECTL: usize = 0xC2C0;
const GEVNTADRLO: usize = 0xC400;
const GEVNTADRHI: usize = 0xC404;
const GEVNTSIZ: usize = 0xC408;
const GEVNTCOUNT: usize = 0xC40C;
const DCFG: usize = 0xC700;
const DCTL: usize = 0xC704;
const DEVTEN: usize = 0xC708;
const DSTS: usize = 0xC70C;
const DGCMD: usize = 0xC714;
const DGCMDPAR: usize = 0xC710;
const DALEPENA: usize = 0xC720;

// GCTL bits
const GCTL_PRTCAPDIR_MASK: u32 = 0x3 << 12;
const GCTL_PRTCAPDIR_DEVICE: u32 = 0x2 << 12;
const GCTL_CORESOFTRESET: u32 = 1 << 11;
const GCTL_SCALEDOWN_MASK: u32 = 0x3 << 4;
const GCTL_DISSCRAMBLE: u32 = 1 << 3;

// DCTL bits
const DCTL_RUN_STOP: u32 = 1 << 31;
const DCTL_CSFTRST: u32 = 1 << 30;

// DCFG bits
const DCFG_SPEED_MASK: u32 = 0x7;
const DCFG_SPEED_HS: u32 = 0;
const DCFG_DEVADDR_SHIFT: u32 = 3;
const DCFG_DEVADDR_MASK: u32 = 0x7F << 3;

// DEVTEN bits
const DEVTEN_DISCONNEVTEN: u32 = 1 << 0;
const DEVTEN_USBRSTEN: u32 = 1 << 1;
const DEVTEN_CONNECTDONEEN: u32 = 1 << 2;

// GEVNTSIZ bits
const GEVNTSIZ_INTMASK: u32 = 1 << 31;

// GSNPSID — DWC3 = 0x5533xxxx, DWC_usb31 = 0x3331xxxx (M1 uses DWC31)
const GSNPSID_DWC3_MASK: u32 = 0xFFFF_0000;
const GSNPSID_DWC31_PREFIX: u32 = 0x3331_0000;

// DEPCMD command codes
const DEPCMD_DEPSTARTCFG: u32 = 0x09;
const DEPCMD_SETEPCONFIG: u32 = 0x01;
const DEPCMD_SETTRANSFRESOURCE: u32 = 0x02;
const DEPCMD_STARTTRANSFER: u32 = 0x06;
const DEPCMD_ENDTRANSFER: u32 = 0x08;
const DEPCMD_SETSTALL: u32 = 0x04;
const DEPCMD_CMDACT: u32 = 1 << 10;
const DEPCMD_HIPRI_FORCERM: u32 = 1 << 9;

// DEPCMD SETEPCONFIG parameter bits
const DEPCFGPAR0_EPTYPE_SHIFT: u32 = 1;
const DEPCFGPAR0_MPS_SHIFT: u32 = 3;
const DEPCFGPAR0_FIFONUM_SHIFT: u32 = 17;
const EP_TYPE_CONTROL: u32 = 0;
const EP_TYPE_BULK: u32 = 2;
const DEPCFGPAR1_EPNUM_SHIFT: u32 = 25;
const DEPCFGPAR1_XFER_CMPL_EN: u32 = 1 << 8;
const DEPCFGPAR1_XFER_NRDY_EN: u32 = 1 << 10;

// TRB bits
const TRB_CTRL_HWO: u32 = 1 << 0;
const TRB_CTRL_LST: u32 = 1 << 1;
const TRB_CTRL_ISP_IMI: u32 = 1 << 10;
const TRB_CTRL_IOC: u32 = 1 << 11;
const TRB_CTRL_TRBCTL_SHIFT: u32 = 4;
const TRBCTL_NORMAL: u32 = 1;
const TRBCTL_SETUP: u32 = 2;
const TRBCTL_STATUS2: u32 = 3;
const TRBCTL_STATUS3: u32 = 4;
const TRBCTL_CONTROL_DATA: u32 = 5;

// Event types
const EVT_NON_EP: u32 = 1 << 0;
const DEVT_DISCONN: u32 = 0;
const DEVT_USBRST: u32 = 1;
const DEVT_CONNECTDONE: u32 = 2;
const DEPEVT_XFERCOMPLETE: u32 = 1;
const DEPEVT_XFERNOTREADY: u32 = 3;

// USB standard requests
const USB_REQ_GET_STATUS: u8 = 0;
const USB_REQ_SET_ADDRESS: u8 = 5;
const USB_REQ_GET_DESCRIPTOR: u8 = 6;
const USB_REQ_SET_CONFIGURATION: u8 = 9;
const USB_DT_DEVICE: u8 = 1;
const USB_DT_CONFIGURATION: u8 = 2;
const USB_DT_STRING: u8 = 3;
const USB_DT_DEVICE_QUALIFIER: u8 = 6;
const USB_DT_BOS: u8 = 15;

// DGCMD global commands (scratchpad setup)
const DGCMD_SET_SCRATCHPAD_LO: u32 = 0x04;
const DGCMD_SET_SCRATCHPAD_HI: u32 = 0x05;
const DGCMD_CMDACT: u32 = 1 << 10;

// PipeHandler offsets (from reg[3] of usb-drd0)
const PIPE_MUX_CTRL: usize = 0x0C;
const PIPE_AON_GEN: usize = 0x1C;
const PIPE_NONSEL_OVERRIDE: usize = 0x20;

// ---------------------------------------------------------------------------
// Static DMA buffers (BSS, DRAM)
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
struct EventBuffer { buf: [u32; 256] }

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct Trb {
    pub bpl: u32,
    pub bph: u32,
    pub size: u32,
    pub ctrl: u32,
}

impl Trb {
    const fn zero() -> Self { Trb { bpl: 0, bph: 0, size: 0, ctrl: 0 } }
}

#[repr(C, align(64))]
struct Ep0Trbs { setup: Trb, data: Trb, status: Trb, _pad: Trb }

#[repr(C, align(64))]
struct SetupPacket { data: [u8; 8] }

#[repr(C, align(64))]
struct Ep0DataBuf { data: [u8; 512] }

#[repr(C, align(64))]
struct BulkTrb { trb: Trb, _pad: [Trb; 3] }

#[repr(C, align(64))]
struct BulkOutBuf { data: [u8; 512] }

#[repr(C, align(64))]
struct BulkInBuf { data: [u8; 4096] }

#[repr(C, align(16384))]
struct Scratchpad { data: [u8; 16384] }

static mut EVT_BUF: EventBuffer = EventBuffer { buf: [0; 256] };
static mut EP0_TRBS: Ep0Trbs = Ep0Trbs {
    setup: Trb::zero(), data: Trb::zero(), status: Trb::zero(), _pad: Trb::zero(),
};
static mut EP0_SETUP_BUF: SetupPacket = SetupPacket { data: [0; 8] };
static mut EP0_DATA_BUF: Ep0DataBuf = Ep0DataBuf { data: [0; 512] };
static mut BULK_OUT_TRB: BulkTrb = BulkTrb { trb: Trb::zero(), _pad: [Trb::zero(); 3] };
static mut BULK_IN_TRB: BulkTrb = BulkTrb { trb: Trb::zero(), _pad: [Trb::zero(); 3] };
static mut BULK_OUT_BUF: BulkOutBuf = BulkOutBuf { data: [0; 512] };
static mut BULK_IN_BUF: BulkInBuf = BulkInBuf { data: [0; 4096] };
static mut SCRATCHPAD: Scratchpad = Scratchpad { data: [0; 16384] };

// ---------------------------------------------------------------------------
// USB Descriptors — ferros M1 device
// ---------------------------------------------------------------------------

// VID/PID: 0x1209/0x4665 (pid.codes, ferros — assigned via pid.codes PR #1208 on 2026-05-13).
// Same PID used by every ferros-shipped USB device. The VSF document's "PIPE message" section disambiguates protocol/role; PID-level multiplexing isn't needed.
static DEVICE_DESC: [u8; 18] = [
    18, 1,       // bLength, bDescriptorType (DEVICE)
    0x00, 0x02,  // bcdUSB (2.00)
    0xFF,        // bDeviceClass (vendor-specific)
    0x00,        // bDeviceSubClass
    0x00,        // bDeviceProtocol
    64,          // bMaxPacketSize0
    0x09, 0x12,  // idVendor (0x1209) LE
    0x65, 0x46,  // idProduct (0x4665) LE
    0x00, 0x01,  // bcdDevice (1.00)
    1, 2, 0,     // iManufacturer, iProduct, iSerialNumber
    1,           // bNumConfigurations
];

static CONFIG_DESC: [u8; 32] = [
    // Configuration descriptor
    9, 2,        // bLength, bDescriptorType (CONFIGURATION)
    32, 0,       // wTotalLength LE
    1,           // bNumInterfaces
    1,           // bConfigurationValue
    0,           // iConfiguration
    0x80,        // bmAttributes (bus-powered)
    250,         // bMaxPower (500mA)
    // Interface descriptor
    9, 4,        // bLength, bDescriptorType (INTERFACE)
    0,           // bInterfaceNumber
    0,           // bAlternateSetting
    2,           // bNumEndpoints
    0xFF,        // bInterfaceClass (vendor)
    0x00, 0x00,  // bInterfaceSubClass, bInterfaceProtocol
    0,           // iInterface
    // Endpoint OUT (bulk, EP1 OUT, 512B)
    7, 5, 0x01, 0x02, 0x00, 0x02, 0,
    // Endpoint IN (bulk, EP1 IN, 512B)
    7, 5, 0x81, 0x02, 0x00, 0x02, 0,
];

static DEVICE_QUALIFIER: [u8; 10] = [
    10, 6, 0x00, 0x02, 0xFF, 0x00, 0x00, 64, 1, 0,
];

// String descriptors: [0]=lang, [1]=manufacturer, [2]=product
static STR0_LANG: [u8; 4] = [4, 3, 0x09, 0x04]; // English (US)
// "ferros" in UTF-16LE
static STR1_MFG: [u8; 16] = [16, 3, b'f',0, b'e',0, b'r',0, b'r',0, b'o',0, b's',0, 0,0];
// "ferros M1" in UTF-16LE
static STR2_PROD: [u8; 22] = [22, 3, b'f',0, b'e',0, b'r',0, b'r',0, b'o',0, b's',0, b' ',0, b'M',0, b'1',0, 0,0];

// BOS descriptor (minimal, required for USB 2.1+)
static BOS_DESC: [u8; 5] = [5, 15, 5, 0, 0]; // bLength, bDescriptorType, wTotalLength, bNumDeviceCaps

// ---------------------------------------------------------------------------
// Device state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Ep0State { Setup, DataIn, DataOut, Status }

/// M1 DWC3 USB device controller.
pub struct M1Usb {
    base: usize,         // DWC3 register base
    dma_offset: i64,     // phys + dma_offset = iova (for DART)
    evt_read_idx: usize,
    ep0_state: Ep0State,
    address: u8,
    configured: bool,
    connected_speed: u32,
    ep0_resource_idx: u8,
    ep1_resource_idx: u8,
    bulk_out_resource_idx: u8,
    bulk_in_resource_idx: u8,
    bulk_out_len: u16,
    pub bulk_out_ready: bool,
    pub bulk_out_armed: bool,
    pub bulk_in_idle: bool,
    // Diagnostics
    pub ep_cmd_ok: u32,
    pub ep_cmd_fail: u32,
    pub last_cmd_status: u32,
    pub evt_count: u32,
    pub setup_count: u32,
    pub dart_config: u32,
    pub dart_tcr_before: u32,
    pub dart_ttbr_before: u32,
    pub dart_tcr_after: u32,
    pub dart_ttbr_after: u32,
    pub dma_offset_val: i64,
    pub l1_phys: u64,
    pub min_buf_page: u64,
    pub l1_readback: u64,
    pub setup_buf_dma: u64,
    pub setup_trb_dma: u64,
    /// Last 8 raw event words seen (ring buffer for debugging)
    pub raw_evts: [u32; 8],
    pub raw_evt_idx: usize,
}

impl M1Usb {
    /// Read a DWC3 register (for diagnostics from kernel_main).
    pub fn read_reg(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.base + offset) }
    }
}

/// Get the lowest 16KB-aligned page address among all DMA buffers.
pub fn dma_buffer_min_page() -> usize {
    let addrs: [usize; 9] = [
        &raw const EVT_BUF as usize,
        &raw const SCRATCHPAD as usize,
        &raw const EP0_TRBS as usize,
        &raw const EP0_SETUP_BUF as usize,
        &raw const EP0_DATA_BUF as usize,
        &raw const BULK_OUT_TRB as usize,
        &raw const BULK_IN_TRB as usize,
        &raw const BULK_OUT_BUF as usize,
        &raw const BULK_IN_BUF as usize,
    ];
    let mut min = usize::MAX;
    for &a in &addrs {
        let p = a & !0x3FFF;
        if p < min { min = p; }
    }
    min
}

/// Delay loop (~us at ~GHz clock).
fn delay(iters: u32) {
    for _ in 0..iters {
        unsafe { core::arch::asm!("nop") };
    }
}

/// M1 platform addresses (from ADT, filled in at init).
pub struct M1UsbAddrs {
    pub dwc3: usize,        // DWC3 core base (reg[0] of usb-drd0)
    pub pipe: usize,        // PipeHandler base (reg[3] of usb-drd0)
    pub atcphy: usize,      // ATCPHY base (reg[0] of atc-phy0)
    pub dart_base0: usize,  // DART USB0 reg[0] (0x382f00000)
    pub dart_base1: usize,  // DART USB0 reg[1] (0x382f80000) — T8020 dual bank
    pub dart_sid: u8,       // DART stream ID (typically 0)
}

impl M1Usb {
    /// Initialize ATCPHY (Apple Type-C PHY).
    fn atcphy_init(atc: usize) {
        unsafe {
            mmio::write32(atc + 0x08, 0x01c1_000f);
            mmio::write32(atc + 0x04, 0x0000_0003);
            mmio::write32(atc + 0x04, 0x0000_0000);
            mmio::write32(atc + 0x1c, 0x008c_0813);
            mmio::write32(atc + 0x00, 0x0000_0002);
        }
    }

    /// Initialize PipeHandler (MUX + reset control).
    fn pipe_handler_init(pipe: usize) {
        unsafe {
            mmio::write32(pipe + PIPE_MUX_CTRL, 0x22);      // Dummy mode
            mmio::write32(pipe + PIPE_AON_GEN, 0x01);        // DWC3_RESET_N
            mmio::write32(pipe + PIPE_NONSEL_OVERRIDE, 0x9332);
        }
    }

    /// Convert a physical address to a DMA address (IOVA).
    fn dma(&self, phys: usize) -> u64 {
        (phys as i64 + self.dma_offset) as u64
    }

    /// Issue a global DWC3 command.
    fn global_cmd(&self, cmd: u32, param: u32) -> bool {
        unsafe {
            mmio::write32(self.base + DGCMDPAR, param);
            mmio::write32(self.base + DGCMD, cmd | DGCMD_CMDACT);
            for _ in 0..100_000u32 {
                if mmio::read32(self.base + DGCMD) & DGCMD_CMDACT == 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Issue a DEPCMD to a physical endpoint.
    fn ep_cmd(&mut self, ep_phys: u8, cmd: u32, par0: u32, par1: u32, par2: u32) -> bool {
        let reg_base = self.base + 0xC800 + (ep_phys as usize) * 16;
        unsafe {
            mmio::write32(reg_base + 0x00, par2);
            mmio::write32(reg_base + 0x04, par1);
            mmio::write32(reg_base + 0x08, par0);
            mmio::write32(reg_base + 0x0C, cmd | DEPCMD_CMDACT);
            for _ in 0..100_000u32 {
                let val = mmio::read32(reg_base + 0x0C);
                if val & DEPCMD_CMDACT == 0 {
                    let status = (val >> 12) & 0xF;
                    if status == 0 {
                        self.ep_cmd_ok += 1;
                        return true;
                    } else {
                        self.ep_cmd_fail += 1;
                        self.last_cmd_status = val;
                        return false;
                    }
                }
            }
        }
        self.ep_cmd_fail += 1;
        false
    }

    /// Read the transfer resource index after a STARTTRANSFER command.
    fn read_resource_idx(&self, ep_phys: u8) -> u8 {
        let reg = self.base + 0xC800 + (ep_phys as usize) * 16 + 0x0C;
        ((unsafe { mmio::read32(reg) } >> 16) & 0x7F) as u8
    }

    /// Force ENDTRANSFER on an endpoint.
    fn force_end_transfer(&mut self, ep_phys: u8, resource_idx: u8) {
        if resource_idx != 0 {
            let cmd = DEPCMD_ENDTRANSFER | DEPCMD_HIPRI_FORCERM | ((resource_idx as u32) << 16);
            self.ep_cmd(ep_phys, cmd, 0, 0, 0);
        }
    }

    /// Initialize the M1 DWC3 USB controller.
    ///
    /// Full bringup: ATCPHY, PipeHandler, core+PHY reset, DART, endpoints.
    /// PMGR power domains are still active from m1n1.
    /// Initialize with a pre-computed DMA offset.
    /// Call `setup_dart()` first to configure the DART and get the offset.
    pub fn init(addrs: &M1UsbAddrs, dma_offset: i64) -> Option<Self> {
        let base = addrs.dwc3;

        // Verify DWC3 core presence
        let snpsid = unsafe { mmio::read32(base + GSNPSID) };
        let prefix = snpsid & GSNPSID_DWC3_MASK;
        if prefix != GSNPSID_DWC31_PREFIX && prefix != 0x5533_0000 {
            return None;
        }

        // Phase 1: DART setup from kernel.
        // m1n1's usb_iodev_shutdown calls dart_shutdown which zeroes TTBRs.
        // We must set up the DART fresh.
        //
        // First check: is the DART locked? If so, we can't program TTBRs.
        // Read DART state before we touch it (dart_base0 is the primary bank)
        let dart_config = unsafe { mmio::read32(addrs.dart_base0 + 0x60) };
        let dart_tcr0 = unsafe { mmio::read32(addrs.dart_base0 + 0x100) };
        let dart_ttbr0 = unsafe { mmio::read32(addrs.dart_base0 + 0x200) };
        // Store for kernel diagnostic printing
        // (we'll expose these via the M1Usb struct)

        // DART is set up by kernel_main before calling init().
        // dma_offset converts physical addresses to < 4GB IOVAs.
        let l1_readback: u64 = 0;

        // Phase 2: Device-mode reconfigure only. No PHY/core reset.
        // m1n1 left PHY powered (PMGR domains stay active after usb_iodev_shutdown).
        // Just CSFTRST to reset the device state machine, then reconfigure.
        unsafe {
            // Ensure device mode
            let gctl = mmio::read32(base + GCTL);
            mmio::write32(base + GCTL, (gctl & !GCTL_PRTCAPDIR_MASK) | GCTL_PRTCAPDIR_DEVICE);

            // HS speed
            let dcfg = mmio::read32(base + DCFG);
            mmio::write32(base + DCFG, (dcfg & !DCFG_SPEED_MASK) | DCFG_SPEED_HS);

            // Device controller soft reset
            let dctl = mmio::read32(base + DCTL);
            mmio::write32(base + DCTL, (dctl & !DCTL_RUN_STOP) | DCTL_CSFTRST);
            for _ in 0..100_000u32 {
                if mmio::read32(base + DCTL) & DCTL_CSFTRST == 0 { break; }
            }
        }
        delay(10_000);

        // Scratchpad setup (DWC31 on M1 requires this)
        let mut dev = M1Usb {
            base,
            dma_offset: dma_offset,
            evt_read_idx: 0,
            ep0_state: Ep0State::Setup,
            address: 0,
            configured: false,
            connected_speed: 0,
            ep0_resource_idx: 0,
            ep1_resource_idx: 0,
            bulk_out_resource_idx: 0,
            bulk_in_resource_idx: 0,
            bulk_out_len: 0,
            bulk_out_ready: false,
            bulk_out_armed: false,
            bulk_in_idle: true,
            ep_cmd_ok: 0,
            ep_cmd_fail: 0,
            last_cmd_status: 0,
            evt_count: 0,
            setup_count: 0,
            dart_config,
            dart_tcr_before: dart_tcr0,
            dart_ttbr_before: dart_ttbr0,
            dart_tcr_after: 0,  // filled in after init
            dart_ttbr_after: 0,
            dma_offset_val: dma_offset,
            l1_phys: 0,
            min_buf_page: 0,
            l1_readback: 0,
            setup_buf_dma: 0,
            setup_trb_dma: 0,
            raw_evts: [0; 8],
            raw_evt_idx: 0,
        };

        // Scratchpad setup
        let scratch_dma = dev.dma(&raw const SCRATCHPAD as usize);
        dev.global_cmd(DGCMD_SET_SCRATCHPAD_LO, scratch_dma as u32);
        dev.global_cmd(DGCMD_SET_SCRATCHPAD_HI, (scratch_dma >> 32) as u32);

        // Event buffer setup
        unsafe {
            let evt_size = core::mem::size_of::<EventBuffer>() as u32;
            mmio::write32(base + GEVNTSIZ, evt_size | GEVNTSIZ_INTMASK);

            let pending = mmio::read32(base + GEVNTCOUNT);
            if pending > 0 { mmio::write32(base + GEVNTCOUNT, pending); }

            // Clear event buffer
            let buf_ptr = &raw mut EVT_BUF as *mut u32;
            for i in 0..256 { core::ptr::write_volatile(buf_ptr.add(i), 0); }

            let evt_dma = dev.dma(&raw const EVT_BUF as usize);
            mmio::write32(base + GEVNTADRLO, evt_dma as u32);
            mmio::write32(base + GEVNTADRHI, (evt_dma >> 32) as u32);
            mmio::write32(base + GEVNTSIZ, evt_size & !GEVNTSIZ_INTMASK);
            mmio::write32(base + GEVNTCOUNT, 0);
        }

        // Enable device events
        unsafe {
            mmio::write32(base + DEVTEN, DEVTEN_USBRSTEN | DEVTEN_CONNECTDONEEN | DEVTEN_DISCONNEVTEN);
        }

        // Configure endpoints
        dev.ep_start_config(0);
        dev.ep0_configure();
        dev.bulk_configure();

        // Start controller
        unsafe {
            let dctl = mmio::read32(base + DCTL);
            mmio::write32(base + DCTL, dctl | DCTL_RUN_STOP);
        }

        // Read back DART state for diagnostics
        dev.dart_tcr_after = unsafe { mmio::read32(addrs.dart_base0 + 0x100) };
        dev.dart_ttbr_after = unsafe { mmio::read32(addrs.dart_base0 + 0x200) };
        dev.dma_offset_val = dma_offset;
        dev.l1_phys = 0;
        dev.min_buf_page = 0;
        dev.l1_readback = l1_readback;
        // Record what DMA addresses we'll use for EP0 SETUP
        let setup_phys = &raw const EP0_SETUP_BUF as usize;
        let trb_phys = &raw const EP0_TRBS as usize;
        dev.setup_buf_dma = dev.dma(setup_phys) as u64;
        dev.setup_trb_dma = dev.dma(trb_phys) as u64;

        Some(dev)
    }

    fn ep_start_config(&mut self, ep_phys: u8) {
        self.ep_cmd(ep_phys, DEPCMD_DEPSTARTCFG, 0, 0, 0);
    }

    fn ep0_configure(&mut self) {
        // EP0 OUT (phys 0)
        let par0 = (EP_TYPE_CONTROL << DEPCFGPAR0_EPTYPE_SHIFT) | (64 << DEPCFGPAR0_MPS_SHIFT);
        let par1 = (0 << DEPCFGPAR1_EPNUM_SHIFT) | DEPCFGPAR1_XFER_CMPL_EN | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(0, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(0, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // EP0 IN (phys 1)
        let par0 = (EP_TYPE_CONTROL << DEPCFGPAR0_EPTYPE_SHIFT) | (64 << DEPCFGPAR0_MPS_SHIFT);
        let par1 = (1 << DEPCFGPAR1_EPNUM_SHIFT) | DEPCFGPAR1_XFER_CMPL_EN | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(1, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(1, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // Enable EP0 IN+OUT
        unsafe {
            let ena = mmio::read32(self.base + DALEPENA);
            mmio::write32(self.base + DALEPENA, ena | 0x3);
        }
    }

    fn bulk_configure(&mut self) {
        self.ep_cmd(0, DEPCMD_DEPSTARTCFG | (2 << 16), 0, 0, 0);

        // EP1 OUT (phys 2) — Bulk 512B
        let par0 = (EP_TYPE_BULK << DEPCFGPAR0_EPTYPE_SHIFT) | (512 << DEPCFGPAR0_MPS_SHIFT);
        let par1 = (2 << DEPCFGPAR1_EPNUM_SHIFT) | DEPCFGPAR1_XFER_CMPL_EN | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(2, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(2, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // EP1 IN (phys 3) — Bulk 512B, FIFO 1
        let par0 = (EP_TYPE_BULK << DEPCFGPAR0_EPTYPE_SHIFT) | (512 << DEPCFGPAR0_MPS_SHIFT)
            | (1 << DEPCFGPAR0_FIFONUM_SHIFT);
        let par1 = (3 << DEPCFGPAR1_EPNUM_SHIFT) | DEPCFGPAR1_XFER_CMPL_EN | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(3, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(3, DEPCMD_SETTRANSFRESOURCE, 2, 0, 0);

        // Enable all 4 EPs
        unsafe {
            let ena = mmio::read32(self.base + DALEPENA);
            mmio::write32(self.base + DALEPENA, ena | 0xF);
        }
    }

    // EP0 helpers for control transfers
    fn ep0_send(&mut self, data: &[u8], max_len: u16) {
        let len = data.len().min(max_len as usize).min(512);
        unsafe {
            let dst = &raw mut EP0_DATA_BUF.data as *mut u8;
            for i in 0..len { core::ptr::write_volatile(dst.add(i), data[i]); }
            core::arch::asm!("dsb sy");

            let buf_phys = &raw const EP0_DATA_BUF as usize;
            let trb_phys = &raw mut EP0_TRBS.data as *mut Trb as usize;
            let trb = &raw mut EP0_TRBS.data;
            let buf_dma = self.dma(buf_phys);
            (*trb).bpl = buf_dma as u32;
            (*trb).bph = (buf_dma >> 32) as u32;
            (*trb).size = len as u32;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | TRB_CTRL_ISP_IMI
                | (TRBCTL_CONTROL_DATA << TRB_CTRL_TRBCTL_SHIFT);
            mmio::cache_clean(buf_phys, len);
            mmio::cache_clean(trb_phys, 16);
            core::arch::asm!("dsb sy");
        }
        self.ep0_state = Ep0State::DataIn;
        let trb_dma = self.dma(unsafe { &raw mut EP0_TRBS.data as *mut Trb as usize });
        if self.ep_cmd(1, DEPCMD_STARTTRANSFER, (trb_dma >> 32) as u32, trb_dma as u32, 0) {
            self.ep1_resource_idx = self.read_resource_idx(1);
        }
    }

    fn ep0_status_in(&mut self) {
        unsafe {
            let trb_phys = &raw mut EP0_TRBS.status as *mut Trb as usize;
            let trb = &raw mut EP0_TRBS.status;
            let buf_dma = self.dma(&raw const EP0_DATA_BUF as usize);
            (*trb).bpl = buf_dma as u32;
            (*trb).bph = (buf_dma >> 32) as u32;
            (*trb).size = 0;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | (TRBCTL_STATUS2 << TRB_CTRL_TRBCTL_SHIFT);
            mmio::cache_clean(trb_phys, 16);
            core::arch::asm!("dsb sy");
        }
        self.ep0_state = Ep0State::Status;
        let trb_dma = self.dma(unsafe { &raw mut EP0_TRBS.status as *mut Trb as usize });
        self.ep_cmd(1, DEPCMD_STARTTRANSFER, (trb_dma >> 32) as u32, trb_dma as u32, 0);
    }

    fn ep0_status_out(&mut self) {
        unsafe {
            let trb_phys = &raw mut EP0_TRBS.status as *mut Trb as usize;
            let trb = &raw mut EP0_TRBS.status;
            let buf_dma = self.dma(&raw const EP0_DATA_BUF as usize);
            (*trb).bpl = buf_dma as u32;
            (*trb).bph = (buf_dma >> 32) as u32;
            (*trb).size = 0;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | (TRBCTL_STATUS3 << TRB_CTRL_TRBCTL_SHIFT);
            mmio::cache_clean(trb_phys, 16);
            core::arch::asm!("dsb sy");
        }
        self.ep0_state = Ep0State::Status;
        let trb_dma = self.dma(unsafe { &raw mut EP0_TRBS.status as *mut Trb as usize });
        self.ep_cmd(0, DEPCMD_STARTTRANSFER, (trb_dma >> 32) as u32, trb_dma as u32, 0);
    }

    fn handle_get_descriptor(&mut self, dt: u8, idx: u8, max_len: u16) -> bool {
        match dt {
            USB_DT_DEVICE => { self.ep0_send(&DEVICE_DESC, max_len); true }
            USB_DT_CONFIGURATION => { self.ep0_send(&CONFIG_DESC, max_len); true }
            USB_DT_STRING => {
                match idx {
                    0 => { self.ep0_send(&STR0_LANG, max_len); true }
                    1 => { self.ep0_send(&STR1_MFG, max_len); true }
                    2 => { self.ep0_send(&STR2_PROD, max_len); true }
                    _ => false
                }
            }
            USB_DT_DEVICE_QUALIFIER => { self.ep0_send(&DEVICE_QUALIFIER, max_len); true }
            USB_DT_BOS => { self.ep0_send(&BOS_DESC, max_len); true }
            _ => false,
        }
    }
}

impl UsbBulk for M1Usb {
    fn poll_event(&mut self) -> UsbEvent {
        let count = unsafe { mmio::read32(self.base + GEVNTCOUNT) };
        if count == 0 { return UsbEvent::None; }

        let n_events = (count / 4) as usize;
        if n_events == 0 { return UsbEvent::None; }

        // Read one event
        let evt = unsafe {
            let buf = &raw const EVT_BUF as *const u32;
            mmio::cache_invalidate(buf.add(self.evt_read_idx) as usize, 4);
            core::ptr::read_volatile(buf.add(self.evt_read_idx))
        };
        self.evt_read_idx = (self.evt_read_idx + 1) % 256;
        self.evt_count += 1;
        self.raw_evts[self.raw_evt_idx % 8] = evt;
        self.raw_evt_idx += 1;
        unsafe { mmio::write32(self.base + GEVNTCOUNT, 4); }

        if evt & EVT_NON_EP != 0 {
            // Device event
            let evt_type = (evt >> 8) & 0xF;
            match evt_type {
                DEVT_USBRST => UsbEvent::Reset,
                DEVT_CONNECTDONE => {
                    let dsts = unsafe { mmio::read32(self.base + DSTS) };
                    UsbEvent::ConnectDone { speed: dsts & 0x7 }
                }
                DEVT_DISCONN => UsbEvent::Disconnect,
                _ => UsbEvent::None,
            }
        } else {
            // Endpoint event
            let ep_phys = (evt >> 1) & 0x1F;
            let evt_type = (evt >> 12) & 0xF;
            match evt_type {
                DEPEVT_XFERCOMPLETE => {
                    if ep_phys == 0 && self.ep0_state == Ep0State::Setup {
                        // EP0 OUT SETUP complete — read the 8-byte setup packet
                        self.setup_count += 1;
                        unsafe {
                            let buf_phys = &raw const EP0_SETUP_BUF as usize;
                            mmio::cache_invalidate(buf_phys, 8);
                            let mut request = [0u8; 8];
                            let src = buf_phys as *const u8;
                            for i in 0..8 {
                                request[i] = core::ptr::read_volatile(src.add(i));
                            }
                            return UsbEvent::Ep0Setup { request };
                        }
                    } else if ep_phys == 0 || ep_phys == 1 {
                        // EP0 data/status stage complete
                        if self.ep0_state == Ep0State::DataIn {
                            // Data sent, now do status OUT
                            self.ep0_status_out();
                        } else if self.ep0_state == Ep0State::Status {
                            // Status complete, re-arm for next SETUP
                            self.ep0_start_setup();
                        }
                        UsbEvent::TransferComplete { ep: ep_phys as u8 }
                    } else if ep_phys == 2 {
                        // Bulk OUT complete — read actual transfer size from TRB
                        unsafe {
                            let trb_phys = &raw const BULK_OUT_TRB as usize;
                            mmio::cache_invalidate(trb_phys, 16);
                            let remaining = core::ptr::read_volatile(&raw const BULK_OUT_TRB.trb.size);
                            let actual = 512u32.saturating_sub(remaining & 0x00FF_FFFF);
                            self.bulk_out_len = actual as u16;
                            self.bulk_out_ready = true;
                            self.bulk_out_armed = false;
                        }
                        UsbEvent::TransferComplete { ep: ep_phys as u8 }
                    } else if ep_phys == 3 {
                        self.bulk_in_idle = true;
                        UsbEvent::TransferComplete { ep: ep_phys as u8 }
                    } else {
                        UsbEvent::TransferComplete { ep: ep_phys as u8 }
                    }
                }
                DEPEVT_XFERNOTREADY => {
                    if ep_phys == 2 && !self.bulk_out_armed {
                        // Auto-arm bulk OUT
                        self.bulk_out_arm();
                    }
                    if ep_phys == 0 && self.ep0_state == Ep0State::Setup {
                        self.ep0_start_setup();
                    }
                    UsbEvent::TransferNotReady { ep: ep_phys as u8 }
                }
                _ => UsbEvent::None,
            }
        }
    }

    fn bulk_out_arm(&mut self) {
        self.bulk_out_armed = true;
        self.force_end_transfer(2, self.bulk_out_resource_idx);

        let buf_phys = &raw const BULK_OUT_BUF as usize;
        let trb_phys = unsafe { &raw mut BULK_OUT_TRB.trb as *mut Trb as usize };

        unsafe {
            let trb = &raw mut BULK_OUT_TRB.trb;
            let buf_dma = self.dma(buf_phys);
            (*trb).bpl = buf_dma as u32;
            (*trb).bph = (buf_dma >> 32) as u32;
            (*trb).size = 512;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC | TRB_CTRL_ISP_IMI
                | (TRBCTL_NORMAL << TRB_CTRL_TRBCTL_SHIFT);
            mmio::cache_clean(trb_phys, 16);
            mmio::cache_clean(buf_phys, 512);
            core::arch::asm!("dsb sy");
        }

        let trb_dma = self.dma(trb_phys);
        if self.ep_cmd(2, DEPCMD_STARTTRANSFER, (trb_dma >> 32) as u32, trb_dma as u32, 0) {
            self.bulk_out_resource_idx = self.read_resource_idx(2);
        }
    }

    fn bulk_out_read(&mut self) -> Option<&[u8]> {
        if !self.bulk_out_ready { return None; }
        self.bulk_out_ready = false;
        let len = self.bulk_out_len as usize;
        if len == 0 { return None; }
        unsafe {
            mmio::cache_invalidate(&raw const BULK_OUT_BUF as usize, len);
            Some(core::slice::from_raw_parts(&raw const BULK_OUT_BUF as *const u8, len))
        }
    }

    fn bulk_in_send(&mut self, data: &[u8]) -> bool {
        if !self.bulk_in_idle { return false; }
        let len = data.len().min(4096);
        if len == 0 { return false; }

        let buf_phys = &raw const BULK_IN_BUF as usize;
        let trb_phys = unsafe { &raw mut BULK_IN_TRB.trb as *mut Trb as usize };

        unsafe {
            let dst = &raw mut BULK_IN_BUF.data as *mut u8;
            for i in 0..len { core::ptr::write_volatile(dst.add(i), data[i]); }
            core::arch::asm!("dsb sy");

            let trb = &raw mut BULK_IN_TRB.trb;
            let buf_dma = self.dma(buf_phys);
            (*trb).bpl = buf_dma as u32;
            (*trb).bph = (buf_dma >> 32) as u32;
            (*trb).size = len as u32;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC | TRB_CTRL_ISP_IMI
                | (TRBCTL_NORMAL << TRB_CTRL_TRBCTL_SHIFT);
            mmio::cache_clean(buf_phys, len);
            mmio::cache_clean(trb_phys, 16);
            core::arch::asm!("dsb sy");
        }

        self.bulk_in_idle = false;
        let trb_dma = self.dma(trb_phys);
        if self.ep_cmd(3, DEPCMD_STARTTRANSFER, (trb_dma >> 32) as u32, trb_dma as u32, 0) {
            self.bulk_in_resource_idx = self.read_resource_idx(3);
            true
        } else {
            self.bulk_in_idle = true;
            false
        }
    }

    fn bulk_in_is_idle(&self) -> bool { self.bulk_in_idle }

    fn handle_reset(&mut self) {
        self.address = 0;
        self.configured = false;
        self.ep0_state = Ep0State::Setup;
        self.bulk_out_ready = false;
        self.bulk_out_armed = false;
        self.bulk_in_idle = true;
        // Re-address to 0
        unsafe {
            let dcfg = mmio::read32(self.base + DCFG);
            mmio::write32(self.base + DCFG, dcfg & !DCFG_DEVADDR_MASK);
        }
    }

    fn handle_connect_done(&mut self) {
        let dsts = unsafe { mmio::read32(self.base + DSTS) };
        self.connected_speed = dsts & 0x7;
        self.bulk_configure();
        self.ep0_start_setup();
    }

    fn handle_disconnect(&mut self) {
        self.configured = false;
        self.bulk_out_ready = false;
        self.bulk_out_armed = false;
        self.bulk_in_idle = true;
    }

    fn handle_setup(&mut self, request: &[u8; 8]) -> bool {
        let bm_request_type = request[0];
        let b_request = request[1];
        let w_value = u16::from_le_bytes([request[2], request[3]]);
        let w_length = u16::from_le_bytes([request[6], request[7]]);

        // Standard device requests (bmRequestType & 0x60 == 0)
        if bm_request_type & 0x60 == 0 {
            match b_request {
                USB_REQ_SET_ADDRESS => {
                    let addr = (w_value & 0x7F) as u8;
                    unsafe {
                        let dcfg = mmio::read32(self.base + DCFG);
                        mmio::write32(self.base + DCFG,
                            (dcfg & !DCFG_DEVADDR_MASK) | ((addr as u32) << DCFG_DEVADDR_SHIFT));
                    }
                    self.address = addr;
                    self.ep0_status_in();
                    return true;
                }
                USB_REQ_GET_DESCRIPTOR => {
                    let dt = (w_value >> 8) as u8;
                    let idx = (w_value & 0xFF) as u8;
                    if self.handle_get_descriptor(dt, idx, w_length) {
                        return true;
                    }
                }
                USB_REQ_SET_CONFIGURATION => {
                    self.configured = w_value == 1;
                    if self.configured {
                        self.bulk_out_arm();
                    }
                    self.ep0_status_in();
                    return true;
                }
                USB_REQ_GET_STATUS => {
                    let status: [u8; 2] = [0, 0];
                    self.ep0_send(&status, w_length);
                    return true;
                }
                _ => {}
            }
        }
        false // Stall unknown requests
    }

    fn ep0_start_setup(&mut self) {
        self.ep0_state = Ep0State::Setup;
        unsafe {
            let setup_phys = &raw const EP0_SETUP_BUF as usize;
            let trb_phys = &raw mut EP0_TRBS.setup as *mut Trb as usize;
            let trb = &raw mut EP0_TRBS.setup;
            let buf_dma = self.dma(setup_phys);
            (*trb).bpl = buf_dma as u32;
            (*trb).bph = (buf_dma >> 32) as u32;
            (*trb).size = 8;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | (TRBCTL_SETUP << TRB_CTRL_TRBCTL_SHIFT);
            mmio::cache_clean(trb_phys, 16);
            core::arch::asm!("dsb sy");
        }
        let trb_dma = self.dma(unsafe { &raw mut EP0_TRBS.setup as *mut Trb as usize });
        if self.ep_cmd(0, DEPCMD_STARTTRANSFER, (trb_dma >> 32) as u32, trb_dma as u32, 0) {
            self.ep0_resource_idx = self.read_resource_idx(0);
        }
    }

    fn ep0_stall(&mut self) {
        self.ep_cmd(0, DEPCMD_SETSTALL, 0, 0, 0);
        self.ep0_start_setup();
    }

    fn cancel_bulk_in(&mut self) {
        self.force_end_transfer(3, self.bulk_in_resource_idx);
        self.bulk_in_idle = true;
    }

    fn shutdown(&mut self) {
        self.force_end_transfer(2, self.bulk_out_resource_idx);
        self.force_end_transfer(3, self.bulk_in_resource_idx);
        unsafe {
            let dctl = mmio::read32(self.base + DCTL);
            mmio::write32(self.base + DCTL, dctl & !DCTL_RUN_STOP);
        }
    }
}
