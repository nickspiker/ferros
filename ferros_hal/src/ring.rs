//! Boot State Ring — generation-ordered ring of VSF documents on UFS/SD.
//!
//! Each ring entry is a proper VSF document (starts with RÅ<, EWE-encoded
//! fields, provenance hash). Any standard VSF reader can parse them.
//!
//! Binary search finds the highest valid generation in exactly 16 reads
//! (log2(65536)). Generation starts at 1 (0 = empty slot).
//!
//! See RING.md for the full specification.

use crate::ufs::UfsController;
use crate::vsf_mini::{VsfWriter, VsfReader, read_qtimer};

/// Ring size: 65536 entries × 4KB = 256MB total.
pub const RING_SIZE: u32 = 1 << 16;

/// Base block for the ring on UFS LUN 0 (4KB blocks).
pub const RING_BASE_BLOCK: u32 = 0x2000;

/// Convert generation number to ring position.
/// Generation 1 → position 0, generation 2 → position 1, etc.
/// Generation 0 is invalid (means empty slot).
#[inline]
pub fn gen_to_pos(generation: u64) -> u32 {
    ((generation - 1) % RING_SIZE as u64) as u32
}

/// Ring entry fields extracted from a VSF document.
#[derive(Clone)]
pub struct RingEntry {
    pub generation: u64,
    pub prev_hash: [u8; 32],
    pub resume_state: [u8; 32],
    pub eagle_time: u64,
    pub hp_hash: [u8; 32],
}

impl RingEntry {
    /// Serialize to a 4KB VSF document.
    ///
    /// Format:
    /// ```text
    /// RÅ<                          magic (4 bytes)
    /// z(0)                         version: Zil
    /// y(0)                         backward compat: Zil
    /// b(header_len)                header byte length
    /// e(u(qtimer_ticks))           Eagle Time (QTIMER for now)
    /// hp(document_hash)            provenance hash (placeholder, filled after)
    /// n(3)                         3 body fields
    /// >                            header close
    /// u(generation)                ordering: binary search key
    /// hp(prev_entry_hash)          ordering: chain link to previous
    /// hp(resume_state_hash)        pointer: HAMT root for state restore
    /// [zero padding to 4096]
    /// ```
    pub fn to_block(&self) -> [u8; 4096] {
        let mut blk = [0u8; 4096];
        let mut w = VsfWriter::new(&mut blk);

        // VSF header
        w.magic();
        w.version(0);           // Zil
        w.backward_version(0);  // Zil

        // Header length placeholder — we'll compute this after writing the header.
        // For simplicity, write a fixed estimate. The header is small and predictable.
        // We know: e(u(8bytes)) + hp(32bytes) + n(1byte) = ~50 bytes header body.
        // Actual header_length = bytes from after b() to '>'.
        let header_len_pos = w.pos();
        w.header_length(0); // placeholder, overwritten below

        let header_body_start = w.pos();

        // Eagle Time (QTIMER ticks — monotonic, not wall clock)
        w.eagle_time_qtimer(self.eagle_time);

        // Provenance hash placeholder — filled after body is written
        let hp_pos = match w.hash_p_placeholder() {
            Some(pos) => pos,
            None => return blk,
        };

        // Field count: 3 body fields (generation, prev_hash, resume_state)
        w.field_count(3);

        // Header close
        w.close();

        let header_body_end = w.pos();

        // --- Body fields ---
        let body_start = w.pos();

        // 1. Generation (ordering key for binary search)
        w.uint(self.generation);

        // 2. Previous entry provenance hash (chain link)
        w.hash_p(&self.prev_hash);

        // 3. Resume state HAMT root (single pointer to everything)
        w.hash_p(&self.resume_state);

        let body_end = w.pos();
        let header_body_len = header_body_end - header_body_start - 1; // -1 for '>'

        // Drop writer to release mutable borrow on blk
        drop(w);

        // Compute provenance hash over body
        let body_hash = blake3::hash(&blk[body_start..body_end]);
        blk[hp_pos..hp_pos + 32].copy_from_slice(body_hash.as_bytes());

        // Fill in header_length: b(0) was written as 'b' '3' 0x00.
        // Overwrite the value byte (pos+2) with actual length.
        if header_body_len <= 255 {
            blk[header_len_pos + 2] = header_body_len as u8;
        }

        blk
    }

    /// Deserialize from a 4KB block. Returns None if not a valid VSF ring entry.
    pub fn from_block(blk: &[u8; 4096]) -> Option<Self> {
        let mut r = VsfReader::new(blk);

        // Verify magic
        if !r.magic() { return None; }

        // Version
        let _ver = r.version()?;
        let _bver = r.backward_version()?;
        let _hlen = r.header_length()?;

        // Eagle Time
        let eagle_time = r.eagle_time_qtimer()?;

        // Provenance hash
        let hp_hash_ref = r.hash_p()?;
        let mut hp_hash = [0u8; 32];
        hp_hash.copy_from_slice(hp_hash_ref);

        // Field count
        let _count = r.field_count()?;

        // Header close
        if !r.close() { return None; }

        // --- Body ---
        let body_start = r.pos;

        // 1. Generation
        let generation = r.uint()?;
        if generation == 0 { return None; } // 0 = invalid/empty

        // 2. Previous hash
        let prev_ref = r.hash_p()?;
        let mut prev_hash = [0u8; 32];
        prev_hash.copy_from_slice(prev_ref);

        // 3. Resume state
        let resume_ref = r.hash_p()?;
        let mut resume_state = [0u8; 32];
        resume_state.copy_from_slice(resume_ref);

        let body_end = r.pos;

        // Verify provenance hash covers the body
        let computed = blake3::hash(&blk[body_start..body_end]);
        if computed.as_bytes() != &hp_hash {
            return None;
        }

        Some(Self {
            generation,
            prev_hash,
            resume_state,
            eagle_time,
            hp_hash,
        })
    }

    /// Genesis entry — generation 1, all hashes zeroed, current QTIMER.
    pub fn genesis() -> Self {
        Self {
            generation: 1,
            prev_hash: [0u8; 32],
            resume_state: [0u8; 32],
            eagle_time: read_qtimer(),
            hp_hash: [0u8; 32], // computed in to_block()
        }
    }

    /// Create next entry in the chain from this entry.
    pub fn next(&self) -> Self {
        Self {
            generation: self.generation + 1,
            prev_hash: self.hp_hash,
            resume_state: [0u8; 32], // caller fills this in
            eagle_time: read_qtimer(),
            hp_hash: [0u8; 32], // computed in to_block()
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

/// Binary search the ring for the highest valid generation.
///
/// Exactly 2 × log2(RING_SIZE) generation reads + 1 full entry read.
/// Empty slots return generation 0, always lower than any real entry.
pub fn scan_ring(ufs: &UfsController) -> ScanResult {
    let mut result = ScanResult {
        generation: 0,
        position: 0,
        entry: None,
        reads: 0,
    };

    let mut lo: u32 = 0;
    let mut size: u32 = RING_SIZE;

    for _ in 0..16 {
        let half = size >> 1;
        let mid = (lo + half) % RING_SIZE;
        let gen_lo = read_generation(ufs, lo, &mut result);
        let gen_mid = read_generation(ufs, mid, &mut result);

        if gen_mid > gen_lo {
            lo = mid;
        }
        size = half;
    }

    // Read the full entry at the converged position.
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

    // Read back and verify
    let read_ocs = ufs.read_block(lba);
    if read_ocs != 0 { return false; }

    let readback = ufs.data_buffer();
    readback == &blk
}

/// Read just the generation from a VSF ring entry at a position.
/// Returns 0 if empty, corrupt, or unreadable.
fn read_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> u64 {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 { return 0; }

    let data = ufs.data_buffer();

    // Quick parse: just read enough to extract generation.
    // Full validation happens in read_entry.
    let mut r = VsfReader::new(data);
    if !r.magic() { return 0; }
    if r.version().is_none() { return 0; }
    if r.backward_version().is_none() { return 0; }
    if r.header_length().is_none() { return 0; }
    if r.eagle_time_qtimer().is_none() { return 0; }
    if r.hash_p().is_none() { return 0; }
    if r.field_count().is_none() { return 0; }
    if !r.close() { return 0; }

    // First body field should be u(generation)
    r.uint().unwrap_or(0)
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
