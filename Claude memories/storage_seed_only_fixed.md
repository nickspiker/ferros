---
name: storage-seed-only-fixed
description: "Owner 2026-11-23 - on the ferros partition the seed is the ONLY fixed-size element; everything else (vault mirrors, clips, logs, kernel's own stores, benches) is a field in one pool, sized as it needs, described by VSF; never carve fixed windows"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-23T17:38:27.761Z
---

Owner, 2026-11-23, after the Pro's vault filled at 8 GiB while ~100 GB of the 128 GB partition sat in other fixed windows: "Fixed areas? HOW THE FUCK DID THAT EVEN HAPPEN!!!" ... "Did you read VSF? Like at all?" ... "There's ONE fixed size element, the seed. the rest is all field. ALL OF IT".

**The rule:** the seed (ferros_seed, the trust anchor) is the one fixed-size element on the ferros partition. Everything else is a field: the vault's two mirrors, RAW clips, the flash log, the kernel's own vault (kvault), any bench or scratch area, the kernel ring itself. Each takes extents from one pool as it needs them, no wider than it needs (VSF's "as wide as it needs to be and no wider", no ceiling), and the map of what is where is self-describing VSF on the disk, never a constant table in the kernel.

**Why:** fixed windows were carved one step at a time by agents (b922919c 2026-11-07 the vault's 8 GiB block window; 684f8d3e/c666cf57 2026-11-19 clips 48 GiB and rawbench 32 GiB), each locally reasonable, none against VSF's founding rule, none shown to the owner; the vault ran out with most of the disk idle and 205 time-lapse stills were refused.

**How to apply:** never add a fixed-size region, window, ring or table on persistent storage (other than the seed). Anything that needs space asks the pool and grows; a size in a constant is a bug. Layout-level designs go to the owner first ([[display_driver_design_first]] applies to storage too). Related: [[vault_flush_per_commit]], [[feedback_one_line]].
