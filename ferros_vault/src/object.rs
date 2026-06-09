//! VSF object types — the fundamental unit of ledger storage.
//!
//! Every piece of data in the Ledger is a VSF object: immutable, content- addressed, and self-describing via elastic-width encoding (EWE).
//!
//! ## Contrast
//! - BTRFS: Items are typed by a u8 in the btrfs_key (INODE_ITEM=1,
//!   DIR_ITEM=12, EXTENT_DATA=108, etc.). Fixed 17-byte keys. Item data lives in leaf nodes with fixed 4K-64K node sizes.
//! - RedoxFS: Nodes are 4096-byte structs with fixed field offsets for
//!   mode, uid, gid, timestamps, and a fixed-layout block pointer array.
//! - Ledger: Objects have no fixed size. VSF EWE means every field is
//!   exactly as wide as its value requires. A 1-byte object takes 1 byte plus its (tiny) header. No padding, no alignment waste.

use alloc::vec::Vec;

use crate::hash::ObjectHash;

/// VSF type tag — identifies the semantic type of a ledger object.
///
/// These correspond to VSF primitive and structured types. `g`, `k`, `h`, `a` are the VSF crypto primitives used at Layer 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum VsfType {
    /// Raw binary data.
    Blob = 0x01,
    /// VSF structured record (key-value).
    Record = 0x02,
    /// VSF sequence (ordered collection).
    Sequence = 0x03,
    /// Capability token (Layer 1 primitive).
    Capability = 0x10,
    /// Mesh commit record (Layer 2).
    MeshCommit = 0x20,
    /// Failure state record (first-class error).
    FailureState = 0x30,
    /// Boot anchor — the mesh-agreed root of the ledger state.
    BootAnchor = 0x40,
}

/// An object's metadata envelope — everything except the content bytes.
///
/// This is what gets stored alongside the content in the object store. The hash is derived from content + metadata fields, not stored separately.
#[derive(Clone, Debug)]
pub struct ObjectMeta {
    /// The object's content-derived hash (its identity and address).
    pub hash: ObjectHash,
    /// VSF type tag.
    pub vsf_type: VsfType,
    /// Object name (may be empty for anonymous objects).
    pub name: Vec<u8>,
    /// Domain this object belongs to (namespace isolation).
    pub domain: Vec<u8>,
    /// Byte length of the content. Redundant (derivable from content) but useful for allocation without reading content.
    pub content_len: u64,
    /// Generation — the mesh commit generation that created this object.
    pub generation: u64,
    /// Parent hash — for delegation chains and object lineage.
    pub parent: Option<ObjectHash>,
}

/// A complete ledger object: metadata envelope + content bytes.
///
/// Immutable after creation. To "modify" an object, create a new one with updated content — it will have a different hash (different identity).
#[derive(Clone, Debug)]
pub struct Object {
    pub meta: ObjectMeta,
    /// The raw content bytes. Interpretation depends on `vsf_type`.
    pub content: Vec<u8>,
}

/// Trait for types that can be serialized into a VSF object.
///
/// This is how higher-level structures (capability tokens, mesh records, failure states) get stored in the ledger — they serialize to Object.
pub trait IntoObject {
    fn into_object(self, domain: &[u8], generation: u64) -> Object;
}

/// Trait for types that can be deserialized from a VSF object.
pub trait FromObject: Sized {
    type Error;
    fn from_object(obj: &Object) -> Result<Self, Self::Error>;
}

/// Builder for constructing objects with the correct hash.
///
/// Ensures the hash is always computed from the canonical representation.
pub struct ObjectBuilder {
    pub vsf_type: VsfType,
    pub name: Vec<u8>,
    pub domain: Vec<u8>,
    pub content: Vec<u8>,
    pub generation: u64,
    pub parent: Option<ObjectHash>,
}

impl ObjectBuilder {
    pub fn new(vsf_type: VsfType) -> Self {
        Self {
            vsf_type,
            name: Vec::new(),
            domain: Vec::new(),
            content: Vec::new(),
            generation: 0,
            parent: None,
        }
    }

    pub fn name(mut self, name: impl Into<Vec<u8>>) -> Self {
        self.name = name.into();
        self
    }

    pub fn domain(mut self, domain: impl Into<Vec<u8>>) -> Self {
        self.domain = domain.into();
        self
    }

    pub fn content(mut self, content: impl Into<Vec<u8>>) -> Self {
        self.content = content.into();
        self
    }

    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    pub fn parent(mut self, parent: ObjectHash) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Finalize the builder, computing the content hash.
    ///
    /// Requires a salt (for permission chain derivation).
    pub fn build(self, salt: &crate::hash::Salt) -> Object {
        let hash = crate::hash::content_hash(
            &self.content,
            &self.name,
            salt,
            &self.domain,
            crate::hash::PermissionLevel::Write,
        );

        Object {
            meta: ObjectMeta {
                hash,
                vsf_type: self.vsf_type,
                name: self.name,
                domain: self.domain,
                content_len: self.content.len() as u64,
                generation: self.generation,
                parent: self.parent,
            },
            content: self.content,
        }
    }
}
