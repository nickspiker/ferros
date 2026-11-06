//! ferros-flash: the partition steps of putting ferros on a phone, as a library (the CLI now, the fluor installer later, both on this).
//! A Pixel's GPT is edited surgically, never cleared and recreated: a fresh table loses the A/B slot attributes the bootloader keeps in the entries, and the phone no longer boots. The carve keeps every entry as it is except two: userdata ends earlier, and `ferros` takes the space after it. When every entry slot is in use (Pixels ship 34, all used), the entry count grows into the blocks the table already reserves before the first partition, so nothing moves.
//! Everything works on images of the disk's two ends (the head from LBA 0, the tail ending at the last LBA), never on a device: reading them off the phone and writing the changed blocks back is the caller's (tools/pixel-carve.sh).

/// GPT type of a ferros partition: BLAKE3("ferros") truncated to 128 bits (RING.md), A0B51225-61C5-0F5A-FFE7-1B644F9CA954, in GPT's on-disk order (the first three fields little-endian).
pub const FERROS_TYPE: [u8; 16] = [0x25, 0x12, 0xB5, 0xA0, 0xC5, 0x61, 0x5A, 0x0F, 0xFF, 0xE7, 0x1B, 0x64, 0x4F, 0x9C, 0xA9, 0x54];
/// The entry count a full table grows to: 64 entries of 128 bytes = 8 KiB, two 4 KiB blocks.
pub const GROWN_COUNT: u32 = 64;
/// Partition starts are kept on 1 MiB boundaries.
pub const ALIGN_BYTES: u64 = 1 << 20;

#[derive(Debug)]
pub enum Error {
    Short(&'static str),
    NoSignature(&'static str),
    HeaderCrc(&'static str),
    EntriesCrc(&'static str),
    Mismatch(&'static str),
    Plan(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Error::Short(w) => write!(f, "{w}: the image is too short"),
            Error::NoSignature(w) => write!(f, "{w}: no \"EFI PART\" signature"),
            Error::HeaderCrc(w) => write!(f, "{w}: header CRC does not match"),
            Error::EntriesCrc(w) => write!(f, "{w}: entry array CRC does not match"),
            Error::Mismatch(w) => write!(f, "primary and backup disagree: {w}"),
            Error::Plan(s) => write!(f, "{s}"),
        }
    }
}

/// CRC-32 (IEEE 802.3, reflected, as GPT uses).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0xEDB8_8320 & (c & 1).wrapping_neg());
        }
    }
    !c
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// The header's own bytes (header_size of them), kept so fields this tool does not know survive.
    pub raw: Vec<u8>,
    /// The whole block the header sits in: a write keeps what follows the header (akita's backup header block carries a stale copy of entries after byte 92; nothing reads it, and a surgical edit leaves it).
    pub block_raw: Vec<u8>,
    pub my_lba: u64,
    pub alt_lba: u64,
    pub first_usable: u64,
    pub last_usable: u64,
    pub disk_guid: [u8; 16],
    pub entries_lba: u64,
    pub count: u32,
    pub entry_size: u32,
}

impl Header {
    fn parse(block: &[u8], what: &'static str) -> Result<Header, Error> {
        if block.len() < 92 {
            return Err(Error::Short(what));
        }
        if &block[0..8] != b"EFI PART" {
            return Err(Error::NoSignature(what));
        }
        let size = u32_at(block, 12) as usize;
        if size < 92 || size > block.len() {
            return Err(Error::Short(what));
        }
        let mut raw = block[..size].to_vec();
        let crc = u32_at(&raw, 16);
        raw[16..20].fill(0);
        if crc32(&raw) != crc {
            return Err(Error::HeaderCrc(what));
        }
        Ok(Header {
            my_lba: u64_at(&raw, 24),
            alt_lba: u64_at(&raw, 32),
            first_usable: u64_at(&raw, 40),
            last_usable: u64_at(&raw, 48),
            disk_guid: raw[56..72].try_into().unwrap(),
            entries_lba: u64_at(&raw, 72),
            count: u32_at(&raw, 80),
            entry_size: u32_at(&raw, 84),
            raw,
            block_raw: block.to_vec(),
        })
    }

    /// The header as written: its fields over the kept bytes, the entries' CRC given, its own CRC computed.
    fn encode(&self, entries_crc: u32) -> Vec<u8> {
        let mut r = self.raw.clone();
        r[24..32].copy_from_slice(&self.my_lba.to_le_bytes());
        r[32..40].copy_from_slice(&self.alt_lba.to_le_bytes());
        r[40..48].copy_from_slice(&self.first_usable.to_le_bytes());
        r[48..56].copy_from_slice(&self.last_usable.to_le_bytes());
        r[56..72].copy_from_slice(&self.disk_guid);
        r[72..80].copy_from_slice(&self.entries_lba.to_le_bytes());
        r[80..84].copy_from_slice(&self.count.to_le_bytes());
        r[84..88].copy_from_slice(&self.entry_size.to_le_bytes());
        r[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        r[16..20].fill(0);
        let c = crc32(&r);
        r[16..20].copy_from_slice(&c.to_le_bytes());
        r
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The entry's own bytes (entry_size of them): attributes and anything past the name survive untouched.
    pub raw: Vec<u8>,
}

impl Entry {
    pub fn type_guid(&self) -> [u8; 16] {
        self.raw[0..16].try_into().unwrap()
    }
    pub fn empty(&self) -> bool {
        self.type_guid() == [0; 16]
    }
    pub fn first(&self) -> u64 {
        u64_at(&self.raw, 32)
    }
    pub fn last(&self) -> u64 {
        u64_at(&self.raw, 40)
    }
    pub fn attrs(&self) -> u64 {
        u64_at(&self.raw, 48)
    }
    pub fn set_last(&mut self, lba: u64) {
        self.raw[40..48].copy_from_slice(&lba.to_le_bytes());
    }
    /// The name, UTF-16LE up to 36 units, as far as it is ASCII (every Pixel name is).
    pub fn name(&self) -> String {
        self.raw[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).map(|u| if u < 128 { u as u8 as char } else { '?' }).collect()
    }
    pub fn new(size: usize, type_guid: [u8; 16], unique: [u8; 16], first: u64, last: u64, name: &str) -> Entry {
        let mut raw = vec![0u8; size];
        raw[0..16].copy_from_slice(&type_guid);
        raw[16..32].copy_from_slice(&unique);
        raw[32..40].copy_from_slice(&first.to_le_bytes());
        raw[40..48].copy_from_slice(&last.to_le_bytes());
        for (i, b) in name.bytes().take(36).enumerate() {
            raw[56 + 2 * i] = b;
        }
        Entry { raw }
    }
}

/// A disk's GPT, read from the two ends of the disk.
#[derive(Clone, Debug)]
pub struct Gpt {
    pub block: usize,
    pub primary: Header,
    pub backup: Header,
    /// Every entry slot, empty ones included, `primary.count` of them.
    pub entries: Vec<Entry>,
}

/// The blocks a table change writes: `lba` and the bytes from there (whole blocks).
#[derive(Clone, Debug)]
pub struct Write {
    pub lba: u64,
    pub bytes: Vec<u8>,
}

fn entries_of(img: &[u8], off: usize, h: &Header, what: &'static str) -> Result<Vec<Entry>, Error> {
    let len = h.count as usize * h.entry_size as usize;
    if img.len() < off + len {
        return Err(Error::Short(what));
    }
    let arr = &img[off..off + len];
    let crc = u32_at(&h.raw, 88);
    if crc32(arr) != crc {
        return Err(Error::EntriesCrc(what));
    }
    Ok(arr.chunks(h.entry_size as usize).map(|c| Entry { raw: c.to_vec() }).collect())
}

impl Gpt {
    /// `head` from LBA 0; `tail` ending at the disk's last LBA, starting at `tail_lba`. Both headers and both arrays are checked (CRCs, and that they describe the same table).
    pub fn read(block: usize, head: &[u8], tail: &[u8], tail_lba: u64) -> Result<Gpt, Error> {
        if head.len() < 2 * block {
            return Err(Error::Short("head"));
        }
        let primary = Header::parse(&head[block..2 * block], "primary header")?;
        if primary.my_lba != 1 {
            return Err(Error::Mismatch("the primary header is not at LBA 1"));
        }
        let entries = entries_of(head, primary.entries_lba as usize * block, &primary, "primary entries")?;
        if primary.alt_lba < tail_lba || ((primary.alt_lba - tail_lba) as usize + 1) * block > tail.len() {
            return Err(Error::Plan(format!("the tail image (from LBA A#{tail_lba}, {} bytes) does not hold the backup header at LBA A#{}", tail.len(), primary.alt_lba)));
        }
        let bo = (primary.alt_lba - tail_lba) as usize * block;
        let backup = Header::parse(&tail[bo..bo + block], "backup header")?;
        if backup.entries_lba < tail_lba {
            return Err(Error::Short("tail: the backup entries start before it"));
        }
        let backup_entries = entries_of(tail, (backup.entries_lba - tail_lba) as usize * block, &backup, "backup entries")?;
        if backup.my_lba != primary.alt_lba || backup.alt_lba != 1 {
            return Err(Error::Mismatch("header locations"));
        }
        if backup.first_usable != primary.first_usable || backup.last_usable != primary.last_usable || backup.disk_guid != primary.disk_guid {
            return Err(Error::Mismatch("usable range or disk GUID"));
        }
        if backup_entries != entries {
            return Err(Error::Mismatch("entry arrays"));
        }
        Ok(Gpt { block, primary, backup, entries })
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| !e.empty() && e.name() == name)
    }

    /// Room for entries before the first partition (primary) and before the backup header (backup), in entries.
    pub fn entry_room(&self) -> u32 {
        let es = self.primary.entry_size as u64;
        let front = (self.primary.first_usable - self.primary.entries_lba) * self.block as u64 / es;
        let back = (self.backup.my_lba - self.backup.entries_lba) * self.block as u64 / es;
        front.min(back) as u32
    }

    /// The carve: userdata keeps its start and ends after `userdata_bytes` (rounded down to whole 1 MiB), and `ferros` (FERROS_TYPE, its unique GUID from the disk's) takes from the next 1 MiB boundary to the last usable LBA. Refused unless userdata is the last partition and runs to the end of the usable range, and unless there is no `ferros` yet.
    pub fn carve(&mut self, userdata_bytes: u64) -> Result<(), Error> {
        if self.find("ferros").is_some() {
            return Err(Error::Plan("there is a ferros partition already".into()));
        }
        let u = self.find("userdata").ok_or(Error::Plan("no userdata partition".into()))?;
        let ud = &self.entries[u];
        if ud.last() != self.primary.last_usable {
            return Err(Error::Plan(format!("userdata ends at LBA A#{}, not at the last usable LBA A#{}", ud.last(), self.primary.last_usable)));
        }
        if self.entries.iter().any(|e| !e.empty() && e.first() > ud.first()) {
            return Err(Error::Plan("userdata is not the last partition".into()));
        }
        let b = self.block as u64;
        let align = ALIGN_BYTES / b;
        let start = ud.first();
        let blocks = userdata_bytes / ALIGN_BYTES * align;
        let ferros_first = (start + blocks).div_ceil(align) * align;
        if blocks == 0 || ferros_first > self.primary.last_usable {
            return Err(Error::Plan(format!("userdata of A#{userdata_bytes} bytes leaves nothing for ferros")));
        }
        // A free slot below the count, else the count grows into the reserved blocks.
        let slot = match self.entries.iter().position(|e| e.empty()) {
            Some(s) => s,
            None => {
                let room = self.entry_room();
                if room < GROWN_COUNT {
                    return Err(Error::Plan(format!("every entry slot is used and the table has room for only A#{room}")));
                }
                let s = self.entries.len();
                let es = self.primary.entry_size as usize;
                self.entries.resize(GROWN_COUNT as usize, Entry { raw: vec![0; es] });
                self.primary.count = GROWN_COUNT;
                self.backup.count = GROWN_COUNT;
                s
            }
        };
        self.entries[u].set_last(start + blocks - 1);
        let h = blake3::Hasher::new().update(b"ferros partition").update(&self.primary.disk_guid).finalize();
        let mut unique: [u8; 16] = h.as_bytes()[..16].try_into().unwrap();
        // RFC 4122 version 4 / variant 1 bits, as a random GUID carries.
        unique[7] = (unique[7] & 0x0F) | 0x40;
        unique[8] = (unique[8] & 0x3F) | 0x80;
        self.entries[slot] = Entry::new(self.primary.entry_size as usize, FERROS_TYPE, unique, ferros_first, self.primary.last_usable, "ferros");
        Ok(())
    }

    fn array(&self) -> Vec<u8> {
        let mut a: Vec<u8> = self.entries.iter().flat_map(|e| e.raw.iter().copied()).collect();
        let b = self.block;
        a.resize(a.len().div_ceil(b) * b, 0);
        a
    }

    /// The blocks to write for this table: primary header and array, backup array and header. Written in this order the backup goes last, so an interrupted write leaves a primary the bootloader reads.
    pub fn writes(&self) -> Vec<Write> {
        let arr = self.array();
        let crc = crc32(&arr[..self.entries.len() * self.primary.entry_size as usize]);
        let header = |h: &Header| {
            let mut v = h.block_raw.clone();
            let e = h.encode(crc);
            v[..e.len()].copy_from_slice(&e);
            v
        };
        vec![
            Write { lba: 1, bytes: header(&self.primary) },
            Write { lba: self.primary.entries_lba, bytes: arr.clone() },
            Write { lba: self.backup.entries_lba, bytes: arr },
            Write { lba: self.backup.my_lba, bytes: header(&self.backup) },
        ]
    }

    /// The images with this table written into them (for reading back with `read`, before anything goes to a device).
    pub fn apply(&self, head: &mut [u8], tail: &mut [u8], tail_lba: u64) {
        for w in self.writes() {
            let (img, off) = if w.lba < tail_lba { (&mut *head, w.lba as usize * self.block) } else { (&mut *tail, (w.lba - tail_lba) as usize * self.block) };
            img[off..off + w.bytes.len()].copy_from_slice(&w.bytes);
        }
    }

    /// One line per partition, in disk order.
    pub fn table(&self) -> String {
        let mut idx: Vec<usize> = (0..self.entries.len()).filter(|&i| !self.entries[i].empty()).collect();
        idx.sort_by_key(|&i| self.entries[i].first());
        let b = self.block as u64;
        let mut s = format!("block A#{}, entries A#{} (room for A#{}), usable LBA A#{}..=A#{}\n", self.block, self.entries.len(), self.entry_room(), self.primary.first_usable, self.primary.last_usable);
        for i in idx {
            let e = &self.entries[i];
            let bytes = (e.last() - e.first() + 1) * b;
            s += &format!("  slot A#{i:<3} {:<20} LBA A#{}..=A#{}  A#{} MiB{}\n", e.name(), e.first(), e.last(), bytes >> 20, if e.attrs() != 0 { format!("  attrs G#{:X}", e.attrs()) } else { String::new() });
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn ferros_type_is_blake3() {
        let h = blake3::hash(b"ferros");
        let g = &h.as_bytes()[..16];
        // GPT's mixed-endian form of the text GUID made from those bytes.
        let disk = [g[3], g[2], g[1], g[0], g[5], g[4], g[7], g[6], g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]];
        assert_eq!(disk, FERROS_TYPE);
    }
}
