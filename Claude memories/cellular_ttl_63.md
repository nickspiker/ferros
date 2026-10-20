---
name: cellular-ttl-63
description: Owner wants the ferros cellular stack's default IPv4 TTL to be 63, not 64 (2026-10-19); may revert later, not now. IPv6 hop limit not specified yet.
metadata:
  type: project
---

The ferros IP stack on the cellular path sends with a default IPv4 TTL of **63**, not the usual 64. Set by the owner on 2026-10-19 "for a weird reason", with "we may set it back at some point but not today".

**Why:** owner's call; the reason was not given and does not need to be asked.

**How to apply:** when the cellular data path of the net stack is built (see [[photon_on_ferros_no_bootstrap]] for the stack's shape), make 63 the default TTL constant for packets leaving over cellular, in one named place so reverting is a one-line change. Open questions to raise when that code is written, not before: whether IPv6 hop limit should also be 63, and whether WiFi shares the same default or keeps 64.
