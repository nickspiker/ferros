//! Interim husky device *ira* — the permanent device root, from the Tensor G3 chip-ID.
//!
//! The *ira* is criterion-0 (see VAULT-KEY.md): permanent, survives factory reset, unchangeable by anyone.
//! On husky the interim source is the gs-chipid block — a die-unique 64-bit serial plus product/lot and silicon revision — read straight from MMIO at EL2: no Linux, no Titan, no keystore stack.
//! It is vendor-known, not secret; secrecy is the sovereignty gap PIPE closes. Confidentiality here is structural (opaque to Android/Google's stack, keyed addressing, no cross-device convergence), and anti-theft is enforced by the fleet registry, not by this value.
//!
//! Layer split: this module only reads husky's hardware identity and names the KDF contexts; the derivation itself is `ferros_vault::anchor::AnchorKey` (the crypto authority) so nothing is hand-rolled here.

use ferros_vault::anchor::AnchorKey;

/// gs-chipid MMIO base on Tensor G3 (zuma.dtsi `chipid@10000000`, size G#D000).
/// Reachable MMU-off at EL2, same as the UFS/S2MPU blocks the shim path already pokes.
const CHIPID_BASE: usize = 0x1000_0000;

/// product_id — SoC product code; low 21 bits carry the lot id (gs-chipid.c `LOTID_MASK`).
const OFF_PRODUCT_ID: usize = 0x00;
/// die-unique serial, low 32 bits (`unique_id_reg`).
const OFF_UNIQUE_ID0: usize = 0x04;
/// die-unique serial, high 32 bits (`unique_id_reg + 4`).
const OFF_UNIQUE_ID1: usize = 0x08;
/// silicon revision (`rev_reg`).
const OFF_REVISION: usize = 0x10;

/// AP hardware-tuning block — 32 bytes of per-*device* fuse trim (gs-chipid.c `ap_hw_tune`, offset G#C300).
/// Un-quantized per-unit calibration, unlike the binned ASV beside it — a candidate ira ingredient, but NOT keyed until a hardware stability probe confirms every bit is fuse-stable (a drifting bit bricks the vault) and that it is actually populated (not zeros).
/// Read and reported for the probe; see `read_ap_hw_tune`.
const OFF_AP_HW_TUNE: usize = 0xC300;
const AP_HW_TUNE_LEN: usize = 32;

/// ASV table — 64 bytes of per-die leakage/voltage binning trim (gs-chipid.c `asv_tbl`, offset G#9000).
/// Binned (coarser than ap_hw_tune) but still per-die; a candidate ira ingredient, reported for the probe, NOT keyed until stability is confirmed.
const OFF_ASV_TBL: usize = 0x9000;
const ASV_TBL_LEN: usize = 64;

/// HPM/ASV extended block — 64 bytes of per-die high-performance-monitor / leakage data (gs-chipid.c `hpm_asv`, offset G#A000).
const OFF_HPM_ASV: usize = 0xA000;
const HPM_ASV_LEN: usize = 64;

/// Per-die DVFS speed-bin class — one byte (gs-chipid.c `dvfs_version`, offset G#900C).
const OFF_DVFS_VERSION: usize = 0x900C;

/// BLAKE3 KDF context for the interim husky ira. Versioned: bump on any change to the material set below, since that re-keys every vault derived from it.
const IRA_CONTEXT: &str = "ferros.ira.husky.v2";

/// Read husky's 16-byte hardware identity: `[product_id, unique_id0, unique_id1, revision]`, each u32 little-endian.
///
/// The die-unique 64-bit serial (`unique_id0`/`unique_id1`) is the permanent per-device part; `product_id`/`revision` pin the SoC model and stepping so a die-collision across product lines can't converge.
/// Pure MMIO reads of the always-mapped chipid block — no side effects, no ordering requirements against anything else.
pub fn read_hw_identity() -> [u8; 16] {
    let rd = |off: usize| unsafe { ferros_hal::mmio::read32(CHIPID_BASE + off) };
    let mut id = [0u8; 16];
    id[0..4].copy_from_slice(&rd(OFF_PRODUCT_ID).to_le_bytes());
    id[4..8].copy_from_slice(&rd(OFF_UNIQUE_ID0).to_le_bytes());
    id[8..12].copy_from_slice(&rd(OFF_UNIQUE_ID1).to_le_bytes());
    id[12..16].copy_from_slice(&rd(OFF_REVISION).to_le_bytes());
    id
}

/// Read the 32-byte AP hardware-tuning fuse block (`G#C300`..`G#C31F`), byte-wide like gs-chipid.c.
///
/// This is *instrumentation*, not key material yet. The stability probe reports these bytes to ramoops across a reboot + temperature cycle; only bits proven fuse-stable (and non-zero) graduate into `derive_ira`, and drifty bits would instead ride fuzzy extraction (see VAULT-KEY.md physics leg). Keyed use before that check risks an unrecoverable vault.
pub fn read_ap_hw_tune() -> [u8; AP_HW_TUNE_LEN] {
    let mut buf = [0u8; AP_HW_TUNE_LEN];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = unsafe { ferros_hal::mmio::read8(CHIPID_BASE + OFF_AP_HW_TUNE + i) };
    }
    buf
}

/// Read the 64-byte ASV table (`G#9000`..`G#903F`), byte-wide like gs-chipid.c.
///
/// Instrumentation only — same probe gate as `read_ap_hw_tune`: must be non-zero (populated) and bit-stable across boots before any of these bits graduate into `derive_ira`.
pub fn read_asv_tbl() -> [u8; ASV_TBL_LEN] {
    let mut buf = [0u8; ASV_TBL_LEN];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = unsafe { ferros_hal::mmio::read8(CHIPID_BASE + OFF_ASV_TBL + i) };
    }
    buf
}

/// Read the 64-byte HPM/ASV extended block (`G#A000`..`G#A03F`), byte-wide like gs-chipid.c. Instrumentation only; same probe gate as the ASV table.
pub fn read_hpm_asv() -> [u8; HPM_ASV_LEN] {
    let mut buf = [0u8; HPM_ASV_LEN];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = unsafe { ferros_hal::mmio::read8(CHIPID_BASE + OFF_HPM_ASV + i) };
    }
    buf
}

/// Read the per-die DVFS speed-bin class byte (`G#900C`). Instrumentation only.
pub fn read_dvfs_version() -> u8 {
    unsafe { ferros_hal::mmio::read8(CHIPID_BASE + OFF_DVFS_VERSION) }
}

/// Derive the husky *ira*: every stable per-device value the AP can reach, mashed through the KDF, with PIPE-style checks (PIPE IRA.md: no valid source set, no ira — refuse to operate).
///
/// Material, in this fixed order (each framed as name, u32 length, bytes):
/// 0. `hwid` — die-unique serial + SoC model/stepping (16 B)
/// 1. `ap_hw_tune` — analog per-die trim, the unforgeable core (32 B)
/// 2. `asv_tbl` (64 B) and 3. `hpm_asv` (64 B) — per-die voltage/speed calibration
/// 4. `dvfs_version` — speed bin (1 B)
/// 5. `devinfo` — the factory identity records (serials, IMEIs, MACs, board/device config, SKU, origin), UFS LUN 3; public but most people don't have them, and they never change short of a board swap
/// 6. `ufs_serial` — the flash device's serial-number string descriptor
///
/// Checks, the software stand-in for PIPE's inverted-pair OTP and KOHI health:
/// - every source is read twice and the reads must agree (a flaky read never becomes an identity);
/// - multi-byte sources must not be degenerate (all G#00 or all G#FF = a read that silently failed);
/// - devinfo must yield psn, dsn and imei1.
/// Any failure returns the failing source's bit (G#100 + index = reads disagreed, G#200 + index = degenerate, G#300 = devinfo missing records) and there is no ira, so the vault stays shut.
/// Proven STABLE across boots and temperature (sources 0-4, IRA-ENTROPY-SOURCES.md; 5-6 are factory data), not proven high-entropy fleet-wide. Context v2 = this material set: a vault sealed under v1 won't reopen (open-or-genesis re-genesises).
pub fn derive_ira() -> Result<[u8; 32], u32> {
    let reads = || -> Result<[alloc::vec::Vec<u8>; 7], u32> {
        Ok([
            read_hw_identity().to_vec(),
            read_ap_hw_tune().to_vec(),
            read_asv_tbl().to_vec(),
            read_hpm_asv().to_vec(),
            alloc::vec![read_dvfs_version()],
            read_devinfo_identity().ok_or(0x300u32)?,
            read_ufs_serial().unwrap_or_default(),
        ])
    };
    let a = reads()?;
    let b = reads()?;
    const NAMES: [&str; 7] = ["hwid", "ap_hw_tune", "asv_tbl", "hpm_asv", "dvfs_version", "devinfo", "ufs_serial"];
    let mut m = alloc::vec::Vec::new();
    for i in 0..7 {
        if a[i] != b[i] {
            return Err(0x100 + i as u32);
        }
        if a[i].len() > 1 && (a[i].iter().all(|&x| x == 0) || a[i].iter().all(|&x| x == 0xFF)) {
            return Err(0x200 + i as u32);
        }
        m.extend_from_slice(NAMES[i].as_bytes());
        m.extend_from_slice(&(a[i].len() as u32).to_le_bytes());
        m.extend_from_slice(&a[i]);
    }
    Ok(AnchorKey::derive(IRA_CONTEXT, &m))
}

/// The ira as vault boot derived it, held for `SYS_IRA` (process.rs): a process with an Ira capability gets `blake3::derive_key("ferros process ira v1 " ‖ its domain, ira)` — a per-purpose key rooted in the device that survives a factory reset ([[ira_network_owns_assignment]]: the Photon device key is one), never the ira itself, and never under a context the kernel's own derivations use.
/// Held in CPU registers, not DRAM, since 2026-10-28 (keys.rs, KEY_REGISTERS.md): vault boot hands it over once and its copy is wiped.
pub fn keep_ira(ira: &mut [u8; 32]) -> bool {
    crate::keys::set(crate::keys::IRA, ira)
}

/// `blake3::derive_key(context, ira)`, computed where the ira is held. None before vault boot has kept one.
pub fn derive(context: &str) -> Option<[u8; 32]> {
    crate::keys::derive(crate::keys::IRA, context)
}


/// devinfo's identity records (UFS LUN 3, 4 KiB LBA 6, after the 128-byte slot header): "DIUS" records of [total u32][key length u32][key NUL][value NUL], total = key length + value length. Returns "key=value\n" lines for the keys below, in this order, each that is present; None unless psn, dsn and imei1 all are.
fn read_devinfo_identity() -> Option<alloc::vec::Vec<u8>> {
    const KEYS: [&[u8]; 12] = [b"psn", b"dsn", b"imei1", b"imei2", b"wlan_mac1", b"bt_addr", b"pcbcfg", b"devcfg", b"sku", b"psku", b"coo", b"dpc"];
    let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
    let mut ok = false;
    for _ in 0..4 {
        if ufs.read_block_lun(3, 6) == 0 && ufs.last_response_status() == 0 {
            ok = true;
            break;
        }
    }
    if !ok {
        return None;
    }
    let d = ufs.data_buffer();
    if &d[0..4] != b"DEVI" {
        return None;
    }
    let mut found: [Option<&[u8]>; 12] = [None; 12];
    let mut p = 0x80;
    while p + 12 <= d.len() {
        if &d[p..p + 4] != b"DIUS" {
            p += 1;
            continue;
        }
        let total = u32::from_le_bytes([d[p + 4], d[p + 5], d[p + 6], d[p + 7]]) as usize;
        let klen = u32::from_le_bytes([d[p + 8], d[p + 9], d[p + 10], d[p + 11]]) as usize;
        if klen == 0 || klen > total || p + 12 + total > d.len() {
            p += 1;
            continue;
        }
        let key = &d[p + 12..p + 12 + klen - 1];
        let val = &d[p + 12 + klen..p + 12 + total];
        let val = &val[..val.iter().position(|&c| c == 0).unwrap_or(val.len())];
        if let Some(i) = KEYS.iter().position(|k| *k == key) {
            found[i] = Some(val);
        }
        p += 12 + total;
    }
    if found[0].is_none() || found[1].is_none() || found[2].is_none() {
        return None;
    }
    let mut out = alloc::vec::Vec::new();
    for (k, v) in KEYS.iter().zip(found.iter()) {
        if let Some(v) = v {
            out.extend_from_slice(k);
            out.push(b'=');
            out.extend_from_slice(v);
            out.push(b'\n');
        }
    }
    Some(out)
}

/// The UFS device's serial-number string (device descriptor iSerialNumber at G#16 → string descriptor, UTF-16), raw bytes after the 2-byte header.
fn read_ufs_serial() -> Option<alloc::vec::Vec<u8>> {
    use ferros_hal::ufs::{upiu, UfsController};
    let ufs = UfsController::new(0x1320_0000);
    let (dev, dev_len, ocs) = ufs.read_descriptor(upiu::DESC_DEVICE, 0);
    if ocs != 0 || dev_len <= 0x16 {
        return None;
    }
    let (s, s_len, ocs) = ufs.read_descriptor(upiu::DESC_STRING, dev[0x16]);
    if ocs != 0 || s_len < 2 || s[1] != upiu::DESC_STRING {
        return None;
    }
    let n = (s[0] as usize).min(s_len);
    Some(s[2..n].to_vec())
}
