---
name: feedback-stop-is-immediate
description: Owner 2026-11-18 - stopping an app or puck is an immediate kill; no close message, no grace period, no shutdown procedure
metadata:
  type: feedback
---

Stopping a kist or puck (the deck's Stop target, Settings → Deck's Stop, the power + volume down chord) ends it at once with init OP_STOP.
There is no close message first, no grace period and no deadline.

**Why:** the owner rejected "close first, then kill after a second" with "shutdown? That's called a fuckup."
He then asked how long light takes from Seattle to Sydney and back (about 83 ms in vacuum): a second is an eternity.
ferros apps persist every change the moment it happens (the persistence rule), so there is nothing to save at shutdown, and an app that needs a shutdown routine is a bug.

**How to apply:**
- Never design shutdown handshakes, grace timers or "ask nicely then kill" paths for user-facing stops.
- If a resource seems to need a clean close (the camera), fix the owner of the resource so a dead holder is cleaned up at once instead.
  The camera driver does this since forge d70b1c81 (the kernel's holder watch).
- The one thing still never killed is a streaming camera *driver* ([[husky_camera_bringup]]); the app holding the camera is safe to kill.
- Related: CLAUDE.md "Do NOT add cleanup code, destructors, or shutdown procedures in critical paths".
