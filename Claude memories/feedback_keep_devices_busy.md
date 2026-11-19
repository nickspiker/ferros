---
name: feedback-keep-devices-busy
description: Owner 2026-11-19 - stage agents, don't swarm them; one planned job per device at a time, few agents, no parallel edits of the same code; but never leave a set-up device idle for hours either
metadata:
  type: feedback
---

**Two corrections from the owner on the same day:**
- **Idle devices:** both phones sat on the bench aimed at lit objects "literally for you" for about 8 hours, mostly idle.
- **Then the opposite fault:** "I'd definitely focus on staging agents rather than keeping all agents busy. that burns more coins and actually wastes time in the long run since edits collide, checkouts happen, touches intersect and things start to clash."
  - At that point the session had about seven agents running: several queued for the same Pro, edits colliding in the camera driver and ferros_cam (merge conflicts), and helpers spawned by agents.

**Why:** many concurrent agents cost more and finish later: they contend for the one device, collide in the same files, and each merge costs a resolution pass. The owner's setup time is also scarce, so a prepared device shouldn't sit unused for hours.

**How to apply:**
- **Stage the work:** a short planned sequence per device, one device-holding agent per phone at a time, the next one started when it reports.
- **Keep it small:** at most a few agents in total (one per device, plus at most one host-only job).
- **No overlap:** no two agents editing the same crate or file at once. Sequence them, or have one agent do both.
- **No helpers:** agents don't spawn helper agents; say so in every brief.
- **Use the bench:** when a device is set up and free, the next staged job takes it promptly, so it's never idle for hours waiting on a decision.
- **Pin devices:** every device command names its serial or port ([[bridge_debug_for_two_devices]]).
