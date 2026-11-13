---
name: feedback-check-published-before-bump
description: "Before any version bump (vsf or any crate), check what is actually published; an unpublished version absorbs changes. Never leave a superseded version in commits"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-11T00:52:09.955Z
---

Owner, 2026-11-10, after vsf's x codebook reissue was bumped to 0.12 by following build.rs's "any codebook change bumps MAJOR" rule while crates.io's newest was 0.10.2 and 0.11 had never shipped: "you thought you'd break everything with a version bump for fun or what? ... And when v11 hasn't even shipped???" and "Make sure we don't have any v12 shit in commits, thas fucks things up."

The bump broke every crate pinning `vsf = { version = "0.11", path = ... }` (fgtw, fluor, tapuwae, toka, kete, tohu, manifestus, opsin, limbus, ferros ahau). They all build from the shared local folder, so the merge alone broke other sessions' builds.

**Why:** a version is for what has shipped. Bumping past an unpublished version breaks every pin for nothing, and leaves dead version numbers in history.

**How to apply:**
- Before bumping, check crates.io (`curl https://crates.io/api/v1/crates/<name>`) and what is pinned downstream.
- An unpublished version takes the change, with its changelog entry folded in.
- A version rule in a build script ("bump MAJOR") means "relative to what is published".
- If a wrong bump gets pushed, replace it in history: squash, then force-push with a lease on your own commit. Don't stack a revert on top.
- Never merge a version change into a shared local repo before every dependent is ready.

Related: [[feedback-vsf-concurrent-edits]].
