---
name: feedback_commit_every_good_point
description: Commit AND push at every good point of progress without asking — uncommitted work gets lost
metadata:
  node_type: memory
  type: feedback
  originSessionId: 75576678-89a2-4635-8c20-1eba33babf4a
  modified: 2026-10-15T02:59:26.917Z
---

Commit and push at every good point in the work (a verified fix, a working feature, a doc/terms decision landing), not in batches at the end.

**Why:** the owner has lost work before when progress sat uncommitted ("shit gets lost otherwise"); standing authorization, restated 2026-10-14.

**How to apply:** as soon as something is verified on hardware or a decision is written into docs, commit with an honest message and push. Never stage the owner's own unrelated uncommitted files (e.g. PORTAL.md, transcribe tools) — stage only your hunks. See [[husky_cold_slot_b]].
