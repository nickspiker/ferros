// FERROS HAL SOURCE MAP — keep updated when pub items or files change
//
// lib.rs ── no_std HAL, module re-exports, alloc extern
//
// mmio.rs ── volatile MMIO register access read32(addr), write32(addr, val) read16(addr), write16(addr, val) read8(addr), write8(addr, val)
//
// console.rs ── framebuffer text console (8x16 VGA font, 2x scaled) struct Console { fb, stride, x_off, y_off, col, row } ::new(fb, stride, width, height), putc(ch), puts(s), clear() FONT: 95-char ASCII bitmap font (0x20-0x7E)
//
// fb.rs ── raw framebuffer pixel access (ABL splash at G#E1000000) fb_write_pixel(fb, stride, x, y, r, g, b) fb_fill_rect(fb, stride, x, y, w, h, r, g, b)
//
// dtb.rs ── minimal DTB/FDT parser struct Dtb — parse from raw pointer ::bootargs(), memory_regions(), reserved_memory() ::find_node(name), find_prop(node, name) ::find_ramoops() → RamoopsConfig
//
// uart.rs ── GENI UART TX (QUP1 SE5 at G#994000) struct Uart { base } ::new(base), probe() → bool, puts(s), putc(u8) enum UartBackend { Geni, None }
//
// sdmmc.rs ── SDHCI SD/MMC controller (SDC2 at G#8804000) struct SdhciController { base, initialized, rca } ::new(base), init() → Result, probe_card() → ProbeResult ::read_block(lba, buf), write_block(lba, buf) ::read_blocks(lba, count, buf) — multi-block CMD23+CMD18 struct ProbeResult { cmd0_ok, cmd8_ok, ..., csd, cid, rca } TLMM pad config, PWRCTL handshake, 4-bit bus width
//
// gcc.rs ── GCC clock controller (SDC2 branches + RCG) sdc2_clock_init() → (ahb_ok, apps_ok) sdc2_set_400khz(), sdc2_set_25mhz() (25MHz needs DLL)
//
// rpmh.rs ── RPMh TCS for LDO power via cmd-db cmd_db_lookup(name) → Option<u32> vrm_set_voltage(addr, mv) → bool vrm_enable(addr) → bool tcs_trigger(slot, cmds) → bool
//
// dpu.rs ── DPU register reads (VIG0/DMA0 source address probing) probe_dpu() → DpuProbe { vig0_src_addr, stride, ... }
//
// spmi.rs ── SPMI arbiter observer reads/writes (v5, G#C440000) ppid(sid, pid) → u16 find_apid(ppid) → Option<u16> read_byte(apid, reg) → Option<u8> write_byte(apid, reg, val) → bool
//
// pstore.rs ── ramoops persistent log (warm reboot survives) struct Ramoops { base, size } ::write(data), ::init_from_dtb(dtb) → Option<Self> struct RamoopsConfig { base, size, console_size, pmsg_size }
//
// usb.rs ── DWC3 USB device mode (G#A600000, Qualcomm wrapper G#A6F8800) struct Dwc3Dev — full device-mode controller state ::init() → Option<Self> ::poll_event() → UsbEvent ::bulk_in_send(data) → bool ::bulk_out_arm(), bulk_out_read(buf) → usize enum UsbEvent { None, Reset, SetupData, TransferComplete, ... } phy_init(), smmu_bypass(), probe() → Dwc3Info Single-TRB model, 1ms pacing for multi-packet inbound
//
// ufs.rs ── UFS host controller (UFSHCI v3.0 at G#1D84000) struct Ufs { base } ::new(), link_is_up() → bool ::nop_out() → bool ::scsi_read(lun, lba, blocks, buf) → bool ::scsi_write(lun, lba, blocks, buf) → bool ::query_descriptor(idn, index, buf) → Option<usize> 4KB blocks, 232GB LUN0, ABL leaves controller enabled
//
// ring.rs ── vault root ring on UFS/SD (spec in RING.md) (implementation pending)
//
// pmic_glink.rs ── SMEM/GLINK transport to ADSP charger_pd (BATTMGR) probe_smem() → SmemProbe — read-only SMEM/GLINK state for diagnostics probe_rtc() → Option<u32> — PMK8350 RTC via SPMI SID=0 PID=G#61 offset=G#48 struct PmicGlink { desc, tx_fifo, rx_fifo, rcid } ::init() → Option<Self> — locate SMEM items 478/479/480 ::open_channel() → bool — GLINK VERSION + OPEN handshake ::bat_status() → Option<BatStatus> — voltage/SOC/current/temp ::property_get(prop) → Option<u32> — single battery property ::set_charge_limit(target_soc, delta) → bool — cap charging at target% struct BatStatus — state, capacity_pct, rate_ma, voltage_mv, source, temp_tenths_k PROP_VOLT_NOW(7), PROP_CURR_NOW(9), PROP_CAPACITY(4), PROP_TEMP(12)

//! Ferros Hardware Abstraction Layer
//!
//! `#![no_std]` — runs bare metal or with alloc provided by the kernel.
//!
//! The `alloc` feature (default) enables modules that require heap allocation (sdmmc, usb, console, pmic_glink). Disable for the seed which has no allocator.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod usb_trait;
pub use usb_trait::{UsbBulk, UsbEvent};

pub mod mmio;
pub mod uart;
pub mod fb;
pub mod dtb;
pub mod ufs;
pub mod gcc;
pub mod rpmh;
pub mod qtimer;            // aarch64 QTIMER read (moved here from old vsf_mini.rs)
pub mod ring;
pub mod gic;
pub mod hyp;
pub mod dpu;
pub mod pstore;
pub mod spmi;

// Modules requiring alloc
#[cfg(feature = "alloc")]
pub mod sdmmc;
#[cfg(feature = "alloc")]
pub mod console;
#[cfg(feature = "alloc")]
pub mod usb;
#[cfg(feature = "alloc")]
pub mod pmic_glink;
#[cfg(feature = "alloc")]
pub mod hamt;
