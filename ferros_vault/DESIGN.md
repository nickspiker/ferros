# Ferros Ledger — Design Rationale

How the Ledger's decisions contrast with BTRFS and RedoxFS at every structural level.

---

## 1. Identity & Addressing

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Object identity | 64-bit objectid + type + offset (17-byte composite key) | 32-bit TreePtr (inode number) | BLAKE3 hash of content (32 bytes). Identity IS the hash. |
| Namespace | Hierarchical: directory trees, subvolumes | Hierarchical: 4-level inode B-tree | **Flat.** Single namespace indexed by hash. No directories. |
| Addressing ceiling | 2^64 objectids | 2^32 inodes | 2^256 hash space. No ceiling in practice. |

**Why:** Fixed-width integer IDs create hard ceilings and require allocation tracking. Content-addressing eliminates both — the object's content determines its address, and collisions in a 256-bit space are physically impossible.

## 2. Mutability & History

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Mutation model | CoW trees, but objects are logically mutable (same inode, new data) | CoW with generation counter | **Append-only.** No object is ever overwritten. Mutation = new object with new hash. |
| History | Snapshots (explicit), otherwise old data freed | Previous generation in header ring (1 deep) | All previous states persist until explicit garbage collection. |

**Why:** Append-only eliminates an entire class of corruption. BTRFS can lose data between snapshots; RedoxFS keeps exactly one previous state. The Ledger keeps everything — garbage collection is a policy decision, not a structural requirement.

## 3. Atomicity

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Atomic unit | Transaction (batched, ~30s flush) | Single CoW write + header ring slot | **Mesh commit.** Write is not real until mesh consensus confirms it across devices. |
| Single point of truth | Superblock (overwritten in place — the ONE exception to CoW) | Header ring slot (gen % 256) | **No single point of truth.** Neither device is canonical — mesh consensus is truth. |
| Crash recovery | Replay from last valid superblock | Scan 256 header ring slots, use newest valid | **Assume hostile previous state.** Mesh arbitrates before mount. Boot trusts nothing. |

**Why:** BTRFS's superblock overwrite is a single point of failure — a torn write there is catastrophic. RedoxFS's 256-slot ring is better but still trusts one device. The Ledger requires multi-device agreement before any state is considered mounted.

## 4. Size & Structure

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Block/sector size | Fixed 4K sectors, 16K nodes (configurable at format) | Fixed 4K blocks, level system for larger allocations | **No fixed-size structures.** VSF EWE (Elastic Width Encoding) throughout. |
| Minimum viable volume | ~256 MiB (superblock + system block group + metadata group) | ~1 MiB (header ring + 4 bootstrap blocks) | **No minimum.** Same design from 8KB flash to distributed cluster. |
| Maximum volume | 16 EiB (64-bit byte offsets) | Disk size (u64 block count) | **No maximum.** Mesh is the scaling mechanism. |

**Why:** Fixed block sizes waste space on small devices and fragment on large ones. VSF's elastic encoding means a ledger object is exactly as large as its content requires — no padding, no alignment waste.

## 5. Permission Model

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Model | POSIX uid/gid/mode (32-bit each) + optional ACLs via xattrs | Unix uid/gid/mode (u32/u32/u16) | **Capability-gated.** Possession of hash = access credential. |
| Escalation | Root (uid 0) bypasses all checks | Root (uid 0) bypasses all checks | **One-way hash chain.** write_hash → read_hash is derivable. read_hash → write_hash is not. No superuser. |
| Revocation | Change permissions (requires root or owner) | Change mode bits | **Rotate salt.** All derived hashes become invalid. No central authority needed. |

**Why:** POSIX permissions assume a trusted kernel enforcing uid checks. The Ledger operates in a capability model where the credential IS the hash — no kernel permission check needed, no root bypass possible, and revocation is cryptographically enforced.

## 6. Checksums & Integrity

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Algorithm | CRC32c default; optional xxhash64, sha256, blake2b | SeaHash (64-bit) | **BLAKE3.** Same hash for addressing AND integrity — no separate checksum tree. |
| Storage | Separate csum tree (data) + embedded in headers (metadata) | Embedded in BlockPtr (8-byte hash per pointer) | **Implicit.** The object's address IS its checksum. Corruption = hash mismatch = object doesn't exist. |
| Verification | Read-time comparison against stored checksum | Read-time comparison against BlockPtr hash | **Automatic.** If you can find it by hash, it's valid. No separate verification step. |

**Why:** BTRFS maintains a separate checksum tree that must stay consistent with the data it protects — a second thing that can go wrong. RedoxFS embeds hashes in pointers, which is better. The Ledger unifies addressing and integrity into a single operation: content-addressing means verification is free.

## 7. Multi-Device / Replication

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Model | RAID profiles (0/1/5/6/10) via chunk tree | Single device only | **Mesh.** Dual SSD, dual vendor minimum. Semantic replication, not block mirroring. |
| Failure handling | Scrub + auto-repair from mirrors | N/A (single device) | **First-class typed VSF records.** Failed writes become queryable objects in the ledger. |
| Consensus | Device with newest valid superblock wins | Newest generation in header ring | **Mesh consensus before mount.** No device is trusted individually. |

**Why:** BTRFS RAID mirrors blocks — it doesn't understand what's in them. The Ledger replicates objects semantically, meaning the mesh can make intelligent decisions about conflicts. Failed states aren't silently discarded; they're preserved as typed records that can be inspected and resolved.

## 8. Boot Sequence

| Decision | BTRFS | RedoxFS | Ledger |
|----------|-------|---------|--------|
| Trust model | Trust superblock at known offset | Trust newest valid header in ring | **Trust nothing.** Boot assumes hostile previous state. |
| Mount sequence | Read superblock → replay log tree → mount | Scan header ring → replay alloc log → mount | **Mesh arbitration → consensus → mount.** Devices must agree before anything is readable. |
| Superblock location | Fixed: 64K, 64M, 256G | Fixed: block_offset + (gen % 256) | **No fixed location.** Mesh protocol locates current state. |

**Why:** Fixed superblock locations are an attack surface (overwrite 3 known offsets = dead filesystem). The Ledger has no magic offsets — the mesh protocol establishes truth from the ground up every boot.

---

## 9. The Mesh Anchor — Solving the Bootstrap Problem

The hardest problem in the Ledger design: how does a device find the current state if there are no fixed offsets? You need *something* at a findable location — but that something shouldn't be findable by an attacker.

The answer: **two trust domains.** The key that tells you where to look lives in a different physical medium than the data. Without the key, the storage device looks like random noise.

### Fairphone 5 Boot Chain (Concrete)

The FP5 uses Qualcomm QCM6490. We don't control the early boot chain.

```
Stage     Owner          What Happens
-----     -----          ----
PBL       Qualcomm ROM   SoC powers on, loads XBL from UFS
XBL       Qualcomm+FP    DDR training, UFS init, TrustZone
ABL       Qualcomm+FP    fastboot protocol, boot image verification
  \-- HERE   ferros       ABL loads our boot.img (bootloader unlocked)
kernel    ferros          needs to find the ledger → anchor protocol
```

We enter the picture at ABL, which loads our kernel from `boot_a`/`boot_b` partitions (standard A/B slot scheme). Everything before ABL is Qualcomm's signed chain — immutable, and we should leave it alone because it initializes the hardware.

### Where the Anchor Key Lives on FP5 — The RPMB Problem

RPMB (Replay Protected Memory Block) exists on the FP5's UFS chip and would be the ideal key store. But we can't easily reach it:

```
ferros kernel (EL1, Normal World)
    │
    │ SMC (Secure Monitor Call)
    ▼
Qualcomm QTEE (EL3, Secure World) ← signed blob, not ours
    │
    │ RPMB auth key (derived from QFPROM hardware fuses)
    ▼
UFS RPMB partition
```

The RPMB authentication key is derived from Qualcomm hardware fuses at first boot, provisioned by XBL, and held exclusively in TrustZone. Our kernel (EL1) cannot access RPMB directly — it must go through QTEE via SMC calls, and QTEE expects Android's Keymaster HAL on the other end. It may refuse a non-Android caller entirely.

**This is the honest constraint of building on someone else's silicon.**

### The Three Phases (Concrete)

**Phase 1 — Dedicated partition (bring-up)**
```
fastboot flash ferros_anchor <key.img>
```
Anchor key lives in a regular UFS partition. Same trust domain as data — no hardware isolation. But security still comes from the mesh: an attacker needs BOTH devices from BOTH vendors. The partition just holds the key; the ring offsets are still BLAKE3-derived and the anchors are still HMAC'd. This is where we start.

**Phase 2 — Custom ABL handoff (mid-term)**
Fairphone publishes bootloader sources (EDK2-based). We build a modified ABL that:
1. Reads the anchor key from RPMB (ABL has TrustZone access)
2. Stashes it in a DTB reserved-memory node
3. Boots our kernel
4. Kernel reads key from DTB, scans anchor ring, zeroes the RAM region

This gets us RPMB-backed security without needing to reverse-engineer QTEE's SMC interface from EL1. One-way handoff: RPMB → ABL → DTB → kernel → zero.

**Phase 3 — Own the stack (Glyph)**
On Glyph hardware we control the secure world. Anchor key lives in a dedicated CSR or crypto coprocessor register. Hardware write-once-per-boot, read-disabled after initial load. Kill-switch zeroes the register → anchors become unfindable → ledger cryptographically erased.

### The Two Trust Domains

```
 Trust Domain A: Key Store              Trust Domain B: Main Storage
 (where the anchor key lives)           (UFS main LUNs)
+------------------------------------+  +------------------------------------+
| Phase 1: ferros_anchor partition   |  | Anchor ring at derived offsets     |
|          (same UFS, less isolated) |  | (VSF EWE encoded, variable size)  |
| Phase 2: RPMB via ABL handoff     |  |                                   |
|          (hardware-isolated)       |  | offset = BLAKE3(key || slot ||    |
| Phase 3: CSR / crypto coproc      |  |          "anchor_offset")         |
|          (kill-switch erasable)    |  |          % device_capacity        |
|                                    |  |                                   |
| Contains:                          |  | anchor → commit_hash → objects    |
|   AnchorKey (32 bytes)             |  +------------------------------------+
|   ring_size (EWE)                  |
+------------------+-----------------+
                   |
                   | key
                   v
          derive_ring_offset()
```

### VSF Encoding (Not Fixed-Size)

The anchor is NOT a 128-byte C struct. It uses VSF Elastic Width Encoding:
- Each field has a 1-byte tag: `[field_id:4][payload_len:4]`
- `generation` of 0 encodes to 1 byte, not 8
- `anchor_seq` of 3 encodes to 1 byte, not 8
- Fixed-width fields (mesh_id, hashes) carry their natural sizes
- A fresh anchor (gen=0, seq=0) is ~70 bytes
- A mature anchor (gen=2^48) is ~90 bytes
- The 32-byte HMAC is always full-width (BLAKE3 keyed hash)

Ring slots reserve 128 bytes for the *maximum* size. Only the encoded bytes are meaningful; the rest is don't-care noise on encrypted storage. The decoder knows when to stop — no padding assumptions.

### Attack Surface (Honest, Per Phase)

| Attack | BTRFS | RedoxFS | Ledger Phase 1 | Ledger Phase 2+ |
|--------|-------|---------|-----------------|-----------------|
| Find bootstrap | 3 known offsets | Known ring offset | Read ferros_anchor partition for key, then derive | Need RPMB/CSR access to get key |
| Read device state | Follow superblock tree | Scan header ring | Still need the key to find ring slots | Device is noise without key |
| Forge anchor | Overwrite 3 offsets | Write header ring | BLAKE3-HMAC (forgery-resistant) | Same + key is hardware-isolated |
| Replay old state | Write old superblock | Write old header | Mesh rejects stale generation | + RPMB monotonic counter |
| Corrupt one device | FS dead | FS dead | Other mesh device has independent key + ring | Same |
| Physical access to one device | Full access | Full access | Need BOTH devices (mesh) | Same + need RPMB auth on each |

**Phase 1 is weaker on single-device key isolation** — the key is on the same UFS. But the mesh is the primary defense: dual device, dual vendor. Phase 2 adds hardware isolation. Phase 3 adds kill-switch erasure.

### Scale Scenarios

**8KB flash (pico):** 1 anchor slot, offset from eFuse key. The anchor IS the bootstrap — ~70 bytes pointing to 1-2 objects.

**FP5 Phase 1:** 64-slot ring per device, key in `ferros_anchor` partition. Mesh (dual device, dual vendor) is the security boundary, not partition isolation.

**FP5 Phase 2:** Same ring, key handed off from RPMB via custom ABL. Hardware-isolated key store. Attacker needs TrustZone compromise on BOTH devices.

**Glyph:** Key in CSR (read-disabled after boot). Kill-switch zeroes CSR = anchors unfindable = cryptographic erasure.

**Distributed cluster:** Each node stores its own anchor ring. Network discovery replaces RPMB — nodes announce mesh_id + latest generation over authenticated channels.
