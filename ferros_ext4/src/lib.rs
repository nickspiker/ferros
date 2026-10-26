//! Read-only ext4: enough to find a file by path and read it.
//!
//! ferros needs this for the modem: its firmware (`modem.bin` in the `modem` partition) is on an ext4 partition Android formatted (docs/oracle/MODEM-BOOT-RECON.md); the per-unit NV files are on f2fs (ferros_f2fs).
//! Supported: 1 KiB-64 KiB blocks, 32- and 64-bit group descriptors, extent-mapped files and directories (any tree depth), uninitialised extents (read as zeros), fast symlinks (target inside the inode) and slow ones, path lookup through symlinks.
//! Not supported, refused with an error: block-mapped (pre-extent) files, inline data, encryption.
//! No journal replay: the partitions are read-only on Android and nothing here writes.
//! The caller supplies the storage as a [`Disk`]; every read is whole filesystem blocks.
#![no_std]

/// Reads whole filesystem blocks. `block` is a filesystem block number, `buf.len()` is a multiple of the block size.
pub trait Disk {
    fn read_blocks(&mut self, block: u64, buf: &mut [u8]) -> Result<(), Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The disk reader failed.
    Io,
    /// Not an ext4 superblock (magic G#EF53).
    NotExt4,
    /// A structure is out of range or inconsistent.
    Corrupt,
    /// A path component does not exist.
    NotFound,
    /// A path component is not a directory.
    NotDir,
    /// The file uses a feature this reader refuses (block maps, inline data, encryption).
    Unsupported,
    /// Too many symlinks while resolving a path.
    Loop,
    /// The caller's buffer is too small for the block size.
    Buffer,
}

/// Largest block size handled (ext4 allows up to 64 KiB).
pub const MAX_BLOCK: usize = 65536;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

const INCOMPAT_64BIT: u32 = 0x80;
const FLAG_EXTENTS: u32 = 0x8_0000;
const FLAG_INLINE: u32 = 0x1000_0000;
const FLAG_ENCRYPT: u32 = 0x800;
const S_IFMT: u16 = 0xF000;
const S_IFDIR: u16 = 0x4000;
const S_IFLNK: u16 = 0xA000;
const S_IFREG: u16 = 0x8000;

/// A mounted (read-only) filesystem: the superblock fields the reader needs.
pub struct Fs<D: Disk> {
    pub disk: D,
    pub block_size: usize,
    inodes_per_group: u32,
    inode_size: usize,
    desc_size: usize,
    desc_block: u64,
    blocks_count: u64,
    groups: u64,
}

/// One inode, as much of it as reading needs.
#[derive(Clone, Copy)]
pub struct Inode {
    pub number: u32,
    pub mode: u16,
    pub size: u64,
    flags: u32,
    /// i_block: the extent tree root (or a fast symlink's target).
    root: [u8; 60],
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & S_IFMT == S_IFLNK
    }
}

/// One mapped run of a file: `len` blocks starting at file block `logical`, at disk block `physical` (None = a hole or an uninitialised extent, reads as zeros).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub logical: u64,
    pub physical: Option<u64>,
    pub len: u64,
}

impl<D: Disk> Fs<D> {
    /// Read the superblock (byte 1024) with `scratch` (at least 4 KiB, and at least one block).
    pub fn mount(mut disk: D, scratch: &mut [u8]) -> Result<Self, Error> {
        // The superblock is at byte 1024 whatever the block size; read the first 4 KiB in 1 KiB-block terms by asking for block 0 of a 4 KiB read only when the block size is known. Start by assuming the caller's reader works in units it was told: read block 0 with a 4 KiB buffer, which covers byte 1024 for any block size from 1 KiB (blocks 0-3) to 64 KiB (first part of block 0).
        if scratch.len() < 4096 {
            return Err(Error::Buffer);
        }
        disk.read_blocks_raw0(&mut scratch[..4096])?;
        let sb = &scratch[1024..2048];
        if u16le(sb, 56) != 0xEF53 {
            return Err(Error::NotExt4);
        }
        let log = u32le(sb, 24);
        if log > 6 {
            return Err(Error::Corrupt);
        }
        let block_size = 1024usize << log;
        if scratch.len() < block_size {
            return Err(Error::Buffer);
        }
        let incompat = u32le(sb, 96);
        let bits64 = incompat & INCOMPAT_64BIT != 0;
        let desc_size = if bits64 { (u16le(sb, 254) as usize).max(32) } else { 32 };
        let blocks_count = u32le(sb, 4) as u64 | if bits64 { (u32le(sb, 336) as u64) << 32 } else { 0 };
        let blocks_per_group = u32le(sb, 32) as u64;
        let inodes_per_group = u32le(sb, 40);
        let inode_size = if u32le(sb, 76) == 0 { 128 } else { u16le(sb, 88) as usize };
        let first_data_block = u32le(sb, 20) as u64;
        if blocks_per_group == 0 || inodes_per_group == 0 || inode_size < 128 || inode_size > block_size {
            return Err(Error::Corrupt);
        }
        let groups = (blocks_count - first_data_block).div_ceil(blocks_per_group);
        Ok(Fs { disk, block_size, inodes_per_group, inode_size, desc_size, desc_block: first_data_block + 1, blocks_count, groups })
    }

    fn read_block(&mut self, block: u64, buf: &mut [u8]) -> Result<(), Error> {
        if block >= self.blocks_count || buf.len() < self.block_size {
            return Err(Error::Corrupt);
        }
        self.disk.read_blocks(block, &mut buf[..self.block_size])
    }

    /// Read inode `n` (1-based).
    pub fn inode(&mut self, n: u32, scratch: &mut [u8]) -> Result<Inode, Error> {
        if n == 0 {
            return Err(Error::Corrupt);
        }
        let group = ((n - 1) / self.inodes_per_group) as u64;
        let index = ((n - 1) % self.inodes_per_group) as u64;
        if group >= self.groups {
            return Err(Error::Corrupt);
        }
        let bs = self.block_size as u64;
        let dpos = group * self.desc_size as u64;
        self.read_block(self.desc_block + dpos / bs, scratch)?;
        let d = &scratch[(dpos % bs) as usize..];
        let table = u32le(d, 8) as u64 | if self.desc_size >= 64 { (u32le(d, 0x28) as u64) << 32 } else { 0 };
        let ipos = index * self.inode_size as u64;
        self.read_block(table + ipos / bs, scratch)?;
        let i = &scratch[(ipos % bs) as usize..];
        let mut root = [0u8; 60];
        root.copy_from_slice(&i[40..100]);
        Ok(Inode { number: n, mode: u16le(i, 0), size: u32le(i, 4) as u64 | (u32le(i, 108) as u64) << 32, flags: u32le(i, 32), root })
    }

    /// The run of `ino` that holds file block `fblock`, walking the extent tree; `scratch` holds one block.
    pub fn map(&mut self, ino: &Inode, fblock: u64, scratch: &mut [u8]) -> Result<Run, Error> {
        if ino.flags & (FLAG_INLINE | FLAG_ENCRYPT) != 0 || ino.flags & FLAG_EXTENTS == 0 {
            return Err(Error::Unsupported);
        }
        let mut node = [0u8; 60];
        node.copy_from_slice(&ino.root);
        let mut in_root = true;
        let mut depth_guard = 0;
        loop {
            let n: &[u8] = if in_root { &node } else { &scratch[..self.block_size] };
            if u16le(n, 0) != 0xF30A {
                return Err(Error::Corrupt);
            }
            let entries = u16le(n, 2) as usize;
            let depth = u16le(n, 6);
            if 12 + entries * 12 > n.len() {
                return Err(Error::Corrupt);
            }
            if depth == 0 {
                // Leaves: find the extent covering fblock, else the hole up to the next one.
                let mut next_start = u64::MAX;
                for e in 0..entries {
                    let o = 12 + e * 12;
                    let lblk = u32le(n, o) as u64;
                    let raw = u16le(n, o + 4) as u64;
                    let (len, init) = if raw > 32768 { (raw - 32768, false) } else { (raw, true) };
                    let start = (u16le(n, o + 6) as u64) << 32 | u32le(n, o + 8) as u64;
                    if fblock >= lblk && fblock < lblk + len {
                        let skip = fblock - lblk;
                        return Ok(Run { logical: fblock, physical: if init { Some(start + skip) } else { None }, len: len - skip });
                    }
                    if lblk > fblock {
                        next_start = next_start.min(lblk);
                    }
                }
                let len = if next_start == u64::MAX { u64::MAX - fblock } else { next_start - fblock };
                return Ok(Run { logical: fblock, physical: None, len });
            }
            // Index node: the last entry whose first block is <= fblock.
            let mut child = None;
            for e in 0..entries {
                let o = 12 + e * 12;
                if (u32le(n, o) as u64) <= fblock || e == 0 {
                    child = Some((u16le(n, o + 8) as u64) << 32 | u32le(n, o + 4) as u64);
                } else {
                    break;
                }
            }
            let Some(c) = child else { return Err(Error::Corrupt) };
            depth_guard += 1;
            if depth_guard > 8 {
                return Err(Error::Corrupt);
            }
            self.read_block(c, scratch)?;
            in_root = false;
        }
    }

    /// Read `buf.len()` bytes of `ino` from byte `offset` (which must be block-aligned), using `scratch` (one block) for tree nodes and partial tails. Returns the bytes read (short at end of file).
    pub fn read(&mut self, ino: &Inode, offset: u64, buf: &mut [u8], scratch: &mut [u8]) -> Result<usize, Error> {
        let bs = self.block_size as u64;
        if offset % bs != 0 {
            return Err(Error::Corrupt);
        }
        if offset >= ino.size {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(ino.size - offset) as usize;
        let mut done = 0usize;
        while done < want {
            let fblock = (offset + done as u64) / bs;
            let run = self.map(ino, fblock, scratch)?;
            let left = want - done;
            let run_bytes = run.len.saturating_mul(bs).min(left as u64) as usize;
            let whole = run_bytes / self.block_size * self.block_size;
            match run.physical {
                None => {
                    let n = run_bytes.max(left.min(self.block_size));
                    buf[done..done + n].fill(0);
                    done += n;
                }
                Some(p) if whole > 0 => {
                    if p + (whole / self.block_size) as u64 > self.blocks_count {
                        return Err(Error::Corrupt);
                    }
                    self.disk.read_blocks(p, &mut buf[done..done + whole])?;
                    done += whole;
                }
                Some(p) => {
                    // Less than a block left: through scratch.
                    self.read_block(p, scratch)?;
                    let n = left.min(self.block_size);
                    buf[done..done + n].copy_from_slice(&scratch[..n]);
                    done += n;
                }
            }
        }
        Ok(want)
    }

    /// Look `name` up in directory `dir`.
    pub fn lookup(&mut self, dir: &Inode, name: &[u8], scratch: &mut [u8], block: &mut [u8]) -> Result<u32, Error> {
        if !dir.is_dir() {
            return Err(Error::NotDir);
        }
        let bs = self.block_size;
        if block.len() < bs {
            return Err(Error::Buffer);
        }
        let mut off = 0u64;
        while off < dir.size {
            let n = self.read(dir, off, &mut block[..bs], scratch)?;
            let mut p = 0usize;
            while p + 8 <= n {
                let ino = u32le(block, p);
                let rec = u16le(block, p + 4) as usize;
                let nlen = block[p + 6] as usize;
                if rec < 8 || p + rec > n {
                    return Err(Error::Corrupt);
                }
                if ino != 0 && nlen == name.len() && p + 8 + nlen <= n && &block[p + 8..p + 8 + nlen] == name {
                    return Ok(ino);
                }
                p += rec;
            }
            off += bs as u64;
        }
        Err(Error::NotFound)
    }

    /// Resolve an absolute path (components separated by '/') to an inode, following symlinks (relative ones against their directory, absolute ones from the root), at most 8 of them.
    pub fn open(&mut self, path: &[u8], scratch: &mut [u8], block: &mut [u8]) -> Result<Inode, Error> {
        // The path being walked lives in a fixed buffer so a symlink target can be spliced in front of the remainder.
        let mut buf = [0u8; 512];
        if path.len() > buf.len() {
            return Err(Error::Corrupt);
        }
        buf[..path.len()].copy_from_slice(path);
        let mut len = path.len();
        let mut pos = 0usize;
        let mut cur = self.inode(2, scratch)?;
        let root = cur;
        let mut links = 0;
        loop {
            while pos < len && buf[pos] == b'/' {
                pos += 1;
            }
            if pos >= len {
                return Ok(cur);
            }
            let end = buf[pos..len].iter().position(|&c| c == b'/').map(|i| pos + i).unwrap_or(len);
            let mut name = [0u8; 255];
            let nl = end - pos;
            if nl > 255 {
                return Err(Error::Corrupt);
            }
            name[..nl].copy_from_slice(&buf[pos..end]);
            let n = self.lookup(&cur, &name[..nl], scratch, block)?;
            let next = self.inode(n, scratch)?;
            if next.is_symlink() {
                links += 1;
                if links > 8 {
                    return Err(Error::Loop);
                }
                let mut target = [0u8; 256];
                let tl = next.size as usize;
                if tl > 255 {
                    return Err(Error::Unsupported);
                }
                if next.flags & FLAG_EXTENTS == 0 && tl < 60 {
                    target[..tl].copy_from_slice(&next.root[..tl]);
                } else {
                    let bs = self.block_size;
                    let r = self.read(&next, 0, &mut block[..bs], scratch)?;
                    if r < tl {
                        return Err(Error::Corrupt);
                    }
                    target[..tl].copy_from_slice(&block[..tl]);
                }
                // New path = target + remainder after this component.
                let rest = len - end;
                if tl + rest > buf.len() {
                    return Err(Error::Corrupt);
                }
                let mut nb = [0u8; 512];
                nb[..tl].copy_from_slice(&target[..tl]);
                nb[tl..tl + rest].copy_from_slice(&buf[end..len]);
                buf = nb;
                len = tl + rest;
                pos = 0;
                if tl > 0 && target[0] == b'/' {
                    cur = root;
                }
                continue;
            }
            cur = next;
            pos = end;
        }
    }
}

/// Reading the first 4 KiB before the block size is known: block 0 for a 4 KiB-block disk. Disks are addressed in filesystem blocks, so `mount` asks for "block 0" with a 4 KiB buffer; a `Disk` must accept a 4 KiB read at block 0 whatever its block size (all Android ext4 volumes use 4 KiB blocks).
trait Raw0 {
    fn read_blocks_raw0(&mut self, buf: &mut [u8]) -> Result<(), Error>;
}
impl<D: Disk> Raw0 for D {
    fn read_blocks_raw0(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.read_blocks(0, buf)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    struct Img(Vec<u8>, usize);
    impl Disk for Img {
        fn read_blocks(&mut self, block: u64, buf: &mut [u8]) -> Result<(), Error> {
            let o = block as usize * self.1;
            if o + buf.len() > self.0.len() {
                return Err(Error::Io);
            }
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
    }

    fn image() -> Option<Vec<u8>> {
        std::env::var("FERROS_EXT4_TEST_IMAGE").ok().and_then(|p| std::fs::read(p).ok())
    }

    /// Run with FERROS_EXT4_TEST_IMAGE=<ext4 image> FERROS_EXT4_TEST_DIR=<the directory it was built from>: every regular file under the directory is opened by path in the image and compared byte for byte.
    #[test]
    fn matches_source_tree() {
        let Some(img) = image() else { return };
        let dir = std::env::var("FERROS_EXT4_TEST_DIR").unwrap();
        let mut scratch = std::vec![0u8; MAX_BLOCK];
        let mut block = std::vec![0u8; MAX_BLOCK];
        let mut fs = Fs::mount(Img(img, 4096), &mut scratch).unwrap();
        assert_eq!(fs.block_size, 4096);
        let mut checked = 0;
        let mut stack = std::vec![std::path::PathBuf::from(&dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                let rel = std::string::String::from(p.strip_prefix(&dir).unwrap().to_str().unwrap());
                let meta = std::fs::symlink_metadata(&p).unwrap();
                if meta.is_dir() {
                    stack.push(p.clone());
                    continue;
                }
                // A symlink is checked through what it points at, with absolute targets taken inside the tree.
                let host = if meta.file_type().is_symlink() {
                    let t = std::fs::read_link(&p).unwrap();
                    if t.is_absolute() { std::path::Path::new(&dir).join(t.strip_prefix("/").unwrap()) } else { p.parent().unwrap().join(t) }
                } else {
                    p.clone()
                };
                if std::fs::metadata(&host).unwrap().is_dir() {
                    let ino = fs.open(std::format!("/{rel}").as_bytes(), &mut scratch, &mut block).unwrap();
                    assert!(ino.is_dir(), "{rel}");
                    continue;
                }
                let want = std::fs::read(&host).unwrap();
                let ino = fs.open(std::format!("/{rel}").as_bytes(), &mut scratch, &mut block).unwrap();
                assert_eq!(ino.size as usize, want.len(), "{rel}");
                let mut got = std::vec![0u8; want.len()];
                let n = fs.read(&ino, 0, &mut got, &mut scratch).unwrap();
                assert_eq!(n, want.len());
                assert!(got == want, "{rel} differs");
                checked += 1;
            }
        }
        assert!(checked > 0);
        std::eprintln!("ext4: {checked} files match");
    }
}
