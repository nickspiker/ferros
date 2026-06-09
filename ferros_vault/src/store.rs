//! Layer 0 — Flat hash-indexed object store.
//!
//! The foundation of the Ledger. All objects live here in a single flat namespace, indexed exclusively by their BLAKE3 hash. No hierarchy, no directory structure, no inode table.
//!
//! ## Contrast
//! - BTRFS: Objects (items) live in B-tree leaves indexed by (objectid,
//!   type, offset). Finding an object requires traversing the tree from root → internal nodes → leaf. Minimum node size: 4K. The tree itself requires ~256 MiB of metadata block group at format time.
//! - RedoxFS: Inodes live in a 4-level tree (256^4 = 4B entries max).
//!   Each level is a block of 256 BlockPtrs. Lookup = 4 block reads. Fixed 4K block size means minimum ~1 MiB overhead.
//! - Ledger: Pure hash table. O(1) lookup. No tree traversal. No fixed
//!   block sizes. The index can be as small as a single entry (8KB flash) or distributed across a mesh cluster. Same interface either way.

use alloc::boxed::Box;

use crate::hash::ObjectHash;
use crate::object::Object;

/// Errors that can occur during store operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// Object not found for the given hash.
    NotFound(ObjectHash),
    /// Hash mismatch — content doesn't match its claimed hash. This indicates corruption or tampering.
    IntegrityViolation {
        expected: ObjectHash,
        actual: ObjectHash,
    },
    /// The underlying device refused the write.
    DeviceError(crate::device::DeviceError),
    /// Mesh consensus was not achieved for this write.
    MeshRejected,
}

/// The object store — Layer 0 of the Ledger.
///
/// This trait defines the fundamental operations. Implementations may target anything from 8KB flash to a distributed mesh cluster.
pub trait ObjectStore {
    /// Retrieve an object by its hash.
    ///
    /// Returns `NotFound` if no object with this hash exists. Returns `IntegrityViolation` if the stored content doesn't match the hash (corruption detected).
    fn get(&self, hash: &ObjectHash) -> Result<Object, StoreError>;

    /// Store an object. The hash is verified against the content before persisting. Returns the object's hash on success.
    ///
    /// This is append-only: if an object with this hash already exists, this is a no-op (content-addressed deduplication).
    fn put(&mut self, object: Object) -> Result<ObjectHash, StoreError>;

    /// Check whether an object with this hash exists, without reading it.
    fn exists(&self, hash: &ObjectHash) -> Result<bool, StoreError>;

    /// Delete an object by hash. This is the ONLY way data is removed, and it requires explicit garbage collection authority.
    ///
    /// In normal operation, objects are never deleted — the ledger is append-only. This exists for garbage collection policies.
    fn gc_remove(&mut self, hash: &ObjectHash) -> Result<(), StoreError>;

    /// Return the total number of objects in the store.
    fn count(&self) -> u64;

    /// Return the total bytes used by stored objects (content + metadata).
    fn bytes_used(&self) -> u64;
}

/// An iterator over all objects in the store.
///
/// No ordering guarantees — hash iteration order is arbitrary.
pub trait ObjectStoreIter {
    fn hashes(&self) -> Box<dyn Iterator<Item = ObjectHash> + '_>;
}

/// Trait for stores that support generation-aware queries.
///
/// Generation = mesh commit generation. Useful for incremental sync.
pub trait GenerationQuery {
    /// Return all object hashes created at or after the given generation.
    fn since_generation(&self, generation: u64) -> Box<dyn Iterator<Item = ObjectHash> + '_>;
}
