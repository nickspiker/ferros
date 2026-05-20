//! Boot State Ring — generation-ordered ring of VSF documents on UFS/SD.
//!
//! Each ring entry is a complete VSF document (RÅ< magic, EWE-encoded fields, provenance hash).
//! Any standard VSF reader (`vsfinfo`, the `vsf` crate, anything that follows the spec) can parse them.
//!
//! Binary search finds the highest valid generation in 2×log2(RING_SIZE) generation reads plus one full entry read.
//! Generation starts at 1 (0 = empty slot).
//!
//! See RING.md for the full specification.

extern crate alloc;

use crate::qtimer::read_qtimer;
use crate::ufs::UfsController;
use alloc::string::ToString;
use alloc::vec;
use ferros_layout::{
    BLOCK_SIZE, KERNEL_RING_BASE, KERNEL_RING_DEPTH, KERNEL_RING_SIZE, VAULT_ROOT_RING_BASE,
    VAULT_ROOT_RING_SIZE,
};
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::types::{EtType, VsfType};
use vsf::verification::is_original;
use vsf::vsf_builder::VsfBuilder;

/// Ring size: 65536 entries × 4KB = 256MB total.
/// (Legacy alias — use `ferros_layout::VAULT_ROOT_RING_SIZE` for new code.)
pub const RING_SIZE: u32 = VAULT_ROOT_RING_SIZE;

/// Base block for the ring on UFS LUN 0 (4KB blocks).
/// (Legacy alias — use `ferros_layout::VAULT_ROOT_RING_BASE` for new code.)
pub const RING_BASE_BLOCK: u32 = VAULT_ROOT_RING_BASE;

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
    /// HAMT root block BLAKE3 hash (zero if no HAMT yet).
    pub hamt_root_hash: [u8; 32],
    /// HAMT root block LBA in the tract (0 if no HAMT yet).
    pub hamt_root_lba: u32,
    /// Plow position: next free block in the tract.
    pub plow_position: u32,
    pub eagle_time: u64,
    pub hp_hash: [u8; 32],
}

/// Build a complete VSF document into a fixed-size block.
/// Returns `None` if the encoded doc exceeds `BLK` bytes.
fn build_into_block<const BLK: usize>(builder: VsfBuilder) -> Option<[u8; BLK]> {
    let doc = builder.build().ok()?;
    if doc.len() > BLK {
        return None;
    }
    let mut blk = [0u8; BLK];
    blk[..doc.len()].copy_from_slice(&doc);
    Some(blk)
}

/// Extract eu6 oscillation count from a header's optional `creation_time` field.
/// Accepts any signed/unsigned integer EtType form and converts to `u64`.
/// Returns 0 when the header omits creation_time (clockless device) or when the variant isn't a known integer Eagle Time.
fn et_to_u64(et: &Option<VsfType>) -> u64 {
    match et {
        Some(VsfType::e(EtType::e5(v))) => *v as u64,
        Some(VsfType::e(EtType::e6(v))) => *v as u64,
        Some(VsfType::e(EtType::e7(v))) => *v as u64,
        _ => 0,
    }
}

/// Extract 32-byte hp from a header's `provenance_hash` field.
fn hp_bytes(hp: &VsfType) -> Option<[u8; 32]> {
    match hp {
        VsfType::hp(v) if v.len() == 32 => {
            let mut out = [0u8; 32];
            out.copy_from_slice(v);
            Some(out)
        }
        _ => None,
    }
}

/// Parse a 4KB block as a complete VSF doc and return the header + bytes-of-doc length.
/// hp is verified; padding past the file_length is ignored.
fn open_doc(blk: &[u8]) -> Option<(VsfHeader, usize)> {
    let (header, _) = VsfHeader::decode(blk).ok()?;
    let file_length = header.file_length;
    if file_length == 0 || file_length > blk.len() {
        return None;
    }
    is_original(&blk[..file_length]).ok()?;
    Some((header, file_length))
}

/// Locate a section by name in a parsed header and parse its body into a `VsfSection`.
fn parse_named_section(blk: &[u8], header: &VsfHeader, name: &str) -> Option<VsfSection> {
    let field = header.fields.iter().find(|f| f.name == name)?;
    let off = field.offset_bytes;
    let size = field.size_bytes;
    if off.checked_add(size)? > blk.len() {
        return None;
    }
    let mut ptr = off;
    VsfSection::parse(&blk[..off + size], &mut ptr).ok()
}

impl RingEntry {
    /// Serialize to a 4KB VSF document. Padding past the encoded length is zero.
    pub fn to_block(&self) -> [u8; 4096] {
        let builder = VsfBuilder::new()
            .version(7, 7)
            .creation_time_oscillations(self.eagle_time as i64)
            .provenance_only()
            .add_section(
                "ring",
                vec![
                    (
                        "generation".to_string(),
                        VsfType::u(self.generation as usize, false),
                    ),
                    ("prev_hash".to_string(), VsfType::hp(self.prev_hash.to_vec())),
                    (
                        "hamt_root_hash".to_string(),
                        VsfType::hp(self.hamt_root_hash.to_vec()),
                    ),
                    (
                        "hamt_root_lba".to_string(),
                        VsfType::u(self.hamt_root_lba as usize, false),
                    ),
                    (
                        "plow_position".to_string(),
                        VsfType::u(self.plow_position as usize, false),
                    ),
                ],
            );
        build_into_block::<4096>(builder).unwrap_or([0u8; 4096])
    }

    /// Deserialize from a 4KB block. Returns `None` if hp verify fails, no `ring` section, or fields missing.
    pub fn from_block(blk: &[u8; 4096]) -> Option<Self> {
        let (header, _) = open_doc(blk)?;
        let section = parse_named_section(blk, &header, "ring")?;

        let mut generation: u64 = 0;
        let mut prev_hash = [0u8; 32];
        let mut hamt_root_hash = [0u8; 32];
        let mut hamt_root_lba: u32 = 0;
        let mut plow_position: u32 = 0;

        for f in &section.fields {
            match f.name.as_str() {
                "generation" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        generation = *v as u64;
                    }
                }
                "prev_hash" => {
                    if let Some(VsfType::hp(v)) = f.values.first() {
                        if v.len() == 32 {
                            prev_hash.copy_from_slice(v);
                        }
                    }
                }
                "hamt_root_hash" | "resume_state" => {
                    if let Some(VsfType::hp(v)) = f.values.first() {
                        if v.len() == 32 {
                            hamt_root_hash.copy_from_slice(v);
                        }
                    }
                }
                "hamt_root_lba" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        hamt_root_lba = *v as u32;
                    }
                }
                "plow_position" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        plow_position = *v as u32;
                    }
                }
                _ => {} // unknown field — ignore
            }
        }

        if generation == 0 {
            return None;
        }

        let eagle_time = et_to_u64(&header.creation_time);
        let hp_hash = hp_bytes(&header.provenance_hash)?;

        Some(Self {
            generation,
            prev_hash,
            hamt_root_hash,
            hamt_root_lba,
            plow_position,
            eagle_time,
            hp_hash,
        })
    }

    /// Genesis entry — generation 1, all hashes zeroed, current QTIMER.
    pub fn genesis() -> Self {
        Self {
            generation: 1,
            prev_hash: [0u8; 32],
            hamt_root_hash: [0u8; 32],
            hamt_root_lba: 0,
            plow_position: ferros_layout::TRACT_BASE,
            eagle_time: read_qtimer(),
            hp_hash: [0u8; 32], // computed in to_block()
        }
    }

    /// Create next entry in the chain from this entry.
    /// Caller should set `hamt_root_hash`, `hamt_root_lba`, `plow_position` before writing.
    pub fn next(&self) -> Self {
        Self {
            generation: self.generation + 1,
            prev_hash: self.hp_hash,
            hamt_root_hash: self.hamt_root_hash,
            hamt_root_lba: self.hamt_root_lba,
            plow_position: self.plow_position,
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
    if write_ocs != 0 {
        return false;
    }

    // Read back and verify
    let read_ocs = ufs.read_block(lba);
    if read_ocs != 0 {
        return false;
    }

    let readback = ufs.data_buffer();
    readback == &blk
}

/// Read just the `generation` field of a ring entry at a position.
/// Returns 0 if empty, corrupt, hp-mismatched, or missing the field.
fn read_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> u64 {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 {
        return 0;
    }
    let data = ufs.data_buffer();

    // Full parse + verify is the simplest correct path; the binary search runs once at boot, not in a hot loop.
    let blk_ref: &[u8; BLOCK_SIZE] = match data[..BLOCK_SIZE].try_into() {
        Ok(b) => b,
        Err(_) => return 0,
    };
    RingEntry::from_block(blk_ref).map(|e| e.generation).unwrap_or(0)
}

/// Read a full entry from the ring at the given position.
fn read_entry(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> Option<RingEntry> {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 {
        return None;
    }
    let data = ufs.data_buffer();
    let blk_ref: &[u8; BLOCK_SIZE] = data[..BLOCK_SIZE].try_into().ok()?;
    RingEntry::from_block(blk_ref)
}

// ===========================================================================
// Kernel Ring — small ring scanned by the seed to find the current kernel
// ===========================================================================

/// Kernel ring entry — points the seed to the current kernel binary.
#[derive(Clone)]
pub struct KernelRingEntry {
    pub generation: u64,
    pub kernel_lba: u32,
    pub kernel_size: u32,
    pub kernel_hash: [u8; 32],
    pub kernel_sig: [u8; 64],
    pub eagle_time: u64,
    pub hp_hash: [u8; 32],
}

impl KernelRingEntry {
    /// Serialize to a 4KB VSF document.
    pub fn to_block(&self) -> [u8; BLOCK_SIZE] {
        let builder = VsfBuilder::new()
            .version(7, 7)
            .creation_time_oscillations(self.eagle_time as i64)
            .provenance_only()
            .add_section(
                "kernel",
                vec![
                    (
                        "generation".to_string(),
                        VsfType::u(self.generation as usize, false),
                    ),
                    (
                        "kernel_lba".to_string(),
                        VsfType::u(self.kernel_lba as usize, false),
                    ),
                    (
                        "kernel_size".to_string(),
                        VsfType::u(self.kernel_size as usize, false),
                    ),
                    (
                        "kernel_hash".to_string(),
                        VsfType::hp(self.kernel_hash.to_vec()),
                    ),
                    (
                        "kernel_sig".to_string(),
                        VsfType::ge(self.kernel_sig.to_vec()),
                    ),
                ],
            );
        build_into_block::<BLOCK_SIZE>(builder).unwrap_or([0u8; BLOCK_SIZE])
    }

    /// Deserialize from a 4KB block. Returns `None` if not a valid kernel ring entry.
    pub fn from_block(blk: &[u8]) -> Option<Self> {
        if blk.len() < BLOCK_SIZE {
            return None;
        }
        let blk_ref: &[u8; BLOCK_SIZE] = blk[..BLOCK_SIZE].try_into().ok()?;
        let (header, _) = open_doc(blk_ref)?;
        let section = parse_named_section(blk_ref, &header, "kernel")?;

        let mut entry = KernelRingEntry {
            generation: 0,
            kernel_lba: 0,
            kernel_size: 0,
            kernel_hash: [0u8; 32],
            kernel_sig: [0u8; 64],
            eagle_time: et_to_u64(&header.creation_time),
            hp_hash: hp_bytes(&header.provenance_hash)?,
        };

        for f in &section.fields {
            match f.name.as_str() {
                "generation" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        entry.generation = *v as u64;
                    }
                }
                "kernel_lba" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        entry.kernel_lba = *v as u32;
                    }
                }
                "kernel_size" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        entry.kernel_size = *v as u32;
                    }
                }
                "kernel_hash" => {
                    if let Some(VsfType::hp(v)) = f.values.first() {
                        if v.len() == 32 {
                            entry.kernel_hash.copy_from_slice(v);
                        }
                    }
                }
                "kernel_sig" => {
                    if let Some(VsfType::ge(v)) = f.values.first() {
                        if v.len() == 64 {
                            entry.kernel_sig.copy_from_slice(v);
                        }
                    }
                }
                _ => {}
            }
        }

        if entry.generation == 0 {
            return None;
        }
        Some(entry)
    }
}

/// Scan the kernel ring for the newest valid generation.
pub fn scan_kernel_ring(ufs: &UfsController) -> ScanResult {
    let mut result = ScanResult {
        generation: 0,
        position: 0,
        entry: None,
        reads: 0,
    };

    let mut lo: u32 = 0;
    let mut size: u32 = KERNEL_RING_SIZE;

    for _ in 0..KERNEL_RING_DEPTH {
        let half = size >> 1;
        let mid = (lo + half) % KERNEL_RING_SIZE;
        let gen_lo = read_kernel_generation(ufs, lo, &mut result);
        let gen_mid = read_kernel_generation(ufs, mid, &mut result);

        if gen_mid > gen_lo {
            lo = mid;
        }
        size = half;
    }

    // Read full entry at converged position
    let lba = KERNEL_RING_BASE + lo;
    result.reads += 1;
    let ocs = ufs.read_block(lba);
    if ocs == 0 {
        let data = ufs.data_buffer();
        let mut blk = [0u8; BLOCK_SIZE];
        blk.copy_from_slice(&data[..BLOCK_SIZE]);
        if let Some(ke) = KernelRingEntry::from_block(&blk) {
            result.generation = ke.generation;
            result.position = lo;
            // entry field is RingEntry-typed; kernel-ring callers use scan_kernel_ring's result.generation/position + a separate read
            result.entry = None;
        }
    }

    result
}

/// Write a kernel ring entry. Returns true if write + verify passed.
pub fn write_kernel_entry(ufs: &UfsController, entry: &KernelRingEntry) -> bool {
    let pos = ((entry.generation - 1) % KERNEL_RING_SIZE as u64) as u32;
    let lba = KERNEL_RING_BASE + pos;
    let blk = entry.to_block();

    let buf = ufs.data_buffer_mut();
    buf.copy_from_slice(&blk);
    let write_ocs = ufs.write_block(lba);
    if write_ocs != 0 {
        return false;
    }

    // Read back and verify
    let read_ocs = ufs.read_block(lba);
    if read_ocs != 0 {
        return false;
    }

    let readback = ufs.data_buffer();
    readback == &blk
}

/// Read just the generation field from a kernel ring position.
fn read_kernel_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> u64 {
    let lba = KERNEL_RING_BASE + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 {
        return 0;
    }
    let data = ufs.data_buffer();
    KernelRingEntry::from_block(data).map(|e| e.generation).unwrap_or(0)
}
