//! Files on Android's partitions, read-only: GPT lookup by partition name on any UFS LU, and an ext4 (ferros_ext4) or f2fs (ferros_f2fs) mount of it.
//!
//! The modem needs this (docs/oracle/MODEM-BOOT-RECON.md): its firmware is `images/default/modem.bin` in the `modem_a` partition (ext4) and its per-unit NV images are `nv_normal.bin` / `nv_protected.bin` in `efs` and `replay_region.bin` in `modem_userdata` (both f2fs).
//! Reads are single 4 KiB UFS blocks through the HAL's bounce buffer, so throughput is one command per block.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use ferros_hal::ufs::UfsController;

const UFS_BASE: usize = 0x1320_0000;

/// A partition: the LU it lives on and its first and last LBA (4 KiB blocks).
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub lun: u8,
    pub first: u64,
    pub last: u64,
}

/// Find a partition by GPT name on LUs 0-7.
pub fn find(name: &str) -> Option<Part> {
    let ufs = UfsController::new(UFS_BASE);
    for lun in 0..8u8 {
        if ufs.read_block_lun(lun, 1) != 0 || ufs.last_response_status() != 0 {
            continue;
        }
        let h = *ufs.data_buffer();
        if &h[0..8] != b"EFI PART" {
            continue;
        }
        let entry_lba = u64::from_le_bytes(h[0x48..0x50].try_into().unwrap());
        let count = u32::from_le_bytes(h[0x50..0x54].try_into().unwrap()).min(256) as usize;
        let esize = u32::from_le_bytes(h[0x54..0x58].try_into().unwrap()) as usize;
        if esize < 128 || esize > 4096 {
            continue;
        }
        let per_block = 4096 / esize;
        let mut i = 0usize;
        while i < count {
            if ufs.read_block_lun(lun, (entry_lba as usize + i / per_block) as u32) != 0 {
                break;
            }
            let blk = *ufs.data_buffer();
            for e in 0..per_block {
                if i >= count {
                    break;
                }
                let ent = &blk[e * esize..(e + 1) * esize];
                i += 1;
                if ent[..16].iter().all(|&b| b == 0) {
                    continue;
                }
                let n: String = ent[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).filter_map(|c| char::from_u32(c as u32)).collect();
                if n == name {
                    return Some(Part { lun, first: u64::from_le_bytes(ent[32..40].try_into().unwrap()), last: u64::from_le_bytes(ent[40..48].try_into().unwrap()) });
                }
            }
        }
    }
    None
}

/// A partition as an ext4 disk: filesystem block n is LBA first + n (both 4 KiB).
pub struct PartDisk {
    ufs: UfsController,
    part: Part,
}

impl PartDisk {
    pub fn new(part: Part) -> Self {
        PartDisk { ufs: UfsController::new(UFS_BASE), part }
    }
}

impl ferros_ext4::Disk for PartDisk {
    fn read_blocks(&mut self, block: u64, buf: &mut [u8]) -> Result<(), ferros_ext4::Error> {
        for (i, chunk) in buf.chunks_mut(4096).enumerate() {
            let lba = self.part.first + block + i as u64;
            if lba > self.part.last || lba > u32::MAX as u64 {
                return Err(ferros_ext4::Error::Corrupt);
            }
            if self.ufs.read_block_lun(self.part.lun, lba as u32) != 0 || self.ufs.last_response_status() != 0 {
                return Err(ferros_ext4::Error::Io);
            }
            let n = chunk.len();
            chunk.copy_from_slice(&self.ufs.data_buffer()[..n]);
        }
        Ok(())
    }
}

impl ferros_f2fs::Disk for PartDisk {
    fn read_block(&mut self, block: u64, buf: &mut [u8; ferros_f2fs::BLOCK]) -> Result<(), ferros_f2fs::Error> {
        let lba = self.part.first + block;
        if lba > self.part.last || lba > u32::MAX as u64 {
            return Err(ferros_f2fs::Error::Corrupt);
        }
        if self.ufs.read_block_lun(self.part.lun, lba as u32) != 0 || self.ufs.last_response_status() != 0 {
            return Err(ferros_f2fs::Error::Io);
        }
        buf.copy_from_slice(&self.ufs.data_buffer()[..ferros_f2fs::BLOCK]);
        Ok(())
    }
}

/// An f2fs mount of a named partition.
pub fn mount_f2fs(name: &str) -> Result<ferros_f2fs::Fs<PartDisk>, String> {
    let part = find(name).ok_or_else(|| alloc::format!("partition {name} not found"))?;
    ferros_f2fs::Fs::mount(PartDisk::new(part)).map_err(|e| alloc::format!("{name}: {e:?}"))
}

/// A whole file from an f2fs partition.
pub fn read_f2fs(name: &str, path: &str) -> Result<Vec<u8>, String> {
    let mut fs = mount_f2fs(name)?;
    let ino = fs.open(path.as_bytes()).map_err(|e| alloc::format!("{name}{path}: {e:?}"))?;
    if !ino.is_file() || ino.size > 64 << 20 {
        return Err(alloc::format!("{name}{path}: not a file of a sane size"));
    }
    let mut v = vec![0u8; ino.size as usize];
    let n = fs.read_at(&ino, 0, &mut v).map_err(|e| alloc::format!("{name}{path}: {e:?}"))?;
    v.truncate(n);
    Ok(v)
}

/// An ext4 mount of a named partition, with its two working blocks.
pub struct Mount {
    pub fs: ferros_ext4::Fs<PartDisk>,
    pub scratch: Vec<u8>,
    pub block: Vec<u8>,
}

pub fn mount(name: &str) -> Result<Mount, String> {
    let part = find(name).ok_or_else(|| alloc::format!("partition {name} not found"))?;
    let mut scratch = vec![0u8; 4096];
    let fs = ferros_ext4::Fs::mount(PartDisk::new(part), &mut scratch).map_err(|e| alloc::format!("{name}: {e:?}"))?;
    if fs.block_size != 4096 {
        return Err(alloc::format!("{name}: block size {} (only 4 KiB handled)", fs.block_size));
    }
    Ok(Mount { fs, scratch, block: vec![0u8; 4096] })
}

impl Mount {
    pub fn open(&mut self, path: &str) -> Result<ferros_ext4::Inode, String> {
        self.fs.open(path.as_bytes(), &mut self.scratch, &mut self.block).map_err(|e| alloc::format!("{path}: {e:?}"))
    }
    /// Read `buf.len()` bytes from block-aligned `offset`.
    pub fn read(&mut self, ino: &ferros_ext4::Inode, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
        self.fs.read(ino, offset, buf, &mut self.scratch).map_err(|e| alloc::format!("read: {e:?}"))
    }
}
