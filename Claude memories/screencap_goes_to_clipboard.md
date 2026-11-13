---
name: screencap-goes-to-clipboard
description: "on-device screencap's home is the clipboard once ferros has one; no \"save to file\" flow; a free frame-sized capture buffer is not to be designed until then"
metadata:
  node_type: memory
  type: project
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-07T18:47:29.329Z
---

Owner, 2026-11-07: "when we get the clipboard running that's where the screencap will live. no 'save to file' stuff. you can go in your navigator and paste but we'll work on that later."

Today the dev tool (tools/screencap) reads a free window surface over the bridge (display op 30). When every surface is held it falls back to the bare canvas.

**Why:** captures are content the person moves (pastes), not files on disk. That fits Rule -1 (no new on-disk locations) and the paste-into-the-navigator flow.

**How to apply:**
- Don't build save-to-file or a reserved capture region.
- When the clipboard exists, op 30's composed frame goes there.
- Until then, the "every surface held" case stays a known gap.
