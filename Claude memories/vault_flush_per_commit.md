---
name: vault-flush-per-commit
description: Owner 2026-11-22 - every persistent commit flushes the flash chip's own write cache before Ok (SYNCHRONIZE CACHE or FUA); Ok means on the medium, and a hold/store Ok carries copies (2, or 1 when a mirror dropped)
metadata:
  type: project
---

Found 2026-11-22 (vault-write work): ferros never flushed the UFS device's write cache, so "written, read back equal" was equal as the device returned it, and the moment after an Ok rested on the device's power-loss handling. Owner, asked to pick (flush per commit / only big saves / leave it): "first".

**Decided:** a cache flush per commit, measured. Every persistent write that answers Ok flushes the device cache first: the vault process's commits (manifestus), kvault, the clips area's seal (the extent's final commit; clips still single-copy and streaming), the slot record, the flash log, devinfo.

**Related, same day:** the hold's finish reports `copies` (2, or 1 when a mirror is degraded for the session; forge 037afa0f, `Hold::keep_verified`); apps rely on that Ok instead of reading back (owner: "there should be a flag for that if the app wants to know if the vault was successful"). The no_std mirror wrote a 4 KiB block at a time (manifestus 9ecc1ff fixed it); the reap moved to the idle edge (ferros 75da4c17).

Related: [[feedback_measure_before_fixing]], CLAUDE.md "Write-verify-then-mirror".
