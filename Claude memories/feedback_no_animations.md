---
name: feedback-no-animations
description: "NO TIMERS in ferros: no waits, holds, delays, animations or time windows; sole exception = the dev gate's 1 s auto-answer (production waits for the press forever)"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 75576678-89a2-4635-8c20-1eba33babf4a
  modified: 2026-11-05T19:26:17.578Z
---

No timers in ferros: "instant, always, no timers" (owner, 2026-10-22, after the map's 16-frame fly-to on tap). Restated with force on 2026-11-05: "NO TIMERS!!!! … that's literally a rule. the ONLY … EXCEPTION is for the 1s wait screen so I can halt the boot and that's ONLY in dev mode, outside dev it hangs on that screen forever until the user presses start. We may make it immediately boot but that's undecided."

**Why:** the owner dislikes animation, and a timer hides the real cost. A Claude session on 2026-10-22 turned the "<1 s boot" target into a timer: home started at press + 1 s, so every boot took at least a second, and the owner could not see why. It was found and removed on 2026-11-05 (commit 7b67a69). The same session had also made "1 s per turn" the boot ring's scale (96370a3), and a later session kept it as if the owner had chosen it.

**How to apply:**
- Never propose or add a wait, hold, delay, "show it for a moment", fixed budget, or timed trigger. Things happen when their dependency is met, never at a time.
- The only exception is the dev build's 1 s auto-answer at "Ready?"; production waits for the press.
- In the UI: camera jumps, view changes, fronts and backs appear in one frame. Gestures are recognised by movement and finger count, never by a time window (a tap = down and up without moving past the slop, however long it took).
- Live data refreshing (the dashboard's facts) is not an animation.
- A code comment that says "owner's design" does not make every number in it the owner's: check before carrying a constant forward.

Related: [[settings-identity-to-device-only]].
