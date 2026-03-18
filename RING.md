# RING — ferros Ring Specification
**Version:** Zila (1)
**Author:** Nick Spiker
**Principle:** The ring is the index. Binary search finds the head. VSF is the format.

---

## What The Ring Is

A ring is a fixed-size array of generation-numbered entries on persistent
storage. Binary search finds the most recent valid entry in O(log2 N)
reads. Rings are the foundation of all ferros persistent state.

```
Instances:
  Stem      kernel ring       256 entries    1MB      block G#C00
  Spine     vault root ring   65536 entries  256MB    block G#2000
  State     state ring        262144 entries 1GB      block G#40000
  Ledger    ledger ring       262144 entries 1GB      block G#80000
```

The spine is the primary example. It stores committed vault state.

```
Given:    a ring of N entries on persistent storage
Find:     the most recent valid entry (highest generation)
Method:   binary search on generation numbers — O(log2 N) reads
Fallback: previous entry always findable via prev_hash chain

Spine points to: current HAMT root, plow position, ledger head
Stem points to:  current kernel binary location, hash, signature
```

The ring does not store objects, enforce capabilities, manage
namespaces, or know what its entries mean. It finds the most
recent valid one. That is all.

The seed scans the stem after self-verification.
The kernel scans the spine after the seed jumps.
See SEED.md and BOOT.md for the full trust model.

---

## Ring Structure

The vault root is a fixed-size ring of N entries, where N is a
power of two. Each entry is one 4KB block — the natural write unit
for both UFS (4KB logical block) and SD card (4KB aligned for FTL
efficiency).

```
Ring layout (N = 65536 = 256MB total):

┌────┬────┬────┬────┬────┬ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┬─────┬─────┐
│  0 │  1 │  2 │  3 │  4 │                     │65534│65535│
└────┴────┴────┴────┴────┴ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┴─────┴─────┘
 4KB  4KB  4KB  4KB  4KB                         4KB   4KB

Each entry: one 4KB block, one VSF document
Write position: generation % N
Wraps: after N writes, overwrites oldest entry
Binary search: 16 reads to find newest (log2 65536 = 16)
```

---

## Unified Block Addressing

Both UFS and SD are addressed with the same block numbers. One
block = 4KB on both media. The SD card's 512-byte sector size is
abstracted away — each 4KB block write translates to 8 consecutive
SD sectors. Same block number, same offset, zero translation math.

```
Block = 4KB on both media

UFS: block N = LBA N (4KB logical blocks, native)
SD:  block N = sector N×8 through N×8+7 (8 × 512-byte sectors)

Same block numbers, same layout, same code path.
```

---

## Partition Layout

All offsets are in 4KB blocks. All boundaries are power-of-two
aligned. All copies are >256 blocks (1MB) apart.

```
Block range             Size     Purpose
────────────────────────────────────────────────────────
G#000 - G#3FF           4MB      Reserved (ABL, GPT)
G#400 - G#403           16KB     Seed copy A
G#800 - G#803           16KB     Seed copy B (4MB from A)
G#C00 - G#CFF           1MB      Stem (kernel ring, 256 entries)
G#2000 - G#11FFF        256MB    Spine (vault root ring, 65536 entries)
G#40000 - G#7FFFF       1GB      State ring (running procs, caps, display)
G#80000 - G#BFFFF       1GB      Ledger ring (categorized events)
G#C0000+                ~230GB   Tract (vault objects, plow-managed)

SD card: identical layout, identical block numbers.
No seed copies on SD (seed is in ABL boot partition).
```

Each kernel/seed copy includes its VSF signature document:
```
[binary: N bytes, 4KB-aligned]
[VSF signature: hp + ge + ke + u (see SEED.md)]
[padding to 4KB alignment]
```

---

## Spine Entry Format (VSF Document)

Every spine entry is a complete VSF document, serialized per
the VSF specification. All integers use EWE encoding — from the
kernel, from day zero. All hashes use the precise VSF type codes
from vsf_type.rs.

```
RÅ<hp(entry_hash)>
  [gen(u{generation})]               EWE, monotonic
  [prev_hash(hp{hash})]              previous entry provenance
                                     genesis: hp([0u8;32])
  [hamt_root(h{hash} u{lba})]       HAMT root node (hash + physical location)
  [plow(u{lba})]                     tract write head position
  [ledger_head(hp{hash})]            ledger chain head provenance
  [kernel_hash(hb{hash})]            kernel rolling hash (BLAKE3)
  [kernel_sig(ge{sig})]              kernel Ed25519 signature
  [eagle_time(ei{t})]                physics-bounded timestamp

Remainder of 4KB block: zeroed (reserved for future fields)
```

Cap table, process snapshots, and display state are vault objects
reachable through the HAMT root — not separate spine fields.

**EWE in the kernel:**

```
The kernel includes a minimal EWE encoder/decoder (~80 lines, no_std).
Same code as VSF, same wire format, same type tags.
No fixed-width integers anywhere on disk.
No ceilings. No migrations. No "we'll do it properly later."

Generation counter example:
  Generation 0-255:        u3 [1 byte]   = 2 bytes total
  Generation 256-65535:    u4 [2 bytes]  = 3 bytes total
  Generation 2^32+:        u5 [4 bytes]  = 5 bytes total
  Generation never:        hits a ceiling

Cost: 1 byte overhead per field for the size marker.
      20 fields × 1 byte = 20 bytes per 4KB entry = 0.5% overhead.
```

**VSF type reference (from vsf_type.rs):**
```
hp  BLAKE3 provenance hash — immutable content identity, 32 bytes
hb  BLAKE3 rolling hash — current state hash, 32 bytes
ge  Ed25519 signature — 64 bytes
ke  Ed25519 public key — 32 bytes
u   EWE unsigned integer — variable width, no ceiling
l   ASCII label — field names, schema identifiers
e   Eagle Time — physics-bounded timestamp
```

---

## Binary Search

The ring is ordered by generation number (monotonically increasing,
wrapping). Binary search finds the highest valid generation in
O(log2 N) reads.

```
Finding the most recent valid entry:

  The ring has N = 2^k entries (k=16 for N=65536).
  Generations increase monotonically and wrap: the ring always
  contains entries spanning a range of N consecutive generations
  (or fewer if < N writes have occurred).

  The "seam" is where the newest entry is adjacent to the oldest
  (the wrap point). Binary search locates this seam.

Algorithm:
  1. Read entry at position 0 and position N/2
  2. Compare generation numbers
  3. The half containing the higher generation has the seam
     (or the higher generation IS the newest)
  4. Subdivide that half, repeat
  5. After log2(N) reads: found the newest entry

  N = 65536 → 16 reads to find newest entry
  Each read: one 4KB block from UFS or SD
  Total boot I/O for vault scan: 16 × 4KB = 64KB

Validation at each read:
  VsfType::hp(BLAKE3, entry_hash) valid?
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
UFS erase block: 4MB (1 allocation unit = 1 segment)
SD erase:        FTL handles it (ERASE_BLK_EN=1)

Vault root ring: 256MB = 64 × 4MB erase blocks

DISCARD pattern:
  As the ring wraps past each 4MB-aligned boundary:
    DISCARD the 4MB region that was just fully overwritten
    FTL reclaims the underlying NAND pages
    Always 4MB-aligned to match UFS erase blocks

  No DISCARD during normal writes — only on wrap boundaries
  Sequential overwrites within each 4MB segment are FTL-optimal
```

---

## Mirror Protocol

Both UFS and SD carry identical vault root rings at identical
block addresses. Mirroring provides redundancy: either device
alone contains the complete boot state.

```
Write protocol (write-verify-then-mirror):
  1. Construct new VSF entry (generation N+1)
  2. Write to UFS at block (ring_base + generation % ring_size)
  3. Read back from UFS, BLAKE3 verify
  4. If verify fails → retry or mark UFS position bad, do not proceed
  5. UFS verified → write to SD at same block address
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
    Write-verify each copied entry
    Normal operation resumes
  If one device is missing (SD removed):
    Boot from UFS only
    When SD inserted: full ring copy from UFS to SD
    Write-verify every copied entry

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
  1. Write new kernel to slot B's block range
  2. Write-verify on UFS
  3. Write-verify on SD
  4. Write new vault root entry pointing to slot B state
  5. Write-verify vault root on both media
  6. Mark slot B as active
  7. Reboot → seed loads slot B kernel
  8. If boot succeeds: slot B becomes new production
  9. If boot fails: fall back to slot A (previous state)

Slot metadata in vault root entry:
  VsfType::l("active_slot")  → VsfType::u(0 or 1)    A or B
```

---

## Wear Distribution

```
Ring of 65536 entries, power of two
Write rotation: strict generation order, (generation % N)
Each position: written exactly once per N writes
Wear: perfectly uniform across the ring

At 1 write per second (aggressive):
  65536 seconds = ~18 hours per full rotation
  UFS rated 3000+ P/E cycles
  3000 × 18 hours = 54,000 hours = ~6 years
  SD rated 500+ P/E cycles
  500 × 18 hours = 9,000 hours = ~1 year

At 1 write per boot (typical):
  65536 boots per full rotation
  UFS: 3000 × 65536 = 196M boots — effectively unlimited
  SD: 500 × 65536 = 32M boots — effectively unlimited
```

---

## Relationship to Other Specs

```
SEED.md:
  Seed verifies kernel → jumps
  Kernel scans spine on boot
  Spine is the FIRST thing the bootloader reads

BOOT.md:
  Stage 2 of kernel boot = spine scan
  Genesis boot creates the initial ring

HAMT.md:
  Spine entry.hamt_root → HAMT root node (hash, lba)
  HAMT provides O(log32 N) object lookup
  Spine provides the HAMT root for each generation

VAULT.md:
  Spine is the entry point into the vault
  Spine entry holds HAMT root + plow position + ledger head
  Tract is the physical storage, plow-managed
  Everything in the vault is reachable from the spine

LEDGER.md:
  Spine entry.ledger_head → ledger chain head hash
  Ledger chain is restored from this pointer on boot

SECURITY_CHAIN.md:
  Seed → kernel → spine: the trust chain
  Spine entries contain kernel hash + signature
  Each generation proves the kernel that wrote it was authentic
```

---

## Formal Properties

```
Theorem VaultRoot_FindNewest:
  ∀ ring of N = 2^k entries:
    binary_search(ring) → entry with highest valid generation
    in exactly k reads (one per binary search level)
    O(log2 N) deterministic
    k=16 for N=65536: 16 reads, 64KB I/O

Theorem VaultRoot_KillswitchReady:
  ∀ kill instant t:
    at most one entry is partially written
    partial entry: VsfType::hp(BLAKE3) fails → entry invalid
    previous entry: always valid (was committed before this write)
    binary search skips invalid entries
    system always boots from last committed state

Theorem VaultRoot_MirrorRedundancy:
  ∀ single device failure:
    other device has complete vault root ring
    boot proceeds from surviving device
    resync restores mirror after replacement
    write-verify protocol ensures no silent corruption propagation

Theorem VaultRoot_WearUniformity:
  ∀ positions p1, p2 in ring:
    E[writes(p1)] = E[writes(p2)]
    strict rotation over power of two space
    mathematically uniform

Theorem VaultRoot_GenerationIntegrity:
  ∀ entry e:
    e.generation embedded in VSF document
    VsfType::hp(BLAKE3) covers generation field
    cannot forge generation without breaking BLAKE3
    binary search trusts generation only after BLAKE3 passes

Theorem VaultRoot_ReproducibleVerification:
  ∀ vault root entry e with kernel_hash field:
    anyone can: build kernel from tagged source
    compute: BLAKE3(built_binary)
    compare: e.kernel_hash == BLAKE3(built_binary)
    match proves: entry was written by kernel built from that source
```

---