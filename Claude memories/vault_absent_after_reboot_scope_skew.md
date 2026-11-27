---
name: vault_absent_after_reboot_scope_skew
description: "An entry 'absent' after a reboot is usually kernel scope skew, not loss: a RAM-reloaded kernel with a scope moved it into the vault process, the flashed kernel without it looks only in its own vault (Pro, cam.*, 2026-11-15)"
metadata:
  node_type: memory
  type: project
---

Found 2026-11-15 on the Pro: `cam.frame` and `cam.boitata.pdma` read absent after the cold boot of b875 and were stored again by hand.
Nothing was lost.
b872's flashed kernel had no `cam` scope, so the bridge put kept them in the kernel's vault; b873, the camera kernel RAM-reloaded, knew `cam` and moved them into the vault process (migration); the flashed kernel again on b875 looked only in its own vault.
The flash log's `store remove "<name>" (process)` line was the migration's removal, not a delete.

**Why:** which names live in the vault process is each kernel's built-in `STORES` table; a RAM reload leaves the flashed kernel, with its own table, for the next boot.
The harm it can do: an older copy re-put by hand under the older kernel is laid over a newer vault-process value by the next migration.

**How to apply:** when a vault entry "vanishes" after a reboot, compare the kernels' scopes first (`git log -G'StoreScope'`), and grep the flash log (`diag "log 256"`) for `store moved` / `store remove ... (process)`; never re-store before checking.
Since the fix (VAULT-OUT.md "Kernels that know different scopes"), a bridge get or rm of an unknown-scope name reads thru to the vault process, and migrations log `store moved "<name>" into the vault process, scope <s>`.
Building in a worktree: `.claude/worktrees/tapuwae` is another agent's copy pointing at its own valhall; build thru a sibling symlink farm in the scratchpad with `--manifest-path`, and copy the ignored `data/oracle/display/husky-hk3/cold-power/*.bin` into the worktree.
