---
name: storage-seed-only-fixed
description: Owner 2026-11-23 - the seed is the ONLY fixed element and must RUN at boot; EVERYTHING else (kernel ring, flash log, kvault, clips, all of it) lives IN the vault so it all rotates and wear-levels; the one special path is RAW video's single-copy unverified fast write
metadata:
  type: feedback
---

Owner, 2026-11-23, after the Pro's vault filled at 8 GiB while ~100 GB of the 128 GB partition sat in other fixed windows: "Fixed areas? HOW THE FUCK DID THAT EVEN HAPPEN!!!" ... "Did you read VSF? Like at all?" ... "There's ONE fixed size element, the seed. the rest is all field. ALL OF IT" ... "if I have 128GB free and I take a video that jams up after ten seconds... completely and utterly unacceptable AND in direct opposition to ALL of my design philosophy." Then, on the design: "It better run the seed. that was the whole fucking point. then EVERYTHING ELSE goes in the vault. Nothing special, that way it all rotates and levels properly. We do have a special write on one image only without verification for RAW video we need fast but that's it."

**The rule:**
- **The seed** (ferros_seed, docs/SEED.md) is the one fixed-size element on the ferros partition, and it RUNS on every boot: it verifies itself, finds and verifies the kernel, jumps. A boot path that skips it (as the Pro's and the 8a's did through 2026-11-23: ABL to the boot partition's hv gate and kernel) is wrong.
- **Everything else lives in the vault**: the kernel ring, the flash log, the kernel's own store, settings, documents, stills, RAW clips, benches. No regions, no windows, no pool of separate fields: one vault over the whole partition beside the seed, so every write rotates through the ring and the flash wears evenly. Sizes come from what is stored, never a constant (VSF: as wide as it needs, no ceiling).
- **One exception, RAW video**: its frames take a special write, one copy only and no read-back verification, because it must be fast. It is still in the vault (rotating with everything else); only the copy count and the verify differ.

- **It is already specced:** RING.md (layout, Migrating Rings: the 4-slot root ring is the one small fixed piece beside the seed, the spine migrates, the tract is the rest of the device), SEED.md, VAULT_ROOT.md, VAULT.md (the reap trails the plow and keeps a clean window ahead; the ledger is a vault tenant). Build to those specs; never invent a parallel design.
- **RAW clips** (owner, 2026-11-23): split across both rings, one half in each, with a custom marker recording the split; one copy, no read-back verify.
- **RAM too:** no declared ceilings; a process gets memory as it uses it, up to physical RAM ("if I open notepad and it requests 1mb ... then I go and open a video ... and it malloc's 4GB of RAM, it better work").

**Why:** fixed windows were carved one step at a time by agents (b922919c 2026-11-07 the vault's 8 GiB window; 684f8d3e/c666cf57 2026-11-19 clips 48 GiB and rawbench 32 GiB; b4f52efd the flash log; c00d0e80/3a93cc7e kvault), none against VSF's founding rule, none shown to the owner; the vault ran out with most of the disk idle and 205 time-lapse stills were refused.

**How to apply:** never add a fixed-size region, window, ring or table on persistent storage other than the seed. Anything that needs space is a vault value. Layout-level designs go to the owner first. Related: [[vault_flush_per_commit]], [[feedback_one_line]].
