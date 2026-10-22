---
name: feedback-no-animations
description: "ferros UI is instant — no animations, no transition timers, no time-windowed gestures"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 75576678-89a2-4635-8c20-1eba33babf4a
  modified: 2026-10-22T20:39:03.914Z
---

No animations in the ferros UI: "instant, always, no timers" (owner, 2026-10-22, after the map's 16-frame fly-to on tap).

**Why:** the owner dislikes animation; a change of view should simply be there on the next frame.

**How to apply:** camera jumps, view changes, fronts/backs appear in one frame; gestures are recognised by movement and finger count, never by a time window (a tap = down and up without moving past the slop, however long it took). Live data refreshing (the dashboard's facts) is not an animation. Related: [[settings-identity-to-device-only]].
