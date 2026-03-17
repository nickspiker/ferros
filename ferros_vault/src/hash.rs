//! BLAKE3 content addressing and permission hash chain.
//!
//! Every object in the Ledger is addressed by a 32-byte BLAKE3 hash of:
//!   `BLAKE3(content || name || salt || domain || permission_level)`
//!
//! The permission hash chain derives read/exec hashes from the write hash:
//!   write_hash = BLAKE3(content || name || salt || domain || "write")
//!   read_hash  = BLAKE3(write_hash || read_seed)
//!   exec_hash  = BLAKE3(write_hash || exec_seed)
//!
//! Write implies read (derivable). Read cannot escalate to write (one-way).
//! Revocation = rotate salt → all derived hashes become invalid.
//!
//! ## Contrast
//! - BTRFS: CRC32c checksums stored in a *separate* csum tree. Addressing
//!   is by (objectid, type, offset) — identity and integrity are decoupled.
//! - RedoxFS: SeaHash (64-bit) embedded in BlockPtr. Better coupling, but
//!   64-bit is not collision-resistant and still separates address from hash.
//! - Ledger: BLAKE3 IS the address. No separate checksum. If you can find
//!   the object by hash, it's valid. Verification is free.

use alloc::format;
use alloc::string::String;

use blake3;

/// 32-byte BLAKE3 hash — the universal identifier for all ledger objects.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ObjectHash(pub [u8; 32]);

impl ObjectHash {
    pub const ZERO: Self = Self([0u8; 32]);

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl core::fmt::Debug for ObjectHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ObjectHash({})", hex_prefix(&self.0))
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    format!("G#{}", bytes.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>())
}

/// Domain separator for permission levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionLevel {
    Write,
    Read,
    Exec,
}

impl PermissionLevel {
    pub fn as_bytes(&self) -> &'static [u8] {
        match self {
            Self::Write => b"write",
            Self::Read => b"read",
            Self::Exec => b"exec",
        }
    }
}

/// Salt for hash derivation. Rotating the salt revokes all derived hashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Salt(pub [u8; 32]);

/// Seed used to derive read or exec hashes from a write hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct DerivedSeed(pub [u8; 32]);

/// Compute the primary content hash for an object.
///
/// `BLAKE3(content || name || salt || domain || permission_level)`
pub fn content_hash(
    content: &[u8],
    name: &[u8],
    salt: &Salt,
    domain: &[u8],
    level: PermissionLevel,
) -> ObjectHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(content);
    hasher.update(name);
    hasher.update(&salt.0);
    hasher.update(domain);
    hasher.update(level.as_bytes());
    ObjectHash(*hasher.finalize().as_bytes())
}

/// Derive a read hash from a write hash.
///
/// `read_hash = BLAKE3(write_hash || read_seed)`
pub fn derive_read_hash(write_hash: &ObjectHash, read_seed: &DerivedSeed) -> ObjectHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&write_hash.0);
    hasher.update(&read_seed.0);
    ObjectHash(*hasher.finalize().as_bytes())
}

/// Derive an exec hash from a write hash.
///
/// `exec_hash = BLAKE3(write_hash || exec_seed)`
pub fn derive_exec_hash(write_hash: &ObjectHash, exec_seed: &DerivedSeed) -> ObjectHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&write_hash.0);
    hasher.update(&exec_seed.0);
    ObjectHash(*hasher.finalize().as_bytes())
}

/// The complete set of hashes for an object — write, read, exec.
#[derive(Clone, Debug)]
pub struct PermissionChain {
    pub write: ObjectHash,
    pub read: ObjectHash,
    pub exec: ObjectHash,
}

/// Compute the full permission chain for an object.
pub fn permission_chain(
    content: &[u8],
    name: &[u8],
    salt: &Salt,
    domain: &[u8],
    read_seed: &DerivedSeed,
    exec_seed: &DerivedSeed,
) -> PermissionChain {
    let write = content_hash(content, name, salt, domain, PermissionLevel::Write);
    let read = derive_read_hash(&write, read_seed);
    let exec = derive_exec_hash(&write, exec_seed);
    PermissionChain { write, read, exec }
}
