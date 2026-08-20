---
name: Key storage strategy
description: Hardware key storage — RPMB for now, BLAKE3 silicon oracle for Glyph
type: project
---

## Current (FP5): RPMB
- UFS RPMB partition — SHA256-HMAC authenticated, write-once key, can't read back
- Standard UFS feature, accessible via UPIU Security Protocol commands
- Clunky (SHA256 not BLAKE3) but functional for development
- This is what Android uses for hardware-backed keystore

## Future (Glyph custom silicon): BLAKE3 Oracle
- Burned-in BLAKE3 with private domain separator
- Private bits added internally, never exposed
- Return value via optical link (no electrical side-channel)
- True CSR: write key, can't read back, crypto ops only

## Runtime key storage: ARM PAC registers (day 0)
- Five PAC key registers at EL1, each 128 bits = 640 bits total
- APIAKey_EL1, APIBKey_EL1, APDAKey_EL1, APDBKey_EL1, APGAKey_EL1
- Hardware registers — never in RAM, never in L1/L2 cache
- ferros doesn't use PAC (ring memory eliminates the need)
- Repurpose for: ChaCha20 key (256 bits = 2 regs), nonce (1 reg), 2 spare
- QCM6490 Cortex-A78: PAC is implemented, registers guaranteed
- Write via MSR instructions at EL1, no TrustZone needed

## Development interim: Derived key
- Derive from UFS UUID (device descriptor) via BLAKE3
- Not secure against physical access
- Gets crypto plumbing working

**Why:** Owner's keys must never be in RAM. Hardware-enforced isolation
is the only acceptable model. RPMB is the best available on FP5.
