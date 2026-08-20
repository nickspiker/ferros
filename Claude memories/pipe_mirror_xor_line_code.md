---
name: pipe-mirror-xor-line-code
description: "PIPE's mirror-XOR signaling is a novel 100%-efficient single-wire bidirectional line code with automatic DC balance and free clock recovery — all from one XOR gate. Patent Claim 12."
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

PIPE's wire signaling is not just a protocol convention — it's a novel **line code** with provable mathematical properties that no published line code achieves simultaneously. Patent Claim 12 captures this; the encoding rule and its properties are:

**Encoding rule (per slot, applied identically at both ends):**
```
wire_value = OWN_IDLE XOR own_data XOR other_last_decoded_data
```

With `enclave_idle = 1`, `host_idle = 0`. Each side registers the most recent decoded bit from the other party.

**Properties — all from the one XOR rule, no separate mechanisms:**

1. **100% encoding efficiency per direction.** One data bit per slot per side. No bit stuffing, no symbol expansion, no lookup tables, no scramblers. Compared to Manchester (50%), 8B/10B (80%), 64B/66B (97%), 128B/130B (98.5%), this is asymptotically perfect.

2. **Automatic DC balance per slot pair.** The cross-coupling term `other_last_decoded_data` forces one party's slot value to compensate the other party's prior data transmission. Any active data bit on one side propagates as a pattern inversion across the slot pair; DC balance is restored within two slots of any data activity, with zero balance-encoding overhead.

3. **100% transition density in idle.** Both data streams at zero yields `1, 0, 1, 0, ...` on the wire (enclave HIGH, host LOW alternating). Every slot transitions, providing free CDR for the receiver — no clock wire, no Manchester doubling, no scrambler.

**Three claimed embodiments:**

- **Claim 12 (base):** the encoding rule itself with slot-alternating ownership.
- **Claim 12a (zero-overhead framing):** leading-zero suppression + trailing-1 marker. For uniformly-random message content (BLAKE3 outputs, conditioner extracts), expected leading-zero savings = 1 bit; trailing-1 cost = 1 bit; net expected framing overhead = exactly 0 bits per message. This is information-theoretically minimum (a delimiter of less than 1 bit is impossible).
- **Claim 12b (float-high full-full duplex):** both parties transmit simultaneously into a wired-OR pull-down; each recovers the other's bit by `other_data = wire XOR own_drive`. Doubles per-wire throughput. DC balance requires either transition-based encoding or a differential physical topology in this embodiment.

**Why:** Surfaced during a design discussion about how PT (Photon Transport) compares to standard line codes. The user pointed out PT is 100% efficient AND DC-balanced AND self-clocking, and corrected my analysis (I had initially thought "100% AND DC-balanced AND self-clocking simultaneously is impossible without an explicit clock wire"). The realization: the cross-coupled XOR rule makes all three properties fall out of the same operation. The user also worked out the leading-zero + trailing-1 framing scheme that brings overhead to provably exactly zero on average.

**How to apply:**
- When discussing PIPE's wire signaling with anyone, describe it as a *line code* (not just a protocol), and cite Claim 12. The line-code framing is the patent-novel part.
- When comparing to USB, Ethernet, PCIe, etc., the comparison table from the Claim 12 prose is the canonical version (NRZ+stuff <100% with stuffing, Manchester 50%, 8B/10B 80%, 64B/66B 97%, 128B/130B 98.5%, mirror-XOR 100%).
- For ferros-ferros PT-over-USB design: the same line code can be implemented inside a USB bulk pipe, getting the 100% efficiency property minus USB's outer framing overhead. The "PT all the way down" alternative would let us achieve the line code on the wire directly (no USB framing); the trade-off is custom PHY in silicon vs. off-the-shelf USB PHY.
- Implementation: single XOR gate per side, one 1-bit register for `other_last_decoded_data`, slot-toggle for ownership. Trivially small.

Related: [[pipe-host-asks-enclave-answers]] (the asymmetric idle pattern), [[pipe-handshake-is-entropy-request]] (one bit deviation = handshake = entropy delivery).
