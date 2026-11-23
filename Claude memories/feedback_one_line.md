---
name: feedback-one-line
description: "Owner 2026-11-20 - ferros is one straight line: forge only, zero branches, no worktrees; agents commit straight to forge (fetch, rebase, push), one shared build farm, deleted leftovers"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-21T23:11:34.032Z
---

Owner, 2026-11-20, on finding ~200 agent worktrees, 374 local and 124 remote branches, and Harbor 97% full: "I'd prefer to be on one straight line and it sounds like it got shrapneled pretty hard. Delete all, yes, should be zero branches."

**The rule:**
- ferros has one branch, `forge`, locally and on origin. No feature branches, no tags as stand-ins for branches.
- Work lands as linear commits on forge: fetch, rebase, push. No merge commits.
- No per-agent worktrees: never `isolation: "worktree"` for ferros agents, never `.claude/worktrees`, never `target/agent-<name>` farms. One shared build farm; agents build in that one farm, sequentially (staged agents, [[feedback_keep_devices_busy]]).
- The farm is /mnt/Harbor/Code/ferros/target/farm/ferros (a detached worktree; siblings symlinked beside it). ONE agent in it at a time: on 2026-11-21 two ran there at once, one pushed the other's commits and popped a stranger's stash. A second job waits, or works read-only. Never `git stash` in it.
- Across sessions: anyone may BUILD in the farm with their own CARGO_TARGET_DIR under target/farm/ (e.g. target/farm/lumis-target), reverting any tracked file a build dirties. Git work (edits to tracked files, checkout, commit, push) takes the lock first: `mkdir target/farm/GIT-LOCK` (fails if held) with a `holder` file naming the session or agent; `rm -r` it once pushed.
- Inside the farm `cargo el0` fails (the main checkout's .cargo alias array is appended to the farm's); run the expanded command. A string alias does not fix it (cargo refuses to merge a string into the main checkout's array).
- Anything left over is deleted once its work is on forge.

**Why:** the branches and worktrees scattered work (real GPS work sat on unmerged branches for days), caused collisions and confusion, and filled Harbor: each farm was 2 to 50 GB of build output, and the hourly btrfs snapshots kept every deleted build too.

**How to apply:** before starting an agent, give it the shared farm and the instruction to commit straight to forge. Held-back work (not for forge yet) goes in a git bundle, not a branch. The 2026-11-20 collapse kept a backup at /mnt/Harbor/Code/ferros-branches-2026-11-20.bundle (photon-ferros-tcp 22818d4a is held there, Photon's call).

Related: [[feedback_keep_devices_busy]], [[feedback_commit_every_good_point]].
- **/mnt/Harbor/Code/ferros is forge's working copy, not the owner's own checkout** (owner, 2026-11-23: "I don't branch bud. that checkout was never authorized." ... "I swear we talked about this."). It must sit on forge, current: on 2026-11-23 it was found 408 commits behind with a duplicate local commit (163fc220, already on forge as 000d3f54) and other sessions' uncommitted edits on top, and Photon's deploy built a stale std/photon from it. Keep it fast-forwarded to origin/forge; uncommitted work in it gets committed to forge by its owner or removed, never left.
