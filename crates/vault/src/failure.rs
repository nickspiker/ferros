//! First-class failure state records.
//!
//! In the Ledger, failures are not exceptions to be caught and discarded. They are typed VSF objects stored in the ledger itself — queryable, inspectable, and part of the permanent record.
//!
//! A failed write doesn't disappear — it becomes a FailureRecord that the mesh can reason about during arbitration.
//!
//! ## Contrast
//! - BTRFS: Errors surface as kernel log messages and errno returns.
//!   Failed writes in a transaction cause the entire transaction to abort — the failure itself is not recorded on disk. RAID scrub can detect and repair corruption, but the failure event is ephemeral.
//! - RedoxFS: Errors returned as `syscall::Error`. Failed writes cause
//!   the transaction to not commit (generation not incremented). No on-disk record of what failed or why.
//! - Ledger: FailureRecord is a VSF object with a hash, stored in the
//!   ledger. You can query "what failures occurred at generation N?" The mesh uses these records to make informed arbitration decisions.

use alloc::vec::Vec;

use crate::device::DeviceId;
use crate::hash::ObjectHash;
use crate::object::{IntoObject, Object};

/// A failure record — first-class typed VSF object describing a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureRecord {
    /// What kind of failure occurred.
    pub kind: FailureKind,
    /// Which device experienced the failure.
    pub device: DeviceId,
    /// The generation during which the failure occurred.
    pub generation: u64,
    /// The object hash that was being written/read when the failure happened, if applicable.
    pub related_object: Option<ObjectHash>,
    /// The commit hash that was in progress when the failure happened, if applicable.
    pub related_commit: Option<ObjectHash>,
    /// Raw device-level error context (firmware error codes, etc.).
    pub device_context: Vec<u8>,
}

/// Categories of failures the Ledger tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// Write did not complete — device reported error during write.
    WriteFailed,
    /// Write completed but verification read returned different data.
    WriteVerifyMismatch,
    /// Read returned data that doesn't match the expected hash.
    ReadCorruption {
        expected: ObjectHash,
        actual: ObjectHash,
    },
    /// Device became unreachable during an operation.
    DeviceUnreachable,
    /// Device reported a hardware fault (SMART failure, etc.).
    HardwareFault,
    /// Mesh consensus detected this device diverged from agreed state.
    StateDivergence {
        device_generation: u64,
        mesh_generation: u64,
    },
    /// Power loss detected during write (device came back with incomplete write evidence).
    PowerLossDuringWrite,
    /// Unknown failure — device_context contains raw error data.
    Unknown,
}

/// Trait for querying failure history.
pub trait FailureLog {
    /// All failure records for a given device.
    fn failures_for_device(&self, device: &DeviceId) -> Vec<FailureRecord>;

    /// All failure records at a given generation.
    fn failures_at_generation(&self, generation: u64) -> Vec<FailureRecord>;

    /// All failure records related to a specific object.
    fn failures_for_object(&self, hash: &ObjectHash) -> Vec<FailureRecord>;

    /// Record a new failure. Returns the hash of the failure record (it is itself a ledger object).
    fn record_failure(&mut self, failure: FailureRecord) -> ObjectHash;

    /// Total number of recorded failures.
    fn failure_count(&self) -> u64;
}

impl IntoObject for FailureRecord {
    fn into_object(self, domain: &[u8], generation: u64) -> Object {
        let _ = (domain, generation);
        todo!("VSF serialization of FailureRecord")
    }
}
