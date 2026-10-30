---
name: feedback-avb-key-freedom
description: "owner 2026-10-28 — the AVB signing key may be used freely (sign, flash, regenerate) as long as it is stored as VSF in /mnt/Harbor/Code/keys/; no need to ask per use"
metadata:
  node_type: memory
  type: feedback
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-29T00:28:39.732Z
---

Owner, 2026-10-28: "as long as you store it VSF and in Code/keys/ then you can pretty much do whatever you want. Individuals will flash it and if the ira doesn't match or is spoofed, they have bigger problems once we eventually lock the boot chain."

**Why:** the AVB key is a per-owner boot key that individuals flash themselves; the real binding is the ira check once the chain is locked, not secrecy-by-process around this key.

**How to apply:** sign and flash with `keys/avb.vsf` (VSF `kgr`/`kvr`) without asking each time; keep it VSF and in the keys dir, never anywhere else, never a non-VSF format. Still never print or commit its bytes. Related: [[avb-signed-unlocked]], [[secrets-canonical-keys-dir]].
