# KEY_REGISTERS — Live Secret Handling in the CPU Register File
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Secrets in flight never touch named memory, and a single bit flip never reaches the owner.

---

## What This Document Defines

How ferros holds active key material — the handle seed, handle proof, vault keys, attestation inputs, the TOKEN/ira/wairua roots — *while in use*, inside the CPU register file rather than in DRAM, and how it keeps a single-event upset (SEU) from silently corrupting a secret and burning someone.

This is distinct from:
- **[VAULT.md](VAULT.md) / [VAULT_ROOT.md](VAULT_ROOT.md)** — at-rest persistent storage (opaque, keyed-name + keyed-contents, inverted-pair on disk).
- **[SECURITY_CHAIN.md](SECURITY_CHAIN.md)** — boot-time trust handoff.
- **[PIPE.md](PIPE.md)** — the hardware enclave that eventually replaces the software device root.

This doc is the *register layer* in between: secrets that are live in the core right now.

---

## The Problem

A secret that is being used (to derive a vault key, sign an attestation, answer a custodian challenge) is, at that moment, sitting in CPU registers. Two failure modes:

1. **It leaks to memory.** General-purpose registers (X0–X30) spill to the stack on every call, get clobbered, and land in DRAM the instant a context switch or interrupt fires. Anything in DRAM is recoverable by cold-boot, DMA, a compromised peer at another EL, or forensics. A secret that touches a named memory address has left the building.

2. **A bit flips and nobody notices.** The register file is bare flip-flops with **no ECC and no parity** on stock ARMv8/v9 (Tensor G3, M1). DRAM/cache *may* have ECC (vendor-dependent, usually not exposed); the register file never does. A single-event upset — cosmic ray, supply glitch, induced fault — flips a bit in a key register **silently**. The corrupted key then derives a wrong vault key (data lost), or produces a wrong attestation (the owner is mis-identified, or worse, an action is authorized against a value that isn't theirs). "Don't want bit flips to go fuck someone over" — exactly this.

The TOKEN identity model leans on the same Byzantine inverted-pair discipline as the at-rest vault. It has to apply **in the register file too**, because that's where the secret is when it matters.

---

## The Register Landscape on stock aarch64

| Surface | Size | Verdict |
|---|---|---|
| GP X0–X30 | 31 × 64-bit | **No.** Spills to stack, clobbered on calls, in DRAM after any trap. |
| NEON/SIMD V0–V31 | 32 × 128-bit = **4096 bits = 128 × 32-bit** | **Yes.** The secret register file. Crypto ext (AES/SHA) operates directly on these — feed a key from V-regs into AES without it ever having a memory address. |
| System regs (TPIDR_EL*, etc.) | a handful × 64-bit | Limited; some are owner-controlled scratch but few and per-EL. Not a key file. |

**There is no clean "16 × 32-bit hardened secret register file" on stock ARM.** What we have is **NEON: 4096 bits, unprotected.** That's plenty of *space* (a 256-bit secret is 8 of 128 V-regs), but the slots have no SEU protection. So the fault-tolerance is **ours, in software**, the same scheme as the vault — the redundancy is the protection, not the registers.

When we make our own chip, *this* becomes custom hardened registers with the completeness invariant enforced in silicon (like PIPE's storage). Until then: software redundancy over NEON.

---

## The Design

### 1. Secrets live in NEON, never in named memory
A live secret is held in V-registers and consumed directly by the crypto instructions. It is never written to a stack slot, a heap allocation, or any named address. The compiler must not be allowed to spill it — pin via inline `asm!` with the V-regs as explicit operands, or a hand-written routine that owns its register allocation. (Rust/LLVM will happily spill NEON regs under pressure; the secret-handling routines must be written so it can't.)

### 2. Inverted-pair + quorum, in registers — same as the vault
Hold each 256-bit secret as **value + complement** across V-register pairs, with the **XOR-to-all-ones completeness invariant checked on every read**. Hold **multiple independent copies** (quorum). A single-bit upset in one copy:
- is **detected** by the invariant (the pair no longer XORs to all-ones),
- is **outvoted** by the agreeing quorum copies,
- is **repaired** from the survivors.

With 4096 NEON bits, a 256-bit secret as inverted-pair (512 bits) × a 3-copy quorum is 1536 bits — comfortably fits, with room for more than one secret live at once. This is the **identical Byzantine inverted-pair + quorum the TOKEN patent claims for storage**, applied to the live working copy. A flip never reaches the owner because it's caught and corrected before the secret is used.

### 3. The trap / context-switch path treats key V-regs as sacred
This is the sharp edge. If a key sits in V-regs and an interrupt or EL transition fires, the trap handler must **not** save those V-regs to a memory context block (which would defeat rule 1 and put a plaintext secret on a stack). On a microkernel where ferros owns every trap vector, the policy is:
- Key-handling runs in a bounded critical section with interrupts masked (`msr daifset`) where feasible, so no trap can fire mid-operation.
- Where a context switch genuinely must occur, the key V-regs are either (a) cleared before yielding and re-derived on return, or (b) if they must persist, saved only into the opaque/wairua-keyed store — never plaintext, and the saved copy gets the inverted-pair treatment. Never a raw `stp q0, q1, [sp]` of a secret.
- Lazy-FP / FP-trap save must be configured so it cannot lazily spill a secret-bearing V-reg behind our backs.

### 4. wairua vs ira at the register layer
The per-session secret (wairua) and permanent secret (ira) distinction from [PIPE.md](PIPE.md) carries up here: a wairua-derived working key, once its session ends, is cleared from the V-regs and — because it was never written to durable medium and the wairua itself is gone — is unrecoverable by construction (see the TOKEN session-expiry property). Clearing the V-regs at deassert/session-end is part of the killswitch/zeroize path.

---

## Open Questions (for when this gets built)

- **Exact NEON pinning in Rust** — does inline `asm!` with `out(vreg)` operands hold across the routine, or do we need a `.s` file for the secret-critical paths? Measure whether LLVM spills under realistic pressure.
- **Interrupt-masked critical-section length** — how long can key ops hold `daifset` without hurting latency? Probably fine (BLAKE3 over 256 bits is fast), but measure.
- **Quorum copy count** — 3 copies (tolerate 1 bad) vs 5 (tolerate 2)? Bound by NEON space and how many secrets are live simultaneously. Start at 3, revisit.
- **M1 vs Tensor G3 differences** — Apple SPRR/GXF and the Tensor's TrustZone-adjacent state: confirm neither path lazily saves V-regs to memory we don't control. (M1 boots via m1n1 at EL2, so we own the context; verify the assumption.)
- **Does the GIC/IRQ path on either target ever auto-save FP/SIMD?** Check before assuming the trap path is fully ours.

---

## Relationship to the TOKEN filing

The **property** — fault-tolerant, Byzantine, inverted-pair handling of identity secrets, detect-and-repair, distinct from and complementary to hardware-level redundancy — is already claimed in the TOKEN provisional (the fault-tolerant-storage and opaque-storage claims). This doc is the **ferros implementation** of that property at the register layer; it adds no patent matter, it builds the claimed mechanism. The patent stays mechanism-agnostic (no register count); ferros picks NEON-on-stock-ARM as the concrete substrate until custom silicon.
