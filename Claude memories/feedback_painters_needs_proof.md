---
name: feedback-painters-needs-proof
description: "any painter's-order write (later overwrites earlier, pixels or hit map) needs a stated proof, like a bounds check; front to back with claim-if-empty is the default"
metadata:
  type: feedback
---

Owner, 2026-11-17, after the deck's helm lost taps to circles under it and fluor's hit map turned out last-writer-wins everywhere: "LIke any painters algorithm needs a proof. there's a few onezie twozies that make sense but not many."

**Why:** painter's order hides bugs the way a defensive clamp does: the picture can look right while the hit map, or a later layer, quietly takes what belongs to the front (payloads/display hit.rs, fixed on helm-front-to-back; fluor's widgets, branch hit-front-to-back).

**How to apply:** the default for pixels and hit maps alike is front to back, the frontmost claims first, a claimed pixel is never written again (AGENT.md "Compositing"). A write that overwrites what an earlier write left (last writer wins, a lay-over, a stamp after) needs a WHY/PROOF comment stating why order cannot matter there, as Rule 0 asks of bounds checks: for example the regions are proven disjoint, or the buffer is write-once and nothing earlier was written. No proof, convert it. Related: [[feedback_darkness_space]].
