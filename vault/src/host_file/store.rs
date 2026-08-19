//! `FileStore` — the host-file vault's [`crate::store::ObjectStore`] implementation.
//!
//! This is now a thin std-side alias over the device-generic backend in [`crate::backend`]: `FileStore = Store<FileDevice>`. All of the vault logic (append-only object envelopes, anchor ring, VSF-wrapped payload, hash index) lives in `crate::backend` and is shared verbatim with the on-hardware `Store<UfsDevice>`. This module only pins the host-file device type and re-exports the backend's public names under the historical `FileStore` / `FileStoreError` paths so existing consumers keep compiling.
//!
//! Object on-disk layout, anchor-slot semantics, and the index-rebuild scan are all documented on [`crate::backend`].

pub use crate::backend::{
    object_region_start, StoreBackendError as FileStoreError, DEFAULT_PAYLOAD_CAPACITY,
    DEFAULT_RING_SIZE, OBJ_HEADER_BYTES, OBJ_MAGIC, OBJ_VERSION,
};

/// FileStore — the ObjectStore impl backed by a single file via [`crate::host_file::device::FileDevice`].
pub type FileStore = crate::backend::Store<crate::host_file::device::FileDevice>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::AnchorKey;
    use crate::device::DeviceId;
    use crate::hash::ObjectHash;
    use crate::host_file::device::FileDevice;
    use crate::object::{Object, ObjectMeta, VsfType};
    use crate::store::{ObjectStore, StoreError};
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec::Vec;

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
        assert_eq!(store.anchor().anchor_seq, 0);
        assert_eq!(store.count(), 1);
        let rc = store.load_root_commit().unwrap();
        assert!(rc.is_empty());
        drop(store);

        // Reopen the same file, verify state survives.
        let device = FileDevice::open(&path, DeviceId([1u8; 16])).unwrap();
        let store = FileStore::open(device, test_key()).unwrap();
        assert_eq!(store.anchor().anchor_seq, 0);
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
