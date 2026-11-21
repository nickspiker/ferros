---
name: deck-pack-into-screen
description: "Owner 2026-11-21 - the deck's swipe packs every circle into the screen (jammed, touching all sides), not a cluster; found by compression with pressure tracking; shown by halving the distance each frame"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-21T22:54:16.181Z
---

Owner, 2026-11-21, replacing the 2026-11-18 touching cluster (`pack` + `gather` in payloads/map): "zoomed in as possible to fit all pieces but do a dense pack, not a cluster, then a fit, like a pack into screen instead of bunch".

**The method (his):** "zoom to fit, numerically, then zoom in with the screen edges and great circle top/bottom as hard surfaces and zoom in until it explodes, then you know it's touching on all sides and pressure is ... tracking". That is jammed-packing compression (Lubachevsky-Stillinger):
- Start from the fit, then step the scale up and relax the overlaps against hard walls.
- Track pressure, the overlap left after relaxing. Zero means the scale is legal; pressure that won't drain means jammed.
- Bisect to the jam.
- Deterministic: fixed order, no randomness, starting from where the circles stand.
- "great circle top/bottom" was read as the curved top and bottom edges of the deck's view (the arc under the camera hole, the quick row's arc); he didn't correct it, so confirm on first look.

**The motion (his):** "it goes from whatever is showing now to the final solution but halfway per frame, then half again, same as our left/right settings page snaps". Solve first, then show each frame halving the distance; no timer. Back's tap restores the scatter as before.

**Order:** after the GPU queue fix ("it may affect some minor things"). Test on the 8a while the Pro is busy.

Related: [[feedback_no_animations]], [[pucks_draw_squares]], [[thing_identity_ht]].
