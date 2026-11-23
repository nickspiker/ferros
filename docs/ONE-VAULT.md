# One vault, and the seed that runs it

Status: design for the owner, 2026-11-23. Nothing here is built. It replaces STORAGE-FIELD.md (a pool of separate fields), which the owner overrode the same day.

The owner, 2026-11-23: "There's ONE fixed size element, the seed. the rest is all field. ALL OF IT", and then: "It better run the seed. that was the whole fucking point. then EVERYTHING ELSE goes in the vault. Nothing special, that way it all rotates and levels properly. We do have a special write on one image only without verification for RAW video we need fast but that's it."

The rule ("Claude memories/storage_seed_only_fixed.md"):
- The seed is the one fixed-size element on the ferros partition, and it runs on every boot: it verifies itself, finds and verifies the kernel, jumps.
- Everything else is vault content: the kernel ring, the flash log, the kernel's own store, settings, documents, stills, RAW clips, benches. One vault over the whole partition beside the seed, so every write rotates through the ring and the flash wears evenly. Sizes come from what is stored, never a constant.
- One exception: RAW video frames are written once, one copy, with no read-back, for speed; still in the vault and still rotating.

Units: a block is 4 KiB; 2^18 blocks are 1 GiB. Offsets are from the start of the ferros partition.

## 0. What is fixed today, and goes

As built on forge b939f147. Every row but the seed becomes vault content and its constant is deleted.

| Thing | Where, how large | Defined at | Carved by |
|---|---|---|---|
| Seed copies A and B | blocks G#400 and G#800, 16 KiB each | ferros_layout/src/lib.rs:29, :32 | b503d534, 2026-03-17 |
| Stem (kernel ring) and kernel copies | G#C00, 256 entries; kernels at G#D00 and G#1500, 8 MiB each at most | ferros_layout/src/lib.rs:39-63 | b503d534 |
| Spine, state ring, ledger ring, old tract | G#2000 (256 MiB), G#40000 and G#80000 (1 GiB each), G#C0000 on | ferros_layout/src/lib.rs:70-114 | b503d534; tools/install only |
| Flash log | 1 GiB in: the old text log (2^14 blocks) and the ring, two copies of 2^14 | ferros_kernel/src/flashlog.rs:26-30 | b4f52efd, 2026-11-17 |
| Kernel's own vault (kvault) | 2 GiB in, two regions of 2^16 blocks | ferros_kernel/src/kvault.rs:18, :20 | c00d0e80, 3a93cc7e |
| Vault process window | 4 GiB in, 2^22 blocks: two copies of 8 GiB at most | ferros_kernel/src/process.rs:4503, payloads/vault/src/main.rs:95-96 | b922919c, 684f8d3e |
| Clips (RAW video) | 24 GiB in, 48 GiB | process.rs:4508-4509, payloads/vault/src/main.rs:90 | 684f8d3e, 2026-11-19 |
| rawbench (development) | 72 GiB in, 32 GiB | process.rs:4511 | 684f8d3e |
| The 8a's bound | 20,357,627 blocks (77.7 GiB) as a build-time assert | process.rs:4513-4514 | 684f8d3e |

Unused today: 20 to 24 GiB, and 104 GiB to the Pro's end (128 GiB). The Pro ran out at 8 GiB a copy with about 100 GiB idle.

Outside the ferros partition and Google's, not ours: devinfo's slot records (LU 3), misc's boot control block, the 8a's `klog`. They stay.

## 1. The boot chain: the seed is the root

Today the Pro and the 8a boot ABL, then the boot partition's image (the gate: the hv, Linux's Image for Android, and the ferros kernel with its vault capsule carried), and the hv jumps straight to the kernel. The seed never runs.

The chain, with the seed as the root:
0. **ABL** loads the boot partition's image, signed with the owner's AVB key as now. It holds the hv and Linux's Image, and no ferros kernel.
1. **The hv** starts at EL2 as now: it keeps core 3, the key chords, the UFS firewall, and Android's path (Linux's Image, on the identity's word). On ferros's path it does one new thing: it reads the seed from its fixed place on the ferros partition (the partition found in the GPT by its type, BLAKE3("ferros")), checks the seed's signature against the owner's key it carries, and jumps to it. Copy A, else copy B.
2. **The seed** checks itself (SEED.md), then reads just enough of the vault to find the stem, then the kernel the stem names; checks the kernel's signature; jumps.
3. **The kernel** runs as today, its vault capsule and init read from the stem the same way.

A kernel update is then a vault write (a new stem entry), not a reflash of the boot partition; the boot partition changes only when the hv or Android's Image does. The chord that jumps from Android to ferros starts the seed too, so every ferros start runs it.

### The seed's way into the vault

The seed holds no keys (the roots live in the kernel's registers, KEY_REGISTERS.md), so what it reads must be readable without them and verifiable without trust:
- The stem and the images it names (the kernel, the vault process's capsule, init, the USB rescue) are stored as plain manifestus entries under well-known keys, not sealed ferros_store values: they are code, not secrets, and each carries the owner's signature, which the seed checks.
- Every block on the way carries its own BLAKE3 (manifestus's `hp`), and the seed checks each one as it reads.

The steps, all reads:
0. **The vault's root slots**: manifestus's four root blocks (`ROOT_SLOTS`, manifestus src/ring.rs:544) at the start of each copy, right after the seed. Each is a hashed, chained entry saying where the spine's ring lives now (the spine moves between residences as it wears, `migrate_ring`). These four blocks per copy are the only other fixed place, and they are part of the vault's own format; they are written only when the spine moves, which is rare.
1. **The spine**: a binary search of the ring for the newest valid entry (as the stem search does today), each entry checked by its hash and chained to the last.
2. **The index**: from the entry's HAMT root down to the stem's well-known key, each node checked by its hash (manifestus `find_leaf`).
3. **The stem**: a small VSF document, its kernels newest first, each with its key, length, BLAKE3 and signature.
4. **The kernel**: its furrows read in runs and checked, the whole image's BLAKE3 against the stem's, the owner's signature against the seed's key.

Anything that fails falls back in order: the other copy's blocks, the previous spine entry (the fence keeps the last four restorable), the stem's previous kernel. The seed is then a small read-only manifestus: the ring search, the HAMT walk and the furrow checks, no writes, no keys, no allocation beyond a bounce buffer.

## 2. One vault over the whole partition

- **The partition after the seed is one vault**, its two copies the two halves of what is left (each half's length read from the GPT at boot, never a constant). Both halves rotate together: position p of the tract is block p of each half, written to half 0, flushed, read back, then half 1 (write-verify-then-mirror, as the vault's commits are now).
- **Everything is a value**: the kernel's own store (kvault, folded in: its scopes become domains of the one vault, as the vault process already holds most of them), the flash log (entries appended as values, its length a setting: how much history to keep), settings, the hold's documents and stills, the stem, benches. Each takes what it holds and gives it back when removed.
- **Wear levels by rotation**: the plow writes forward around the whole partition and the reap moves what is live ahead of it, so every block is written once a lap. The cost is the reap's moves: for each lap, everything live is copied once, so a vault that is half full writes about one extra byte for each byte stored, and one 80 percent full about four. The vault says full while a sixteenth of the tract is still clean, and stops reaping for new values once more than three quarters is live (manifestus 0eb4f80): in the tests it was full at 65 to 80 percent live, short of where that cost runs away, and a full vault can always delete.

## 3. The one special path: RAW video

A clip is one value whose body is an extent: a run of tract positions reserved at the plow for it, written by the recorder straight to the flash, one copy, no read-back, no cache flush per frame (as clips are today, 684f8d3e, only now inside the tract instead of a window beside it).

- **One copy in a mirrored ring**: position p is a block on each half, so a clip of n blocks reserves n/2 positions and writes its first half on half 0's blocks and its second on half 1's (striped), so nothing is wasted on the other copy. Its record (owner, length, BLAKE3, which half holds which part) is an ordinary mirrored, verified value, as the extent table is now (manifestus extent.rs `table_key`).
- **Sealing**: at the clip's end, the seal commit flushes the device's cache (which takes every frame with it) before the record says sealed; a clip is then on the medium, unverified block by block but hashed whole.
- **The reap**: when the reap reaches a clip's positions, the clip is live like any value and moves ahead of the plow, copied at the flash's rate in runs, still one copy each half, its BLAKE3 checked once on the copy (the move is an ordinary vault write, verified; only the recorder's own write skips the read-back). A removed clip's positions are dead and cost nothing.
- A clip as long as the clips area today (40 GB) moves once a lap, like everything else; that is the price of every block wearing evenly.

## 4. What it means for manifestus and the kernel

manifestus:
- The device: each copy is a half of the partition (`max_blocks` its length; the tract created at its full length at genesis, so it never grows).
- Extents inside the tract: reserve a run of positions at the plow under a placeholder leaf; the writer fills them directly; the seal records length and BLAKE3; the open's tree walk marks them live by position (it already does for extent leaves); the reap moves them as runs and patches their leaf.
- Plain entries under well-known keys for the stem and the images it names (today every ferros value is sealed by ferros_store; these are manifestus entries the kernel writes and the seed reads).
- Unchanged: the spine, its migration, the HAMT, the mirror's protocol, the fence, the full-vault rules.

The kernel's early boot, before the vault process runs:
- It reads the stem's images (the vault process's capsule, init, the USB rescue) with the seed's same read-only walk.
- One engine writes the vault at a time. Proposed: the kernel holds the vault itself from the moment it starts (opening it as kvault does today), writes its early log lines and anything else it must, then commits, closes it and spawns the vault process, which opens it; never both at once. Until it opens, the kernel's lines wait in the pre-boot buffer the flash log already has (2^6 entries), and a crash before then leaves its crumbs in RAM for the next boot (as `diag crumbs` reads them now).

## 5. The migration, describe-first, nothing lost

On the Pro:
0. **Backups first**, each hashed on both ends into keys/<device>/: the hold's 205 stills (`items get`), the vault process's window, kvault's regions, the flash log, the clips area's taken extents (921,562 blocks), and both boot slots as they are.
1. **Build and test at the desk**: the seed with its read-only vault walk, the hv's load of the seed, manifestus's whole-partition copies, in-tract extents and plain entries, the kernel's sequential ownership of the vault.
2. **The new vault beside the old**: created over the whole partition with its plow starting in the free tail (104 GiB on), so the old regions (kvault, the flash log, the 16 GiB window, the clip) are not touched while their content is copied in. Content moves by value thru the running vault processes (every domain, every item, the clip as an extent), each checked by hash on arrival.
3. **The stem and the switch**: the kernel and its images written as the new vault's stem; both boot slots flashed with the gate that loads the seed (the hv and Linux's Image only); a cold boot runs the seed into the new vault.
4. **The old regions** are no longer referenced and stay intact until the plow laps around to them (about 28 GiB of writes on the Pro before it reaches the old window); until then a rollback to the old slots finds everything where it was.
5. **The constants in section 0 are deleted**, so nothing can carve a window again.

On the 8a: the same steps on its 77.7 GiB partition, its plow starting past its old regions, and its flash log moved from Android's `klog` into the vault (or left there: the one question for the 8a).

## 6. Short term: the 205 stills

The Pro's vault is full at 8 GiB a copy and refuses every write.
- Copy the stills off now in any case: it is step 0's backup.
- Delete some only to shoot before step 3 boots; with manifestus 0eb4f80 and ferros 174c79c5 on the Pro, removals land in the full state, and about 0.5 to 1 GiB a copy must go before a new still fits.
- After step 3 nothing needs deleting: the vault is the whole partition.

## Decisions for the owner

0. The vault's four root blocks per copy, right after the seed, as the one fixed place the seed reads besides itself (part of manifestus's format, written only when the spine moves). Yes or no.
1. The hv loads and checks the seed from the ferros partition (proposed), rather than the seed riding in the boot image.
2. The kernel holds the vault before the vault process and hands it over (proposed), rather than keeping its early lines in RAM only.
3. A clip striped across both halves, one copy each part (proposed), rather than on one half with the other half's positions left empty.
