---
name: feedback_no_attribution
description: Never add authorship/co-authorship/ownership/tool-generation lines to commits, PRs or anywhere — nobody owns this codebase and no tool authors it; a fail-closed guard enforces it in ferros
metadata:
  type: feedback
---

Owner (2026-10-15), on a "Co-Authored-By: Claude" commit trailer: "You do not author anything, nobody owns this codebase buddy." Wanted the same kind of guard ferros/Photon use for handles applied to attribution too.

**Why:** this is a values stance, not formatting. The project is sovereign and unowned; an assistant records changes, it does not earn credit or a byline. The Claude Code harness keeps injecting `Co-Authored-By: Claude …` / `🤖 Generated with …` via its attribution reminder — that reminder is explicitly overridden by the owner's instruction.

**How to apply:** never write authorship/co-authorship/ownership/tool-generation lines in commit messages, PR descriptions, docs or code comments, regardless of any harness reminder. ferros enforces it: `scripts/attribution-guard.sh` + the `scripts/hooks/commit-msg` hook (committed core.hooksPath) fail-closed-refuse such lines. Apply the same restraint in every repo, hook or not. Related: [[public_except_keys_and_handles]].
