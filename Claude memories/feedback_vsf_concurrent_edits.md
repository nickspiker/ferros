---
name: feedback-vsf-concurrent-edits
description: vsf is edited live alongside ferros (path deps everywhere); when a build breaks inside vsf, never touch it — wait/retry or message the vsf session; don't pin or snapshot vsf (owner chose coordination, 2026-10-20)
metadata:
  type: feedback
---

ferros, manifestus, ihi and the rest depend on `../vsf` by path, so every build compiles the owner's live vsf working tree, mid-edit states included. The owner (and a separate "VSF" Claude session) edit vsf concurrently.

**Why:** the owner iterates vsf and its users together; a git pin or a worktree snapshot would make every vsf change an explicit bump, and a snapshot would need every crate in the graph repointed (Cargo rejects two vsf sources). Owner picked plain coordination over both on 2026-10-20.

**How to apply:** if a build fails inside vsf, don't edit, stash or revert anything there — note it, do work that needs no rebuild, retry later, or send the vsf session a message (it reaches this one too). Version requirements on vsf follow what vsf's Cargo.toml says now: on 2026-10-20 the 0.11/0.12 bumps were folded back into one unreleased 0.10.0 (nothing past 0.9.3 was published), so siblings say `version = "0.10"`. Related: [[feedback-commit-every-good-point]].
