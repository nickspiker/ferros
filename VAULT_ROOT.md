# VAULT_ROOT — ferros Vault Root Specification
**Version:** 0.0  
**Author:** Nick Spiker  
**Principle:** The address space is the index. The tree is arithmetic.

---

## What Vault Root Is

An ordered keystore. That is its entire purpose.

```
Given:    the root space
Find:     the most recent valid boot state
Fallback: previous state always findable via ordering
Point to: current state in ferros_vault
```

Nothing else. Vault root does not store objects, enforce
capabilities, manage namespaces, or know what a boot snapshot
contains. It finds the most recent valid one. That is all.

---

## The Core Insight

The root space is a flat array of equally-sized blocks, power of
two in count. We own this entire address space. We designed it.
Therefore we know where every possible block lives before reading
a single byte.

The binary tree is not a data structure written to disk.
It is arithmetic — a virtual navigation aid that exists only
in the traversal logic. Nothing is written at internal nodes.
Only leaves exist on disk.

```
Internal nodes:   address ranges only
                  pure arithmetic subdivision
                  nothing written, nothing to corrupt

Leaf blocks:      the actual boot state snapshots
                  evenly distributed across root space
                  ordered by generation number
```

---

## Structure

Root space is N blocks where N is a power of two:

```
┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
│  0  │  1  │  2  │  3  │  4  │  5  │  6  │  7  │  ... N-1
└─────┴─────┴─────┴─────┴─────┴─────┴─────┴─────┘

Each block: one 4KB SSD page
            VSF document
            BLAKE3 mandatory hash
            generation number
            prev_hash → previous valid block
            vault_ptr → current vault state

Blocks: evenly divided, ordered, power of two count
        uniform and deterministic
        each block position known and fixed
```

Virtual tree (arithmetic only, nothing written):

```
Level 0 (root):  entire space       [0 .. N-1]
Level 1:         [0 .. N/2-1]       [N/2 .. N-1]
Level 2:         [0 .. N/4-1]  etc.
...
Leaves:          individual blocks
```

---

## Finding Most Recent Valid Block

We know where every block lives. We subdivide arithmetically.
At each level we read one block from each half and compare
generation numbers. Follow the higher valid generation.

```
Boot traversal:

1. Split root space in half
   Read representative block from each half
   Higher valid generation? → descend into that half

2. Split chosen half in half
   Read one block from each quarter
   Higher valid generation? → descend

3. Repeat until single block remains
   That block = most recent valid boot state

Reads: log_2(N)
Cost:  O(log_2 N)
Scan:  never
```

Validation at each step:

```
Block candidate found:
  BLAKE3(block) matches block.hash?   integrity check
  block.generation matches derived?   ordering check
  Both match? → valid
  Either fails? → follow prev_hash → try previous
```

The generation is derived from the traversal itself.
We know what generation to expect before reading the block.
The block cannot lie about its generation.
We already know.

---

## Finding Oldest Block (Next Write Location)

Same traversal in reverse. Follow lowest valid generation
at each subdivision instead of highest.

```
During boot traversal:  already visited every level
                        already know which branch was older
                        oldest location known as byproduct

Next write target:      lowest valid generation block
Address computation:    AND mask on generation number
                        simple bit operation, power of two space

Example (8 blocks, 3 bits):
  generations found: [6, 2, 7, 1, 5, 3, 8, 4]
  oldest:            block 3 (generation 1)
  next write:        block 3
  new generation:    9
```

Write always goes to the oldest block. Wear is even and
deterministic across the entire root space.

---

## Ordering and Fallback

Every block carries:

```
generation:  VsfType::u(n)             EWE, monotonic, no ceiling
prev_hash:   VsfType::h(BLAKE3, hash)  points to previous valid block
block_hash:  VsfType::h(BLAKE3, hash)  mandatory integrity
vault_ptr:   VsfType::h(BLAKE3, hash)  points into ferros_vault
```

If the most recent block is corrupt:

```
BLAKE3 fails → block invalid
Follow prev_hash → previous valid block
Generation: ordered, always less than current
Try that block → valid? done
Invalid? → follow its prev_hash
Repeat

Termination: guaranteed
  Generation monotonically decreasing along prev_hash chain
  Chain reaches genesis in finite steps
  At least one valid block always exists
  Dual device mirror ensures this under hardware failure
```

---

## Wear Distribution

```
N blocks, power of two
Write rotation: strict generation order, oldest block first
Each block: written exactly once per N writes
Wear: perfectly uniform across entire root space

No hot spots:   nothing fixed, no counter block, no index block
No cold spots:  every block written in rotation
Deterministic:  write location computable by AND mask
Provable:       uniform wear is a mathematical property
                of strict rotation over power of two space
```

---

## Block Format

Every vault root block is a VSF document:

```
VSF document:

  Header:
    VsfType::h(BLAKE3, block_hash)      mandatory, auto-computed
    VsfType::l("ferros.vault_root")     schema identifier

  Ordering section ("vault_root.order"):
    VsfType::l("generation")  → VsfType::u(n)              EWE
    VsfType::l("prev_hash")   → VsfType::h(BLAKE3, hash)   previous block
                                genesis: VsfType::h(BLAKE3, [0u8;32])

  Pointer section ("vault_root.ptr"):
    VsfType::l("vault_ptr")   → VsfType::h(BLAKE3, hash)   vault object root
    VsfType::l("ledger_head") → VsfType::h(BLAKE3, hash)   ledger chain head
    VsfType::l("snap_hash")   → VsfType::h(BLAKE3, hash)   boot snapshot
```

Total block size: well within 4KB SSD page.
Remainder of page: zeroed, reserved for future fields.

---

## Relationship to Ring

Rings are the prior art for ordered keystores:

```
Ring:       ordered keystore        ✓
            kill-safe               ✓
            wear distributed        ✓
            previous always findable ✓
            linear search           ✗  O(n) boot scan

Vault root: ordered keystore        ✓
            kill-safe               ✓
            wear uniform            ✓
            previous always findable ✓
            linear search           eliminated
            boot lookup             O(log_2 N)
```

The ring is the concept. Vault root is the ring with its
only weakness eliminated by owning and understanding the
entire address space.

Nobody has done this for OS boot state management.

---

## Formal Properties

```
Theorem VaultRoot_Purpose:
  vault_root provides exactly two guarantees:
  1. location(root_space) → most recent valid block address
  2. ordering: ∀ invalid block b:
               prev_hash(b) → previous valid block
               prev_hash(b).generation < b.generation always

Theorem VaultRoot_BootCorrectness:
  ∀ boot instant t:
    traverse(root_space, max_valid_generation) →
      block with highest committed generation before t
    O(log_2 N) reads, deterministic

Theorem VaultRoot_WearUniformity:
  ∀ blocks b1, b2 in root_space:
    E[writes(b1)] = E[writes(b2)]
    writes perfectly uniform by strict rotation
    provable from power of two space + oldest-first write policy

Theorem VaultRoot_CorruptionRecovery:
  ∀ corrupt block b:
    BLAKE3(b) invalid → follow prev_hash
    prev_hash chain: monotonically decreasing generation
    termination: guaranteed in finite steps
    result: most recent valid committed state

Theorem VaultRoot_GenerationIntegrity:
  generation derived from traversal, not from block claim
  block cannot misrepresent its generation
  forgery: BLAKE3 preimage resistance, 2^-256
```

---

## Open Questions

```
1. N (number of blocks in root space)
   Determines: tree depth, boot read count, rotation period
   Constraint: power of two, fits wear budget
   Candidate: 4MB / 4KB = 1024 blocks, log_2(1024) = 10 reads
   Decision: pending hardware target

2. Representative block selection per half
   During traversal: which block in each half do we read?
   Options: midpoint, first block, last block, fixed offset
   Must be: deterministic, same answer every boot
   Decision: pending

3. Mirror interaction
   Both devices have independent root spaces
   Higher valid generation wins
   Sync protocol: after successful boot, resync lower device
   Formal spec: pending
```

---

*VAULT_ROOT 0.0 — The address space is the index. The tree is arithmetic.*
*Author: Nick Spiker*