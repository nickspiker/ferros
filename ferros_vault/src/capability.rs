//! Layer 1 — Capability tokens and delegation chains.
//!
//! Access control in the Ledger is capability-based: possession of a hash
//! IS the credential. There are no user IDs, no group IDs, no permission
//! bits checked by a kernel. If you have the hash, you have access.
//!
//! Capabilities are structured VSF types that can be delegated (creating
//! a child capability with equal or reduced permissions) and revoked
//! (by rotating the salt that produced the parent hash).
//!
//! ## Contrast
//! - BTRFS: POSIX uid/gid/mode stored in btrfs_inode_item (u32 uid,
//!   u32 gid, u32 mode). Root (uid 0) bypasses ALL permission checks.
//!   ACLs via xattrs add complexity but don't change the trust model.
//! - RedoxFS: Unix uid/gid/mode (u32/u32/u16) in Node struct. Root
//!   bypass explicitly coded: `if uid == 0 { return true; }`.
//! - Ledger: No superuser. No uid. No mode bits. The hash chain is the
//!   entire access control system. write_hash → read_hash is derivable;
//!   read_hash → write_hash is computationally infeasible (one-way).

use alloc::vec::Vec;

use crate::hash::{DerivedSeed, ObjectHash, Salt};
use crate::object::{IntoObject, Object};

/// A capability token — the fundamental access credential.
///
/// Holding this token grants the specified permission level to the
/// target object. Tokens are themselves VSF objects stored in the ledger.
#[derive(Clone, Debug)]
pub struct CapabilityToken {
    /// Hash of the object this capability grants access to.
    pub target: ObjectHash,
    /// The permission level this token grants.
    pub level: CapabilityLevel,
    /// The hash that proves this capability (the credential itself).
    pub credential: ObjectHash,
    /// Parent capability this was delegated from, if any.
    pub delegated_from: Option<ObjectHash>,
    /// Constraints on this capability (time bounds, delegation depth, etc.).
    pub constraints: CapabilityConstraints,
    /// Generation when this capability was created.
    pub generation: u64,
}

/// Permission level granted by a capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityLevel {
    /// Can read the object's content.
    Read,
    /// Can execute/invoke the object.
    Exec,
    /// Can create new objects that reference this one (implies Read).
    Write,
}

/// Constraints that can be attached to a capability token.
#[derive(Clone, Debug, Default)]
pub struct CapabilityConstraints {
    /// Maximum number of further delegations allowed. None = unlimited.
    pub max_delegation_depth: Option<u32>,
    /// Current delegation depth (0 = original grant).
    pub current_depth: u32,
    /// Optional expiry generation. Capability invalid after this generation.
    pub expires_at_generation: Option<u64>,
    /// Domain restriction. If set, capability only valid in this domain.
    pub domain_restriction: Option<Vec<u8>>,
}

/// Errors from capability operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    /// The credential hash doesn't match the expected value.
    InvalidCredential,
    /// Attempted to delegate beyond the maximum depth.
    DelegationDepthExceeded,
    /// The capability has expired (generation exceeded).
    Expired { expired_at: u64, current: u64 },
    /// Domain mismatch — capability not valid in the requested domain.
    DomainMismatch,
    /// The parent capability was revoked (salt rotated).
    Revoked,
}

/// The capability engine — validates and delegates capabilities.
pub trait CapabilityEngine {
    /// Verify that a credential hash grants the claimed access level
    /// to the target object.
    fn verify(
        &self,
        token: &CapabilityToken,
        current_generation: u64,
    ) -> Result<(), CapabilityError>;

    /// Delegate a capability to create a child token with equal or
    /// reduced permissions.
    fn delegate(
        &self,
        parent: &CapabilityToken,
        new_level: CapabilityLevel,
        new_constraints: CapabilityConstraints,
    ) -> Result<CapabilityToken, CapabilityError>;

    /// Revoke a capability by rotating its salt. All tokens derived
    /// from this salt become invalid.
    fn revoke(&mut self, target: &ObjectHash, new_salt: Salt) -> Result<(), CapabilityError>;

    /// Look up the current salt for an object (needed for verification).
    fn current_salt(&self, target: &ObjectHash) -> Option<Salt>;

    /// Look up the seeds for deriving read/exec hashes from a write hash.
    fn derived_seeds(&self, target: &ObjectHash) -> Option<(DerivedSeed, DerivedSeed)>;
}

impl IntoObject for CapabilityToken {
    fn into_object(self, domain: &[u8], generation: u64) -> Object {
        // Stub: serialize the token into VSF binary format.
        // Real implementation will use VSF EWE encoding.
        let _ = (domain, generation);
        todo!("VSF serialization of CapabilityToken")
    }
}
