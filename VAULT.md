# VAULT — ferros Persistent Object Store
**Version:** Zilor (2)
**Author:** Nick Spiker
**Principle:** Everything that persists lives in the vault. VSF is the format. The spine is the only way in.

---

## What The Vault Is

The vault is the persistent object store for ferros, and [`manifestus`](../manifestus) is the engine that implements it — one engine, host and kernel, no fork.
(The in-repo `ferros_vault` crate is the legacy implementation the kernel still links today; it is being retired — see Engine Migration below.)
The vault is not a filesystem in the Unix sense.
It has no directories, no inodes, no path strings, no mount points.
It has objects, addresses, and a HAMT that makes them findable.

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
│        reap →  [occupied: live+dead mix]  → plow             │
│        plow →  [clean: zero live blocks]  → reap (wrapped)   │
│                                                              │
│  plow = append head. Writes land here, blind, contiguous.    │
│  reap = cleaning head, trailing the plow by ≤ one lap.       │
│  occupied = [reap, plow)   clean = [plow, reap + len)        │
│  INVARIANT: the clean region contains zero live blocks.      │
│  no block allocator, no free list; new data never fragments  │
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
(block G#C0000 onward — approximately 230GB on UFS). It carries two
cursors in the same monotone domain: the **plow** (append head) and the
**reap** (cleaning head), with `reap ≤ plow ≤ reap + len`. The region
between reap and plow is occupied — a mix of live blocks and dead ones
(deleted, overwritten, orphaned). The region from plow around to reap
is clean: it contains no live blocks, by invariant, so appends write
into it blindly — no read, no classification, no relocation on the
write path. Reclamation is a separate, windowed activity at the reap
(see The Reap below).

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

The plow is the sole write mechanism for the tract, and it only ever
appends.

```
New object write:
  Write to tract at plow → read back → BLAKE3 verify on UFS
  Write same bytes to SD  → read back → BLAKE3 verify on SD
  Both verified → advance plow

The clean-region invariant makes the write path trivial:
  every position in [plow, reap + len) holds nothing live,
  so a write is a write — no read-before, no classify,
  no relocate, no reserved slots. Multi-block values land
  as one contiguous run (split at most once by ring wrap).
```

Because appends are contiguous, a value's physical layout is a
handful of **runs** — (start, count) extents — not a per-block
scatter. That is what makes unbounded object sizes representable
in a single 4KB leaf (see Object Storage Modes).

### The Reap (windowed cleaning)

The reap trails the plow by at most one lap and reclaims occupied
space in bounded windows. This is the compactor, the defragmenter,
and the wear leveler, in one pass, and it is the ONLY relocation
mechanism in the engine.

```
One window (W blocks at the reap, W sized ~len/64):
  1. Scan [reap, reap + W): sealed + referenced by the live
     index → survivor; anything else (zeroed, orphaned,
     superseded, torn) → garbage.
  2. Append survivors at the plow — ordinary verified writes
     into clean space, order preserved. Source (reap window)
     and target (plow) can NEVER overlap: the target is clean
     by invariant. No staging area, no bounce buffer, and
     redundancy never drops below two verified copies.
  3. Repair references: survivors self-address (leaves carry
     their key, furrows their owner and index, nodes their
     route and depth), so each names its own repair path.
     Value runs split at window boundaries update their
     leaf's extent list; moved nodes and leaves re-anchor
     thru the COW path.
  4. Commit one generation: new HAMT root + advanced reap.
     Everything in the retired window is now provisional
     garbage; the window joins the clean region — but see
     the fence below for when it becomes writable.

Crash at any step: the committed head still references every
survivor at its ORIGINAL position (untouched — cleaning only
ever writes into clean space), so the window simply replays.
Copies whose commit never landed are orphans; a later window
reaps them.
```

Cleaning runs under space pressure (an append hitting the fence
triggers a window) and proactively (dead space > 25% of the tract
→ one window per commit), so amplification stays incremental and
bounded; nothing ever stops the world.

Wear leveling is a free side effect — appends rotate uniformly,
and the reap forcibly migrates even never-rewritten cold data once
per lap, so no position can sit out the rotation.

### The Rollback Fence

The fence keeps the last K generations fully restorable, expressed
as one integer compare in the monotone domain:

```
fence = min over the last K spine entries of (reap_i + len_i)
appends allowed while plow < fence
```

Why this is sufficient: appends only land in space that was
already clean at every generation still in the window, and clean
space contains nothing those generations reference (the invariant
is re-established by each cleaning commit: survivors are re-homed
in the SAME generation that advances the reap). A freshly retired
window is therefore quarantined automatically — it becomes
writable only once every generation that could reference its
corpses has aged out of the K-window.

Heartbeat generations (commits that re-assert the head's root,
reap, and geometry under a new generation number) slide older
entries out of the window when the tract is too tight to make
progress; they can never raise the fence past what the committed
head survives, because they only ever repeat committed values.

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

Two modes, determined by object size — Lone (inline) and Extent
(run list, any size). Each is a HAMT leaf format; see HAMT.md for
node encoding details.

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

### Extent (run list in leaf, any size)

Object is stored as furrows (extent data blocks) in the tract. Because
the plow only appends into clean space, a fresh value's furrows are one
contiguous run (two if the ring wraps mid-value). The HAMT leaf records
**runs** — (start, count) pairs — not per-block LBAs:

```
RÅ<hp(provenance) hb(content_hash)>
  [d("vault.extent")]
  [size(u{total_bytes})]
  [s(u{run_start}) c(u{run_count})]     ← repeated per run
```

Fragmentation is bounded by construction, not by luck: only the reap
splits runs (a cleaning window moves the in-window portion of a value
and leaves the rest, adding at most two boundary fragments per window
crossed), and consecutive windows re-append a value's blocks in order,
so fragments coalesce as the reap passes. With ~190 run slots in a
leaf and windows sized ~len/64, the representable object size exceeds
the tract itself — the leaf declares ANY size in one 4KB block.

There is no chained mode and no extent-node indirection: runs made
the pointer count logarithmic in fragmentation instead of linear in
size, so one leaf suffices. (The former per-LBA "direct" leaf format
remains decodable for migration; the reap rewrites such values into
extent form the first time it touches them.)

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
  [plow(u{monotone_total})]        append head
  [reap(u{monotone_total})]        cleaning head — fence input
  [ledger_head(hp{hash})]
  [kernel_hash(hb{hash})]
  [kernel_sig(ge{sig})]
  [eagle_time(ei{t})]
```

An entry missing the reap field (pre-extent format) contributes the
maximally restrictive fence value (`plow_i - len_i`, i.e. zero append
budget); K subsequent generations age it out — old vaults migrate
themselves thru ordinary cleaning, no format break, no migration tool.

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
  Reap window reaches the zeroed block
  Not sealed → garbage → left behind, space retired with the window
  Stale HAMT pointers resolve to None on lookup (zeroed = deleted)

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

Theorem Vault_CleanInvariant:
  ∀ committed generation g:
    ∀ position p ∈ clean(g) = [plow_g, reap_g + len_g):
      no block referenced by g's HAMT lives at p
  Established at genesis (all clean), preserved by appends
  (they only add references INTO clean space at the plow) and
  by cleaning commits (survivors re-homed in the same generation
  that retires their window).

Theorem Vault_ReapLiveness:
  ∀ block b in a reap window:
    b sealed ∧ live_index[lba(b)] == hp(b) → survivor, re-append + repair
    b sealed ∧ live_index[lba(b)] ≠ hp(b) → garbage (orphan/superseded)
    b zeroed                               → deleted, garbage
  Survivors keep their originals intact until the window commit
  lands; a crashed window replays from the originals.

Theorem Vault_RollbackFence:
  ∀ generation i within the last K:
    appends allowed only while plow < reap_i + len_i
    → appends land only in space clean AT generation i
    → (by Vault_CleanInvariant) nothing i references is overwritten
    → generation i fully restorable until it exits the window

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
    appends rotate uniformly; the reap migrates even cold,
    never-rewritten data once per lap — no position sits out
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

manifestus README (../manifestus):
  The engine's own doc: commit-object wire format, threat model,
  redundancy vs crash-proofness, kill-harness methodology
  This spec is the contract; that README is the implementation record
```

---

## Engine Migration — ferros_vault → manifestus

**The decision:** the kernel swaps off the in-repo `ferros_vault` crate and onto `manifestus`, and `ferros_vault` is retired at the swap.
Today the kernel links `ferros_vault` (`path = ../vault`, non-optional); all recent engine work — reap, extent leaves, kill harnesses, bad-block relocation — lives in the standalone `manifestus` repo and is not what the kernel compiles.
This section is the record that the divergence is known and the direction is chosen, so nobody "fixes" the old crate.

**Why manifestus wins:**
- One engine, kill-tested where it is cheap to test. The commit protocol is proven by exhaustive kill -9 harnesses on the host; the kernel inherits the proof instead of re-earning it on hardware.
- No size ceilings. Extent leaves declare any-size values in one 4KB block (runs, not per-block pointers), so the old lone/direct/chained mode distinctions collapse.
- A proper hash-mapped store. 32-way COW HAMT keyed on the content hash — `put`, `get`, `delete`, nothing else.

**The shape of the work:**
1. `manifestus` core goes `no_std` — the engine already sees only `read(lba)`, `write(lba)`, `flush()`, so this is feature-gating the host conveniences (FileDev, std collections), not a redesign.
2. The kernel supplies `BlockDev` over `ferros_hal::ufs` / `sdmmc` — block I/O and ring binary search already work there.
3. **LiveSet retirement** (self-address liveness, O(1) resume) — the current host LiveSet is an in-memory map rebuilt on open; the kernel needs liveness derived from the blocks' own self-addressing. Required before large tracts are practical, and it is the one piece that is new design rather than porting.

**The blob-access rethink (open design, direction chosen):**
`ferros_vault`'s access patterns assumed objects small enough to materialize whole.
With extent leaves there is literally no size limit, so the kernel API cannot be `get(key) -> Vec<u8>` — a value can exceed RAM.
The run list makes the fix natural: runs are (start, count) pairs, so byte-offset → LBA is arithmetic over at most ~190 runs, and random access into a value costs one leaf read plus the target furrows.
So the kernel-side surface becomes `get_range(key, offset, len)` (plus a streaming iterator built on it), with whole-object `get` kept as the degenerate case for lone objects.
Furrow-level `hb` hashes mean a partial read is still integrity-checked per block without hashing the whole value.

**The event sink (errors and logs):**
The engine emits events through an injected sink trait, not a logger — manifestus core carries no `log` crate, no `std::io`, no destination knowledge.
The sink is the fourth, outbound leg of the engine's contract: `read(lba)`, `write(lba)`, `flush()`, `emit(event)`.
Each embedder picks the destination:
- **Photon (host):** `log.vsf` — the host-profile stand-in for the ledger, same VSF event schema.
- **ferros kernel:** `Ledger::Vault::Repair` / `Ledger::Vault::Reap` (see [LEDGER.md](LEDGER.md)) for survivable faults — notify-always, because the repair *rate* is the flash-death early warning.
- **Engine-fatal faults** (cannot commit, both mirrors failing) never route through the ledger — the ledger is a tenant of the vault, and the vault cannot record its own death. They go to the RAM diag ring (PT DIAG) + framebuffer, pstore/ramoops on warm reboot. The watchdog is never the thing it watches.

**What does not change at the swap:** VSF sealing, spine commit semantics, the rollback fence, dual-mirror write-verify, the capability layers above the store, and the on-disk format — manifestus already implements this spec; the kernel is catching up to it, not the reverse.

---

## Implementation Status

```
manifestus (host profile):  the engine, complete and kill-tested
  Two-cursor tract (plow + reap), blind appends, windowed cleaning
  Extent leaves (any-size values), legacy direct decode + self-migration
  Rollback fence min(reap_i + len_i), heartbeat generations
  Grow (fallocate-first, geometry-second), dual-mirror write-verify
  Migrating rings (root ring, fixed residency, A/B ordering)
  Bad-block relocation design (relocate-don't-repair)
  62 tests / 9 suites, four kill -9 harnesses
  Consumers: Photon (kete/FlatStorage), Cairn

ferros kernel profile:
  ferros_vault (legacy crate):  what the kernel links TODAY — frozen,
    retired at the swap; do not extend it
  ferros_hal::ufs / sdmmc / ring:  block I/O + binary search working
  Engine swap (manifestus no_std core + HAL BlockDev):  next
  LiveSet retirement (self-address liveness, O(1) resume):  next —
    prerequisite of the swap at scale
  Blob access rework (get_range + streaming over run lists):  with swap
  Encryption at rest, access() sections, namespaces:  future
  Cross-device vault sync:  post-networking
```

---
