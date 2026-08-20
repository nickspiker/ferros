---
name: USB session auth and encrypted channel design
description: Ed25519 challenge-response + X25519 DH + ChaCha20 session encryption for PT commands over USB
type: project
---

USB session authentication and encryption — agreed design as of 2026-03-17.

**Auth handshake:**
1. Kernel sends 32-byte challenge nonce
2. Bridge signs with Ed25519 private key + sends ephemeral X25519 pubkey
3. Kernel verifies signature, generates own ephemeral X25519 keypair, sends pubkey
4. Both sides: X25519 DH → BLAKE3-derived ChaCha20 session key
5. All further PT packets encrypted with session key

**Capability tiers:**
- Unauthenticated: DIAG, BEAM_KERNEL_RING (public info only)
- Authenticated: RELOAD, REBOOT, BEAM_LEDGER, BEAM_HAMT
- Dev build only: BEAM_RAW
- Dev mode (pubkey=zeros): auth skipped, all caps open

**Key reuse:** same Ed25519 keypair signs kernels (build time) and authenticates USB sessions (runtime). Public key baked into seed, available to kernel.

**Crypto stack:** ed25519-compact (includes X25519), ChaCha20 (pure Rust, ~100 lines), BLAKE3 for key derivation. No new dependencies beyond what seed already uses.

**Future:** post-quantum ("clutch") layer to be added later on top of this foundation.

**Why:** caps in plaintext over USB = replay attacks. Session encryption prevents sniffing and replay. Ephemeral keys mean every session has a unique key.

**How to apply:** implement as part of the beam command infrastructure. The PT layer gains an AUTH command type that precedes any privileged cap usage.
