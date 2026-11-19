---
name: feedback-keep-devices-busy
description: Owner 2026-11-19 - when the phones are set up on the bench (aimed at lit scenes, plugged in), keep them busy every hour; never leave a device idle waiting on a decision or a queue
metadata:
  type: feedback
---

The owner set both phones on the bench aimed at lit objects "literally for you" for about 8 hours, and found they had sat mostly idle while agents queued for one device at a time and the coordinator waited on his answers.

**Why:** his setup time is the scarce resource; an idle device he prepared is wasted work on his side.

**How to apply:**
- When a device is free, give it work at once: verification runs, orientation and colour checks, measurements, regressions, anything that uses the scene he set up.
- Run host-only work (drivers, decoders, protocol crates) in parallel from the start; don't hold it behind device slots.
- Don't wait on owner decisions for work that doesn't need them; ask, and keep going on what doesn't depend on the answer.
- If the host's USB or a device drops, say so at once and move every agent to host work until it's back.
- Pin every device command to its serial or port, so parallel work never hits the wrong phone ([[bridge_debug_for_two_devices]]).
