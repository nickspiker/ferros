//! `ferros_vault::Device` backed by the live UFS link, windowed onto the ferros partition.
//!
//! The vault speaks in arbitrary byte ranges (`read_at`/`write_at`); UFS speaks in
//! 4 KiB logical blocks. This adapter bridges the two: it translates a device-relative
//! byte offset into an absolute UFS LBA inside the ferros partition, reads/writes whole
//! blocks, and does read-modify-write for any access that doesn't land on block edges.
//!
//! The partition is the one carved from userdata's tail on husky (Pixel 8):
//! GPT LBA 28_881_920..=62_436_346, type GUID BLAKE3("ferros"), 128 GiB, `by-name/ferros`.
//! Because the vault offset is partition-relative, LBA 0 of this device is that base LBA —
//! the vault can never address a block outside its own partition.
//!
//! Exynos per-command nexus: every SCSI command must set its tag's bit in
//! HCI_UTRL_NEXUS_TYPE (reg_hci + 0x40) before the doorbell, exactly as the vendor
//! driver's `setup_xfer_req` hook does. `send_command` in the HAL is vendor-neutral and
//! does not do this, so — like the proven adopt-path shim — we set it here before each op.

use alloc::vec::Vec;
use ferros_hal::ufs::UfsController;
use ferros_vault::device::{Device, DeviceError, DeviceId, DeviceInfo, DeviceIoKind};

/// UFS logical block size on husky (4 KiB).
const BLOCK: u64 = 4096;

/// ferros partition geometry on the husky UFS (see module docs).
pub const FERROS_BASE_LBA: u32 = 28_881_920;
pub const FERROS_LAST_LBA: u32 = 62_436_346;
pub const FERROS_BLOCKS: u32 = FERROS_LAST_LBA - FERROS_BASE_LBA + 1; // 33_554_427

/// Exynos vendor HCI block base on husky (reg_hci) — holds HCI_UTRL_NEXUS_TYPE at +0x40.
const REG_HCI: usize = 0x1320_1100;
const NEXUS_TYPE: usize = 0x40;

/// A vault `Device` mapped onto a contiguous LBA window of the UFS device.
pub struct UfsDevice {
    ufs: UfsController,
    hci_base: usize,
    base_lba: u32,
    blocks: u32,
    info: DeviceInfo,
}

impl UfsDevice {
    /// Wrap a live controller as a device spanning `blocks` LBAs starting at `base_lba`.
    pub fn new(ufs: UfsController, hci_base: usize, base_lba: u32, blocks: u32) -> Self {
        let capacity = blocks as u64 * BLOCK;
        let mut id = [0u8; 16];
        id.copy_from_slice(b"ferros-ufs-husky");
        let info = DeviceInfo {
            id: DeviceId(id),
            capacity,
            vendor: Vec::from(&b"exynos-ufs"[..]),
            model: Vec::from(&b"husky-ferros"[..]),
            atomic_write_size: Some(BLOCK),
            has_volatile_cache: true,
        };
        Self { ufs, hci_base, base_lba, blocks, info }
    }

    /// The ferros partition on husky, over an already-adopted controller.
    pub fn ferros(ufs: UfsController) -> Self {
        Self::new(ufs, REG_HCI, FERROS_BASE_LBA, FERROS_BLOCKS)
    }

    /// Set tag 0's SCSI nexus bit before a data-transfer command (Exynos setup_xfer_req).
    fn scsi_nexus(&self) {
        unsafe {
            let p = (self.hci_base + NEXUS_TYPE) as *mut u32;
            let t = core::ptr::read_volatile(p);
            core::ptr::write_volatile(p, t | 1);
            core::arch::asm!("dsb sy");
        }
    }

    /// Translate a device-relative block index to an absolute LBA, bounds-checked.
    fn lba(&self, block_index: u64) -> Result<u32, DeviceError> {
        if block_index >= self.blocks as u64 {
            return Err(DeviceError::OutOfBounds {
                offset: block_index * BLOCK,
                len: BLOCK,
                capacity: self.capacity(),
            });
        }
        Ok(self.base_lba + block_index as u32)
    }

    fn bounds(&self, offset: u64, len: usize) -> Result<(), DeviceError> {
        let end = offset
            .checked_add(len as u64)
            .ok_or(DeviceError::OutOfBounds { offset, len: len as u64, capacity: self.capacity() })?;
        if end > self.capacity() {
            return Err(DeviceError::OutOfBounds { offset, len: len as u64, capacity: self.capacity() });
        }
        Ok(())
    }
}

impl Device for UfsDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DeviceError> {
        self.bounds(offset, buf.len())?;
        let mut done = 0usize;
        let mut cur = offset;
        while done < buf.len() {
            let lba = self.lba(cur / BLOCK)?;
            let within = (cur % BLOCK) as usize;
            let n = core::cmp::min(BLOCK as usize - within, buf.len() - done);
            self.scsi_nexus();
            if self.ufs.read_block(lba) != 0 {
                return Err(DeviceError::IoError(DeviceIoKind::ReadFailed));
            }
            let db = self.ufs.data_buffer();
            buf[done..done + n].copy_from_slice(&db[within..within + n]);
            done += n;
            cur += n as u64;
        }
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), DeviceError> {
        self.bounds(offset, data.len())?;
        let mut done = 0usize;
        let mut cur = offset;
        while done < data.len() {
            let lba = self.lba(cur / BLOCK)?;
            let within = (cur % BLOCK) as usize;
            let n = core::cmp::min(BLOCK as usize - within, data.len() - done);
            // Partial block: read-modify-write so we never clobber neighbouring bytes.
            if within != 0 || n != BLOCK as usize {
                self.scsi_nexus();
                if self.ufs.read_block(lba) != 0 {
                    return Err(DeviceError::IoError(DeviceIoKind::ReadFailed));
                }
            }
            self.ufs.data_buffer_mut()[within..within + n].copy_from_slice(&data[done..done + n]);
            self.scsi_nexus();
            if self.ufs.write_block(lba) != 0 {
                return Err(DeviceError::IoError(DeviceIoKind::WriteFailed));
            }
            done += n;
            cur += n as u64;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DeviceError> {
        // write_block completes synchronously (OCS polled), so writes are on the device.
        // A SCSI SYNCHRONIZE CACHE (10) to flush the device-side write cache is not yet
        // wired; the vault's write-verify-then-mirror protocol is the durability guard.
        Ok(())
    }

    fn info(&self) -> &DeviceInfo {
        &self.info
    }
}
