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
const IRA_CONTEXT: &str = "ferros.ira.husky.v1";

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

/// Derive the husky *ira* from the per-die chip-ID material proven stable on hardware.
///
/// Keyed material (all validated 2026-08-21 — populated AND byte-identical across four boots spanning a freezer cold-soak and a toaster heat-soak; see IRA-ENTROPY-SOURCES.md):
/// `read_hw_identity()` (16B: die-unique serial + SoC model/stepping) ‖ `ap_hw_tune` (32B analog per-die trim — the unforgeable core) ‖ `asv_tbl` (64B) ‖ `hpm_asv` (64B) ‖ `dvfs_version` (1B speed bin) = 177 bytes folded through the KDF.
/// Context bumped v0 -> v1 to mark this material set: a vault sealed under v0's identity-only key won't reopen (open-or-genesis re-genesises — correct for the bring-up transition).
/// Still EXCLUDED until their read paths + stability are validated: UFS serial, MCT ring-osc, and the vendor-dispersion sources — each further addition re-keys and must be a deliberate, tested step. NOTE: these fields are proven STABLE (won't brick the vault), not yet proven HIGH-ENTROPY — per-die fleet variance is unmeasured from one device (ap_hw_tune's tail-zeros hint width > entropy).
pub fn derive_ira() -> [u8; 32] {
    let hwid = read_hw_identity();
    let ap = read_ap_hw_tune();
    let asv = read_asv_tbl();
    let hpm = read_hpm_asv();
    let dvfs = read_dvfs_version();
    let mut m = [0u8; 16 + 32 + 64 + 64 + 1];
    m[0..16].copy_from_slice(&hwid);
    m[16..48].copy_from_slice(&ap);
    m[48..112].copy_from_slice(&asv);
    m[112..176].copy_from_slice(&hpm);
    m[176] = dvfs;
    AnchorKey::derive(IRA_CONTEXT, &m)
}

/// The vault AnchorKey for this husky device: `from_ira(derive_ira())`.
///
/// Replaces the `[0x5A; 32]` bring-up constant. Same value every boot on the same phone; a different phone yields a different key; survives factory reset because the chip-ID does. Any vault sealed under the old constant will not open under this key — the open-or-genesis path simply re-genesises, which is correct for the bring-up transition.
pub fn anchor_key() -> AnchorKey {
    AnchorKey::from_ira(&derive_ira())
}
