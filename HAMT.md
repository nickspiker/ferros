# HAMT — Hash Array Mapped Trie
**Version:** Zila (1)
**Author:** Nick Spiker
**Principle:** Edit one entry. Write four nodes. Share everything else.

---

## What HAMT Is

The persistent hash map powering vault object lookup.

```
External identity:  provenance hash (permanent, never changes)
Internal question:  where is the latest version of this object?
Answer:             HAMT lookup → current block (hash, lba)

HAMT guarantees:
  O(log_32 N) lookup    effectively constant (4-6 nodes for any N)
  O(log_32 N) edit      2-4 node writes per change
  COW by design         old versions intact, new root per edit
  No full rewrite       ever, for any edit
```

The HAMT lives inside the tract (see VAULT.md). Its nodes are
ordinary tract blocks — the plow relocates them like any other
live block. The index indexes itself.

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
                  unordered on disk — deliberate, not accidental
```

HAMT does not support range queries or sorted iteration.
Every access is: "give me the object with this provenance hash."
That is the only question the vault asks.

---

## Branching

32-way branching. Each level consumes 5 bits of the hash.

```
k bits per level → 2^k children → 256/k max levels

k=1 (binary):   256 levels. Deep. Many reads.
k=4 (16-way):    64 levels. 4 reads for 1M entries.
k=5 (32-way):    51 levels. 4 reads for 1M entries. ← this one
k=8 (256-way):   32 levels. 3 reads. Fat nodes.

32-way is the Goldilocks point:
  Bitmap = one u32, popcount = one instruction
  N = 1,000,000 → 4 reads
  N = 1,000,000,000 → 6 reads
  Effectively constant for any realistic N
```

---

## Node Formats

### Internal Node

```
RÅ<hp(node_hash)>
  [d("hamt.node")]
  [v_u0(presence[32])]         ← bit-packed bool vector, 32 elements
  [v_h(child_hashes[])]        ← BLAKE3 hashes, popcount(bitmap) entries
  [v_u(child_lbas[])]          ← LBAs, parallel array, same index

Each child: (hash, lba)
  hash = BLAKE3 of the child node, verified on read
  lba  = physical block address in the tract

Node address: BLAKE3(node contents) = hp in header

Sparse: only present children stored (popcount indexing)
  Bit N in presence vector → child N exists
  Index into child arrays = popcount(presence[0..N])
```

### Leaf Node — Lone (inline object, < ~3.9KB)

Object content directly in the leaf. One disk read.

```
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.lone")]
  [access()                           ← optional, per-object override
    [admin(ke{pubkey})]
    [writers() ke{...} ...]
    [readers() [wrap() ke{} kx{} v{}] ...]
  ]
  [v(content)]
```

Objects without `access()` inherit their namespace's ACD.
With access section inline, content budget shrinks (~116B per reader
wrap). If content + access > 4KB → promote to direct.

### Leaf Node — Direct (furrow LBAs in leaf, < ~4MB)

```
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.direct")]
  [size(u{total_bytes})]
  [v_u(furrow_lbas[])]          ← up to ~1000 LBAs
  [access() ...]                ← optional, inline if fits
  [acl(h{hash} u{lba})]        ← optional, spill pointer if access too large
```

Each furrow carries its own hb for per-block integrity.
The extent list only needs LBAs — no hashes in the list.

### Leaf Node — Chained (extent chain, > ~4MB)

```
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.chained")]
  [size(u{total_bytes})]
  [head(h{hash} u{lba})]       ← first extent node
  [access() ...]                ← optional
  [acl(h{hash} u{lba})]        ← optional
```

### Extent Node (chain link)

```
RÅ<hp(node_hash)>
  [d("vault.extent")]
  [v_u(furrow_lbas[])]          ← up to ~1000 LBAs
  [next(h{hash} u{lba})]       ← absent if last node
```

### Furrow (extent data block)

```
RÅ<hp(provenance) hb(block_hash)>
  [m(block_index)]
  [v(payload)]
```

~45 bytes overhead, ~4050 bytes payload. Minimal VSF envelope.
Every block on disk has VSF magic — the plow's liveness scanner
has one code path for all blocks.

---

## Lookup

```
lookup(provenance_hash) → Option<(hash, lba)>:

  (root_hash, root_lba) = spine.hamt_root
  node = read_and_verify(root_lba, root_hash)

  for level in 0..:
    bit = (provenance_hash >> (level * 5)) & 0x1F
    if !node.presence[bit]:
      return None                         ← not found
    index = popcount(node.presence[0..bit])
    child_hash = node.child_hashes[index]
    child_lba  = node.child_lbas[index]
    node = read_and_verify(child_lba, child_hash)
    if node.is_leaf():
      if node.provenance == provenance_hash:
        return Some(node.value)           ← found
      else:
        return None                       ← collision path, different key

Reads: log_32(N) nodes
       N = 1,000,000 entries → 4 reads
       N = 1,000,000,000 entries → 6 reads

Every hop: read block at lba, verify BLAKE3 == expected hash.
Corruption at any level → fall back to previous spine generation.
```

---

## Insert / Update (COW)

```
insert(provenance_hash, value):

  path = traverse(root, provenance_hash)    collect (node, index) top to bottom

  new_leaf = Leaf { provenance_hash, value }
  write(new_leaf) at plow → get (leaf_hash, leaf_lba)

  for (node, index) in path.reverse():
    new_node = copy(node)
    new_node.child_hashes[index] = leaf_hash or previous new_node hash
    new_node.child_lbas[index]   = leaf_lba or previous new_node lba
    write(new_node) at plow → get (node_hash, node_lba)
    leaf_hash = node_hash
    leaf_lba  = node_lba

  new_root = (final node_hash, final node_lba)
  → stored in next spine entry

Old nodes: untouched in tract
           still referenced by previous spine generation
           plow tramples them when it rotates thru

Writes: log_32(N) + 1 nodes
        typically 2-4 new blocks per edit
        never a full rewrite
```

---

## Delete

Two mechanisms, used together:

```
Fast delete (O(1)):
  Zero VSF magic bytes on both disks
  Object immediately invisible to reads
  HAMT entry becomes stale (points to zeroed block)
  Stale entry cleaned up during plow rotation

Full delete (O(log_32 N)):
  Standard HAMT remove: rebuild path with leaf removed
  COW produces new root without that entry
  Used during plow cleanup or when immediate consistency needed
```

---

## COW Versioning

Every edit produces a new root. Old structure is intact.

```
Generation 1:   (root_hash_1, root_lba_1) → HAMT state 1
Generation 2:   (root_hash_2, root_lba_2) → HAMT state 2
                shares most nodes with state 1
                only changed path is new

Spine entry: always points to current (root_hash, root_lba)
Previous spine entry: still points to root_1
                      fully readable
                      valid for rollback
```

This is how vault versioning works. The HAMT root in the
spine entry IS the version identifier. Changing one object
produces a new HAMT root. The spine records that new root
in the next generation entry.

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

## BLAKE3 Integrity

Every node carries mandatory VSF BLAKE3 hash.

```
Corrupt leaf:       BLAKE3 fails → lookup returns None
                    try previous spine generation

Corrupt internal:   BLAKE3 fails → cannot traverse this path
                    try previous spine generation via prev_hash

Forge a node:       BLAKE3 preimage resistance 2^-256
                    content-addressed: forged content = different address
                    cannot inject false mappings
```

---

## Plow Interaction

HAMT nodes live in the tract. The plow treats them like any
other block.

```
Plow reaches a live HAMT internal node:
  Relocate: write copy at plow, update PARENT node (COW path upward)
  This produces a new root → new spine entry
  Batched with other relocations in the same commit

Plow reaches a live HAMT leaf (lone):
  Contains an inlined object
  Promotion opportunity: de-inline object to separate tract block
  Leaf becomes direct reference instead of inline content
  Batched into spine commit

Plow reaches a dead HAMT node (superseded by COW):
  Trample. No update needed.
```

---

## Implementation Notes

```
Bit manipulation:
  5 bits per level: hash >> (level * 5) & 0x1F
  Bitmap index:     popcount(presence[0..bit])
  Use: u32::count_ones() for popcount — single ARM instruction

Hash chunking:
  256-bit BLAKE3 hash / 5 bits per level = 51 levels maximum
  In practice: 4-6 levels for any realistic dataset
  51 levels handles 32^51 entries — not a concern

Vector encoding:
  v_u0: bit-packed booleans, 32 elements = 4 bytes
  v_h:  packed BLAKE3 hashes, 32 bytes each
  v_u:  packed EWE integers (LBAs), ~4 bytes each
  Parallel arrays: same index into v_h and v_u = one child

Node size:
  Sparse internal node (6 children): ~240 bytes + overhead
  Dense internal node (32 children): ~1200 bytes + overhead
  All well within 4KB block budget
  One node per block, always
```

---

## Formal Properties

```
Theorem HAMT_Lookup:
  ∀ provenance hash p inserted at generation G:
    lookup(root_G, p) = (block_hash, block_lba) inserted at G

  ∀ generation G' > G where p not modified:
    lookup(root_G', p) = same result
    COW preserves unmodified entries

Theorem HAMT_EditCost:
  ∀ edit e on HAMT of size N:
    nodes_written(e) = log_32(N) + 1
    ≤ 7 for any N ≤ 2^32
    ≤ 13 for any N ≤ 2^64
    effectively constant

Theorem HAMT_Integrity:
  ∀ node n:
    BLAKE3(n) = n.hp (mandatory VSF hash)
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

## What To Read

```
Phil Bagwell's original HAMT paper (2001):
  "Ideal Hash Trees"
  The canonical reference, small and readable

Clojure's PersistentHashMap:
  Production HAMT implementation, well documented
  32-way branching, popcount indexing
  Good reference for the bitmap manipulation
```

---
