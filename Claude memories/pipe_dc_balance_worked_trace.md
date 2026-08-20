---
name: pipe_dc_balance_worked_trace
description: verified 80-slot full-duplex mirror-XOR trace proving wire DC-balance from unbalanced tx streams
metadata: 
  node_type: memory
  type: project
  originSessionId: 347a3aa5-ea2c-46e2-8167-b1057e0b16e8
---

A hand-worked, code-verified full-duplex mirror-XOR trace for the PIPE patent line-code section. Carries the strongest single fact in the DC-balance argument: the two endpoints' transmit streams are individually unbalanced, but the wire (their XOR superposition) is exactly 50/50.

```
host_msg : 0000000001101101011100110110011100000000   (ASCII "msg" + framing)
encl_msg : 1111111110010010100011001001100011111111   (== ~host_msg, since enclave idle=1 host idle=0)
host_tx  : 00000000000000000010100010100010001010100000101000101000001010100000000000000000
encl_tx  : 01010101010101010100000100000100010000000101000001000001010000000101010101010101
line     : 01010101010101010110100110100110011010100101101001101001011010100101010101010101
```

Verified facts (all confirmed in Python): `encl_msg == ~host_msg`; `line == host_tx XOR encl_tx`; line is exactly 40 ones / 40 zeros over 80 slots. The punchline contrast: **host_tx is 15/65, encl_tx is 25/55 — each wildly unbalanced — yet the wire is exactly 40/40.** Neither side balances itself; the slot-interleave plus mirror compensation balances the wire.

This supersedes the hedged "balanced in expectation with one-slot transient" framing from the synthetic 8-slot FIG. 3 example — the real trace lands exactly 50/50 on an actual payload. See [[pipe_mirror_xor_line_code]] and [[pipe_pubkey_does_five_jobs]].
