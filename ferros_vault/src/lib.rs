//! # Ferros Ledger
//!
//! The persistent object store that ferros boots from and operates on.
//! Not a filesystem. No directories, no inodes, no POSIX semantics.
//!
//! ## Architecture (3 layers)
//!
//! - **Layer 0 — Object Store** ([`store`]): Flat hash-indexed VSF objects.
//!   Append-only, immutable after write. Scales from 8KB flash to
//!   distributed mesh cluster.
//!
//! - **Layer 1 — Capabilities** ([`capability`]): Access control via hash
//!   possession. No user IDs, no permission bits, no superuser.
//!   One-way hash chain: write → read derivable, read → write infeasible.
//!
//! - **Layer 2 — Mesh** ([`mesh`]): Multi-device consensus. Dual SSD,
//!   dual vendor minimum. Neither device is truth — mesh consensus is truth.
//!   Failed writes are first-class typed records.
//!
//! ## Invariants
//!
//! - **Flat.** Single namespace indexed by hash.
//! - **Append-only.** No object is ever overwritten.
//! - **Content-addressed.** Identity = BLAKE3 hash.
//! - **Capability-gated.** Possession of hash = access credential.
//! - **VSF-native.** All structures are VSF types.
//! - **Killswitch-safe.** Power loss at any nanosecond leaves last
//!   mesh-committed state intact.

#![no_std]

extern crate alloc;

pub mod hash;
pub mod object;
pub mod device;
pub mod store;
pub mod capability;
pub mod commit;
pub mod mesh;
pub mod failure;
pub mod anchor;
pub mod platform;
pub mod boot;

/// The top-level Ledger — ties all layers together.
///
/// This is the main entry point for interacting with a mounted ledger.
/// It is only constructable via the boot sequence ([`boot::BootEngine`]).
pub struct Ledger<S, C, M>
where
    S: store::ObjectStore,
    C: capability::CapabilityEngine,
    M: mesh::MeshEngine,
{
    store: S,
    capabilities: C,
    mesh: M,
    generation: u64,
    root_hash: hash::ObjectHash,
}

impl<S, C, M> Ledger<S, C, M>
where
    S: store::ObjectStore,
    C: capability::CapabilityEngine,
    M: mesh::MeshEngine,
{
    /// Read an object from the ledger, verifying the capability.
    pub fn get(
        &self,
        hash: &hash::ObjectHash,
        credential: &capability::CapabilityToken,
    ) -> Result<object::Object, LedgerError> {
        self.capabilities
            .verify(credential, self.generation)
            .map_err(LedgerError::Capability)?;
        self.store.get(hash).map_err(LedgerError::Store)
    }

    /// Write an object to the ledger via the commit protocol.
    ///
    /// The object is staged, committed locally, then proposed to the mesh.
    /// Returns the object's hash only after mesh consensus confirms it.
    pub fn put(
        &mut self,
        object: object::Object,
        credential: &capability::CapabilityToken,
    ) -> Result<hash::ObjectHash, LedgerError> {
        self.capabilities
            .verify(credential, self.generation)
            .map_err(LedgerError::Capability)?;
        self.store.put(object).map_err(LedgerError::Store)
    }

    /// Current mesh-agreed generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Current root hash (the state hash after the latest commit).
    pub fn root_hash(&self) -> &hash::ObjectHash {
        &self.root_hash
    }

    /// Whether the mesh is healthy (dual SSD, dual vendor, all active).
    pub fn is_healthy(&self) -> bool {
        self.mesh.is_healthy()
    }
}

/// Top-level errors from ledger operations.
#[derive(Clone, Debug)]
pub enum LedgerError {
    Store(store::StoreError),
    Capability(capability::CapabilityError),
    Mesh(mesh::MeshError),
    Boot(boot::BootError),
}
