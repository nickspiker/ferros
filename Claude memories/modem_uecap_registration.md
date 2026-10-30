---
name: modem-uecap-registration
description: "cold ferros modem registers on Verizon LTE/NR since 2026-10-29 (diag modem s10/s11); the blocker was the uecap file service (CP's block size 4000, token+k per block), not RIL; Android kprobe capture method"
metadata:
  node_type: memory
  type: project
  originSessionId: a305f35f-299d-4e22-bd4c-7f99024cbeea
  modified: 2026-10-30T02:22:15.747Z
---

2026-10-29 evening: the husky CP's data stack (stack 1, ch246) registers from a cold ferros, NR then LTE, about 1.2 s after SET_ALLOW_NETWORK (commit 03cde33, MODEM-BOOT-RECON.md last section).

The blocker was the modem service's UE-capability file reads on ch130. The CP names a block size (4000) and books one token per block. ferros answered 1 KiB blocks under one token, and the CP silently never searched. Blocks over 2 KiB need SIT multi-frame packets, which ferros's link layer now sends.

**Why:** weeks of RIL-side guessing (profiles, attach APN, replays of all 264 Android requests at Android's timing) changed nothing. The difference was in what ferros *answers* the CP, not in what it sends.

**How to apply:**
- When the CP misbehaves silently, diff ferros's *responses* to CP→AP requests against Android's, not only the AP→CP requests.
- The capture tool is kprobes on stock Android + Magisk: cpif `txq_write` (skb data +0xd8) and `pass_skb_to_demux` (+0xe0), armed from /data/adb/post-fs-data.d/ril-debug.sh.
- Captures stay local: they carry SIM identity.
- Next step is SETUP_DATA_CALL plus pktproc DL/UL. See [[cellular-start-authorized]].
