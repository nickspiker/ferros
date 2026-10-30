---
name: modem-uecap-registration
description: "cold ferros modem registers on Verizon LTE/NR (diag modem s10/s11) and moves IP both ways (s12: data call + ICMPv6 echo on rmnet ch 197) since 2026-10-29; the registration blocker was the uecap file service (CP's block size 4000, token+k per block), not RIL; Android kprobe capture method"
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
- The capture tool is kprobes on stock Android + Magisk: cpif `txq_write` (+0xd8 = skb head, where the SIT header sits) and `pass_skb_to_demux` (+0xe0 = skb data); UL packets at `pktproc_send_pkt_to_cp` (data +0xe0, channel at +0x48 in the cb), armed from /data/adb/post-fs-data.d/ril-debug.sh.
- Captures stay local: they carry SIM identity.
- Data path works (2dd0a0c): SETUP_DATA_CALL on VZWINTERNET, UL queue 1 with lcid = 181 + cid - 1, the size padded +76; an ICMPv6 echo to 2001:4860:4860::8888 answers in 63 ms. The RS gets no RA, and none is needed (the reply's address is global).
- IP over cellular works (c5e35ee); since db3ea61 the modem is a driver PROCESS (payloads/modem, tools/husky-modem.sh; input up|off|sN) serving `rmnet` to ip (slot 52, CALL_LIVE); kernel keeps only the boot handover block. Traps: UDP answers come down the legacy RAW ring as SIT frames on ch 197, not pktproc; EL3 refuses PMU *reads* (-22) so hardware-grant reads go direct. Not started by init yet (owner to decide). See [[cellular-start-authorized]].
- Open (2026-10-29 night, commit 732b91f): DNS answers that come down the legacy RAW ring wait in the CP until the AP sends something (TRACE: AAAA queries at 44.4/45.4/46.4 s answered at 47.44 s, 2 ms after the next UL packet); TCP over pktproc DL flows at once. The driver also recovers by itself now (rmnet thread, RESTART 7, re-bring-up on a dead service); a re-established call once came back IPv4 only.
- WiFi on the night of 2026-10-29: every join refused (CP off made no difference; router throttle suspected, unproven). The kernel's WiFi keeper restarts pcie1 with backoff 64→1024 s; ip rides cellular meanwhile (CALL_LIVE needs a receiving server).
