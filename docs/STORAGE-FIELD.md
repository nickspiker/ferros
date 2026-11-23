# Storage as a field

Status: design for the owner, 2026-11-23. Nothing here is built.

The owner, 2026-11-23, after the Pro's vault filled at 8 GiB while most of its 128 GiB partition sat idle in other fixed windows: "There's ONE fixed size element, the seed. the rest is all field. ALL OF IT".

The rule ("Claude memories/storage_seed_only_fixed.md"): on the ferros partition the seed is the only fixed-size element. Everything else takes room from one pool as it needs it, no wider than it needs, and the map of what is where is a self-describing VSF document on the disk, never a constant in the kernel. This is VSF's own rule (every number as wide as it needs to be and no wider, no ceiling) applied to the disk.

Units: a block is 4 KiB; 2^18 blocks are 1 GiB. Offsets are from the start of the ferros partition.

## 0. What is fixed today

Every fixed size or place on the ferros partition, as built on forge b939f147.

| Thing | Where, how large | Defined at | Carved by | Used today |
|---|---|---|---|---|
| Seed copies A and B | blocks G#400 and G#800, 16 KiB each | ferros_layout/src/lib.rs:29, :32 | b503d534, 2026-03-17 | written by tools/install (the M1 and SD path); the Pro and the 8a boot thru ABL and their boot partitions (the hv's gate carries the kernel and the vault capsule), so neither reads them |
| Stem (kernel ring) | block G#C00, 256 entries; kernel copies A at G#D00 and B at G#1500, at most 2048 blocks (8 MiB) each | ferros_layout/src/lib.rs:39-63 | b503d534 | ferros_seed, ferros_hal ring.rs, tools/install; not on the Pro's or 8a's boot path |
| Spine (vault root ring) | block G#2000, 2^16 entries (256 MiB) | ferros_layout/src/lib.rs:70-81 | b503d534 | tools/install only; the vaults keep their own rings inside their regions |
| State ring, ledger ring | G#40000 (1 GiB in) and G#80000 (2 GiB in), 2^18 blocks (1 GiB) each | ferros_layout/src/lib.rs:88-101 | b503d534 | tools/install only; their places are now taken by the flash log and kvault below |
| Tract (old design) | G#C0000 (3 GiB in) to G#3A0_0000 | ferros_layout/src/lib.rs:108-114 | b503d534 | ferros_hal ring.rs and tools/install; not the vaults in use |
| Flash log, old text log | 1 GiB in, 2^14 blocks (64 MiB) | ferros_kernel/src/flashlog.rs:26-27 | b4f52efd, 2026-11-17 | read once by the converter, then erased |
| Flash log ring | right after it, two copies of 2^14 blocks (64 MiB each) | flashlog.rs:29-30 | b4f52efd | the Pro; the 8a's log is in Android's `klog` partition |
| Kernel's own vault (kvault) | 2 GiB in, two regions of 2^16 blocks (256 MiB each) | ferros_kernel/src/kvault.rs:18, :20 | c00d0e80 (2026-10-20), 3a93cc7e (2026-11-06) | both boards |
| Vault process window | 4 GiB in, 2^22 blocks (16 GiB): mirror 0 its first half, mirror 1 its second, each a tract that starts at 2^17 blocks (512 MiB) and doubles to 2^21 (8 GiB) | ferros_kernel/src/process.rs:4503 (`WINDOWS`), payloads/vault/src/main.rs:95-96 | b922919c (2026-11-07), 684f8d3e | both boards; the Pro's is full |
| Unused | 20 GiB to 24 GiB (4 GiB) | process.rs:4513 `PROVEN_FREE_FROM` | 684f8d3e | nothing |
| Clips (RAW video) | 24 GiB in, 12 x 2^20 blocks (48 GiB) | process.rs:4508-4509, payloads/vault/src/main.rs:90 | 684f8d3e, 2026-11-19 | both boards; the Pro has 921,562 blocks (3.5 GiB) in it |
| rawbench (development) | 72 GiB in, 2^23 blocks (32 GiB) | process.rs:4511 | 684f8d3e | development builds only; never past the 8a's end by an assert |
| Unused | 104 GiB to the partition's end (the Pro: 33,554,427 blocks, 128 GiB) | | | nothing |
| The 8a's bound | `PROVEN_FREE_TO` = 20,357,627 blocks (77.7 GiB), the 8a's partition, so clips fit both boards | process.rs:4514 | 684f8d3e | a build-time assert |

Outside the ferros partition, and Google's layout rather than ours: the slot records in devinfo (LU 3), misc's boot control block, and the 8a's `klog`. They stay as they are; the rule is about our partition.

How it happened: each window was carved one step at a time, each locally reasonable, none set against VSF's rule, and none shown to the owner as a layout. The Pro ran out at 8 GiB a copy with about 100 GiB idle.

## 1. The design: one pool, the seed, and a map

### The pool

The ferros partition less the seed is one pool of blocks. Room is handed out as extents: a first block and a length, any length. A consumer (a field) holds a list of extents, asks for more when it needs more, and gives extents back when it is done with them. Nothing has a ceiling but the disk.

The fields:
- the vault's copy 0 and copy 1, each its own list of extents, never sharing one; write-verify-then-mirror is unchanged, since manifestus's mirror sees two devices whatever they are made of;
- the kernel's own vault (kvault), the same way (or folded into the vault process later, which VAULT-OUT.md already plans);
- the flash log, two copies; its length is a setting (how much history to keep), not a carve;
- each RAW clip, one extent of its own, single-copy as now, taken from the same pool and given back when the clip is removed;
- the stem and its kernel images, on a device whose boot ferros owns;
- benches and scratch (rawbench), only while one runs.

The allocator is the kernel's: it already hands out the block windows as capabilities and checks every call against them. A field's capability becomes its list of extents; a process never sees a block outside its own.

### The map

The map of what is where is one VSF document: a complete VSF file (magic, version, Eagle-time creation, provenance hash, field count), readable by vsfinfo. Its sections:
- `pool`: the partition's length, the map's generation (unsigned), its predecessor's provenance hash;
- one section per field: its name, who may hold it (the capability's kind), its extents as (first, blocks) pairs, every number EWE-encoded, as wide as it needs to be;
- `free`: the free extents (or none: free is whatever no field holds, computed at load; to decide).

The map is a field like the others. It is kept in two copies on separate extents, each written, flushed past the device's cache, read back and compared before the next is touched (write-verify-then-mirror, as the vault's commits are since a0a7056e). A change writes generation n+1 to fresh extents; the old generation's extents are freed only once both copies of n+1 are verified and the anchor names them. A power cut anywhere leaves generation n or n+1 whole.

### How the map is found

Nothing is fixed but the seed, so the way to the map starts at the seed. Two ways, for the owner to choose:

- **(a) Anchors inside the seed element.** The seed's fixed area holds, beside its two code copies, two anchor blocks: each a small VSF document naming the map's generation, the extents of both its copies and their provenance hashes. Updated last: anchor A written, flushed, read back, then anchor B. At boot, the anchor whose generation is highest and whose map's hash checks wins; the other is the fallback. The seed's code never changes; its anchors are its data. The fixed element is then "the seed: code and anchors", one fixed size.
- **(b) The map right after the seed.** The map's first copy always starts at the block after the seed's end, its length its own (VSF says how long it is). No anchor blocks; but the first copy's place is fixed by the seed's size, and a map that grows moves everything after it, so its first extent must be given room to grow into, which is a carve again.

(a) is the proposal: it keeps exactly one fixed element and lets the map live anywhere.

The stem (the kernel ring) is found the same way: seed, anchor, map, the `stem` field's extents; its binary search for the newest kernel entry runs over the extents' blocks as today's runs over its fixed 256. On the Pro and the 8a, which boot thru ABL and their boot partitions, the kernel itself reads the anchor at its first storage step and opens every field from the map.

## 2. What manifestus and kete need

Very little, because a tract is already one logical run of blocks over a device:
- A device over a list of extents (in ferros: the vault process's `Half` and kvault's `Region` become one `FieldDev`): logical block to (extent, offset); a run that crosses an extent's end is split there.
- Growing is appending an extent: `BlockDev::grow_from(old, new)` asks the pool for new minus old blocks (several extents if the pool is fragmented); refused, the vault is full and says so (TractFull, manifestus 06bafbc and 0eb4f80). `max_blocks` becomes what the device holds plus what the pool could give.
- Nothing in the tract, the HAMT, the spine or the mirror changes. A tract never shrinks today; giving room back is a later step (compact, then truncate the tract and return its tail extents).
- kete (Photon's host vault) is untouched: its devices are files, and a file is already a field.

## 3. The migration, without losing a byte

The trick: describe first, move nothing. Today's regions become the map's first fields exactly where they lie.

On the Pro:
0. Backups to Harbor (keys/<device>/), each with its hash checked against the device: the hold's 205 stills (`items get`, which is also today's stopgap), a raw read of the vault process's window (both copies), kvault's regions, the flash log, and the clips area's taken extents (921,562 blocks).
1. Write the first map, generation 0, describing today's layout as fields: vault copy 0 = one extent (4 GiB in, its tract's blocks), copy 1 likewise (12 GiB in), kvault's two regions, the flash log's two copies, each recorded clip's extent (from the vault's extents table), the seed and stem as they are. Everything else is free: 20 to 24 GiB, the rest of the old clips window, 72 GiB to the end. Write the anchors.
2. A kernel and vault process that open their fields from the map instead of `WINDOWS`, `REGION_BLOCKS`, the flash log's offsets and the vault's `MIRROR` and `HALF`. Flashed to both slots before any field grows: a kernel from before would still read the old windows and would not see an extent appended past them (the trap in stored_capsules_need_flashed_kernel.md).
3. The vault grows by appending extents: the 8 GiB ceiling is gone the moment step 2 boots, with nothing moved.
4. rawbench leaves the map (a bench takes extents only while it runs); the unused tails of the old clips window are already free in step 1.
5. Every constant in the inventory above is removed from the code, so nothing can carve a window again.

On the 8a: the same steps on its 77.7 GiB partition; its flash log stays in Android's `klog` (outside our partition) or joins the pool, the owner's choice.

Fragmentation needs no compaction step: the vault reads and writes runs, and a run split at an extent's end costs one more command.

## 4. Short term: the 205 stills

The vault on the Pro is full at 8 GiB a copy, and every write to it is refused until room is made.

- Copying the stills off (`items get`) is right in any case: it is the backup that step 0 needs.
- Deleting some is only needed if the owner wants to shoot before step 2 is on the device. With the fixes in manifestus 0eb4f80 and ferros 174c79c5 on the device, removals land even in the full state, and roughly 0.5 to 1 GiB a copy must go before a new still fits (the vault keeps a sixteenth of its tract free for deletes and the reap).
- Once the field design's step 2 boots, nothing needs to be deleted: the vault simply grows.
- The full-vault fixes stay either way: a pool can run out too, and then the vault must still say "full", still delete, and still get its room back.

## Decisions for the owner

0. The map's anchor: (a) inside the seed element (proposed) or (b) the map right after the seed.
1. The allocator in the kernel (proposed), beside the capabilities it already checks.
2. The flash log's length as a setting, and the 8a's log in `klog` or the pool.
3. kvault as its own field, or folded into the vault process now.
4. Whether `free` is written in the map or computed from the fields at load.
