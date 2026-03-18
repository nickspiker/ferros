//! HAMT — Hash Array Mapped Trie for vault object lookup.
//!
//! 32-way branching, 5 bits per hash level, COW by construction.
//! Every node is one 4KB VSF document in the tract.
//!
//! See HAMT.md for the full specification.

extern crate alloc;

use alloc::vec::Vec;
use crate::vsf_mini::{VsfWriter, VsfReader, VSF_MAGIC};

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

/// Count of set bits below position `bit` in a u32 bitmap.
/// This gives the index into the sparse child array.
#[inline]
fn sparse_index(bitmap: u32, bit: u32) -> usize {
    (bitmap & ((1u32 << bit) - 1)).count_ones() as usize
}

// ---------------------------------------------------------------------------
// Block I/O trait — the kernel implements this with UFS + plow
// ---------------------------------------------------------------------------

/// Trait for reading and writing 4KB blocks.
///
/// The kernel provides this using UFS (+ SD mirror). The HAMT logic
/// is pure — it doesn't know about UFS, SD, or the plow.
pub trait BlockIO {
    /// Read a 4KB block at `lba`. Returns None on I/O error.
    fn read_block(&self, lba: u32) -> Option<[u8; BLOCK_SIZE]>;

    /// Write a 4KB block at the current plow position.
    /// Returns the LBA where it was written, or None on failure.
    /// The implementation handles write-verify and mirror protocol.
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
    /// Format:
    /// ```text
    /// RÅ< z(7) y(7) b(N) e(qtimer) hp(zeros→patched) n(1) >
    /// [ l("hamt.node")
    ///   v_u0(bitmap[32])       ← 32-element bit-packed bool vector
    ///   v_h(child_hashes[])    ← BLAKE3 hashes, popcount entries
    ///   v_u(child_lbas[])      ← LBAs, parallel array
    /// ]
    /// ```
    pub fn to_block(&self) -> [u8; BLOCK_SIZE] {
        let mut blk = [0u8; BLOCK_SIZE];
        let mut w = VsfWriter::new(&mut blk);

        // VSF header
        w.magic();
        w.version(7);
        w.backward_version(7);

        // Header length placeholder: b(0) = 'b' '3' 0x00
        let header_len_pos = w.pos();
        w.header_length(0);
        let header_body_start = w.pos();

        // Eagle time (QTIMER)
        w.eagle_time_qtimer(crate::vsf_mini::read_qtimer());

        // Provenance hash placeholder
        let hp_pos = w.hash_p_placeholder().unwrap();

        // Field count: 1 section
        w.field_count(1);

        // Close header
        w.close();

        let header_body_end = w.pos();

        // Body: single anonymous section
        w.section_open_anonymous();

        // Label
        w.label("hamt.node");

        // v_u0: bit-packed bool vector (32 elements = 4 bytes)
        w.put_raw(b"v_u0");
        w.put_raw(&self.bitmap.to_le_bytes());

        // v_h: child hashes array
        w.put_raw(b"v_h");
        w.put_ewe_uint_pub(self.child_count() as u64);
        for h in &self.child_hashes {
            w.put_raw(h);
        }

        // v_u: child LBAs array
        w.put_raw(b"v_u");
        w.put_ewe_uint_pub(self.child_count() as u64);
        for &lba in &self.child_lbas {
            w.put_ewe_uint_pub(lba as u64);
        }

        w.section_close();

        // Patch header_length (same pattern as ring.rs)
        let header_body_len = header_body_end - header_body_start - 1; // -1 for '>'
        drop(w);
        if header_body_len <= 255 {
            blk[header_len_pos + 2] = header_body_len as u8;
        }

        // Compute BLAKE3 with hp field zeroed (it's already zeros)
        let hash = blake3::hash(&blk);
        blk[hp_pos..hp_pos + 32].copy_from_slice(hash.as_bytes());

        blk
    }

    /// Parse from a 4KB block. Returns None if not a valid HAMT node.
    pub fn from_block(blk: &[u8; BLOCK_SIZE], expected_hash: &[u8; 32]) -> Option<Self> {
        // Verify BLAKE3: zero out hp field, hash, compare
        let mut verify_buf = *blk;
        let hp_pos = find_hp_position(&verify_buf)?;
        verify_buf[hp_pos..hp_pos + 32].fill(0);
        let computed = blake3::hash(&verify_buf);
        if computed.as_bytes() != expected_hash {
            return None;
        }

        let mut r = VsfReader::new(blk);

        // Skip VSF header
        if !r.magic() { return None; }
        r.version()?;
        r.backward_version()?;
        r.header_length()?;

        // Skip eagle time
        r.eagle_time_qtimer();

        // Read provenance hash (skip it, we already verified)
        r.hash_p()?;

        // Field count
        r.field_count()?;

        // Close header
        if !r.close() { return None; }

        // Body: section open
        if r.read_byte_raw()? != b'[' { return None; }

        // Label: "hamt.node"
        let label = r.label()?;
        if label != b"hamt.node" { return None; }

        // v_u0: bitmap
        if r.read_byte_raw()? != b'v' { return None; }
        if r.read_byte_raw()? != b'_' { return None; }
        if r.read_byte_raw()? != b'u' { return None; }
        if r.read_byte_raw()? != b'0' { return None; }
        let bitmap_bytes = r.read_bytes_raw(4)?;
        let bitmap = u32::from_le_bytes([
            bitmap_bytes[0], bitmap_bytes[1],
            bitmap_bytes[2], bitmap_bytes[3],
        ]);

        let count = bitmap.count_ones() as usize;

        // v_h: child hashes
        if r.read_byte_raw()? != b'v' { return None; }
        if r.read_byte_raw()? != b'_' { return None; }
        if r.read_byte_raw()? != b'h' { return None; }
        let h_count = r.read_ewe_uint()? as usize;
        if h_count != count { return None; }

        let mut child_hashes = Vec::with_capacity(count);
        for _ in 0..count {
            let h = r.read_bytes_raw(32)?;
            let mut arr = [0u8; 32];
            arr.copy_from_slice(h);
            child_hashes.push(arr);
        }

        // v_u: child LBAs
        if r.read_byte_raw()? != b'v' { return None; }
        if r.read_byte_raw()? != b'_' { return None; }
        if r.read_byte_raw()? != b'u' { return None; }
        let u_count = r.read_ewe_uint()? as usize;
        if u_count != count { return None; }

        let mut child_lbas = Vec::with_capacity(count);
        for _ in 0..count {
            let lba = r.read_ewe_uint()? as u32;
            child_lbas.push(lba);
        }

        Some(InternalNode { bitmap, child_hashes, child_lbas })
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
/// Format:
/// ```text
/// RÅ< z(7) y(7) b(N) e(qtimer) hp(provenance) hb(body_hash) n(1) >
/// [ l("vault.lone") v(content) ]
/// ```
pub fn lone_leaf_to_block(provenance: &[u8; 32], content: &[u8]) -> Option<[u8; BLOCK_SIZE]> {
    if content.len() > BLOCK_SIZE - 128 {
        // Too large for inline — rough check, exact budget ~3950 bytes
        return None;
    }

    let mut blk = [0u8; BLOCK_SIZE];
    let mut w = VsfWriter::new(&mut blk);

    w.magic();
    w.version(7);
    w.backward_version(7);

    let header_len_pos = w.pos();
    w.header_length(0);
    let header_body_start = w.pos();

    w.eagle_time_qtimer(crate::vsf_mini::read_qtimer());
    w.hash_p(provenance);

    // Body hash = BLAKE3 of content
    let body_hash = blake3::hash(content);
    w.hash_b(body_hash.as_bytes());

    w.field_count(1);

    // Close header
    w.close();

    let header_body_end = w.pos();

    // Body
    w.section_open_anonymous();
    w.label("vault.lone");

    // v(content): 'v' + EWE(len) + bytes
    w.put_raw(b"v");
    w.put_ewe_uint_pub(content.len() as u64);
    w.put_raw(content);

    w.section_close();

    // Patch header_length
    let header_body_len = header_body_end - header_body_start - 1;
    drop(w);
    if header_body_len <= 255 {
        blk[header_len_pos + 2] = header_body_len as u8;
    }

    Some(blk)
}

/// Check if a block is a lone leaf (has "vault.lone" label).
/// Returns (provenance_hash, body_hash, content_slice) if valid.
pub fn parse_lone_leaf(blk: &[u8; BLOCK_SIZE]) -> Option<([u8; 32], [u8; 32], &[u8])> {
    let mut r = VsfReader::new(blk);

    if !r.magic() { return None; }
    r.version()?;
    r.backward_version()?;
    r.header_length()?;

    // Eagle time (skip)
    r.eagle_time_qtimer();

    let provenance = *r.hash_p()?;
    let body_hash = *r.hash_b()?;

    r.field_count()?;
    if !r.close() { return None; }

    // Section open
    if r.read_byte_raw()? != b'[' { return None; }

    let label = r.label()?;
    if label != b"vault.lone" { return None; }

    // v(content)
    if r.read_byte_raw()? != b'v' { return None; }
    let len = r.read_ewe_uint()? as usize;
    let content = r.read_bytes_raw(len)?;

    // Verify body hash
    let computed = blake3::hash(content);
    if computed.as_bytes() != &body_hash {
        return None;
    }

    Some((provenance, body_hash, content))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the position of the hp hash bytes in a VSF document.
/// Scans for 'h' 'p' EWE(31) pattern after the magic.
fn find_hp_position(blk: &[u8]) -> Option<usize> {
    // hp is: 'h' 'p' '3' 0x1F [32 bytes]
    // The EWE encoding of 31 is: '3' 0x1F
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

/// Identify what kind of node a block contains.
pub enum NodeKind {
    Internal,
    Lone,
    Direct,
    Chained,
    Extent,
    Unknown,
}

/// Peek at a block's label to determine its kind.
pub fn identify_block(blk: &[u8; BLOCK_SIZE]) -> NodeKind {
    // Quick scan for label after section open
    // Label format: 'l' EWE(len) bytes
    // We look for known labels
    for i in 0..blk.len().saturating_sub(16) {
        if blk[i] == b'l' {
            // Try to read the label
            let mut r = VsfReader::new(&blk[i..]);
            if let Some(label) = r.label() {
                return match label {
                    b"hamt.node" => NodeKind::Internal,
                    b"vault.lone" => NodeKind::Lone,
                    b"vault.direct" => NodeKind::Direct,
                    b"vault.chained" => NodeKind::Chained,
                    b"vault.extent" => NodeKind::Extent,
                    _ => NodeKind::Unknown,
                };
            }
        }
    }
    NodeKind::Unknown
}

// ---------------------------------------------------------------------------
// Write helper
// ---------------------------------------------------------------------------

/// Serialize a node, write it via BlockIO, return its BlockRef.
fn write_node(io: &mut impl BlockIO, node: &InternalNode) -> Option<BlockRef> {
    let blk = node.to_block();
    // Hash is already embedded in the block by to_block()
    let hp_pos = find_hp_position(&blk)?;
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&blk[hp_pos..hp_pos + 32]);
    let lba = io.write_block(&blk)?;
    Some(BlockRef { hash, lba })
}

/// Compute the BLAKE3 hash for an already-serialized block (hp zeroed, then hash).
fn block_hash(blk: &[u8; BLOCK_SIZE]) -> Option<[u8; 32]> {
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
/// Returns the block reference for the object, or None if not found.
/// On BLAKE3 mismatch at any node, returns None (caller should try
/// previous spine generation).
pub fn lookup(
    io: &impl BlockIO,
    root: &BlockRef,
    key: &[u8; 32],
) -> Option<BlockRef> {
    let mut current = *root;

    for level in 0..MAX_DEPTH {
        let blk = io.read_block(current.lba)?;

        // Verify BLAKE3
        let mut verify_buf = blk;
        let hp_pos = find_hp_position(&verify_buf)?;
        verify_buf[hp_pos..hp_pos + 32].fill(0);
        let computed = blake3::hash(&verify_buf);
        if computed.as_bytes() != &current.hash {
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
                let mut r = VsfReader::new(&blk);
                if !r.magic() { return None; }
                r.version()?;
                r.backward_version()?;
                r.header_length()?;
                r.eagle_time_qtimer();
                let prov = r.hash_p()?;
                if prov == key {
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
/// `leaf_block` is the already-serialized 4KB leaf (lone, direct, or chained).
/// `leaf_provenance` is the key (provenance hash from the leaf header).
///
/// The caller is responsible for writing the leaf block to storage first.
/// This function writes new internal nodes via `io.write_block()` and
/// returns the new root (hash, lba).
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
        let mut verify_buf = blk;
        let hp_pos = find_hp_position(&verify_buf)?;
        verify_buf[hp_pos..hp_pos + 32].fill(0);
        let computed = blake3::hash(&verify_buf);
        if computed.as_bytes() != &current.hash {
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
                let mut r = VsfReader::new(&blk);
                if !r.magic() { return None; }
                r.version()?;
                r.backward_version()?;
                r.header_length()?;
                r.eagle_time_qtimer();
                let existing_prov = *r.hash_p()?;

                if &existing_prov == leaf_provenance {
                    // Update: replace this leaf. Path already collected,
                    // just rebuild upward with new leaf ref.
                    break;
                } else {
                    // Collision: two different keys at same path position.
                    // Create intermediate nodes until they diverge.
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

/// Remove a key from the HAMT. Returns new root reference.
///
/// Uses COW: rebuilds the path without the leaf.
/// Returns None if key not found or on I/O error.
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

        let mut verify_buf = blk;
        let hp_pos = find_hp_position(&verify_buf)?;
        verify_buf[hp_pos..hp_pos + 32].fill(0);
        let computed = blake3::hash(&verify_buf);
        if computed.as_bytes() != &current.hash {
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
                let mut r = VsfReader::new(&blk);
                if !r.magic() { return None; }
                r.version()?;
                r.backward_version()?;
                r.header_length()?;
                r.eagle_time_qtimer();
                let prov = r.hash_p()?;
                if prov == key {
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

    // Rebuild bottom-up, removing the leaf from the last node
    let last = path.len() - 1;

    // If node has exactly one child left, we could collapse it,
    // but for simplicity we keep single-child nodes. The plow
    // cleanup can optimize this during rotation.

    let mut child_ref = write_node(io, &path[last].node.without_child(path[last].bit))?;

    // Continue up the path
    for i in (0..last).rev() {
        let updated = path[i].node.with_child(path[i].bit, child_ref);
        child_ref = write_node(io, &updated)?;
    }

    Some(child_ref)
}
