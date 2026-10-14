---
name: ferros_wake_policy_prox_gate
description: "ferros display-wake requirement — power button lights the panel ONLY if raw proximity is far; native policy, no reactive undo"
metadata: 
  node_type: memory
  type: project
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

Owner's wake spec (stated 2026-08-21): the screen turns on ONLY when the power button is pressed AND nothing is near the proximity sensor. Calls, notifications, plug/unplug, bumps, ambient-light changes — none of them may light the panel. Any orientation (flat on a table counts as covered), no on-ear heuristic.

**Where this belongs:** in ferros, the power-button handler reads the RAW proximity sensor (Pixel 8 husky: TMD3719, `type=android.sensor.proximity(8)`, maxRange 5.0, near reads 0.0) and only drives the display on if far. Because ferros owns the entire power/display path, there is no wake to race and no flash — nothing else can turn the panel on. Do NOT gate this in the Linux/GKI kernel: on Android the button just emits a `KEY_POWER` evdev event flagged WAKE; the actual "light the display" decision is framework (`PhoneWindowManager.interceptKeyBeforeQueueing` → `PowerManagerService.wakeUp`), not the kernel. A kernel recompile alone will not gate it. This is an input/display-policy rule, native to ferros.

Explicitly reject Google's on-ear fusion: the FUSED `com.google.sensor.prox_voice_call` (`0x0101002a`, vendor type 131092) releases screen-off when orientation exits the ~0.87 rad ear cone, which is exactly the "flat on table → screen wakes" failure. ferros uses the raw sensor only, near==off period.

**Husky/Android stopgap RETIRED (2026-09-09):** PocketGuard (Magisk priv-app that undid covered wakes via `su -c "input keyevent 223"`) was removed from the phone at the owner's request — the raw-vs-fused proximity fight was too glitchy in practice, and there is no clean layer on Android to win it (the wake decision is in PhoneWindowManager, the on-ear fusion is in AoC). Conclusion: don't retry this on Android; it only gets solved when ferros owns the whole stack. Source is kept for reference at ferros `tools/pocketguard/`. Related: [[pixel8_dedicated_dev_device]].
