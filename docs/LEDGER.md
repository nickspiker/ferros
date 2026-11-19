# LEDGER — ferros Logging Specification
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Facts are immutable. VSF is the format. The chain does not lie.

---

## Stage 0 as built: the flash ring (2026-11-17)

The flash log (ferros_kernel/src/flashlog.rs) is the ledger's first running part: every line is an entry, kept in a ring on the flash outside the vault, so it works with the vault broken.
The format, the ring and the seal are ferros_ledger (`--features ring`); the kernel gives it blocks, keys and the clock.
What the rest of this document specifies and stage 0 does not do yet is listed at the end of this section.

### Entry format (as built)

Every entry is one complete VSF document, built with `VsfBuilder` and vsf's schema builders, nothing pushed by hand:

```
RÅ< z y b l [e6: Eagle time] hp: provenance n (ledger o b n) >
[ (seq: u) (boot: u) (prev: hp) (class: u) (key: hp) (nonce: mb) (tag: mp) (sealed: vX) ]

sealed, once opened: [ (category: d) (writer: x|a) (words: x|a) (refused: u, when any) ]   section `ledger_payload`
```

**The words, lean** (owner, 2026-11-17: "lean on text"): every text field takes vsf's `x-compact` table, 712 characters (printable ASCII but `^`, tab and newline, 49 of Latin-1's 96, some of Latin Extended-A, the Russian and Ukrainian alphabets, and the commonest of several other scripts), about 10 KB in the kernel (ferros_ledger `entry::lean`):
- every character in the table: `x`, Huffman-coded;
- else, if the text is all ASCII (the table lacks `^`, which the kernel's lines use for powers of two, and most control characters): `a`, byte for byte, nothing lost;
- else each character outside the table is replaced by the marker `?` and counted: the entry's `refused` holds how many (absent when none), and the render says `(A#n characters outside the text table, shown as ?)`. A refused character is never dropped silently. A `?` that was typed and one that stands in are told apart only by the count.

- `seq`: the entry's number in the ring's one chain, from 0. `boot`: the boot that wrote it. Both VSF's auto-sized `u` (EWE): `u3` + 1 byte for small numbers, growing as needed, no fixed width anywhere.
- `prev`: the provenance hash of the entry before; the first ever names BLAKE3 of 32 zero bytes. The header's `hp` is this entry's hash, the one the next names.
- The Eagle time is the header's creation time (`e6`, oscillations). Before the device has read its clock this boot (the RTC is read at service start) the header has no time at all: VSF's way for a device that cannot know the time. Never a counter, never UNIX time.
- `class`: which key the entry is sealed under (0 system, 1 personal; the next table). `key`: that key's public id (BLAKE3 `derive_key` of a key-id context over the key), so a reader knows whether it holds the key before it tries.
- `nonce` (24 bytes, `mb`), `tag` (16 bytes, `mp`), `sealed` (`v` wrap `X`, XChaCha20-Poly1305): the seal, each in a typed field of its own.
- The seal's associated data is a `ledger_bind` section of `seq`, `boot`, `prev`, `class`, `key` and the time, so none of them can be moved onto another entry's ciphertext.
- The nonce is synthetic: keyed BLAKE3 (a subkey of the seal key) over the bound fields and the plaintext, cut to 24 bytes. No entropy is needed (an entry can be sealed before the wairua is drawn), and the same nonce only comes again for the same words under the same open fields, which the chain never repeats.
- The category, the writer and the words are sealed: on the flash only the class, the place in the chain and the time are in the clear.

The integrity is VSF's provenance hash over the whole document, then the chain over the hashes, then the AEAD over the payload: three layers, each checked on every read.

### Classes

The category decides the class (owner, 2026-11-17):

| Category | Class | What | Sealed under |
|---|---|---|---|
| `text` | system | the kernel's lines, and a process's console lines it asked to keep (`line`, `line_from`): stage 0's every existing caller | the ira's ledger key |
| `ledger` | system | the ledger's own notes: a torn last write found at open, the pre-boot buffer overrun | the ira's ledger key |
| `bench` | system | `diag "log bench"`'s timed entries | the ira's ledger key |
| `person.text` | personal | lines about the person: today the alias given to a kist (which kists they use) | the person's ledger key |
| the spec's tree, when built (stage 1): `kernel.*`, `usb.*`, `vault.*`, `ringfs.*`, `vsf` | system | boot steps, clocks, lock holds, devices, storage | the ira's ledger key |
| `photon.*`, `token.*`, `app.<cap_hash>` | personal | peers, messages, identity, kists' own entries | the person's ledger key |

- **System** entries are sealed under a key derived from the ira (`flashlog.rs` `system_key`, the one place it is made, context `ferros 2026-11-17 ledger system seal v0`): readable by this device at any boot, with the vault broken or nobody attested; unreadable off it. Never the identity's keys. The coordinator chose this root pending the owner's word; changing it is a change to `system_key` alone.
- **Personal** entries are sealed under the attested identity's ledger key (kvault `IdKeys.ledger`, from the identity root under `ferros 2026-11-17 identity ledger v0`, held sealed under the wairua, handed to the flash log at unlock and taken back at lock): readable only while that person is present.
- A personal line with no diagnostic value is not logged at all. A refusal the system has to diagnose is a system line with the person's part left out (the alias refusal names no kist).
- A personal entry made while nobody is attested is **dropped, and counted** (`diag log`'s state). Not held for the next attest: the next person may be someone else, and an entry held in RAM would then be sealed to them.

### The flash ring

- Where: the ferros partition from 1 GiB + 64 MiB in (right after the old text log), two copies of 2^14 blocks of 4 KiB: copy A at 0, its mirror B at 2^14. About 160 thousand entries a copy at today's line lengths.
  The owner approved this place on 2026-11-17 ("Ledger, yes, ring is OK"): a new on-disk location, between the old log (1 GiB) and the vault's mirrors (2 GiB), in space nothing else used.
- A block holds complete entry documents back to back; what follows the last is unwritten space (zeros), never parsed. There is no header, no index and no bookkeeping structure on the flash at all: the only things in the region are entries.
- Writing, write-verify-then-mirror: the entry goes into the head block in RAM, the whole block is written to A, read back and compared, then written to B. Only then does `line` return: an entry is on the flash, in both copies, before the call returns.
- Wrapping: an entry that does not fit in the head block starts the next block, which drops the oldest block's entries. Nothing stops when the ring is full.
- Words too long for one block go as several entries, in order.
- The head, found by the chain: blocks are written in ring order, so the first entry's `seq` rises from block 0 to the head block and drops after it. A binary search finds the head block in about log2(2^14) reads (28 block reads for 2^10 blocks a copy on the host bench, both copies read); then from its last entry the chain decides, forward for as long as the next block's first entry names it as `prev`. A block damaged in both copies on the way falls back to reading every block.
- Every entry is checked against its provenance hash, A first, B where A fails, and it must follow the one before (`seq` + 1, `prev`): every entry either copy holds whole is kept.
- A torn write: A is written whole before B is touched, so a write cut off leaves the entry damaged in one copy and absent from the other. At the end of the head block (or as the start of the block after it) that is the torn last write: named (`Torn`), skipped, and the next entry goes in its place. Nothing earlier is lost.
- Damage anywhere else is named, never passed over: the block and byte, what failed (`Corrupt`, `OneCopy`), and the gap or break it leaves in the chain (`SeqGap`, `ChainBreak`).

### The pre-boot buffer and the clock

- Nothing reaches the flash unsealed. The kernel opens the ring right after the UFS link is up (finding the head needs no key: the hashes are over the sealed documents), but sealing needs the ira, which vault boot derives a few steps later.
- What the kernel says before then waits in the pre-boot buffer (ferros_ledger `preboot`, 2^6 entries in the kernel's static memory, the oldest overwritten and counted) and goes to the RAM log as before. The moment the ira is held (`flashlog::flush` from vault boot) the buffer is written, sealed and in order; an overrun is written as a `ledger` entry.
- A boot that dies in that window (the UFS link up, the ira not yet derived) is covered by the glass console and the ramoops report, not the flash log.
- A waiting entry keeps the counter tick it was made at, in RAM only; when it is written its Eagle time is worked out from that tick if the clock has been read by then, else it carries none.
- Entries made after the ira and before the RTC is read (service start) carry no time. Their order is the chain's; their boot is in `boot`.

### Reading it

- `ferros-bridge diag log` (or `diag "log <n>"`): the newest 2^8 (or n) entries, at most a reply's room, rendered as `b<boot> <local wall time> <writer>: <words>`. The Eagle time becomes the host's local time for display only; a category other than `text` is shown in brackets; an entry made before the clock shows `(no clock yet)`.
- `--raw` writes the answer's VSF documents as they came; `ferros-bridge log-render <file>` renders a saved one.
- The answer is a run of complete VSF documents: `ledger_state` first (this boot, the next `seq`, the head block, the pre-boot buffer, personal entries dropped, writes failed, whether a person is present), then each entry as stored followed by `ledger_open` (its hash, category, writer, words) when this device holds its key now, and `ledger_finding` documents for everything found wrong, by block and byte, with a `kind` number and the words. A system entry that does not open under this device's key is a finding (`Shut`); a personal entry with nobody present is shown as sealed.
- **Until signed bridge commands exist (BRIDGE.md B2), whoever holds the USB cable reads the log opened.** The kernel opens the entries for the answer. The seal covers a chip taken off the board, not the cable. `diag log` should need the bridge right once B2 is built.

### The old text log

The 2026-10-20 stopgap (1 GiB in, 2^14 blocks, a fixed-width header and plain `b<boot> t<ms> <text>` lines) is converted once and never parsed again (AGENT.md: no compatibility reader for a text encoding):
- `ferros-bridge log-legacy <out.vsf>` reads its raw blocks (`diag "log legacy <first> <count>"`; the kernel hands out blocks and parses nothing), keeps each line as a `ledger_legacy` document (`seq`, `prev`, `boot`, `since_boot_ms`, `words`: typed fields, chained like entries, the conversion's time in the header), checks the file it wrote, and with `--erase` zeroes the old blocks on the device (`diag "log legacy erase"`, each block read back).
- `since_boot_ms` is the old line's counter milliseconds, kept as the number it was: those lines had no Eagle time and none is made up for them.
- The kernel reads one thing of the old region on its own: the header's boot count, once, while the ring holds no entry, so boot numbers go on (b930, b931, ...). Once the ring has an entry, its `boot` is the count; once the old log is erased there is nothing to read.
- The converted file is the device's data: keep it with the device's (keys/<device>/), not in a repository.

### What an entry costs

Measured on the host (x86-64, release, `cargo run --release -p ferros_ledger --features ring --example bench`):

| | old line | entry |
|---|---|---|
| CPU | 0.13 us (format, copy) | 16 us (seal key, seal, document, hash); 12 us to verify and open |
| bytes | 69 | 413 |
| block I/O | 2 writes (tail block, header) | 2 writes and 1 read (A, read back, B) |

The device measure is `ferros-bridge diag "log bench"`: the old line's work and its two block writes, the seal key from the ira, the seal and document, and a whole append (writing 2^3 `bench` entries).
The I/O dominates on the device (the old line's two writes under the kernel lock took up to 0.66 ms, `diag lock`); the entry adds a block read and tens of microseconds of CPU.
The kernel grew by 89 KB (text 84 KB, data 5 KB) over the build before the ledger: vsf's schema builders and parser where the kernel did not use them yet, the seal, the ring, and the 10 KB `x-compact` table. With `x-full` (every script, NFC) it was 207 KB of text.

### Where stage 0 stops

- `writer_cap` and `writer_sig` are not populated: there are no writer keys or ledger caps yet. The writer is named (`writer`, sealed).
- One chain over the whole ring, not one per category, and no `cat_seq`: the category is sealed, so per-category order is not visible on the flash. A reader groups by category after opening.
- The kernel is the ledger's only writer and keeps it itself; the userspace ledger server, its caps and its category registry are not built.
- The words are text (`x`, or `a` where the lean table lacks an ASCII character). Stage 1 moves the busiest lines to typed fields (below).

### Deaths (2026-11-18)

A process's end and the kernel's own are the first entries with typed fields beside the words (ferros_ledger `death`, KERNEL.md "A process that dies"):

| Category | Writer | When |
|---|---|---|
| `kernel.process` | the process | every end: an exit, a panic (its report, SYS_PANIC), a fault (the trap) |
| `kernel.panic` | `kernel` | at the boot after a kernel panic or an early fault, from its record in the ramoops page (only when the record's magic and hash hold) |

The fields, in the sealed payload after `refused`, each present only when it applies (every number `u`, every text `x` or `a` as the words are, `lean`):

| Field | What |
|---|---|
| `end` | 0 exit, 1 panic, 2 fault, 3 the kernel panicked, 4 the kernel faulted, 5 ended from outside (numbers on the flash: append, never renumber) |
| `exit` | the exit value |
| `message`, `file`, `line`, `column` | the panic's message (2^9 bytes at most) and where (the file as the compiler named it, 2^8 bytes at most) |
| `thread` | the thread: the kernel's name (`name.k`) and the one it gave itself |
| `esr`, `far`, `pc`, `lr` | a fault's syndrome and addresses; a process's PC and LR are offsets into its arc |
| `frames` | the call chain, innermost first, several values in one field: the PC, then each frame record's return address (offsets into the arc; the kernel's own addresses for `kernel.panic`), at most 2^5 |
| `code` | where the image starts in the arc (G#1000): `frame - code` is the address in the ELF |
| `last_words` | the last console line (2^8 bytes at most) |
| `of_boot`, `core`, `crumb` | the kernel's end: the boot it was in, its core, its last crumb |

- The sentence stays in the words (`display: panicked at payloads/display/src/main.rs:…: range end index …`), for `diag log`; nothing reads the values back out of it.
- The fields are bounded so an entry stays one block: the ring splits only the words, and a death's fields go with the first half.
- `ledger_open` carries the same fields, so the answer hands them over opened.
- A reader from before the fields opens the entry as before: vsf's schema parse passes over a field it does not know.
- `diag "log deaths [n] [blocks]"` answers with the newest n deaths in the newest blocks (`Ring::back`: block by block from the head, newest first, each system entry opened to see its category); `ferros-bridge deaths` reads it.
- How long they stay: the ring keeps its entries until it wraps. On the Pro on 2026-11-18 (b976) the ring stood at entry A#13364 in block A#1485 of A#16384, about nine entries a block, so it holds about 147 thousand entries; 13364 entries since the ring began on 2026-11-17 puts it at about ten days of this device's development logging, more on a device left alone.

### Stage 1: the busiest lines, as typed fields

The ten sites that write most, by how often their path runs, and the fields each would carry instead of words (every number `u` or `i`, every time `e6`, every duration in oscillations, every name `d`):

| Site | Today's words | Category | Typed fields |
|---|---|---|---|
| process.rs:5390, a process's console line kept (`line_from`, camera and the drivers) | the driver's own text | `kernel.console` until each driver has its own | per driver: `step` (u), `status` (u), the driver's values; the writer stays the process |
| devsched.rs:172, lock held long | holds, the last tick and its microseconds, the holder | `kernel.lock` | `holds` (u), `last_at` (e6), `held` (u, oscillations), `syscall` (u), `op` (u), `thread` (u) |
| service.rs:3122, heartbeat | up seconds, USB commands, slices | `kernel.heartbeat` | `up` (u, oscillations), `usb_cmds` (u), `slices` (five u) |
| pd.rs:84/93/133/297, power-domain steps after a jump | domain and step | `kernel.power` | `domain` (d), `step` (u), `status` (u), `serror` (u0) |
| camera_power.rs:84-259, camera on and off | module, steps, time taken, S2MPUs | `device.camera` | `module` (u), `step` (u), `took` (u, oscillations), `s2mpu_fenced` (u), `result` (u) |
| process.rs `death_line`, a process ended, panicked or faulted (built 2026-11-18: "Deaths" above) | name, exit value, last line | `kernel.process` | the writer is the name; `end`, `exit`, `message`, `file`, `line`, `column`, `thread`, `esr`, `far`, `pc`, `lr`, `frames`, `code`, `last_words` |
| service.rs:1259-1363, restart and on-demand starts | name, capsule bytes, outcome | `kernel.process` | `name` (d), `step` (u), `bytes` (u), `result` (u) |
| service.rs:2424-2437, USB TCPC role | role, cc, power, switch | `usb.tcpc` | `role` (u), `cc` (u), `power` (u), `switch` (u), `alert` (u) |
| service.rs:2883-2917 and 3301-3372, kist offers and long commands | bytes, status, milliseconds | `kernel.kist` | `bytes` (u), `status` (u), `took` (u, oscillations), `why` (u) |
| process.rs:1807/3756/5638, an interrupt storm or a syscall buffer trapped | line, process, offsets | `kernel.trap` | `syscall` (u), `op` (u), `offset` (u), `address` (u), `length` (u), `pc` (u), `lr` (u), `thread` (u), `intid` (u) |

The boot's own lines (`boot cold`, `bootloader saw`, `kvault open ok in N ms`, the S2MPU fences, the zone) are rarer but every boot has them: `kernel.boot` with `kind` (u: cold, reload, jump), `took` (u, oscillations), `generation` (u), `values` (u), `fenced` (u) per S2MPU.

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
                      killswitch ready — Ring FS backs it, BLAKE3 proves it
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
├── Ledger::Vault
│   ├── Ledger::Vault::Create      object created (hp, type)
│   ├── Ledger::Vault::Delete      object deleted (hp)
│   ├── Ledger::Vault::Update      object updated (hp, old_hb, new_hb)
│   ├── Ledger::Vault::Commit      spine entry written (gen)
│   ├── Ledger::Vault::Plow        plow relocation events
│   ├── Ledger::Vault::Repair      survivable engine faults: verify-retry
│   │                              succeeded, bad block relocated, one
│   │                              mirror degraded — notify-always; the
│   │                              RATE of repairs is the flash-death
│   │                              early warning, so silent repairs are
│   │                              recorded, never swallowed
│   └── Ledger::Vault::Reap        reap window stats, value migration
├── Ledger::RingFS
│   ├── Ledger::RingFS::Boot
│   ├── Ledger::RingFS::Write
│   └── Ledger::RingFS::Repair
├── Ledger::VSF
└── Ledger::App::<cap_hash>     per-app, minted at stow
                                identified by app cap hash, not name
```

**The one event class that must NOT go through the ledger:** engine-fatal storage faults (vault cannot commit, both mirrors failing, spine unwritable).
The ledger is a tenant of the vault, so "the vault is dying" cannot be recorded via the vault — the watchdog can never be the thing it is watching.
Fatal storage events go to the RAM diag ring (retrieved via `ferros-bridge diag` / PT DIAG) and the framebuffer console; on Pixel, pstore/ramoops preserves them across a warm reboot.
See [VAULT.md](VAULT.md) Engine Migration for the event-sink split.
On the host profile, Photon's `log.vsf` plays the ledger's role for the same VSF event schema — the format is shared, only the sink differs.

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

5. Application stow
   Ledger::App::<cap_hash> category minted
   App receives Cap<Write, Ledger::App::<its_cap_hash>>
   Cannot write to any other category
```

---

## Killswitch Ready

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

As built (stage 0, above): XChaCha20-Poly1305 under a system key from the ira or the person's ledger key, by the entry's class; the nonce and tag in typed fields, the provenance hash over the sealed form.
The `boot_key` design below (per-boot fresh) would leave a past boot's entries unreadable at the next; the flash log has to be read across boots, so its system key is the device's own and the same every boot.

VSF handles this. Not the Ledger server. Not a separate layer.

```
VSF on disk:
  BLAKE3 hash: always present, auto-computed
  ChaCha20 encryption: document encrypted with boot_key
  boot_key: derived from hardware key, never RAM, per-boot fresh
            FP5: UFS RPMB (SHA256-HMAC, write-once, can't read back)
            Glyph: BLAKE3 silicon oracle (private domain separator, optical link)
            See SECURITY_CHAIN.md key storage section

  Encrypted VSF document is still a VSF document
  Type markers intact (O(1) skip still works)
  Content: ciphertext
  Hash: over ciphertext (integrity of encrypted form)

Ledger uses boot_key encryption (system namespace).
  No per-reader key wrapping — kernel is the only writer/reader.
  User-facing vault objects use per-namespace content keys
  with X25519-wrapped reader access (see VAULT.md Access Control).
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

Theorem Ledger_KillswitchReady:
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