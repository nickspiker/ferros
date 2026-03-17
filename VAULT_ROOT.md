# VAULT_ROOT — ferros Vault Root Specification
**Version:** Zil (0)
**Author:** Nick Spiker 
**Principle:** The ring is the index. Binary search finds the head. VSF is the format.

---

## What Vault Root Is

A ring of boot state snapshots. That is its entire purpose.

```
Given:    a ring of N entries on persistent storage
Find:     the most recent valid boot state (highest generation)
Method:   binary search on generation numbers — O(log2 N) reads
Fallback: previous entry always findable via prev_hash chain
Point to: current HAMT root, cap table, process snapshots
```

Nothing else. Vault root does not store objects, enforce
capabilities, manage namespaces, or know what a boot snapshot
contains. It finds the most recent valid one. That is all.

The bootloader (kernel first stage) scans the vault root ring
after bootstrap verifies the kernel and jumps. See SECURITY_CHAIN.md
for the full trust model.

---

## Ring Structure

The vault root is a fixed-size ring of N entries, where N is a
power of two. Each entry is one 4KB block — the natural write unit
for both UFS (4KB logical block) and SD card (4KB aligned for FTL
efficiency).

```
Ring layout (N = 1024 = 4MB total):

┌────┬────┬────┬────┬────┬ ─ ─ ─ ┬────┬────┐
│  0 │  1 │  2 │  3 │  4 │       │1022│1023│
└────┴────┴────┴────┴────┴ ─ ─ ─ ┴────┴────┘
 4KB  4KB  4KB  4KB  4KB          4KB  4KB

Each entry: one 4KB block, one VSF document
Write position: generation % N
Wraps: after N writes, overwrites oldest entry
```

The ring lives at a fixed offset on each storage device:

```
UFS:  blocks 0-1023 of a dedicated 4MB region (LBA aligned)
SD:   blocks 0-1023 of a dedicated 4MB region (LBA aligned)

Both rings are mirrors. Higher valid generation wins on boot.
```

---

## Entry Format (VSF Document)

Every vault root entry is a complete VSF document, serialized per
the VSF specification. All integers use EWE encoding. All hashes
are VsfType::h (BLAKE3, 32 bytes).

```
VSF document (fits in one 4KB block):

Header:
  VsfType::h(BLAKE3, entry_hash)          mandatory, auto-computed
  VsfType::l("ferros.vault_root")         schema identifier

Ordering section ("vault_root.order"):
  VsfType::l("generation")    → VsfType::u(n)              EWE, monotonic
  VsfType::l("prev_hash")    → VsfType::h(BLAKE3, hash)   previous entry
                                genesis: VsfType::h(BLAKE3, [0u8;32])

State section ("vault_root.state"):
  VsfType::l("hamt_root")    → VsfType::h(BLAKE3, hash)   HAMT root node
  VsfType::l("cap_snapshot") → VsfType::h(BLAKE3, hash)   capability table
  VsfType::l("proc_snapshot")→ VsfType::h(BLAKE3, hash)   running processes
  VsfType::l("ledger_head")  → VsfType::h(BLAKE3, hash)   ledger chain head

Integrity section ("vault_root.integrity"):
  VsfType::l("kernel_hash")  → VsfType::h(BLAKE3, hash)   kernel that wrote this
  VsfType::l("eagle_time")   → EtType::ei(t)              physics-bounded timestamp

Remainder of 4KB block: zeroed (reserved for future fields)
```

**Why VSF for every entry:**

```
Traditional ring:   fixed struct, hope the fields are enough,
                    migration when you need more fields

VSF ring:           self-describing, skip unknown fields,
                    add fields without breaking old readers,
                    mandatory BLAKE3 catches any corruption,
                    encryption at rest via VSF ChaCha20 envelope
```

---

## Binary Search

The ring is ordered by generation number (monotonically increasing,
wrapping). Binary search finds the highest valid generation in
O(log2 N) reads.

```
Finding the most recent valid entry:

  The ring has N = 2^k entries. Generations increase monotonically
  and wrap: the ring always contains entries spanning a range of
  N consecutive generations (or fewer if < N writes have occurred).

  The "seam" is where the newest entry is adjacent to the oldest
  (the wrap point). Binary search locates this seam.

Algorithm:
  1. Read entry at position 0 and position N/2
  2. Compare generation numbers
  3. The half containing the higher generation has the seam
     (or the higher generation IS the newest)
  4. Subdivide that half, repeat
  5. After log2(N) reads: found the newest entry

  N = 1024 → 10 reads to find newest entry
  Each read: one 4KB block from UFS or SD

Validation at each read:
  VsfType::h(BLAKE3, entry_hash) valid?
    Yes → entry is usable, compare generation
    No  → entry is corrupt, treat as empty

Edge cases:
  All entries empty (first boot):
    Binary search finds no valid entries → genesis
    Create entry 0 with generation 1

  Sparse ring (< N entries written):
    Empty slots have no valid VSF header → treated as empty
    Binary search still works — empty = generation 0

  Single corrupt entry:
    Skipped during search
    prev_hash chain finds previous valid entry
```

---

## DISCARD Strategy

Both UFS and SD have Flash Translation Layers that handle erase
internally. We never erase directly. But DISCARD (TRIM) tells the
FTL "this region is no longer in use, you can reclaim it."

```
UFS erase block: 4MB (1 allocation unit = 1 segment = 1024 × 4KB blocks)
SD erase:        FTL handles it (ERASE_BLK_EN=1, 512-byte granularity)

DISCARD pattern:
  The ring is 4MB total (1024 × 4KB entries)
  This equals exactly one UFS erase block

  For a ring that wraps every 1024 writes:
    No DISCARD needed — the ring is one erase block
    FTL sees sequential overwrites within one erase unit
    This is optimal for flash — sequential writes, single region

  For larger rings (future, userspace):
    DISCARD in 4MB chunks as the ring wraps past each segment
    Always 4MB-aligned to match UFS erase blocks
```

---

## Mirror Protocol

Both UFS and SD carry identical vault root rings. Mirroring provides
redundancy: either device alone contains the complete boot state.

```
Write protocol (write-verify-then-mirror):
  1. Construct new VSF entry (generation N+1)
  2. Write to UFS at position (generation % ring_size)
  3. Read back from UFS, BLAKE3 verify
  4. If verify fails → retry or mark UFS position bad, do not proceed
  5. UFS verified → write to SD at same logical position
  6. Read back from SD, BLAKE3 verify
  7. If SD verify fails → UFS still has it, SD marked degraded
  8. Both verified → generation N+1 committed on both media

  Never write to the second device until the first is verified.
  A generation is "committed" when at least one device holds
  a verified copy.

Read protocol (boot):
  1. Binary search on UFS ring → generation_ufs
  2. Binary search on SD ring → generation_sd
  3. Use max(generation_ufs, generation_sd)
  4. If equal: use UFS (faster)

Resync protocol (after boot):
  If UFS and SD generations differ:
    Copy entries from higher to lower until equal
    Normal operation resumes
  If one device is missing (SD removed):
    Boot from UFS only
    When SD inserted: full ring copy from UFS to SD

Device replacement:
  Remove SD → system runs on UFS only
  Insert new SD → full ring copy from UFS, then normal mirror
  UFS dies → boot from SD, buy new phone, SD has everything
```

---

## Dual-Slot (A/B)

Two independent boot paths, like Android A/B but for the vault root.
Each slot points to a different kernel+state combination.

```
Slot A: current production state
Slot B: update staging area

Kernel update flow:
  1. Write new kernel to slot B's partition
  2. Write new vault root entry pointing to slot B state
  3. Verify: read back, BLAKE3 check
  4. Mark slot B as active
  5. Reboot → bootstrap loads slot B kernel
  6. If boot succeeds: slot B becomes new production
  7. If boot fails: fall back to slot A (previous state)

Slot metadata in vault root entry:
  VsfType::l("active_slot")  → VsfType::u(0 or 1)    A or B
```

---

## Relationship to Other Specs

```
SECURITY_CHAIN.md:
  Bootstrap verifies kernel → jumps
  Kernel (bootloader stage) scans vault root ring
  Vault root is the FIRST thing the bootloader reads

HAMT.md:
  Vault root entry.hamt_root → HAMT root node hash
  HAMT provides O(log32 N) object lookup
  Vault root provides the HAMT root for each generation

LEDGER.md:
  Vault root entry.ledger_head → ledger chain head hash
  Ledger chain is restored from this pointer on boot

VAULT.md:
  Vault root is the entry point into the vault
  Everything in the vault is reachable from the vault root
  HAMT root → objects → everything
```

---

## Wear Distribution

```
Ring of N entries, power of two
Write rotation: strict generation order, (generation % N)
Each position: written exactly once per N writes
Wear: perfectly uniform across the ring

N = 1024, kernel writes ~1 entry per boot snapshot:
  1024 boots per full rotation
  UFS rated 3000+ P/E cycles
  Ring lasts: 3,000,000+ boots — effectively unlimited

Even at 100 writes/day (aggressive):
  1024 / 100 = 10.24 days per rotation
  3000 rotations × 10.24 days = 84 years
  Wear is not a concern for the kernel ring
```

---

## Formal Properties

```
Theorem VaultRoot_FindNewest:
  ∀ ring of N = 2^k entries:
    binary_search(ring) → entry with highest valid generation
    in exactly k reads (one per binary search level)
    O(log2 N) deterministic

Theorem VaultRoot_KillSafety:
  ∀ kill instant t:
    at most one entry is partially written
    partial entry: VsfType::h(BLAKE3) fails → entry invalid
    previous entry: always valid (was committed before this write)
    binary search skips invalid entries
    system always boots from last committed state

Theorem VaultRoot_MirrorRedundancy:
  ∀ single device failure:
    other device has complete vault root ring
    boot proceeds from surviving device
    resync restores mirror after replacement

Theorem VaultRoot_WearUniformity:
  ∀ positions p1, p2 in ring:
    E[writes(p1)] = E[writes(p2)]
    strict rotation over power of two space
    mathematically uniform

Theorem VaultRoot_GenerationIntegrity:
  ∀ entry e:
    e.generation embedded in VSF document
    VsfType::h(BLAKE3) covers generation field
    cannot forge generation without breaking BLAKE3
    binary search trusts generation only after BLAKE3 passes
```

---

## Open Questions

```
1. Ring size (N)
   Current candidate: 1024 (4MB, 10 reads to find newest)
   Could be: 256 (1MB, 8 reads) or 4096 (16MB, 12 reads)
   Decision: 1024 is fine — 10 reads × 4KB = 40KB total I/O at boot

2. Ring base address on UFS
   Needs: fixed LBA range that doesn't conflict with Android partitions
   Option: use a dedicated UFS LUN (if available)
   Option: allocate at end of LUN0
   Decision: pending partition table analysis

3. Ring base address on SD
   Needs: fixed LBA range outside the GPT partition table
   Option: dedicated partition
   Option: raw blocks after GPT
   Decision: pending

4. ABL signing for nag elimination
   Requires: understanding vbmeta key format
   See SECURITY_CHAIN.md for current status
   Decision: accept nag for now, research later
```

---

*VAULT_ROOT 0.1 — The ring is the index. Binary search finds the head. VSF is the format.*
*Author: Nick Spiker*
