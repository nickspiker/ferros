//! Boot State Ring — generation-ordered ring of boot snapshots on UFS/SD.
//!
//! A power-of-two ring of 4KB entries, each a binary record with BLAKE3
//! integrity. Binary search finds the highest valid generation in exactly
//! 16 reads (log2(65536)).
//!
//! See RING.md for the full specification.
//!
//! ## Storage Layout (unified 4KB blocks, same on UFS + SD)
//!
//! Ring lives at blocks 0x2000-0x11FFF (256MB, 65536 entries).
//! Each entry = one 4KB block = one generation snapshot.
//! Write position = generation % RING_SIZE.

use crate::ufs::UfsController;

/// Ring size: 65536 entries × 4KB = 256MB total.
/// log2(65536) = 16 reads to find the newest entry.
pub const RING_SIZE: u32 = 1 << 16;

/// Base block for the ring on UFS LUN 0 (4KB blocks).
/// 0x2000 = 32MB offset, above seed + kernel copies.
pub const RING_BASE_BLOCK: u32 = 0x2000;

/// Magic bytes identifying a ring entry.
const ENTRY_MAGIC: [u8; 8] = *b"FERROSV0";

/// Byte offset where the BLAKE3 entry hash begins.
const HASH_OFFSET: usize = 176;

/// Byte range that the BLAKE3 covers: [0..HASH_OFFSET].
const HASHED_RANGE: usize = HASH_OFFSET;

/// Boot state ring entry — binary format (fits in one 4KB block).
///
/// Layout (all fields little-endian):
/// ```text
/// [0..8]      magic: "FERROSV0"
/// [8..16]     generation: u64
/// [16..48]    prev_hash: [u8; 32]     BLAKE3 of previous entry
/// [48..80]    hamt_root: [u8; 32]     BLAKE3 of HAMT root node
/// [80..112]   cap_hash: [u8; 32]      BLAKE3 of capability table snapshot
/// [112..144]  proc_hash: [u8; 32]     BLAKE3 of process snapshot
/// [144..176]  ledger_head: [u8; 32]   BLAKE3 of ledger chain head
/// [176..208]  entry_hash: [u8; 32]    BLAKE3 of bytes [0..176]
/// [208..4096] reserved (zeroed)
/// ```
#[derive(Clone)]
pub struct RingEntry {
    pub generation: u64,
    pub prev_hash: [u8; 32],
    pub hamt_root: [u8; 32],
    pub cap_hash: [u8; 32],
    pub proc_hash: [u8; 32],
    pub ledger_head: [u8; 32],
    pub entry_hash: [u8; 32],
}

impl RingEntry {
    /// Serialize entry to a 4KB block.
    pub fn to_block(&self) -> [u8; 4096] {
        let mut blk = [0u8; 4096];
        blk[0..8].copy_from_slice(&ENTRY_MAGIC);
        blk[8..16].copy_from_slice(&self.generation.to_le_bytes());
        blk[16..48].copy_from_slice(&self.prev_hash);
        blk[48..80].copy_from_slice(&self.hamt_root);
        blk[80..112].copy_from_slice(&self.cap_hash);
        blk[112..144].copy_from_slice(&self.proc_hash);
        blk[144..176].copy_from_slice(&self.ledger_head);
        // Compute BLAKE3 of [0..HASH_OFFSET]
        let hash = blake3::hash(&blk[..HASHED_RANGE]);
        blk[HASH_OFFSET..HASH_OFFSET + 32].copy_from_slice(hash.as_bytes());
        blk
    }

    /// Deserialize from a 4KB block. Returns None if magic or hash is invalid.
    pub fn from_block(blk: &[u8; 4096]) -> Option<Self> {
        if blk[0..8] != ENTRY_MAGIC {
            return None;
        }
        let expected = blake3::hash(&blk[..HASHED_RANGE]);
        if &blk[HASH_OFFSET..HASH_OFFSET + 32] != expected.as_bytes() {
            return None;
        }

        let generation = u64::from_le_bytes([
            blk[8], blk[9], blk[10], blk[11],
            blk[12], blk[13], blk[14], blk[15],
        ]);

        let mut prev_hash = [0u8; 32];
        prev_hash.copy_from_slice(&blk[16..48]);
        let mut hamt_root = [0u8; 32];
        hamt_root.copy_from_slice(&blk[48..80]);
        let mut cap_hash = [0u8; 32];
        cap_hash.copy_from_slice(&blk[80..112]);
        let mut proc_hash = [0u8; 32];
        proc_hash.copy_from_slice(&blk[112..144]);
        let mut ledger_head = [0u8; 32];
        ledger_head.copy_from_slice(&blk[144..176]);
        let mut entry_hash = [0u8; 32];
        entry_hash.copy_from_slice(&blk[HASH_OFFSET..HASH_OFFSET + 32]);

        Some(Self {
            generation,
            prev_hash,
            hamt_root,
            cap_hash,
            proc_hash,
            ledger_head,
            entry_hash,
        })
    }

    /// Genesis entry — generation 1 at position 0.
    /// Generation starts at 1 because 0 means "empty slot" in the binary search.
    pub fn genesis() -> Self {
        Self {
            generation: 1,
            prev_hash: [0u8; 32],
            hamt_root: [0u8; 32],
            cap_hash: [0u8; 32],
            proc_hash: [0u8; 32],
            ledger_head: [0u8; 32],
            entry_hash: [0u8; 32], // computed in to_block()
        }
    }
}

/// Result of scanning the ring.
pub struct ScanResult {
    /// Highest valid generation found (0 = empty ring).
    pub generation: u64,
    /// Ring position of the newest entry.
    pub position: u32,
    /// The newest entry (if found).
    pub entry: Option<RingEntry>,
    /// Number of block reads performed.
    pub reads: u32,
}

/// Convert generation number to ring position.
/// Generation 1 → position 0, generation 2 → position 1, etc.
/// Generation 0 is invalid (means empty slot).
#[inline]
pub fn gen_to_pos(generation: u64) -> u32 {
    ((generation - 1) % RING_SIZE as u64) as u32
}

/// Binary search the ring for the highest valid generation.
///
/// Generations start at 1 (0 = empty slot). Position = (gen - 1) % N.
/// The "seam" where newest is adjacent to oldest is found by comparing
/// generations at midpoints. Empty slots read as 0, always lower than
/// any real entry.
///
/// Exactly log2(RING_SIZE) = 16 reads to find the newest entry.
pub fn scan_ring(ufs: &UfsController) -> ScanResult {
    let mut result = ScanResult {
        generation: 0,
        position: 0,
        entry: None,
        reads: 0,
    };

    // Binary search: compare generations at lo and mid.
    // The half with the higher generation contains the seam (newest entry).
    // Empty slots return 0, which is always < any real generation.
    let mut lo: u32 = 0;
    let mut size: u32 = RING_SIZE;

    for _ in 0..16 {
        let half = size >> 1;
        let mid = (lo + half) % RING_SIZE;
        let gen_lo = read_generation(ufs, lo, &mut result);
        let gen_mid = read_generation(ufs, mid, &mut result);

        if gen_mid > gen_lo {
            // Higher generation in the right half — seam is there
            lo = mid;
        }
        // Otherwise: higher (or equal) in the left half, lo stays
        size = half;
    }

    // lo has converged to the newest entry's position.
    // Read the full entry to return it.
    if let Some(entry) = read_entry(ufs, lo, &mut result) {
        result.generation = entry.generation;
        result.position = lo;
        result.entry = Some(entry);
    }

    result
}

/// Write a new entry to the ring. Returns true if write + read-back verify passed.
pub fn write_entry(ufs: &UfsController, entry: &RingEntry) -> bool {
    let pos = gen_to_pos(entry.generation);
    let lba = RING_BASE_BLOCK + pos;
    let blk = entry.to_block();

    // Write
    let buf = ufs.data_buffer_mut();
    buf.copy_from_slice(&blk);
    let write_ocs = ufs.write_block(lba);
    if write_ocs != 0 { return false; }

    // Read back and verify (write-verify protocol per RING.md)
    let read_ocs = ufs.read_block(lba);
    if read_ocs != 0 { return false; }

    let readback = ufs.data_buffer();
    readback == &blk
}

/// Read just the generation number from a ring position.
/// Returns 0 if the entry is empty, corrupt, or unreadable.
fn read_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> u64 {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 { return 0; }

    let data = ufs.data_buffer();

    // Quick check: magic valid?
    if data[0..8] != ENTRY_MAGIC { return 0; }

    // Quick check: BLAKE3 valid?
    let expected = blake3::hash(&data[..HASHED_RANGE]);
    if &data[HASH_OFFSET..HASH_OFFSET + 32] != expected.as_bytes() { return 0; }

    u64::from_le_bytes([
        data[8], data[9], data[10], data[11],
        data[12], data[13], data[14], data[15],
    ])
}

/// Read a full entry from the ring at the given position.
fn read_entry(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> Option<RingEntry> {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 { return None; }

    let data = ufs.data_buffer();
    RingEntry::from_block(data)
}
