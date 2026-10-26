//! Read-only f2fs: enough to find a file by path and read it.
//!
//! ferros needs this for the modem: the per-unit NV files (`nv_normal.bin`, `nv_protected.bin` in `efs`, `replay_region.bin` in `modem_userdata`) are on f2fs partitions Android formatted (docs/oracle/MODEM-BOOT-RECON.md).
//! The layout follows the kernel's fs/f2fs (f2fs_fs.h): a superblock at byte 1024, two checkpoint packs (the newer valid one is current), the node address table (NAT) in two copies per block chosen by the checkpoint's bitmap, with recent changes in the checkpoint's NAT journal (compacted or normal summaries), inodes and direct/indirect node blocks, dentry blocks.
//! Supported: 4 KiB blocks, extra attributes, inline xattrs (fixed or flexible size), inline data, inline dentries, direct, indirect and double-indirect node trees, holes.
//! Not supported, refused with an error: encrypted or compressed files, large NAT bitmaps, cp_payload blocks.
//! No roll-forward recovery: what fsync wrote after the last checkpoint is not seen (the files the modem keeps are written whole and checkpointed).
//! The caller supplies the storage as a [`Disk`]; every read is one 4 KiB filesystem block.
#![no_std]

/// Reads one 4 KiB filesystem block. `block` is a filesystem block number (from the partition's start).
pub trait Disk {
    fn read_block(&mut self, block: u64, buf: &mut [u8; BLOCK]) -> Result<(), Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The disk reader failed.
    Io,
    /// Not an f2fs superblock (magic G#F2F52010).
    NotF2fs,
    /// A structure is out of range or inconsistent.
    Corrupt,
    /// A path component does not exist.
    NotFound,
    /// A path component is not a directory.
    NotDir,
    /// A feature this reader refuses (encryption, compression, large NAT bitmaps, cp_payload).
    Unsupported,
}

pub const BLOCK: usize = 4096;
const MAGIC: u32 = 0xF2F5_2010;
const NAT_PER_BLOCK: u32 = (BLOCK / 9) as u32;
const ADDRS_PER_INODE: usize = 923;
const ADDRS_PER_BLOCK: u64 = 1018;
const NIDS_PER_BLOCK: u64 = 1018;
const I_ADDR: usize = 360;
const I_NID: usize = I_ADDR + ADDRS_PER_INODE * 4;
const INLINE_XATTR: u8 = 0x01;
const INLINE_DATA: u8 = 0x02;
const INLINE_DENTRY: u8 = 0x04;
const EXTRA_ATTR: u8 = 0x20;
const DEFAULT_INLINE_XATTR_ADDRS: usize = 50;
const FEATURE_FLEXIBLE_INLINE_XATTR: u32 = 0x40;
const CP_COMPACT_SUM: u32 = 0x4;
const CP_LARGE_NAT_BITMAP: u32 = 0x400;
const ENCRYPT_FL: u8 = 0x04;
const COMPR_FL: u32 = 0x04;
const S_IFMT: u16 = 0xF000;
const S_IFDIR: u16 = 0x4000;
const S_IFREG: u16 = 0x8000;
/// A dentry block: 214 slots, a 27-byte bitmap, 3 reserved bytes, 11-byte entries, 8-byte name slots.
const DENTRY_SLOTS: usize = 214;
/// The NAT journal's place in a summary block (after 512 seven-byte entries) and its size.
const SUM_ENTRIES_SIZE: usize = 512 * 7;
const JOURNAL_SIZE: usize = BLOCK - 5 - SUM_ENTRIES_SIZE;
const NAT_JOURNAL_ENTRIES: usize = (JOURNAL_SIZE - 2) / 13;

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u32le(b, o) as u64 | (u32le(b, o + 4) as u64) << 32
}

/// A mounted (read-only) filesystem: the superblock and checkpoint fields the reader needs.
pub struct Fs<D: Disk> {
    pub disk: D,
    pub block_count: u64,
    root_ino: u32,
    nat_blkaddr: u64,
    log_blocks_per_seg: u32,
    features: u32,
    /// The current NAT copy per NAT block (the checkpoint's NAT version bitmap, MSB first), up to 8 KiB of it.
    nat_bitmap: [u8; 1024],
    nat_bitmap_len: usize,
    /// The checkpoint's NAT journal: (nid, block address) of entries not yet written back.
    journal: [(u32, u32); NAT_JOURNAL_ENTRIES],
    journal_len: usize,
    pub checkpoint_ver: u64,
}

/// One inode, as much of it as reading needs: the whole node block (addresses and inline data live in it).
pub struct Inode {
    pub number: u32,
    pub mode: u16,
    pub size: u64,
    inline: u8,
    block: [u8; BLOCK],
}

impl Inode {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }
    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }
    /// i_addr slots taken by extra attributes (i_extra_isize, in words).
    fn extra(&self) -> usize {
        if self.inline & EXTRA_ATTR != 0 { u16le(&self.block, I_ADDR) as usize / 4 } else { 0 }
    }
    fn xattr_addrs(&self, features: u32) -> usize {
        if self.inline & INLINE_XATTR == 0 {
            0
        } else if features & FEATURE_FLEXIBLE_INLINE_XATTR != 0 && self.inline & EXTRA_ATTR != 0 {
            u16le(&self.block, I_ADDR + 2) as usize
        } else {
            DEFAULT_INLINE_XATTR_ADDRS
        }
    }
    /// The data block addresses the inode itself holds.
    fn addrs(&self, features: u32) -> usize {
        ADDRS_PER_INODE.saturating_sub(self.extra() + self.xattr_addrs(features))
    }
    fn addr(&self, i: usize) -> u32 {
        u32le(&self.block, I_ADDR + (self.extra() + i) * 4)
    }
    /// Inline data: after one reserved word, up to the inline xattrs.
    fn inline_data(&self, features: u32) -> &[u8] {
        let start = I_ADDR + (self.extra() + 1) * 4;
        let len = 4 * self.addrs(features).saturating_sub(1);
        &self.block[start..start + len]
    }
}

impl<D: Disk> Fs<D> {
    /// Read the superblock and the current checkpoint.
    pub fn mount(mut disk: D) -> Result<Self, Error> {
        let mut b = [0u8; BLOCK];
        disk.read_block(0, &mut b)?;
        let sb = &b[1024..];
        if u32le(sb, 0) != MAGIC {
            return Err(Error::NotF2fs);
        }
        if u32le(sb, 16) != 12 {
            return Err(Error::Unsupported);
        }
        let log_blocks_per_seg = u32le(sb, 20);
        let block_count = u64le(sb, 36);
        let cp_blkaddr = u32le(sb, 76) as u64;
        let nat_blkaddr = u32le(sb, 84) as u64;
        let root_ino = u32le(sb, 96);
        let cp_payload = u32le(sb, 1664);
        let features = u32le(sb, 2180);
        if cp_payload != 0 || log_blocks_per_seg > 12 {
            return Err(Error::Unsupported);
        }
        let mut fs = Fs { disk, block_count, root_ino, nat_blkaddr, log_blocks_per_seg, features, nat_bitmap: [0; 1024], nat_bitmap_len: 0, journal: [(0, 0); NAT_JOURNAL_ENTRIES], journal_len: 0, checkpoint_ver: 0 };
        // The two packs: the newer whose first and last blocks carry the same version (a pack interrupted mid-write does not).
        let mut best: Option<(u64, u64)> = None;
        for pack in [cp_blkaddr, cp_blkaddr + (1 << log_blocks_per_seg)] {
            fs.read(pack, &mut b)?;
            let ver = u64le(&b, 0);
            let total = u32le(&b, 136) as u64;
            if total == 0 || total > 1 << log_blocks_per_seg {
                continue;
            }
            let mut last = [0u8; BLOCK];
            fs.read(pack + total - 1, &mut last)?;
            if u64le(&last, 0) == ver && best.is_none_or(|(v, _)| ver > v) {
                best = Some((ver, pack));
            }
        }
        let (ver, pack) = best.ok_or(Error::Corrupt)?;
        fs.checkpoint_ver = ver;
        fs.read(pack, &mut b)?;
        let flags = u32le(&b, 132);
        if flags & CP_LARGE_NAT_BITMAP != 0 {
            return Err(Error::Unsupported);
        }
        let sit_bytes = u32le(&b, 156) as usize;
        let nat_bytes = u32le(&b, 160) as usize;
        if 192 + sit_bytes + nat_bytes > BLOCK || nat_bytes > fs.nat_bitmap.len() {
            return Err(Error::Corrupt);
        }
        fs.nat_bitmap[..nat_bytes].copy_from_slice(&b[192 + sit_bytes..192 + sit_bytes + nat_bytes]);
        fs.nat_bitmap_len = nat_bytes;
        // The NAT journal: the first summary block (compacted: the journal at its start; normal: the hot data summary's journal, after its entries).
        let start_sum = u32le(&b, 140) as u64;
        fs.read(pack + start_sum, &mut b)?;
        let j = if flags & CP_COMPACT_SUM != 0 { 0 } else { SUM_ENTRIES_SIZE };
        let n = (u16le(&b, j) as usize).min(NAT_JOURNAL_ENTRIES);
        for k in 0..n {
            let e = j + 2 + k * 13;
            // nid, then the NAT entry: version u8, ino u32, block address u32.
            fs.journal[k] = (u32le(&b, e), u32le(&b, e + 4 + 5));
        }
        fs.journal_len = n;
        Ok(fs)
    }

    fn read(&mut self, block: u64, buf: &mut [u8; BLOCK]) -> Result<(), Error> {
        if block >= self.block_count {
            return Err(Error::Corrupt);
        }
        self.disk.read_block(block, buf)
    }

    /// A node's block address: the checkpoint's journal first, then the current NAT copy.
    fn node_addr(&mut self, nid: u32) -> Result<u32, Error> {
        if let Some(&(_, a)) = self.journal[..self.journal_len].iter().find(|e| e.0 == nid) {
            return Ok(a);
        }
        let block_off = (nid / NAT_PER_BLOCK) as u64;
        if block_off as usize / 8 >= self.nat_bitmap_len {
            return Err(Error::Corrupt);
        }
        let bps = 1u64 << self.log_blocks_per_seg;
        let mut addr = self.nat_blkaddr + (block_off << 1) - (block_off & (bps - 1));
        if self.nat_bitmap[block_off as usize / 8] & (0x80 >> (block_off % 8)) != 0 {
            addr += bps;
        }
        let mut b = [0u8; BLOCK];
        self.read(addr, &mut b)?;
        let e = (nid % NAT_PER_BLOCK) as usize * 9;
        Ok(u32le(&b, e + 5))
    }

    /// A node block (an inode, or a direct or indirect node), checked against its footer's nid.
    fn node(&mut self, nid: u32, buf: &mut [u8; BLOCK]) -> Result<(), Error> {
        let a = self.node_addr(nid)?;
        if a == 0 || a == u32::MAX {
            return Err(Error::Corrupt);
        }
        self.read(a as u64, buf)?;
        if u32le(buf, BLOCK - 24) != nid {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    /// Read inode `ino`.
    pub fn inode(&mut self, ino: u32) -> Result<Inode, Error> {
        let mut block = [0u8; BLOCK];
        self.node(ino, &mut block)?;
        let inode = Inode { number: ino, mode: u16le(&block, 0), size: u64le(&block, 16), inline: block[3], block };
        if inode.block[2] & ENCRYPT_FL != 0 || u32le(&inode.block, 80) & COMPR_FL != 0 {
            return Err(Error::Unsupported);
        }
        Ok(inode)
    }

    pub fn root(&mut self) -> Result<Inode, Error> {
        self.inode(self.root_ino)
    }

    /// The disk block of file block `i` (0 = a hole).
    pub fn map(&mut self, ino: &Inode, i: u64) -> Result<u32, Error> {
        let direct = ino.addrs(self.features) as u64;
        if i < direct {
            return Ok(ino.addr(i as usize));
        }
        let i = i - direct;
        let nid = |k: usize| u32le(&ino.block, I_NID + k * 4);
        let mut b = [0u8; BLOCK];
        // Two direct nodes, two indirect, one double-indirect (the inode's i_nid[0..5]): the node to start from, its depth, the index within it.
        let (a, n, ni) = (ADDRS_PER_BLOCK, NIDS_PER_BLOCK, NIDS_PER_BLOCK * ADDRS_PER_BLOCK);
        let (mut n_at, depth, mut i) = if i < 2 * a {
            (nid((i / a) as usize), 0u32, i % a)
        } else if i - 2 * a < 2 * ni {
            let j = i - 2 * a;
            (nid(2 + (j / ni) as usize), 1, j % ni)
        } else {
            let j = i - 2 * a - 2 * ni;
            if j >= n * ni {
                return Err(Error::Corrupt);
            }
            (nid(4), 2, j)
        };
        let mut span = a * n.pow(depth);
        for _ in 0..depth {
            if n_at == 0 {
                return Ok(0);
            }
            self.node(n_at, &mut b)?;
            span /= NIDS_PER_BLOCK;
            n_at = u32le(&b, (i / span) as usize * 4);
            i %= span;
        }
        if n_at == 0 {
            return Ok(0);
        }
        self.node(n_at, &mut b)?;
        Ok(u32le(&b, i as usize * 4))
    }

    /// Read from `offset` into `buf`; returns the bytes read (short at the end of the file).
    pub fn read_at(&mut self, ino: &Inode, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        if offset >= ino.size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(ino.size - offset) as usize;
        if ino.inline & INLINE_DATA != 0 {
            let d = ino.inline_data(self.features);
            let end = (offset as usize + n).min(d.len());
            let got = end.saturating_sub(offset as usize);
            buf[..got].copy_from_slice(&d[offset as usize..end]);
            buf[got..n].fill(0);
            return Ok(n);
        }
        let mut b = [0u8; BLOCK];
        let mut done = 0;
        while done < n {
            let pos = offset + done as u64;
            let within = (pos % BLOCK as u64) as usize;
            let take = (BLOCK - within).min(n - done);
            let a = self.map(ino, pos / BLOCK as u64)?;
            if a == 0 || a == u32::MAX {
                buf[done..done + take].fill(0);
            } else {
                self.read(a as u64, &mut b)?;
                buf[done..done + take].copy_from_slice(&b[within..within + take]);
            }
            done += take;
        }
        Ok(n)
    }

    /// A name in a directory: its inode number.
    pub fn lookup(&mut self, dir: &Inode, name: &[u8]) -> Result<u32, Error> {
        if !dir.is_dir() {
            return Err(Error::NotDir);
        }
        if dir.inline & INLINE_DENTRY != 0 {
            // Inline: as many slots as fit the inline area at 11 + 8 bytes and one bitmap bit each, the bitmap, reserved bytes, entries, names.
            let d = dir.inline_data(self.features);
            let slots = d.len() * 8 / ((11 + 8) * 8 + 1);
            let bitmap = slots.div_ceil(8);
            let reserved = d.len() - (slots * (11 + 8) + bitmap);
            return find(d, slots, 0, bitmap + reserved, bitmap + reserved + slots * 11, name).ok_or(Error::NotFound);
        }
        let mut b = [0u8; BLOCK];
        for i in 0..dir.size.div_ceil(BLOCK as u64) {
            let a = self.map(dir, i)?;
            if a == 0 || a == u32::MAX {
                continue;
            }
            self.read(a as u64, &mut b)?;
            if let Some(ino) = find(&b, DENTRY_SLOTS, 0, 30, 30 + DENTRY_SLOTS * 11, name) {
                return Ok(ino);
            }
        }
        Err(Error::NotFound)
    }

    /// Every entry of a directory, to `f(name, ino)`.
    pub fn list(&mut self, dir: &Inode, f: &mut dyn FnMut(&[u8], u32)) -> Result<(), Error> {
        if !dir.is_dir() {
            return Err(Error::NotDir);
        }
        if dir.inline & INLINE_DENTRY != 0 {
            let d = dir.inline_data(self.features);
            let slots = d.len() * 8 / ((11 + 8) * 8 + 1);
            let bitmap = slots.div_ceil(8);
            let reserved = d.len() - (slots * (11 + 8) + bitmap);
            each(d, slots, 0, bitmap + reserved, bitmap + reserved + slots * 11, f);
            return Ok(());
        }
        let mut b = [0u8; BLOCK];
        for i in 0..dir.size.div_ceil(BLOCK as u64) {
            let a = self.map(dir, i)?;
            if a != 0 && a != u32::MAX {
                self.read(a as u64, &mut b)?;
                each(&b, DENTRY_SLOTS, 0, 30, 30 + DENTRY_SLOTS * 11, f);
            }
        }
        Ok(())
    }

    /// A path from the root, `/`-separated.
    pub fn open(&mut self, path: &[u8]) -> Result<Inode, Error> {
        let mut cur = self.root()?;
        for part in path.split(|&c| c == b'/').filter(|p| !p.is_empty()) {
            let ino = self.lookup(&cur, part)?;
            cur = self.inode(ino)?;
        }
        Ok(cur)
    }
}

/// Walk a dentry area: `slots` entries, their bitmap at `bm` (bit k of byte k/8, least significant first), entries (hash u32, ino u32, name length u16, type u8) at `ents`, 8-byte name slots at `names`; a long name takes consecutive slots.
fn each(d: &[u8], slots: usize, bm: usize, ents: usize, names: usize, f: &mut dyn FnMut(&[u8], u32)) {
    let mut k = 0;
    while k < slots {
        if d[bm + k / 8] & (1 << (k % 8)) == 0 {
            k += 1;
            continue;
        }
        let e = ents + k * 11;
        let ino = u32le(d, e + 4);
        let len = u16le(d, e + 8) as usize;
        let start = names + k * 8;
        if len == 0 || start + len > d.len() {
            k += 1;
            continue;
        }
        f(&d[start..start + len], ino);
        k += len.div_ceil(8);
    }
}

fn find(d: &[u8], slots: usize, bm: usize, ents: usize, names: usize, name: &[u8]) -> Option<u32> {
    let mut hit = None;
    each(d, slots, bm, ents, names, &mut |n, ino| {
        if hit.is_none() && n == name {
            hit = Some(ino);
        }
    });
    hit
}
