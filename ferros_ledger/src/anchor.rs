//! Mesh Anchor — the bootstrap onramp for the Ledger.
//!
//! # The Problem
//!
//! The Ledger has no superblock, no fixed offsets, no magic numbers.
//! But something has to bootstrap. You need a toehold — a way to go
//! from "powered on, know nothing" to "found the commit chain."
//!
//! # The Answer: Two Trust Domains
//!
//! The anchor key lives in a *different trust domain* than the data.
//! The key tells you where to look; the data tells you what's there.
//! Without the key, the storage device looks like random noise.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │ Trust Domain A: Key Store (survives power loss)     │
//! │                                                     │
//! │  Fairphone 5:  UFS RPMB partition                   │
//! │  Glyph:        RISC-V CSR / crypto coprocessor      │
//! │  8KB pico:     eFuse or OTP                         │
//! │  Dev/test:     file on host, or RAM                 │
//! │                                                     │
//! │  Contains: AnchorKey (32 bytes) + ring_size (EWE)   │
//! └──────────────────────┬──────────────────────────────┘
//!                        │ key
//!                        ▼
//!       derive_ring_offset(key, slot) → offset on device
//!                        │
//!                        ▼
//! ┌─────────────────────────────────────────────────────┐
//! │ Trust Domain B: Storage Device (UFS main area)      │
//! │                                                     │
//! │  At each derived offset: a VSF-encoded anchor       │
//! │  (variable size, self-describing, HMAC'd)           │
//! │                                                     │
//! │  anchor → commit_hash → CommitRecord → objects      │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! # Fairphone 5 Boot Reality
//!
//! The FP5 runs a Qualcomm QCM6490. We don't control the early boot
//! chain — and we shouldn't pretend otherwise.
//!
//! ```text
//! Stage        Who Controls   What Happens
//! ─────        ────────────   ────────────
//! PBL          Qualcomm ROM   SoC powers on, loads XBL from UFS
//! XBL          Qualcomm+FP    DDR init, TrustZone, UFS driver
//! ABL          Qualcomm+FP    fastboot, boot image verification
//!   ╰── HERE   us (unlocked)  ABL loads our boot.img from boot_a/b
//! ferros       us             kernel starts, needs to find ledger
//! ```
//!
//! **We enter the picture at ABL.** Bootloader must be unlocked
//! (`fastboot oem unlock`). ABL loads our kernel from the boot
//! partition. From that point forward, it's our code.
//!
//! **Where the anchor key lives on FP5 — the honest version:**
//!
//! RPMB (Replay Protected Memory Block) exists on the UFS chip and
//! would be ideal — but we can't easily reach it:
//!
//! ```text
//! ferros kernel (EL1, Normal World)
//!     │
//!     │ SMC (Secure Monitor Call)
//!     ▼
//! TrustZone / QTEE (EL3, Secure World) ← Qualcomm's signed blob
//!     │
//!     │ RPMB auth key (from QFPROM fuses, provisioned at first boot)
//!     ▼
//! UFS RPMB partition
//! ```
//!
//! The RPMB auth key is derived from hardware fuses, provisioned by
//! XBL, and held in TrustZone. Normal-world code (our kernel) cannot
//! access RPMB directly — it must go through Qualcomm's QTEE via SMC
//! calls, and QTEE may refuse a non-Android caller.
//!
//! **Phase 1 (bring-up): Dedicated partition**
//! Anchor key lives in a `ferros_anchor` partition on regular UFS.
//! Flash via `fastboot flash ferros_anchor <key.img>`.
//! Same trust domain as data — but mesh consensus across two devices
//! from different vendors still provides the core security property.
//! An attacker needs BOTH devices, not just one.
//!
//! **Phase 2 (mid-term): Custom ABL handoff**
//! Build a modified ABL (EDK2-based, Fairphone publishes sources).
//! Our ABL reads RPMB (ABL has TrustZone access), stashes the anchor
//! key in a reserved-memory DTB node, then boots our kernel. Our
//! kernel reads it from the DTB. One-way handoff: RPMB → ABL → DTB
//! → kernel. Key is in RAM only during boot, cleared after anchor
//! ring scan completes.
//!
//! **Phase 3 (Glyph): Own the stack**
//! On Glyph hardware we control the secure world. Anchor key lives
//! in a dedicated CSR or crypto coprocessor register. Kill-switch
//! zeroes the register → anchors become unfindable → ledger
//! cryptographically erased.
//!
//! # VSF Encoding
//!
//! The anchor is NOT a fixed-size struct. It's VSF-encoded:
//! - Fields use Elastic Width Encoding (EWE)
//! - A generation of 0 takes 1 byte, not 8
//! - A fresh anchor (gen=0, seq=0) is ~40 bytes
//! - A mature anchor (gen=2^48, full hashes) is ~90 bytes
//! - The HMAC is always 32 bytes (fixed by BLAKE3)
//! - Total size is self-describing — the decoder knows when to stop
//!
//! The ring slot allocation reserves space for the *maximum* anchor
//! size, but only the actual encoded bytes are written. The rest is
//! don't-care (indistinguishable from noise on encrypted storage).

use alloc::vec::Vec;

use crate::device::{Device, DeviceError, DeviceId};
use crate::hash::ObjectHash;
use crate::mesh::MeshId;

// ---------------------------------------------------------------------------
// Anchor structure (in-memory representation)
// ---------------------------------------------------------------------------

/// The mesh anchor — minimal bootstrap pointer, VSF-encoded on disk.
///
/// This is the in-memory representation. Serialization to/from bytes
/// uses VSF Elastic Width Encoding (see `encode_anchor`/`decode_anchor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshAnchor {
    /// Which mesh this device belongs to.
    pub mesh_id: MeshId,
    /// This device's identity within the mesh.
    pub device_id: DeviceId,
    /// The last generation this device confirmed via mesh consensus.
    pub generation: u64,
    /// Hash of the CommitRecord at this generation.
    pub root_commit: ObjectHash,
    /// Sequence number within the anchor ring (monotonically increasing).
    pub anchor_seq: u64,
    /// BLAKE3-keyed-hash of all above fields.
    pub hmac: [u8; 32],
}

/// Anchor ring slot reservation size (bytes).
///
/// 256 bytes per slot. VSF EWE-encoded anchor lives inside; unused
/// bytes are don't-care noise on encrypted storage.
///
/// Why 256 and not 128:
/// - Current max encoded anchor is ~120 bytes (fits in 128).
/// - But 128 leaves zero room for future fields (device health
///   snapshot, mesh topology hash, monotonic timestamp, etc.).
/// - 256 buys forward compatibility without breaking the format.
/// - At 1MB ring: 4,096 slots instead of 8,192. Still enormous.
/// - 256 = BLAKE3 output width in bits. Natural alignment.
/// - The "wasted" ~130 bytes per slot are noise on encrypted
///   storage — indistinguishable from the rest of the device.
///
/// Ring math: `1MB ÷ 256B = 4,096 rollback points`
/// - @ 1 commit/sec  = 68 minutes of history
/// - @ 1 commit/min  = 2.8 days
/// - @ 1 commit/10m  = 28 days
pub const ANCHOR_SLOT_SIZE: usize = 256;

/// Default anchor ring size: 1MB.
///
/// `1MB ÷ 256B = 4,096 slots`. Each slot is at a BLAKE3-derived
/// secret offset on the device. The ring provides crash safety
/// (power loss during write leaves N-1 previous anchors intact)
/// and rollback depth (boot can recover to any of the last 4,096
/// committed generations).
pub const ANCHOR_RING_DEFAULT_BYTES: u64 = 1024 * 1024;

/// Number of slots that fit in the default 1MB ring.
pub const ANCHOR_RING_DEFAULT_SLOTS: u64 =
    ANCHOR_RING_DEFAULT_BYTES / ANCHOR_SLOT_SIZE as u64;

// ---------------------------------------------------------------------------
// Anchor key and key store
// ---------------------------------------------------------------------------

/// 32-byte key stored in a separate trust domain.
///
/// On FP5: lives in UFS RPMB (authenticated, anti-replay).
/// On Glyph: lives in RISC-V CSR / crypto coprocessor.
/// On dev: lives in a file or RAM.
#[derive(Clone, Copy, Debug)]
pub struct AnchorKey(pub [u8; 32]);

/// Configuration for the anchor ring on a specific device.
#[derive(Clone, Debug)]
pub struct AnchorRingConfig {
    /// Number of anchor slots in the ring.
    /// 1 (8KB flash) to 256+ (phone/server).
    pub ring_size: u64,
    /// The anchor key for this device.
    pub key: AnchorKey,
    /// Device capacity in bytes (to bound ring offset derivation).
    pub device_capacity: u64,
}

/// Where the anchor key physically lives.
///
/// Each variant maps to a concrete hardware path. The boot code
/// dispatches on this to know which driver/protocol to use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStoreBackend {
    /// **FP5 Phase 1 (bring-up):** Dedicated UFS partition.
    ///
    /// `fastboot flash ferros_anchor <key.img>`
    ///
    /// Same trust domain as data. Security comes from mesh (dual
    /// device agreement), not from partition isolation. Attacker
    /// needs both devices.
    UfsPartition {
        /// GPT partition name (e.g., b"ferros_anchor").
        partition_name: Vec<u8>,
        /// Byte offset within the partition where the key starts.
        offset: u64,
    },

    /// **FP5 Phase 2 (custom ABL):** ABL reads RPMB, hands key to
    /// kernel via DTB reserved-memory node.
    ///
    /// The key is in RAM only during early boot. Kernel reads it
    /// from the DTB, scans the anchor ring, then zeroes the RAM
    /// region. Not persisted in normal-world storage.
    AblDtbHandoff {
        /// DTB node path (e.g., b"/reserved-memory/ferros-anchor-key").
        dtb_node: Vec<u8>,
    },

    /// **FP5 Phase 2 alt:** Direct RPMB via TrustZone SMC calls.
    ///
    /// Requires reverse-engineering or documentation of Qualcomm's
    /// QTEE RPMB interface. Non-trivial. May not be possible on all
    /// firmware versions.
    UfsRpmb {
        /// RPMB frame address where the anchor key starts.
        rpmb_address: u32,
    },

    /// **Glyph (Phase 3):** RISC-V CSR in crypto coprocessor.
    ///
    /// Register is hardware write-once-per-boot, read-disabled after
    /// initial load. Kill-switch zeroes it.
    RiscvCsr {
        /// CSR address.
        csr: u16,
    },

    /// **Pico devices:** One-time programmable fuse.
    Efuse {
        bank: u32,
        offset: u32,
    },

    /// **Testing only.** RAM-backed, does not survive power loss.
    Memory,
}

/// Trait for the out-of-band key store.
///
/// Implementations are platform-specific. The boot code calls this
/// to get the anchor key before it can read anything from main storage.
pub trait AnchorKeyStore {
    /// What backend this store uses (for diagnostics/logging).
    fn backend(&self) -> KeyStoreBackend;

    /// Retrieve the anchor key for a device.
    fn get_anchor_key(&self, device_id: &DeviceId) -> Result<AnchorKey, AnchorError>;

    /// Retrieve the full ring config for a device.
    fn get_ring_config(&self, device_id: &DeviceId) -> Result<AnchorRingConfig, AnchorError>;

    /// Store the anchor key (during initial mesh formation).
    fn set_anchor_key(
        &mut self,
        device_id: &DeviceId,
        key: AnchorKey,
    ) -> Result<(), AnchorError>;

    /// Store the full ring config.
    fn set_ring_config(
        &mut self,
        device_id: &DeviceId,
        config: AnchorRingConfig,
    ) -> Result<(), AnchorError>;
}

// ---------------------------------------------------------------------------
// Ring offset derivation
// ---------------------------------------------------------------------------

/// Derive the physical offset of anchor ring slot N on a device.
///
/// `offset = BLAKE3(anchor_key || slot_index || "anchor_offset") % usable_space`
///
/// Without the key, slot offsets look random. The device appears as
/// undifferentiated noise — there's no magic number or fixed offset
/// to scan for.
pub fn derive_ring_offset(key: &AnchorKey, slot_index: u64, device_capacity: u64) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&key.0);
    hasher.update(&slot_index.to_le_bytes());
    hasher.update(b"anchor_offset");
    let hash = hasher.finalize();

    let raw = u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap());
    // Ensure the slot doesn't wrap past end of device.
    raw % (device_capacity.saturating_sub(ANCHOR_SLOT_SIZE as u64).max(1))
}

// ---------------------------------------------------------------------------
// HMAC
// ---------------------------------------------------------------------------

/// Compute the HMAC for an anchor (covers all fields except hmac itself).
pub fn compute_anchor_hmac(anchor: &MeshAnchor, key: &AnchorKey) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(&key.0);
    hasher.update(&anchor.mesh_id.0);
    hasher.update(&anchor.device_id.0);
    hasher.update(&anchor.generation.to_le_bytes());
    hasher.update(&anchor.root_commit.0);
    hasher.update(&anchor.anchor_seq.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Verify an anchor's HMAC (constant-time).
pub fn verify_anchor_hmac(anchor: &MeshAnchor, key: &AnchorKey) -> bool {
    let expected = compute_anchor_hmac(anchor, key);
    constant_time_eq(&anchor.hmac, &expected)
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// VSF Elastic Width Encoding for anchors
// ---------------------------------------------------------------------------

/// VSF EWE tag bytes for anchor fields.
/// Each field is prefixed with a 1-byte tag that encodes:
///   bits [7:4] = field id (0..5)
///   bits [3:0] = payload length in bytes (0..15, 0 means "next byte is length")
///
/// Fixed-size fields (mesh_id, device_id, root_commit, hmac) encode
/// their length directly. Variable-size fields (generation, anchor_seq)
/// use only as many bytes as the value requires.
mod ewe {
    pub const TAG_MESH_ID: u8 = 0x00;    // field 0, length in low nibble
    pub const TAG_DEVICE_ID: u8 = 0x10;  // field 1
    pub const TAG_GENERATION: u8 = 0x20; // field 2
    pub const TAG_ROOT_COMMIT: u8 = 0x30; // field 3
    pub const TAG_ANCHOR_SEQ: u8 = 0x40; // field 4
    pub const TAG_HMAC: u8 = 0x50;       // field 5

    /// Encode a u64 using the minimum number of bytes needed.
    pub fn ewe_len_u64(val: u64) -> usize {
        if val == 0 { return 1; }
        ((64 - val.leading_zeros() + 7) / 8) as usize
    }

    /// Write a u64 in little-endian using exactly `len` bytes.
    pub fn write_u64_le(buf: &mut [u8], val: u64, len: usize) {
        let bytes = val.to_le_bytes();
        buf[..len].copy_from_slice(&bytes[..len]);
    }

    /// Read a u64 from `len` little-endian bytes.
    pub fn read_u64_le(buf: &[u8], len: usize) -> u64 {
        let mut bytes = [0u8; 8];
        bytes[..len].copy_from_slice(&buf[..len]);
        u64::from_le_bytes(bytes)
    }
}

/// Encode a MeshAnchor to VSF EWE bytes. Returns the number of bytes written.
///
/// The output buffer must be at least `ANCHOR_SLOT_SIZE` bytes.
/// Only the actually-used bytes are meaningful; the rest are untouched.
pub fn encode_anchor(anchor: &MeshAnchor, buf: &mut [u8]) -> usize {
    let mut pos = 0;

    // Field 0: mesh_id (fixed 16 bytes)
    buf[pos] = ewe::TAG_MESH_ID | 16;
    pos += 1;
    buf[pos..pos + 16].copy_from_slice(&anchor.mesh_id.0);
    pos += 16;

    // Field 1: device_id (fixed 16 bytes)
    buf[pos] = ewe::TAG_DEVICE_ID | 16;
    pos += 1;
    buf[pos..pos + 16].copy_from_slice(&anchor.device_id.0);
    pos += 16;

    // Field 2: generation (variable width)
    let gen_len = ewe::ewe_len_u64(anchor.generation);
    buf[pos] = ewe::TAG_GENERATION | (gen_len as u8);
    pos += 1;
    ewe::write_u64_le(&mut buf[pos..], anchor.generation, gen_len);
    pos += gen_len;

    // Field 3: root_commit (fixed 32 bytes)
    // 32 doesn't fit in 4 bits, so we use 0 = "next byte is length"
    buf[pos] = ewe::TAG_ROOT_COMMIT | 0;
    pos += 1;
    buf[pos] = 32;
    pos += 1;
    buf[pos..pos + 32].copy_from_slice(&anchor.root_commit.0);
    pos += 32;

    // Field 4: anchor_seq (variable width)
    let seq_len = ewe::ewe_len_u64(anchor.anchor_seq);
    buf[pos] = ewe::TAG_ANCHOR_SEQ | (seq_len as u8);
    pos += 1;
    ewe::write_u64_le(&mut buf[pos..], anchor.anchor_seq, seq_len);
    pos += seq_len;

    // Field 5: hmac (fixed 32 bytes)
    buf[pos] = ewe::TAG_HMAC | 0;
    pos += 1;
    buf[pos] = 32;
    pos += 1;
    buf[pos..pos + 32].copy_from_slice(&anchor.hmac);
    pos += 32;

    pos
}

/// Decode a MeshAnchor from VSF EWE bytes. Returns None if invalid.
pub fn decode_anchor(buf: &[u8]) -> Option<MeshAnchor> {
    let mut pos = 0;
    let mut anchor = MeshAnchor {
        mesh_id: MeshId([0; 16]),
        device_id: DeviceId([0; 16]),
        generation: 0,
        root_commit: ObjectHash([0; 32]),
        anchor_seq: 0,
        hmac: [0; 32],
    };
    let mut fields_seen = 0u8;

    while pos < buf.len() && fields_seen < 6 {
        if pos >= buf.len() { return None; }
        let tag_byte = buf[pos];
        pos += 1;

        let field_id = (tag_byte >> 4) & 0x0F;
        let mut length = (tag_byte & 0x0F) as usize;

        // length == 0 means next byte holds the real length
        if length == 0 {
            if pos >= buf.len() { return None; }
            length = buf[pos] as usize;
            pos += 1;
        }

        if pos + length > buf.len() { return None; }

        match field_id {
            0 => { // mesh_id
                if length != 16 { return None; }
                anchor.mesh_id.0.copy_from_slice(&buf[pos..pos + 16]);
            }
            1 => { // device_id
                if length != 16 { return None; }
                anchor.device_id.0.copy_from_slice(&buf[pos..pos + 16]);
            }
            2 => { // generation
                if length > 8 { return None; }
                anchor.generation = ewe::read_u64_le(&buf[pos..], length);
            }
            3 => { // root_commit
                if length != 32 { return None; }
                anchor.root_commit.0.copy_from_slice(&buf[pos..pos + 32]);
            }
            4 => { // anchor_seq
                if length > 8 { return None; }
                anchor.anchor_seq = ewe::read_u64_le(&buf[pos..], length);
            }
            5 => { // hmac
                if length != 32 { return None; }
                anchor.hmac.copy_from_slice(&buf[pos..pos + 32]);
            }
            _ => {
                // Unknown field — skip it (forward compatibility).
            }
        }

        pos += length;
        fields_seen += 1;
    }

    // All 6 fields must be present.
    if fields_seen < 6 { return None; }
    Some(anchor)
}

// ---------------------------------------------------------------------------
// Ring operations
// ---------------------------------------------------------------------------

/// Read the newest valid anchor from a device's anchor ring.
///
/// Scans all ring slots, decodes VSF EWE, verifies HMACs, returns
/// the anchor with the highest sequence number. Returns None if no
/// valid anchor found (uninitialized or all corrupted).
pub fn read_latest_anchor(
    device: &dyn Device,
    config: &AnchorRingConfig,
) -> Result<Option<MeshAnchor>, AnchorError> {
    let mut best: Option<MeshAnchor> = None;

    for slot in 0..config.ring_size {
        let offset = derive_ring_offset(&config.key, slot, config.device_capacity);
        let mut buf = [0u8; ANCHOR_SLOT_SIZE];

        match device.read_at(offset, &mut buf) {
            Ok(()) => {}
            Err(_) => continue,
        }

        if let Some(anchor) = decode_anchor(&buf) {
            if verify_anchor_hmac(&anchor, &config.key) {
                match &best {
                    None => best = Some(anchor),
                    Some(current) if anchor.anchor_seq > current.anchor_seq => {
                        best = Some(anchor);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(best)
}

/// Write a new anchor to the next ring slot (VSF EWE encoded).
///
/// Computes HMAC, encodes to EWE, writes to the slot at
/// `anchor_seq % ring_size`, then flushes.
pub fn write_anchor(
    device: &mut dyn Device,
    config: &AnchorRingConfig,
    anchor: &mut MeshAnchor,
) -> Result<(), AnchorError> {
    anchor.hmac = compute_anchor_hmac(anchor, &config.key);

    let slot = anchor.anchor_seq % config.ring_size;
    let offset = derive_ring_offset(&config.key, slot, config.device_capacity);

    let mut buf = [0u8; ANCHOR_SLOT_SIZE];
    let _encoded_len = encode_anchor(anchor, &mut buf);
    // We write the full slot — unused bytes are noise on encrypted storage.

    device
        .write_at(offset, &buf)
        .map_err(AnchorError::DeviceError)?;
    device.flush().map_err(AnchorError::DeviceError)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from anchor operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// Key store has no key for this device.
    NoKeyForDevice(DeviceId),
    /// No valid anchor found on device.
    NoValidAnchor(DeviceId),
    /// HMAC verification failed.
    HmacMismatch(DeviceId),
    /// Key store refused a write.
    KeyStoreWriteFailed,
    /// RPMB-specific: counter mismatch (possible replay attack).
    RpmbCounterMismatch { expected: u32, got: u32 },
    /// RPMB-specific: authentication failed.
    RpmbAuthFailed,
    /// Device I/O error.
    DeviceError(DeviceError),
}
