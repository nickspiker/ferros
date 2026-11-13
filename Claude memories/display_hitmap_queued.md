---
name: display-hitmap-queued
description: "Queued 2026-11-11: the display composes a u16 owner hit map (fluor's HitId model) for touch routing, written at the layer that makes a pixel opaque (early out); dev view shows the map every 4th frame, toggled by a developer checkbox or swiping up from back and home together"
metadata:
  type: project
---

Owner, 2026-11-11, on touch routing (today op 27 walks windows front to back reading their pixels' clear bit; op 41 recomputes the system's geometry): "I mean, I already built it in fluor so sure?" (fluor: `HitId = u16`, one per pixel, written by each widget as it paints, `render_content_into(.., hit_id)`, read by the host). Then: "the hitmap is never even touched if the pixel is opaque. early out! Also, let's add a debug/devel checkbox for toggling on/off the hitmap on the fourth frame so we get three frames normal and fourth is always hitmap so we can see both. Quick shortcut in dev only is swipe up from both back and home to toggle on/off."

**Build (after the window-reap fix lands, same display code):**
- The display's front-to-back compose writes the owner's u16 id once, at the layer that makes the pixel opaque, then stops (nothing behind touched); ids for app windows, console circles, the camera doorway, quick-row circles, back/home halves.
- op 27 and op 41 become one read of the map; the hand-kept geometry copy goes. Two levels: display map picks the window, the app's fluor map picks the widget.
- Full resolution (8 MB on husky): the display grows its arc via ARCS (SYS_ARC 40) rather than squeezing its 32 MiB heap.
- Dev only: every 4th presented frame shows the map (flat colour per owner) instead of the image; a developer-page checkbox and a two-finger swipe up from back and home together toggle it.
- On that view, any redraw of the map picks a new colour palette (owner, 2026-11-11: "on that fourth frame, any redraw to the map should trigger a new colour palette"), so every change of ownership shows as a colour change.

Related: [[display-global-mask-then-layers]], [[feedback-darkness-space]].
