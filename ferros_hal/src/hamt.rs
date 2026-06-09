//! HAMT — Hash Array Mapped Trie for vault object lookup.
//!
//! 32-way branching, 5 bits per hash level, COW by construction. Every node is one 4KB VSF document in the tract.
//!
//! See HAMT.md for the full specification.

extern crate alloc;

use alloc::vec::Vec;
use vsf::file_format::{VsfHeader, VsfSection};
use vsf::types::VsfType;
use vsf::vsf_builder::VsfBuilder;

/// VSF magic bytes (`RÅ<` in UTF-8 = 0x52 0xC3 0x85 0x3C).
pub const VSF_MAGIC: [u8; 4] = [0x52, 0xC3, 0x85, 0x3C];

/// Bits consumed per trie level.
const BITS_PER_LEVEL: u32 = 5;

/// Children per node (2^5 = 32).
const BRANCHING: u32 = 1 << BITS_PER_LEVEL;

/// Maximum trie depth (256-bit hash / 5 bits per level).
const MAX_DEPTH: u32 = 51;

/// Block size (4KB).
const BLOCK_SIZE: usize = 4096;

/// Reference to a block in the tract: (BLAKE3 hash, LBA).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRef {
    pub hash: [u8; 32],
    pub lba: u32,
}

/// Extract the 5-bit index at a given trie level from a 256-bit hash.
#[inline]
fn hash_index(key: &[u8; 32], level: u32) -> u32 {
    let bit_offset = level * BITS_PER_LEVEL;
    let byte_idx = (bit_offset / 8) as usize;
    let bit_shift = bit_offset % 8;

    if byte_idx >= 32 {
        return 0;
    }

    // Read up to 2 bytes to handle the 5-bit field crossing a byte boundary.
    let wide = if byte_idx + 1 < 32 {
        (key[byte_idx] as u16) | ((key[byte_idx + 1] as u16) << 8)
    } else {
        key[byte_idx] as u16
    };

    ((wide >> bit_shift) & 0x1F) as u32
}

/// Count of set bits below position `bit` in a u32 bitmap. This gives the index into the sparse child array.
#[inline]
fn sparse_index(bitmap: u32, bit: u32) -> usize {
    (bitmap & ((1u32 << bit) - 1)).count_ones() as usize
}

// ---------------------------------------------------------------------------
// VSF helpers (shared between internal nodes and leaves)
// ---------------------------------------------------------------------------

/// Build a complete VSF document into a fixed-size block. Returns `None` if the encoded doc exceeds `BLK` bytes.
fn build_into_block<const BLK: usize>(builder: VsfBuilder) -> Option<[u8; BLK]> {
    let doc = builder.build().ok()?;
    if doc.len() > BLK {
        return None;
    }
    let mut blk = [0u8; BLK];
    blk[..doc.len()].copy_from_slice(&doc);
    Some(blk)
}

/// Open a 4KB block as a VSF doc and return its parsed header. Returns `None` if the magic is missing or the header doesn't parse.
fn open_header(blk: &[u8]) -> Option<VsfHeader> {
    let (header, _) = VsfHeader::decode(blk).ok()?;
    Some(header)
}

/// Locate a named section in a parsed header and parse its body.
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

/// Find the position of the hp hash bytes in a VSF document. Scans for `'h' 'p' '3' 0x1F` (hp + EWE-encoded length 31) after the magic. Both the old vsf_mini encoder and the new vsf crate emit hp as len-1 in EWE form, so this byte pattern remains stable.
pub fn find_hp_position(blk: &[u8]) -> Option<usize> {
    for i in 4..blk.len().saturating_sub(36) {
        if blk[i] == b'h' && blk[i + 1] == b'p'
            && blk[i + 2] == b'3' && blk[i + 3] == 0x1F
        {
            return Some(i + 4);
        }
    }
    None
}

/// Check if a block has VSF magic (first 4 bytes = RÅ<).
pub fn has_vsf_magic(blk: &[u8]) -> bool {
    blk.len() >= 4 && blk[..4] == VSF_MAGIC
}

// ---------------------------------------------------------------------------
// Block I/O trait — the kernel implements this with UFS + plow
// ---------------------------------------------------------------------------

/// Trait for reading and writing 4KB blocks.
///
/// The kernel provides this using UFS (+ SD mirror). The HAMT logic is pure — it doesn't know about UFS, SD, or the plow.
pub trait BlockIO {
    /// Read a 4KB block at `lba`. Returns None on I/O error.
    fn read_block(&self, lba: u32) -> Option<[u8; BLOCK_SIZE]>;

    /// Write a 4KB block at the current plow position. Returns the LBA where it was written, or None on failure. The implementation handles write-verify and mirror protocol.
    fn write_block(&mut self, data: &[u8; BLOCK_SIZE]) -> Option<u32>;
}

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// Parsed HAMT internal node.
pub struct InternalNode {
    /// 32-bit presence bitmap: bit N set → child N exists.
    pub bitmap: u32,
    /// Child hashes (parallel with lbas), length = popcount(bitmap).
    pub child_hashes: Vec<[u8; 32]>,
    /// Child LBAs (parallel with child_hashes).
    pub child_lbas: Vec<u32>,
}

/// What we found at a leaf position.
pub enum LeafData<'a> {
    /// Lone: inline content in the leaf.
    Lone {
        provenance: [u8; 32],
        body_hash: [u8; 32],
        content: &'a [u8],
    },
    /// Direct: furrow LBAs stored in the leaf.
    Direct {
        provenance: [u8; 32],
        body_hash: [u8; 32],
        size: u64,
        furrow_lbas: Vec<u32>,
    },
}

/// Result of a lookup: what the HAMT found.
pub enum LookupResult<'a> {
    /// Object found with inline content.
    Found(LeafData<'a>),
    /// Key not present in the trie.
    NotFound,
    /// Integrity failure (BLAKE3 mismatch). Caller should try previous generation.
    Corrupt,
}

// ---------------------------------------------------------------------------
// VSF serialization — internal nodes
// ---------------------------------------------------------------------------

impl InternalNode {
    /// Number of children present.
    pub fn child_count(&self) -> usize {
        self.bitmap.count_ones() as usize
    }

    /// Serialize to a 4KB VSF document.
    ///
    /// Section `hamt.node` with three fields:
    /// - `bitmap`: single u (32-bit presence mask)
    /// - `child_hashes`: multi-valued, popcount(bitmap) × hp(32 bytes)
    /// - `child_lbas`: multi-valued, popcount(bitmap) × u
    pub fn to_block(&self) -> [u8; BLOCK_SIZE] {
        let qtimer = crate::qtimer::read_qtimer();

        let mut section = VsfSection::new("hamt.node");
        section.add_field("bitmap", VsfType::u(self.bitmap as usize, false));

        let hashes: Vec<VsfType> = self
            .child_hashes
            .iter()
            .map(|h| VsfType::hp(h.to_vec()))
            .collect();
        section.add_field_multi("child_hashes", hashes);

        let lbas: Vec<VsfType> = self
            .child_lbas
            .iter()
            .map(|lba| VsfType::u(*lba as usize, false))
            .collect();
        section.add_field_multi("child_lbas", lbas);

        let builder = VsfBuilder::new()
            .version(7, 7)
            .creation_time_oscillations(qtimer as i64)
            .provenance_only()
            .add_section_direct(section);

        build_into_block::<BLOCK_SIZE>(builder).unwrap_or([0u8; BLOCK_SIZE])
    }

    /// Parse from a 4KB block. Returns None if not a valid HAMT node. `expected_hash` is the kernel-side block hash (BLAKE3 of full 4KB with hp zeroed); it is verified before parsing.
    pub fn from_block(blk: &[u8; BLOCK_SIZE], expected_hash: &[u8; 32]) -> Option<Self> {
        // Verify kernel block hash: zero hp in a copy, hash full 4KB, compare.
        let mut verify_buf = *blk;
        let hp_pos = find_hp_position(&verify_buf)?;
        verify_buf[hp_pos..hp_pos + 32].fill(0);
        let computed = blake3::hash(&verify_buf);
        if computed.as_bytes() != expected_hash {
            return None;
        }

        let header = open_header(blk)?;
        let section = parse_named_section(blk, &header, "hamt.node")?;

        let mut bitmap: u32 = 0;
        let mut child_hashes: Vec<[u8; 32]> = Vec::new();
        let mut child_lbas: Vec<u32> = Vec::new();

        for f in &section.fields {
            match f.name.as_str() {
                "bitmap" => {
                    if let Some(VsfType::u(v, _)) = f.values.first() {
                        bitmap = *v as u32;
                    }
                }
                "child_hashes" => {
                    for v in &f.values {
                        if let VsfType::hp(bytes) = v {
                            if bytes.len() == 32 {
                                let mut arr = [0u8; 32];
                                arr.copy_from_slice(bytes);
                                child_hashes.push(arr);
                            }
                        }
                    }
                }
                "child_lbas" => {
                    for v in &f.values {
                        if let VsfType::u(n, _) = v {
                            child_lbas.push(*n as u32);
                        }
                    }
                }
                _ => {}
            }
        }

        let expected = bitmap.count_ones() as usize;
        if child_hashes.len() != expected || child_lbas.len() != expected {
            return None;
        }

        Some(InternalNode {
            bitmap,
            child_hashes,
            child_lbas,
        })
    }

    /// Create an empty node (no children).
    pub fn empty() -> Self {
        InternalNode {
            bitmap: 0,
            child_hashes: Vec::new(),
            child_lbas: Vec::new(),
        }
    }

    /// Get child reference at bitmap position `bit`, if present.
    pub fn get_child(&self, bit: u32) -> Option<BlockRef> {
        if self.bitmap & (1 << bit) == 0 {
            return None;
        }
        let idx = sparse_index(self.bitmap, bit);
        Some(BlockRef {
            hash: self.child_hashes[idx],
            lba: self.child_lbas[idx],
        })
    }

    /// Return a new node with child at `bit` set/updated.
    pub fn with_child(&self, bit: u32, child: BlockRef) -> Self {
        let mut new = self.clone_node();
        if new.bitmap & (1 << bit) != 0 {
            // Update existing
            let idx = sparse_index(new.bitmap, bit);
            new.child_hashes[idx] = child.hash;
            new.child_lbas[idx] = child.lba;
        } else {
            // Insert new
            let idx = sparse_index(new.bitmap, bit);
            new.bitmap |= 1 << bit;
            new.child_hashes.insert(idx, child.hash);
            new.child_lbas.insert(idx, child.lba);
        }
        new
    }

    /// Return a new node with child at `bit` removed.
    pub fn without_child(&self, bit: u32) -> Self {
        let mut new = self.clone_node();
        if new.bitmap & (1 << bit) != 0 {
            let idx = sparse_index(new.bitmap, bit);
            new.bitmap &= !(1 << bit);
            new.child_hashes.remove(idx);
            new.child_lbas.remove(idx);
        }
        new
    }

    fn clone_node(&self) -> Self {
        InternalNode {
            bitmap: self.bitmap,
            child_hashes: self.child_hashes.clone(),
            child_lbas: self.child_lbas.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Lone leaf serialization
// ---------------------------------------------------------------------------

/// Serialize a lone leaf (inline object) to a 4KB block.
///
/// Section `vault.lone` with three fields:
/// - `provenance_key`: hp (the trie key — content-derived BLAKE3 of the original object)
/// - `body_hash`: hp (BLAKE3 of the inline content)
/// - `content`: v(b'b', bytes) — raw inline bytes
///
/// The vsf header's own hp field is the file integrity hash (auto-computed by the builder); it is separate from the `provenance_key` which is the HAMT lookup key.
pub fn lone_leaf_to_block(provenance: &[u8; 32], content: &[u8]) -> Option<[u8; BLOCK_SIZE]> {
    if content.len() > BLOCK_SIZE - 256 {
        // Conservative budget — header + field framing eats ~200B; leave headroom.
        return None;
    }

    let qtimer = crate::qtimer::read_qtimer();
    let body_hash = blake3::hash(content);

    let mut section = VsfSection::new("vault.lone");
    section.add_field("provenance_key", VsfType::hp(provenance.to_vec()));
    section.add_field("body_hash", VsfType::hp(body_hash.as_bytes().to_vec()));
    section.add_field("content", VsfType::v(b'b', content.to_vec()));

    let builder = VsfBuilder::new()
        .version(7, 7)
        .creation_time_oscillations(qtimer as i64)
        .provenance_only()
        .add_section_direct(section);

    build_into_block::<BLOCK_SIZE>(builder)
}

/// Helper: extract the 32-byte provenance key from a parsed leaf section.
fn section_provenance_key(section: &VsfSection) -> Option<[u8; 32]> {
    let field = section.fields.iter().find(|f| f.name == "provenance_key")?;
    match field.values.first()? {
        VsfType::hp(bytes) if bytes.len() == 32 => {
            let mut out = [0u8; 32];
            out.copy_from_slice(bytes);
            Some(out)
        }
        _ => None,
    }
}

/// Parse a lone leaf. Returns (provenance_key, body_hash, content_slice) if valid and the embedded body hash matches BLAKE3(content).
pub fn parse_lone_leaf(blk: &[u8; BLOCK_SIZE]) -> Option<([u8; 32], [u8; 32], Vec<u8>)> {
    let header = open_header(blk)?;
    let section = parse_named_section(blk, &header, "vault.lone")?;

    let mut provenance = [0u8; 32];
    let mut body_hash = [0u8; 32];
    let mut content: Option<Vec<u8>> = None;

    for f in &section.fields {
        match f.name.as_str() {
            "provenance_key" => {
                if let Some(VsfType::hp(v)) = f.values.first() {
                    if v.len() == 32 {
                        provenance.copy_from_slice(v);
                    }
                }
            }
            "body_hash" => {
                if let Some(VsfType::hp(v)) = f.values.first() {
                    if v.len() == 32 {
                        body_hash.copy_from_slice(v);
                    }
                }
            }
            "content" => {
                if let Some(VsfType::v(_, bytes)) = f.values.first() {
                    content = Some(bytes.clone());
                }
            }
            _ => {}
        }
    }

    let content = content?;
    let computed = blake3::hash(&content);
    if computed.as_bytes() != &body_hash {
        return None;
    }
    Some((provenance, body_hash, content))
}

/// Read the provenance key from a leaf block without verifying content. Used by `lookup` / `insert` / `remove` to compare against a query key.
fn leaf_provenance(blk: &[u8; BLOCK_SIZE]) -> Option<[u8; 32]> {
    let header = open_header(blk)?;

    // Try each known leaf section type. Only one will match.
    for name in ["vault.lone", "vault.direct", "vault.chained"].iter() {
        if let Some(section) = parse_named_section(blk, &header, name) {
            if let Some(key) = section_provenance_key(&section) {
                return Some(key);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Block kind identification
// ---------------------------------------------------------------------------

/// Identify what kind of node a block contains.
pub enum NodeKind {
    Internal,
    Lone,
    Direct,
    Chained,
    Extent,
    Unknown,
}

/// Peek at a block's first section name to determine its kind.
pub fn identify_block(blk: &[u8; BLOCK_SIZE]) -> NodeKind {
    let header = match open_header(blk) {
        Some(h) => h,
        None => return NodeKind::Unknown,
    };
    let first = match header.fields.first() {
        Some(f) => f.name.as_str(),
        None => return NodeKind::Unknown,
    };
    match first {
        "hamt.node" => NodeKind::Internal,
        "vault.lone" => NodeKind::Lone,
        "vault.direct" => NodeKind::Direct,
        "vault.chained" => NodeKind::Chained,
        "vault.extent" => NodeKind::Extent,
        _ => NodeKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Write helper
// ---------------------------------------------------------------------------

/// Serialize a node, write it via BlockIO, return its BlockRef. The hash is the kernel block hash (full 4KB BLAKE3 with hp zeroed).
fn write_node(io: &mut impl BlockIO, node: &InternalNode) -> Option<BlockRef> {
    let blk = node.to_block();
    let hash = block_hash(&blk)?;
    let lba = io.write_block(&blk)?;
    Some(BlockRef { hash, lba })
}

/// Compute the kernel block hash (BLAKE3 of full 4KB block with hp zeroed). This is the hash stored in BlockRef — distinct from the vsf-internal hp field, which only covers the file_length bytes.
pub fn block_hash(blk: &[u8; BLOCK_SIZE]) -> Option<[u8; 32]> {
    let hp_pos = find_hp_position(blk)?;
    let mut tmp = *blk;
    tmp[hp_pos..hp_pos + 32].fill(0);
    Some(*blake3::hash(&tmp).as_bytes())
}

// ---------------------------------------------------------------------------
// HAMT operations
// ---------------------------------------------------------------------------

/// Lookup a provenance hash in the HAMT.
///
/// Returns the block reference for the object, or None if not found. On BLAKE3 mismatch at any node, returns None (caller should try previous spine generation).
pub fn lookup(
    io: &impl BlockIO,
    root: &BlockRef,
    key: &[u8; 32],
) -> Option<BlockRef> {
    let mut current = *root;

    for level in 0..MAX_DEPTH {
        let blk = io.read_block(current.lba)?;

        // Verify kernel block hash before trusting the node body.
        let computed = block_hash(&blk)?;
        if &computed != &current.hash {
            return None; // Corrupt
        }

        match identify_block(&blk) {
            NodeKind::Internal => {
                let node = InternalNode::from_block(&blk, &current.hash)?;
                let bit = hash_index(key, level);
                current = node.get_child(bit)?;
            }
            NodeKind::Lone | NodeKind::Direct | NodeKind::Chained => {
                // Leaf — check if provenance matches
                let prov = leaf_provenance(&blk)?;
                if &prov == key {
                    return Some(current);
                } else {
                    return None; // Different key at this path
                }
            }
            _ => return None,
        }
    }

    None // Exceeded max depth
}

/// Path entry for COW rebuilding.
struct PathEntry {
    node: InternalNode,
    bit: u32,
}

/// Insert or update an object in the HAMT. Returns new root reference.
///
/// `leaf_ref` is the BlockRef of an already-written leaf (lone, direct, or chained). `leaf_provenance` is the key (provenance hash from the leaf body).
///
/// The caller is responsible for writing the leaf block to storage first. This function writes new internal nodes via `io.write_block()` and returns the new root (hash, lba).
pub fn insert(
    io: &mut impl BlockIO,
    root: &BlockRef,
    leaf_ref: BlockRef,
    leaf_provenance: &[u8; 32],
) -> Option<BlockRef> {
    // Collect path from root to insertion point
    let mut path: Vec<PathEntry> = Vec::new();
    let mut current = *root;

    for level in 0..MAX_DEPTH {
        let blk = io.read_block(current.lba)?;

        // Verify
        let computed = block_hash(&blk)?;
        if &computed != &current.hash {
            return None;
        }

        match identify_block(&blk) {
            NodeKind::Internal => {
                let node = InternalNode::from_block(&blk, &current.hash)?;
                let bit = hash_index(leaf_provenance, level);

                if let Some(child) = node.get_child(bit) {
                    // Child exists — descend
                    path.push(PathEntry { node, bit });
                    current = child;
                } else {
                    // Empty slot — insert here
                    path.push(PathEntry { node, bit });
                    break;
                }
            }
            NodeKind::Lone | NodeKind::Direct | NodeKind::Chained => {
                // Existing leaf at this position
                let existing_prov = leaf_provenance_or_zero(&blk);

                if &existing_prov == leaf_provenance {
                    // Update: replace this leaf. Path already collected, just rebuild upward with new leaf ref.
                    break;
                } else {
                    // Collision: two different keys at same path position. Create intermediate nodes until they diverge.
                    let existing_ref = current;
                    let mut collision_level = level;

                    loop {
                        let existing_bit = hash_index(&existing_prov, collision_level);
                        let new_bit = hash_index(leaf_provenance, collision_level);

                        if existing_bit != new_bit {
                            // They diverge here — create node with both children
                            let split_node = InternalNode::empty()
                                .with_child(existing_bit, existing_ref)
                                .with_child(new_bit, leaf_ref);

                            let mut child_ref = write_node(io, &split_node)?;

                            for entry in path.iter().rev() {
                                child_ref = write_node(
                                    io,
                                    &entry.node.with_child(entry.bit, child_ref),
                                )?;
                            }

                            return Some(child_ref);
                        }

                        // Same bit — need to go deeper
                        collision_level += 1;
                        if collision_level >= MAX_DEPTH {
                            return None; // Should never happen with 256-bit hashes
                        }
                    }
                }
            }
            _ => return None,
        }
    }

    // Rebuild path bottom-up (COW)
    let mut child_ref = leaf_ref;

    for entry in path.iter().rev() {
        child_ref = write_node(io, &entry.node.with_child(entry.bit, child_ref))?;
    }

    Some(child_ref)
}

/// Read the provenance key from a leaf block; returns zeros if parsing fails. Used in collision paths where we've already identified the block as a leaf.
fn leaf_provenance_or_zero(blk: &[u8; BLOCK_SIZE]) -> [u8; 32] {
    leaf_provenance(blk).unwrap_or([0u8; 32])
}

/// Remove a key from the HAMT. Returns new root reference.
///
/// Uses COW: rebuilds the path without the leaf. Returns None if key not found or on I/O error.
pub fn remove(
    io: &mut impl BlockIO,
    root: &BlockRef,
    key: &[u8; 32],
) -> Option<BlockRef> {
    // Collect path from root to the leaf
    let mut path: Vec<PathEntry> = Vec::new();
    let mut current = *root;
    let mut found = false;

    for level in 0..MAX_DEPTH {
        let blk = io.read_block(current.lba)?;

        let computed = block_hash(&blk)?;
        if &computed != &current.hash {
            return None;
        }

        match identify_block(&blk) {
            NodeKind::Internal => {
                let node = InternalNode::from_block(&blk, &current.hash)?;
                let bit = hash_index(key, level);
                if let Some(child) = node.get_child(bit) {
                    path.push(PathEntry { node, bit });
                    current = child;
                } else {
                    return None; // Key not found
                }
            }
            NodeKind::Lone | NodeKind::Direct | NodeKind::Chained => {
                let prov = leaf_provenance(&blk)?;
                if &prov == key {
                    found = true;
                    break;
                } else {
                    return None; // Different key
                }
            }
            _ => return None,
        }
    }

    if !found || path.is_empty() {
        return None;
    }

    // Rebuild bottom-up, removing the leaf from the last node.
    let last = path.len() - 1;

    // If a node ends up with exactly one child, we could collapse it, but for simplicity we keep single-child nodes. The plow cleanup can optimize this during rotation.

    let mut child_ref = write_node(io, &path[last].node.without_child(path[last].bit))?;

    // Continue up the path
    for i in (0..last).rev() {
        let updated = path[i].node.with_child(path[i].bit, child_ref);
        child_ref = write_node(io, &updated)?;
    }

    Some(child_ref)
}
