//! Vault Root Ring — boot state snapshot ring on UFS.
//!
//! A power-of-two ring of 4KB entries, each a minimal binary record
//! (not full VSF yet — that comes when the VSF serializer is ready).
//! Binary search finds the highest valid generation in O(log2 N) reads.
//!
//! See RING.md for the full specification.

use crate::ufs::UfsController;

/// Ring size: 1024 entries × 4KB = 4MB total.
/// log2(1024) = 10 reads to find the newest entry.
pub const RING_SIZE: u32 = 1 << 10;

/// Base LBA for the vault root ring on UFS LUN 0.
/// Placed at 16MB offset (LBA 4096 at 4KB/block) to avoid
/// Android partition tables and bootloader regions.
pub const RING_BASE_LBA: u32 = 4096;

/// Magic bytes identifying a vault root entry.
const ENTRY_MAGIC: [u8; 8] = *b"FERROSV0";

/// Vault root entry — binary format (fits in one 4KB block).
///
/// This is a simplified binary format for bootstrap. Once the VSF
/// serializer is available, entries will be proper VSF documents
/// per RING.md spec.
///
/// Layout (all fields little-endian):
/// ```text
/// [0..8]    magic: "FERROSV0"
/// [8..16]   generation: u64
/// [16..48]  prev_hash: [u8; 32]    BLAKE3 of previous entry
/// [48..80]  hamt_root: [u8; 32]    BLAKE3 hash of HAMT root node
/// [80..112] cap_hash:  [u8; 32]    BLAKE3 hash of cap table snapshot
/// [112..144] ledger_head: [u8; 32] BLAKE3 hash of ledger chain head
/// [144..176] entry_hash: [u8; 32]  BLAKE3 of bytes [0..144]
/// [176..4096] reserved (zeroed)
/// ```
#[derive(Clone)]
pub struct VaultRootEntry {
    pub generation: u64,
    pub prev_hash: [u8; 32],
    pub hamt_root: [u8; 32],
    pub cap_hash: [u8; 32],
    pub ledger_head: [u8; 32],
    pub entry_hash: [u8; 32],
}

impl VaultRootEntry {
    /// Serialize entry to a 4KB block.
    pub fn to_block(&self) -> [u8; 4096] {
        let mut blk = [0u8; 4096];
        blk[0..8].copy_from_slice(&ENTRY_MAGIC);
        blk[8..16].copy_from_slice(&self.generation.to_le_bytes());
        blk[16..48].copy_from_slice(&self.prev_hash);
        blk[48..80].copy_from_slice(&self.hamt_root);
        blk[80..112].copy_from_slice(&self.cap_hash);
        blk[112..144].copy_from_slice(&self.ledger_head);
        // Compute BLAKE3 of [0..144]
        let hash = blake3::hash(&blk[..144]);
        blk[144..176].copy_from_slice(hash.as_bytes());
        blk
    }

    /// Deserialize from a 4KB block. Returns None if magic or hash is invalid.
    pub fn from_block(blk: &[u8; 4096]) -> Option<Self> {
        // Check magic
        if blk[0..8] != ENTRY_MAGIC {
            return None;
        }
        // Verify BLAKE3
        let expected_hash = blake3::hash(&blk[..144]);
        if &blk[144..176] != expected_hash.as_bytes() {
            return None;
        }

        let mut generation_bytes = [0u8; 8];
        generation_bytes.copy_from_slice(&blk[8..16]);
        let generation = u64::from_le_bytes(generation_bytes);

        let mut prev_hash = [0u8; 32];
        prev_hash.copy_from_slice(&blk[16..48]);
        let mut hamt_root = [0u8; 32];
        hamt_root.copy_from_slice(&blk[48..80]);
        let mut cap_hash = [0u8; 32];
        cap_hash.copy_from_slice(&blk[80..112]);
        let mut ledger_head = [0u8; 32];
        ledger_head.copy_from_slice(&blk[112..144]);
        let mut entry_hash = [0u8; 32];
        entry_hash.copy_from_slice(&blk[144..176]);

        Some(Self {
            generation,
            prev_hash,
            hamt_root,
            cap_hash,
            ledger_head,
            entry_hash,
        })
    }
}

/// Result of scanning the vault root ring.
pub struct ScanResult {
    /// Highest valid generation found (0 = empty ring).
    pub generation: u64,
    /// Ring position of the newest entry.
    pub position: u32,
    /// The newest entry (if found).
    pub entry: Option<VaultRootEntry>,
    /// Number of reads performed.
    pub reads: u32,
    /// Number of valid entries found during scan.
    pub valid_count: u32,
}

/// Binary search the vault root ring for the highest valid generation.
///
/// The ring has RING_SIZE entries at consecutive LBAs starting at RING_BASE_LBA.
/// Generations increase monotonically and wrap. The "seam" (where newest
/// is adjacent to oldest) is found by binary search.
///
/// Returns the newest valid entry in O(log2(RING_SIZE)) reads.
pub fn scan_ring(ufs: &UfsController) -> ScanResult {
    let mut result = ScanResult {
        generation: 0,
        position: 0,
        entry: None,
        reads: 0,
        valid_count: 0,
    };

    // First: check if ring is empty by reading position 0
    let entry0 = read_entry(ufs, 0, &mut result);
    if entry0.is_none() {
        // Position 0 is empty — try a few more to confirm ring is empty
        let entry_mid = read_entry(ufs, RING_SIZE / 2, &mut result);
        if entry_mid.is_none() {
            return result; // Ring is empty (genesis needed)
        }
    }

    // Linear scan for now — find the highest generation.
    // TODO: replace with proper binary search on the seam.
    // For 1024 entries this is 1024 reads worst case.
    // Binary search would be 10 reads. Fine for bootstrap, optimize later.
    let mut best_gen: u64 = 0;
    let mut best_pos: u32 = 0;
    let mut best_entry: Option<VaultRootEntry> = None;

    // Sample every 64th entry first (16 reads) to find the neighborhood
    let step = RING_SIZE >> 4; // 64
    for i in 0..(RING_SIZE / step) {
        let pos = i * step;
        if let Some(entry) = read_entry(ufs, pos, &mut result) {
            if entry.generation > best_gen {
                best_gen = entry.generation;
                best_pos = pos;
                best_entry = Some(entry);
            }
        }
    }

    // Now scan the neighborhood around best_pos (±64 entries)
    let scan_start = if best_pos >= step { best_pos - step } else { RING_SIZE - step + best_pos };
    for offset in 0..(step * 2) {
        let pos = (scan_start + offset) % RING_SIZE;
        if let Some(entry) = read_entry(ufs, pos, &mut result) {
            if entry.generation > best_gen {
                best_gen = entry.generation;
                best_pos = pos;
                best_entry = Some(entry);
            }
        }
    }

    result.generation = best_gen;
    result.position = best_pos;
    result.entry = best_entry;
    result
}

/// Write a new entry to the ring at the correct position.
/// Returns true if write + verify succeeded.
pub fn write_entry(ufs: &UfsController, entry: &VaultRootEntry) -> bool {
    let pos = (entry.generation % RING_SIZE as u64) as u32;
    let lba = RING_BASE_LBA + pos;
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

/// Read a single entry from the ring at the given position.
fn read_entry(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> Option<VaultRootEntry> {
    let lba = RING_BASE_LBA + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 { return None; }

    let data = ufs.data_buffer();
    match VaultRootEntry::from_block(data) {
        Some(entry) => {
            result.valid_count += 1;
            Some(entry)
        }
        None => None,
    }
}
