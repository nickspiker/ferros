//! Boot State Ring: a generation-ordered ring of VSF documents on UFS/SD.
//!
//! Each ring entry is a complete VSF document (RÅ< magic, EWE-encoded fields, Eagle creation time, provenance hash), built with the schema builders. Any standard VSF reader (`vsfinfo`, the `vsf` crate, anything that follows the spec) can parse them.
//!
//! The documents (`RingEntry`, `KernelRingEntry`) are portable: ferros-install builds the same bytes on the host that the seed and the kernel read on the device, from this one definition.
//! The UFS scan and write functions are aarch64 only.
//!
//! Binary search finds the highest valid generation in 2×log2(RING_SIZE) generation reads plus one full entry read.
//! The spine counts from generation 0 (RING.md): an empty slot is one that does not verify, never a generation number.
//! The stem (kernel ring) still counts from generation 1, its 0 meaning an empty slot, as the seed's scan reads it.
//!
//! See RING.md for the full specification.

extern crate alloc;

#[cfg(target_arch = "aarch64")]
use crate::ufs::UfsController;
#[cfg(target_arch = "aarch64")]
use ferros_layout::{KERNEL_RING_BASE, KERNEL_RING_DEPTH, KERNEL_RING_SIZE};
use ferros_layout::{VAULT_ROOT_RING_BASE, VAULT_ROOT_RING_SIZE};
use vsf::file_format::VsfHeader;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::types::{EtType, VsfType};
use vsf::vsf_builder::VsfBuilder;

/// Ring size: 65536 entries × 4KB = 256MB total. (Legacy alias: use `ferros_layout::VAULT_ROOT_RING_SIZE` for new code.)
pub const RING_SIZE: u32 = VAULT_ROOT_RING_SIZE;

/// Base block for the ring on UFS LUN 0 (4KB blocks). (Legacy alias: use `ferros_layout::VAULT_ROOT_RING_BASE` for new code.)
pub const RING_BASE_BLOCK: u32 = VAULT_ROOT_RING_BASE;

/// The spine entry's section name.
const RING_SECTION: &str = "ring";
/// The stem entry's section name.
const KERNEL_SECTION: &str = "kernel";

/// Convert a spine generation to its ring position: generation 0 is position 0, straight across (RING.md).
#[inline]
pub fn gen_to_pos(generation: u64) -> u32 {
    (generation % RING_SIZE as u64) as u32
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
    /// The document's creation time, Eagle oscillations (the header's `e6`), as the writer's caller gave it.
    pub eagle_time: i64,
    pub hp_hash: [u8; 32],
}

/// Build a complete VSF document into a fixed-size block. Returns `None` if the build fails or the document exceeds `BLK` bytes.
fn build_into_block<const BLK: usize>(builder: VsfBuilder) -> Option<[u8; BLK]> {
    let doc = builder.build().ok()?;
    if doc.len() > BLK {
        return None;
    }
    let mut blk = [0u8; BLK];
    blk[..doc.len()].copy_from_slice(&doc);
    Some(blk)
}

/// One complete document of one section: the section encoded by its schema, stamped with `now` (Eagle oscillations), provenance hash computed by the builder.
fn document_block(name: &str, section: SectionBuilder, now: i64) -> Option<[u8; 4096]> {
    let body = section.encode().ok()?;
    build_into_block::<4096>(VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed(name, body))
}

/// Verify the document at the start of a block and parse its section by `schema`, with the header for the creation time and provenance hash.
/// The document is the block's first `file_length` bytes, the rest zero padding: the provenance hash covers exactly those bytes, so that is the one span verified (`parse_document` checks it).
fn open_section(blk: &[u8], schema: SectionSchema) -> Option<(VsfHeader, SectionBuilder)> {
    let (header, _) = VsfHeader::decode(blk).ok()?;
    // WHY `get`: file_length is read from the block (disk input); one past the block would slice out of bounds and panic the seed.
    let doc = blk.get(..header.file_length)?;
    let section = SectionBuilder::parse_document(schema, doc, None).ok()?;
    Some((header, section))
}

/// The first value of a field.
fn first(section: &SectionBuilder, name: &str) -> Option<VsfType> {
    section.get_fields(name).first().and_then(|f| f.values.first()).cloned()
}

/// An unsigned field.
fn unsigned(section: &SectionBuilder, name: &str) -> Option<u64> {
    first(section, name)?.as_u64()
}

/// An unsigned field that names a block or a size: a value past u32 is no block on this device.
fn unsigned32(section: &SectionBuilder, name: &str) -> Option<u32> {
    // WHY try_from: the value is read from disk; `as u32` would wrap a corrupt value into a real block number.
    u32::try_from(unsigned(section, name)?).ok()
}

/// A 32-byte `hp` field.
fn hash32(section: &SectionBuilder, name: &str) -> Option<[u8; 32]> {
    match first(section, name)? {
        // WHY the length check: the schema matches `hp` of any length; one that is not 32 bytes is not a BLAKE3 hash.
        VsfType::hp(v) => v.as_slice().try_into().ok(),
        _ => None,
    }
}

/// The header's creation time, which every writer here stamps as `e6` oscillations.
fn creation_time(header: &VsfHeader) -> Option<i64> {
    match header.creation_time {
        Some(VsfType::e(EtType::e6(t))) => Some(t),
        _ => None,
    }
}

/// The header's 32-byte provenance hash.
fn provenance(header: &VsfHeader) -> Option<[u8; 32]> {
    match &header.provenance_hash {
        VsfType::hp(v) => v.as_slice().try_into().ok(),
        _ => None,
    }
}

impl RingEntry {
    /// The spine entry's section.
    pub fn schema() -> SectionSchema {
        SectionSchema::new(RING_SECTION)
            .field("generation", TypeConstraint::AnyUnsigned)
            .field("prev_hash", TypeConstraint::Blake3Provenance)
            .field("hamt_root_hash", TypeConstraint::Blake3Provenance)
            .field("hamt_root_lba", TypeConstraint::AnyUnsigned)
            .field("plow_position", TypeConstraint::AnyUnsigned)
    }

    /// Serialize to a 4KB VSF document stamped with `eagle_time`. Padding past the encoded length is zero. None if the document cannot be built (a bug: an entry is a few hundred bytes).
    pub fn to_block(&self) -> Option<[u8; 4096]> {
        let section = Self::schema()
            .build()
            .set("generation", VsfType::u(self.generation as usize, false))
            .ok()?
            .set("prev_hash", VsfType::hp(self.prev_hash.to_vec()))
            .ok()?
            .set("hamt_root_hash", VsfType::hp(self.hamt_root_hash.to_vec()))
            .ok()?
            .set("hamt_root_lba", VsfType::u(self.hamt_root_lba as usize, false))
            .ok()?
            .set("plow_position", VsfType::u(self.plow_position as usize, false))
            .ok()?;
        document_block(RING_SECTION, section, self.eagle_time)
    }

    /// Deserialize from a block (the document at its start). None if the provenance hash does not verify, there is no `ring` section, or a field is missing or not its type.
    pub fn from_block(blk: &[u8]) -> Option<Self> {
        let (header, section) = open_section(blk, Self::schema())?;
        Some(Self {
            generation: unsigned(&section, "generation")?,
            prev_hash: hash32(&section, "prev_hash")?,
            hamt_root_hash: hash32(&section, "hamt_root_hash")?,
            hamt_root_lba: unsigned32(&section, "hamt_root_lba")?,
            plow_position: unsigned32(&section, "plow_position")?,
            eagle_time: creation_time(&header)?,
            hp_hash: provenance(&header)?,
        })
    }

    /// Genesis entry: generation 0, previous hash all zeros (RING.md), no HAMT yet, the plow at the tract's start, made at `now` (Eagle oscillations, from the caller's clock).
    pub fn genesis(now: i64) -> Self {
        Self {
            generation: 0,
            prev_hash: [0u8; 32],
            hamt_root_hash: [0u8; 32],
            hamt_root_lba: 0,
            plow_position: ferros_layout::TRACT_BASE,
            eagle_time: now,
            hp_hash: [0u8; 32], // computed in to_block()
        }
    }

    /// Create next entry in the chain from this entry, made at `now` (Eagle oscillations, from the caller's clock). Caller should set `hamt_root_hash`, `hamt_root_lba`, `plow_position` before writing.
    pub fn next(&self, now: i64) -> Self {
        Self {
            generation: self.generation + 1,
            prev_hash: self.hp_hash,
            hamt_root_hash: self.hamt_root_hash,
            hamt_root_lba: self.hamt_root_lba,
            plow_position: self.plow_position,
            eagle_time: now,
            hp_hash: [0u8; 32], // computed in to_block()
        }
    }
}

/// Result of scanning the ring.
pub struct ScanResult {
    /// Highest valid generation found (0 for an empty ring; for the spine, generation 0 is a real entry, so `entry` tells the two apart).
    pub generation: u64,
    /// Ring position of the newest entry.
    pub position: u32,
    /// The newest entry (if found).
    pub entry: Option<RingEntry>,
    /// Number of block reads performed.
    pub reads: u32,
}

/// Binary search the ring for the highest valid generation. Exactly 2 × log2(RING_SIZE) generation reads + 1 full entry read. An empty slot reads as None, below every generation (None < Some(0)).
#[cfg(target_arch = "aarch64")]
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
#[cfg(target_arch = "aarch64")]
pub fn write_entry(ufs: &UfsController, entry: &RingEntry) -> bool {
    let pos = gen_to_pos(entry.generation);
    let lba = RING_BASE_BLOCK + pos;
    let Some(blk) = entry.to_block() else { return false };

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

/// Read just the `generation` field of a ring entry at a position. None if empty, corrupt, hp-mismatched, or missing the field.
#[cfg(target_arch = "aarch64")]
fn read_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> Option<u64> {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 {
        return None;
    }
    // Full parse + verify is the simplest correct path; the binary search runs once at boot, not in a hot loop.
    RingEntry::from_block(ufs.data_buffer()).map(|e| e.generation)
}

/// Read a full entry from the ring at the given position.
#[cfg(target_arch = "aarch64")]
fn read_entry(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> Option<RingEntry> {
    let lba = RING_BASE_BLOCK + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 {
        return None;
    }
    RingEntry::from_block(ufs.data_buffer())
}

// ===========================================================================
// Kernel Ring: the small ring scanned by the seed to find the current kernel
// ===========================================================================

/// Kernel ring entry: points the seed to the current kernel binary.
#[derive(Clone)]
pub struct KernelRingEntry {
    /// From 1; 0 is an empty slot to the seed's scan.
    pub generation: u64,
    pub kernel_lba: u32,
    pub kernel_size: u32,
    pub kernel_hash: [u8; 32],
    pub kernel_sig: [u8; 64],
    /// The document's creation time, Eagle oscillations (the header's `e6`), as the writer's caller gave it.
    pub eagle_time: i64,
    pub hp_hash: [u8; 32],
}

impl KernelRingEntry {
    /// An entry to write: the kernel at `kernel_lba`, `kernel_size` bytes, its BLAKE3 hash and Ed25519 signature, made at `now` (Eagle oscillations, from the caller's clock).
    pub fn new(generation: u64, kernel_lba: u32, kernel_size: u32, kernel_hash: [u8; 32], kernel_sig: [u8; 64], now: i64) -> Self {
        Self {
            generation,
            kernel_lba,
            kernel_size,
            kernel_hash,
            kernel_sig,
            eagle_time: now,
            hp_hash: [0u8; 32], // computed in to_block()
        }
    }

    /// The stem entry's section.
    pub fn schema() -> SectionSchema {
        SectionSchema::new(KERNEL_SECTION)
            .field("generation", TypeConstraint::AnyUnsigned)
            .field("kernel_lba", TypeConstraint::AnyUnsigned)
            .field("kernel_size", TypeConstraint::AnyUnsigned)
            .field("kernel_hash", TypeConstraint::Blake3Provenance)
            .field("kernel_sig", TypeConstraint::Ed25519Sig)
    }

    /// Serialize to a 4KB VSF document stamped with `eagle_time`. Padding past the encoded length is zero. None if the document cannot be built (a bug: an entry is a few hundred bytes).
    pub fn to_block(&self) -> Option<[u8; 4096]> {
        let section = Self::schema()
            .build()
            .set("generation", VsfType::u(self.generation as usize, false))
            .ok()?
            .set("kernel_lba", VsfType::u(self.kernel_lba as usize, false))
            .ok()?
            .set("kernel_size", VsfType::u(self.kernel_size as usize, false))
            .ok()?
            .set("kernel_hash", VsfType::hp(self.kernel_hash.to_vec()))
            .ok()?
            .set("kernel_sig", VsfType::ge(self.kernel_sig.to_vec()))
            .ok()?;
        document_block(KERNEL_SECTION, section, self.eagle_time)
    }

    /// Deserialize from a block (the document at its start). None if the provenance hash does not verify, there is no `kernel` section, a field is missing or not its type, or the generation is 0 (an empty slot).
    pub fn from_block(blk: &[u8]) -> Option<Self> {
        let (header, section) = open_section(blk, Self::schema())?;
        let kernel_sig = match first(&section, "kernel_sig")? {
            // WHY the length check: the schema matches `ge` of any length; one that is not 64 bytes is not an Ed25519 signature.
            VsfType::ge(v) => v.as_slice().try_into().ok()?,
            _ => return None,
        };
        let entry = KernelRingEntry {
            generation: unsigned(&section, "generation")?,
            kernel_lba: unsigned32(&section, "kernel_lba")?,
            kernel_size: unsigned32(&section, "kernel_size")?,
            kernel_hash: hash32(&section, "kernel_hash")?,
            kernel_sig,
            eagle_time: creation_time(&header)?,
            hp_hash: provenance(&header)?,
        };
        if entry.generation == 0 {
            return None;
        }
        Some(entry)
    }
}

/// Scan the kernel ring for the newest valid generation.
#[cfg(target_arch = "aarch64")]
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
        if let Some(ke) = KernelRingEntry::from_block(ufs.data_buffer()) {
            result.generation = ke.generation;
            result.position = lo;
            // entry field is RingEntry-typed; kernel-ring callers use scan_kernel_ring's result.generation/position + a separate read
            result.entry = None;
        }
    }

    result
}

/// Write a kernel ring entry. Returns true if write + verify passed.
#[cfg(target_arch = "aarch64")]
pub fn write_kernel_entry(ufs: &UfsController, entry: &KernelRingEntry) -> bool {
    let pos = ((entry.generation - 1) % KERNEL_RING_SIZE as u64) as u32;
    let lba = KERNEL_RING_BASE + pos;
    let Some(blk) = entry.to_block() else { return false };

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
#[cfg(target_arch = "aarch64")]
fn read_kernel_generation(ufs: &UfsController, pos: u32, result: &mut ScanResult) -> u64 {
    let lba = KERNEL_RING_BASE + pos;
    result.reads += 1;

    let ocs = ufs.read_block(lba);
    if ocs != 0 {
        return 0;
    }
    KernelRingEntry::from_block(ufs.data_buffer()).map(|e| e.generation).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 0x0123_4567_89AB_CDEF;

    #[test]
    fn spine_genesis_round_trips() {
        let blk = RingEntry::genesis(NOW).to_block().expect("genesis builds");
        let e = RingEntry::from_block(&blk).expect("genesis parses");
        assert_eq!(e.generation, 0);
        assert_eq!(e.prev_hash, [0u8; 32]);
        assert_eq!(e.hamt_root_hash, [0u8; 32]);
        assert_eq!(e.hamt_root_lba, 0);
        assert_eq!(e.plow_position, ferros_layout::TRACT_BASE);
        assert_eq!(e.eagle_time, NOW);
        assert_ne!(e.hp_hash, [0u8; 32]);
        assert_eq!(gen_to_pos(e.generation), 0);
    }

    #[test]
    fn spine_next_chains_and_round_trips() {
        let g = RingEntry::from_block(&RingEntry::genesis(NOW).to_block().unwrap()).unwrap();
        let mut n = g.next(NOW + 1);
        n.hamt_root_hash = [7u8; 32];
        n.hamt_root_lba = 0x1_2345;
        n.plow_position = 0xC_0400;
        let e = RingEntry::from_block(&n.to_block().unwrap()).unwrap();
        assert_eq!(e.generation, 1);
        assert_eq!(e.prev_hash, g.hp_hash);
        assert_eq!(e.hamt_root_hash, [7u8; 32]);
        assert_eq!(e.hamt_root_lba, 0x1_2345);
        assert_eq!(e.plow_position, 0xC_0400);
        assert_eq!(e.eagle_time, NOW + 1);
    }

    #[test]
    fn a_flipped_byte_does_not_verify() {
        let blk = RingEntry::genesis(NOW).to_block().unwrap();
        let (header, _) = VsfHeader::decode(&blk).unwrap();
        for i in 0..header.file_length {
            let mut bad = blk;
            bad[i] ^= 1;
            assert!(RingEntry::from_block(&bad).is_none(), "byte {} flipped still parsed", i);
        }
        assert!(RingEntry::from_block(&[0u8; 4096]).is_none());
    }

    #[test]
    fn kernel_entry_round_trips() {
        let mut sig = [0u8; 64];
        for (i, b) in sig.iter_mut().enumerate() {
            *b = i as u8;
        }
        let k = KernelRingEntry::new(1, ferros_layout::KERNEL_A_BASE, 0x2F_1234, [0xA5; 32], sig, NOW);
        let e = KernelRingEntry::from_block(&k.to_block().unwrap()).expect("stem entry parses");
        assert_eq!(e.generation, 1);
        assert_eq!(e.kernel_lba, ferros_layout::KERNEL_A_BASE);
        assert_eq!(e.kernel_size, 0x2F_1234);
        assert_eq!(e.kernel_hash, [0xA5; 32]);
        assert_eq!(e.kernel_sig, sig);
        assert_eq!(e.eagle_time, NOW);
        assert_ne!(e.hp_hash, [0u8; 32]);
    }

    #[test]
    fn kernel_generation_zero_is_empty() {
        let k = KernelRingEntry::new(0, 0, 0, [0; 32], [0; 64], NOW);
        assert!(KernelRingEntry::from_block(&k.to_block().unwrap()).is_none());
    }
}
