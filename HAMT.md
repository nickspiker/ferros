# HAMT — Hash Array Mapped Trie
**Version:** Zil (0)
**Author:** Nick Spiker  
**Principle:** Edit one entry. Write four nodes. Share everything else.

---

## What HAMT Is

The persistent hash map powering ferros_vault object lookup.

```
External identity:  provenance hash (permanent, never changes)
Internal question:  where is the latest version of this object?
Answer:             HAMT lookup → current block hash

HAMT guarantees:
  O(log_32 N) lookup    effectively constant (4-6 nodes for any N)
  O(log_32 N) edit      2-4 node writes per change
  COW by design         old versions intact, new root per edit
  No full rewrite       ever, for any edit
```

---

## Why Not RedoxFS B-Tree

```
RedoxFS B-tree:   good for ordered data, range queries
                  COW implemented on top of block layer
                  tied to their specific block abstraction
                  overkill for pure hash lookup

HAMT:             designed for pure hash lookup
                  COW by construction, not by adaptation
                  simpler structure, simpler proof
                  no ordering overhead we don't need
```

RedoxFS is worth reading for block allocator patterns and atomic
write strategy. Their indexing structure: not applicable here.

---

## Structure

### Internal Node

```
32-way branching. Each node covers 5 bits of the hash.

VSF document:
  VsfType::h(BLAKE3, node_hash)          mandatory integrity
  VsfType::l("ferros.hamt.internal")     schema
  bitmap:   VsfType::u5(u32)             32-bit presence bitmap
                                         bit N set = child N exists
  children: [VsfType::h(BLAKE3, h); popcount(bitmap)]
                                         only present children stored
                                         sparse, no null pointers

Node address: BLAKE3(node contents)
              content-addressed
              same contents = same address always
```

### Leaf Node

```
VSF document:
  VsfType::h(BLAKE3, node_hash)          mandatory integrity
  VsfType::l("ferros.hamt.leaf")         schema
  key:   VsfType::h(BLAKE3, provenance)  the object's permanent identity
  value: VsfType::h(BLAKE3, block_hash)  current physical location
```

### Root

```
Single node hash: VsfType::h(BLAKE3, root_hash)
Stored in: vault root leaf (vault_ptr field)
Changes: every edit produces a new root hash
Old root: still valid, still readable, until GC
```

---

## Lookup

```
lookup(provenance_hash: [u8;32]) → Option<[u8;32]>:

  node = read(root_hash)
  for chunk in provenance_hash.chunks_of_5_bits():
    bit = chunk as u32
    if !node.bitmap & (1 << bit):
      return None   ← not found
    index = popcount(node.bitmap & ((1 << bit) - 1))
    child_hash = node.children[index]
    node = read(child_hash)
    if node.is_leaf():
      if node.key == provenance_hash:
        return Some(node.value)
      else:
        return None   ← hash collision path, different key

Reads: log_32(N) nodes
       N = 1,000,000 entries → 4 reads
       N = 1,000,000,000 entries → 6 reads
       Effectively constant for any realistic N
```

---

## Insert / Update (COW)

```
insert(provenance_hash, block_hash):

  path = traverse(root, provenance_hash)   collect nodes top to bottom
  
  new_leaf = LeafNode { key: provenance_hash, value: block_hash }
  write(new_leaf)   ← new VSF document on disk
  
  for node in path.reverse():
    new_node = copy(node)
    new_node.children[relevant_index] = hash(new_leaf or previous new_node)
    new_node.recompute_bitmap()
    write(new_node)   ← new VSF document on disk
    new_leaf = new_node
  
  new_root = hash(final new_node)
  
  Old nodes: untouched on disk
             still referenced by previous vault generation
             valid until GC
  New root:  new vault generation points here
  
Writes: log_32(N) + 1 nodes
        typically 2-4 VSF documents per edit
        never a full rewrite
```

---

## Delete

```
delete(provenance_hash):

  Same path traversal as insert
  Remove leaf
  Rebuild path upward with updated bitmaps
  Empty internal node: collapsed (parent bitmap bit cleared)
  
Writes: same as insert, log_32(N) nodes
```

---

## COW Versioning

Every edit produces a new root hash. Old structure is intact.

```
Generation 1:   root_hash_1 → HAMT state 1
Generation 2:   root_hash_2 → HAMT state 2
                shares most nodes with state 1
                only changed path is new
                
Vault root leaf: always points to current root_hash
Previous vault generation: still points to root_hash_1
                           fully readable
                           valid for rollback
```

This is how vault versioning works. The HAMT root hash in the
vault root leaf IS the version identifier. Changing one object
produces a new HAMT root. The vault root binary counter tree
records that new root in the next generation leaf.

---

## BLAKE3 Integrity

Every node carries mandatory VSF BLAKE3 hash.

```
Corrupt leaf:       BLAKE3 fails → lookup returns None
                    try previous vault generation
                    
Corrupt internal:   BLAKE3 fails → cannot traverse this path
                    try previous vault generation via prev_hash

Forge a node:       BLAKE3 preimage resistance 2^-256
                    content-addressed: forged content = different address
                    cannot inject false mappings
```

---

## Collision Handling

Standard HAMT collision strategy:

```
Two different provenance hashes with identical 5-bit path:
  Create collision node at that depth
  Store both leaves
  Full key comparison at leaf resolves which is which
  
Probability of collision at depth D:
  P = 1/32^D
  At depth 4: 1/32^4 = 1/1,048,576
  Extremely rare, handled correctly, no performance impact
```

---

## Node Storage

All HAMT nodes are vault objects. They live in the vault object
store, not in a separate index structure.

```
Vault object store:
  Regular vault objects (boot snapshots, app data, ledger backing)
  HAMT nodes (the index itself)
  All: VSF documents
  All: BLAKE3 content-addressed
  All: written atomically
  All: COW by construction
```

No separate index partition. No special block region. The HAMT
is just more vault objects. The vault indexes itself.

---

## No_std Implementation

HAMT implementation requirements:

```
no_std:         yes, required for kernel-adjacent use
no heap alloc:  no — HAMT nodes require allocation
                uses ferros_vault bump allocator
                or Vec where alloc available

Node size:      variable (sparse children array)
                typical internal node: ~200-400 bytes
                leaf node: ~80 bytes
                all well within VSF document budget
```

---

## Relationship to Vault Root

```
Vault root (binary counter tree):
  Finds current vault generation leaf
  That leaf contains: root_hash of current HAMT

HAMT:
  Given root_hash: finds any object by provenance hash
  Two lookups total:
    1. Vault root → current generation → HAMT root hash
    2. HAMT traversal → object block hash
  Both O(log N), both deterministic, both BLAKE3-verified
```

---

## Formal Properties

```
Theorem HAMT_Lookup:
  ∀ provenance hash p inserted at generation G:
    lookup(root_hash_G, p) = block_hash inserted at G
    
  ∀ generation G' > G where p not modified:
    lookup(root_hash_G', p) = same block_hash
    COW preserves unmodified entries

Theorem HAMT_EditCost:
  ∀ edit e on HAMT of size N:
    nodes_written(e) = log_32(N) + 1
    ≤ 7 for any N ≤ 2^32
    ≤ 13 for any N ≤ 2^64
    effectively constant

Theorem HAMT_Integrity:
  ∀ node n:
    BLAKE3(n) = n.node_hash (mandatory VSF hash)
    corrupt(n) → BLAKE3 mismatch → n excluded from traversal
    
  ∀ lookup path:
    every node on path verified by BLAKE3
    corrupt path → falls back to previous generation
    cannot return corrupt data silently

Theorem HAMT_COW_Isolation:
  ∀ generations G1 < G2:
    edit at G2 does not modify any node referenced by G1
    G1 remains fully readable and consistent
    nodes shared between G1 and G2: immutable by construction
```

---

## Implementation Notes

```
Bit manipulation:
  5 bits per level: hash >> (level * 5) & 0x1F
  Bitmap index:     popcount(bitmap & ((1 << bit) - 1))
  Use: u32::count_ones() for popcount — single instruction

Hash chunking:
  256-bit BLAKE3 hash / 5 bits per level = 51 levels maximum
  In practice: 4-6 levels for any realistic dataset
  51 levels handles 32^51 entries — not a concern

Node writes:
  Each new node: new VSF document
  Written to vault object store via atomic Ring FS write
  Node hash computed from content (mandatory VSF hash)
  Old node: remains valid until GC sweep
```

---

## What To Read

```
Phil Bagwell's original HAMT paper (2001):
  "Ideal Hash Trees"
  The canonical reference, small and readable

Clojure's PersistentHashMap:
  Production HAMT implementation, well documented
  32-way branching, popcount indexing
  Good reference for the bitmap manipulation

RedoxFS source:
  Read for: block allocator patterns, atomic write strategy
  Skip for: indexing structure (B-tree, not applicable)
```

---

*HAMT 0.0 — Edit one entry. Write four nodes. Share everything else.*  
*Author: Nick Spiker*