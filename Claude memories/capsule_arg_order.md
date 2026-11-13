---
name: capsule-arg-order
description: "bridge capsule takes <in.bin> <out.vsf> FIRST, flags after; flags first writes a file named --masked and leaves the old capsule to be stored (2026-11-11)"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-12T05:19:01.028Z
---

`ferros-bridge capsule <code.bin> <out.vsf> --masked --text-len N` — the two paths are positional and come first. Written as `capsule <bin> --masked --text-len N <out>`, it treats `--masked` as the output path, writes the new capsule there, and `target/<name>.capsule.vsf` stays the old build; storing that silently ships stale code (and the seal of the new bin doesn't match it).

**Why:** 2026-11-11 the whole day's masked deploys (vault-read-3, ip held receives, ip TCP rebuild, NFC tag unlock, init, kists) went out as old capsules on both devices; noticed only because the Pro's NFC log still printed full tag UIDs.

**How to apply:** after making any capsule, check the `signed capsule: ... -> target/<name>.capsule.vsf` line names the intended file; before claiming a deploy, check a string or size unique to the new build is in what was stored. Related: [[masked-capsule-needs-seal]].
