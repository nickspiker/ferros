---
name: pipe-pt-unified-stack
description: "Photon Transport (PT) is the unified protocol stack; PIPE is its single-wire chip-level embodiment. Five layers L0-L4 (Medium, Line code, Framing, Message format, Crypto). PT-over-USB-C in ferros reuses L2-L4 unchanged."
metadata: 
  node_type: memory
  type: project
  originSessionId: 3623524f-ac39-42d0-b4d5-31df8cb753bf
---

Photon Transport (PT) is the medium-agnostic protocol stack used across the PIPE chip and ferros host. PIPE is one embodiment of PT (PT-over-single-wire). ferros's USB transport is another embodiment (PT-over-USB-C). They share L2-L4 verbatim and differ only at L0-L1.

**The PT stack:**

| Layer | Function | PIPE embodiment | ferros embodiment |
|---|---|---|---|
| L0 — Medium | physical substrate | single bidirectional wire + pull-down | USB-C differential pair |
| L1 — Line code | bit-to-medium encoding | mirror-XOR (Claim 12) | 8b/10b or 64b/66b (standard USB) |
| L2 — Framing | message boundaries | leading-zero suppress + trailing-1 (Claim 12a) | same |
| L3 — Message format | structured payload + crypto envelope | 5-purpose ephemeral pubkey + ciphertext + trailing 1 (Claim 13) | same |
| L4 — Crypto session | ECDH + AEAD + identity binding | Claim 11 (ira/wairua) | same |

**OSI critique (per Nick): OSI starts at L1 and forgets L0 — the medium itself.** Copper parasitics, fiber dispersion, air fading all constrain achievable line codes; the medium deserves a layer. OSI also conflates medium + line code + framing into one overloaded L1, has fictional L5/L6 (no real protocol uses them), and provides no home for authentication (which ends up scattered across L2/L3/L4/L7). PT is 5 layers, each with one job.

**Patent scope:** the current provisional (claims.md) covers PIPE specifically — Claim 12 (mirror-XOR line code) is the single-wire L1 instantiation. A planned continuation application will lift Claims 12a + 13 out of the single-wire context and claim PT broadly.

**Why:** Nick envisions PT as a TCP/IP-replacement stack that scales from a chip's single wire up to USB and beyond, with cryptographic session-in-one-roundtrip as the primary differentiator. Session establishment in conventional TCP/IP requires ~12-18 messages (DHCP + ARP + DNS + TCP + TLS + app-auth); PT does it in 1 tx + 1 rx.

**How to apply:** when discussing protocols in [[pipe-trng-silicon-primitives]] / [[pipe-mirror-xor-line-code]] / [[pipe-pubkey-does-five-jobs]] or ferros USB code, reference the PT stack layers explicitly. PIPE work is L0-L1 single-wire specific; ferros USB transport is L0-L1 USB-specific. L2-L4 are shared and live conceptually in PT, not in either project alone.
