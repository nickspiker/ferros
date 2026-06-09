//! Layer 2 — Multi-device mesh consensus and arbitration.
//!
//! The mesh is how the Ledger achieves reliability without trusting any single device. Minimum configuration: dual SSD, dual vendor. Neither device is truth — mesh consensus is truth.
//!
//! The mesh arbitrates before the ledger is considered mounted. Boot assumes hostile previous state. Failed write states are preserved as first-class typed VSF records, not silently discarded.
//!
//! ## Contrast
//! - BTRFS: RAID via chunk tree. Block-level mirroring — the filesystem
//!   doesn't know *what* it's replicating, just that stripe N goes to device M. Recovery reads the alternate stripe. Superblock at 3 fixed offsets is the single root of trust.
//! - RedoxFS: Single-device only. No replication, no multi-device.
//!   The 256-slot header ring provides crash recovery on one device.
//! - Ledger: Semantic replication. The mesh replicates *objects*, not
//!   blocks. It can reason about conflicts because it understands the content. No fixed superblock offsets — mesh protocol establishes truth from the ground up.

use alloc::vec::Vec;

use crate::commit::CommitRecord;
use crate::device::DeviceId;
use crate::failure::FailureRecord;
use crate::hash::ObjectHash;

/// Unique identifier for a mesh (the set of devices forming a ledger).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(pub [u8; 16]);

/// The current state of a mesh member device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceState {
    /// Device is participating normally in mesh consensus.
    Active,
    /// Device is reachable but behind on generations (catching up).
    Syncing { behind_by: u64 },
    /// Device is unreachable. Last known generation recorded.
    Offline { last_generation: u64 },
    /// Device reported a failure state during its last write.
    Failed { failure: FailureRecord },
    /// Device has been permanently removed from the mesh.
    Evicted,
}

/// A mesh member — one storage device participating in consensus.
#[derive(Clone, Debug)]
pub struct MeshMember {
    pub device_id: DeviceId,
    pub state: DeviceState,
    /// The highest generation this device has confirmed.
    pub confirmed_generation: u64,
    /// Vendor identifier — mesh requires dual-vendor minimum to avoid correlated firmware failures.
    pub vendor: Vec<u8>,
}

/// The result of a mesh vote on a proposed commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshVote {
    /// Device confirms the commit is valid and written.
    Confirm,
    /// Device rejects the commit (with reason).
    Reject(MeshRejectReason),
    /// Device did not respond within the timeout.
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshRejectReason {
    /// Hash mismatch — device computed a different hash for the content.
    HashMismatch { expected: ObjectHash, got: ObjectHash },
    /// Device is out of space.
    NoSpace,
    /// Device detected a generation conflict.
    GenerationConflict { expected: u64, got: u64 },
    /// Device is in a failed state and cannot accept writes.
    DeviceFailed,
}

/// Mesh consensus result — the outcome of a commit proposal.
#[derive(Clone, Debug)]
pub enum ConsensusResult {
    /// All devices confirmed. Commit is durable.
    Committed { generation: u64 },
    /// Consensus reached but some devices lagging — commit is durable but degraded. Lagging devices will catch up.
    CommittedDegraded {
        generation: u64,
        lagging: Vec<DeviceId>,
    },
    /// Consensus not reached. Commit is NOT durable. The failure records describe what went wrong on each device.
    Failed { failures: Vec<(DeviceId, MeshVote)> },
}

/// The mesh consensus engine.
///
/// Responsible for proposing commits, collecting votes, and determining whether consensus has been achieved.
pub trait MeshEngine {
    /// Propose a commit to all mesh members. Returns consensus result.
    fn propose_commit(&mut self, commit: &CommitRecord) -> ConsensusResult;

    /// Query the current state of all mesh members.
    fn members(&self) -> &[MeshMember];

    /// The current mesh-agreed generation number.
    fn current_generation(&self) -> u64;

    /// Add a device to the mesh. Requires existing mesh consensus.
    fn add_member(&mut self, device_id: DeviceId, vendor: Vec<u8>) -> Result<(), MeshError>;

    /// Remove a device from the mesh. Requires existing mesh consensus.
    fn evict_member(&mut self, device_id: DeviceId) -> Result<(), MeshError>;

    /// Resolve a conflict between two devices that disagree on state. Returns the hash of the winner.
    fn resolve_conflict(
        &self,
        a: &DeviceId,
        b: &DeviceId,
        generation: u64,
    ) -> Result<ObjectHash, MeshError>;

    /// Check whether the mesh meets minimum redundancy requirements (dual SSD, dual vendor).
    fn is_healthy(&self) -> bool;
}

/// Errors from mesh operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    /// Not enough active devices for consensus.
    InsufficientDevices { active: usize, required: usize },
    /// All devices are from the same vendor (violates dual-vendor requirement).
    SingleVendor,
    /// The specified device is not a mesh member.
    UnknownDevice(DeviceId),
    /// Mesh is not mounted (boot arbitration hasn't completed).
    NotMounted,
    /// A conflict could not be resolved automatically.
    UnresolvableConflict,
}
