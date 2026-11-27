//! Atomic commit protocol — Copy-on-Write semantics for the Ledger.
//!
//! A commit is the atomic unit of state change. New objects are written to free space, then a commit record is proposed to the mesh. The commit either succeeds atomically (all mesh members confirm) or does not happen at all. There is no partial state.
//!
//! ## Contrast
//! - BTRFS: Transaction batches writes over ~30 seconds, then CoW-updates
//!   the tree and overwrites the superblock. The superblock overwrite is the ONE exception to CoW — a torn superblock write is catastrophic. Recovery: replay log tree from last valid superblock.
//! - RedoxFS: Individual CoW writes, each incrementing a generation counter.
//!   Header written to ring slot (gen % 256). Crash = scan ring for newest valid header. Simple and effective but single-device only.
//! - Ledger: No superblock overwrite ever. Commits are mesh-agreed records.
//!   The commit record itself is an immutable VSF object in the ledger. Crash at any point = mesh re-arbitrates from each device's last confirmed generation. No torn writes possible.

use alloc::vec::Vec;

use crate::hash::ObjectHash;
use crate::device::DeviceId;

/// A commit record — describes one atomic state transition.
///
/// This is itself a VSF object stored in the ledger, making the commit history a first-class queryable data structure.
#[derive(Clone, Debug)]
pub struct CommitRecord {
    /// Hash of this commit record itself.
    pub hash: ObjectHash,
    /// The generation number this commit creates.
    pub generation: u64,
    /// Hash of the previous commit (forms a hash chain).
    pub parent_commit: ObjectHash,
    /// Hashes of all new objects added in this commit.
    pub new_objects: Vec<ObjectHash>,
    /// Hashes of objects marked for garbage collection in this commit.
    pub gc_objects: Vec<ObjectHash>,
    /// The root hash — the hash of the "state object" that represents the entire ledger state after this commit.
    pub root_hash: ObjectHash,
    /// Which device proposed this commit.
    pub proposer: DeviceId,
    /// Timestamp (monotonic, not wall-clock — no NTP dependency).
    pub timestamp_monotonic: u64,
}

/// A pending commit — accumulates writes before proposing to the mesh.
///
/// This is the in-memory staging area. Nothing is durable until the commit is proposed and accepted by the mesh.
pub struct PendingCommit {
    /// Objects staged for inclusion in this commit.
    pub staged_objects: Vec<crate::object::Object>,
    /// Objects staged for garbage collection.
    pub gc_candidates: Vec<ObjectHash>,
    /// The generation this commit will create if accepted.
    pub target_generation: u64,
    /// The parent commit this builds on.
    pub parent_commit: ObjectHash,
}

impl PendingCommit {
    /// Create a new pending commit building on the given parent.
    pub fn new(parent_commit: ObjectHash, target_generation: u64) -> Self {
        Self {
            staged_objects: Vec::new(),
            gc_candidates: Vec::new(),
            target_generation,
            parent_commit,
        }
    }

    /// Stage an object for inclusion in this commit.
    pub fn stage(&mut self, object: crate::object::Object) {
        self.staged_objects.push(object);
    }

    /// Stage an object hash for garbage collection.
    pub fn stage_gc(&mut self, hash: ObjectHash) {
        self.gc_candidates.push(hash);
    }
}

/// The commit engine — manages the lifecycle of commits.
pub trait CommitEngine {
    /// Begin a new pending commit.
    fn begin(&mut self) -> PendingCommit;

    /// Finalize a pending commit into a CommitRecord. This computes hashes but does NOT yet propose to the mesh.
    fn finalize(&self, pending: PendingCommit) -> CommitRecord;

    /// Write staged objects to the local device (pre-mesh). Objects are written but not yet committed — they become durable only after mesh consensus.
    fn write_staged(
        &mut self,
        commit: &CommitRecord,
    ) -> Result<(), crate::store::StoreError>;

    /// Roll back a failed commit — discard staged objects that were written but not mesh-confirmed.
    fn rollback(&mut self, commit: &CommitRecord) -> Result<(), crate::store::StoreError>;

    /// Retrieve the commit chain — all commits from the given generation back to the genesis commit.
    fn history(&self, from_generation: u64) -> Vec<CommitRecord>;

    /// The current confirmed generation.
    fn current_generation(&self) -> u64;
}
