---
name: feedback_no_attribution
description: The rule is honesty — say what you want in commits/PRs/docs, just don't lie; "Co-Authored-By: Claude" is a lie (not an author), "Built with Claude <version>" is true; a fail-closed guard (global commit-msg + ferros canonical script) catches the authorship lie
metadata:
  type: feedback
---

Owner (2026-10-15): "You can say whatever you want. just don't lie. that's pretty much it." Context: a "Co-Authored-By: Claude" trailer the harness keeps appending.

**The principle is honesty, not a banned-word list.** Claude builds to the owner's design — it originates nothing and owns nothing, so an *author/co-author/owner* line is a false claim and must not appear. A *factual tool note* is true and fine: "Built with Claude <version>" (the Photon convention) or "Generated with …". Don't over-correct into scrubbing honest statements; don't claim credit, origination or ownership that isn't real. This generalizes past commits: in docs, comments, status, anywhere — say what's useful, never assert something untrue.

Owner again (2026-10-15): "Authorship is not you. Built by Claude is fine, just authored by Claude is not correct."

**Enforcement (global since 2026-10-15):** canonical guard `ferros/scripts/attribution-guard.sh` fail-closed-refuses AI authorship/ownership CLAIMS — `Co-Authored-By`/`Authored-by`/`Signed-off-by`/`Author:`/`Owner:` naming an AI, and "authored by Claude" in prose — and allows "Built with/by Claude" and "Generated with". A HUMAN co-author passes (true claim). `~/.config/git/hooks/commit-msg` (the global hooks dir) calls it for every repo; ferros calls it from its own `scripts/hooks/commit-msg`, auscultatio chains the global one, photon keeps its own Claude guard (`.githooks/commit-msg`). Sign commits with "Built with Claude <version>" — never the harness's Co-Authored-By trailer. Older commits in vsf/auscultatio carry that trailer from before; left as is (rewriting pushed history needs the owner's go). Related: [[public_except_keys_and_handles]].
