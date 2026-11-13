---
name: feedback-darkness-space
description: "fluor pixels are α + PREMULTIPLIED darkness, composited front to back with under only; light over black = α − darkness; a glow is light at falling α, never colourless opacity; no Porter-Duff over anywhere"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-07T18:37:53.093Z
---

Owner, 2026-11-07, said "DARKNESS" several times while I got it backwards: I painted the status circles over the home screen with Porter-Duff, gave them a "darkness glow" that was opacity with no colour (a black shadow, invisible on black), and the display decoded fluor pixels as straight darkness, `(ONE − d) × α`.

The model:
- fluor stores α + darkness; `under` deposits darkness × α, so darkness is premultiplied and darkness ≤ α.
- What is still transparent is white in darkness space until something goes under it. Black under it adds `ONE − α`, so the light over black is `α − darkness`.
- Front to back: the frontmost layer goes into an empty accumulator first. "Drawn later" means "behind". The glass edge is first, then the bars, then the scene.
- A glow or shadow is its own coloured layer at a falling α, composed under like anything else.
- Opacity is 16 bits everywhere (text too: display glyphs.rs rasterises at Q16 since 2026-11-07).

**Why:** compositing is filter stacking (fluor README §3). Over is the same operator in the wrong coordinates and the wrong order, and the owner treats it as a plague. The rule is now in fluor's and Photon's AGENT.md ("Compositing: Front to Back, `under` Only").

**How to apply:**
- Before writing any pixel code, read fluor's README §3.
- Never write `a·(1 − c) + b·c` between layers.
- Decode α + darkness as `α − d`.
- In the ferros display, use `glass_front` / `under_pm` / fluor's `Blend::under`.

Related: [[feedback-vsf-rgb-only]], [[display-global-mask-then-layers]].
