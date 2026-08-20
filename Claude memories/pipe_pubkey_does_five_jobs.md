---
name: pipe-pubkey-does-five-jobs
description: "PIPE message format — one 256-bit ephemeral pubkey field simultaneously provides key transport, decryption, authentication, DC balance, AND frame leading edge. Patent Claim 13."
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

PIPE messages on the wire are framed as:

```
[ephemeral pubkey, 256 bits, leading-zeros suppressed]
[encrypted message body]
[single trailing 1]
```

The pubkey field is the *only* framing structure. It serves five simultaneous functions:

| Function | Mechanism |
|---|---|
| 1. Key transport | ECDH ephemeral pubkey — receiver combines with its static privkey to derive shared symmetric key |
| 2. Decryption material | Same pubkey is the only header the receiver needs to decrypt the body |
| 3. Implicit authentication | Wrong key → ciphertext decrypts to garbage → silent discard. No MAC needed. |
| 4. DC balance on the wire | Uniformly-distributed pubkey + mirror-XOR line code → on-wire statistics indistinguishable from idle |
| 5. Frame leading edge | First 1 bit of pubkey (after leading-zero suppression) marks message start |

No separate header byte, no key-ID, no length field, no MAC field, no DC-balance encoding overhead. Five functions, 256 bits.

**Why:** Surfaced when sketching the per-message wire format with the user. The "pubkey does five things" insight is a non-obvious composition of (i) Claim 12's line code, (ii) Claim 12a's leading-zero / trailing-1 framing, (iii) standard ECDH hybrid encryption, (iv) Claim 9a's silent-failure policy.

**How to apply:**
- When implementing the wire codec, the receiver-side decoder is: detect first 1 in idle stream → buffer until trailing 1 → prepend zeros to reach known total length → split first 256 bits as pubkey, rest as ciphertext → derive shared key via ECDH → decrypt → if structurally valid use; else discard silently.
- The sender just emits its ephemeral pubkey concatenated with the ciphertext, leading-zero-suppressed, with a trailing 1.
- Optional integrity hash inside the ciphertext is the user's choice; the decryption itself provides authentication-by-construction.
- For ECDH, use X25519 (256-bit pubkey, fast on small silicon) or curve-uniform encoding for max wire statistics.

Patent Claim 13 captures this. Prior-art secure-element protocols (ATECC608, A71CH, Optiga, TPM, DS28E15) all use separate fields per function — PIPE collapses them.

Related: [[pipe-mirror-xor-line-code]] (Claim 12 — the line code this builds on), [[pipe-host-asks-enclave-answers]] (silent failure policy), [[pipe-handshake-is-entropy-request]] (the handshake special-case of this format).
