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

**There is no clean "16 × 32-bit hardened secret register file" on stock ARM.** What we have is **NEON: 4096 bits, unprotected.** That's plenty of *space* (a 256-bit secret is 8 of 128 V-regs), but the slots have no SEU protection. So the fault-tolerance is **ours, in software** — but only the part software can honestly deliver on this substrate.

When we make our own chip, *this* becomes custom hardened registers with the completeness invariant enforced in silicon (like PIPE's storage). Until then: software **detection** over NEON, and **freeze** on failure. Not software repair — see below for why.

---

## The Design (stock silicon — what ships)

The shipped rule, in one line: **inverted-pair hold; check XOR-to-all-ones immediately before consume; on mismatch, zero all live secret V-regs then freeze — never repair, never return.**

### 1. Secrets live in NEON, never in named memory
A live secret is held in V-registers and consumed directly by the crypto instructions. It is never written to a stack slot, a heap allocation, or any named address. The compiler must not be allowed to spill it — the secret's **entire lifetime stays inside a single `asm!` block** (see the implementation section). The no-spill guarantee only holds while the secret never crosses back into Rust; passing a secret V-reg through a Rust boundary is out of scope and unprotected. (Rust/LLVM will happily spill NEON regs under pressure; the secret-handling routines are written so it can't.)

### 2. Inverted-pair, checked before consume — detect, do not repair
Hold each 256-bit secret as **value + complement** in V-register pairs, with the **XOR-to-all-ones completeness invariant checked immediately before every consume** — same instant, same masked critical section as the crypto op, no instructions between check and use, so a flip cannot slip in after the check.

A single-bit upset in the pair breaks the invariant (value XOR complement is no longer all-ones) and is **detected**. It is **not repaired**, and that is deliberate:

- **Repair needs a quorum, and a quorum needs the no-spill guarantee to be perfect — which it is not on stock ARM.** Holding N copies to outvote a bad one only pays off if the "secret never touches named memory" invariant is truly enforced, and on stock silicon it is best-effort, not provable end-to-end. Paying the full 4× redundancy cost for a guarantee we can't fully make is detection-theater at full price.
- **SEU flips can be correlated.** In storage, copies fail independently over time. In the register file, one glitch/particle event in a shared clock domain and supply rail can flip bits across adjacent V-regs at once. Repair-by-majority assumes an independence the substrate does not guarantee. Detection still fires; majority repair cannot be trusted.

So on stock silicon the guarantee is: **detect always; fail closed by freezing** (§3). "A flip never reaches the owner" is achieved by *stopping*, not by *correcting*. Quorum-repair moves to custom silicon, where we control whether registers spill and can enforce independence — see the custom-silicon section.

### 3. On corruption: zero all live secret V-regs, then freeze (NGARO-style)
This is the fail-closed path, and it maps directly onto the [NGARO silent-wire state](PIPE.md) — integrity fails, the machine goes dark. On a detected mismatch, in order:

1. **Mask** all interrupts (`msr daifset, #G#F`) — so nothing can preempt between zero and freeze and snapshot a half-cleared state.
2. **Zero all live secret V-regs**, not just the pair that tripped — a correlated event that broke pair A may have corrupted pair B sitting alongside it; wiping only the failed pair leaves the other live. Same cost (a handful of `movi`s), closes the gap. This reuses the §5 session-end zeroize routine.
3. **Freeze** — `wfi` in an infinite loop, never return, never consume the value.

Zeroing *before* the freeze matters even though a power cycle would evaporate the registers anyway: during the frozen-but-still-powered window (someone walks away from a hung machine), there must be no secret sitting in the core for a JTAG/debug read or glitch-resume to lift.

**Why we do not try to physically self-destruct here.** The premise of this path is a corrupted core — the same core you would be asking to orchestrate a kill. A CPU that cannot be trusted to hold a secret correctly cannot be trusted to correctly execute "now short your own VDD"; the flip that hit the key could hit the branch that fires the killshot. And on stock M1/Tensor the AP cannot crowbar its own supply rail anyway (PMIC sequencing lives behind SEP / secure-world SMC). The watchdog can never be the thing it is watching. On stock silicon, **freeze IS the destruct**: the working key is wairua-derived and session-scoped (§5), so a dead, unpowered, never-persisted register is unrecoverable by construction. Physical zeroize belongs in custom silicon, in a monitor independent of the core.

### 4. The trap / context-switch path treats key V-regs as sacred
If a key sits in V-regs and an interrupt or EL transition fires, the trap handler must **not** save those V-regs to a memory context block (which would defeat §1 and put a plaintext secret on a stack). On a microkernel where ferros owns every trap vector, the policy is:
- Key-handling runs in a bounded critical section with interrupts masked (`msr daifset`), so no ordinary trap fires mid-operation. (Per-target caveat below: `daifset` seals the M1 EL2 path but not a Tensor secure-world preemption.)
- Where a context switch genuinely must occur, the key V-regs are either (a) cleared before yielding and re-derived on return, or (b) if they must persist, saved only into the opaque/wairua-keyed store — never plaintext, and the saved copy gets the inverted-pair treatment. Never a raw `stp q0, q1, [sp]` of a secret.
- Stock ARM never auto-saves FP/SIMD in hardware — "lazy FP" is a software technique (the OS leaves `CPACR_EL1.FPEN` / `CPTR_EL2` trapping and its own handler does the save). Since ferros owns that handler, it must be configured so it cannot lazily spill a secret-bearing V-reg behind our backs.

### 5. wairua vs ira at the register layer
The per-session secret (wairua) and permanent secret (ira) distinction from [PIPE.md](PIPE.md) carries up here: a wairua-derived working key, once its session ends, is cleared from the V-regs and — because it was never written to durable medium and the wairua itself is gone — is unrecoverable by construction (see the TOKEN session-expiry property). The clear-all-live-secret-V-regs routine is shared by two callers: **session-end** (normal zeroize) and **corruption-freeze** (§3). Clearing at deassert/session-end is part of the killswitch/zeroize path.

---

## Implementation: derive → audit → freeze → guard

Secrets are handled in **hand-audited assembly**, not Rust the compiler recompiles each build, because the instant LLVM is near a secret V-reg it may spill it and the spill defeats everything. But we do not hand-write the asm blind — we generate it from Rust, audit it, and freeze the reviewed output as the artifact.

**Design A (monolithic block).** For the secret-critical paths (key derivation, attestation sign — where a wrong output burns the owner), the whole operation — mask, load pair, check, consume, zero, freeze — lives in **one `asm!` block** so the compiler cannot interleave anything between the check and the consume. Nothing spills because nothing crosses back into Rust mid-secret.

**Three tiers of trust — the codegen decides which one we live in:**

1. **Ship Rust, trust LLVM each build** — *rejected.* A compiler bump or a caller's register pressure can silently reintroduce a spill; nothing tells you.
2. **Generate asm, ship it unmodified** — *provably safe through the Rust ecosystem.* The frozen asm *is* the Rust program; there is no recompile to drift. This is the goal. It only counts as safe-for-secrets if the generated asm happens to contain no SIMD-to-stack spill — Rust never promised "no NEON spill," so Tier 2 faithfully ships a spill if LLVM emits one.
3. **Generate asm, modify it, ship the modified** — safe *only if the diff is proven/audited.* Forced when Tier 2's output spills or the freeze tail is wrong.

**The workflow:**
- **Derive** — write the operation in Rust, `rustc --target aarch64-unknown-none -O --emit asm`.
- **Audit** — read the emitted `.s`; confirm no `str q`/`stp q`/`str x`/`stp x` to `[sp]` and no stack frame; confirm the mask/zero-all/freeze tail is correct (per-target).
- **Freeze** — commit the reviewed `.s` as the build artifact. The Rust version is demoted to a documented derivation reference, not compiled into the secret path.
- **Guard** — record the exact toolchain version (asm drift is invisible otherwise) and add a CI canary that greps the shipped `.s` for SIMD/GP-to-stack stores; a hit fails the build.

---

## Proven Result (the reason §2/§3 is real, not aspirational)

The minimal probe — inverted-pair load + XOR-to-all-ones check + zero-all + freeze — was compiled for the kernel target and the emitted asm read. Source and audited output live in [keyreg/keyreg_probe.rs](keyreg/keyreg_probe.rs) and [keyreg/keyreg_probe.s](keyreg/keyreg_probe.s).

- **Toolchain:** rustc 1.94.0 (4a4ef493e 2026-03-02), `--target aarch64-unknown-none -O`.
- **Result:** **Tier 2 — no stack frame allocated, no `str/stp q`, no GP-to-stack store.** The secret stays in `v0`–`v4` from load through zero. The generated asm *is* the program; it can ship unmodified.

The emitted function body:

```asm
keyreg_check_or_die:
	mov	x8, x0                      // pointer shuffle (not secret)
	msr	DAIFSet, #15                // mask — seal critical section
	ld1	{ v0.16b }, [x8]            // value
	ld1	{ v1.16b }, [x1]            // complement
	eor	v2.16b, v0.16b, v1.16b      // all-ones iff pair intact
	mvn	v3.16b, v2.16b              // all-zero iff intact
	umaxv	b4, v3.16b                 // any set bit => corruption
	mov	w0, v4.s[0]
	cbz	w0, .Ltmp0                  // intact -> consume
	movi	v0.16b, #0                 // corruption: zero ALL live secret regs
	movi	v1.16b, #0
	movi	v2.16b, #0
	movi	v3.16b, #0
.Ltmp1:
	wfi                                // freeze — never returns
	b	.Ltmp1
.Ltmp0:
	mov	x9, v0.d[0]                 // consume (probe: move a word out)
	mov	x0, x9
	ret
```

The only GP moves are pointer shuffles and the return — no secret touches the stack.

**Caveats (the "mostly safe" corner cases):**
1. The guarantee holds for the `asm!`-block region only — because the secret never leaves it. A *caller* that holds a secret V-reg across a Rust boundary voids it. The rule: one block, whole lifetime.
2. The probe loads from pointers, so *in the probe* the secret came from memory. The shipped load source must itself be non-named (derived in-block, or from the opaque/wairua store). The probe validates the hold/check/zero/freeze mechanism, not provenance.
3. Emitted at `-O`; a debug build may differ. The shipped build pins opt level and toolchain (recorded above).
4. The `msr daifset` + `wfi` tail is correct for M1 at EL2. On Tensor it does not seal against secure-world preemption — see per-target notes.

---

## Per-target notes: M1 vs Tensor G3

Stock ARM never autosaves V-regs in hardware on either chip. The real question is whether anything at a **higher privilege level than ferros** saves V-regs to memory we do not control, and whether `daifset` actually seals the critical section. The two targets diverge:

| | **M1 (EL2 via m1n1)** | **Pixel 8 Pro (Tensor G3)** |
|---|---|---|
| ferros's EL | **EL2** — we own it (kernel installs `vbar_el2`) | EL2 *after* `fastboot oem pkvm disable`; else EL1 under pKVM |
| Above us during operation | SecureROM (EL3), iBoot, SEP — none run in the steady state; SEP is a separate die and never touches AP V-regs | TrustZone (EL3) + hypervisor — **active**, can preempt the AP via secure interrupt / SMC |
| `daifset` seals the section? | **Yes** — we own every vector; no one above autonomously runs | **Not fully** — a Secure interrupt to EL3 or an SError can enter above `daifset` |
| §3 freeze guarantee | Holds as written | Weaker — confirm true EL2 (`oem pkvm disable`) before trusting the masked section; SPRR/GXF governs execute/permission, not SIMD autosave, so it does not spill secrets |

M1 is the clean case and the active dev target. Pixel 8 Pro is PRIMARY but boxed; its secure-world preemption is the one place the shipped §3 seal is best-effort until the EL is verified on hardware.

---

## Open Questions (remaining)

- ~~**Exact NEON pinning in Rust**~~ — **answered.** Tier 2 on rustc 1.94.0, `-O`, `aarch64-unknown-none`: no spill (see Proven Result). Re-verify on toolchain bump via the CI canary.
- **Interrupt-masked critical-section length** — how long can key ops hold `daifset` without hurting latency? BLAKE3 over 256 bits is fast, but measure once real consume paths exist.
- **Tensor secure-world preemption** — confirm on hardware that `oem pkvm disable` gives true EL2 and that no secure interrupt breaches the masked section during a key op. This is the one open per-target risk to §3.
- **Multiple live secrets** — with quorum dropped, register pressure eases, but confirm the audit/no-spill result still holds when several inverted-pairs are live at once in one block.

---

## Custom silicon (later — where the dropped guarantees come back)

The two things stock silicon cannot honestly deliver both belong here, enforced by hardware **independent of the core that might be corrupted**:

- **Quorum + repair** — hardened registers with the completeness invariant in silicon (like PIPE's storage), where register spill is under our control and copy independence can be guaranteed, so majority-repair is trustworthy rather than theatrical.
- **Physical zeroize / VDD crowbar** — a dedicated integrity monitor that watches the core and can destroy secret state, precisely because it is *not* the thing that got flipped. This is the trustworthy version of the "short VDD to ground" idea: it lives in logic that a corrupted core cannot subvert.

Same principle as PIPE's completeness invariant living in hardware, not software: the watchdog is separate from the watched.

---

## Relationship to the TOKEN filing

The **property** — fault-tolerant, Byzantine, inverted-pair handling of identity secrets, detect-and-fail-safe, distinct from and complementary to hardware-level redundancy — is already claimed in the TOKEN provisional (the fault-tolerant-storage and opaque-storage claims). This doc is the **ferros implementation** of that property at the register layer; it adds no patent matter, it builds the claimed mechanism. On stock silicon the mechanism is **detect-and-freeze** (repair and physical destruct need hardware we do not yet own); on custom silicon it becomes **detect-repair-destroy** enforced by an independent monitor. The patent stays mechanism-agnostic (no register count, no repair-vs-freeze mandate); ferros picks NEON-on-stock-ARM with detect-and-freeze as the concrete substrate until custom silicon.
