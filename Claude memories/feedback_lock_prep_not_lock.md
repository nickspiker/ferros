---
name: feedback-lock-prep-not-lock
description: "owner 2026-10-28 — go as far as wanted on bootloader-lock preparation, but never run the actual lock (fastboot flashing lock) until a release is close; overnight phone work = nothing risky"
metadata:
  node_type: memory
  type: feedback
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-29T05:38:27.528Z
---

Owner, 2026-10-28: "on the lock, you can go as far as you want except for the actual lock. not sure I will do that until we get close to publishing a release, which we are far from." Also for unattended (overnight) phone work: "don't do anything risky".

**Why:** locking wipes data and closes the way back; it belongs to release time, not development. Unattended, nobody can power-cycle a frozen phone (an Android freeze under the hv does not reset itself, HV.md 2026-10-19).

**How to apply:** signing, verifying, firewalls, locked-build switches are all fine; never `fastboot flashing lock`. Unattended: every flash recoverable from fastboot with host backups, prove slot b before touching slot a, no freeze hunting. Related: [[avb-signed-unlocked]], [[ufs-firewall-decided]].
