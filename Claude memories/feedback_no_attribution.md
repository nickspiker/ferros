---
name: feedback_no_attribution
description: The rule is honesty — say what you want in commits/PRs/docs, just don't lie; "Co-Authored-By: Claude" is a lie (not an author), "Built with Claude <version>" is true; a fail-closed guard in ferros catches the authorship lie
metadata:
  type: feedback
---

Owner (2026-10-15): "You can say whatever you want. just don't lie. that's pretty much it." Context: a "Co-Authored-By: Claude" trailer the harness keeps appending.

**The principle is honesty, not a banned-word list.** Claude builds to the owner's design — it originates nothing and owns nothing, so an *author/co-author/owner* line is a false claim and must not appear. A *factual tool note* is true and fine: "Built with Claude <version>" (the Photon convention) or "Generated with …". Don't over-correct into scrubbing honest statements; don't claim credit, origination or ownership that isn't real. This generalizes past commits: in docs, comments, status, anywhere — say what's useful, never assert something untrue.

**Enforcement:** `scripts/attribution-guard.sh` + `scripts/hooks/commit-msg` (committed core.hooksPath) fail-closed-refuse authorship/ownership CLAIMS (Co-Authored-By, Authored-by, Author/Owner: Claude) and allow "Built with"/"Generated with". Mirrors photon/.githooks/commit-msg. Related: [[public_except_keys_and_handles]].
