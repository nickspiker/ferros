---
name: pucks-draw-squares
description: Puck drawing path decided 2026-11-18: display asks with diameter + turn, puck answers a square of G2t16 pixels, display trims + places; no op list; build a fresh clock puck and delete the display's wired-in clock
metadata:
  type: project
---

Owner, 2026-11-18: "What's wrong with supplying a diameter and having the kist return a square vector in sixteen bit that ferros will trim and place?" That replaced the drawing-ops list proposed in docs/PUCKS.md. Never re-propose an op list.

- The ask: diameter, turn (the gravity turn every circle has), sub-pixel offset, theme, focus. The answer: a square of VSF RGB g2t 16-bit pixels (fluor `G2t16`, op 22 format 1), which has HDR headroom above white (owner: "pucks can do bright HDR for a gallery puck or video"). A mailbox layer; the display masks by the circle and composes `under`.
- Rate: no limit. During a gesture the display scales and turns the newest square and asks again each shown frame; sharp again when the puck catches up. A continuously moving puck (the clock's rings) renders on shown frames (10 Hz idle, panel rate focused); others on their own content deadline.
- The deck's circles never cover more than the screen (owner), so the pucks' pixels and work are bounded by about one screen; off-glass pucks draw nothing.
- Owner: "ideally we just build this fresh and make another clock proper, then delete the weird wired in one." The display's `WIDGET_CLOCK` code and the map's `widget("Clock")` go once the fresh clock puck matches it.

**Why:** nothing parsed (a buffer, not a language), fluor draws everything, the circle mask is the puck's right.

**How to apply:** open questions 3 to 12 live in docs/PUCKS.md; check there before building rights, slots or lifecycle. See [[ferros-vocabulary]], [[fluor_pixel_depth]], [[feedback-no-animations]].
