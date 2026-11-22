---
name: identity-roots-survive-crash
description: Owner 2026-11-21 - a crashed and restarted identity service (fgtw) gets the person's identity roots back from the kernel; only a device restart (reboot) loses them
metadata:
  type: project
---

Owner, 2026-11-21, on the restart work (forge 3908b576..82b908c4): "I think identity roots come back after crash, as long as it wasn't a restart."

**Decided:**
- A process crash is not a reason to type the handle again. When fgtw dies and the kernel restarts it, the person's roots (identity seed, handle proof) are handed back by the kernel, and ahau never shows the attest screen for it.
- A device restart (a reboot, a cold boot, a jump) is where the roots end: unless unlock-at-boot is on, the handle is typed again ([[identity_not_on_disk_default]]).

**How to apply:** the kernel holds what it needs to hand back while a person is present (kernel-only, like the person key, [[person_keys_at_attest]]; in registers or sealed by the wairua, never plain in RAM a process can read, [[keys_in_registers]]), and drops it at lock and at reset. Not built as of 2026-11-21: the restart work restarts ahau after fgtw so the attest screen appears when fgtw comes back without roots.

Related: [[ferros_attest_os_level]], [[auth_flow_decisions]].
