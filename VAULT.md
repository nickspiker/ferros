# VAULT — ferros Persistent Object Store
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Everything that persists lives in the vault. VSF is the format. The vault root is the only way in.

---

## What The Vault Is

`ferros_vault` is the persistent object store for ferros. It is not
a filesystem in the Unix sense. It has no directories, no inodes,
no path strings, no mount points. It has objects, addresses, and
a HAMT that makes them findable.

```
Not this:   /home/user/documents/file.txt
            path string → inode → blocks

This:       VsfType::h(BLAKE3, object_hash) → vault object
            possession of the hash = the address
            HAMT root in vault root entry = how you find current state
            the object's BLAKE3 hash = its identity and integrity proof
```

---

## What Lives In The Vault

```
Boot snapshots:     HAMT root, cap table, process entry points
                    indexed via vault root ring (see VAULT_ROOT.md)
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
Kernel code:        separate signed partition, not a vault concern
Ephemeral IPC:      in-memory, dies on kill, not persisted
Display buffers:    VSF compositor manages its own memory
Logs as files:      ferros_ledger is the log, not vault objects
```

---

## Object Model

Every vault object is a VSF document. No exceptions.

```
Native VSF object:
  VsfType::hp(content_hash)          ← BLAKE3 provenance hash (immutable identity)
  content: VSF fields                ← the actual object

Non-VSF wrapped object:
  VsfType::hp(plaintext_hash)        ← provenance of plaintext content
  VsfType::hb(cipher_hash)           ← rolling hash of encrypted form
  VsfType::ge(signature)             ← Ed25519 signature (proves origin)
  VsfType::v(b'e', encrypted_bytes)  ← ChaCha20 encrypted content

  Three checks prove:
    hp: content identity matches expected provenance
    hb: ciphertext has not been tampered with
    ge: object was written by a key holder (not forged)
```

Object address = BLAKE3 hash of content (provenance hash). Content-addressed storage.
The address IS the integrity proof. You cannot have the address
of a corrupt object — the hash would not match.

---

## Capability Gating

Every vault operation requires a capability:

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
  Vault root ring: kernel-only, no userspace cap
  Only the flash tool (ferros-mkimg via fastboot) can write the seed
  See SECURITY_CHAIN.md for trust model
```

No capability = object does not exist from your perspective.
Not access denied. Does not exist. Same rule as everywhere in ferros.

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                  VAULT SERVER                        │
│              (userspace, cap-gated IPC)              │
│                                                      │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────┐  │
│  │  Namespace  │  │  Object      │  │  Storage   │  │
│  │  Registry   │  │  Store       │  │  Backend   │  │
│  │  (cap tree) │  │  (HAMT)      │  │  (mirrored)│  │
│  └─────────────┘  └──────────────┘  └────────────┘  │
└──────────────────────────────────────────────────────┘
                      │ cap-gated IPC only
       ┌──────────────┼──────────────┐
       ▼              ▼              ▼
  Kernel (boot)   Ledger server   App processes
  Cap<Write,      Cap<Write,      Cap<Write,
  Vault::Boot>    Vault::Ledger>  Vault::App::<hash>>
```

Kernel first stage (bootloader) reads the vault root directly
during boot — before the vault server exists. After boot, the
vault server owns all vault access via cap-gated IPC.

---

## Storage Layout

```
Physical storage (each device — UFS and SD mirror):

┌──────────────────────────────┐
│  Seed copies A + B           │  ← 2 copies, >= 1MB apart
│  Kernel copies A + B         │  ← 2 copies, >= 1MB apart
│  (see SECURITY_CHAIN.md)     │  ← Ed25519 signed, BLAKE3 verified
├──────────────────────────────┤
│  Vault Root Ring             │  ← 4MB (1024 × 4KB entries)
│  (see VAULT_ROOT.md)         │  ← generation-ordered, binary search
│  VSF documents, BLAKE3       │  ← write-verify-then-mirror
├──────────────────────────────┤
│  Userspace Ledger Ring       │  ← 1GB (spec only, future)
│  (see LEDGER.md)             │  ← structured events, 4KB writes
├──────────────────────────────┤
│  Userspace Log Ring          │  ← 1GB (spec only, future)
│                              │  ← verbose events, batched
├──────────────────────────────┤
│  Vault Object Store          │  ← remainder of device
│  HAMT-indexed (see HAMT.md)  │  ← content-addressed, CoW
│  Every block: VSF document   │  ← BLAKE3 integrity by construction
│  Every block: encrypted      │  ← ChaCha20 at rest
└──────────────────────────────┘

Mirror protocol:
  Write to UFS → read back → BLAKE3 verify
  Then write to SD → read back → BLAKE3 verify
  Both verified → committed
  See VAULT_ROOT.md for full mirror protocol

Hardware:
  UFS: 232GB, 4KB blocks, 4MB erase blocks, full wear leveling
  SD:  953GB, 512B blocks (4KB aligned writes), FTL wear leveling
  Common write unit: 4KB (natural for both)
```

---

## Object Indexing (HAMT)

The vault uses a Hash Array Mapped Trie for object lookup.
See HAMT.md for the full specification.

```
Vault root entry → hamt_root hash → HAMT root node
HAMT lookup: provenance_hash → current block hash
  O(log32 N): 4-6 node reads for any realistic dataset
  COW by construction: edit produces new root, old intact

HAMT nodes are themselves vault objects:
  VSF documents, BLAKE3 content-addressed
  Stored in the vault object store
  The index indexes itself

No separate index partition. No special block region.
```

---

## Kill Safety

```
Write path:
  Object constructed as VSF document
  BLAKE3 computed (mandatory, automatic)
  Write to UFS → verify → write to SD → verify
  HAMT path updated (COW: new nodes, old nodes untouched)
  New HAMT root hash written to vault root ring entry
  Generation counter incremented

Kill fires mid-write:
  Partial VSF document: mandatory hash fails → discarded
  Partial HAMT update: old HAMT root still valid (COW)
  Partial vault root entry: BLAKE3 fails → previous entry valid
  Partial mirror write: at least one device has the previous state

Recovery:
  Boot → vault root binary search → highest valid generation
  HAMT root → all objects committed before kill: intact
  In-flight object: absent, not partial
  Ledger chain: valid through last committed entry
```

---

## Relationship to Other Specs

```
SECURITY_CHAIN.md:
  Seed verifies kernel → kernel scans vault root
  Vault root is the FIRST thing the bootloader reads
  Protected regions: no cap for seed or vault root ring

VAULT_ROOT.md:
  The ring of boot state snapshots
  Each entry points to an HAMT root + cap snapshot + ledger head
  Binary search finds highest valid generation

HAMT.md:
  The object index
  Provenance hash → current block location
  COW: every edit produces new root, old versions intact

LEDGER.md:
  The event log
  Tenant of the vault (chain entries are vault objects)
  ledger_head in vault root entry → chain restoration on boot

ARCHITECTURE.md:
  Why this design instead of Linux/Unix patterns
  Structural elimination of vulnerability classes
```

---

## Encryption

VSF handles encryption. The vault server does not implement crypto.

```
At rest: all vault objects encrypted with boot_key
  boot_key: ChaCha20(device_key_in_CSR, boot_nonce)
  per-boot fresh, never written to RAM

VSF encrypted object:
  content encrypted: ChaCha20(boot_key, object_nonce)
  pre-encryption hash: VsfType::h(BLAKE3, plaintext_hash)
  post-encryption hash: VsfType::h(BLAKE3, ciphertext_hash)
  nonce: VsfType::u(n) EWE, per-object, never reused

Nothing hits storage without encryption.
Nothing hits storage without a BLAKE3 hash.
Both properties: structural, not policy.
```

---

## Formal Properties

```
Theorem Vault_ObjectIntegrity:
  ∀ vault object o:
    address(o) = BLAKE3(content(o))
    corrupt(o) → address(o) ≠ BLAKE3(corrupted_content)
    → object excluded from store

  Corollary: you cannot hold the address of a corrupt object

Theorem Vault_CapabilityConfinement:
  ∀ process p without Cap<_, Vault::Namespace::N>:
    objects in N do not exist from p's perspective
    not access denied — not addressable

Theorem Vault_KillSafety:
  ∀ kill instant t:
    ∀ object o where o.committed_at < t:
      o recoverable via vault root binary search ∧
      BLAKE3(o) valid
    ∀ in-flight object f at t:
      f either fully committed or fully absent
      partial f: VSF mandatory hash fails → discarded

Theorem Vault_TenantIsolation:
  ∀ namespaces N1 ≠ N2:
    compromise(N1) → objects in N2 unaffected
    Cap<Write, N1> grants nothing in N2
    ferros_ledger compromise → vault boot snapshots unaffected
    app compromise → other app objects unaffected

Theorem Vault_MirrorRedundancy:
  ∀ single device failure:
    other device has complete vault state
    boot proceeds from surviving device
    resync restores mirror after replacement
```

---

## Implementation Status

```
ferros_vault crate: exists (persistent object store skeleton)
ferros_hal::ufs:   UFS probe working (geometry, NOP, descriptors)
ferros_hal::sdmmc: SD card R/W working (4-bit, 400KHz, multi-block)

Working now:
  UFS controller probe (UFSHCI v3.0, link up, NOP verified)
  SD card identification + block R/W + CSD decode
  Hot-reload over USB (development iteration)

Next:
  UFS SCSI READ(10)/WRITE(10) block I/O
  Vault root ring implementation (binary search)
  Seed signature verification (Ed25519 + BLAKE3)
  HAMT implementation

Future:
  Vault server (userspace, cap-gated IPC)
  Full namespace registry
  Garbage collection of old HAMT nodes
  Userspace ledger ring (1GB)
  Cross-device vault sync (post-networking)
```

---

## What Can Wait

```
- Vault server in userspace (kernel-direct access fine for bootstrap)
- Full namespace admin
- Garbage collection of old object versions
- Userspace ledger ring (spec only for now)
- Cross-device vault sync (post-networking)
- App namespace provisioning (post-cap system)
```

---

*VAULT 0 — Everything that persists. Nothing that shouldn't.*
*Author: Nick Spiker*
