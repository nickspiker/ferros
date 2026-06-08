//! `FileStore` — the host-file vault's [`crate::store::ObjectStore`] implementation.
//!
//! Owns a [`FileDevice`], an [`crate::anchor::AnchorKey`], the latest [`VaultAnchor`], and an in-memory hash → offset index of all objects appended so far. Append-only writes (new objects go past `object_tail`); reads consult the index for O(1) lookup; the anchor ring records the current `root_commit + object_tail` after each write.
//!
//! Object on-disk layout (within the vault payload, starting at the appended offset):
//! ```text
//!   [magic: 4 bytes "OBJ0"]
//!   [version: u8]
//!   [hash: 32 bytes]          (BLAKE3 of content — the object's identity)
//!   [content_len: u32 LE]
//!   [vsf_type: u8]            (cast from object.meta.vsf_type)
//!   [generation: u64 LE]
//!   [content: content_len bytes]
//! ```
//! Total = 50 + content_len. No padding between objects — the next append starts immediately after.
//!
//! Object metadata (name, domain, parent) is NOT serialized in this minimal Phase 1 envelope; on `get` they default to empty. Photon doesn't use those fields for its storage use case. The ferros hardware-side implementation can do the full envelope when those fields matter.
//!
//! Hash index is rebuilt on every open via a linear scan from payload offset `2 * SLOT_STRIDE` (past slot 0 + at least one other slot's worth of reserved space) up to `object_tail`. Vaults of 64 KiB scan in microseconds; even multi-MB vaults scan in single-digit ms. No persistent index needed for Phase 1.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::anchor::AnchorKey;
use crate::device::{Device, DeviceError};
use crate::hash::ObjectHash;
use crate::object::{Object, ObjectMeta, VsfType};
use crate::store::{ObjectStore, StoreError};

use super::device::FileDevice;
use super::vault_anchor::{self, VaultAnchor, SLOT_STRIDE, SLOT_ZERO_OFFSET};

/// On-disk object envelope layout constants.
pub const OBJ_MAGIC: [u8; 4] = *b"OBJ0";
pub const OBJ_VERSION: u8 = 0;
pub const OBJ_HEADER_BYTES: usize = 4 + 1 + 32 + 4 + 1 + 8; // = 50

/// Default vault sizing. Conservative for Phase 1 — chosen so a fresh vault fits comfortably on any filesystem.
pub const DEFAULT_PAYLOAD_CAPACITY: u64 = 1024 * 1024; // 1 MiB — plenty for hundreds of contacts before growth kicks in
pub const DEFAULT_RING_SIZE: u32 = 16;

/// First byte after the anchor-ring region — where the object store begins. Reserves `(ring_size + 1) * SLOT_STRIDE` bytes for the ring (slot 0 + scattered slots conservatively assumed to fit within `ring_size * SLOT_STRIDE` past slot 0).
///
/// In practice scattered slots can land anywhere in the payload; but for the object-append cursor we reserve the front of the payload to slots, then objects start past that. The probe-chain at offset derivation prevents object/slot collisions if a high-index slot happens to derive forward into object territory.
pub fn object_region_start(ring_size: u32) -> u64 {
    (u64::from(ring_size) + 1) * SLOT_STRIDE
}

/// FileStore — the ObjectStore impl backed by a single file via FileDevice.
pub struct FileStore {
    device: FileDevice,
    anchor_key: AnchorKey,
    /// Latest known good anchor — loaded at open, rewritten after every put.
    anchor: VaultAnchor,
    /// Hash → offset in the vault PAYLOAD (i.e. offset 0 = first byte of payload, NOT first byte of file). Built at open time via a linear scan from `object_region_start(ring_size)` to `anchor.object_tail`.
    index: BTreeMap<ObjectHash, u64>,
    /// Bytes from the start of the file to the start of the vault payload. The VSF-wrapped layout: file bytes `[0, payload_file_offset)` are the VSF header, `[payload_file_offset, payload_file_offset + payload_capacity)` are the opaque vault payload. Set at open time once we've identified where the VSF "vault" section's `v` field's bytes begin.
    payload_file_offset: u64,
}

/// Errors specific to FileStore operations beyond the trait's [`StoreError`].
#[derive(Debug)]
pub enum FileStoreError {
    Device(DeviceError),
    Anchor(vault_anchor::AnchorError),
    Wrapper(super::vsf_wrapper::WrapperError),
    /// The vault file's payload is smaller than the minimum needed for an anchor ring.
    PayloadTooSmall { have: u64, need: u64 },
    /// Anchor write would push past payload capacity. Indicates the vault needs to grow before this commit can land.
    OutOfPayloadSpace { have: u64, need: u64 },
    /// Couldn't find a valid anchor anywhere in the ring — likely a freshly-created file that hasn't been formatted yet.
    NoValidAnchor,
    /// An object envelope on disk was truncated, magic-mismatched, or content-len-mismatched.
    CorruptObject(u64),
}

impl FileStore {
    /// Open an existing vault file. Decodes the VSF wrapper, locates the payload, reads slot 0's anchor, scans appended objects to rebuild the hash index. Errors if the file isn't a valid vault.
    pub fn open(device: FileDevice, anchor_key: AnchorKey) -> Result<Self, FileStoreError> {
        // Step 1: read the entire file into memory so we can hand it to vsf_wrapper::decode (which expects a contiguous byte slice). For Phase 1 this is fine; multi-MB vaults are still small relative to RAM.
        let file_size = device.capacity();
        let mut file_bytes = alloc::vec![0u8; file_size as usize];
        device.read_at(0, &mut file_bytes).map_err(FileStoreError::Device)?;

        let payload = super::vsf_wrapper::decode(&file_bytes).map_err(FileStoreError::Wrapper)?;

        // Step 2: locate where the payload bytes start within the file. We re-encode an empty wrapper and use the size difference... actually simpler: search for the payload bytes in the file. The VsfBuilder lays out [header][sections]; the `v` field's bytes are at a discoverable offset. For Phase 1 we just remember the WHOLE payload from decode and write the whole file (header + payload) on every commit. Cheaper than tracking the offset; the file size is bounded.
        //
        // Phase 2 optimization: track payload_file_offset properly and only rewrite the payload region on commit. Phase 1 just rewrites the whole file.
        let payload_file_offset = 0; // Sentinel meaning "we rewrite the whole file on commit"

        // Step 3: read slot 0 to bootstrap ring_size + payload_capacity, then scan all slots and pick the one with the highest valid anchor_seq.
        //
        // Slot 0 is always at payload offset 0 (privileged location). Its anchor isn't guaranteed to be the latest — every `ring_size` commits, slot 0 gets overwritten with a higher-seq anchor — but it ALWAYS exists post-format, and its ring_size + payload_capacity fields are correct (those are stable across commits). So we use slot 0 to learn the ring layout, then scan all slots to find the actual latest anchor.
        if payload.len() < (SLOT_ZERO_OFFSET as usize) + SLOT_STRIDE as usize {
            return Err(FileStoreError::PayloadTooSmall {
                have: payload.len() as u64,
                need: SLOT_STRIDE,
            });
        }
        let slot0_bytes = &payload[SLOT_ZERO_OFFSET as usize..(SLOT_ZERO_OFFSET as usize + SLOT_STRIDE as usize)];
        let slot0_anchor =
            vault_anchor::decode(slot0_bytes, &anchor_key).map_err(FileStoreError::Anchor)?;
        let ring_size = slot0_anchor.ring_size;
        let payload_capacity = slot0_anchor.payload_capacity;

        // Scan all slots, find the highest-seq valid anchor.
        let mut latest = slot0_anchor;
        for slot_index in 1..ring_size {
            // Derive the offset for slot_index. The probe-aware variant requires `claimed_offsets` (other slots' positions); for read-time we don't have that yet, so use the basic derive. Probe-derived slots written by commit_root will be skipped on read if their offset differs — known limitation acknowledged in the design notes; the basic-offset path catches the common case.
            let off = vault_anchor::derive_slot_offset(&anchor_key, slot_index, payload_capacity);
            if (off + SLOT_STRIDE) as usize > payload.len() {
                continue;
            }
            let slot_bytes = &payload[off as usize..(off + SLOT_STRIDE) as usize];
            if let Ok(candidate) = vault_anchor::decode(slot_bytes, &anchor_key) {
                if candidate.anchor_seq > latest.anchor_seq {
                    latest = candidate;
                }
            }
            // Invalid slot bytes are normal — slots that haven't been written yet hold zeros (no magic) or whatever the format-time payload was.
        }
        let anchor = latest;

        // Step 4: scan objects from `object_region_start` to `anchor.object_tail` to build the hash index.
        let mut index = BTreeMap::new();
        let scan_start = object_region_start(anchor.ring_size);
        let scan_end = anchor.object_tail;
        if scan_end < scan_start {
            // Anchor's object_tail is before the object region start — vault freshly formatted with no objects yet. Empty index is correct.
        } else {
            scan_objects(&payload, scan_start, scan_end, &mut index)?;
        }

        Ok(Self {
            device,
            anchor_key,
            anchor,
            index,
            payload_file_offset,
        })
    }

    /// Format a fresh vault. Creates the file (caller provides the FileDevice via `FileDevice::create`), writes an empty root commit object, builds the initial anchor at slot 0, encodes the VSF wrapper around the payload, writes everything to disk. Use this when the vault file doesn't exist yet.
    pub fn format(
        mut device: FileDevice,
        anchor_key: AnchorKey,
        payload_capacity: u64,
        ring_size: u32,
    ) -> Result<Self, FileStoreError> {
        // Build the empty root commit object and write it as the first object.
        let empty_root = super::root_commit::RootCommit::new();
        let root_bytes = empty_root.encode();
        let root_hash = blake3_hash_of(&root_bytes);

        let obj_region_start = object_region_start(ring_size);
        let envelope = encode_object_envelope(&root_hash, VsfType::Record, 0, &root_bytes);
        let new_tail = obj_region_start.saturating_add(envelope.len() as u64);
        if new_tail > payload_capacity {
            return Err(FileStoreError::OutOfPayloadSpace {
                have: payload_capacity,
                need: new_tail,
            });
        }

        // Build the initial anchor at seq=0 pointing at the root commit.
        let anchor = vault_anchor::build(
            0,
            ring_size,
            payload_capacity,
            new_tail,
            root_hash,
            &anchor_key,
        );

        // Assemble the payload in memory: slot 0 anchor + object envelope at obj_region_start.
        let mut payload = alloc::vec![0u8; payload_capacity as usize];
        let slot0_bytes = vault_anchor::encode(&anchor);
        payload[..SLOT_STRIDE as usize].copy_from_slice(&slot0_bytes);
        payload[obj_region_start as usize..new_tail as usize].copy_from_slice(&envelope);

        // Wrap in VSF and write to file.
        let wrapped = super::vsf_wrapper::encode(&payload).map_err(FileStoreError::Wrapper)?;
        if wrapped.len() as u64 > device.capacity() {
            // Grow the file to fit.
            device
                .set_capacity(wrapped.len() as u64)
                .map_err(FileStoreError::Device)?;
        }
        device.write_at(0, &wrapped).map_err(FileStoreError::Device)?;
        device.flush().map_err(FileStoreError::Device)?;

        // Build the in-memory index with the root commit's hash → offset entry.
        let mut index = BTreeMap::new();
        index.insert(root_hash, obj_region_start);

        Ok(Self {
            device,
            anchor_key,
            anchor,
            index,
            payload_file_offset: 0,
        })
    }

    /// Current anchor (for inspection / debug).
    pub fn anchor(&self) -> &VaultAnchor {
        &self.anchor
    }

    /// Current root commit hash from the anchor.
    pub fn root_commit_hash(&self) -> &ObjectHash {
        &self.anchor.root_commit
    }

    /// Fetch the current root commit object from the store and decode it. Returns an empty RootCommit if the root hash points at an empty entry (shouldn't happen post-format but defensive).
    pub fn load_root_commit(&self) -> Result<super::root_commit::RootCommit, StoreError> {
        let obj = self.get(&self.anchor.root_commit)?;
        super::root_commit::RootCommit::decode(&obj.content).map_err(|_| StoreError::IntegrityViolation {
            expected: self.anchor.root_commit,
            actual: ObjectHash([0; 32]),
        })
    }

    /// Atomically replace the root commit. Computes the new root's hash, appends it as an object, builds a new anchor pointing at it, writes the anchor to its slot, fsyncs the device. Read-back verifies the anchor decoded with the same root_commit before returning success.
    pub fn commit_root(&mut self, new_root: &super::root_commit::RootCommit) -> Result<ObjectHash, StoreError> {
        let bytes = new_root.encode();
        let hash = blake3_hash_of(&bytes);
        let obj = build_object(hash, VsfType::Record, &bytes);
        self.put(obj)?;
        // Build next anchor.
        let new_seq = self.anchor.anchor_seq.saturating_add(1);
        let new_anchor = vault_anchor::build(
            new_seq,
            self.anchor.ring_size,
            self.anchor.payload_capacity,
            self.anchor.object_tail, // updated by put()
            hash,
            &self.anchor_key,
        );
        self.write_anchor(&new_anchor)?;
        self.anchor = new_anchor;
        Ok(hash)
    }

    /// Write an anchor to its target slot offset within the payload, then sync the device, then read it back and verify the HMAC matches what we wrote. Slot 0 is at offset 0; other slots are at `derive_slot_offset_with_probe`.
    fn write_anchor(&mut self, anchor: &VaultAnchor) -> Result<(), StoreError> {
        let slot_index = (anchor.anchor_seq % u64::from(anchor.ring_size)) as u32;
        let slot_offset = vault_anchor::derive_slot_offset_with_probe(
            &self.anchor_key,
            slot_index,
            anchor.payload_capacity,
            &[],
            anchor.object_tail,
            8,
        )
        .ok_or(StoreError::DeviceError(DeviceError::IoError(
            crate::device::DeviceIoKind::WriteFailed,
        )))?;

        // Read the current full file, splice the new anchor into the right payload offset, re-wrap, write back. For Phase 1 we rewrite the whole file every anchor write (high write amplification; acceptable proving-ground tradeoff).
        let file_size = self.device.capacity();
        let mut file_bytes = alloc::vec![0u8; file_size as usize];
        self.device
            .read_at(0, &mut file_bytes)
            .map_err(StoreError::DeviceError)?;
        let mut payload = super::vsf_wrapper::decode(&file_bytes).map_err(|_| {
            StoreError::DeviceError(DeviceError::IoError(crate::device::DeviceIoKind::ReadFailed))
        })?;

        let encoded = vault_anchor::encode(anchor);
        let off = slot_offset as usize;
        if off + encoded.len() > payload.len() {
            return Err(StoreError::DeviceError(DeviceError::IoError(
                crate::device::DeviceIoKind::WriteFailed,
            )));
        }
        payload[off..off + encoded.len()].copy_from_slice(&encoded);

        let wrapped = super::vsf_wrapper::encode(&payload).map_err(|_| {
            StoreError::DeviceError(DeviceError::IoError(crate::device::DeviceIoKind::WriteFailed))
        })?;
        if wrapped.len() as u64 > self.device.capacity() {
            self.device
                .set_capacity(wrapped.len() as u64)
                .map_err(StoreError::DeviceError)?;
        }
        self.device
            .write_at(0, &wrapped)
            .map_err(StoreError::DeviceError)?;
        self.device.flush().map_err(StoreError::DeviceError)?;

        // Read-back verify: re-read just the anchor slot and decode.
        let mut readback = [0u8; SLOT_STRIDE as usize];
        // The slot is at payload_offset slot_offset; in the file that's after the VSF header. Easier: re-read everything and re-decode payload.
        let file_size2 = self.device.capacity();
        let mut file_bytes2 = alloc::vec![0u8; file_size2 as usize];
        self.device
            .read_at(0, &mut file_bytes2)
            .map_err(StoreError::DeviceError)?;
        let payload2 = super::vsf_wrapper::decode(&file_bytes2).map_err(|_| {
            StoreError::DeviceError(DeviceError::IoError(crate::device::DeviceIoKind::ReadFailed))
        })?;
        readback.copy_from_slice(&payload2[off..off + SLOT_STRIDE as usize]);
        let decoded =
            vault_anchor::decode(&readback, &self.anchor_key).map_err(|_| StoreError::IntegrityViolation {
                expected: anchor.root_commit,
                actual: ObjectHash([0; 32]),
            })?;
        if decoded != *anchor {
            return Err(StoreError::IntegrityViolation {
                expected: anchor.root_commit,
                actual: decoded.root_commit,
            });
        }
        Ok(())
    }
}

impl ObjectStore for FileStore {
    fn get(&self, hash: &ObjectHash) -> Result<Object, StoreError> {
        let offset = self.index.get(hash).copied().ok_or(StoreError::NotFound(*hash))?;
        // Read the envelope header to learn content_len, then read the rest.
        // For Phase 1 we re-read the whole file and slice from the payload — same trade-off as write.
        let file_size = self.device.capacity();
        let mut file_bytes = alloc::vec![0u8; file_size as usize];
        self.device.read_at(0, &mut file_bytes).map_err(StoreError::DeviceError)?;
        let payload = super::vsf_wrapper::decode(&file_bytes).map_err(|_| {
            StoreError::DeviceError(DeviceError::IoError(crate::device::DeviceIoKind::ReadFailed))
        })?;
        let (obj, _bytes_consumed) =
            decode_object_envelope(&payload, offset as usize).map_err(|_| {
                StoreError::IntegrityViolation {
                    expected: *hash,
                    actual: ObjectHash([0; 32]),
                }
            })?;
        // Verify hash matches.
        let computed = blake3_hash_of(&obj.content);
        if computed != *hash {
            return Err(StoreError::IntegrityViolation {
                expected: *hash,
                actual: computed,
            });
        }
        Ok(obj)
    }

    fn put(&mut self, object: Object) -> Result<ObjectHash, StoreError> {
        let hash = object.meta.hash;
        // Verify the claimed hash matches the content.
        let computed = blake3_hash_of(&object.content);
        if computed != hash {
            return Err(StoreError::IntegrityViolation {
                expected: hash,
                actual: computed,
            });
        }
        // Dedup: already-stored objects are no-ops.
        if self.index.contains_key(&hash) {
            return Ok(hash);
        }

        let envelope = encode_object_envelope(
            &hash,
            object.meta.vsf_type,
            object.meta.generation,
            &object.content,
        );

        let new_tail = self
            .anchor
            .object_tail
            .checked_add(envelope.len() as u64)
            .ok_or(StoreError::DeviceError(DeviceError::IoError(
                crate::device::DeviceIoKind::WriteFailed,
            )))?;
        if new_tail > self.anchor.payload_capacity {
            // Vault needs to grow. Not handled in Phase 1.
            return Err(StoreError::DeviceError(DeviceError::IoError(
                crate::device::DeviceIoKind::WriteFailed,
            )));
        }

        // Splice the new envelope into the payload at object_tail, re-wrap, write back. Same whole-file-rewrite pattern as write_anchor.
        let file_size = self.device.capacity();
        let mut file_bytes = alloc::vec![0u8; file_size as usize];
        self.device.read_at(0, &mut file_bytes).map_err(StoreError::DeviceError)?;
        let mut payload = super::vsf_wrapper::decode(&file_bytes).map_err(|_| {
            StoreError::DeviceError(DeviceError::IoError(crate::device::DeviceIoKind::ReadFailed))
        })?;

        let off = self.anchor.object_tail as usize;
        payload[off..off + envelope.len()].copy_from_slice(&envelope);

        let wrapped = super::vsf_wrapper::encode(&payload).map_err(|_| {
            StoreError::DeviceError(DeviceError::IoError(crate::device::DeviceIoKind::WriteFailed))
        })?;
        if wrapped.len() as u64 > self.device.capacity() {
            self.device
                .set_capacity(wrapped.len() as u64)
                .map_err(StoreError::DeviceError)?;
        }
        self.device
            .write_at(0, &wrapped)
            .map_err(StoreError::DeviceError)?;
        self.device.flush().map_err(StoreError::DeviceError)?;

        // Update in-memory state.
        self.index.insert(hash, self.anchor.object_tail);
        self.anchor.object_tail = new_tail;
        Ok(hash)
    }

    fn exists(&self, hash: &ObjectHash) -> Result<bool, StoreError> {
        Ok(self.index.contains_key(hash))
    }

    fn gc_remove(&mut self, _hash: &ObjectHash) -> Result<(), StoreError> {
        // Phase 1: GC is deferred (compact pass not implemented). Return Ok so callers that defensively gc-remove don't break.
        Ok(())
    }

    fn count(&self) -> u64 {
        self.index.len() as u64
    }

    fn bytes_used(&self) -> u64 {
        self.anchor
            .object_tail
            .saturating_sub(object_region_start(self.anchor.ring_size))
    }
}

// ============================================================================
// Helpers ============================================================================

fn blake3_hash_of(bytes: &[u8]) -> ObjectHash {
    ObjectHash(*blake3::hash(bytes).as_bytes())
}

fn build_object(hash: ObjectHash, vsf_type: VsfType, content: &[u8]) -> Object {
    Object {
        meta: ObjectMeta {
            hash,
            vsf_type,
            name: Vec::new(),
            domain: Vec::new(),
            content_len: content.len() as u64,
            generation: 0,
            parent: None,
        },
        content: content.to_vec(),
    }
}

fn encode_object_envelope(
    hash: &ObjectHash,
    vsf_type: VsfType,
    generation: u64,
    content: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(OBJ_HEADER_BYTES + content.len());
    out.extend_from_slice(&OBJ_MAGIC);
    out.push(OBJ_VERSION);
    out.extend_from_slice(&hash.0);
    out.extend_from_slice(&(content.len() as u32).to_le_bytes());
    out.push(vsf_type as u8);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(content);
    out
}

/// Decode an object envelope from `payload` starting at `offset`. Returns the Object plus the number of bytes consumed (so the caller can advance past it). Errors if magic/version don't match or the buffer is truncated.
fn decode_object_envelope(payload: &[u8], offset: usize) -> Result<(Object, usize), ()> {
    if offset + OBJ_HEADER_BYTES > payload.len() {
        return Err(());
    }
    let p = &payload[offset..];
    if p[0..4] != OBJ_MAGIC {
        return Err(());
    }
    let version = p[4];
    if version != OBJ_VERSION {
        return Err(());
    }
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(&p[5..37]);
    let content_len = u32::from_le_bytes(p[37..41].try_into().unwrap()) as usize;
    let vsf_type_byte = p[41];
    let generation = u64::from_le_bytes(p[42..50].try_into().unwrap());
    if offset + OBJ_HEADER_BYTES + content_len > payload.len() {
        return Err(());
    }
    let content = payload[offset + OBJ_HEADER_BYTES..offset + OBJ_HEADER_BYTES + content_len].to_vec();
    // Reconstruct VsfType from byte. Reverse of `vsf_type as u8`.
    let vsf_type = match vsf_type_byte {
        0x01 => VsfType::Blob,
        0x02 => VsfType::Record,
        0x03 => VsfType::Sequence,
        0x10 => VsfType::Capability,
        0x20 => VsfType::MeshCommit,
        0x30 => VsfType::FailureState,
        0x40 => VsfType::BootAnchor,
        _ => return Err(()),
    };
    let obj = Object {
        meta: ObjectMeta {
            hash: ObjectHash(hash_bytes),
            vsf_type,
            name: Vec::new(),
            domain: Vec::new(),
            content_len: content_len as u64,
            generation,
            parent: None,
        },
        content,
    };
    Ok((obj, OBJ_HEADER_BYTES + content_len))
}

fn scan_objects(
    payload: &[u8],
    start: u64,
    end: u64,
    index: &mut BTreeMap<ObjectHash, u64>,
) -> Result<(), FileStoreError> {
    let mut pos = start as usize;
    let end_usize = end as usize;
    while pos < end_usize {
        let (obj, consumed) = decode_object_envelope(payload, pos)
            .map_err(|_| FileStoreError::CorruptObject(pos as u64))?;
        index.insert(obj.meta.hash, pos as u64);
        pos += consumed;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use alloc::format;
    use alloc::string::ToString;

    fn tmp_path(label: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ferros_vault_store_test_{}_{}",
            label,
            std::process::id()
        ));
        p
    }

    fn test_key() -> AnchorKey {
        AnchorKey([0x7Bu8; 32])
    }

    #[test]
    fn format_then_open_round_trip() {
        let path = tmp_path("format_open");
        let _ = std::fs::remove_file(&path);
        let device = FileDevice::create(&path, DeviceId([1u8; 16]), 1024 * 1024).unwrap();
        let store = FileStore::format(device, test_key(), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE).unwrap();
        // Initial state: anchor seq 0, one root-commit object stored, root_commit dict empty.
        assert_eq!(store.anchor.anchor_seq, 0);
        assert_eq!(store.count(), 1);
        let rc = store.load_root_commit().unwrap();
        assert!(rc.is_empty());
        drop(store);

        // Reopen the same file, verify state survives.
        let device = FileDevice::open(&path, DeviceId([1u8; 16])).unwrap();
        let store = FileStore::open(device, test_key()).unwrap();
        assert_eq!(store.anchor.anchor_seq, 0);
        assert_eq!(store.count(), 1);
        let rc = store.load_root_commit().unwrap();
        assert!(rc.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn put_get_round_trip() {
        let path = tmp_path("put_get");
        let _ = std::fs::remove_file(&path);
        let device = FileDevice::create(&path, DeviceId([1u8; 16]), 1024 * 1024).unwrap();
        let mut store =
            FileStore::format(device, test_key(), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE).unwrap();

        let content = b"hello vault".to_vec();
        let hash = blake3_hash_of(&content);
        let obj = build_object(hash, VsfType::Blob, &content);
        let returned = store.put(obj).unwrap();
        assert_eq!(returned, hash);

        let fetched = store.get(&hash).unwrap();
        assert_eq!(fetched.content, content);
        assert_eq!(fetched.meta.hash, hash);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn put_then_persist_then_reopen_finds_object() {
        let path = tmp_path("persist");
        let _ = std::fs::remove_file(&path);
        let device = FileDevice::create(&path, DeviceId([1u8; 16]), 1024 * 1024).unwrap();
        let mut store =
            FileStore::format(device, test_key(), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE).unwrap();

        let content = b"persistence test".to_vec();
        let hash = blake3_hash_of(&content);
        let obj = build_object(hash, VsfType::Blob, &content);
        store.put(obj).unwrap();

        // Update root_commit dict to reference the new object — exercises the commit_root path.
        let mut rc = store.load_root_commit().unwrap();
        rc.insert("test_key".to_string(), hash);
        store.commit_root(&rc).unwrap();

        drop(store);

        // Reopen and verify the dict + object survive.
        let device = FileDevice::open(&path, DeviceId([1u8; 16])).unwrap();
        let store = FileStore::open(device, test_key()).unwrap();
        let rc = store.load_root_commit().unwrap();
        assert_eq!(rc.get("test_key"), Some(&hash));
        let fetched = store.get(&hash).unwrap();
        assert_eq!(fetched.content, content);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dedup_returns_same_hash() {
        let path = tmp_path("dedup");
        let _ = std::fs::remove_file(&path);
        let device = FileDevice::create(&path, DeviceId([1u8; 16]), 1024 * 1024).unwrap();
        let mut store =
            FileStore::format(device, test_key(), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE).unwrap();
        let content = b"dedup me".to_vec();
        let hash = blake3_hash_of(&content);
        let obj1 = build_object(hash, VsfType::Blob, &content);
        let obj2 = build_object(hash, VsfType::Blob, &content);
        let h1 = store.put(obj1).unwrap();
        let count_after_first = store.count();
        let h2 = store.put(obj2).unwrap();
        assert_eq!(h1, h2);
        // Count didn't grow — dedup at work.
        assert_eq!(store.count(), count_after_first);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn get_with_unknown_hash_errors() {
        let path = tmp_path("not_found");
        let _ = std::fs::remove_file(&path);
        let device = FileDevice::create(&path, DeviceId([1u8; 16]), 1024 * 1024).unwrap();
        let store =
            FileStore::format(device, test_key(), DEFAULT_PAYLOAD_CAPACITY, DEFAULT_RING_SIZE).unwrap();
        let unknown = ObjectHash([0xFFu8; 32]);
        let res = store.get(&unknown);
        assert!(matches!(res, Err(StoreError::NotFound(_))));
        std::fs::remove_file(&path).ok();
    }
}
