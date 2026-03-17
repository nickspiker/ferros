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
    /// RÅ< z(7) y(7) b(header_len) e(u(qtimer)) hp(zeros→patched) n(1) >
    /// [d("ring")
    ///   (d("generation"):u(N))
    ///   (d("prev_hash"):hp(...))
    ///   (d("resume_state"):hp(...))
    /// ]
    /// [zero padding to 4096]
    ///
    /// hp = BLAKE3 of entire document with hp field zeroed to 32×0x00
    /// ```
    pub fn to_block(&self) -> [u8; 4096] {
        let mut blk = [0u8; 4096];
        let mut w = VsfWriter::new(&mut blk);

        // --- VSF header ---
        w.magic();
        w.version(7);           // Luna
        w.backward_version(7);  // Luna

        // Header length placeholder (b field)
        let header_len_pos = w.pos();
        w.header_length(0); // placeholder, patched below

        let header_body_start = w.pos();

        // Eagle Time (QTIMER ticks — monotonic, not wall clock yet)
        w.eagle_time_qtimer(self.eagle_time);

        // Provenance hash placeholder — 32 zero bytes, patched after full document is written
        let hp_pos = match w.hash_p_placeholder() {
            Some(pos) => pos,
            None => return blk,
        };

        // n(1) — one section in the body
        w.field_count(1);

        // Header close
        w.close();

        let header_body_end = w.pos();

        // --- Body: one section with 3 fields ---
        // Section name omitted from body (< 1MB from header per VSF spec).
        // Name "ring" lives in header TOC only.
        w.section_open_anonymous();

        // Field 1: generation (ordering key for binary search)
        w.field_open("generation");
        w.uint(self.generation);
        w.field_close();

        // Field 2: prev_hash (chain link to previous entry)
        w.field_open("prev_hash");
        w.hash_p(&self.prev_hash);
        w.field_close();

        // Field 3: resume_state (HAMT root pointer)
        w.field_open("resume_state");
        w.hash_p(&self.resume_state);
        w.field_close();

        w.section_close();

        // Patch header_length
        let header_body_len = header_body_end - header_body_start - 1; // -1 for '>'
        drop(w);

        if header_body_len <= 255 {
            blk[header_len_pos + 2] = header_body_len as u8;
        }

        // Compute provenance hash: BLAKE3 of entire document with hp zeroed
        // hp is already zeros (placeholder), so just hash the whole block
        let doc_hash = blake3::hash(&blk);
        blk[hp_pos..hp_pos + 32].copy_from_slice(doc_hash.as_bytes());

        blk
    }

    /// Deserialize from a 4KB block. Returns None if not a valid VSF ring entry.
    pub fn from_block(blk: &[u8; 4096]) -> Option<Self> {
        let mut r = VsfReader::new(blk);

        // --- Header ---
        if !r.magic() { return None; }
        let _ver = r.version()?;
        let _bver = r.backward_version()?;
        let _hlen = r.header_length()?;
        let eagle_time = r.eagle_time_qtimer()?;

        // Record hp position for verification
        let hp_pos_in_buf = r.pos;
        let hp_hash_ref = r.hash_p()?;
        let mut hp_hash = [0u8; 32];
        hp_hash.copy_from_slice(hp_hash_ref);

        let _count = r.field_count()?;
        if !r.close() { return None; }

        // --- Verify provenance hash ---
        // BLAKE3 of entire block with hp field zeroed
        let mut temp = *blk;
        for i in 0..32 { temp[hp_pos_in_buf + 4 + i] = 0; } // +4 = 'h' 'p' '3' 31
        let computed = blake3::hash(&temp);
        if computed.as_bytes() != &hp_hash {
            return None;
        }

        // --- Body: parse section [ ...fields... ] ---
        // Anonymous section (< 1MB from header, name in TOC only)
        if r.read_byte_raw()? != b'[' { return None; }

        // Parse fields: (d("name"):value)
        let mut generation: u64 = 0;
        let mut prev_hash = [0u8; 32];
        let mut resume_state = [0u8; 32];

        while r.peek_tag() == Some(b'(') {
            r.read_byte_raw(); // consume '('
            let fname = r.dict_key_str()?;
            if r.read_byte_raw()? != b':' { return None; } // field separator

            match fname {
                "generation" => { generation = r.uint()?; }
                "prev_hash" => {
                    let h = r.hash_p()?;
                    prev_hash.copy_from_slice(h);
                }
                "resume_state" => {
                    let h = r.hash_p()?;
                    resume_state.copy_from_slice(h);
                }
                _ => { r.skip_field(); } // unknown field — skip
            }

            if r.read_byte_raw()? != b')' { return None; } // field close
        }

        if r.read_byte_raw()? != b']' { return None; } // section close

        if generation == 0 { return None; } // 0 = invalid/empty

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
/// Parses header + section structure to find the generation field.
fn read_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> u64 {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 { return 0; }

    let data = ufs.data_buffer();

    // Quick parse: skip header to reach body
    let mut r = VsfReader::new(data);
    if !r.magic() { return 0; }
    if r.version().is_none() { return 0; }
    if r.backward_version().is_none() { return 0; }
    if r.header_length().is_none() { return 0; }
    if r.eagle_time_qtimer().is_none() { return 0; }
    if r.hash_p().is_none() { return 0; }
    if r.field_count().is_none() { return 0; }
    if !r.close() { return 0; }

    // Body: [(d("generation"):u(N))...]  (anonymous section, no d("ring"))
    if r.read_byte_raw() != Some(b'[') { return 0; }
    if r.read_byte_raw() != Some(b'(') { return 0; } // field open
    if r.dict_key_str().is_none() { return 0; } // skip field name "generation"
    if r.read_byte_raw() != Some(b':') { return 0; } // separator
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
