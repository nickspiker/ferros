---
name: pipe-handshake-is-entropy-request
description: "PIPE handshake = entropy request. Single bit in, 256 bits of fresh hardware entropy out. No \"GET_RANDOM\" command needed."
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

The PIPE protocol has no separate "give me entropy" command because the handshake event itself is the entropy request. The chip's architectural constraints make this collapse inevitable, not a design choice:

- Handshake (one inverted slot pair in a 256+1+256 envelope per Claim 7b) is the only way to start a session.
- Every handshake invariably triggers a fresh KOHI to mint `wairua` (Claim 10) — the chip cannot establish a session without it.
- The first response message after handshake is `BLAKE3("ferros-pipe/handshake" || ira || wairua)` — which IS 256 bits of physically-fresh, ira-bound, MAC-authenticated entropy.

So the OS-to-chip entropy interface is literally: send 1 bit, wait ~110 ms, receive 256 bits. No opcode, no negotiation, no GET_RANDOM, no protocol stack.

**Why:** Captured during a discussion about how an OS would consume PIPE as a hardware RNG. I had described a 3-step design with an explicit ENTROPY_REQUEST opcode; user pointed out the protocol already does this — handshake IS the request. Patent Claim 11 added.

**How to apply:**
- When designing OS-side libraries that consume PIPE entropy, do not invent message types for entropy. Just call handshake.
- The protocol intentionally provides no path to entropy WITHOUT also opening a session — they're the same operation. This is a feature, not a limitation: it ensures that consumers of entropy must be authenticated session participants, not anonymous wire snoopers.
- For multiple chunks of entropy, either re-handshake (slow but cleanest, fresh wairua each time) or use the established session's keyed-hash chain to derive more bits (faster, deterministic given the chain state).
- This collapsing-of-operations is the hallmark of PIPE's protocol minimalism. When designing a new feature for the chip, ask: "can this be accomplished by exploiting an existing protocol event rather than adding a new message type?" The answer is usually yes for PIPE.

Related: [[pipe-host-asks-enclave-answers]] (handshake is one of the few host-initiated events; no ambient state push), [[pipe-slow-is-security]] (110ms per entropy call is by design).
