# VAULT — ferros Persistent Object Store
**Version:** Zila (1)
**Author:** Nick Spiker
**Principle:** Everything that persists lives in the vault. VSF is the format. The spine is the only way in.

---

## What The Vault Is

`ferros_vault` is the persistent object store for ferros. It is not
a filesystem in the Unix sense. It has no directories, no inodes,
no path strings, no mount points. It has objects, addresses, and
a HAMT that makes them findable.

```
Not this:   /home/user/documents/file.txt
            path string → inode → blocks

This:       hp(BLAKE3, object_hash) → vault object
            possession of the hash = the address
            HAMT root in spine entry = how you find current state
            the object's BLAKE3 hash = its identity and integrity proof
```

---

## What Lives In The Vault

```
Boot snapshots:     HAMT root, cap table, process entry points
                    indexed via spine (see RING.md)
                    restored on boot by kernel first stage

Ledger backing:     ferros_ledger's chain storage
                    Ledger is a tenant of the vault, not the vault

App data:           persistent application state
                    each app holds Cap<Write, Vault::App::<hash>>
                    cannot address other app's objects

Keys (encrypted):   wrapped in VSF ChaCha20 envelope
                    actual key material: always in CSR, never here
                    key metadata, wrapped derivation material only

Capability state:   CSpace snapshots, part of boot snapshot
                    not separately addressable

VSF objects:        anything else that needs to persist
                    native VSF: stored as-is
                    non-VSF: wrapped, integrity-guaranteed
```

---

## What Does Not Live In The Vault

```
Keys in plaintext:  never. CSR only. Always.
Seed binary:        protected partition, no cap issued for R/W
Kernel code:        stem (kernel ring), not a vault concern
Ephemeral IPC:      in-memory, dies on kill, not persisted
Display buffers:    VSF compositor manages its own memory
Logs as files:      ferros_ledger is the log, not vault objects
```

---

## Object Identity

Every vault object is a VSF document. Identity, integrity, and
authorship are distinguished by three hash fields:

```
hp  provenance hash     born at creation, never changes
                        permanent identity across all edits
                        BLAKE3 of content at birth

hb  body hash           current content integrity
                        changes when content changes
                        BLAKE3 of content as-is

ge  signature           Ed25519, proves authorship
                        changes if re-signed

Rules:
  hp alone       →  identity AND integrity (immutable objects)
  hp + hb        →  hp = identity, hb = integrity
  hp + hb + ge   →  identity + integrity + authorship
```

Object address = provenance hash. Content-addressed storage.
The address IS the integrity proof for immutable objects.
You cannot hold the address of a corrupt object — the hash
would not match.

---

## Storage Architecture

The vault's physical storage is organized into two layers:
the **tract** and the **HAMT**.

```
┌──────────────────────────────────────────────────────────────┐
│                         TRACT                                │
│        the physical ring where all blocks live                │
│                                                              │
│  ← plow advances this way ←                                 │
│                                                              │
│  [live][dead][dead][live][live][dead][new][new][new]→plow    │
│                                                              │
│  plow = write head, advances and wraps                       │
│  live blocks = relocate to plow as it approaches             │
│  dead blocks = trample, free                                 │
│  no block allocator, no free list, no fragmentation          │
│  free space = [plow, plow-1 wrapped], always contiguous      │
├──────────────────────────────────────────────────────────────┤
│                         HAMT                                 │
│        the logical index, lives INSIDE the tract             │
│                                                              │
│  hp(provenance) → (hash, lba) for any object                │
│  32-way branching, 5 bits per hash level                     │
│  COW: every edit → new root, old intact                      │
│  4-6 reads for any realistic dataset                         │
│  See HAMT.md for full specification                          │
└──────────────────────────────────────────────────────────────┘
```

The **tract** is a log-structured ring covering the entire HAMT region
(block G#C0000 onward — approximately 230GB on UFS). It has a single
write head called the **plow**. The plow advances forward with every
write, wraps around at the end, and handles garbage collection
implicitly: live blocks are relocated to the plow position, dead blocks
are trampled.

The **HAMT** (Hash Array Mapped Trie) is the object index. It maps
provenance hashes to physical block locations. HAMT nodes are themselves
blocks in the tract — they are subject to the plow just like data
objects. The index indexes itself.

The **spine** (vault root ring, 65536 entries at block G#2000) records
commit points. Each spine entry stores the current HAMT root, plow
position, and ledger head. The spine is the boot entry point —
binary search finds the latest committed generation.

---

### The Plow

The plow is the sole write mechanism for the tract.

```
New object write:
  Write to tract at plow → read back → BLAKE3 verify on UFS
  Write same bytes to SD  → read back → BLAKE3 verify on SD
  Both verified → advance plow

Plow reaches a live block:
  Always relocate: copy to current plow position, update HAMT
  Exception: plow is flush against live block → leave in place
             (it's already where it would be written)

Plow reaches a dead block (not in HAMT):
  Trample. Advance plow.

Plow reaches a zeroed block (deleted):
  Check both disks — both must be zero
  If HAMT entry still points here → remove from HAMT
  Advance plow
```

Write amplification is bounded: each object relocation writes
one data block plus ~4 HAMT node updates (COW path). Relocations
are batched into spine commits (see Batch Commits below).

Wear leveling is a free side effect — the plow rotates writes
across the entire tract uniformly.

---

### Tract Capacity

```
UFS total:            60.8M blocks × 4KB = 232GB
Fixed regions:        786,432 blocks (seeds, stem, spine, state, ledger)
Tract:                ~60M blocks ≈ 230GB

Per live object (small):
  object block:       1 block
  HAMT path:          ~4 blocks (COW: leaf + 3 internal nodes)
  total:              ~5 blocks

Live capacity:        ~12M small objects in 230GB
                      fewer large objects, proportional to size
                      HAMT overhead constant regardless of object size
```

---

## Object Storage Modes

Three modes, determined by object size. Each mode is a different
HAMT leaf format. See HAMT.md for node encoding details.

### Lone (inline, < ~3.9KB)

Object content lives directly in the HAMT leaf node. One disk read
returns both the index entry and the object.

```
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.lone")]
  [v(content)]
```

Fresh writes are always lone when possible — best read performance.
During plow rotation, lone objects may be promoted to direct
(de-inlined) to allow batch commits without HAMT churn.

### Direct (furrow LBAs in leaf, < ~4MB)

Object is stored as furrows (extent data blocks) in the tract. The
HAMT leaf holds a compact LBA list for all furrows.

```
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.direct")]
  [size(u{total_bytes})]
  [v_u(furrow_lbas[])]
```

At ~4 bytes per LBA (EWE, 26-bit addresses), approximately 1000
LBAs fit in a leaf after overhead ≈ ~4MB max object size.

### Chained (extent chain, > ~4MB)

Object exceeds what one leaf can index. The leaf points to the first
extent node. Each extent node lists up to ~1000 furrow LBAs and
optionally points to the next extent node.

```
Leaf:
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.chained")]
  [size(u{total_bytes})]
  [head(h{hash} u{lba})]

Extent node (lives in tract):
RÅ<hp(node_hash)>
  [d("vault.extent")]
  [v_u(furrow_lbas[])]
  [next(h{hash} u{lba})]          ← absent if last node
```

A 10MB photo: leaf + 3 extent nodes + ~2500 furrows.
4 reads for the full LBA list, then sequential furrow reads.

### Furrows (extent data blocks)

Each furrow is a minimal VSF document — not full-spec VSF with
named fields and sections, just enough for identity, integrity,
and position. VSF magic bytes are present so the plow's liveness
scanner has one code path for all blocks.

```
RÅ<hp(provenance) hb(block_hash)>
  [m(block_index)]
  [v(payload)]
```

~45 bytes overhead, ~4050 bytes payload per 4KB block.
Every furrow carries its own hb — corruption of any single block
in a large object is detected independently.

---

## The Spine (Commit Log)

The spine is the vault root ring (65536 entries × 4KB at block
G#2000). It records committed vault state. Each entry is a VSF
document. See RING.md for binary search mechanics, mirror protocol,
and wear analysis.

### Spine Entry Format

```
RÅ<hp(entry_hash)>
  [gen(u{generation})]
  [prev_hash(hp{hash})]
  [hamt_root(h{hash} u{lba})]
  [plow(u{lba})]
  [ledger_head(hp{hash})]
  [kernel_hash(hb{hash})]
  [kernel_sig(ge{sig})]
  [eagle_time(ei{t})]
```

The spine entry is the **transaction commit point**. Everything
written to the tract between spine entries is provisional — power
loss before a spine commit means those writes are orphaned. The
plow will trample them on the next pass.

### Batch Commits

Multiple vault writes coalesce into one spine entry. The HAMT root
in that entry reflects all writes in the batch.

```
Commit triggers (whichever fires first):
  RAM buffer full    → commit immediately, timing irrelevant
  1s timer fires     → commit whatever is pending

Low activity:   timer drives. One spine entry per second.
High activity:  capacity drives. Commit as fast as buffer fills.
```

Buffer size is the tuning knob — it sets maximum latency between
a write and its commit under load. Power of 2 (64 or 128 pending
HAMT paths, depending on available kernel RAM).

### Spine Capacity

```
65536 entries, wraps.
At 1 commit/second:                  ~18 hours of history
At 1 commit/second, batched per 5s:  ~91 days of history
```

The spine never blocks — it wraps and tramples old entries.
Current state is the latest entry. History is the ledger's job.

---

## Deletion

Delete = zero the VSF magic bytes on both disks.

```
Protocol:
  1. Zero block on UFS → read back → confirm zeros
  2. Zero block on SD  → read back → confirm zeros
  3. Both confirmed zero → deletion committed

Lookup after deletion:
  HAMT → lba → read block → no VSF magic → return None

Cleanup:
  Plow encounters zeroed block during rotation
  Check both disks — both must be zero
  If HAMT entry still points here → remove from HAMT
  Advance plow

Recovery:
  Zeroed blocks invisible to recovery scan
  Deleted objects never reappear
  No tombstones needed
```

Flash erases to zero. Writing zeros is writing "natural" state.
No HAMT COW path on delete — O(1) write to both disks.
HAMT cleanup happens naturally during plow rotation.

---

## Write Path

```
1. Construct object as VSF document
2. Write to UFS at plow → read back → BLAKE3 verify
3. Write same bytes to SD → read back → BLAKE3 verify
4. Both verified → advance plow
5. Update in-memory HAMT (COW: new leaf, new path to root)
6. Accumulate in batch buffer

When batch commits (buffer full or 1s timer):
7. Write dirty HAMT nodes to tract → verify on both disks
8. Write spine entry (new hamt_root, new plow, new gen)
   → verify on both disks
9. COMMITTED
```

SD mirrors exact UFS procedure and bit representation immediately
after UFS is in a known good committed state and verified.

---

## Recovery

```
Normal boot:
  Spine binary search → highest valid gen → hamt_root + plow
  HAMT traversal for any object. Fast. Deterministic.

Spine intact, HAMT damaged:
  Spine gives hamt_root → partial HAMT traversal
  Corrupt HAMT node → BLAKE3 fails → fall back to previous gen
  Previous spine entry has previous HAMT root → older but valid

Full recovery (spine + HAMT both damaged):
  Linear scan: read every 4KB block in tract
  Identify by VSF magic + hp field
  Reconstruct HAMT from all valid objects
  Zeroed blocks skipped → deleted objects absent by construction
  Replay ledger to re-apply any deletions
  Write reconstructed HAMT + new spine entry

Mirror recovery (one device failed):
  Boot from surviving device
  Full copy to replacement device
  Resume mirroring
```

---

## Killswitch Ready

```
Power disappears during object write (step 2-4):
  Block partially written → BLAKE3 fails → discarded
  Plow unchanged → orphaned block trampled on next pass

Power disappears during HAMT update (step 7):
  Dirty HAMT nodes partially written → BLAKE3 fails
  Previous spine entry still valid → previous HAMT root intact

Power disappears during spine write (step 8):
  Partial spine entry → BLAKE3 fails → skipped by binary search
  Previous spine entry is current → previous state intact

Result:
  Every committed write is fully durable on both disks
  Every in-flight write is fully absent
  No partial state ever visible
```

---

## Access Control

Two independent enforcement layers. Both must pass. Neither alone
is sufficient.

### Layer 1: Kernel Capabilities (fast path)

Every vault operation requires a capability. The kernel enforces
this in memory — no IPC, no crypto, just a register check.

```
Cap<Read,  Vault::Object::<hash>>   read a specific object
Cap<Write, Vault::Namespace::<hash>> write objects in a namespace
Cap<Admin, Vault::Namespace::<hash>> manage namespace, mint caps

Boot snapshot namespace:
  Cap<Write, Vault::Boot>    kernel only, at boot
  Cap<Read,  Vault::Boot>    kernel only, at boot

Ledger namespace:
  Cap<Write, Vault::Ledger>  Ledger server only

App namespace:
  Cap<Write, Vault::App::<app_cap_hash>>  app only
  Identified by app's capability hash, not app name
  Minted at install, revoked at uninstall

Protected regions (NO cap ever issued):
  Seed partition: kernel denies all read/write caps
  Spine: kernel-only, no userspace cap
  Only the flash tool (ferros-mkimg via fastboot) can write the seed
  See SECURITY_CHAIN.md for trust model
```

No capability = object does not exist from your perspective.
Not access denied. Does not exist. Same rule as everywhere in ferros.

Capabilities are the fast path. They prevent software from reaching
objects it should not touch. They do not survive physical disk access.

### Layer 2: Cryptographic Access Control (hard path)

Every vault object can carry an `access()` section — a first-class
VSF section that controls who can read or write the object using
cryptographic keys. This layer survives physical access: pulling the
UFS chip and reading it yields only ciphertext without the right
private key.

```
access() section format (inside the object's VSF document):

RÅ<hp(provenance) hb(content_hash)>
  [d("vault.lone")]
  [access()
    [admin(ke{admin_pubkey})]
    [writers()
      ke{writer_A_pubkey}
      ke{writer_B_pubkey}
    ]
    [readers()
      [wrap()
        ke{reader_pubkey}
        kx{ephemeral_pubkey}
        v{encrypted_content_key}     ← ChaCha20-Poly1305, 48 bytes
      ]
      [wrap()
        ke{reader_pubkey}
        kx{ephemeral_pubkey}
        v{encrypted_content_key}
      ]
    ]
  ]
  [v(content)]
```

**Read access:**
```
Each namespace has a symmetric content key (ChaCha20).
Key is wrapped per-reader:
  X25519(reader_pubkey, ephemeral_secret) → shared secret
  ChaCha20-Poly1305(shared, content_key)  → encrypted_content_key

Each wrap: ke(32B) + kx(32B) + v(48B) = ~116 bytes per reader
  ke  = reader's Ed25519 public key (identity)
  kx  = X25519 ephemeral public key (key exchange)
  v   = content key encrypted + Poly1305 tag

Adding a reader: wrap content key with new pubkey, append to
access section. Content blocks untouched.
```

**Write access:**
```
Writers list = public keys whose ge signatures the vault accepts.
Writing an object:
  Writer signs with Ed25519 → ge in the object
  Vault checks ge against access() writers list
  No matching ke? Write rejected.
```

**Admin:**
```
Single admin ke in access(). Only admin can modify the
access section itself — add/remove readers, writers.
Admin signs the updated access section with ge.
```

**Namespace inheritance:**
```
Namespace root object carries the access() section for the namespace.
Objects without their own access() inherit the namespace ACD.
Per-object override: object has its own access() → trumps namespace.
```

**Inline vs spill:**
```
access() inline:    fits in the object's 4KB block
                    ~20 readers before lone content shrinks too much

access() spills:    too many readers for one block
                    object gets acl(h{hash} u{lba}) pointer field
                    ACD lives in its own tract block(s)
                    same pattern as lone → direct promotion
```

**New VSF type: kx (X25519 public key)**
```
kx  X25519 public key — 32 bytes, key exchange
    same k family as ke (Ed25519, signing)
    distinct tags prevent using signing keys for exchange
    wire format: 'k' 'x' EWE(31) [32 bytes]
```

---

### Revocation

```
Soft revoke (default):
  Remove reader wrap from access() section
  Kernel caps enforce the gap immediately
  Plow re-encrypts blocks naturally during rotation
  Old content key useless once all blocks re-encrypted

Hard revoke (immediate):
  Generate new content key
  Re-wrap for all remaining readers
  Plow writes re-encrypted blocks now → verify → zero old on both disks
  Same mechanics as normal plow relocation, just triggered immediately

Writer revoke:
  Remove ke from writers list
  Existing signed objects remain valid (immutable, already committed)
  Future writes from that key rejected immediately
```

Soft and hard are the same operation at different speeds. The kernel
cap layer covers the gap between removing the wrap and re-encrypting
the content.

---

## Encryption

### Key Hierarchy

```
device_key       CSR (PAC registers), never RAM, never disk
                 5 × 128-bit: APIAKey, APIBKey, APDAKey, APDBKey, APGAKey
                 640 bits total

boot_key         ChaCha20(device_key, boot_nonce)
                 per-boot fresh, derived on startup, never persisted

namespace_key    BLAKE3(boot_key || namespace_hash)
                 per-namespace, deterministic from boot_key
                 system namespaces (Boot, Ledger, Stem) use boot_key directly

content_key      random, per-namespace (or per-object for isolation)
                 wrapped in access() section per-reader
                 X25519 DH + ChaCha20-Poly1305
```

### At Rest

```
VSF encrypted object:
  content:     ChaCha20(content_key, object_nonce) — ciphertext
  hp:          BLAKE3(plaintext at birth) — provenance, never changes
  hb:          BLAKE3(ciphertext) — integrity of encrypted form
  nonce:       u(n) EWE, per-object, never reused

System namespaces (no per-reader keys):
  Boot, Ledger, Stem → encrypted with boot_key
  Only the kernel reads these, no multi-party access

User namespaces (per-reader keys):
  App, shared → encrypted with content_key
  content_key wrapped per-reader in access() section

Nothing hits storage without encryption.
Nothing hits storage without a BLAKE3 hash.
Both properties: structural, not policy.
```

---

## Storage Layout

```
Physical storage (each device — UFS and SD mirror):

Block range             Size     Purpose
────────────────────────────────────────────────────────
G#000 - G#3FF           4MB      Reserved (ABL, GPT)
G#400 - G#403           16KB     Seed copy A
G#800 - G#803           16KB     Seed copy B (4MB from A)
G#C00 - G#CFF           1MB      Stem (kernel ring, 256 entries)
G#2000 - G#11FFF        256MB    Spine (vault root ring, 65536 entries)
G#40000 - G#7FFFF       1GB      State ring
G#80000 - G#BFFFF       1GB      Ledger ring
G#C0000+                ~230GB   Tract (vault objects, plow-managed)

SD card: identical layout, identical block numbers.

Mirror protocol:
  Write to UFS → read back → BLAKE3 verify
  Then write to SD → read back → BLAKE3 verify
  Both verified → committed
  SD mirrors exact UFS procedure and bit representation
  immediately after UFS is in a known good committed state

Hardware:
  UFS: 232GB, 4KB blocks, 4MB erase blocks, full wear leveling
  SD:  953GB, 512B blocks (4KB aligned writes), FTL wear leveling
  Common write unit: 4KB (natural for both)
```

---

## Formal Properties

```
Theorem Vault_ObjectIntegrity:
  ∀ vault object o:
    hp(o) = BLAKE3(content_at_birth(o))
    corrupt(o) → hb(o) ≠ BLAKE3(corrupted_content)
    → object excluded from store

  Corollary: you cannot hold the address of a corrupt object

Theorem Vault_PlowLiveness:
  ∀ block b at plow horizon:
    b has VSF magic ∧ HAMT[hp(b)].lba == lba(b) → live, relocate
    b has VSF magic ∧ HAMT[hp(b)].lba ≠ lba(b) → dead, trample
    b has no VSF magic (zeroed)                   → deleted, trample
    cleanup of stale HAMT entries: automatic during rotation

Theorem Vault_CapabilityConfinement:
  ∀ process p without Cap<_, Vault::Namespace::N>:
    objects in N do not exist from p's perspective
    not access denied — not addressable

Theorem Vault_KillswitchReady:
  ∀ kill instant t:
    ∀ object o where o.committed_at < t:
      o recoverable via spine binary search ∧
      BLAKE3(o) valid
    ∀ in-flight object f at t:
      f either fully committed or fully absent
      partial f: VSF mandatory hash fails → discarded

Theorem Vault_CryptoAccessControl:
  ∀ object o with access() section:
    read(o) requires possession of private key k where
      ke(public(k)) ∈ o.access.readers
    physical disk access without k → ciphertext only
    X25519 DH: 2^-128 key recovery
    ChaCha20-Poly1305: 2^-128 forgery

Theorem Vault_DualLayerEnforcement:
  ∀ vault access attempt:
    Cap check fails  → object does not exist (kernel enforced)
    Crypto check fails → object is ciphertext (math enforced)
    both must pass for plaintext access
    neither alone is sufficient

Theorem Vault_TenantIsolation:
  ∀ namespaces N1 ≠ N2:
    compromise(N1) → objects in N2 unaffected
    Cap<Write, N1> grants nothing in N2
    content_key(N1) reveals nothing about content_key(N2)
    ferros_ledger compromise → vault boot snapshots unaffected
    app compromise → other app objects unaffected

Theorem Vault_MirrorRedundancy:
  ∀ single device failure:
    other device has complete vault state
    boot proceeds from surviving device
    resync restores mirror after replacement

Theorem Vault_WearUniformity:
  ∀ positions p1, p2 in tract:
    E[writes(p1)] = E[writes(p2)]
    plow rotation: mathematically uniform
```

---

## Relationship to Other Specs

```
SECURITY_CHAIN.md:
  Seed verifies kernel → kernel scans spine
  Spine is the FIRST thing the bootloader reads
  Protected regions: no cap for seed or spine

RING.md:
  Ring mechanics: generation numbering, binary search, mirror protocol
  Spine = vault root ring instance
  Stem = kernel ring instance

HAMT.md:
  Object index: provenance hash → block location
  COW: every edit → new root, old intact
  Three leaf formats: lone, direct, chained (with optional access() section)
  Branching, collision handling, vector encoding

LEDGER.md:
  Event log: ledger_head in spine entry → chain restoration on boot
  Records vault operations: create, delete, update

ARCHITECTURE.md:
  Why this design instead of Linux/Unix patterns
  Structural elimination of vulnerability classes
```

---

## Implementation Status

```
ferros_vault crate:   exists (persistent object store skeleton)
ferros_hal::ufs:      UFS R/W working (SCSI READ/WRITE, 232GB LUN0)
ferros_hal::sdmmc:    SD card R/W working (4-bit, 400KHz, multi-block)
ferros_hal::ring:     Ring binary search working (kernel ring, vault root)

Working now:
  UFS block I/O (read, write, verify)
  SD card block I/O (read, write, verify)
  Ring binary search + write-verify
  Hot-reload over USB (development iteration)

Next:
  HAMT implementation (v_u0 bitmap, v_h/v_u vectors, COW)
  Tract with plow (log-structured gravity ring)
  Spine entry with plow field
  Lone/direct/chained leaf formats
  Deletion (zero-header, both disks)
  Batch commit logic

Future:
  Vault server (userspace, cap-gated IPC)
  Full namespace registry
  Extent chains (chained objects > 4MB)
  Encryption at rest (ChaCha20, key hierarchy)
  Per-namespace content keys, X25519 key wrapping
  access() section format, hard/soft revocation
  kx (X25519 pubkey) encoder/decoder in vsf_mini
  Cross-device vault sync (post-networking)
```

---
