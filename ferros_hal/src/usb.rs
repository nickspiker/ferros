//! Synopsys DWC3 USB controller driver (device mode).
//!
//! # WARNING — Development Quality
//!
//! This driver works for the current dev loop (diag, hot-reload) but is NOT production quality. Known issues:
//!
//! - **1ms send pacing**: bridge sleeps 1ms between OUT sends because single-TRB
//!   re-arm can't keep up with back-to-back host transfers. Proper fix: TRB ring or UPDATETRANSFER.
//! - **ENDTRANSFER per inbound packet**: wasteful, generates spurious events.
//!   Proper fix: TRB ring with pre-armed slots.
//! - **No error recovery**: stalled transfer = dead session, needs power cycle.
//! - **No kernel-side timeout**: bridge disconnect mid-transfer hangs forever.
//! - **28 debug counters**: pub fields in Dwc3Dev, most never read. Clean up.
//! - **bulk_out_armed tracking**: fragile flag, should be replaced by proper
//!   endpoint state machine.
//! - **No GIC/interrupt support**: polling only, burns CPU. Needs GIC setup + WFI.
//!
//! QCM6490 USB layout (from Linux DTS): Qualcomm wrapper: 0x0A6F_8800 (0x400 bytes) DWC3 core:        0x0A60_0000 (0xE000 bytes)
//!
//! DWC3 global registers start at core_base + 0xC100. Device-mode registers start at core_base + 0xC700.
//!
//! ## DWC3 Device Mode Architecture
//!
//! The DWC3 uses a command-based interface for endpoint management:
//! - **Event buffer**: DWC3 writes events (interrupts) to a DRAM ring buffer
//! - **TRBs** (Transfer Request Blocks): Describe DMA transfers for each endpoint
//! - **DEPCMD**: Per-endpoint command register for config/start/end transfers
//!
//! Endpoint numbering: physical EP = direction << 1 | ep_num EP0 OUT = 0, EP0 IN = 1, EP1 OUT = 2, EP1 IN = 3, ...

use crate::mmio;

// ---------------------------------------------------------------------------
// Base addresses
// ---------------------------------------------------------------------------

/// DWC3 core base address — set by platform init before calling Dwc3Dev::init(). Default: QCM6490 (G#A600000). Tensor G3: G#11210000.
static DWC3_BASE_ADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0x0A60_0000);

/// Get the current DWC3 base address.
#[inline(always)]
pub fn dwc3_base() -> usize {
    DWC3_BASE_ADDR.load(core::sync::atomic::Ordering::Relaxed)
}

/// Set the DWC3 base address. Must be called before Dwc3Dev::init().
pub fn set_dwc3_base(base: usize) {
    DWC3_BASE_ADDR.store(base, core::sync::atomic::Ordering::Relaxed);
}

/// Legacy constant for backward compatibility.
pub const DWC3_BASE: usize = 0x0A60_0000;

/// Qualcomm USB wrapper base (QCM6490 only).
pub const QCOM_WRAPPER: usize = 0x0A6F_8800;

// ---------------------------------------------------------------------------
// DWC3 Global register offsets (from DWC3_BASE) Hardware register map — not all used yet, defined for completeness.
// ---------------------------------------------------------------------------

#[allow(dead_code)] const GSBUSCFG0: usize = 0xC100;
#[allow(dead_code)] const GSBUSCFG1: usize = 0xC104;
const GCTL: usize = 0xC110;
const GSTS: usize = 0xC118;
const GSNPSID: usize = 0xC120;
#[allow(dead_code)] const GGPIO: usize = 0xC124;
#[allow(dead_code)] const GUID: usize = 0xC128;
#[allow(dead_code)] const GUCTL: usize = 0xC12C;
const GHWPARAMS0: usize = 0xC140;
const GHWPARAMS1: usize = 0xC144;
#[allow(dead_code)] const GHWPARAMS2: usize = 0xC148;
const GHWPARAMS3: usize = 0xC14C;
#[allow(dead_code)] const GHWPARAMS4: usize = 0xC150;
#[allow(dead_code)] const GHWPARAMS5: usize = 0xC154;
#[allow(dead_code)] const GHWPARAMS6: usize = 0xC158;
#[allow(dead_code)] const GHWPARAMS7: usize = 0xC15C;
#[allow(dead_code)] const GHWPARAMS8: usize = 0xC160;

// PHY interface config (important: PHYSOFTRST must be cleared after block reset)
const GUSB2PHYCFG: usize = 0xC200;
const GUSB3PIPECTL: usize = 0xC2C0;

// Event buffer registers (per-interrupter, n=0 for single interrupter)
const GEVNTADRLO: usize = 0xC400; // + 16*n
const GEVNTADRHI: usize = 0xC404;
const GEVNTSIZ: usize = 0xC408;
const GEVNTCOUNT: usize = 0xC40C;

// ---------------------------------------------------------------------------
// DWC3 Device-mode register offsets
// ---------------------------------------------------------------------------

const DCFG: usize = 0xC700;
const DCTL: usize = 0xC704;
const DEVTEN: usize = 0xC708;
const DSTS: usize = 0xC70C;
/// Device Active Logical Endpoint Enable — each bit enables one physical EP.
const DALEPENA: usize = 0xC720;

// Per-endpoint registers: base + 0xC800 + 16*ep_phys
#[allow(dead_code)] const DEPCMDPAR2: usize = 0xC800; // + 16*n
#[allow(dead_code)] const DEPCMDPAR1: usize = 0xC804;
#[allow(dead_code)] const DEPCMDPAR0: usize = 0xC808;
#[allow(dead_code)] const DEPCMD: usize = 0xC80C;

// ---------------------------------------------------------------------------
// GCTL bits
// ---------------------------------------------------------------------------

const GCTL_PRTCAPDIR_MASK: u32 = 0x3 << 12;
const GCTL_PRTCAPDIR_DEVICE: u32 = 0x2 << 12;

// ---------------------------------------------------------------------------
// DCTL bits
// ---------------------------------------------------------------------------

const DCTL_RUN_STOP: u32 = 1 << 31;
const DCTL_CSFTRST: u32 = 1 << 30;


// ---------------------------------------------------------------------------
// DCFG bits
// ---------------------------------------------------------------------------

/// Device speed [2:0].
const DCFG_SPEED_MASK: u32 = 0x7;
#[allow(dead_code)] const DCFG_SPEED_SS: u32 = 4;
const DCFG_SPEED_HS: u32 = 0;
/// Device address [10:3].
const DCFG_DEVADDR_SHIFT: u32 = 3;
const DCFG_DEVADDR_MASK: u32 = 0x7F << 3;

// ---------------------------------------------------------------------------
// DEVTEN bits (device event enables)
// ---------------------------------------------------------------------------

const DEVTEN_DISCONNEVTEN: u32 = 1 << 0;
const DEVTEN_USBRSTEN: u32 = 1 << 1;
const DEVTEN_CONNECTDONEEN: u32 = 1 << 2;
const DEVTEN_CMDCMPLEN: u32 = 1 << 14;

// ---------------------------------------------------------------------------
// DSTS bits
// ---------------------------------------------------------------------------

const DSTS_CONNECTSPD_MASK: u32 = 0x7;
#[allow(dead_code)] const DSTS_SPEED_HIGH: u32 = 0;
#[allow(dead_code)] const DSTS_SPEED_FULL: u32 = 1;
#[allow(dead_code)] const DSTS_SPEED_SUPER: u32 = 4;
#[allow(dead_code)] const DSTS_SPEED_SUPER_PLUS: u32 = 5;

// GSNPSID
const GSNPSID_DWC3_PREFIX: u32 = 0x5533_0000;
const GSNPSID_DWC3_MASK: u32 = 0xFFFF_0000;

// GEVNTSIZ bits
const GEVNTSIZ_INTMASK: u32 = 1 << 31;

// ---------------------------------------------------------------------------
// DEPCMD command codes
// ---------------------------------------------------------------------------

/// Set endpoint configuration.
const DEPCMD_DEPSTARTCFG: u32 = 0x09;
/// Set endpoint transfer resource configuration.
const DEPCMD_SETEPCONFIG: u32 = 0x01;
/// Set endpoint transfer resource.
const DEPCMD_SETTRANSFRESOURCE: u32 = 0x02;
/// Start transfer.
const DEPCMD_STARTTRANSFER: u32 = 0x06;
/// End transfer.
const DEPCMD_ENDTRANSFER: u32 = 0x08;
/// Update transfer (add TRBs to active transfer).
#[allow(dead_code)] const DEPCMD_UPDATETRANSFER: u32 = 0x07;
/// Set EP stall.
const DEPCMD_SETSTALL: u32 = 0x04;
/// Clear EP stall.
const DEPCMD_CLEARSTALL: u32 = 0x05;

/// Command Active bit — set when issuing, cleared by HW on completion.
const DEPCMD_CMDACT: u32 = 1 << 10;
/// Command Interrupt on Complete.
#[allow(dead_code)] const DEPCMD_CMDIOC: u32 = 1 << 8;
/// High Priority / Force Remove — required for forced ENDTRANSFER.
const DEPCMD_HIPRI_FORCERM: u32 = 1 << 9;

// ---------------------------------------------------------------------------
// DEPCMD SETEPCONFIG parameter bits (PAR0)
// ---------------------------------------------------------------------------

/// EP type [5:1] in DEPCMDPAR0.
const DEPCFGPAR0_EPTYPE_SHIFT: u32 = 1;
/// Max packet size [28:3] in DEPCMDPAR0 — actually [25:3].
const DEPCFGPAR0_MPS_SHIFT: u32 = 3;
/// FIFO number [21:17] in DEPCMDPAR0.
const DEPCFGPAR0_FIFONUM_SHIFT: u32 = 17;
/// Burst size [25:22] in DEPCMDPAR0.
#[allow(dead_code)] const DEPCFGPAR0_BRSTSIZ_SHIFT: u32 = 22;

/// EP type values.
const EP_TYPE_CONTROL: u32 = 0;
#[allow(dead_code)] const EP_TYPE_ISOC: u32 = 1;
const EP_TYPE_BULK: u32 = 2;
#[allow(dead_code)] const EP_TYPE_INTERRUPT: u32 = 3;

// DEPCMDPAR1 bits
/// USB endpoint number [29:25].
const DEPCFGPAR1_EPNUM_SHIFT: u32 = 25;
/// Transfer event enable.
const DEPCFGPAR1_XFER_CMPL_EN: u32 = 1 << 8;
/// Xfer not ready enable.
const DEPCFGPAR1_XFER_NRDY_EN: u32 = 1 << 10;

// ---------------------------------------------------------------------------
// TRB (Transfer Request Block) — 16 bytes, must be in DMA-accessible DRAM
// ---------------------------------------------------------------------------

/// TRB Control bits.
const TRB_CTRL_HWO: u32 = 1 << 0;   // Hardware Owns
const TRB_CTRL_LST: u32 = 1 << 1;   // Last TRB
const TRB_CTRL_ISP_IMI: u32 = 1 << 10; // Interrupt on Short/Miss
const TRB_CTRL_IOC: u32 = 1 << 11;     // Interrupt on Complete
/// TRB type field [9:4] (6 bits).
const TRB_CTRL_TRBCTL_SHIFT: u32 = 4;

/// TRB types.
const TRBCTL_NORMAL: u32 = 1;
const TRBCTL_SETUP: u32 = 2;         // Control-Setup (EP0 OUT)
const TRBCTL_STATUS2: u32 = 3;       // Control-Status 2 (no-data)
const TRBCTL_STATUS3: u32 = 4;       // Control-Status 3 (with data)
const TRBCTL_CONTROL_DATA: u32 = 5;  // Control-Data

// ---------------------------------------------------------------------------
// Event types (from event buffer entries)
// ---------------------------------------------------------------------------

/// Device-specific event (bit 0 = 0 for EP event, 1 for device event).
const EVT_NON_EP: u32 = 1 << 0;

/// Device event types [11:8].
const DEVT_DISCONN: u32 = 0;
const DEVT_USBRST: u32 = 1;
const DEVT_CONNECTDONE: u32 = 2;
#[allow(dead_code)] const DEVT_ULSTCHNG: u32 = 3;
#[allow(dead_code)] const DEVT_CMDCMPLT: u32 = 10;

/// EP event: transfer complete.
const DEPEVT_XFERCOMPLETE: u32 = 1;
/// EP event: transfer not ready.
const DEPEVT_XFERNOTREADY: u32 = 3;

// ---------------------------------------------------------------------------
// USB Standard Requests
// ---------------------------------------------------------------------------

const USB_REQ_GET_STATUS: u8 = 0;
const USB_REQ_SET_ADDRESS: u8 = 5;
const USB_REQ_GET_DESCRIPTOR: u8 = 6;
const USB_REQ_SET_CONFIGURATION: u8 = 9;

/// Vendor debug readout request (bmRequestType G#C0, device-to-host vendor).
/// Returns 16 LE u32 counters — a live window into the PT dispatch loop that works even when the bulk path is wedged, since EP0 keeps running.
pub const VENDOR_REQ_DBG: u8 = 0x5A;

/// Raw event-path readout (bmRequestType G#C0): last 12 EP event words + count + ep2 TRB snapshots + last DEPCMD failure.
pub const VENDOR_REQ_DBG2: u8 = 0x5B;

/// Debug counter block served by VENDOR_REQ_DBG. Slots 0-11 belong to the kernel PT loop; slots 12-15 are driver state. Single-core, volatile access only.
pub static mut DBG_PT: [u32; 16] = [0; 16];

/// Set a debug counter slot.
pub fn dbg_set(idx: usize, val: u32) {
    unsafe { core::ptr::write_volatile(&raw mut DBG_PT[idx & 15], val); }
}

/// Increment a debug counter slot.
pub fn dbg_bump(idx: usize) {
    unsafe {
        let p = &raw mut DBG_PT[idx & 15];
        core::ptr::write_volatile(p, core::ptr::read_volatile(p).wrapping_add(1));
    }
}

const USB_DT_DEVICE: u8 = 1;
const USB_DT_CONFIGURATION: u8 = 2;
const USB_DT_STRING: u8 = 3;
const USB_DT_INTERFACE: u8 = 4;
const USB_DT_ENDPOINT: u8 = 5;
const USB_DT_DEVICE_QUALIFIER: u8 = 6;
const USB_DT_BOS: u8 = 15;

// ---------------------------------------------------------------------------
// Qualcomm wrapper registers (offsets from QCOM_WRAPPER)
// ---------------------------------------------------------------------------

const QCOM_GENERAL_CFG: usize = 0x08;
const QCOM_HS_PHY_CTRL: usize = 0x10;
const QCOM_SS_PHY_CTRL: usize = 0x30;
#[allow(dead_code)] const QCOM_PWR_EVNT_IRQ_STAT: usize = 0x58;

// ---------------------------------------------------------------------------
// GCC (Global Clock Controller) reset registers
// ---------------------------------------------------------------------------

const GCC_BASE: usize = 0x0010_0000;
const GCC_QUSB2PHY_PRIM_BCR: usize = GCC_BASE + 0x12000;
#[allow(dead_code)] const GCC_USB30_PRIM_BCR: usize = GCC_BASE + 0x0F000;
#[allow(dead_code)] const GCC_USB30_PRIM_GDSCR: usize = GCC_BASE + 0x0F004;

// ---------------------------------------------------------------------------
// SNPS Femto v2 HS PHY registers (offsets from QUSB2_PHY_BASE = 0x088E3000)
// ---------------------------------------------------------------------------

const QUSB2_PHY_BASE: usize = 0x088E_3000;
const QMP_USB3_PHY_BASE: usize = 0x088E_8000;

// PHY register offsets
const PHY_UTMI_CTRL0: usize = 0x3C;
const PHY_UTMI_CTRL5: usize = 0x50;
const PHY_HS_CTRL_COMMON0: usize = 0x54;
const PHY_HS_CTRL_COMMON1: usize = 0x58;
const PHY_HS_CTRL_COMMON2: usize = 0x5C;
const PHY_HS_CTRL1: usize = 0x60;
const PHY_HS_CTRL2: usize = 0x64;
const PHY_CFG0: usize = 0x94;
const PHY_REFCLK_CTRL: usize = 0xA0;

// PHY bit definitions
const PHY_UTMI_CTRL0_SLEEPM: u32 = 1 << 0;
const PHY_UTMI_CTRL5_POR: u32 = 1 << 1;
const PHY_HS_COMMON0_SIDDQ: u32 = 1 << 2;
const PHY_HS_COMMON0_FSEL_MASK: u32 = 0x7 << 4;
const PHY_HS_COMMON1_VBUSVLDEXTSEL: u32 = 1 << 4;
const PHY_HS_COMMON1_PLLBTUNE: u32 = 1 << 5;
const PHY_HS_COMMON2_VREGBYPASS: u32 = 1 << 0;
const PHY_HS_CTRL1_VBUSVLDEXT: u32 = 1 << 0;
const PHY_HS_CTRL2_SUSPEND_N: u32 = 1 << 2;
const PHY_HS_CTRL2_SUSPEND_N_SEL: u32 = 1 << 3;
const PHY_CFG0_CMN_CTRL_OVERRIDE: u32 = 1 << 1;

// QSCRATCH HS PHY CTRL bits
const QSCRATCH_HS_UTMI_OTG_VBUS_VALID: u32 = 1 << 20;
const QSCRATCH_HS_SW_SESSVLD_SEL: u32 = 1 << 28;
// QSCRATCH SS PHY CTRL bits
const QSCRATCH_SS_LANE0_PWR_PRESENT: u32 = 1 << 24;

// ---------------------------------------------------------------------------
// Static buffers — placed in BSS, guaranteed DRAM
// ---------------------------------------------------------------------------

/// Event buffer — 256 entries * 4 bytes = 1024 bytes. Cache-line aligned (64B) to isolate from other DMA buffers.
#[repr(C, align(64))]
struct EventBuffer {
    buf: [u32; 256],
}

/// EP0 TRB ring — small, just 4 TRBs for setup/data/status. Cache-line aligned (64B) to isolate from other DMA buffers.
#[repr(C, align(64))]
struct Ep0Trbs {
    setup: Trb,   // Setup stage TRB
    data: Trb,    // Data stage TRB
    status: Trb,  // Status stage TRB
    _pad: Trb,    // Padding/link
}

/// A single TRB.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct Trb {
    pub bpl: u32,   // Buffer Pointer Low
    pub bph: u32,   // Buffer Pointer High
    pub size: u32,  // Transfer size + status
    pub ctrl: u32,  // Control (HWO, type, flags)
}

impl Trb {
    const fn zero() -> Self {
        Trb { bpl: 0, bph: 0, size: 0, ctrl: 0 }
    }
}

static mut EVT_BUF: EventBuffer = EventBuffer { buf: [0; 256] };
static mut EP0_TRBS: Ep0Trbs = Ep0Trbs {
    setup: Trb::zero(),
    data: Trb::zero(),
    status: Trb::zero(),
    _pad: Trb::zero(),
};

/// EP0 setup packet buffer (8 bytes, but cache-line aligned to prevent DMA bleed-over from SETUP DMA writes into adjacent buffers).
#[repr(C, align(64))]
struct SetupPacket {
    data: [u8; 8],
}
static mut EP0_SETUP_BUF: SetupPacket = SetupPacket { data: [0; 8] };

/// EP0 data buffer for control transfers. Cache-line aligned (64B) to isolate from other DMA buffers.
#[repr(C, align(64))]
struct Ep0DataBuf {
    data: [u8; 512],
}
static mut EP0_DATA_BUF: Ep0DataBuf = Ep0DataBuf { data: [0; 512] };

/// Bulk endpoint TRBs — one per direction, cache-line aligned.
#[repr(C, align(64))]
struct BulkOutTrb { trb: Trb, _pad: [Trb; 3] }
#[repr(C, align(64))]
struct BulkInTrb { trb: Trb, _pad: [Trb; 3] }

static mut BULK_OUT_TRB: BulkOutTrb = BulkOutTrb { trb: Trb::zero(), _pad: [Trb::zero(); 3] };
static mut BULK_IN_TRB: BulkInTrb = BulkInTrb { trb: Trb::zero(), _pad: [Trb::zero(); 3] };

/// Bulk OUT data buffer (host→device). 512B for HS bulk MPS.
#[repr(C, align(64))]
struct BulkOutBuf { data: [u8; 512] }
/// Bulk IN data buffer (device→host). 4KB for batching log output.
#[repr(C, align(64))]
struct BulkInBuf { data: [u8; 4096] }

static mut BULK_OUT_BUF: BulkOutBuf = BulkOutBuf { data: [0; 512] };
static mut BULK_IN_BUF: BulkInBuf = BulkInBuf { data: [0; 4096] };

// ---------------------------------------------------------------------------
// Probe (read-only diagnostics)
// ---------------------------------------------------------------------------

/// Result of probing the DWC3 controller.
pub struct Dwc3Info {
    pub snpsid: u32,
    pub gctl: u32,
    pub gsts: u32,
    pub hwparams0: u32,
    pub hwparams1: u32,
    pub hwparams3: u32,
    pub dsts: u32,
    pub dcfg: u32,
}

impl Dwc3Info {
    pub fn is_valid(&self) -> bool {
        (self.snpsid & GSNPSID_DWC3_MASK) == GSNPSID_DWC3_PREFIX
    }

    pub fn revision(&self) -> u16 {
        (self.snpsid & 0xFFFF) as u16
    }

    pub fn num_eps(&self) -> u32 {
        (self.hwparams3 >> 12) & 0x3F
    }

}

pub fn probe() -> Dwc3Info {
    unsafe {
        Dwc3Info {
            snpsid: mmio::read32(dwc3_base() + GSNPSID),
            gctl: mmio::read32(dwc3_base() + GCTL),
            gsts: mmio::read32(dwc3_base() + GSTS),
            hwparams0: mmio::read32(dwc3_base() + GHWPARAMS0),
            hwparams1: mmio::read32(dwc3_base() + GHWPARAMS1),
            hwparams3: mmio::read32(dwc3_base() + GHWPARAMS3),
            dsts: mmio::read32(dwc3_base() + DSTS),
            dcfg: mmio::read32(dwc3_base() + DCFG),
        }
    }
}

/// Diagnostic register dump after init. Reads back all critical registers so we can see the controller state.
pub struct Dwc3Diag {
    pub dctl: u32,
    pub dsts: u32,
    pub dcfg: u32,
    pub devten: u32,
    pub gctl: u32,
    pub gsts: u32,
    pub gevntadrlo: u32,
    pub gevntadrhi: u32,
    pub gevntsiz: u32,
    pub gevntcount: u32,
    // Qualcomm wrapper
    pub qcom_general_cfg: u32,
    pub qcom_hs_phy_ctrl: u32,
    pub qcom_ss_phy_ctrl: u32,
    // QUSB2 PHY (0x88E3000)
    pub qusb2_pwr_ctrl: u32,
    // QMP USB3 PHY (0x88E8000)
    pub qmp_pwr_ctrl: u32,
    // USB2 PHY status
    pub usb2_phy_utmi_ctrl: u32,
    // Exception count during reads
    pub exc_during_phy: u32,
    // DWC3 PHY interface (check PHYSOFTRST bit 31)
    pub gusb2phycfg: u32,
    pub gusb3pipectl: u32,
    // DALEPENA — endpoint enable mask (must have bits 0+1 for EP0)
    pub dalepena: u32,
    // Event buffer raw content (first 4 words)
    pub evt_buf_raw: [u32; 4],
}

/// Return the physical addresses of DMA buffers so we can check what address the kernel computed vs what ended up in the DWC3 register.
pub fn dma_buffer_addrs() -> (u64, u64, u64, u64) {
    let evt = &raw const EVT_BUF as usize as u64;
    let trbs = &raw const EP0_TRBS as usize as u64;
    let setup = &raw const EP0_SETUP_BUF as usize as u64;
    let data = &raw const EP0_DATA_BUF as usize as u64;
    (evt, trbs, setup, data)
}

pub fn dump_diag(exc_fn: fn() -> u64) -> Dwc3Diag {
    let exc_before = exc_fn();
    let (qusb2_pwr, qmp_pwr, usb2_utmi);
    unsafe {
        qusb2_pwr = mmio::read32(QUSB2_PHY_BASE + 0x18); // PWR_CTRL1
        qmp_pwr = mmio::read32(QMP_USB3_PHY_BASE + 0x18); // PCS_POWER_DOWN_CONTROL
        usb2_utmi = mmio::read32(QUSB2_PHY_BASE + 0x3C); // DEBUG_CTRL
    }
    let exc_after = exc_fn();

    // Read event buffer raw content
    let evt_raw = unsafe {
        let base = &raw const EVT_BUF as *const u32;
        [
            core::ptr::read_volatile(base),
            core::ptr::read_volatile(base.add(1)),
            core::ptr::read_volatile(base.add(2)),
            core::ptr::read_volatile(base.add(3)),
        ]
    };

    unsafe {
        Dwc3Diag {
            dctl: mmio::read32(dwc3_base() + DCTL),
            dsts: mmio::read32(dwc3_base() + DSTS),
            dcfg: mmio::read32(dwc3_base() + DCFG),
            devten: mmio::read32(dwc3_base() + DEVTEN),
            gctl: mmio::read32(dwc3_base() + GCTL),
            gsts: mmio::read32(dwc3_base() + GSTS),
            gevntadrlo: mmio::read32(dwc3_base() + GEVNTADRLO),
            gevntadrhi: mmio::read32(dwc3_base() + GEVNTADRHI),
            gevntsiz: mmio::read32(dwc3_base() + GEVNTSIZ),
            gevntcount: mmio::read32(dwc3_base() + GEVNTCOUNT),
            qcom_general_cfg: mmio::read32(QCOM_WRAPPER + QCOM_GENERAL_CFG),
            qcom_hs_phy_ctrl: mmio::read32(QCOM_WRAPPER + QCOM_HS_PHY_CTRL),
            qcom_ss_phy_ctrl: mmio::read32(QCOM_WRAPPER + QCOM_SS_PHY_CTRL),
            qusb2_pwr_ctrl: qusb2_pwr,
            qmp_pwr_ctrl: qmp_pwr,
            usb2_phy_utmi_ctrl: usb2_utmi,
            exc_during_phy: (exc_after - exc_before) as u32,
            gusb2phycfg: mmio::read32(dwc3_base() + GUSB2PHYCFG),
            gusb3pipectl: mmio::read32(dwc3_base() + GUSB3PIPECTL),
            dalepena: mmio::read32(dwc3_base() + DALEPENA),
            evt_buf_raw: evt_raw,
        }
    }
}


// ---------------------------------------------------------------------------
// PHY + QSCRATCH initialization
// ---------------------------------------------------------------------------

/// Small delay loop (~us scale at ~1GHz).
fn phy_delay(iters: u32) {
    for _ in 0..iters {
        unsafe { core::arch::asm!("nop") };
    }
}

/// Read-modify-write helper for PHY registers.
unsafe fn phy_rmw(addr: usize, clear: u32, set: u32) {
    let val = unsafe { mmio::read32(addr) };
    unsafe { mmio::write32(addr, (val & !clear) | set); }
}

// Cache ops moved to crate::mmio::cache_clean / cache_invalidate

/// Initialize the SNPS Femto v2 USB2 HS PHY.
///
/// This is the PHY used on SC7280/QCM6490. ABL tears it down during boot handoff, so we must re-initialize it for USB to work.
///
/// Returns true if PHY init completed (no way to verify PLL lock on this PHY — it relies on timing delays). Bypass SMMU for all non-secure DMA. The apps_smmu at 0x15000000 is active and terminates unmatched streams. Setting nsCR0.CLIENTPD (bit 0) bypasses translation for all NS masters, allowing DWC3 DMA to use physical addresses directly.
pub fn smmu_bypass() {
    const APPS_SMMU: usize = 0x1500_0000;
    const NS_CR0: usize = APPS_SMMU + 0x400;
    unsafe {
        let val = mmio::read32(NS_CR0);
        mmio::write32(NS_CR0, val | 1); // Set CLIENTPD
    }
}

pub fn phy_init() -> bool {
    unsafe {
        // Phase 0: Assert PHYSOFTRST to isolate DWC3 from PHY during reset. The DWC3↔PHY interface must be disconnected while we reset and re-init the PHY, otherwise the DWC3 loses sync with the PHY and falls back to FS (PHY TX corrupted → host sees no valid packets).
        let phycfg = mmio::read32(dwc3_base() + GUSB2PHYCFG);
        mmio::write32(dwc3_base() + GUSB2PHYCFG, phycfg | (1 << 31));  // Assert PHYSOFTRST
        let pipectl = mmio::read32(dwc3_base() + GUSB3PIPECTL);
        mmio::write32(dwc3_base() + GUSB3PIPECTL, pipectl | (1 << 31)); // Assert USB3 PHYSOFTRST
        phy_delay(10_000);

        // Phase 1: GCC reset cycle for the USB2 PHY
        mmio::write32(GCC_QUSB2PHY_PRIM_BCR, 1);   // Assert reset
        phy_delay(50_000); // ~200us
        mmio::write32(GCC_QUSB2PHY_PRIM_BCR, 0);   // De-assert reset
        phy_delay(50_000);

        // Phase 2: SNPS Femto v2 PHY register init sequence (follows phy-qcom-snps-femto-v2.c init)
        let p = QUSB2_PHY_BASE;

        // Enable common control override
        phy_rmw(p + PHY_CFG0, 0, PHY_CFG0_CMN_CTRL_OVERRIDE);

        // Assert PHY power-on-reset
        phy_rmw(p + PHY_UTMI_CTRL5, 0, PHY_UTMI_CTRL5_POR);

        // Clear FSEL (frequency select)
        phy_rmw(p + PHY_HS_CTRL_COMMON0, PHY_HS_COMMON0_FSEL_MASK, 0);

        // Set PLLBTUNE
        phy_rmw(p + PHY_HS_CTRL_COMMON1, 0, PHY_HS_COMMON1_PLLBTUNE);

        // Set reference clock select = 2
        phy_rmw(p + PHY_REFCLK_CTRL, 0x3, 0x2);

        // Set VBUSVLDEXTSEL0
        phy_rmw(p + PHY_HS_CTRL_COMMON1, 0, PHY_HS_COMMON1_VBUSVLDEXTSEL);

        // Set VBUSVLDEXT0
        phy_rmw(p + PHY_HS_CTRL1, 0, PHY_HS_CTRL1_VBUSVLDEXT);

        // Set VREGBYPASS
        phy_rmw(p + PHY_HS_CTRL_COMMON2, 0, PHY_HS_COMMON2_VREGBYPASS);

        // Force unsuspend: set SUSPEND_N and SUSPEND_N_SEL
        phy_rmw(p + PHY_HS_CTRL2, 0,
            PHY_HS_CTRL2_SUSPEND_N | PHY_HS_CTRL2_SUSPEND_N_SEL);

        // Set SLEEPM (exit sleep)
        phy_rmw(p + PHY_UTMI_CTRL0, 0, PHY_UTMI_CTRL0_SLEEPM);

        // Clear SIDDQ (enable signal detection)
        phy_rmw(p + PHY_HS_CTRL_COMMON0, PHY_HS_COMMON0_SIDDQ, 0);

        // De-assert POR
        phy_rmw(p + PHY_UTMI_CTRL5, PHY_UTMI_CTRL5_POR, 0);

        // Wait for PLL lock (~200us)
        phy_delay(100_000);

        // Clear suspend override (let hardware manage suspend)
        phy_rmw(p + PHY_HS_CTRL2, PHY_HS_CTRL2_SUSPEND_N_SEL, 0);

        // Clear common control override
        phy_rmw(p + PHY_CFG0, PHY_CFG0_CMN_CTRL_OVERRIDE, 0);

        // Phase 2b: QSCRATCH wrapper — UTMI clock mux + VBUS override. Must happen BEFORE clearing PHYSOFTRST so the DWC3 reconnects to the PHY with the correct clock source already selected.
        let q = QCOM_WRAPPER;

        // Select UTMI clock (no SS PHY pipe clock)
        phy_rmw(q + QCOM_GENERAL_CFG, 0, 1 << 8); // PIPE_UTMI_CLK_DIS
        phy_delay(25_000); // ~100us
        phy_rmw(q + QCOM_GENERAL_CFG, 0, (1 << 0) | (1 << 3)); // CLK_SEL | PHYSTATUS_SW
        phy_delay(25_000);
        phy_rmw(q + QCOM_GENERAL_CFG, 1 << 8, 0); // Re-enable clock

        // Override VBUS valid (device mode, no VBUS detection HW)
        phy_rmw(q + QCOM_HS_PHY_CTRL, 0,
            QSCRATCH_HS_UTMI_OTG_VBUS_VALID | QSCRATCH_HS_SW_SESSVLD_SEL);

        // SS PHY lane power present (needed even if SS not used)
        phy_rmw(q + QCOM_SS_PHY_CTRL, 0, QSCRATCH_SS_LANE0_PWR_PRESENT);

        // Phase 3: Deassert PHYSOFTRST — reconnect DWC3 to the freshly initialized PHY. Clock mux and VBUS are already configured.
        let phycfg = mmio::read32(dwc3_base() + GUSB2PHYCFG);
        mmio::write32(dwc3_base() + GUSB2PHYCFG, phycfg & !(1 << 31));
        let pipectl = mmio::read32(dwc3_base() + GUSB3PIPECTL);
        mmio::write32(dwc3_base() + GUSB3PIPECTL,
            (pipectl & !(1 << 31)) | (1 << 28)); // Clear PHYSOFTRST + DISRXDETINP3
        phy_delay(100_000); // Wait for DWC3↔PHY handshake
    }

    true
}

// ---------------------------------------------------------------------------
// DWC3 Device Mode Controller
// ---------------------------------------------------------------------------

/// DWC3 device-mode controller state.
pub struct Dwc3Dev {
    evt_read_idx: usize,    // Current read position in event buffer
    ep0_state: Ep0State,    // EP0 control transfer state machine
    address: u8,            // Assigned USB address (0 = default)
    configured: bool,       // SET_CONFIGURATION received
    connected_speed: u32,   // From DSTS after ConnectDone
    pub halt_ok: bool,      // Whether DEVCTRLHLT was observed during init
    pub dsts_at_halt: u32,  // DSTS value when halt check finished
    pub last_ep_cmd_ok: bool,  // Last ep_cmd result
    pub ep_cmd_fail_count: u32, // Total failed ep_cmds
    pub ep1_xfer_complete: u32, // XferComplete events on EP0 IN (ep_phys=1)
    pub ep0_xfer_notready: u32, // XferNotReady events total
    pub status_out_count: u32,  // Times ep0_status_out was called
    pub status_in_count: u32,   // Times ep0_status_in was called
    pub ep1_data_notready: u32, // XferNotReady on EP1 while DataIn (STARTTRANSFER retry)
    pub cmd_status_fail: u32,   // DEPCMD completed with non-zero CMDSTATUS
    pub last_cmd_status: u32,   // Full DEPCMD register value of last CMDSTATUS failure
    pub last_cmd_ep: u8,        // EP of last CMDSTATUS failure
    pub last_cmd_type: u32,     // Command type of last CMDSTATUS failure
    ep0_resource_idx: u8,       // Transfer resource index for EP0 (0 = no active xfer)
    ep1_resource_idx: u8,       // Transfer resource index for EP1 (0 = no active xfer)
    pub ep1_start_ok: u32,      // STARTTRANSFER successes on EP1
    pub ep1_start_fail: u32,    // STARTTRANSFER failures on EP1 (first attempt)
    pub ep1_retry_ok: u32,      // STARTTRANSFER retry successes on EP1
    pub ep1_retry_fail: u32,    // STARTTRANSFER retry failures on EP1
    pending_ep1_len: u16,       // Pending data length for XferNotReady retry
    pending_ep1_trbctl: u32,    // Pending TRB type for XferNotReady retry
    pub setup_count: u32,       // Total SETUP packets received
    pub ep0_setup_arm_ok: u32,  // ep0_start_setup STARTTRANSFER successes
    pub ep0_setup_arm_fail: u32,// ep0_start_setup STARTTRANSFER failures (both attempts)
    pub last_setup_brequest: u8,// bRequest of last SETUP received
    pub last_setup_wvalue: u16, // wValue of last SETUP received
    pub ep0_status_out_arm_fail: u32, // ep0_status_out STARTTRANSFER failures (both)
    pub last_evt_raw: u32,      // Last raw event word (for debug)
    // Diagnostics: readback of data buffer after copy, before DMA
    pub last_send_preview: u32, // First 4 bytes of EP0_DATA_BUF after volatile copy
    pub last_send_len: u16,     // Length passed to ep0_send
    pub last_send_buf_addr: u32, // Low 32 bits of EP0_DATA_BUF physical address
    pub last_send_trb_addr: u32, // Low 32 bits of data TRB physical address
    pub last_send_src_addr: u32, // Low 32 bits of source data slice address
    pub last_send_src_preview: u32, // First 4 bytes of source data (volatile read)
    // Bulk endpoint state
    bulk_out_resource_idx: u8,  // Transfer resource index for bulk OUT (phys EP 2)
    bulk_in_resource_idx: u8,   // Transfer resource index for bulk IN (phys EP 3)
    /// Bytes received in last bulk OUT transfer
    pub bulk_out_len: u16,
    /// New bulk OUT data available for kernel to consume
    pub bulk_out_ready: bool,
    pub bulk_out_armed: bool,
    /// Bulk IN transfer idle (buffer available for next send)
    pub bulk_in_idle: bool,
    pub bulk_out_xfer_complete: u32,
    pub bulk_in_xfer_complete: u32,
    /// Rolling ring of the last 12 raw EP event words, oldest overwritten first. Served by VENDOR_REQ_DBG2 to diagnose event-path bugs without a console.
    pub evt_ring: [u32; 12],
    /// Total EP events captured into evt_ring.
    pub evt_ring_n: u32,
    /// Bulk OUT TRB size word snapshot taken right after cache invalidate at ep2 XferComplete (remaining count in low 24 bits).
    pub ep2_trb_size_snap: u32,
    /// Bulk OUT TRB ctrl word snapshot at ep2 XferComplete (HWO bit 0 tells whether hardware ever consumed the TRB).
    pub ep2_trb_ctrl_snap: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Ep0State {
    Setup,      // Waiting for SETUP packet
    DataIn,     // Sending data to host
    DataOut,    // Receiving data from host
    Status,     // Status stage
}

/// Event types returned from poll.
#[derive(Clone, Copy)]
pub enum UsbEvent {
    None,
    Reset,
    ConnectDone { speed: u32 },
    Disconnect,
    Ep0Setup { request: [u8; 8] },
    TransferComplete { ep: u8 },
    TransferNotReady { ep: u8 },
}

impl Dwc3Dev {
    /// Initialize the DWC3 in device mode.
    ///
    /// Takes over from ABL without soft reset (preserves PHY state). Stops the controller, reconfigures event buffer and EP0, then restarts. Returns None if initialization fails. Warm takeover: skip CSFTRST, halt controller, reprogram buffers + EPs, restart. For use when ABL already initialized the DWC3 (Tensor G3, M1 via m1n1).
    pub fn warm_init() -> Option<Self> {
        // Check DWC3 is alive
        let snpsid = unsafe { mmio::read32(dwc3_base() + GSNPSID) };
        if snpsid == 0 || snpsid == 0xFFFF_FFFF {
            return None; // Clock gated, no DWC3
        }

        // Set device mode
        unsafe {
            let gctl = mmio::read32(dwc3_base() + GCTL);
            mmio::write32(dwc3_base() + GCTL,
                (gctl & !GCTL_PRTCAPDIR_MASK) | GCTL_PRTCAPDIR_DEVICE);
        }

        // Halt controller (clear RUN_STOP, wait for DEVCTRLHLT)
        unsafe {
            let dctl = mmio::read32(dwc3_base() + DCTL);
            mmio::write32(dwc3_base() + DCTL, dctl & !DCTL_RUN_STOP);
            for _ in 0..100_000u32 {
                if mmio::read32(dwc3_base() + DSTS) & (1 << 22) != 0 { break; } // DEVCTRLHLT
            }
        }

        // Mask events
        unsafe {
            let evt_size = core::mem::size_of::<EventBuffer>() as u32;
            mmio::write32(dwc3_base() + GEVNTSIZ, evt_size | GEVNTSIZ_INTMASK);
        }

        // Drain pending events
        unsafe {
            let pending = mmio::read32(dwc3_base() + GEVNTCOUNT);
            if pending > 0 {
                mmio::write32(dwc3_base() + GEVNTCOUNT, pending);
            }
        }

        // Set up our event buffer
        unsafe {
            let evt_addr = &raw const EVT_BUF as usize;
            let evt_size = core::mem::size_of::<EventBuffer>() as u32;
            for i in 0..256 { EVT_BUF.buf[i] = 0; }
            mmio::write32(dwc3_base() + GEVNTADRLO, evt_addr as u32);
            mmio::write32(dwc3_base() + GEVNTADRHI, (evt_addr >> 32) as u32);
            mmio::write32(dwc3_base() + GEVNTSIZ, evt_size & !GEVNTSIZ_INTMASK);
            mmio::write32(dwc3_base() + GEVNTCOUNT, 0);
        }

        // Enable device events
        unsafe {
            mmio::write32(dwc3_base() + DEVTEN,
                DEVTEN_USBRSTEN | DEVTEN_CONNECTDONEEN |
                DEVTEN_DISCONNEVTEN | DEVTEN_CMDCMPLEN);
        }

        // Set speed to High-Speed
        unsafe {
            let dcfg = mmio::read32(dwc3_base() + DCFG);
            mmio::write32(dwc3_base() + DCFG, (dcfg & !DCFG_SPEED_MASK) | DCFG_SPEED_HS);
        }

        let dsts_now = unsafe { mmio::read32(dwc3_base() + DSTS) };
        let mut dev = Dwc3Dev {
            evt_read_idx: 0,
            ep0_state: Ep0State::Setup,
            address: 0,
            configured: false,
            connected_speed: 0,
            halt_ok: true,
            dsts_at_halt: dsts_now,
            last_ep_cmd_ok: true,
            ep_cmd_fail_count: 0,
            ep1_xfer_complete: 0,
            ep0_xfer_notready: 0,
            status_out_count: 0,
            status_in_count: 0,
            ep1_data_notready: 0,
            cmd_status_fail: 0,
            last_cmd_status: 0,
            last_cmd_ep: 0,
            last_cmd_type: 0,
            ep0_resource_idx: 0,
            ep1_resource_idx: 0,
            ep1_start_ok: 0,
            ep1_start_fail: 0,
            ep1_retry_ok: 0,
            ep1_retry_fail: 0,
            pending_ep1_len: 0,
            pending_ep1_trbctl: 0,
            setup_count: 0,
            ep0_setup_arm_ok: 0,
            ep0_setup_arm_fail: 0,
            last_setup_brequest: 0,
            last_setup_wvalue: 0,
            ep0_status_out_arm_fail: 0,
            last_evt_raw: 0,
            last_send_preview: 0,
            last_send_len: 0,
            last_send_buf_addr: 0,
            last_send_trb_addr: 0,
            last_send_src_addr: 0,
            last_send_src_preview: 0,
            bulk_out_resource_idx: 0,
            bulk_in_resource_idx: 0,
            bulk_out_len: 0,
            bulk_out_ready: false,
            bulk_out_armed: false,
            bulk_in_idle: true,
            bulk_out_xfer_complete: 0,
            bulk_in_xfer_complete: 0,
            evt_ring: [0; 12],
            evt_ring_n: 0,
            ep2_trb_size_snap: 0,
            ep2_trb_ctrl_snap: 0,
        };

        // Configure endpoints
        dev.ep_start_config(0);
        dev.ep0_configure();
        dev.bulk_configure();

        // Start controller
        unsafe {
            let dctl = mmio::read32(dwc3_base() + DCTL);
            mmio::write32(dwc3_base() + DCTL, dctl | DCTL_RUN_STOP);
        }

        Some(dev)
    }

    /// Full cold init with CSFTRST. Use on QCM6490 where ABL leaves caches hot.
    pub fn init() -> Option<Self> {
        // Ensure device mode
        unsafe {
            let gctl = mmio::read32(dwc3_base() + GCTL);
            mmio::write32(dwc3_base() + GCTL,
                (gctl & !GCTL_PRTCAPDIR_MASK) | GCTL_PRTCAPDIR_DEVICE);
        }

        // Device controller soft reset (CSFTRST) — resets the device-mode state machine and FIFOs without affecting global/SMMU state. This ensures clean state after PHY re-init.
        let halt_ok;
        unsafe {
            let dctl = mmio::read32(dwc3_base() + DCTL);
            mmio::write32(dwc3_base() + DCTL, (dctl & !DCTL_RUN_STOP) | DCTL_CSFTRST);
            // Wait for CSFTRST to self-clear
            halt_ok = {
                let mut ok = false;
                for _ in 0..100_000u32 {
                    if mmio::read32(dwc3_base() + DCTL) & DCTL_CSFTRST == 0 {
                        ok = true;
                        break;
                    }
                }
                ok
            };
        }
        phy_delay(10_000);

        // Mask events before reprogramming the event buffer
        unsafe {
            let evt_size = core::mem::size_of::<EventBuffer>() as u32;
            mmio::write32(dwc3_base() + GEVNTSIZ, evt_size | GEVNTSIZ_INTMASK);
        }

        // Acknowledge any pending events from ABL
        unsafe {
            let pending = mmio::read32(dwc3_base() + GEVNTCOUNT);
            if pending > 0 {
                mmio::write32(dwc3_base() + GEVNTCOUNT, pending);
            }
        }

        // Set up event buffer with our DRAM address
        unsafe {
            let evt_addr = &raw const EVT_BUF as usize;
            let evt_size = core::mem::size_of::<EventBuffer>() as u32;

            // Clear the event buffer
            for i in 0..256 {
                EVT_BUF.buf[i] = 0;
            }

            mmio::write32(dwc3_base() + GEVNTADRLO, evt_addr as u32);
            mmio::write32(dwc3_base() + GEVNTADRHI, (evt_addr >> 32) as u32);
            // Unmask events now that address is set
            mmio::write32(dwc3_base() + GEVNTSIZ, evt_size & !GEVNTSIZ_INTMASK);
            mmio::write32(dwc3_base() + GEVNTCOUNT, 0);
        }

        // Enable device events
        unsafe {
            mmio::write32(dwc3_base() + DEVTEN,
                DEVTEN_USBRSTEN | DEVTEN_CONNECTDONEEN |
                DEVTEN_DISCONNEVTEN | DEVTEN_CMDCMPLEN);
        }

        // Set device speed to High-Speed (USB2) — SS PHY may be torn down by ABL
        unsafe {
            let dcfg = mmio::read32(dwc3_base() + DCFG);
            mmio::write32(dwc3_base() + DCFG, (dcfg & !DCFG_SPEED_MASK) | DCFG_SPEED_HS);
        }

        let dsts_now = unsafe { mmio::read32(dwc3_base() + DSTS) };
        let mut dev = Dwc3Dev {
            evt_read_idx: 0,
            ep0_state: Ep0State::Setup,
            address: 0,
            configured: false,
            connected_speed: 0,
            halt_ok,
            dsts_at_halt: dsts_now,
            last_ep_cmd_ok: true,
            ep_cmd_fail_count: 0,
            ep1_xfer_complete: 0,
            ep0_xfer_notready: 0,
            status_out_count: 0,
            status_in_count: 0,
            ep1_data_notready: 0,
            cmd_status_fail: 0,
            last_cmd_status: 0,
            last_cmd_ep: 0,
            last_cmd_type: 0,
            ep0_resource_idx: 0,
            ep1_resource_idx: 0,
            ep1_start_ok: 0,
            ep1_start_fail: 0,
            ep1_retry_ok: 0,
            ep1_retry_fail: 0,
            pending_ep1_len: 0,
            pending_ep1_trbctl: 0,
            setup_count: 0,
            ep0_setup_arm_ok: 0,
            ep0_setup_arm_fail: 0,
            last_setup_brequest: 0,
            last_setup_wvalue: 0,
            ep0_status_out_arm_fail: 0,
            last_evt_raw: 0,
            last_send_preview: 0,
            last_send_len: 0,
            last_send_buf_addr: 0,
            last_send_trb_addr: 0,
            last_send_src_addr: 0,
            last_send_src_preview: 0,
            bulk_out_resource_idx: 0,
            bulk_in_resource_idx: 0,
            bulk_out_len: 0,
            bulk_out_ready: false,
            bulk_out_armed: false,
            bulk_in_idle: true,
            bulk_out_xfer_complete: 0,
            bulk_in_xfer_complete: 0,
            evt_ring: [0; 12],
            evt_ring_n: 0,
            ep2_trb_size_snap: 0,
            ep2_trb_ctrl_snap: 0,
        };

        // Configure all endpoints
        dev.ep_start_config(0);
        dev.ep0_configure();
        dev.bulk_configure();

        // Set Run/Stop to connect
        unsafe {
            let dctl = mmio::read32(dwc3_base() + DCTL);
            mmio::write32(dwc3_base() + DCTL, dctl | DCTL_RUN_STOP);
        }

        Some(dev)
    }

    /// Issue DEPSTARTCFG command to start endpoint configuration from `ep_phys`.
    fn ep_start_config(&mut self, ep_phys: u8) {
        self.ep_cmd(ep_phys, DEPCMD_DEPSTARTCFG, 0, 0, 0);
    }

    /// Configure EP0 (both OUT=0 and IN=1).
    fn ep0_configure(&mut self) {
        // EP0 OUT (physical EP 0) — MPS=64 for HS/FS (no SS PHY)
        let par0 = (EP_TYPE_CONTROL << DEPCFGPAR0_EPTYPE_SHIFT)
            | (64 << DEPCFGPAR0_MPS_SHIFT);
        let par1 = (0 << DEPCFGPAR1_EPNUM_SHIFT) // USB EP number = 0
            | DEPCFGPAR1_XFER_CMPL_EN
            | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(0, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(0, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0); // 1 transfer resource

        // EP0 IN (physical EP 1)
        let par0 = (EP_TYPE_CONTROL << DEPCFGPAR0_EPTYPE_SHIFT)
            | (64 << DEPCFGPAR0_MPS_SHIFT)
            | (0 << DEPCFGPAR0_FIFONUM_SHIFT);
        let par1 = (1 << DEPCFGPAR1_EPNUM_SHIFT) // physical EP 1 = EP0 IN
            | DEPCFGPAR1_XFER_CMPL_EN
            | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(1, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(1, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // Enable EP0 OUT (bit 0) and EP0 IN (bit 1) in DALEPENA. Without this, the endpoint data path is disabled — DMA completes but the FIFO never drains to the PHY, so no packets reach the wire.
        unsafe {
            let ena = mmio::read32(dwc3_base() + DALEPENA);
            mmio::write32(dwc3_base() + DALEPENA, ena | 0x3); // bits 0+1 = EP0 OUT+IN
        }
    }

    /// Configure bulk endpoints: EP1 OUT (phys 2) and EP1 IN (phys 3). Must be called after ep0_configure() and again in handle_connect_done().
    fn bulk_configure(&mut self) {
        // DEPSTARTCFG with XferRscIdx=2 to preserve EP0 config. XferRscIdx goes in DEPCMD[22:16], issued on EP0.
        self.ep_cmd(0, DEPCMD_DEPSTARTCFG | (2 << 16), 0, 0, 0);

        // EP1 OUT (physical EP 2) — Bulk, 512B MPS
        let par0 = (EP_TYPE_BULK << DEPCFGPAR0_EPTYPE_SHIFT)
            | (512 << DEPCFGPAR0_MPS_SHIFT);
        let par1 = (2 << DEPCFGPAR1_EPNUM_SHIFT)
            | DEPCFGPAR1_XFER_CMPL_EN
            | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(2, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(2, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // EP1 IN (physical EP 3) — Bulk, 512B MPS, FIFO 1
        let par0 = (EP_TYPE_BULK << DEPCFGPAR0_EPTYPE_SHIFT)
            | (512 << DEPCFGPAR0_MPS_SHIFT)
            | (1 << DEPCFGPAR0_FIFONUM_SHIFT); // FIFO 1 (FIFO 0 = EP0 IN)
        let par1 = (3 << DEPCFGPAR1_EPNUM_SHIFT)
            | DEPCFGPAR1_XFER_CMPL_EN
            | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(3, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(3, DEPCMD_SETTRANSFRESOURCE, 2, 0, 0); // 2 resources — avoids leak when STARTTRANSFER from within TransferComplete

        // Enable all 4 EPs in DALEPENA (bits 0-3)
        unsafe {
            let ena = mmio::read32(dwc3_base() + DALEPENA);
            mmio::write32(dwc3_base() + DALEPENA, ena | 0xF);
        }
    }

    /// Arm bulk OUT (phys EP 2) to receive up to 512 bytes from host. Whether bulk OUT has been armed (pending STARTTRANSFER).
    pub fn bulk_out_needs_arm(&self) -> bool {
        !self.bulk_out_ready && !self.bulk_out_armed
    }

    /// Whether bulk IN (phys EP 3) is idle and ready to send the next chunk.
    pub fn bulk_in_is_idle(&self) -> bool {
        self.bulk_in_idle
    }

    pub fn bulk_out_arm(&mut self) {
        self.bulk_out_armed = true;

        // ENDTRANSFER only if a transfer is actually in flight (resource index nonzero — XferComplete zeroes it). The old unconditional ENDTRANSFER errored a DEPCMD on every normal re-arm and is the prime suspect for the spurious len=0 ep2 event that eats the first PT DATA packet.
        if self.bulk_out_resource_idx != 0 {
            self.force_end_transfer_unconditional(2);
        }

        let buf_addr = &raw const BULK_OUT_BUF as usize;
        let trb_addr = unsafe { &raw mut BULK_OUT_TRB.trb } as usize;

        unsafe {
            let trb = &raw mut BULK_OUT_TRB.trb;
            (*trb).bpl = buf_addr as u32;
            (*trb).bph = (buf_addr >> 32) as u32;
            (*trb).size = 512;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | TRB_CTRL_ISP_IMI
                | (TRBCTL_NORMAL << TRB_CTRL_TRBCTL_SHIFT);
            crate::mmio::cache_clean(trb_addr, 16);
            crate::mmio::cache_clean(buf_addr, 512);
            core::arch::asm!("dsb sy");
        }

        if self.ep_cmd(2, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
            let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 2 * 16 + 0x0C) };
            self.bulk_out_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
        } else {
            self.force_end_transfer_unconditional(2);
            unsafe {
                let trb = &raw mut BULK_OUT_TRB.trb;
                (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                    | TRB_CTRL_ISP_IMI
                    | (TRBCTL_NORMAL << TRB_CTRL_TRBCTL_SHIFT);
                crate::mmio::cache_clean(trb_addr, 16);
            }
            if self.ep_cmd(2, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
                let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 2 * 16 + 0x0C) };
                self.bulk_out_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
            }
        }
    }

    /// Queue data for bulk IN (phys EP 3, device→host). Copies data to DMA buffer and starts transfer. Returns false if a transfer is already in progress.
    pub fn bulk_in_send(&mut self, data: &[u8]) -> bool {
        if !self.bulk_in_idle { return false; }
        let len = data.len().min(4096);
        if len == 0 { return false; }

        let buf_addr = &raw const BULK_IN_BUF as usize;
        let trb_addr = unsafe { &raw mut BULK_IN_TRB.trb } as usize;

        unsafe {
            let dst = &raw mut BULK_IN_BUF.data as *mut u8;
            for i in 0..len {
                core::ptr::write_volatile(dst.add(i), data[i]);
            }
            core::arch::asm!("dsb sy");

            let trb = &raw mut BULK_IN_TRB.trb;
            (*trb).bpl = buf_addr as u32;
            (*trb).bph = (buf_addr >> 32) as u32;
            (*trb).size = len as u32;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | TRB_CTRL_ISP_IMI  // complete on short packet (< 512 bytes)
                | (TRBCTL_NORMAL << TRB_CTRL_TRBCTL_SHIFT);
            crate::mmio::cache_clean(buf_addr, len);
            crate::mmio::cache_clean(trb_addr, 16);
            core::arch::asm!("dsb sy"); // ensure DMA-visible before STARTTRANSFER
        }

        self.bulk_in_idle = false;

        if self.ep_cmd(3, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
            let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 3 * 16 + 0x0C) };
            self.bulk_in_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
        } else {
            self.force_end_transfer_unconditional(3);
            unsafe {
                let trb = &raw mut BULK_IN_TRB.trb;
                (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                    | (TRBCTL_NORMAL << TRB_CTRL_TRBCTL_SHIFT);
                crate::mmio::cache_clean(trb_addr, 16);
            }
            if !self.ep_cmd(3, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
                self.bulk_in_idle = true;
                return false;
            }
            let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 3 * 16 + 0x0C) };
            self.bulk_in_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
        }
        true
    }

    /// Read received bulk OUT data. Returns slice of received bytes, or None if no data available. Caller must call bulk_out_arm() after.
    pub fn bulk_out_read(&mut self) -> Option<&[u8]> {
        if !self.bulk_out_ready { return None; }
        self.bulk_out_ready = false;
        let len = self.bulk_out_len as usize;
        if len == 0 { return None; }
        unsafe {
            crate::mmio::cache_invalidate(&raw const BULK_OUT_BUF as usize, len);
            Some(core::slice::from_raw_parts(
                &raw const BULK_OUT_BUF as *const u8, len))
        }
    }

    /// Issue a DEPCMD to a physical endpoint. Blocks until command completes.
    fn ep_cmd(&mut self, ep_phys: u8, cmd: u32, par0: u32, par1: u32, par2: u32) -> bool {
        let base = dwc3_base() + 0xC800 + (ep_phys as usize) * 16;
        unsafe {
            mmio::write32(base + 0x00, par2);  // DEPCMDPAR2
            mmio::write32(base + 0x04, par1);  // DEPCMDPAR1
            mmio::write32(base + 0x08, par0);  // DEPCMDPAR0
            mmio::write32(base + 0x0C, cmd | DEPCMD_CMDACT); // DEPCMD

            // Poll for completion (CMDACT clears)
            for _ in 0..100_000u32 {
                let val = mmio::read32(base + 0x0C);
                if val & DEPCMD_CMDACT == 0 {
                    let status = (val >> 12) & 0xF;
                    if status != 0 {
                        self.last_ep_cmd_ok = false;
                        self.cmd_status_fail += 1;
                        self.last_cmd_status = val;
                        self.last_cmd_ep = ep_phys;
                        self.last_cmd_type = cmd & 0xF;
                        return false;
                    }
                    self.last_ep_cmd_ok = true;
                    return true;
                }
            }
        }
        self.last_ep_cmd_ok = false;
        self.ep_cmd_fail_count += 1;
        false
    }

    /// Prepare EP0 to receive a SETUP packet. If STARTTRANSFER fails, force ENDTRANSFER and retry.
    pub fn ep0_start_setup(&mut self) {
        let setup_addr = &raw const EP0_SETUP_BUF as usize;

        // Clear setup buffer
        unsafe {
            for i in 0..8 {
                EP0_SETUP_BUF.data[i] = 0;
            }
        }

        // Build setup TRB
        let trb_addr = unsafe { &raw mut EP0_TRBS.setup } as usize;
        unsafe {
            let trb = &raw mut EP0_TRBS.setup;
            (*trb).bpl = setup_addr as u32;
            (*trb).bph = (setup_addr >> 32) as u32;
            (*trb).size = 8; // SETUP is always 8 bytes
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | (TRBCTL_SETUP << TRB_CTRL_TRBCTL_SHIFT);
        }

        // Clean cache lines so DWC3 DMA sees the TRB and setup buffer
        unsafe {
            crate::mmio::cache_clean(setup_addr, 8);
            crate::mmio::cache_clean(trb_addr, 16);
        }

        // Start transfer on EP0 OUT
        if self.ep_cmd(0, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
            self.ep0_setup_arm_ok += 1;
        } else {
            self.force_end_transfer_unconditional(0);
            unsafe {
                let trb = &raw mut EP0_TRBS.setup;
                (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                    | (TRBCTL_SETUP << TRB_CTRL_TRBCTL_SHIFT);
                crate::mmio::cache_clean(trb_addr, 16);
            }
            if !self.ep_cmd(0, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
                self.ep0_setup_arm_fail += 1;
            } else {
                self.ep0_setup_arm_ok += 1;
            }
        }
        let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 0x0C) };
        self.ep0_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
        self.ep0_state = Ep0State::Setup;
    }

    /// Send data on EP0 IN (physical EP 1) with proactive STARTTRANSFER. If the transfer resource is occupied, force ENDTRANSFER and retry. If both attempts fail, data stays in EP0_DATA_BUF for XferNotReady retry.
    fn ep0_send(&mut self, data: &[u8], trbctl: u32) {
        let len = data.len().min(512);

        // Capture source address and data BEFORE copy (diagnose 0x40 bug)
        let src_ptr = data.as_ptr();
        self.last_send_src_addr = src_ptr as u32;
        if len >= 4 {
            unsafe {
                // Volatile read of source to bypass any caching
                let mut src_preview = 0u32;
                for i in 0..4 {
                    let b = core::ptr::read_volatile(src_ptr.add(i));
                    src_preview |= (b as u32) << (i * 8);
                }
                self.last_send_src_preview = src_preview;
            }
        } else if len > 0 {
            let mut src_preview = 0u32;
            for i in 0..len {
                unsafe {
                    let b = core::ptr::read_volatile(src_ptr.add(i));
                    src_preview |= (b as u32) << (i * 8);
                }
            }
            self.last_send_src_preview = src_preview;
        } else {
            self.last_send_src_preview = 0;
        }

        // Copy data to DMA buffer using volatile writes to prevent compiler from optimizing away or reordering stores.
        unsafe {
            let dst = &raw mut EP0_DATA_BUF.data as *mut u8;
            for i in 0..len {
                core::ptr::write_volatile(dst.add(i), data[i]);
            }
            // Explicit barrier: ensure all volatile stores are visible to DMA masters before we set up the TRB.
            core::arch::asm!("dsb sy");
        }

        // Capture diagnostic readback (volatile read of what we just wrote)
        let buf_addr = &raw const EP0_DATA_BUF as usize;
        self.last_send_buf_addr = buf_addr as u32;
        self.last_send_len = len as u16;
        if len >= 4 {
            unsafe {
                let p = buf_addr as *const u32;
                self.last_send_preview = core::ptr::read_volatile(p);
            }
        } else if len > 0 {
            let mut preview = 0u32;
            for i in 0..len {
                unsafe {
                    let b = core::ptr::read_volatile((buf_addr as *const u8).add(i));
                    preview |= (b as u32) << (i * 8);
                }
            }
            self.last_send_preview = preview;
        } else {
            self.last_send_preview = 0;
        }

        // Store pending info for XferNotReady fallback
        self.pending_ep1_len = len as u16;
        self.pending_ep1_trbctl = trbctl;

        self.ep1_start_transfer(len, trbctl);
    }

    /// Actually issue STARTTRANSFER on EP1 with data already in EP0_DATA_BUF. Used by both ep0_send (proactive) and XferNotReady handler (reactive).
    fn ep1_start_transfer(&mut self, len: usize, trbctl: u32) {
        let buf_addr = &raw const EP0_DATA_BUF as usize;
        let trb_addr = unsafe { &raw mut EP0_TRBS.data } as usize;
        self.last_send_trb_addr = trb_addr as u32;

        unsafe {
            let trb = &raw mut EP0_TRBS.data;
            (*trb).bpl = buf_addr as u32;
            (*trb).bph = (buf_addr >> 32) as u32;
            (*trb).size = len as u32;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | (trbctl << TRB_CTRL_TRBCTL_SHIFT);
        }

        // Clean data buffer + TRB so DWC3 DMA sees them
        unsafe {
            if len > 0 {
                crate::mmio::cache_clean(buf_addr, len);
            }
            crate::mmio::cache_clean(trb_addr, 16);
        }

        // Issue STARTTRANSFER on EP1. If it fails (resource occupied), force ENDTRANSFER and retry once.
        if self.ep_cmd(1, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
            self.ep1_start_ok += 1;
        } else {
            self.ep1_start_fail += 1;
            self.force_end_transfer_unconditional(1);
            // Re-set HWO since ENDTRANSFER may have cleared it
            unsafe {
                let trb = &raw mut EP0_TRBS.data;
                (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                    | (trbctl << TRB_CTRL_TRBCTL_SHIFT);
                crate::mmio::cache_clean(trb_addr, 16);
            }
            if self.ep_cmd(1, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
                self.ep1_retry_ok += 1;
            } else {
                self.ep1_retry_fail += 1;
                // Both failed — XferNotReady handler will retry later
                return;
            }
        }
        // Save resource index from successful STARTTRANSFER
        let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 16 + 0x0C) };
        self.ep1_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
        // Clear pending — transfer is armed, don't retry from XferNotReady
        self.pending_ep1_len = 0;
        self.pending_ep1_trbctl = 0;
    }

    /// Send zero-length status IN (for SET_ADDRESS, SET_CONFIGURATION).
    fn ep0_status_in(&mut self) {
        self.status_in_count += 1;
        self.ep0_send(&[], TRBCTL_STATUS2);
        self.ep0_state = Ep0State::Status;
    }

    /// Receive zero-length status OUT after sending data. If STARTTRANSFER fails, force ENDTRANSFER and retry (like ep0_send).
    fn ep0_status_out(&mut self) {
        self.status_out_count += 1;
        let buf_addr = &raw const EP0_DATA_BUF as usize;
        let trb_addr = unsafe { &raw mut EP0_TRBS.status } as usize;

        unsafe {
            let trb = &raw mut EP0_TRBS.status;
            (*trb).bpl = buf_addr as u32;
            (*trb).bph = (buf_addr >> 32) as u32;
            (*trb).size = 0;
            (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                | (TRBCTL_STATUS3 << TRB_CTRL_TRBCTL_SHIFT);
        }

        // Clean TRB so DWC3 DMA sees it
        unsafe { crate::mmio::cache_clean(trb_addr, 16); }

        // Start transfer on EP0 OUT (physical EP 0)
        if !self.ep_cmd(0, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
            // Retry: force ENDTRANSFER then re-arm
            self.force_end_transfer_unconditional(0);
            unsafe {
                let trb = &raw mut EP0_TRBS.status;
                (*trb).ctrl = TRB_CTRL_HWO | TRB_CTRL_LST | TRB_CTRL_IOC
                    | (TRBCTL_STATUS3 << TRB_CTRL_TRBCTL_SHIFT);
                crate::mmio::cache_clean(trb_addr, 16);
            }
            if !self.ep_cmd(0, DEPCMD_STARTTRANSFER, 0, trb_addr as u32, (trb_addr >> 32) as u32) {
                self.ep0_status_out_arm_fail += 1;
            }
        }
        let cmd_reg = unsafe { mmio::read32(dwc3_base() + 0xC800 + 0x0C) };
        self.ep0_resource_idx = ((cmd_reg >> 16) & 0x7F) as u8;
        self.ep0_state = Ep0State::Status;
    }

    /// STALL EP0 (both directions) for unsupported requests.
    pub fn ep0_stall(&mut self) {
        self.ep_cmd(0, DEPCMD_SETSTALL, 0, 0, 0);
        self.ep_cmd(1, DEPCMD_SETSTALL, 0, 0, 0);
        self.ep0_state = Ep0State::Setup;
        self.ep0_start_setup();
    }

    /// Poll for events. Returns the next event, or None.
    pub fn poll_event(&mut self) -> UsbEvent {
        let count = unsafe { mmio::read32(dwc3_base() + GEVNTCOUNT) } & 0xFFFC;
        if count == 0 {
            return UsbEvent::None;
        }

        // Invalidate event buffer cache line so we see DWC3's DMA writes
        let evt_base = &raw const EVT_BUF as usize;
        unsafe { crate::mmio::cache_invalidate(evt_base + self.evt_read_idx, 4); }

        // Read one event (4 bytes)
        let evt = unsafe { core::ptr::read_volatile(&EVT_BUF.buf[self.evt_read_idx / 4]) };
        self.evt_read_idx = (self.evt_read_idx + 4) % core::mem::size_of::<EventBuffer>();

        // Acknowledge this event
        unsafe { mmio::write32(dwc3_base() + GEVNTCOUNT, 4) };

        self.last_evt_raw = evt;
        self.evt_ring[(self.evt_ring_n as usize) % 12] = evt;
        self.evt_ring_n = self.evt_ring_n.wrapping_add(1);

        // Parse event
        if evt & EVT_NON_EP != 0 {
            // Device event
            let evt_type = (evt >> 8) & 0xF;
            match evt_type {
                DEVT_USBRST => UsbEvent::Reset,
                DEVT_CONNECTDONE => {
                    let dsts = unsafe { mmio::read32(dwc3_base() + DSTS) };
                    self.connected_speed = dsts & DSTS_CONNECTSPD_MASK;
                    UsbEvent::ConnectDone { speed: self.connected_speed }
                }
                DEVT_DISCONN => UsbEvent::Disconnect,
                _ => UsbEvent::None,
            }
        } else {
            // EP event
            let ep_phys = ((evt >> 1) & 0x1F) as u8;
            let evt_type = (evt >> 6) & 0xF;
            match evt_type {
                DEPEVT_XFERCOMPLETE => {
                    // Clear resource index — transfer is done
                    if ep_phys == 0 { self.ep0_resource_idx = 0; }
                    if ep_phys == 1 { self.ep1_resource_idx = 0; }
                    if ep_phys == 2 { self.bulk_out_resource_idx = 0; }
                    if ep_phys == 3 { self.bulk_in_resource_idx = 0; }

                    // Bulk OUT complete — read actual length from TRB
                    if ep_phys == 2 {
                        self.bulk_out_armed = false;
                        self.bulk_out_xfer_complete += 1;
                        unsafe {
                            crate::mmio::cache_invalidate(&raw mut BULK_OUT_TRB.trb as usize, 16);
                            let remaining = BULK_OUT_TRB.trb.size & 0x00FF_FFFF;
                            self.bulk_out_len = (512u32.saturating_sub(remaining)) as u16;
                            // Snapshot the raw TRB words for VENDOR_REQ_DBG2 — distinguishes a stale cache read (size still 512, HWO set) from a spurious event (TRB untouched) from a normal short read.
                            self.ep2_trb_size_snap = BULK_OUT_TRB.trb.size;
                            self.ep2_trb_ctrl_snap = BULK_OUT_TRB.trb.ctrl;
                        }
                        self.bulk_out_ready = true;
                        return UsbEvent::TransferComplete { ep: ep_phys };
                    }

                    // Bulk IN complete — mark idle
                    if ep_phys == 3 {
                        self.bulk_in_xfer_complete += 1;
                        self.bulk_in_idle = true;
                        return UsbEvent::TransferComplete { ep: ep_phys };
                    }

                    if ep_phys == 0 && self.ep0_state == Ep0State::Setup {
                        // SETUP packet = host is (re)configuring — clear armed flags so bulk endpoints get re-armed by the main loop
                        self.bulk_out_armed = false;
                        // SETUP packet received — invalidate cache to see DMA data
                        let setup_addr = &raw const EP0_SETUP_BUF as usize;
                        unsafe { crate::mmio::cache_invalidate(setup_addr, 8); }
                        let mut req = [0u8; 8];
                        unsafe {
                            for i in 0..8 {
                                req[i] = core::ptr::read_volatile(&EP0_SETUP_BUF.data[i]);
                            }
                        }
                        self.setup_count += 1;
                        self.last_setup_brequest = req[1];
                        self.last_setup_wvalue = (req[3] as u16) << 8 | req[2] as u16;
                        return UsbEvent::Ep0Setup { request: req };
                    }
                    // EP0 IN data stage complete → transition to Status and wait for XferNotReady before arming status OUT. (Reactive model — matches Linux DWC3 driver. Proactive status arming can race with DWC3's internal state machine.)
                    if ep_phys == 1 && self.ep0_state == Ep0State::DataIn {
                        self.ep1_xfer_complete += 1;
                        self.ep0_state = Ep0State::Status;
                        return UsbEvent::None;
                    }
                    // EP0 IN status stage complete (SET_ADDRESS, SET_CONFIG) → re-arm SETUP
                    if ep_phys == 1 && self.ep0_state == Ep0State::Status {
                        self.ep1_xfer_complete += 1;
                        self.ep0_state = Ep0State::Setup;
                        self.ep0_start_setup();
                        return UsbEvent::None;
                    }
                    // EP0 OUT status stage complete (after data IN) → re-arm SETUP
                    if ep_phys == 0 && self.ep0_state == Ep0State::Status {
                        self.ep0_state = Ep0State::Setup;
                        self.ep0_start_setup();
                        return UsbEvent::None;
                    }
                    UsbEvent::TransferComplete { ep: ep_phys }
                }
                DEPEVT_XFERNOTREADY => {
                    self.ep0_xfer_notready += 1;

                    // Bulk OUT XferNotReady — arm receive if configured
                    if ep_phys == 2 && self.configured {
                        self.bulk_out_arm();
                        return UsbEvent::None;
                    }
                    // Bulk IN XferNotReady — host polling, NAK until we send
                    if ep_phys == 3 {
                        return UsbEvent::None;
                    }

                    // EP1 XferNotReady in DataIn → proactive STARTTRANSFER failed, retry now that DWC3 is ready
                    if ep_phys == 1 && self.ep0_state == Ep0State::DataIn {
                        self.ep1_data_notready += 1;
                        if self.pending_ep1_trbctl != 0 {
                            let len = self.pending_ep1_len as usize;
                            let trbctl = self.pending_ep1_trbctl;
                            self.ep1_start_transfer(len, trbctl);
                        }
                        return UsbEvent::None;
                    }
                    // EP0 XferNotReady in DataIn → host wants status OUT
                    if ep_phys == 0 && self.ep0_state == Ep0State::DataIn {
                        self.ep0_status_out();
                        return UsbEvent::None;
                    }
                    // EP0 XferNotReady in Status → host wants status OUT
                    if ep_phys == 0 && self.ep0_state == Ep0State::Status {
                        self.ep0_status_out();
                        return UsbEvent::None;
                    }
                    // EP1 XferNotReady in Status → retry status IN
                    if ep_phys == 1 && self.ep0_state == Ep0State::Status {
                        if self.pending_ep1_trbctl != 0 {
                            let len = self.pending_ep1_len as usize;
                            let trbctl = self.pending_ep1_trbctl;
                            self.ep1_start_transfer(len, trbctl);
                        }
                        return UsbEvent::None;
                    }
                    if ep_phys == 1 && self.ep0_state == Ep0State::DataOut {
                        self.ep0_status_in();
                        return UsbEvent::None;
                    }
                    // EP0 XferNotReady in Setup → SETUP TRB wasn't armed (ep0_start_setup failed), re-arm now
                    if ep_phys == 0 && self.ep0_state == Ep0State::Setup {
                        self.ep0_start_setup();
                        return UsbEvent::None;
                    }
                    UsbEvent::TransferNotReady { ep: ep_phys }
                }
                _ => UsbEvent::None,
            }
        }
    }

    /// Cancel a pending bulk IN transfer and fully reset the endpoint. Used when the host disconnects/times out mid-transfer.
    pub fn cancel_bulk_in(&mut self) {
        if self.bulk_in_resource_idx != 0 {
            self.end_transfer_raw(3, self.bulk_in_resource_idx as u32);
        } else {
            self.force_end_transfer_unconditional(3);
        }
        self.ep_cmd(3, DEPCMD_CLEARSTALL, 0, 0, 0);
        self.bulk_in_idle = true;
    }

    /// Recover a stale endpoint: ENDTRANSFER + CLEARSTALL + re-arm. Call this when bulk_in_send() or bulk_out_arm() fails persistently.
    pub fn recover_endpoint(&mut self, ep_phys: u8) {
        self.force_end_transfer_unconditional(ep_phys);
        self.ep_cmd(ep_phys as u8, DEPCMD_CLEARSTALL, 0, 0, 0);
        match ep_phys {
            2 => {
                self.bulk_out_armed = false;
                self.bulk_out_ready = false;
                self.bulk_out_arm();
            }
            3 => {
                self.bulk_in_idle = true;
            }
            _ => {}
        }
    }

    /// Full shutdown: end all transfers, clear Run/Stop, soft reset. Call before hot-reload to leave DWC3 in a clean state for the next kernel's init().
    pub fn shutdown(&mut self) {
        // End any in-flight transfers on all endpoints
        self.handle_disconnect();
        // Clear Run/Stop (disconnect from bus)
        unsafe {
            let dctl = mmio::read32(dwc3_base() + DCTL);
            mmio::write32(dwc3_base() + DCTL, dctl & !DCTL_RUN_STOP);
        }
        phy_delay(10_000);
        // Device controller soft reset
        unsafe {
            let dctl = mmio::read32(dwc3_base() + DCTL);
            mmio::write32(dwc3_base() + DCTL, dctl | DCTL_CSFTRST);
            for _ in 0..100_000u32 {
                if mmio::read32(dwc3_base() + DCTL) & DCTL_CSFTRST == 0 { break; }
            }
        }
        // Mask events so DWC3 doesn't write to stale event buffer
        unsafe {
            let evt_size = core::mem::size_of::<EventBuffer>() as u32;
            mmio::write32(dwc3_base() + GEVNTSIZ, evt_size | GEVNTSIZ_INTMASK);
        }
    }

    /// Clean up all in-flight transfers on disconnect. Call from the kernel's Disconnect event handler so endpoints are in a known state when the host reconnects.
    pub fn handle_disconnect(&mut self) {
        // End any in-flight bulk transfers
        if self.bulk_out_resource_idx != 0 {
            self.end_transfer_raw(2, self.bulk_out_resource_idx as u32);
        }
        if self.bulk_in_resource_idx != 0 {
            self.end_transfer_raw(3, self.bulk_in_resource_idx as u32);
        }
        self.bulk_out_armed = false;
        self.bulk_out_ready = false;
        self.bulk_in_idle = true;
        self.configured = false;
    }

    pub fn force_end_transfer_unconditional(&mut self, ep_phys: u8) {
        self.end_transfer_raw(ep_phys, 1);
    }

    /// Issue raw ENDTRANSFER command with HIPRI_FORCERM (force remove). No CMDIOC — forced end doesn't generate completion events. Silently ignores CMDSTATUS errors.
    fn end_transfer_raw(&mut self, ep_phys: u8, rsc_idx: u32) {
        let base = dwc3_base() + 0xC800 + (ep_phys as usize) * 16;
        let saved = (self.cmd_status_fail, self.last_cmd_status, self.last_cmd_ep, self.last_cmd_type);
        unsafe {
            mmio::write32(base + 0x0C,
                DEPCMD_ENDTRANSFER | DEPCMD_HIPRI_FORCERM | DEPCMD_CMDACT
                | (rsc_idx << 16));
            for _ in 0..100_000u32 {
                if mmio::read32(base + 0x0C) & DEPCMD_CMDACT == 0 { break; }
            }
        }
        // DWC3 databook: wait ~100µs after ENDTRANSFER before new STARTTRANSFER
        phy_delay(50_000);
        self.cmd_status_fail = saved.0;
        self.last_cmd_status = saved.1;
        self.last_cmd_ep = saved.2;
        self.last_cmd_type = saved.3;
        match ep_phys {
            0 => self.ep0_resource_idx = 0,
            1 => self.ep1_resource_idx = 0,
            2 => self.bulk_out_resource_idx = 0,
            3 => self.bulk_in_resource_idx = 0,
            _ => {}
        }
    }

    /// Handle a USB bus reset event. ENDTRANSFER + clear stall + clear address. Full DEPSTARTCFG + SETEPCONFIG + SETTRANSFRESOURCE happens in handle_connect_done (after speed is known), ensuring SETTRANSFRESOURCE is always the LAST config command before transfers start.
    pub fn handle_reset(&mut self) {
        self.address = 0;
        self.configured = false;
        self.ep0_state = Ep0State::Setup;
        self.pending_ep1_len = 0;
        self.pending_ep1_trbctl = 0;

        // End any in-flight transfers so their resources are freed.
        if self.ep0_resource_idx != 0 {
            self.end_transfer_raw(0, self.ep0_resource_idx as u32);
        }
        if self.ep1_resource_idx != 0 {
            self.end_transfer_raw(1, self.ep1_resource_idx as u32);
        }
        if self.bulk_out_resource_idx != 0 {
            self.end_transfer_raw(2, self.bulk_out_resource_idx as u32);
        }
        if self.bulk_in_resource_idx != 0 {
            self.end_transfer_raw(3, self.bulk_in_resource_idx as u32);
        }
        self.bulk_out_ready = false;
        self.bulk_out_armed = false;
        self.bulk_in_idle = true;

        // Clear any stall condition
        self.ep_cmd(0, DEPCMD_CLEARSTALL, 0, 0, 0);
        self.ep_cmd(1, DEPCMD_CLEARSTALL, 0, 0, 0);
        self.ep_cmd(2, DEPCMD_CLEARSTALL, 0, 0, 0);
        self.ep_cmd(3, DEPCMD_CLEARSTALL, 0, 0, 0);

        // Clear device address
        unsafe {
            let dcfg = mmio::read32(dwc3_base() + DCFG);
            mmio::write32(dwc3_base() + DCFG, dcfg & !DCFG_DEVADDR_MASK);
        }
    }

    /// Handle ConnectDone — read speed, full EP0 re-init.
    ///
    /// SETEPCONFIG Modify doesn't work after USB bus reset on DWC3 3.30a (QCM6490) — the second enumeration always times out. So we do full DEPSTARTCFG + SETEPCONFIG(Init) + SETTRANSFRESOURCE here.
    ///
    /// The previous config descriptor timeout was caused by a separate XferNotReady ZLP bug, not by SETTRANSFRESOURCE corruption.
    pub fn handle_connect_done(&mut self) {
        let dsts = unsafe { mmio::read32(dwc3_base() + DSTS) };
        self.connected_speed = dsts & DSTS_CONNECTSPD_MASK;

        let mps: u32 = match self.connected_speed {
            4 | 5 => 512,
            _ => 64,
        };

        // Clear stalls on bulk endpoints before re-init. handle_reset() does this, but if a transfer was in-flight when the host disconnected, the endpoint hardware may have stalled between reset and connect-done.
        self.ep_cmd(2, DEPCMD_CLEARSTALL, 0, 0, 0);
        self.ep_cmd(3, DEPCMD_CLEARSTALL, 0, 0, 0);

        // Full re-init after USB bus reset
        self.ep_start_config(0);

        // EP0 OUT
        let par0 = (EP_TYPE_CONTROL << DEPCFGPAR0_EPTYPE_SHIFT)
            | (mps << DEPCFGPAR0_MPS_SHIFT);
        let par1 = (0 << DEPCFGPAR1_EPNUM_SHIFT)
            | DEPCFGPAR1_XFER_CMPL_EN
            | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(0, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(0, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // EP0 IN
        let par0 = (EP_TYPE_CONTROL << DEPCFGPAR0_EPTYPE_SHIFT)
            | (mps << DEPCFGPAR0_MPS_SHIFT)
            | (0 << DEPCFGPAR0_FIFONUM_SHIFT);
        let par1 = (1 << DEPCFGPAR1_EPNUM_SHIFT)
            | DEPCFGPAR1_XFER_CMPL_EN
            | DEPCFGPAR1_XFER_NRDY_EN;
        self.ep_cmd(1, DEPCMD_SETEPCONFIG, par0, par1, 0);
        self.ep_cmd(1, DEPCMD_SETTRANSFRESOURCE, 1, 0, 0);

        // Re-configure bulk endpoints after reset
        self.bulk_configure();
    }

    /// Handle EP0 SETUP packet. Returns true if handled.
    pub fn handle_setup(&mut self, req: &[u8; 8]) -> bool {
        let bm_request_type = req[0];
        let b_request = req[1];
        let w_value = (req[3] as u16) << 8 | req[2] as u16;
        let _w_index = (req[5] as u16) << 8 | req[4] as u16;
        let w_length = (req[7] as u16) << 8 | req[6] as u16;

        let _dir_in = bm_request_type & 0x80 != 0;

        match b_request {
            USB_REQ_SET_ADDRESS => {
                // Write address to DCFG before the status ZLP, matching Linux's DWC3 driver.  The DWC3 handles the USB 2.0 spec requirement internally: it responds to the status stage on the old address, then switches to the new address. Deferring the write creates a race where the host sends the next SETUP to the new address before DCFG is updated.
                self.address = w_value as u8;
                unsafe {
                    let dcfg = mmio::read32(dwc3_base() + DCFG);
                    mmio::write32(dwc3_base() + DCFG,
                        (dcfg & !DCFG_DEVADDR_MASK)
                        | ((self.address as u32) << DCFG_DEVADDR_SHIFT));
                }
                self.ep0_status_in();
                true
            }
            USB_REQ_GET_DESCRIPTOR => {
                let desc_type = (w_value >> 8) as u8;
                let desc_idx = (w_value & 0xFF) as u8;
                self.handle_get_descriptor(desc_type, desc_idx, w_length)
            }
            USB_REQ_SET_CONFIGURATION => {
                self.configured = w_value != 0;
                if self.configured {
                    self.bulk_out_arm();
                    self.bulk_in_idle = true;
                }
                self.ep0_status_in();
                true
            }
            USB_REQ_GET_STATUS => {
                // Return 2 bytes of zeros (self-powered, no remote wakeup)
                self.ep0_send(&[0, 0], TRBCTL_CONTROL_DATA);
                self.ep0_state = Ep0State::DataIn;
                true
            }
            VENDOR_REQ_DBG2 if bm_request_type == 0xC0 => {
                // Raw event-path readout: last 12 EP event words (oldest first), total event count, ep2 TRB size+ctrl snapshots, DEPCMD failure detail.
                let mut buf = [0u8; 64];
                let n = self.evt_ring_n as usize;
                for i in 0..12 {
                    let idx = if n >= 12 { (n + i) % 12 } else { i };
                    buf[i * 4..i * 4 + 4].copy_from_slice(&self.evt_ring[idx].to_le_bytes());
                }
                buf[48..52].copy_from_slice(&self.evt_ring_n.to_le_bytes());
                buf[52..56].copy_from_slice(&self.ep2_trb_size_snap.to_le_bytes());
                buf[56..60].copy_from_slice(&self.ep2_trb_ctrl_snap.to_le_bytes());
                buf[60..64].copy_from_slice(&self.last_cmd_status.to_le_bytes());
                let len = (w_length as usize).min(64);
                self.ep0_send(&buf[..len], TRBCTL_CONTROL_DATA);
                self.ep0_state = Ep0State::DataIn;
                true
            }
            VENDOR_REQ_DBG if bm_request_type == 0xC0 => {
                // Vendor debug readout: 16 LE u32 counters, independent of the bulk path. Slots 0-11 are written by the kernel PT loop via dbg_set/dbg_bump; slots 12-15 are driver state filled here.
                dbg_set(12, self.bulk_out_xfer_complete);
                dbg_set(13, self.bulk_in_xfer_complete);
                dbg_set(14, (self.bulk_in_idle as u32)
                    | ((self.bulk_out_armed as u32) << 1)
                    | ((self.bulk_out_ready as u32) << 2)
                    | ((self.configured as u32) << 3));
                dbg_set(15, self.cmd_status_fail);
                let mut buf = [0u8; 64];
                for i in 0..16 {
                    let v = unsafe { core::ptr::read_volatile(&raw const DBG_PT[i]) };
                    buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
                let len = (w_length as usize).min(64);
                self.ep0_send(&buf[..len], TRBCTL_CONTROL_DATA);
                self.ep0_state = Ep0State::DataIn;
                true
            }
            _ => false, // STALL
        }
    }

    /// Handle GET_DESCRIPTOR request.
    fn handle_get_descriptor(&mut self, desc_type: u8, desc_idx: u8, max_len: u16) -> bool {
        let desc: &[u8] = match desc_type {
            USB_DT_DEVICE => &DEVICE_DESC,
            USB_DT_CONFIGURATION => &CONFIG_DESC,
            USB_DT_STRING => {
                // String descriptors are packed into a single static to avoid the compiler generating a lookup table of absolute pointers. ABL loads the kernel at a different address than the linker assumes, so absolute pointers in .rodata tables are wrong. Using a single base (resolved via PC-relative `adr`) plus integer offsets is position-independent.
                let base = &raw const ALL_STRING_DESCS as *const u8;
                let (off, slen) = match desc_idx {
                    0 => (0usize, 4usize),
                    1 => (4, 24),   // "Nick Spiker"
                    2 => (28, 14),  // "ferros"
                    3 => (42, 4),   // "0"
                    _ => return false,
                };
                let desc = unsafe {
                    core::slice::from_raw_parts(base.add(off), slen)
                };
                let len = desc.len().min(max_len as usize);
                self.ep0_send(&desc[..len], TRBCTL_CONTROL_DATA);
                self.ep0_state = Ep0State::DataIn;
                return true;
            }
            USB_DT_DEVICE_QUALIFIER => &DEVICE_QUALIFIER_DESC,
            USB_DT_BOS => &BOS_DESC,
            _ => return false,
        };

        let len = desc.len().min(max_len as usize);
        self.ep0_send(&desc[..len], TRBCTL_CONTROL_DATA);
        self.ep0_state = Ep0State::DataIn;
        true
    }

    /// Current USB address.
    pub fn address(&self) -> u8 { self.address }

    /// Whether device is configured.
    pub fn is_configured(&self) -> bool { self.configured }

    /// Connected speed.
    pub fn connected_speed(&self) -> u32 { self.connected_speed }
}

// ---------------------------------------------------------------------------
// USB Descriptors — Vendor-specific device (for VSF/Photon Transport)
// ---------------------------------------------------------------------------

/// Device descriptor (18 bytes).
static DEVICE_DESC: [u8; 18] = [
    18,                 // bLength
    USB_DT_DEVICE,      // bDescriptorType
    0x00, 0x02,         // bcdUSB = 2.00 (HS-only, no SS PHY)
    0xFF,               // bDeviceClass = Vendor Specific
    0x00,               // bDeviceSubClass
    0x00,               // bDeviceProtocol
    64,                 // bMaxPacketSize0 = 64 (required for HS, valid for FS)
    0x09, 0x12,         // idVendor = 0x1209 (pid.codes)
    0x65, 0x46,         // idProduct = 0x4665 (ferros — requested via pid.codes PR #1208, pending merge)
    0x00, 0x01,         // bcdDevice = 1.00
    1,                  // iManufacturer (string index 1)
    2,                  // iProduct (string index 2)
    3,                  // iSerialNumber (string index 3)
    1,                  // bNumConfigurations
];

/// Configuration descriptor (9 config + 9 interface + 7 EP OUT + 7 EP IN = 32 bytes).
static CONFIG_DESC: [u8; 32] = [
    // Configuration descriptor
    9,                  // bLength
    USB_DT_CONFIGURATION, // bDescriptorType
    32, 0,              // wTotalLength
    1,                  // bNumInterfaces
    1,                  // bConfigurationValue
    0,                  // iConfiguration
    0xC0,               // bmAttributes = Self-powered
    0x00,               // bMaxPower = 0 (self-powered, no bus draw)

    // Interface descriptor
    9,                  // bLength
    USB_DT_INTERFACE,   // bDescriptorType
    0,                  // bInterfaceNumber
    0,                  // bAlternateSetting
    2,                  // bNumEndpoints
    0xFF,               // bInterfaceClass = Vendor Specific
    0x01,               // bInterfaceSubClass
    0x02,               // bInterfaceProtocol
    0,                  // iInterface (no string)

    // EP1 OUT (host→device) — Bulk, 512B MPS
    7,                  // bLength
    USB_DT_ENDPOINT,    // bDescriptorType
    0x01,               // bEndpointAddress = EP1 OUT
    0x02,               // bmAttributes = Bulk
    0x00, 0x02,         // wMaxPacketSize = 512
    0x00,               // bInterval

    // EP1 IN (device→host) — Bulk, 512B MPS
    7,                  // bLength
    USB_DT_ENDPOINT,    // bDescriptorType
    0x81,               // bEndpointAddress = EP1 IN
    0x02,               // bmAttributes = Bulk
    0x00, 0x02,         // wMaxPacketSize = 512
    0x00,               // bInterval
];

/// Device qualifier descriptor (for HS hosts asking about other speeds).
static DEVICE_QUALIFIER_DESC: [u8; 10] = [
    10,                 // bLength
    USB_DT_DEVICE_QUALIFIER,
    0x00, 0x02,         // bcdUSB = 2.00
    0xFF, 0x00, 0x00,   // class/subclass/protocol
    64,                 // bMaxPacketSize0
    1,                  // bNumConfigurations
    0,                  // bReserved
];

/// BOS descriptor (minimal — required for USB 3.x enumeration).
static BOS_DESC: [u8; 22] = [
    // BOS descriptor header
    5,                  // bLength
    USB_DT_BOS,         // bDescriptorType
    22, 0,              // wTotalLength
    2,                  // bNumDeviceCaps

    // USB 2.0 Extension
    7,                  // bLength
    0x10,               // bDescriptorType = Device Capability
    0x02,               // bDevCapabilityType = USB 2.0 Extension
    0x02, 0x00, 0x00, 0x00, // bmAttributes = LPM supported

    // SuperSpeed USB Device Capability
    10,                 // bLength
    0x10,               // bDescriptorType = Device Capability
    0x03,               // bDevCapabilityType = SuperSpeed
    0x00,               // bmAttributes
    0x0E, 0x00,         // wSpeedsSupported = FS | HS | SS
    0x01,               // bFunctionalitySupport = FS
    0x0A,               // bU1DevExitLat = 10us
    0x20, 0x00,         // wU2DevExitLat = 32us
];

/// All string descriptors packed contiguously. Using a single static avoids the compiler generating a lookup table of absolute pointers (which break when ABL loads the kernel at a different address than the linker assumed).
///
/// Layout: [STRING_DESC_0 (4B)] [STRING_DESC_1 "Nick Spiker" (24B)] [STRING_DESC_2 "ferros" (14B)] [STRING_DESC_3 "0" (4B)] Offsets: 0, 4, 28, 42  Total: 46 bytes
static ALL_STRING_DESCS: [u8; 46] = [
    // String 0: Language ID (English US) — offset 0, length 4
    4, USB_DT_STRING, 0x09, 0x04,
    // String 1 (iManufacturer): "Nick Spiker" — offset 4, length 24
    24, USB_DT_STRING,
    b'N', 0, b'i', 0, b'c', 0, b'k', 0, b' ', 0, b'S', 0, b'p', 0, b'i', 0, b'k', 0, b'e', 0, b'r', 0,
    // String 2 (iProduct): "ferros" — offset 28, length 14
    14, USB_DT_STRING,
    b'f', 0, b'e', 0, b'r', 0, b'r', 0, b'o', 0, b's', 0,
    // String 3 (iSerial): "0" — offset 42, length 4
    4, USB_DT_STRING,
    b'0', 0,
];
