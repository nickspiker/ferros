---
name: cellular-start-authorized
description: "owner 2026-10-26 — the modem may be STARTed and brought up for data on its own (unlimited data plan, \"you literally cannot go wrong\"); never voice/POTS/SMS/dialing without explicit consent; the SIT/libsitril RE is a night-shift job"
metadata:
  node_type: memory
  type: feedback
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-26T20:50:41.379Z
---

Owner, 2026-10-26: bringing the cellular radio up (G#A400 START, the post-start handshake, the remote-file service, SIT control for radio power / SIM / registration / a data PDN, the raw-IP data path) is authorized and suits a night shift; the plan is unlimited, so data use is no concern. The one line: never bring up the voice/POTS side (calls, dialing, SMS) without explicit consent — "unless you somehow bring up the POTS stack without consent and start dialing 900 numbers".

**Why:** the MVNO is data-only by design ([[mvno-data-only]]); the earlier hold on START was about putting the phone on the air under its identity, which the owner has now cleared for data.
**How to apply:** START and data bring-up need no further ask; keep IMS/voice/SMS paths untouched and say so in commits; per-unit NV and identity data stay off the repo ([[husky-factory-partitions-backup]]).
