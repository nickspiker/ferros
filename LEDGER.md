# LEDGER — ferros Logging Specification
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Facts are immutable. VSF is the format. The chain does not lie.

---

## Philosophy

A log is not a debugging convenience. It is a **formal record of system events** — capability-gated, cryptographically ordered, append-only, and killswitch-ready by construction.

VSF is not a serialization layer bolted onto the Ledger. VSF **is** the Ledger. Every entry is a VSF document. Native VSF on disk stays native. Non-VSF data gets wrapped. The filesystem is eventually VSF topology — the Ledger is its first expression.

```
Traditional logging:  invent a format, hope it's big enough,
                      add auth later, add encryption later,
                      add integrity later, regret everything

Ledger:               VSF from entry zero
                      EWE integers — no ceiling, ever
                      BLAKE3 chain — VsfType::h, first-class
                      encryption at rest — VSF handles it
                      capability-gated — possession is authorization
                      kill-safe — Ring FS backs it, BLAKE3 proves it
```

No artificial limits. No retrofitted integrity. No format migration. VSF was designed for exactly this.

---

## Core Properties

```
VSF-native:         every entry is a VSF document
                    on-disk format = wire format = in-memory format
                    no translation layer, no impedance mismatch

Append-only:        entries cannot be modified or deleted
                    VSF mandatory hash catches any attempt

Content-addressed:  each entry identified by VsfType::hp(BLAKE3, hash)
                    the provinence hash IS the address, no separate index needed

Chained:            each entry's prev_hash commits to predecessor
                    VsfType::hp(BLAKE3, prev_entry_hash)
                    tampering breaks chain, detectable by construction

Cap-gated:          Cap<Write, Ledger::Category> to append
                    Cap<Read, Ledger::Category> to query
                    categories are capability-scoped, not ACL-scoped

Killswitch ready:          Ring FS backed, atomic writes, generation-numbered
                    partial write → VSF mandatory hash fails → discard
                    last committed entry always recoverable

Identity-bound:     VsfType::g(Ed25519, writer_signature)
                    writer capability encodes identity
                    cannot forge entry for category you don't hold

Unbounded:          sequence counter: VsfType::u(n) — EWE encoded
                    grows as needed, never hits ceiling
                    same for every counter, every size field, everywhere
```

---

## VSF as the Filesystem

The Ring FS is eventually VSF topology. Every block on disk is either:

```
Native VSF:    stored as-is, parsed directly
               integrity: VSF mandatory BLAKE3, always computed
               encryption: VSF handles at rest

Non-VSF:       wrapped in VSF envelope
               VsfType::b (binary blob) + VsfType::hb (integrity)
               becomes first-class VSF citizen
               same integrity guarantees as native

Result:        no block on disk lacks integrity proof (hb or g)
               no block lacks a VSF type
               the FS IS the VSF document tree

Fragmenting and updates: Content is split into disk friendly chunks (4KB on Fairphone 5 and SanDisk SD) by Photon Transport type chunking. VSF also allows many empty sections to simplify fragmentation management.
```

The Ledger is the first part of that tree to exist. Boot the Ledger, you have the seed of the FS.

---

## Entry Format

Every Ledger entry is a VSF document. No exceptions.

```
VSF document structure:

Header:
  VsfType::h(BLAKE3, document_hash)    ← mandatory, auto-computed
  VsfType::l("ferros.ledger")          ← schema identifier

Identity section ("ledger.identity"):
  VsfType::l("category")    → VsfType::l(category_path)
  VsfType::l("writer_cap")  → VsfType::h(BLAKE3, cap_token_hash)
  VsfType::l("writer_sig")  → VsfType::g(Ed25519, entry_signature)
                              proves writer held cap at write time

Ordering section ("ledger.order"):
  VsfType::l("global_seq")  → VsfType::u(n)     EWE, grows as needed
  VsfType::l("cat_seq")     → VsfType::u(n)     EWE, per category
  VsfType::l("eagle_time")  → EtType::ei(t)     physics-bounded timestamp
  VsfType::l("prev_hash")   → VsfType::hp(BLAKE3, prev_entry_provinence_hash)
                              genesis: VsfType::hp(BLAKE3, 0)

Payload section ("ledger.payload"):
  VsfType::l("level")       → VsfType::u(level)
  VsfType::l("event")       → VSF-typed structured event (see below)
                              never a free-form string in production
```

**Why EWE for sequence numbers (see VSF variable width encoding):**

```
Entry 0-255:        'u' '3' + 1 byte   = 3 bytes total
Entry 256-65535:    'u' '4' + 2 bytes  = 4 bytes total
Entry 2^32+:        'u' '5' + 4 bytes  = 6 bytes total
Entry 2^64+:        'u' '6' + 8 bytes  = 10 bytes total
Entry 2^256+:       'u' '8' + 32 bytes = 34 bytes total
Entry heat-death and change:   'u' 'Z'            = still works!

Overhead: always 2 bytes
Ceiling:  does not exist
Migration: never needed
```

---

## Chain Integrity

Each category maintains an independent signed BLAKE3 hash chain, expressed entirely in VSF types:

```
Entry 0 (genesis):
  prev_sig    = VsfType::g(sign(BLAKE3([0])))
  document_sig = VSF signature over entire document

Entry N:
  prev_sig    = VsfType::g(sign(BLAKE3(document_sig(Entry N-1))))
  document_sig = VSF signature over entire document

VSF signature:
  computed automatically on every write
  cannot be stripped (removes field, breaks document structure)
  cannot be faked (BLAKE3 preimage resistance)
  verified automatically on every read
```

**Tamper detection:**

```
Delete entry K:      chain breaks at K+1 (prev_sig mismatch)
Modify entry K:      VSF signature fails immediately
Insert entry:        cat_seq gap detected + chain break
Reorder entries:     cat_seq + eagle_time ordering violated
Forge entry:         writer_sig verification fails
                     writer_cap sig doesn't match held cap
Any corruption:      VSF signature catches it before chain check
                     two independent integrity layers
```

---

## Category Tree

```
Ledger (root)
├── Ledger::Kernel
│   ├── Ledger::Kernel::Boot
│   ├── Ledger::Kernel::IPC
│   ├── Ledger::Kernel::Memory
│   ├── Ledger::Kernel::Capability
│   └── Ledger::Kernel::Kill
├── Ledger::USB
│   ├── Ledger::USB::PHY
│   ├── Ledger::USB::DWC3
│   ├── Ledger::USB::Enumeration
│   └── Ledger::USB::PT
├── Ledger::Photon
│   ├── Ledger::Photon::Transport
│   ├── Ledger::Photon::TOKEN
│   └── Ledger::Photon::Messages
├── Ledger::TOKEN
│   ├── Ledger::TOKEN::Attestation
│   └── Ledger::TOKEN::Auth
├── Ledger::RingFS
│   ├── Ledger::RingFS::Boot
│   ├── Ledger::RingFS::Write
│   └── Ledger::RingFS::Repair
├── Ledger::VSF
└── Ledger::App::<cap_hash>     per-app, minted at install
                                identified by app cap hash, not name
```

Category caps follow standard ferros capability rules:

```
Cap<Write, Ledger::USB::PT>     append to USB PT category
Cap<Read,  Ledger::USB::PT>     query USB PT category
Cap<Admin, Ledger::USB>         mint subcategory caps under USB

Mint rules: same as all ferros caps
  Can narrow rights downward
  Cannot expand rights upward
  Cannot mint caps for categories not held as admin
  Ledger::USB admin cannot mint Ledger::Kernel caps
  Pulling higher level caps revokes all children
```

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                  LEDGER SERVER                      │
│           (userspace — capability-gated)            │
│                                                     │
│  ┌──────────────┐ ┌─────────────┐ ┌───────────────┐  │
│  │  Category    │ │   Chain     │ │   Ring FS     │ │
│  │  Registry    │ │  Validator  │ │   Backend     │ │
│  │  (cap tree)  │ │ (VSF hash)  │ │ (VSF on disk) │ │
│  └──────────────┘ └─────────────┘ └───────────────┘  │
└─────────────────────────┬────────────────────────────┘
                          │ Cap-gated IPC only
         ┌────────────────┼────────────────┐
         ▼                ▼                ▼
    Kernel logs      USB/PT logs      Photon logs
    Cap<Write,       Cap<Write,       Cap<Write,
    Ledger::         Ledger::         Ledger::
    Kernel>          USB::PT>         Photon>
```

**Userspace, not kernel. Always.**

```
Kernel Ledger:
  Kernel grows → proof surface expands
  Logging bug → kernel panic
  Logging CVE → kernel privilege
  Violates microkernel principle

Userspace Ledger:
  Bug → server crashes, restarts, chain intact on Ring FS
  CVE → capability-bounded, can't touch what it wasn't granted
  Kernel stays minimal, stays proven
  Ledger proof is independent, doesn't touch kernel proof
  Kernel is just another client after boot — nothing special
```

---

## Boot Sequence

```
0. Bootstrap code traverses ferrosFS tree for latest valid generation
   Verifies and loads kernel code to memory:
     Once latest code is validated and loaded, processor is handed to kernel

1. Kernel boots
   Pre-Ledger buffer active:
     Fixed-size ring in BSS
     Raw VSF documents, unchained
     No cap validation (server not up yet)
     Overwrites oldest on full

2. Ring FS server online

3. Ledger server online
   Receives pre-Ledger buffer via IPC
   Establishes genesis entry:
     prev_hash = VsfType::h(BLAKE3, [0u8; 32])
   Flushes pre-boot buffer as entries 0..N
   Chain starts, cap validation starts
   Kernel receives Cap<Write, Ledger::Kernel>

4. All subsequent servers launched
   Each receives appropriate category write cap
   All events from this point: thru Ledger server
   Kernel is just another client — nothing special

5. Application install
   Ledger::App::<cap_hash> category minted
   App receives Cap<Write, Ledger::App::<its_cap_hash>>
   Cannot write to any other category
```

---

## Kill Safety

```
Write path:
  Server receives entry via Cap-gated IPC
  Validates writer cap → VsfType::g signature check
  Constructs VSF document
  VSF mandatory integrity computed automatically
  Atomic write to Ring FS (generation bump)
  Updates chain head: VsfType::g(sign(BLAKE3(new_entry_integrity)))
  Returns entry integrity to writer as receipt

Kill fires mid-write:
  Ring FS atomic write: committed or not, never partial
  VSF mandatory hash: corrupt document detected on read, discarded
  Chain: last valid entry = last committed entry
  No partial entries ever visible
  No partial VSF documents ever accepted

Worst case: lose one in-flight entry
            chain valid thru last committed entry
            all prior entries intact, provable, queryable
```

---

## Encryption at Rest

VSF handles this. Not the Ledger server. Not a separate layer.

```
VSF on disk:
  BLAKE3 hash: always present, auto-computed
  ChaCha20 encryption: document encrypted with boot_key
  boot_key: in CSR, never RAM, per-boot fresh (talk to me when we get to this part)

  Encrypted VSF document is still a VSF document
  Type markers intact (O(1) skip still works)
  Content: ciphertext
  Hash: over ciphertext (integrity of encrypted form)
```

Nothing hits disk without a VSF wrapper. Nothing in a VSF wrapper lacks integrity proof. This is not policy — it is structural.

---

## PT-Over-USB Integration

Full PT always. USB handles transport reliability. PT framing stays whole. One protocol, one proof, one Ledger category.

```
Every PT packet logged to Ledger::USB::PT:

  Receipt:
    kind:         PTPacketKind::Data
    chunk:        VsfType::u(n)
    payload_hash: VsfType::h(BLAKE3, chunk_hash)

  ACK sent:
    kind:         PTPacketKind::Ack
    chunk:        VsfType::u(n)

  Reassembly complete:
    kind:         PTPacketKind::Complete
    payload_hash: VsfType::h(BLAKE3, full_payload_hash)

Debug entries stripped in production.
In dev: full PT trace, VSF chain-verifiable.
BLAKE3 in Ledger matches PT spec BLAKE3.
One audit trail covers both transport and protocol integrity.
```

---

## Formal Properties

```
Theorem Ledger_Integrity:
  ∀ entry e in category C:
    VSF mandatory hash(e) valid ∧
    e.prev_hash = document_hash(predecessor(e))

  Proof: VSF mandatory hash auto-computed, cannot be stripped,
         BLAKE3 preimage resistance: 2^-256 forgery probability

Theorem Ledger_WriterAuthenticity:
  ∀ entry e written to category C:
    VsfType::g(Ed25519, e.writer_sig) valid under writer's key ∧
    e.writer_cap_hash = BLAKE3(cap token held at write time)

  Corollary: cannot forge entry for category without held cap

Theorem Ledger_KillSafety:
  ∀ kill instant t:
    ∀ entry e where e.committed_at < t:
      e recoverable from Ring FS ∧
      VSF mandatory hash valid ∧
      chain intact thru e
    ∀ in-flight entry f at t:
      f either fully committed or fully absent
      VSF partial document: mandatory hash fails, discarded

Theorem Ledger_CategoryIsolation:
  ∀ categories C1 ≠ C2:
    compromise(C1) → chain(C2) unaffected
    Cap<Write, C1> grants nothing in C2

Theorem Ledger_UnboundedOrdering:
  ∀ sequence counter s:
    s encoded as VsfType::u(n) — EWE
    no ceiling exists in any physically realizable deployment
    2 bytes overhead regardless of magnitude
    universe runs out of atoms before VSF runs out of sequence space
```

---

## Day 0 Requirements (Non-Negotiable)

```
1. VSF from entry zero
   No "we'll add proper format later"
   Every entry: VSF document, mandatory hash, always

2. BLAKE3 chain from entry zero
   Genesis: prev_hash = VsfType::h(BLAKE3, [0u8; 32])
   Every entry chains to predecessor
   No unchained entries, ever

3. EWE sequence numbers
   VsfType::u(n) — not u64, not u32, not any fixed width
   Starts at u3 (8-bit), grows automatically
   Never hits a ceiling

4. Structured payloads only
   Define VSF sections for events needed now
   Add sections as needed
   VsfType::x strings: dev builds only, stripped at production
   Never add free-form string to production payload

5. writer_cap_hash and writer_sig populated from day zero
   Validation tightens over time
   Absent fields cannot be added retroactively

6. Eagle Time timestamps
   EtType::f6(t) — not wall clock, not uptime counter
   Physics-bounded, monotonic, unforgeable
```

---

## What Can Wait

```
- Ring FS persistence (in-memory VSF chain fine for now)
- Read caps and query interface (write path first)
- Category admin UI (hardcode initial categories)
- Retention and garbage collection
- Cross-device log sync (post-networking)
- Full encryption at rest (VSF structure ready, key integration later)
```

---

*Author: Nick Spiker*