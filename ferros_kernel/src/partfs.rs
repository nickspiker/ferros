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

/// Find a partition by GPT name on LUs 0-7. Its reads go under the kernel lock, as every UFS command must while processes use the vault.
pub fn find(name: &str) -> Option<Part> {
    crate::process::with_lock_held_or(|| find_locked(name))
}

fn find_locked(name: &str) -> Option<Part> {
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

/// The extents of a logical partition inside `super` (Android's dynamic partitions, liblp metadata_format.h), read-only: the geometry at byte 4096 (magic "gDla"), slot 0's metadata header at byte 12288 (magic "0PLA": tables descriptors at 80 partitions, 92 extents, each {offset, count, entry size}, the tables after the header), the partition entry by name (name[36], attributes, first extent, extent count, group), and its extents (u64 sectors, u32 type, u64 first sector, u32 source; 512-byte sectors). Each extent must be linear, on `super` itself, 4 KiB aligned; anything else is refused, not guessed at. The metadata's checksums are not checked: it is read only to find an ext4 filesystem that checks itself.
pub fn logical_extents(name: &str) -> Result<Vec<Part>, String> {
    crate::process::with_lock_held_or(|| logical_extents_locked(name))
}

fn logical_extents_locked(name: &str) -> Result<Vec<Part>, String> {
    let sup = find("super").ok_or_else(|| String::from("partition super not found"))?;
    let ufs = UfsController::new(UFS_BASE);
    let block = |lba: u64| -> Result<[u8; 4096], String> {
        // The metadata names blocks inside super: outside it is malformed metadata, not a read to make.
        if lba > sup.last - sup.first || ufs.read_block_lun(sup.lun, (sup.first + lba) as u32) != 0 || ufs.last_response_status() != 0 {
            return Err(alloc::format!("super: read of block {lba} failed"));
        }
        Ok(*ufs.data_buffer())
    };
    let u32at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    if u32at(&block(1)?, 0) != 0x616C_4467 {
        return Err(String::from("super: no LP geometry"));
    }
    let hdr = block(3)?;
    if u32at(&hdr, 0) != 0x414C_5030 {
        return Err(String::from("super: no LP metadata header"));
    }
    let (hsize, tsize) = (u32at(&hdr, 8) as usize, u32at(&hdr, 44) as usize);
    // The header and its tables, read whole: they are a few KiB (metadata_max_size bounds them; 64 KiB here).
    if hsize + tsize > 1 << 16 {
        return Err(alloc::format!("super: metadata of A#{} bytes", hsize + tsize));
    }
    let mut meta = Vec::with_capacity(hsize + tsize + 4096);
    for k in 0..(hsize + tsize).div_ceil(4096) as u64 {
        meta.extend_from_slice(&block(3 + k)?);
    }
    let table = |d: usize| (hsize + u32at(&meta, d) as usize, u32at(&meta, d + 4) as usize, u32at(&meta, d + 8) as usize);
    let (poff, pcount, psize) = table(80);
    let (eoff, ecount, esize) = table(92);
    if psize < 52 || esize < 24 || poff + pcount * psize > meta.len() || eoff + ecount * esize > meta.len() {
        return Err(String::from("super: LP tables outside the metadata"));
    }
    for p in 0..pcount {
        let e = &meta[poff + p * psize..poff + (p + 1) * psize];
        let n = &e[..36];
        let n = &n[..n.iter().position(|&c| c == 0).unwrap_or(36)];
        if n != name.as_bytes() {
            continue;
        }
        let (first, count) = (u32at(e, 40) as usize, u32at(e, 44) as usize);
        if first + count > ecount {
            return Err(alloc::format!("{name}: extents {first}..{} outside the table", first + count));
        }
        let mut out = Vec::new();
        for k in first..first + count {
        let x = &meta[eoff + k * esize..eoff + (k + 1) * esize];
        let (sectors, ty, start, source) = (u64at(x, 0), u32at(x, 8), u64at(x, 12), u32at(x, 20));
        if ty != 0 || source != 0 || start % 8 != 0 || sectors % 8 != 0 || sectors == 0 {
            return Err(alloc::format!("{name}: extent type {ty} source {source} start {start} sectors {sectors} not a 4 KiB-aligned linear extent on super"));
        }
        let (first_lba, last_lba) = (sup.first + start / 8, sup.first + start / 8 + sectors / 8 - 1);
        if last_lba > sup.last {
            return Err(alloc::format!("{name}: extent runs past super"));
        }
        out.push(Part { lun: sup.lun, first: first_lba, last: last_lba });
        }
        return Ok(out);
    }
    Err(alloc::format!("{name}: not in super's metadata"))
}

/// A logical partition laid out as one extent, as a plain `Part` (vendor_a on this build).
pub fn find_logical(name: &str) -> Result<Part, String> {
    let x = logical_extents(name)?;
    match x.as_slice() {
        [p] => Ok(*p),
        _ => Err(alloc::format!("{name}: A#{} extents (one handled here)", x.len())),
    }
}

/// `diag "part vermagic <logical>"`: the first `vermagic=` string in a logical partition's raw blocks, across all its extents — the kernel the modules in it were built for (every .ko carries one). Read-only; no filesystem is mounted, so a partition laid out in many extents (vendor_dlkm_a is) needs no extent-aware mount.
pub fn vermagic(rep: &mut Vec<u8>, name: &str) {
    crate::process::with_lock_held_or(|| vermagic_locked(rep, name))
}

fn vermagic_locked(rep: &mut Vec<u8>, name: &str) {
    let run = || -> Result<String, String> {
        let parts = logical_extents(name)?;
        let ufs = UfsController::new(UFS_BASE);
        let (mut blocks, mut carry) = (0u64, Vec::new());
        for p in parts.iter() {
            for lba in p.first..=p.last {
                if ufs.read_block_lun(p.lun, lba as u32) != 0 || ufs.last_response_status() != 0 {
                    return Err(alloc::format!("read of LBA {lba} failed"));
                }
                blocks += 1;
                // The last 64 bytes of the previous block ride along, so a string split across two blocks is still found.
                let mut buf = core::mem::take(&mut carry);
                buf.extend_from_slice(ufs.data_buffer());
                if let Some(at) = buf.windows(9).position(|w| w == b"vermagic=") {
                    let end = buf[at..].iter().position(|&c| c == 0).map(|e| at + e).unwrap_or(buf.len());
                    return Ok(alloc::format!("{name}: A#{} extents, found after A#{blocks} blocks: {}\n", parts.len(), String::from_utf8_lossy(&buf[at..end])));
                }
                carry = buf[buf.len() - 64..].to_vec();
            }
        }
        Err(alloc::format!("no vermagic in A#{blocks} blocks over A#{} extents", parts.len()))
    };
    let s = run().unwrap_or_else(|e| alloc::format!("{name}: {e}\n"));
    rep.extend_from_slice(s.as_bytes());
}

/// An ext4 mount of a partition already found (a logical one from `find_logical`, say).
pub fn mount_part(name: &str, part: Part) -> Result<Mount, String> {
    let mut scratch = vec![0u8; 4096];
    let fs = ferros_ext4::Fs::mount(PartDisk::new(part), &mut scratch).map_err(|e| alloc::format!("{name}: {e:?}"))?;
    if fs.block_size != 4096 {
        return Err(alloc::format!("{name}: block size {} (only 4 KiB handled)", fs.block_size));
    }
    Ok(Mount { fs, scratch, block: vec![0u8; 4096] })
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
            // One UFS command under the kernel lock, as the vault's are (a syscall holds it): the modem service reads files here while processes use the vault.
            let ufs = &mut self.ufs;
            let (lun, n) = (self.part.lun, chunk.len());
            let ok = crate::process::with_lock_held_or(|| {
                let ok = ufs.read_block_lun(lun, lba as u32) == 0 && ufs.last_response_status() == 0;
                if ok {
                    chunk.copy_from_slice(&ufs.data_buffer()[..n]);
                }
                ok
            });
            if !ok {
                return Err(ferros_ext4::Error::Io);
            }
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
        // One UFS command under the kernel lock, as in the ext4 reader above.
        let ufs = &mut self.ufs;
        let lun = self.part.lun;
        let ok = crate::process::with_lock_held_or(|| {
            let ok = ufs.read_block_lun(lun, lba as u32) == 0 && ufs.last_response_status() == 0;
            if ok {
                buf.copy_from_slice(&ufs.data_buffer()[..ferros_f2fs::BLOCK]);
            }
            ok
        });
        if !ok {
            return Err(ferros_f2fs::Error::Io);
        }
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

/// A partition's AVB `vbmeta`, read-only (`diag part avb <name>`, BOOT.md "Locked-bootloader endgame"): behind the AVB footer in its last 64 bytes (a partition that carries its own, as `boot`, `vendor_boot`, `dtbo` do), or at its start (a `vbmeta_*` partition). What the owner's re-signed top-level `vbmeta` must copy, so it vouches for the build actually installed rather than a factory image's.
pub fn avb_vbmeta(name: &str) -> Result<Vec<u8>, String> {
    crate::process::with_lock_held_or(|| avb_vbmeta_locked(name))
}

fn avb_vbmeta_locked(name: &str) -> Result<Vec<u8>, String> {
    let part = find(name).ok_or_else(|| alloc::format!("no partition {name}"))?;
    let ufs = UfsController::new(UFS_BASE);
    let read = |lba: u64| -> Result<[u8; 4096], String> {
        if lba > part.last || lba > u32::MAX as u64 || ufs.read_block_lun(part.lun, lba as u32) != 0 || ufs.last_response_status() != 0 {
            return Err(alloc::format!("read of LBA {lba} failed"));
        }
        Ok(*ufs.data_buffer())
    };
    let span = |off: u64, len: usize| -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(len);
        let mut at = off;
        while out.len() < len {
            let b = read(part.first + at / 4096)?;
            let s = (at % 4096) as usize;
            let n = (4096 - s).min(len - out.len());
            out.extend_from_slice(&b[s..s + n]);
            at += n as u64;
        }
        Ok(out)
    };
    let last = read(part.last)?;
    let f = &last[4096 - 64..];
    if &f[0..4] == b"AVBf" {
        let be = |at: usize| u64::from_be_bytes(f[at..at + 8].try_into().unwrap());
        let (off, size) = (be(20), be(28) as usize);
        if size > 1 << 16 {
            return Err(alloc::format!("footer's vbmeta is {size} bytes"));
        }
        return span(off, size);
    }
    let head = read(part.first)?;
    if &head[0..4] == b"AVB0" {
        // Header, then the two blocks it sizes (big-endian at 12 and 20).
        let be = |at: usize| u64::from_be_bytes(head[at..at + 8].try_into().unwrap()) as usize;
        let size = 256 + be(12) + be(20);
        if size > 1 << 16 {
            return Err(alloc::format!("vbmeta is {size} bytes"));
        }
        return span(0, size);
    }
    Err(alloc::format!("{name}: no AVB footer and no vbmeta at its start"))
}
