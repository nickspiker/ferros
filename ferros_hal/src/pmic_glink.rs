//! Qualcomm pmic-glink driver — SMEM/GLINK transport to ADSP charger_pd.
//!
//! The charger PMIC (PM7250B SID 8) is owned by the ADSP. APPS reaches it via:
//!
//! ```text
//! APPS ──[IPCC doorbell G#408000]──► ADSP ↕ SMEM FIFOs G#80900000 TOC[478-480] ↕ GLINK protocol (intentless) ↕ channel "PMIC_RTR_ADSP_APPS" ↕ pmic-glink framing (owner+type+opcode) ↕ BATTMGR opcodes (status, property, charge ctrl)
//! ```
//!
//! ABL boots the ADSP and initializes GLINK before handing off. SMEM items 478/479/480 are pre-allocated; ADSP may have already written a VERSION packet to RX FIFO by the time our kernel starts.
//!
//! ## RTC note
//!
//! No pmic-glink RTC path exists. The RTC (PMK8350) lives at SPMI SID=0, PID=G#61, registers G#6148-G#614B (4 bytes LE = Unix epoch seconds). [`probe_rtc`] reads it directly if the arbiter grants access.

use crate::mmio;

// ---------------------------------------------------------------------------
// IPCC — Inter-Processor Communication Controller (G#408000)
// ---------------------------------------------------------------------------

const IPCC_BASE: usize = 0x0040_8000;
const IPCC_SEND_ID: usize = IPCC_BASE + 0x0C; // write to ring remote doorbell

/// Doorbell to signal ADSP GLINK: client=LPASS(3)<<16 | signal=GLINK_QMP(0). IPCC client IDs (SC7280/QCM6490 from Linux qcom-ipcc.h): AOP=0, TZ=1, MPSS=2, LPASS=3, SLPI=4, CDSP=5, NPU=6, APSS=7. Signal 0 = IPCC_MPROC_SIGNAL_GLINK_QMP (standard GLINK doorbell).
const ADSP_DOORBELL: u32 = 3 << 16 | 0;  // LPASS client, GLINK_QMP signal
/// SMP2P doorbell: same LPASS client, signal 2 = IPCC_MPROC_SIGNAL_SMP2P.
const ADSP_SMP2P_DOORBELL: u32 = 3 << 16 | 2;

// ---------------------------------------------------------------------------
// SMEM — Shared Memory (G#80900000, 2MB)
// ---------------------------------------------------------------------------

const SMEM_BASE: usize = 0x8090_0000;
// Actual smem_header layout (from drivers/soc/qcom/smem.c): proc_comm[4]:  4 × {command,status,data1,data2} × 4 = 64 bytes → 0x00 version[32]:   32 × 4 = 128 bytes → 0x40 initialized:   u32 → 0xC0 free_offset:   u32 → 0xC4 available:     u32 → 0xC8 reserved:      u32 → 0xCC toc[512]:      512 × 16 bytes → 0xD0 Actual smem_header layout (from drivers/soc/qcom/smem.c): proc_comm[4]:  4 × {command,status,data1,data2} × 4 = 64 bytes → 0x00 version[32]:   32 × 4 = 128 bytes → 0x40 initialized:   u32 → 0xC0 free_offset:   u32 → 0xC4 available:     u32 → 0xC8 reserved:      u32 → 0xCC toc[512]:      512 × 16 bytes → 0xD0
const SMEM_INIT_OFF: usize    = 0xC0; // u32 — must be 1 when SMEM is ready
const SMEM_VERSION_OFF: usize  = 0x40; // version[32] array start
const SMEM_TOC_OFF: usize     = 0xD0; // global heap TOC: 512 × 16-byte entries
const SMEM_TOC_STRIDE: usize  = 16;

// Ptable (partition table) — maps private partitions between processor pairs. Located in the last 4KB of SMEM: SMEM_BASE + SMEM_SIZE - 0x1000. smem_ptable header (32 bytes): [0]  magic[4]    = "$TOC" = bytes 24 54 4F 43, LE u32 = G#434F5424 [4]  version     = u32 [8]  num_entries = u32 [12] reserved[5] = u32 × 5 = 20 bytes [32] entries[]   = smem_ptable_entry × num_entries smem_ptable_entry (48 bytes): [0]  offset   u32 — byte offset from SMEM_BASE to partition data [4]  size     u32 — partition size in bytes [8]  flags    u32 [12] host0    u16 — first host (APPS=0, ADSP=2, ...) [14] host1    u16 — second host [16] cacheline u32 [20] reserved[7] u32
const SMEM_PTABLE_OFF: usize      = 0x1FF000; // last 4KB: SMEM_BASE + SMEM_SIZE - 0x1000
const SMEM_PTABLE_MAGIC: u32      = 0x434F_5424; // bytes 24 54 4F 43 = "$TOC" LE
const SMEM_PTABLE_HDR_SIZE: usize = 32;
const SMEM_PTABLE_ENTRY_SIZE: usize = 48;

// Private partition header (smem_partition_header, 32 bytes): [0]  magic    u32 = G#54525024 (bytes 24 50 52 54 = "$PRT") [4]  host0    u16 [6]  host1    u16 [8]  size     u32 [12] offset_free_uncached u32 — next free byte offset from partition base [16] offset_free_cached   u32 [20] reserved[3] u32
const SMEM_PARTITION_MAGIC: u32    = 0x5452_5024; // bytes 24 50 52 54 = "$PRT" LE

// Private entry header (smem_private_entry, 16 bytes): [0]  canary       u16 = G#A5A5 for valid entries [2]  item         u16 [4]  size         u32 — padded data size (data + padding_data) [8]  padding_data u16 — padding bytes at end of data region [10] padding_hdr  u16 — padding bytes at end of entry header [12] reserved     u32
const SMEM_PRIVATE_CANARY: u16 = 0xa5a5;

const SMEM_ITEM_DESC:    usize = 478; // 32-byte GLINK descriptor (head/tail pointers)
const SMEM_ITEM_ADSP_TX: usize = 479; // ADSP TX FIFO (ADSP writes → APPS reads)
const SMEM_ITEM_APPS_TX: usize = 480; // APPS TX FIFO (APPS writes → ADSP reads), APPS allocates

// ---------------------------------------------------------------------------
// SMP2P — Shared Memory Point-to-Point (readiness signaling)
// ---------------------------------------------------------------------------
//
// ADSP waits for APPS to clear the "stop" bit in SMP2P before starting GLINK transport. Without this signal, ADSP's charger_pd stays dormant.
//
// SMEM items: 443 = APPS→ADSP (outbound, APPS writes) 429 = ADSP→APPS (inbound, ADSP writes)
//
// smp2p_smem_item layout (G#154 = 340 bytes): [0x00] magic          u32 = G#504D5324 ("$SMP") [0x04] version        u8  = 1 [0x05] features       u24 (3 bytes) [0x08] local_pid      u16 (writer's processor ID) [0x0A] remote_pid     u16 (reader's processor ID) [0x0C] total_entries  u16 (max 16) [0x0E] valid_entries  u16 (currently valid) [0x10] flags          u32 (SSR flags) [0x14] entries[16]    each: name[16] + value[4] = 20 bytes
//
// Processor IDs: APPS=0, Modem=1, ADSP/LPASS=2, WCNSS=3, SLPI=4, CDSP=5
//
// Entry "master-kernel": bit 0 of value = stop signal. Set = APPS requesting ADSP to stop. Clear = APPS ready, ADSP may proceed.

const SMP2P_APPS_TO_ADSP: usize = 443; // SMEM item: APPS→ADSP SMP2P
const SMP2P_ADSP_TO_APPS: usize = 429; // SMEM item: ADSP→APPS SMP2P
const SMP2P_MAGIC: u32 = 0x504D_5324;  // "$SMP" in LE
const SMP2P_MAX_ENTRY: usize = 16;
const SMP2P_ENTRY_SIZE: usize = 20;    // name[16] + value[4]
const SMP2P_HEADER_SIZE: usize = 0x14; // 20 bytes before entries[]

const FIFO_SIZE: usize = 0x4000; // 16 KiB — must be a power of 2
const FIFO_MASK: usize = FIFO_SIZE - 1;

// SMEM TOC entry layout (each field u32 LE): [0] allocated  — 1 = in use [4] offset     — byte offset from SMEM_BASE to data [8] size [12] aux_base

// GLINK descriptor layout (32 bytes, all u32 LE) — from ADSP's perspective: [0]  ADSP tx_tail — APPS advances when consuming from item 479 (our rx_tail) [4]  ADSP tx_head — ADSP advances when writing to item 479     (our rx_head) [8]  ADSP rx_tail — ADSP advances when consuming from item 480 (our tx_tail) [12]  ADSP rx_head — APPS advances when writing to item 480     (our tx_head)

// ---------------------------------------------------------------------------
// GLINK protocol
// ---------------------------------------------------------------------------

const GLINK_CMD_VERSION:     u16 = 0;
const GLINK_CMD_VERSION_ACK: u16 = 1;
const GLINK_CMD_OPEN:        u16 = 2;
const GLINK_CMD_OPEN_ACK:    u16 = 4;
const GLINK_CMD_TX_DATA:     u16 = 9;

const GLINK_VERSION_1: u16 = 1;
const GLINK_FEATURE_INTENTLESS: u32 = 1 << 1; // no intent exchange needed

const CHANNEL_NAME: &[u8] = b"PMIC_RTR_ADSP_APPS\0"; // 19 bytes incl. null
const OUR_LCID: u16 = 1; // local channel ID we assign

// ---------------------------------------------------------------------------
// pmic-glink message constants
// ---------------------------------------------------------------------------

const OWNER_BATTMGR: u32 = 0x800A;

const MSG_REQ_RESP: u32 = 1;
#[allow(dead_code)]
const MSG_NOTIFY: u32 = 2;

const OP_BAT_STATUS:    u32 = 0x01; // full battery snapshot
const OP_PROPERTY_GET:  u32 = 0x30; // single property read
const OP_CHG_CTRL:      u32 = 0x48; // charge control limit

/// Battery property IDs for [`PmicGlink::property_get`].
pub const PROP_CAPACITY:  u32 = 4;  // percent 0-100
pub const PROP_VOLT_NOW:  u32 = 7;  // µV
pub const PROP_CURR_NOW:  u32 = 9;  // µA (signed as u32 two's complement)
pub const PROP_TEMP:      u32 = 12; // tenths °C
pub const PROP_CHG_CTRL:  u32 = 24; // 0=off, 1=limited

const POLL_MAX: u32 = 8_000_000; // spin iterations before timeout (~2-4 seconds)

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Read-only SMEM/GLINK state snapshot. Zero cost, safe to call any time.
pub struct SmemProbe {
    /// SMEM initialized flag (must be 1 for GLINK to work).
    pub initialized:    u32,
    /// version[7] — expected A#15 for global heap (SMEM_GLOBAL_HEAP_VERSION).
    pub version7:       u32,
    /// Global heap TOC[478] allocated flag (usually 0 — items in private partition).
    pub desc_alloc:     u32,
    /// Global heap TOC[479] allocated flag.
    pub tx_alloc:       u32,
    /// Global heap TOC[480] allocated flag.
    pub rx_alloc:       u32,
    /// Descriptor: ADSP's read position in TX FIFO.
    pub tx_tail:        u32,
    /// Descriptor: our write position in TX FIFO.
    pub tx_head:        u32,
    /// Descriptor: our read position in RX FIFO.
    pub rx_tail:        u32,
    /// Descriptor: ADSP's write position in RX FIFO — non-zero means ADSP wrote.
    pub rx_head:        u32,
    /// Ptable magic at SMEM_BASE+G#1FF000 (G#434F5424 = "$TOC" if valid).
    pub ptable_magic:   u32,
    /// Number of ptable entries.
    pub ptable_entries: u32,
    /// Offset of APPS(0)↔ADSP(2) private partition from SMEM_BASE, G#FFFFFFFF if not found.
    pub adsp_part_off:  u32,
    /// Size of APPS↔ADSP private partition.
    pub adsp_part_size: u32,
    /// Partition magic at the partition base (G#54525024 = "$PRT" if valid).
    pub part_magic:     u32,
    /// Partition offset_free_uncached (uncached items end here).
    pub part_free_off:  u32,
    /// Partition offset_free_cached (cached items start here; smaller = more cached items).
    pub part_free_cac:  u32,
    /// Whether item 478 (GLINK descriptor) was found in private partition.
    pub priv_desc_found: bool,
    /// Whether item 479 (TX FIFO) was found in private partition.
    pub priv_tx_found:   bool,
    /// Whether item 480 (RX FIFO) was found in private partition.
    pub priv_rx_found:   bool,
}

/// Battery snapshot from `BATTMGR_BAT_STATUS` (opcode G#01).
#[derive(Copy, Clone, Default)]
pub struct BatStatus {
    /// Bitmask: bit0=discharging, bit1=charging, bit2=critical.
    pub state:          u32,
    /// State of charge, 0-100 percent.
    pub capacity_pct:   u32,
    /// Charge/discharge current in mA (interpretation depends on `state`).
    pub rate_ma:        u32,
    /// Terminal voltage in millivolts.
    pub voltage_mv:     u32,
    /// Charging source: 0=none, 1=AC, 2=USB, 3=wireless.
    pub source:         u32,
    /// Temperature in tenths of Kelvin (subtract A#2732 for tenths of °C).
    pub temp_tenths_k:  u32,
}

impl BatStatus {
    pub fn is_charging(&self) -> bool { self.state & 2 != 0 }
    pub fn temp_tenths_c(&self) -> i32 { self.temp_tenths_k as i32 - 2732 }
}

// Internal: a decoded GLINK frame from the RX FIFO.
struct GlinkMsg {
    cmd:         u16,
    param1:      u16,
    param2:      u32,
    payload:     [u8; 256], // first 256 bytes of payload (enough for all BATTMGR responses)
    payload_len: usize,
}

// ---------------------------------------------------------------------------
// Standalone probes (no state mutation)
// ---------------------------------------------------------------------------

/// Read SMEM/GLINK state without touching any head/tail pointers.
///
/// Safe to call at any time during boot for diagnostics.
pub fn probe_smem() -> SmemProbe {
    let initialized = unsafe { mmio::read32(SMEM_BASE + SMEM_INIT_OFF) };
    let version7    = unsafe { mmio::read32(SMEM_BASE + SMEM_VERSION_OFF + 7 * 4) };

    // --- global heap TOC (items may be 0 if stored in private partition) ---
    let desc_entry = smem_toc_ptr(SMEM_ITEM_DESC);
    let tx_entry   = smem_toc_ptr(SMEM_ITEM_ADSP_TX);
    let rx_entry   = smem_toc_ptr(SMEM_ITEM_APPS_TX);
    let desc_alloc = unsafe { mmio::read32(desc_entry) };
    let tx_alloc   = unsafe { mmio::read32(tx_entry) };
    let rx_alloc   = unsafe { mmio::read32(rx_entry) };

    let (tx_tail, tx_head, rx_tail, rx_head) = if desc_alloc == 1 {
        let off = unsafe { mmio::read32(desc_entry + 4) } as usize;
        let d = SMEM_BASE + off;
        unsafe { (
            mmio::read32(d + 0),
            mmio::read32(d + 4),
            mmio::read32(d + 8),
            mmio::read32(d + 12),
        ) }
    } else {
        (0, 0, 0, 0)
    };

    // --- ptable ---
    let ptable_base   = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic  = unsafe { mmio::read32(ptable_base) };
    let ptable_entries = if ptable_magic == SMEM_PTABLE_MAGIC {
        unsafe { mmio::read32(ptable_base + 8) }
    } else {
        0
    };

    // Find APPS(0)↔ADSP(2) private partition.
    let (adsp_part_off, adsp_part_size, part_magic, part_free_off, part_free_cac) =
        find_adsp_partition(ptable_base, ptable_entries as usize);

    // Walk private partition (both uncached and cached regions) for items 478/479/480.
    let (priv_desc_found, priv_tx_found, priv_rx_found) =
        if adsp_part_off != 0xFFFF_FFFF && part_magic == SMEM_PARTITION_MAGIC {
            let part_base = SMEM_BASE + adsp_part_off as usize;
            let psize     = adsp_part_size as usize;
            let fuc       = part_free_off as usize;
            let fca       = part_free_cac as usize;
            let d = find_private_item(part_base, fuc, fca, psize, SMEM_ITEM_DESC);
            let t = find_private_item(part_base, fuc, fca, psize, SMEM_ITEM_ADSP_TX);
            let r = find_private_item(part_base, fuc, fca, psize, SMEM_ITEM_APPS_TX);
            (d.is_some(), t.is_some(), r.is_some())
        } else {
            (false, false, false)
        };

    // Read descriptor head/tail from private partition if found.
    let (tx_tail, tx_head, rx_tail, rx_head) = if priv_desc_found {
        let part_base = SMEM_BASE + adsp_part_off as usize;
        let psize     = adsp_part_size as usize;
        let fuc       = part_free_off as usize;
        let fca       = part_free_cac as usize;
        if let Some(d) = find_private_item(part_base, fuc, fca, psize, SMEM_ITEM_DESC) {
            unsafe { (
                mmio::read32(d + 0),
                mmio::read32(d + 4),
                mmio::read32(d + 8),
                mmio::read32(d + 12),
            ) }
        } else {
            (tx_tail, tx_head, rx_tail, rx_head)
        }
    } else {
        (tx_tail, tx_head, rx_tail, rx_head)
    };

    SmemProbe {
        initialized, version7,
        desc_alloc, tx_alloc, rx_alloc,
        tx_tail, tx_head, rx_tail, rx_head,
        ptable_magic, ptable_entries,
        adsp_part_off, adsp_part_size,
        part_magic, part_free_off, part_free_cac,
        priv_desc_found, priv_tx_found, priv_rx_found,
    }
}

/// Dump all ptable entries: fills `out` with (host0, host1, offset, size) tuples. Returns the number of entries written.
pub fn dump_ptable_entries(out: &mut [(u16, u16, u32, u32)]) -> usize {
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC { return 0; }
    let count = (unsafe { mmio::read32(ptable_base + 8) } as usize).min(out.len()).min(64);
    let entries_base   = ptable_base + SMEM_PTABLE_HDR_SIZE;
    for i in 0..count {
        let e    = entries_base + i * SMEM_PTABLE_ENTRY_SIZE;
        let off  = unsafe { mmio::read32(e + 0) };
        let size = unsafe { mmio::read32(e + 4) };
        let h0   = unsafe { mmio::read16(e + 12) };
        let h1   = unsafe { mmio::read16(e + 14) };
        out[i] = (h0, h1, off, size);
    }
    count
}

/// Scan all items in the APPS↔ADSP private partition. Fills `out` with (item_id, data_size) pairs.  Returns count found.
pub fn scan_adsp_items(out: &mut [(u16, u32)]) -> usize {
    scan_partition_items(2, out)
}

/// Scan all items in the APPS↔CDSP private partition (host0=0, host1=5). pmic-glink runs on CDSP on some QCM6490 builds.
pub fn scan_cdsp_items(out: &mut [(u16, u32)]) -> usize {
    scan_partition_items(5, out)
}

/// Scan all items in any partition identified by host pair.
pub fn scan_host_items(host_a: u16, host_b: u16, out: &mut [(u16, u32)]) -> usize {
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC { return 0; }
    let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
    let (part_off, part_size, part_magic, free_unc, free_cac) =
        find_host_partition(ptable_base, ptable_entries, host_a, host_b);
    if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC { return 0; }
    let part_base = SMEM_BASE + part_off as usize;
    let mut n = 0;
    let unc_start = part_base + 32;
    let unc_end   = part_base + free_unc as usize;
    let mut cur = unc_start;
    while cur + 16 <= unc_end && n < out.len() {
        let canary = unsafe { mmio::read16(cur + 0) };
        if canary != SMEM_PRIVATE_CANARY { break; }
        let item_id     = unsafe { mmio::read16(cur + 2) };
        let data_size   = unsafe { mmio::read32(cur + 4) };
        let padding_hdr = unsafe { mmio::read16(cur + 10) } as usize;
        out[n] = (item_id, data_size);
        n += 1;
        let step = 16 + padding_hdr + data_size as usize;
        if step == 0 { break; }
        cur += step;
    }
    let cac_start = part_base + free_cac as usize;
    let cac_end   = part_base + part_size as usize;
    cur = cac_start;
    while cur + 16 <= cac_end && n < out.len() {
        let canary = unsafe { mmio::read16(cur + 0) };
        if canary != SMEM_PRIVATE_CANARY { break; }
        let item_id     = unsafe { mmio::read16(cur + 2) };
        let data_size   = unsafe { mmio::read32(cur + 4) };
        let padding_hdr = unsafe { mmio::read16(cur + 10) } as usize;
        out[n] = (item_id, data_size);
        n += 1;
        let step = 16 + padding_hdr + data_size as usize;
        if step == 0 { break; }
        cur += step;
    }
    n
}

fn scan_partition_items(remote_host: u16, out: &mut [(u16, u32)]) -> usize {
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC { return 0; }
    let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
    let (part_off, part_size, part_magic, free_unc, free_cac) =
        find_host_partition(ptable_base, ptable_entries, 0, remote_host);
    if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC { return 0; }
    let part_base = SMEM_BASE + part_off as usize;
    let mut n = 0;
    // Scan uncached region (items grow from partition_base+32 upward).
    let unc_start = part_base + 32;
    let unc_end   = part_base + free_unc as usize;
    let mut cur = unc_start;
    while cur + 16 <= unc_end && n < out.len() {
        let canary = unsafe { mmio::read16(cur + 0) };
        if canary != SMEM_PRIVATE_CANARY { break; }
        let item_id     = unsafe { mmio::read16(cur + 2) };
        let data_size   = unsafe { mmio::read32(cur + 4) };
        let padding_hdr = unsafe { mmio::read16(cur + 10) } as usize;
        out[n] = (item_id, data_size);
        n += 1;
        let step = 16 + padding_hdr + data_size as usize;
        if step == 0 { break; }
        cur += step;
    }
    // Scan cached region (items grow from partition end downward).
    let cac_start = part_base + free_cac as usize;
    let cac_end   = part_base + part_size as usize;
    cur = cac_start;
    while cur + 16 <= cac_end && n < out.len() {
        let canary = unsafe { mmio::read16(cur + 0) };
        if canary != SMEM_PRIVATE_CANARY { break; }
        let item_id     = unsafe { mmio::read16(cur + 2) };
        let data_size   = unsafe { mmio::read32(cur + 4) };
        let padding_hdr = unsafe { mmio::read16(cur + 10) } as usize;
        out[n] = (item_id, data_size);
        n += 1;
        let step = 16 + padding_hdr + data_size as usize;
        if step == 0 { break; }
        cur += step;
    }
    n
}

/// Dump a slice of the APPS↔ADSP private partition as hex bytes (for diagnostics).
///
/// Returns up to `out.len()` bytes starting at `offset` from the partition base. Returns the number of bytes actually copied.
pub fn dump_adsp_partition(offset: usize, out: &mut [u8]) -> usize {
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC { return 0; }
    let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
    let (part_off, part_size, part_magic, _, _) =
        find_adsp_partition(ptable_base, ptable_entries);
    if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC { return 0; }
    let part_base = SMEM_BASE + part_off as usize;
    let avail = (part_size as usize).saturating_sub(offset);
    let n = out.len().min(avail);
    for i in 0..n {
        out[i] = unsafe { mmio::read8(part_base + offset + i) };
    }
    n
}

/// Read raw ADSP partition state WITHOUT modifying anything.
///
/// Returns (desc_addr, th, rx_fifo_addr, rx_bytes_16) where:
/// - `desc_addr`: physical address of item 478 (0 if not found)
/// - `th`: desc[4] = ADSP tx_head (non-zero means ADSP pre-wrote VERSION)
/// - `rx_fifo_addr`: physical address of item 479 (ADSP TX FIFO)
/// - `rx_bytes_16`: first 16 bytes of item 479 (raw ADSP TX FIFO content)
pub fn probe_adsp_raw() -> (u32, u32, u32, [u8; 16]) {
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC {
        return (0, 0, 0, [0u8; 16]);
    }
    let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
    let (part_off, part_size, part_magic, free_unc, free_cac) =
        find_host_partition(ptable_base, ptable_entries, 0, 2); // ADSP = host 2
    if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC {
        return (0, 0, 0, [0u8; 16]);
    }
    let part_base = SMEM_BASE + part_off as usize;
    let desc_addr = match find_private_item(part_base, free_unc as usize,
                                            free_cac as usize, part_size as usize,
                                            SMEM_ITEM_DESC) {
        Some(a) => a,
        None    => return (0, 0, 0, [0u8; 16]),
    };
    let rx_addr = match find_private_item(part_base, free_unc as usize,
                                          free_cac as usize, part_size as usize,
                                          SMEM_ITEM_ADSP_TX) {
        Some(a) => a,
        None    => return (desc_addr as u32, 0, 0, [0u8; 16]),
    };
    let th = unsafe { mmio::read32(desc_addr + 4) };
    let mut rx_bytes = [0u8; 16];
    for i in 0..16 {
        rx_bytes[i] = unsafe { mmio::read8(rx_addr + i) };
    }
    (desc_addr as u32, th, rx_addr as u32, rx_bytes)
}

/// Battery state from QG (fuel gauge) + SDAM direct SPMI reads.
///
/// Bypasses GLINK entirely — reads raw hardware registers. QG at SID 8 PID G#C8, SDAM at SID 8 PID G#70.
#[derive(Copy, Clone, Default)]
pub struct QgBatteryState {
    /// Raw QG VBAT ADC code from C8[G#50-G#51] (u16 LE).
    pub vbat_raw: u16,
    /// Estimated battery voltage in mV (vbat_raw × G#9C / G#28, ~2.44 mV/code).
    pub vbat_mv: u32,
    /// SDAM 70[G#40] validity byte (non-zero = QG data valid).
    pub sdam_valid: u8,
    /// SDAM 70 data region (10 bytes at G#40-G#49).
    pub sdam_data: [u8; 10],
    /// SCHG_USB C9 status: reg[G#09] = USB charger real-time status.
    pub usb_rt_sts: u8,
    /// SCHG_CHGR C7 status: reg[G#09] (real-time charge status).
    pub chgr_rt_sts: u8,
    /// SCHG_MISC CB: reg[G#07] = MISC status.
    pub misc_sts: u8,
}

/// Read battery state directly from QG/SDAM/SCHG registers via SPMI.
///
/// Works without GLINK — uses SPMI arbiter-granted read access to SID 8.
pub fn probe_battery_qg() -> QgBatteryState {
    use crate::spmi;
    let mut st = QgBatteryState::default();

    // QG at SID 8, PID 0xC8: read VBAT raw at registers 0x50-0x51.
    let c8_ppid = spmi::ppid(8, 0xC8);
    if let Some(apid) = spmi::find_apid(c8_ppid) {
        let lo = spmi::read_byte(apid, 0x50).unwrap_or(0) as u16;
        let hi = spmi::read_byte(apid, 0x51).unwrap_or(0) as u16;
        st.vbat_raw = lo | (hi << 8);
        // Conversion: ~2.441 mV per code (5000 mV / 2048 codes, 11-bit effective). Use integer math: raw * 5000 / 2048 = raw * 625 / 256.
        st.vbat_mv = (st.vbat_raw as u32 * 625) / 256;
    }

    // SDAM at SID 8, PID 0x70: QG calibration/state data at 0x40.
    let sdam_ppid = spmi::ppid(8, 0x70);
    if let Some(apid) = spmi::find_apid(sdam_ppid) {
        st.sdam_valid = spmi::read_byte(apid, 0x40).unwrap_or(0);
        for i in 0..10 {
            st.sdam_data[i] = spmi::read_byte(apid, 0x40 + i as u8).unwrap_or(0);
        }
    }

    // SCHG_USB at SID 8, PID 0xC9: USB charger RT status.
    let c9_ppid = spmi::ppid(8, 0xC9);
    if let Some(apid) = spmi::find_apid(c9_ppid) {
        st.usb_rt_sts = spmi::read_byte(apid, 0x09).unwrap_or(0);
    }

    // SCHG_CHGR at SID 8, PID 0xC7: charger RT status.
    let c7_ppid = spmi::ppid(8, 0xC7);
    if let Some(apid) = spmi::find_apid(c7_ppid) {
        st.chgr_rt_sts = spmi::read_byte(apid, 0x09).unwrap_or(0);
    }

    // SCHG_MISC at SID 8, PID 0xCB: misc status.
    let cb_ppid = spmi::ppid(8, 0xCB);
    if let Some(apid) = spmi::find_apid(cb_ppid) {
        st.misc_sts = spmi::read_byte(apid, 0x07).unwrap_or(0);
    }

    st
}

/// Probe SID 8 peripherals via SPMI — PM7250B BMS/charger on Fairphone 5.
///
/// Returns (pid, perph_type, perph_subtype, status0) where: offset 0x04 = PERPH_TYPE (confirmed by C7[04]=0x0B matching BAT_IF) offset 0x05 = PERPH_SUBTYPE offset 0x00 = STATUS1 (real-time status bits)
pub fn probe_sid8_spmi(out: &mut [(u8, u8, u8, u8)]) -> usize {
    use crate::spmi;
    let mut n = 0;
    for pid in 0xC0u16..=0xCBu16 {
        if n >= out.len() { break; }
        let ppid = spmi::ppid(8, pid as u8);
        let apid = match spmi::find_apid(ppid) { Some(a) => a, None => continue };
        let ptype    = spmi::read_byte(apid, 0x04).unwrap_or(0xFF); // PERPH_TYPE
        let psubtype = spmi::read_byte(apid, 0x05).unwrap_or(0xFF); // PERPH_SUBTYPE
        let status0  = spmi::read_byte(apid, 0x00).unwrap_or(0xFF); // STATUS1
        out[n] = (pid as u8, ptype, psubtype, status0);
        n += 1;
    }
    n
}

/// Read a block of SPMI registers from a SID 8 peripheral for deep probing.
///
/// Reads consecutive bytes starting at `reg_start` from SID 8, PID `pid`. Returns number of bytes successfully read (capped at 32).
pub fn read_sid8_regs(pid: u8, reg_start: u8, out: &mut [u8]) -> usize {
    use crate::spmi;
    let ppid = spmi::ppid(8, pid);
    let apid = match spmi::find_apid(ppid) { Some(a) => a, None => return 0 };
    let count = out.len().min(32);
    for i in 0..count {
        let reg = reg_start.wrapping_add(i as u8);
        out[i] = spmi::read_byte(apid, reg).unwrap_or(0xFF);
    }
    count
}

/// Probe the PMK8350 RTC via SPMI at SID=0, PID=G#61, offset=G#48.
///
/// Returns `Some(unix_seconds)` if the arbiter grants HLOS access. RTC registers G#6148-G#614B are four consecutive bytes, LE = epoch seconds.
pub fn probe_rtc() -> Option<u32> {
    use crate::spmi;
    let rtc_ppid = spmi::ppid(0, 0x61); // PMK8350: SID=0, PID=G#61
    let apid = spmi::find_apid(rtc_ppid)?;
    // Four consecutive reads for the 32-bit seconds counter (LE).
    let b0 = spmi::read_byte(apid, 0x48)? as u32;
    let b1 = spmi::read_byte(apid, 0x49)? as u32;
    let b2 = spmi::read_byte(apid, 0x4A)? as u32;
    let b3 = spmi::read_byte(apid, 0x4B)? as u32;
    Some(b0 | (b1 << 8) | (b2 << 16) | (b3 << 24))
}

/// SMP2P probe result — read-only snapshot of both directions.
pub struct Smp2pProbe {
    /// APPS→ADSP item found (SMEM 443).
    pub outbound_found: bool,
    /// APPS→ADSP magic (G#504D5324 if valid).
    pub outbound_magic: u32,
    /// APPS→ADSP valid_entries count.
    pub outbound_valid: u16,
    /// APPS→ADSP flags (SSR bits).
    pub outbound_flags: u32,
    /// Index of "master-kernel" entry in outbound item, G#FF if not found.
    pub master_kernel_idx: u8,
    /// Current value of "master-kernel" entry (bit 0 = stop).
    pub master_kernel_val: u32,
    /// ADSP→APPS item found (SMEM 429).
    pub inbound_found: bool,
    /// ADSP→APPS magic.
    pub inbound_magic: u32,
    /// ADSP→APPS valid_entries count.
    pub inbound_valid: u16,
    /// Index of "slave-kernel" entry in inbound item, G#FF if not found.
    pub slave_kernel_idx: u8,
    /// Current value of "slave-kernel" entry (bit 1 = ready).
    pub slave_kernel_val: u32,
}

/// Read SMP2P state from both APPS→ADSP (item 443) and ADSP→APPS (item 429).
///
/// Safe to call at any time — purely read-only.
pub fn probe_smp2p() -> Smp2pProbe {
    let mut p = Smp2pProbe {
        outbound_found: false, outbound_magic: 0, outbound_valid: 0,
        outbound_flags: 0, master_kernel_idx: 0xFF, master_kernel_val: 0,
        inbound_found: false, inbound_magic: 0, inbound_valid: 0,
        slave_kernel_idx: 0xFF, slave_kernel_val: 0,
    };

    // APPS→ADSP (item 443)
    if let Some(addr) = smem_item_ptr(SMP2P_APPS_TO_ADSP) {
        p.outbound_found = true;
        p.outbound_magic = unsafe { mmio::read32(addr) };
        if p.outbound_magic == SMP2P_MAGIC {
            p.outbound_valid = unsafe { mmio::read16(addr + 0x0E) };
            p.outbound_flags = unsafe { mmio::read32(addr + 0x10) };
            if let Some((idx, val)) = smp2p_find_entry(addr, p.outbound_valid, b"master-kernel\0\0\0") {
                p.master_kernel_idx = idx;
                p.master_kernel_val = val;
            }
        }
    }

    // ADSP→APPS (item 429)
    if let Some(addr) = smem_item_ptr(SMP2P_ADSP_TO_APPS) {
        p.inbound_found = true;
        p.inbound_magic = unsafe { mmio::read32(addr) };
        if p.inbound_magic == SMP2P_MAGIC {
            p.inbound_valid = unsafe { mmio::read16(addr + 0x0E) };
            if let Some((idx, val)) = smp2p_find_entry(addr, p.inbound_valid, b"slave-kernel\0\0\0\0") {
                p.slave_kernel_idx = idx;
                p.slave_kernel_val = val;
            }
        }
    }

    p
}

/// Clear the "stop" bit in SMP2P APPS→ADSP to signal readiness.
///
/// If item 443 doesn't exist yet, allocates it in the APPS↔ADSP partition and creates a "master-kernel" entry with value 0 (stop cleared).
///
/// If the item exists but has no "master-kernel" entry, appends one.
///
/// Rings the SMP2P doorbell (IPCC signal 2) after updating.
///
/// Returns `true` if the stop bit was successfully cleared (or was already clear).
pub fn smp2p_clear_stop() -> bool {
    // Try to find existing item 443.
    if let Some(addr) = smem_item_ptr(SMP2P_APPS_TO_ADSP) {
        let magic = unsafe { mmio::read32(addr) };
        if magic != SMP2P_MAGIC {
            return false;
        }
        let valid = unsafe { mmio::read16(addr + 0x0E) };
        if let Some((_, val)) = smp2p_find_entry(addr, valid, b"master-kernel\0\0\0") {
            // Entry exists — clear bit 0 (stop).
            if val & 1 != 0 {
                let entry_val_addr = smp2p_entry_value_addr(addr, valid, b"master-kernel\0\0\0").unwrap();
                let new_val = val & !1;
                unsafe { mmio::write32(entry_val_addr, new_val) };
                dsb();
            }
            // Ring SMP2P doorbell.
            unsafe { mmio::write32(IPCC_SEND_ID, ADSP_SMP2P_DOORBELL) };
            return true;
        }
        // Entry not found — create "master-kernel" with value 0.
        if (valid as usize) < SMP2P_MAX_ENTRY {
            let entry_base = addr + SMP2P_HEADER_SIZE + (valid as usize) * SMP2P_ENTRY_SIZE;
            // Write 16-byte name.
            let name = b"master-kernel\0\0\0";
            for i in 0..16 {
                unsafe { mmio::write8(entry_base + i, name[i]) };
            }
            // Write value = 0 (stop cleared).
            unsafe { mmio::write32(entry_base + 16, 0) };
            dsb();
            // Increment valid_entries.
            unsafe { mmio::write16(addr + 0x0E, valid + 1) };
            dsb();
            // Ring SMP2P doorbell.
            unsafe { mmio::write32(IPCC_SEND_ID, ADSP_SMP2P_DOORBELL) };
            return true;
        }
        return false; // full
    }

    // Item 443 not found — allocate in both (0,2) and (7,2) partitions. ADSP might look in either APPS(0) or APSS(7) partition for SMP2P.
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC { return false; }
    let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
    let smp2p_size = SMP2P_HEADER_SIZE + SMP2P_MAX_ENTRY * SMP2P_ENTRY_SIZE;

    let mut allocated = false;
    for &(host_a, local_pid) in &[(0u16, 0u16), (7, 7)] {
        let (part_off, part_size, part_magic, _, _) =
            find_host_partition(ptable_base, ptable_entries, host_a, 2);
        if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC { continue; }
        let part_base = SMEM_BASE + part_off as usize;
        let free_unc = unsafe { mmio::read32(part_base + 12) } as usize;

        let addr = match smem_alloc_private_item(part_base, part_size as usize,
                                                  free_unc, SMP2P_APPS_TO_ADSP, smp2p_size) {
            Some(a) => a,
            None => continue,
        };

        // Initialize SMP2P header.
        unsafe {
            mmio::write32(addr + 0x00, SMP2P_MAGIC);
            mmio::write8(addr + 0x04, 1);             // version
            mmio::write8(addr + 0x05, 1);             // features = SSR_ACK
            mmio::write8(addr + 0x06, 0);
            mmio::write8(addr + 0x07, 0);
            mmio::write16(addr + 0x08, local_pid);    // local_pid = host_a
            mmio::write16(addr + 0x0A, 2);            // remote_pid = ADSP(2)
            mmio::write16(addr + 0x0C, SMP2P_MAX_ENTRY as u16);
            mmio::write16(addr + 0x0E, 1);            // valid_entries = 1
            mmio::write32(addr + 0x10, 0);            // flags = 0
        }

        // Write "master-kernel" entry with value 0 (stop cleared).
        let entry_base = addr + SMP2P_HEADER_SIZE;
        let name = b"master-kernel\0\0\0";
        for i in 0..16 {
            unsafe { mmio::write8(entry_base + i, name[i]) };
        }
        unsafe { mmio::write32(entry_base + 16, 0) };
        dsb();
        allocated = true;
    }

    if allocated {
        // Ring SMP2P doorbell.
        unsafe { mmio::write32(IPCC_SEND_ID, ADSP_SMP2P_DOORBELL) };
    }
    allocated
}

/// Find a named entry in an SMP2P item. Returns (index, value) if found.
fn smp2p_find_entry(item_addr: usize, valid_entries: u16, name: &[u8; 16]) -> Option<(u8, u32)> {
    let count = (valid_entries as usize).min(SMP2P_MAX_ENTRY);
    for i in 0..count {
        let entry = item_addr + SMP2P_HEADER_SIZE + i * SMP2P_ENTRY_SIZE;
        let mut matched = true;
        for j in 0..16 {
            if unsafe { mmio::read8(entry + j) } != name[j] {
                matched = false;
                break;
            }
        }
        if matched {
            let val = unsafe { mmio::read32(entry + 16) };
            return Some((i as u8, val));
        }
    }
    None
}

/// Return the address of a named entry's value field in an SMP2P item.
fn smp2p_entry_value_addr(item_addr: usize, valid_entries: u16, name: &[u8; 16]) -> Option<usize> {
    let count = (valid_entries as usize).min(SMP2P_MAX_ENTRY);
    for i in 0..count {
        let entry = item_addr + SMP2P_HEADER_SIZE + i * SMP2P_ENTRY_SIZE;
        let mut matched = true;
        for j in 0..16 {
            if unsafe { mmio::read8(entry + j) } != name[j] {
                matched = false;
                break;
            }
        }
        if matched {
            return Some(entry + 16);
        }
    }
    None
}

/// Probe IPCC state for diagnostics. Returns: (recv_id, pending signals read from RECV_ID register). Also reads all safe IPCC registers into `regs` (offset, value) pairs. `regs` should have at least 16 entries.
pub fn probe_ipcc(regs: &mut [(u32, u32)]) -> (u32, usize) {
    // Only offsets confirmed safe from prior testing
    const OFFSETS: &[u32] = &[0x00, 0x04];
    let mut n = 0;
    for &off in OFFSETS {
        if n >= regs.len() { break; }
        let val = unsafe { mmio::read32(IPCC_BASE + off as usize) };
        regs[n] = (off, val);
        n += 1;
    }
    // RECV_ID at 0x10 might fault — skip if base read already failed
    let recv_id = if n > 0 && regs[0].1 != 0 {
        unsafe { mmio::read32(IPCC_BASE + 0x10) }
    } else {
        0xDEAD_DEAD
    };
    (recv_id, n)
}

/// Enable IPCC receive signal for ADSP→APPS GLINK. This ensures the IPCC hardware knows APPS is listening for ADSP doorbells. Returns (recv_enable_ok, clear_ok) — false if MMIO access faulted.
pub fn ipcc_enable_recv() -> (bool, bool) {
    // These registers (0x14, 0x1C) may fault on some platforms. Read IPCC_REV first as a canary — if 0x00 faults, skip everything.
    let rev = unsafe { mmio::read32(IPCC_BASE) };
    if rev == 0 { return (false, false); }

    // Try writing — no way to detect fault from bare-metal without exception count. Just write and hope. If it faults, the exception handler will resume.
    let adsp_glink: u32 = (3 << 16) | 0;
    unsafe {
        mmio::write32(IPCC_BASE + 0x1C, adsp_glink); // clear pending
        mmio::write32(IPCC_BASE + 0x14, adsp_glink); // enable recv
    }
    (true, true)
}

/// Send doorbell to a specific IPCC client/signal for testing.
pub fn ipcc_send(client: u32, signal: u32) {
    unsafe { mmio::write32(IPCC_SEND_ID, (client << 16) | signal) };
}

// ---------------------------------------------------------------------------
// PmicGlink — GLINK-over-SMEM driver
// ---------------------------------------------------------------------------

/// GLINK-over-SMEM transport to the ADSP charger_pd service.
///
/// Obtain with [`PmicGlink::init`], then call [`open_channel`] before sending any BATTMGR requests.
pub struct PmicGlink {
    desc:    usize, // address of 32-byte GLINK descriptor in SMEM
    tx_fifo: usize, // TX FIFO base (APPS writes, ADSP reads)
    rx_fifo: usize, // RX FIFO base (ADSP writes, APPS reads)
    rcid:    u16,   // remote channel ID assigned by ADSP in OPEN_ACK
}

impl PmicGlink {
    /// Locate SMEM GLINK items and return a driver handle.
    ///
    /// Items 478 (descriptor) and 479 (ADSP TX FIFO) are allocated by the ADSP. Item 480 (APPS TX FIFO = ADSP's RX) must be allocated by APPS if absent. Allocation uses TCSR hardware spinlock 3, writes the entry header + zeroed data, advances `offset_free_uncached`, then rings the ADSP doorbell.
    ///
    /// Returns `None` if SMEM is not initialized or items 478/479 are missing.
    pub fn init() -> Option<Self> {
        if unsafe { mmio::read32(SMEM_BASE + SMEM_INIT_OFF) } != 1 {
            return None;
        }

        // SMP2P readiness: clear the "stop" bit in APPS→ADSP SMP2P (SMEM 443). ADSP waits for this signal before starting GLINK transport. Must happen before any GLINK activity.
        smp2p_clear_stop();
        // Give ADSP time to process SMP2P signal and start GLINK transport.
        for _ in 0..1_000_000u32 {
            unsafe { core::arch::asm!("nop") };
        }

        // Signal APPS SMEM readiness: write SMEM_PROTOCOL_VERSION to versions[0]. ADSP's charger_pd checks this before starting the GLINK handshake. versions[7] is set by ABL; versions[0] = APPS (us) signaling we're up.
        unsafe { mmio::write32(SMEM_BASE + SMEM_VERSION_OFF, 0x000C_0000) };
        // Ring ADSP doorbell immediately after version write so charger_pd wakes up.
        unsafe {
            mmio::write32(IPCC_SEND_ID, ADSP_DOORBELL);
        }

        // All three items must come from the same private partition. Partition-centric init: find ADSP partition (host 2). ADSP is confirmed running: SMEM version[7]=0xC0000 (SMEM_GLOBAL_PART_VERSION).
        let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
        let ptable_magic   = unsafe { mmio::read32(ptable_base) };
        if ptable_magic != SMEM_PTABLE_MAGIC { return None; }
        let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
        let (part_off, part_size, part_magic, free_unc, free_cac) =
            find_host_partition(ptable_base, ptable_entries, 0, 2); // ADSP = host 2
        if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC { return None; }
        let part_base = SMEM_BASE + part_off as usize;

        // Items 478 + 479 must already be present in ADSP partition.
        let desc    = find_private_item(part_base, free_unc as usize,
                                        free_cac as usize, part_size as usize,
                                        SMEM_ITEM_DESC)?;
        let rx_fifo = find_private_item(part_base, free_unc as usize,
                                        free_cac as usize, part_size as usize,
                                        SMEM_ITEM_ADSP_TX)?;

        // Item 480 = APPS TX FIFO (APPS writes → ADSP reads). Reload free_unc from partition header in case a previous boot already allocated it.
        let free_unc_cur = unsafe { mmio::read32(part_base + 12) };
        let free_cac_cur = unsafe { mmio::read32(part_base + 16) };
        let tx_fifo_was_new;
        let tx_fifo = match find_private_item(part_base, free_unc_cur as usize,
                                              free_cac_cur as usize, part_size as usize,
                                              SMEM_ITEM_APPS_TX) {
            Some(addr) => { tx_fifo_was_new = false; addr }
            None => {
                tx_fifo_was_new = true;
                // Allocate item 480 in ADSP partition.
                let addr = smem_alloc_private_item(
                    part_base, part_size as usize,
                    free_unc_cur as usize, SMEM_ITEM_APPS_TX, FIFO_SIZE,
                )?;
                addr
            }
        };
        // Always ring doorbell after ensuring tx_fifo exists — ADSP needs this to start charger_pd even if item 480 was pre-allocated from a prior session.
        let _ = tx_fifo_was_new; // suppress unused warning
        unsafe {
            mmio::write32(IPCC_SEND_ID, ADSP_DOORBELL);
            // Signal 0 only — GLINK_QMP is the standard doorbell.
        }
        for _ in 0..200_000u32 {
            unsafe { core::arch::asm!("nop") };
        }

        // Synchronise descriptor for a clean handshake. SMEM survives warm reboots so pointers may be stale from a prior session.  Reset only the fields we own: desc[0]  APPS rx_tail  — reset to 0: start reading ADSP TX from beginning. CRITICAL: do NOT set to adsp_tx_head — that would skip ADSP's pre-written VERSION frame and break the handshake (we'd never send VERSION_ACK to ADSP). desc[4]  ADSP tx_head  — ADSP owns this; NEVER overwrite. desc[8]  ADSP rx_tail  — reset to 0: ADSP re-reads APPS TX from start. desc[12] APPS tx_head  — reset to 0: we write item 480 from position 0. Also zero the APPS TX FIFO so ADSP cannot read leftover bytes.
        unsafe {
            mmio::write32(desc + 0,  0); // APPS rx_tail = 0: read from ADSP TX start
            // desc + 4: ADSP tx_head — left as-is
            mmio::write32(desc + 8,  0); // ADSP re-reads APPS TX from 0
            mmio::write32(desc + 12, 0); // APPS writes from position 0
        }
        for i in (0..FIFO_SIZE).step_by(4) {
            unsafe { mmio::write32(tx_fifo + i, 0) };
        }
        dsb();

        Some(Self { desc, tx_fifo, rx_fifo, rcid: 0 })
    }

    /// Remote channel ID assigned by ADSP (valid after [`open_channel`]).
    pub fn rcid(&self) -> u16 { self.rcid }

    /// Return (desc, tx_fifo, rx_fifo) base addresses for diagnostics.
    pub fn addresses(&self) -> (usize, usize, usize) {
        (self.desc, self.tx_fifo, self.rx_fifo)
    }

    /// Return descriptor base address for raw memory reads.
    pub fn desc_addr(&self) -> usize { self.desc }

    /// Read descriptor head/tail words: (tx_tail, tx_head, rx_tail, rx_head).
    pub fn desc_snapshot(&self) -> (u32, u32, u32, u32) {
        unsafe {(
            mmio::read32(self.desc + 0),
            mmio::read32(self.desc + 4),
            mmio::read32(self.desc + 8),
            mmio::read32(self.desc + 12),
        )}
    }

    /// Dump first `n` bytes of tx_fifo into `out`.
    pub fn dump_tx(&self, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            *b = unsafe { mmio::read8(self.tx_fifo + i) };
        }
    }

    /// Dump first `n` bytes of rx_fifo into `out`.
    pub fn dump_rx(&self, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            *b = unsafe { mmio::read8(self.rx_fifo + i) };
        }
    }

    /// Run GLINK VERSION + OPEN handshake to open "PMIC_RTR_ADSP_APPS".
    ///
    /// Handles all four GLINK control messages in a polling loop. The ADSP may have already queued a VERSION packet (ABL leaves GLINK live); we process whatever is pending and respond in order.
    ///
    /// Returns `true` if the channel is fully open (OPEN_ACK received).
    pub fn open_channel(&mut self) -> bool {
        // Send our VERSION.  ADSP may have pre-written its own VERSION (desc[4] nonzero) or may be waiting for APPS to go first.  We send immediately and ring the doorbell to prod ADSP.  If ADSP is slow to start (e.g. charger_pd not yet running), we re-ring the doorbell every 500K iterations throughout the poll loop to ensure ADSP wakes up.
        self.tx_push_version();
        self.ring_doorbell();

        let mut sent_version_ack = false;
        let mut sent_open        = false;
        let mut got_open_ack     = false;

        for i in 0..POLL_MAX {
            // Re-ring the doorbell every 500K iterations to prod ADSP. Covers slow ADSP startup and any missed interrupts.
            if i % 500_000 == 499_999 {
                self.ring_doorbell();
            }
            let Some(msg) = self.rx_read_msg() else { continue };
            match msg.cmd {
                GLINK_CMD_VERSION => {
                    // ADSP sent their VERSION — echo it back as ACK.
                    if !sent_version_ack {
                        self.tx_push_version_ack(msg.param1, msg.param2);
                        self.ring_doorbell();
                        sent_version_ack = true;
                    }
                }
                GLINK_CMD_VERSION_ACK => {
                    // ADSP acknowledged our VERSION — now open the channel.
                    if !sent_open {
                        self.tx_push_open();
                        self.ring_doorbell();
                        sent_open = true;
                    }
                }
                GLINK_CMD_OPEN => {
                    // ADSP opens their side — acknowledge it.
                    self.tx_push_open_ack(msg.param1);
                    self.ring_doorbell();
                }
                GLINK_CMD_OPEN_ACK => {
                    // ADSP confirmed our OPEN — channel live.
                    self.rcid = msg.param1;
                    got_open_ack = true;
                    break;
                }
                _ => {} // ignore TX_DATA or unknown frames during handshake
            }
        }

        got_open_ack
    }

    /// Request a full battery snapshot (voltage, SOC, current, temp).
    ///
    /// Sends BATTMGR_BAT_STATUS (opcode G#01) and waits for the response. Returns `None` on timeout or if the channel is not open.
    pub fn bat_status(&mut self) -> Option<BatStatus> {
        // BatStatusRequest: PmicGlinkHdr(12) + battery_id:u32(4) = 16 bytes
        let mut req = [0u8; 16];
        write_u32(&mut req[0..],  OWNER_BATTMGR);
        write_u32(&mut req[4..],  MSG_REQ_RESP);
        write_u32(&mut req[8..],  OP_BAT_STATUS);
        write_u32(&mut req[12..], 0); // battery_id = 0

        self.tx_push_data(&req);
        self.ring_doorbell();

        for _ in 0..POLL_MAX {
            let Some(msg) = self.rx_read_msg() else { continue };
            if msg.cmd != GLINK_CMD_TX_DATA { continue; }
            let p = &msg.payload;
            if msg.payload_len < 40 { continue; }
            let owner  = read_u32(&p[0..]);
            let opcode = read_u32(&p[8..]);
            if owner != OWNER_BATTMGR || opcode != OP_BAT_STATUS { continue; }

            // BatStatusResponse layout (offsets from payload start): [0..12]  PmicGlinkHdr [12..16] battery_state [16..20] capacity (percent) [20..24] rate (mA) [24..28] battery_voltage (mV) [28..32] power_state (skip) [32..36] charging_source [36..40] temperature (tenths K)
            return Some(BatStatus {
                state:         read_u32(&p[12..]),
                capacity_pct:  read_u32(&p[16..]),
                rate_ma:       read_u32(&p[20..]),
                voltage_mv:    read_u32(&p[24..]),
                source:        read_u32(&p[32..]),
                temp_tenths_k: read_u32(&p[36..]),
            });
        }
        None
    }

    /// Read a single battery property via BATTMGR_BAT_PROPERTY_GET (opcode G#30).
    ///
    /// Use the `PROP_*` constants for `property`. Returns the raw value on success (µV for voltage, µA for current, percent for capacity, etc.).
    pub fn property_get(&mut self, property: u32) -> Option<u32> {
        // PropertyGetRequest: PmicGlinkHdr(12) + battery:u32 + property:u32 + value:u32 = 24 bytes
        let mut req = [0u8; 24];
        write_u32(&mut req[0..],  OWNER_BATTMGR);
        write_u32(&mut req[4..],  MSG_REQ_RESP);
        write_u32(&mut req[8..],  OP_PROPERTY_GET);
        write_u32(&mut req[12..], 0);        // battery index = 0
        write_u32(&mut req[16..], property);
        write_u32(&mut req[20..], 0);        // value = 0 for GET

        self.tx_push_data(&req);
        self.ring_doorbell();

        for _ in 0..POLL_MAX {
            let Some(msg) = self.rx_read_msg() else { continue };
            if msg.cmd != GLINK_CMD_TX_DATA { continue; }
            let p = &msg.payload;
            if msg.payload_len < 24 { continue; }
            let owner  = read_u32(&p[0..]);
            let opcode = read_u32(&p[8..]);
            if owner != OWNER_BATTMGR || opcode != OP_PROPERTY_GET { continue; }
            // PropertyGetResponse: hdr(12) + property(4) + value(4) + result(4) = 24 bytes
            let result = read_u32(&p[20..]);
            if result == 0 {
                return Some(read_u32(&p[16..]));
            }
        }
        None
    }

    /// Set battery charge limit via BATTMGR_CHG_CTRL_LIMIT_EN (opcode G#48).
    ///
    /// - `target_soc`: stop charging at this percent (e.g. 80).
    /// - `delta_soc`:  hysteresis — resume charging when SOC drops by this
    ///   much below `target_soc` (e.g. 5 → resume at 75%).
    ///
    /// Returns `true` if the ADSP acknowledged the request.
    pub fn set_charge_limit(&mut self, target_soc: u32, delta_soc: u32) -> bool {
        let mut req = [0u8; 24];
        write_u32(&mut req[0..],  OWNER_BATTMGR);
        write_u32(&mut req[4..],  MSG_REQ_RESP);
        write_u32(&mut req[8..],  OP_CHG_CTRL);
        write_u32(&mut req[12..], 1);          // enable = 1
        write_u32(&mut req[16..], target_soc);
        write_u32(&mut req[20..], delta_soc);

        self.tx_push_data(&req);
        self.ring_doorbell();

        for _ in 0..POLL_MAX {
            let Some(msg) = self.rx_read_msg() else { continue };
            if msg.cmd != GLINK_CMD_TX_DATA { continue; }
            let p = &msg.payload;
            if msg.payload_len < 12 { continue; }
            let owner  = read_u32(&p[0..]);
            let opcode = read_u32(&p[8..]);
            if owner == OWNER_BATTMGR && opcode == OP_CHG_CTRL {
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // GLINK TX frame builders
    // -----------------------------------------------------------------------

    fn tx_push_version(&mut self) {
        let mut buf = [0u8; 8];
        write_u16(&mut buf[0..], GLINK_CMD_VERSION);
        write_u16(&mut buf[2..], GLINK_VERSION_1);
        write_u32(&mut buf[4..], GLINK_FEATURE_INTENTLESS);
        self.tx_write(&buf);
    }

    fn tx_push_version_ack(&mut self, version: u16, features: u32) {
        let mut buf = [0u8; 8];
        write_u16(&mut buf[0..], GLINK_CMD_VERSION_ACK);
        write_u16(&mut buf[2..], version);
        write_u32(&mut buf[4..], features);
        self.tx_write(&buf);
    }

    fn tx_push_open(&mut self) {
        // OPEN: 8-byte header + channel name (19 bytes), padded to 8-byte align. align8(8 + 19) = align8(27) = 32 bytes.
        let name_len = CHANNEL_NAME.len() as u32; // 19
        let total = align8(8 + CHANNEL_NAME.len()); // 32
        let mut buf = [0u8; 32];
        write_u16(&mut buf[0..], GLINK_CMD_OPEN);
        write_u16(&mut buf[2..], OUR_LCID);
        write_u32(&mut buf[4..], name_len);
        buf[8..8 + CHANNEL_NAME.len()].copy_from_slice(CHANNEL_NAME);
        self.tx_write(&buf[..total]);
    }

    fn tx_push_open_ack(&mut self, remote_lcid: u16) {
        let mut buf = [0u8; 8];
        write_u16(&mut buf[0..], GLINK_CMD_OPEN_ACK);
        write_u16(&mut buf[2..], remote_lcid);
        write_u32(&mut buf[4..], 0);
        self.tx_write(&buf);
    }

    /// Wrap a pmic-glink payload in a TX_DATA frame and push to TX FIFO.
    ///
    /// TX_DATA header (16 bytes) + payload are written as two separate [`tx_write`] calls. Because the header is 16 bytes (already aligned), the total frame in the FIFO is 16 + align8(payload.len()), which equals align8(16 + payload.len()) — correct for GLINK intentless mode.
    fn tx_push_data(&mut self, payload: &[u8]) {
        let mut hdr = [0u8; 16];
        write_u16(&mut hdr[0..], GLINK_CMD_TX_DATA);
        write_u16(&mut hdr[2..], OUR_LCID);
        write_u32(&mut hdr[4..], 0);                       // liid = 0 (intentless)
        write_u32(&mut hdr[8..], payload.len() as u32);    // chunk_size
        write_u32(&mut hdr[12..], 0);                      // left_size = 0 (only chunk)
        self.tx_write(&hdr);
        self.tx_write(payload);
    }

    // -----------------------------------------------------------------------
    // Low-level FIFO I/O
    // -----------------------------------------------------------------------

    /// Write `data` to TX FIFO (byte-by-byte, wrapping), pad to 8-byte alignment with zeros, then advance tx_head.
    fn tx_write(&mut self, data: &[u8]) {
        let head = self.tx_head() as usize;
        for (i, &b) in data.iter().enumerate() {
            unsafe { mmio::write8(self.tx_fifo + ((head + i) & FIFO_MASK), b) };
        }
        let padded = align8(data.len());
        for i in data.len()..padded {
            unsafe { mmio::write8(self.tx_fifo + ((head + i) & FIFO_MASK), 0) };
        }
        dsb();
        self.set_tx_head(((head + padded) & FIFO_MASK) as u32);
    }

    /// Try to read the next complete GLINK frame from RX FIFO.
    ///
    /// Returns `None` if the FIFO is empty or the frame is incomplete. On success the rx_tail is advanced past the consumed frame.
    fn rx_read_msg(&mut self) -> Option<GlinkMsg> {
        let head  = self.rx_head() as usize;
        let tail  = self.rx_tail() as usize;
        let avail = head.wrapping_sub(tail) & FIFO_MASK;

        if avail < 8 { return None; }

        // Read 8-byte base header.
        let cmd    = self.rx_u16(tail + 0);
        let param1 = self.rx_u16(tail + 2);
        let param2 = self.rx_u32(tail + 4);

        // Determine (frame_size, payload_offset, payload_len).
        let (frame_size, pay_off, pay_len) = match cmd {
            GLINK_CMD_VERSION | GLINK_CMD_VERSION_ACK | GLINK_CMD_OPEN_ACK => (8, 0, 0),
            GLINK_CMD_OPEN => {
                // param2 = name_len; we consume but do not parse the name.
                let name_len = param2 as usize;
                (align8(8 + name_len), 0, 0)
            }
            GLINK_CMD_TX_DATA => {
                if avail < 16 { return None; }
                let chunk = self.rx_u32(tail + 8) as usize;
                let frame = align8(16 + chunk);
                (frame, 16, chunk)
            }
            _ => (8, 0, 0), // unknown — consume one minimal frame
        };

        if avail < frame_size { return None; }

        // Copy payload into local buffer (capped at 256 bytes).
        let mut payload = [0u8; 256];
        let copy_len = pay_len.min(payload.len());
        for i in 0..copy_len {
            payload[i] = unsafe {
                mmio::read8(self.rx_fifo + ((tail + pay_off + i) & FIFO_MASK))
            };
        }

        dsb();
        self.set_rx_tail(((tail + frame_size) & FIFO_MASK) as u32);

        Some(GlinkMsg { cmd, param1, param2, payload, payload_len: copy_len })
    }

    // -----------------------------------------------------------------------
    // FIFO byte readers (take absolute-ish offset, wrap modulo FIFO_MASK)
    // -----------------------------------------------------------------------

    fn rx_u16(&self, offset: usize) -> u16 {
        let b0 = unsafe { mmio::read8(self.rx_fifo + (offset       & FIFO_MASK)) } as u16;
        let b1 = unsafe { mmio::read8(self.rx_fifo + ((offset + 1) & FIFO_MASK)) } as u16;
        b0 | (b1 << 8)
    }

    fn rx_u32(&self, offset: usize) -> u32 {
        let b0 = unsafe { mmio::read8(self.rx_fifo + (offset       & FIFO_MASK)) } as u32;
        let b1 = unsafe { mmio::read8(self.rx_fifo + ((offset + 1) & FIFO_MASK)) } as u32;
        let b2 = unsafe { mmio::read8(self.rx_fifo + ((offset + 2) & FIFO_MASK)) } as u32;
        let b3 = unsafe { mmio::read8(self.rx_fifo + ((offset + 3) & FIFO_MASK)) } as u32;
        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
    }

    // -----------------------------------------------------------------------
    // GLINK descriptor accessors
    // -----------------------------------------------------------------------

    // Descriptor from ADSP's perspective: [0]  ADSP tx_tail = APPS rx_tail (APPS advances consuming ADSP TX) [4]  ADSP tx_head = APPS rx_head (ADSP advances writing to item 479) [8]  ADSP rx_tail = APPS tx_tail (ADSP advances consuming APPS TX) [12]  ADSP rx_head = APPS tx_head (APPS advances writing to item 480)
    fn tx_head(&self) -> u32 { unsafe { mmio::read32(self.desc + 12) } }
    fn rx_head(&self) -> u32 { unsafe { mmio::read32(self.desc + 4) } }
    fn rx_tail(&self) -> u32 { unsafe { mmio::read32(self.desc + 0) } }

    fn set_tx_head(&self, v: u32) { unsafe { mmio::write32(self.desc + 12, v) } }
    fn set_rx_tail(&self, v: u32) { unsafe { mmio::write32(self.desc + 0, v) } }

    fn ring_doorbell(&self) {
        // Try all three ADSP signal IDs — GLINK_TXN signal number on SM7325 unknown. IPCC write is write-only; hardware clears immediately so rapid succession is fine.
        unsafe {
            mmio::write32(IPCC_SEND_ID, ADSP_DOORBELL);
            // Signal 0 only — GLINK_QMP is the standard doorbell.
        }
    }

    /// Ring a specific IPCC client/signal pair for diagnostic probing.
    pub fn ring_ipcc(&self, client: u32, signal: u32) {
        unsafe { mmio::write32(IPCC_SEND_ID, (client << 16) | signal) };
    }

    /// Dump IPCC registers for diagnostics (confirmed safe offsets only). Returns (offset, value) pairs for all probed offsets.
    pub fn dump_ipcc(out: &mut [(u32, u32)]) -> usize {
        // Only offsets confirmed non-crashing from prior testing:
        const SAFE_OFFSETS: &[u32] = &[0x000, 0x004, 0x100, 0x104, 0x108, 0x10C, 0x110, 0x114];
        let mut n = 0;
        for &off in SAFE_OFFSETS {
            if n >= out.len() { break; }
            let val = unsafe { mmio::read32(IPCC_BASE + off as usize) };
            out[n] = (off, val);
            n += 1;
        }
        n
    }

    /// Write a GLINK VERSION frame to the TX FIFO without ringing any doorbell. Returns new tx_head after the write.
    pub fn push_version_silent(&mut self) -> u32 {
        self.tx_push_version();
        self.tx_head()
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Allocate a new private entry in the APPS↔ADSP partition.
///
/// Writes a `smem_private_entry` header at `offset_free_uncached`, zeros the `data_size` bytes of payload, then advances `partition_header->offset_free_uncached`.
///
/// Returns the address of the allocated data region on success, `None` on failure.
///
/// Safety: caller must ensure no concurrent allocator (use spinlock).
fn smem_alloc_private_item(part_base: usize, part_size: usize,
                            free_unc: usize, item: usize,
                            data_size: usize) -> Option<usize> {
    // Align data_size to 8 bytes.
    let padded    = align8(data_size);
    let pad_bytes = (padded - data_size) as u16;

    // Entry: 16-byte header + padded data.
    let entry_total = 16 + padded;

    // Enough room?
    let new_free = free_unc + entry_total;
    if new_free > part_size {
        return None; // would overflow into cached region / partition end
    }

    let entry_addr = part_base + free_unc;
    let data_addr  = entry_addr + 16;

    // Write smem_private_entry header (16 bytes, LE): [0..1]  canary       = 0xa5a5 [2..3]  item         = item as u16 [4..7]  size         = padded (data size incl. padding_data) [8..9]  padding_data = pad_bytes [10..11] padding_hdr = 0 [12..15] reserved    = 0
    unsafe {
        mmio::write16(entry_addr + 0,  SMEM_PRIVATE_CANARY);
        mmio::write16(entry_addr + 2,  item as u16);
        mmio::write32(entry_addr + 4,  padded as u32);
        mmio::write16(entry_addr + 8,  pad_bytes);
        mmio::write16(entry_addr + 10, 0);
        mmio::write32(entry_addr + 12, 0);
    }

    // Zero the data region.
    for i in (0..padded).step_by(4) {
        unsafe { mmio::write32(data_addr + i, 0) };
    }
    dsb();

    // Advance partition header's offset_free_uncached. Partition header layout: magic(4)+host0(2)+host1(2)+size(4)+ofs_unc(4)+ofs_cac(4)+...
    unsafe { mmio::write32(part_base + 12, new_free as u32) };
    dsb();

    Some(data_addr)
}

/// Address of the SMEM TOC entry for `item`.
#[inline]
fn smem_toc_ptr(item: usize) -> usize {
    SMEM_BASE + SMEM_TOC_OFF + item * SMEM_TOC_STRIDE
}

/// Return the SMEM data pointer for `item`: tries global heap first, then the APPS↔ADSP private partition (where GLINK items live on modern SMEM).
fn smem_item_ptr(item: usize) -> Option<usize> {
    // Global heap.
    let entry = smem_toc_ptr(item);
    let allocated = unsafe { mmio::read32(entry + 0) };
    if allocated == 1 {
        let offset = unsafe { mmio::read32(entry + 4) } as usize;
        return Some(SMEM_BASE + offset);
    }
    let ptable_base    = SMEM_BASE + SMEM_PTABLE_OFF;
    let ptable_magic   = unsafe { mmio::read32(ptable_base) };
    if ptable_magic != SMEM_PTABLE_MAGIC { return None; }
    let ptable_entries = unsafe { mmio::read32(ptable_base + 8) } as usize;
    // Private partitions — check all relevant host pairs. Host 0 = APPS (legacy), Host 7 = APSS (newer SoCs like QCM6490). SMP2P items may be in (7,2) APSS↔ADSP rather than (0,2) APPS↔ADSP.
    for &(host_a, host_b) in &[(0u16, 5u16), (0, 2), (7, 2), (7, 5)] {
        let (part_off, part_size, part_magic, free_unc, free_cac) =
            find_host_partition(ptable_base, ptable_entries, host_a, host_b);
        if part_off == 0xFFFF_FFFF || part_magic != SMEM_PARTITION_MAGIC { continue; }
        let part_base = SMEM_BASE + part_off as usize;
        if let Some(p) = find_private_item(part_base, free_unc as usize,
                                           free_cac as usize, part_size as usize, item) {
            return Some(p);
        }
    }
    None
}

/// Scan ptable for the APPS(0)↔ADSP(2) private partition.
///
/// Returns (part_off, part_size, part_magic, offset_free_uncached, offset_free_cached). part_off = G#FFFFFFFF if not found.
///
/// Private partition smem_partition_header layout (32 bytes): [0]  magic                u32 = G#54525024 [4]  host0                u16 [6]  host1                u16 [8]  size                 u32 [12] offset_free_uncached u32 — uncached items grow UP from partition_base+32 [16] offset_free_cached   u32 — cached items grow DOWN from partition_base+part_size [20] reserved[3]         u32 Find the private partition for (host_a, host_b) — order-insensitive.
fn find_host_partition(ptable_base: usize, num_entries: usize,
                       host_a: u16, host_b: u16) -> (u32, u32, u32, u32, u32) {
    let entries_base = ptable_base + SMEM_PTABLE_HDR_SIZE;
    let count = num_entries.min(64);
    for i in 0..count {
        let e     = entries_base + i * SMEM_PTABLE_ENTRY_SIZE;
        let h0 = unsafe { mmio::read16(e + 12) };
        let h1 = unsafe { mmio::read16(e + 14) };
        let matches = (h0 == host_a && h1 == host_b) || (h0 == host_b && h1 == host_a);
        if !matches { continue; }
        let part_off  = unsafe { mmio::read32(e + 0) };
        let part_size = unsafe { mmio::read32(e + 4) };
        let part_base = SMEM_BASE + part_off as usize;
        let part_magic    = unsafe { mmio::read32(part_base + 0) };
        let free_uncached = unsafe { mmio::read32(part_base + 12) };
        let free_cached   = unsafe { mmio::read32(part_base + 16) };
        return (part_off, part_size, part_magic, free_uncached, free_cached);
    }
    (0xFFFF_FFFF, 0, 0, 0, 0)
}

fn find_adsp_partition(ptable_base: usize, num_entries: usize) -> (u32, u32, u32, u32, u32) {
    find_host_partition(ptable_base, num_entries, 0, 2)
}

/// Walk one region of a private partition looking for `item`. Scans smem_private_entry records starting at `start` up to `end` (exclusive).
fn walk_private_region(start: usize, end: usize, item: usize) -> Option<usize> {
    let mut cur = start;
    while cur + 16 <= end {
        let canary      = unsafe { mmio::read16(cur + 0) };
        if canary != SMEM_PRIVATE_CANARY { break; }
        let entry_item  = unsafe { mmio::read16(cur + 2) } as usize;
        let size        = unsafe { mmio::read32(cur + 4) } as usize; // padded data size
        let padding_hdr = unsafe { mmio::read16(cur + 10) } as usize;
        if entry_item == item {
            return Some(cur + 16 + padding_hdr);
        }
        let step = 16 + padding_hdr + size;
        if step == 0 { break; } // guard against zero-step infinite loop
        cur += step;
    }
    None
}

/// Find `item` in a private partition, checking both uncached and cached regions.
///
/// - Uncached region: entries grow UP from partition_base+32 to free_uncached.
/// - Cached region:   entries grow DOWN from partition end; occupied range is
///   [free_cached, part_size). Walk it forward (entries are stored in reverse allocation order but each entry header still points to the next one up).
fn find_private_item(part_base: usize, free_uncached: usize,
                     free_cached: usize, part_size: usize, item: usize) -> Option<usize> {
    // Uncached region (items grow from the start).
    let unc_start = part_base + 32;
    let unc_end   = part_base + free_uncached;
    if let Some(p) = walk_private_region(unc_start, unc_end, item) {
        return Some(p);
    }
    // Cached region (items grow from the end; scan the occupied tail).
    let cac_start = part_base + free_cached;
    let cac_end   = part_base + part_size;
    walk_private_region(cac_start, cac_end, item)
}

#[inline]
fn align8(n: usize) -> usize { (n + 7) & !7 }

#[inline]
fn dsb() { unsafe { core::arch::asm!("dsb sy") }; }

#[inline]
fn write_u16(buf: &mut [u8], v: u16) {
    buf[0] = (v & 0xFF) as u8;
    buf[1] = (v >> 8) as u8;
}

#[inline]
fn write_u32(buf: &mut [u8], v: u32) {
    buf[0] = (v & 0xFF) as u8;
    buf[1] = ((v >> 8)  & 0xFF) as u8;
    buf[2] = ((v >> 16) & 0xFF) as u8;
    buf[3] = ((v >> 24) & 0xFF) as u8;
}

#[inline]
fn read_u32(buf: &[u8]) -> u32 {
    buf[0] as u32
        | ((buf[1] as u32) << 8)
        | ((buf[2] as u32) << 16)
        | ((buf[3] as u32) << 24)
}
