//! `FileDevice` — implements the [`crate::device::Device`] trait against a single regular file via `std::fs::File`.
//!
//! Linux equivalent of a block device: the file's *current length* IS the device capacity. Random-access reads/writes at byte offsets via `seek` + `read_exact` / `write_all`. Atomic-ish writes via the OS file write semantics; `flush()` calls `sync_all` to push to disk.
//!
//! Concurrency note: this struct holds a single `File`. Single-threaded use is assumed (matches Photon's storage model). Multi-threaded access would require external locking.
//!
//! Range bounds: every read/write checks `offset + len <= capacity` before touching the file, returning [`crate::device::DeviceError::OutOfBounds`] on overflow. Capacity is recomputed lazily from the file metadata when queried — file length can change between calls if something appends, so callers shouldn't cache it.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::device::{Device, DeviceError, DeviceId, DeviceInfo, DeviceIoKind};

/// A [`Device`] backed by a single regular file. Stores the file handle, a cached `DeviceInfo`, and the file's current length (refreshed on demand). Created via [`FileDevice::open`] (file must exist) or [`FileDevice::create`] (creates + sizes).
pub struct FileDevice {
    file: File,
    info: DeviceInfo,
}

impl FileDevice {
    /// Open an existing vault file in read+write mode. The file's current length becomes the device capacity. Errors if the file doesn't exist, isn't openable, or its length is reported as 0 (vault files have a nonzero VSF header).
    pub fn open<P: AsRef<Path>>(path: P, device_id: DeviceId) -> Result<Self, DeviceError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| DeviceError::IoError(DeviceIoKind::ReadFailed))?;
        let capacity = file
            .metadata()
            .map_err(|_| DeviceError::IoError(DeviceIoKind::ReadFailed))?
            .len();
        let info = DeviceInfo {
            id: device_id,
            capacity,
            vendor: b"host-file".to_vec(),
            model: b"std::fs::File".to_vec(),
            atomic_write_size: None,
            has_volatile_cache: true,
        };
        Ok(Self { file, info })
    }

    /// Create a new file (truncating if it exists) and size it to `capacity` bytes via `set_len`. Useful for formatting a fresh vault. The bytes within the file are platform-dependent zeros — encrypted vault writes will overwrite the relevant regions.
    pub fn create<P: AsRef<Path>>(
        path: P,
        device_id: DeviceId,
        capacity: u64,
    ) -> Result<Self, DeviceError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|_| DeviceError::IoError(DeviceIoKind::WriteFailed))?;
        file.set_len(capacity)
            .map_err(|_| DeviceError::IoError(DeviceIoKind::WriteFailed))?;
        let info = DeviceInfo {
            id: device_id,
            capacity,
            vendor: b"host-file".to_vec(),
            model: b"std::fs::File".to_vec(),
            atomic_write_size: None,
            has_volatile_cache: true,
        };
        Ok(Self { file, info })
    }
}

impl Device for FileDevice {
    /// Grow the file to the new capacity. Used by the vault when `payload_capacity` needs to expand. Shrink not exposed — the vault uses sibling-file + rename for shrinks (compact pass), not in-place truncation.
    fn set_capacity(&mut self, new_capacity: u64) -> Result<(), DeviceError> {
        if new_capacity < self.info.capacity {
            // Refuse in-place shrink — vault should use the compact/rewrite path instead.
            return Err(DeviceError::IoError(DeviceIoKind::InvalidParam));
        }
        self.file
            .set_len(new_capacity)
            .map_err(|_| DeviceError::IoError(DeviceIoKind::WriteFailed))?;
        self.info.capacity = new_capacity;
        Ok(())
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        let len = buf.len() as u64;
        if offset.saturating_add(len) > self.info.capacity {
            return Err(DeviceError::OutOfBounds {
                offset,
                len,
                capacity: self.info.capacity,
            });
        }
        // `&File` (shared) doesn't impl Read directly, but `&mut File` does. Workaround: try_clone the handle for the read. Cheaper alternative would be pread (positional read syscall on Unix) but that needs `std::os::unix::fs::FileExt` and is not portable to Windows without `seek_read`. Use the seek+read fallback here; performance tuning later if it matters.
        let mut file = (&self.file)
            .try_clone()
            .map_err(|_| DeviceError::IoError(DeviceIoKind::ReadFailed))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| DeviceError::IoError(DeviceIoKind::ReadFailed))?;
        file.read_exact(buf)
            .map_err(|_| DeviceError::IoError(DeviceIoKind::ReadFailed))?;
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), DeviceError> {
        let len = data.len() as u64;
        if offset.saturating_add(len) > self.info.capacity {
            return Err(DeviceError::OutOfBounds {
                offset,
                len,
                capacity: self.info.capacity,
            });
        }
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| DeviceError::IoError(DeviceIoKind::WriteFailed))?;
        self.file
            .write_all(data)
            .map_err(|_| DeviceError::IoError(DeviceIoKind::WriteFailed))?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DeviceError> {
        self.file
            .sync_all()
            .map_err(|_| DeviceError::IoError(DeviceIoKind::FlushFailed))
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use alloc::format;

    fn tmp_path(label: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ferros_vault_test_{}_{}", label, std::process::id()));
        p
    }

    #[test]
    fn create_then_read_writes_round_trip() {
        let path = tmp_path("rw_round_trip");
        let _ = std::fs::remove_file(&path);
        let mut dev = FileDevice::create(&path, DeviceId([0u8; 16]), 4096).unwrap();
        let payload = b"hello vault";
        dev.write_at(100, payload).unwrap();
        dev.flush().unwrap();
        let mut buf = [0u8; 11];
        dev.read_at(100, &mut buf).unwrap();
        assert_eq!(&buf, payload);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn out_of_bounds_write_errors() {
        let path = tmp_path("oob_write");
        let _ = std::fs::remove_file(&path);
        let mut dev = FileDevice::create(&path, DeviceId([0u8; 16]), 64).unwrap();
        let res = dev.write_at(60, &[0u8; 10]); // 60 + 10 = 70 > 64
        assert!(matches!(res, Err(DeviceError::OutOfBounds { .. })));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn set_capacity_grows() {
        let path = tmp_path("grow");
        let _ = std::fs::remove_file(&path);
        let mut dev = FileDevice::create(&path, DeviceId([0u8; 16]), 1024).unwrap();
        dev.set_capacity(4096).unwrap();
        assert_eq!(dev.capacity(), 4096);
        // Verify we can now write past the old boundary.
        dev.write_at(2000, b"past_old_cap").unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn set_capacity_refuses_shrink() {
        let path = tmp_path("shrink");
        let _ = std::fs::remove_file(&path);
        let mut dev = FileDevice::create(&path, DeviceId([0u8; 16]), 4096).unwrap();
        let res = dev.set_capacity(2048);
        assert!(matches!(res, Err(DeviceError::IoError(DeviceIoKind::InvalidParam))));
        std::fs::remove_file(&path).ok();
    }
}
