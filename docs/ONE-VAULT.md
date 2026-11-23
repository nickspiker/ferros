# One vault: the implementation plan

Status: plan, 2026-11-23. Nothing here is built. It maps the owner's existing specs to what is built and lists the steps that close the gap. It invents no design: where a spec is silent or contradicts itself, it says so and leaves the choice to the owner (the decisions at the end).

The owner, 2026-11-23: "There's ONE fixed size element, the seed. the rest is all field. ALL OF IT", and "It better run the seed. that was the whole fucking point. then EVERYTHING ELSE goes in the vault. Nothing special, that way it all rotates and levels properly. We do have a special write on one image only without verification for RAW video we need fast but that's it." The rule and its history: "Claude memories/storage_seed_only_fixed.md".

The specs, in force: RING.md (Zila), SEED.md, VAULT_ROOT.md, VAULT.md, BOOT.md; STORE.md and HOLD-API.md for the hold.

Units: a block is 4 KiB; 2^18 blocks are 1 GiB. Offsets are from the start of the ferros partition. "Built" means on forge 3a4668d8 and manifestus eb7f91e..0eb4f80.

## 0. What departs from the specs today

| Spec says | Built | Where | Carved by |
|---|---|---|---|
| ABL loads the seed; the seed verifies itself, finds the kernel, jumps (SEED.md "Execution Flow", "Android Integration"; BOOT.md "Dual Boot") | ABL loads the gate (the hv, Linux's Image, the ferros kernel with its vault capsule carried) and the hv jumps to the kernel; the seed never runs | BOOT.md "As built on husky"; HV.md | 2026-10-22 on |
| The tract is the rest of the device (RING.md "Partition Layout", VAULT.md "Tract Capacity") | the vault process's tract is a 16 GiB window, two 8 GiB copies | ferros_kernel/src/process.rs:4503, payloads/vault/src/main.rs:95-96 | b922919c, 684f8d3e |
| The ledger is a tenant of the vault (VAULT.md "What Lives In The Vault") | the flash log is a fixed ring of its own at 1 GiB, two copies of 2^14 blocks | ferros_kernel/src/flashlog.rs:26-30 | b4f52efd |
| One vault | the kernel's own vault (kvault) is a second vault in two fixed 256 MiB regions at 2 GiB | ferros_kernel/src/kvault.rs:18-20 | c00d0e80, 3a93cc7e |
| Extents live in the tract (VAULT.md "Extent", "Furrows") | RAW clips are extents in a 48 GiB area beside the vault | process.rs:4508-4509, payloads/vault/src/main.rs:90 | 684f8d3e |
| (nothing) | rawbench, a 32 GiB development area | process.rs:4511 | 684f8d3e |
| The spine migrates (RING.md "Migrating Rings") | the spine is fixed at block 0 of each region, 2^8 entries (manifestus's host profile) | kvault.rs:403 `Vault::open`, payloads/vault/src/main.rs:918 `open_lazy` | |
| DISCARD on 4 MiB-aligned wrap boundaries (RING.md "DISCARD Strategy") | never issued (`BlockDev::discard` exists, nothing calls it) | manifestus src/block.rs:48 | |

What follows the specs already: manifestus is the engine (VAULT.md "Engine Migration", done), with the two-cursor tract, the plow, the windowed reap, the fence, extents and furrows, the spine, write-verify-then-mirror with a cache flush per commit, and the migrating-ring code (`open_migrating`, the root ring, `migrate_ring`) built and tested but unused. The pre-boot log buffer is as BOOT.md says. The stem's ring search exists in ferros_seed and ferros_hal ring.rs, with tools/install writing ferros_layout's fixed table.

## 1. Spec by spec

### RING.md "Partition Layout" and VAULT.md "Storage Layout"

- **The spec:** seed copies, then the stem, spine, state ring and ledger ring at fixed blocks, then the tract.
- **Superseded within the specs themselves:** RING.md "Migrating Rings" moves the spine out of a fixed place. VAULT.md says the ledger is a tenant of the vault, and RING.md "Spine Entry Format" says cap tables, process snapshots and display state are vault objects reached thru the HAMT root, not a separate ring.
- **The plan follows those:**
  - the state ring and the ledger ring become vault content;
  - the spine migrates;
  - the only fixed pieces left are the seed and the root ring's four slots (RING.md "The wear tree", level 0).
- **The stem** is the one place the specs disagree with the owner's rule:
  - RING.md's table puts it at a fixed block;
  - VAULT.md "What Does Not Live In The Vault" says "Kernel code: stem (kernel ring), not a vault concern";
  - the owner, 2026-11-23, puts the kernel ring in the vault.

  Decision 0.
- **Steps:**
  - ferros_layout's fixed table is deleted once nothing reads it (tools/install, ferros_hal ring.rs, ferros_seed).
  - RING.md's and VAULT.md's tables are rewritten to the seed, the root ring and the tract. These are the owner's specs, so decision 3.

### RING.md "Migrating Rings (dumb-flash profile)"

- **The wear tree, as specced:**
  - level 0: the root ring, 4 slots, fixed, written once per migration;
  - level 1: the spine, migrating, written once per commit;
  - level 2: the tract, the rest of the device, written once per lap.
- **Built:** manifestus `open_migrating`, `RootEntry`, `read_root`/`append_root`, `migrate_ring` with the A/B ordering (manifestus src/vault.rs:415, src/ring.rs:544-700).
- **Departs:** ferros opens the host profile (spine at block 0, R = ∞).
- **The residence pool:** RING.md specs v1 as a dedicated band after the root ring (`[root: 4][band: M x N][tract]`), which is a fixed band, and an endgame where residences come from the tract's own clean space via the reap. The endgame is "deliberately deferred until the raw-NAND target is real". The owner's rule (no fixed band) asks for the endgame now. Decision 1.
- **Steps:**
  - Open the one vault with `open_migrating`.
  - If decision 1 is the endgame, manifestus allocates the next residence at the plow, as a run of N clean blocks reserved like an extent. The reap hands a retired residence back as clean space. The root entry records `at` and `was` as today.

### RING.md "Mirror Protocol" and BOOT.md "Mirror Media"

- **The spec:** two devices at identical block addresses, written UFS first, verified, then the second. On the Pixel, which has no SD slot, BOOT.md "Mirror Media" makes the second copy "on UFS (placement-diverse)".
- **Built:** manifestus's mirror, batched in runs (9ecc1ff), with a cache flush per commit (a0a7056e), over two fixed regions.
- **Plan:** the two copies are the two halves of the partition after the seed and the root ring, each half's length taken from the GPT at boot. Tract position p is block p of each half: identical addresses, placement-diverse.
- **Resync** (RING.md "Resync protocol") is manifestus's degraded-mirror path.

### RING.md "Dual-Slot (A/B)" and BOOT.md "Dual Boot (Pixel 8)"

- **The spec:** both boot slots identical, holding the seed. Kernel A/B selected by `active_slot` in the vault root entry, falling back to the previous kernel when the new one fails.
- **Built:** both slots hold the identical gate, but the gate carries the kernel, not the seed. The hv came after these specs and stays: it is Android's path and the chords' (HV.md).
- **Plan:**
  - The gate carries the hv, Linux's Image and the seed in place of the kernel. On ferros's path the hv jumps to the seed, as it jumps to the kernel today.
  - Both slots stay identical and are updated B, verify, then A.
  - Kernel A/B moves into the vault: two kernels kept, `active_slot` in the spine entry as RING.md specs it.

### RING.md "DISCARD Strategy" and "Wear Distribution"

- **Plan:**
  - When the reap retires a window, manifestus issues `discard` for every 4 MiB-aligned span the retirement completed, on both halves.
  - The UFS HAL gains UNMAP.
- **Wear** then follows the spec as it stands: strict rotation of the spine; the tract written once a lap; cold data moved by the reap once a lap (VAULT.md "The Reap").

### SEED.md

- **"Execution Flow":**
  - **Built:** ferros_seed (b503d534): self-check, a binary search of the stem at a fixed base, jump.
  - **Gap:** step 3, "kernel stored at known LBA", becomes VAULT_ROOT.md's arithmetic. The seed reads the root ring, then binary-searches the spine residence it names for the newest valid entry. That entry carries `kernel_hash` and `kernel_sig` (RING.md "Spine Entry Format"). The seed then reads the kernel and checks both.
  - **Where the kernel's blocks are:** the spec's spine entry names the kernel's hash and signature but not its place. Decision 0 picks the place.
- **"Storage Layout":**
  - The spec has seed copies A and B on the partition (G#400, G#800) and, on the Pixel, the seed loaded by ABL from the boot slots ("Android Integration").
  - Built: the partition copies are written only by tools/install.
  - Decision 2: whether the Pixel keeps the partition copies or its two boot slots are the seed's two copies.
- **"Public Key Management":** key rotation as specced, done on the boot slots, B then A.
- **"Code Budget"** (under 512 lines, under 16 KiB): the seed already binary-searches a ring. With decision 0's recommended form it adds only the root entry and the spine search, and stays in budget.

### VAULT_ROOT.md

- **"Finding Most Recent Valid Block" and "Finding Oldest Block":** built as the ring binary search in manifestus, ferros_seed and ferros_hal.
- **Its "Open Questions":**
  - **The representative block and mirror interaction:** manifestus answers both (the ring's own search; the higher valid generation of the two copies wins, then resync).
  - **N, the spine's entry count:** still open. RING.md says 65536; ferros runs 2^8. Decision 4.

### VAULT.md

- **"What Lives In The Vault":** the plan moves in everything VAULT.md lists:
  - **The ledger:** the flash log's entries become vault values. The kernel keeps its pre-boot buffer as BOOT.md says until it holds the vault.
  - **Boot snapshots and capability state.**
  - **App data:** already the vault process's domains.
  - **kvault, folded in:** its names (init, sealed, capsules, the kernel's scopes) become domains of the one vault, as VAULT-OUT.md already plans.
- **"Storage Architecture", "The Plow", "The Reap", "The Rollback Fence":** built in manifestus. Since 2026-11-23 it also has:
  - the reap ahead at the idle edge (75da4c17);
  - full as TractFull with deletes still possible (06bafbc, 0eb4f80).
- **"Tract Capacity":** the spec counts the tract as the device less the fixed regions; it becomes the device less the seed and the root ring.
- **"Extent" and "Furrows":** built. RAW clips join the tract (section 2).

### BOOT.md

- **"Boot Stages":** "seed verifies kernel, jumps", then Stage 2, the spine scan. Today the kernel opens kvault, and the vault process opens its own vault.
- **Plan:** the kernel scans the one vault's spine. It holds the vault itself (writing its early ledger lines) until the vault process starts, then commits, closes it and hands it over: one engine at a time.
- **"Pre-Boot Log Buffer":** as built.
- **"Genesis Boot":** the genesis plow is the tract's start after the root ring (the spec wrote G#C0000).

### STORE.md and HOLD-API.md

Unchanged. The hold's items are vault values. A full vault answers `Error::Full` (174c79c5). A RAW clip's body is an extent (section 2).

## 2. RAW clips, the one special write (the owner's decision, 2026-11-23)

The owner: split across both rings, one half in each, with a custom marker recording the split; one copy, no read-back verify.

- **A clip is an extent value in the tract** (VAULT.md "Extent"). Recording reserves ceil(n/2) clean positions at the plow. The recorder writes the clip's first half to half 0's blocks at those positions and its second half to half 1's, straight to the flash: one copy, no read-back, no cache flush per frame.
- **The marker:** the clip's leaf is an ordinary extent leaf plus one field saying the extent is split. It is mirrored and verified like every leaf. Its runs name the positions once; the split field says that ring 0 holds bytes 0 to n/2 and ring 1 the rest.
- **Sealing:** at the clip's end, its leaf is committed with its length and BLAKE3, after the cache flush that takes every frame to the medium (the flush per commit, a0a7056e).
- **The reap:** a split extent's positions are live on both halves with different contents.
  - The reap moves it as a run, reading each half's part from its own ring and writing it to the same ring at the plow, again split, again one copy.
  - The move is an ordinary reap write, read back and compared (VAULT.md "The Reap": survivors are verified writes); only the recorder's own write skips the read-back.
  - Its BLAKE3 is checked once on the move.
- **Reads:** the mirror's read path gains "this position from this ring" for split extents (today it reads the first healthy copy).
- **A lost half** loses that half of every clip: the price of one copy, stated, not hidden.
- **manifestus:** extents move from the separate area (src/extent.rs, a table of stretches outside the tract) into the tract: reserve at the plow under a placeholder leaf, the split field, the reap's split move, the per-ring read.

## 3. The kernel's early boot

- The seed reads only what it needs: the root ring, the spine's newest entry, and the kernel as decision 0 places it. It writes nothing and holds no keys. The kernel is code, not a secret, and carries the owner's signature (SEED.md).
- The kernel reads the vault process's capsule, init and the USB rescue the same way. These are signed values, so they are readable before keys are held.
- Sealed values wait for the ira, as they do now.

## 4. RAM: the same rule

The owner, 2026-11-23: "if I open notepad and it requests 1mb ... then I go and open a video ... and it malloc's 4GB of RAM, it better work."
- **Today:** declared ceilings: each payload's heap size (`HEAP_BYTES` in 11 payloads, e.g. payloads/vault/src/main.rs:26 at 56 MiB, payloads/beamtest/src/main.rs:20), each arc's ceiling (`diag arcs` "ceiling"), and the kernel heap (ferros_kernel/src/main.rs:186, 12 MiB).
- **Plan, a separate step:** heaps grow by asking the kernel for frames as they use them (arcs already grow, as `diag arcs` "grew" counts show), up to physical memory, and the declared caps are removed. Its own plan follows once the storage steps are under way.

## 5. The migration: describe first, nothing lost

On the Pro:
0. **Backups,** hashed on both ends into keys/<device>/:
   - the hold's 205 stills (`items get`);
   - the vault process's window;
   - kvault's regions;
   - the flash log;
   - the clips area's taken extents (921,562 blocks);
   - both boot slots.
1. **At the desk,** each part host-tested:
   - manifestus: whole-partition halves, `open_migrating` (decision 1), split extents in the tract, `discard`;
   - the seed's root ring and spine read (decision 0);
   - the hv's jump to the seed;
   - the kernel's sequential hold of the vault.
2. **The new vault beside the old,** its plow started at a position whose blocks in both halves hold nothing old:
   - on the Pro that is from 28 GiB into half 0, which is about 92 GiB into the partition in half 1 (the old rawbench, which holds nothing to keep);
   - content moved by value thru the running vault processes (every domain, every item, the clip as a split extent), each checked by hash on arrival;
   - kvault's names and the flash log's entries moved in as domains and ledger values.
3. **The kernel into the vault** as decision 0 places it, and both boot slots flashed with the gate that runs the seed. A cold boot then runs the seed into the new vault.
4. **The old regions** stay untouched until the plow laps around to them (36 GiB of positions on the Pro), so a rollback to the old slots finds everything as it was.
5. **Every constant in section 0** is deleted, and RING.md's and VAULT.md's tables are rewritten (decision 3).

The 8a, the same: its 77.7 GiB partition, the plow started past its old regions, and its flash log moved from Android's `klog` into the vault as ledger values.

### The 205 stills until then

- The Pro's vault is full at 8 GiB a copy. Copy the stills off now: that is step 0's backup.
- Delete only to shoot before step 3 boots. With 0eb4f80 and 174c79c5 on the Pro, removals land in the full state, and about 0.5 to 1 GiB a copy must go before a new still fits.
- After step 3 nothing needs deleting.

## Decisions for the owner

0. **The kernel in the vault, against VAULT.md's "Kernel code: stem, not a vault concern".** The owner's rule says in. Recommended form: the spine entry, beside its `kernel_hash` and `kernel_sig`, gains the kernel's runs. The seed then reads root ring, spine, kernel, and walks no index, which keeps it within SEED.md's budget. The kernel is an ordinary extent value the reap moves. The alternative: the kernel as a value under a well-known key, which the seed finds by walking the HAMT.
1. **Spine residences from the tract now** (RING.md's endgame), or the v1 fixed band after the root ring, which the spec chose first.
2. **The Pixel's seed copies:** its two boot slots as the seed's two copies (as SEED.md's "Android Integration" has ABL load it), with or without the partition copies at G#400 and G#800.
3. **Rewrite RING.md's partition table and VAULT.md's "Storage Layout"** to the seed, the four-slot root ring and the tract (the specs are the owner's to change).
4. **The spine's entry count N:** RING.md's 65536 (256 MiB a residence) or ferros's 2^8 today.
