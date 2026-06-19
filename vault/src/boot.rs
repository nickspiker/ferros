//! Boot sequence — hostile-state recovery and mesh mount.
//!
//! The Ledger boot sequence assumes nothing about the previous state. Every boot is a recovery. The mesh must arbitrate and agree on the current state before the ledger is considered mounted.
//!
//! ## Contrast
//! - BTRFS: Boot reads the superblock at offset 0x10000 (or mirrors at
//!   0x4000000, 0x4000000000). If valid, follow tree roots. If log tree exists, replay it. Trust the superblock — it's the root of all truth. Fixed offsets mean an attacker knows exactly where to strike.
//! - RedoxFS: Boot scans 256 header ring slots. Newest valid header
//!   (highest generation with valid SeaHash) wins. Replay allocation log from that header. Simple and effective for single-device.
//! - Ledger: No fixed offsets. No header ring. Mesh protocol queries
//!   each device for its last confirmed generation, compares state, resolves conflicts, and only mounts when consensus is reached. If devices disagree, failure records are created and the conflict is resolved (or escalated) before any reads are allowed.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::anchor::{AnchorError, AnchorKeyStore, AnchorRingConfig, MeshAnchor};
use crate::device::{Device, DeviceId};
use crate::failure::FailureRecord;
use crate::hash::ObjectHash;
use crate::mesh::{MeshId, MeshMember};

/// Boot stages — the ledger progresses thru these in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootStage {
    /// Initial probe — discovering which devices are present.
    DeviceDiscovery,
    /// Reading each device's last confirmed state.
    StateCollection,
    /// Comparing device states and detecting conflicts.
    ConflictDetection,
    /// Resolving conflicts between disagreeing devices.
    ConflictResolution,
    /// Mesh consensus achieved — ledger is mounting.
    Mounting,
    /// Ledger is mounted and ready for operations.
    Mounted,
    /// Boot failed — could not achieve consensus.
    Failed(BootError),
}

/// Errors that can prevent boot from completing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootError {
    /// No devices found at all.
    NoDevices,
    /// Only one device found (below dual-SSD minimum).
    InsufficientDevices { found: usize },
    /// All devices are from the same vendor (violates dual-vendor).
    SingleVendor,
    /// The out-of-band key store has no key/config for a device. Cannot derive anchor ring offsets without this.
    NoAnchorKey(DeviceId),
    /// No valid anchor found on a device (uninitialized, all slots corrupted, or wrong key).
    NoValidAnchor(DeviceId),
    /// Anchor HMAC failed — possible tampering or key mismatch.
    AnchorTampered(DeviceId),
    /// The commit chain referenced by an anchor is broken. The anchor points to a commit hash that doesn't exist or whose parent chain doesn't verify.
    BrokenCommitChain {
        device: DeviceId,
        generation: u64,
        missing_hash: ObjectHash,
    },
    /// Devices disagree on state and automatic resolution failed.
    UnresolvableConflict {
        devices: Vec<DeviceId>,
        failures: Vec<FailureRecord>,
    },
    /// All devices report failure states — no healthy device to boot from.
    AllDevicesFailed,
    /// The mesh ID doesn't match across devices (not the same ledger).
    MeshIdMismatch {
        expected: MeshId,
        found: Vec<(DeviceId, MeshId)>,
    },
    /// Anchor-layer error.
    Anchor(AnchorError),
}

/// State collected from a single device during boot.
///
/// Populated by reading the device's anchor ring via the out-of-band key store. The anchor tells us where to find the commit chain — from there we can verify the device's object store is consistent.
#[derive(Clone, Debug)]
pub struct DeviceBootState {
    pub device_id: DeviceId,
    /// The anchor read from this device's ring (if any valid anchor found).
    pub anchor: Option<MeshAnchor>,
    /// The last generation this device confirmed (from anchor).
    pub last_generation: u64,
    /// The root hash at that generation (from anchor).
    pub root_hash: ObjectHash,
    /// The mesh ID this device claims to belong to (from anchor).
    pub mesh_id: MeshId,
    /// Any failure records this device has from its last operation.
    pub pending_failures: Vec<FailureRecord>,
    /// Whether this device's state is internally consistent (its own hashes check out, anchor HMAC valid).
    pub self_consistent: bool,
}

/// The boot engine — orchestrates the full bootstrap sequence.
///
/// ## The Onramp Protocol
///
/// ```text
/// 1. DeviceDiscovery:   Enumerate available storage devices.
/// 2. AnchorRetrieval:   For each device, read anchor key from out-of-band store
///                       (UEFI/CSR/HSM), derive ring offsets, scan ring for newest valid anchor.
/// 3. StateCollection:   Each anchor gives us: mesh_id, generation, root_commit.
///                       Follow root_commit hash into the object store to verify the commit chain is intact.
/// 4. ConflictDetection: Compare anchors across devices. If all agree on
///                       generation + root_commit, no conflict.
/// 5. ConflictResolution: If devices disagree:
///                       - Higher generation with valid commit chain wins.
///                       - Equal generation but different root = corruption or
///                         fork — create FailureRecords, attempt repair from the device with a valid chain.
///                       - If no device has a valid chain, boot fails.
/// 6. Mounting:          Write agreed-upon anchor to any stale devices,
///                       mount the object store from the agreed root.
/// ```
pub trait BootEngine {
    /// Run the full boot sequence. Returns the mounted mesh state or a boot error.
    ///
    /// `key_store` provides the out-of-band anchor keys/configs. This is the ONLY external dependency — everything else is derived from the devices themselves.
    fn boot(
        &mut self,
        devices: &mut [Box<dyn Device>],
        key_store: &dyn AnchorKeyStore,
    ) -> Result<BootResult, BootError>;

    /// Probe a single device: read its anchor ring via the key store.
    fn probe_device(
        &self,
        device: &dyn Device,
        config: &AnchorRingConfig,
    ) -> Result<DeviceBootState, BootError>;

    /// Compare boot states from multiple devices and detect conflicts.
    fn detect_conflicts(&self, states: &[DeviceBootState]) -> Vec<BootConflict>;

    /// Attempt to resolve a conflict automatically. Returns the winning state's root hash, or fails.
    fn resolve_conflict(&self, conflict: &BootConflict) -> Result<ObjectHash, BootError>;
}

/// A conflict detected during boot between two or more devices.
#[derive(Clone, Debug)]
pub struct BootConflict {
    /// Devices involved in the conflict.
    pub devices: Vec<DeviceId>,
    /// The generation where they diverge.
    pub diverge_at_generation: u64,
    /// Each device's claimed root hash at the divergence point.
    pub claimed_roots: Vec<(DeviceId, ObjectHash)>,
}

/// The result of a successful boot.
#[derive(Clone, Debug)]
pub struct BootResult {
    /// The agreed-upon current generation.
    pub generation: u64,
    /// The agreed-upon root hash.
    pub root_hash: ObjectHash,
    /// The mesh members and their states post-boot.
    pub members: Vec<MeshMember>,
    /// Any failure records generated during boot itself.
    pub boot_failures: Vec<FailureRecord>,
}
