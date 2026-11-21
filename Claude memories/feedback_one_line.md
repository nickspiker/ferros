---
name: feedback-one-line
description: Owner 2026-11-20 - ferros is one straight line: forge only, zero branches, no worktrees; agents commit straight to forge (fetch, rebase, push), one shared build farm, deleted leftovers
metadata:
  type: feedback
---

Owner, 2026-11-20, on finding ~200 agent worktrees, 374 local and 124 remote branches, and Harbor 97% full: "I'd prefer to be on one straight line and it sounds like it got shrapneled pretty hard. Delete all, yes, should be zero branches."

**The rule:**
- ferros has one branch, `forge`, locally and on origin. No feature branches, no tags as stand-ins for branches.
- Work lands as linear commits on forge: fetch, rebase, push. No merge commits.
- No per-agent worktrees: never `isolation: "worktree"` for ferros agents, never `.claude/worktrees`, never `target/agent-<name>` farms. One shared build farm; an agent that must not touch the owner's main checkout uses that one farm, sequentially (staged agents, [[feedback_keep_devices_busy]]).
- Anything left over is deleted once its work is on forge.

**Why:** the branches and worktrees scattered work (real GPS work sat on unmerged branches for days), caused collisions and confusion, and filled Harbor: each farm was 2 to 50 GB of build output, and the hourly btrfs snapshots kept every deleted build too.

**How to apply:** before starting an agent, give it the shared farm and the instruction to commit straight to forge. Held-back work (not for forge yet) goes in a git bundle, not a branch. The 2026-11-20 collapse kept a backup at /mnt/Harbor/Code/ferros-branches-2026-11-20.bundle (photon-ferros-tcp 22818d4a is held there, Photon's call).

Related: [[feedback_keep_devices_busy]], [[feedback_commit_every_good_point]].
