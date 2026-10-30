---
name: cellular-start-authorized
description: "owner — the modem may be brought up freely during experimentation: data (2026-10-26) and, since 2026-10-29, the whole radio incl. POTS/SMS/MMS stacks and emergency attaches; what ships is decided later (data-only MVNO); never actually place a call, send a text or dial 911/premium numbers on my own"
metadata:
  node_type: memory
  type: feedback
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-29T22:06:04.709Z
---

Owner, 2026-10-26: bringing the cellular radio up (START, the post-start handshake, the remote-file service, SIT control for radio power / SIM / registration / a data PDN, the raw-IP data path) is authorized and suits a night shift; the plan is unlimited, so data use is no concern.

Owner, 2026-10-29, after the CP emergency-attached on its own (registration state 20, REG_EM) during the radio-on runs: "You can turn on whatever you want during the experimentation phase, POTS, MMS/SMS included, it honestly does not matter to me. We just ship it with ... well, we'll sort that out." So during experimentation the voice/IMS/SMS stacks may be brought up and emergency attaches are fine; the shipping configuration is the owner's later decision ([[mvno-data-only]]).

**Why:** experimentation needs the modem's real behaviour; the earlier holds were about putting the phone on the air under its identity, now cleared.
**How to apply:** bring up whatever the experiment needs without asking. Still never, on my own initiative, place a call, send an SMS/MMS, or dial 911 or a premium number — those reach real people and cost real money; ask first if an experiment would need one. Per-unit NV and identity data stay off the repo ([[husky-factory-partitions-backup]]).
