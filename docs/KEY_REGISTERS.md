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

## Off DRAM: where the ira and wairua can rest (measured 2026-10-28)

The design above holds a secret in V-registers only for the length of one asm block. It does not say where the ira and wairua (64 bytes together) rest between uses for a whole boot, and NEON cannot hold them there: the kernel saves every process's V0–V31 on each trap, LLVM cannot reserve a V-register, and the kernel's own Rust uses them. So the resting place is system registers that nothing in ferros uses and EL0 cannot read. `diag keyreg` (ferros_kernel/src/keyreg.rs) measured the candidates on husky's X3 (MIDR G#411FD4E0) and an A715 (G#411FD4D0); both agree.

**Bank 0: the pointer-authentication keys.** APIA, APIB, APDA and APDB are 128 bits each, 64 bytes exactly. Every bit holds, survives an SMC into EL3, and survives idle (`wfi`/`wfe`). ferros signs no pointers: SCTLR_EL2's EnIA/EnIB/EnDA/EnDB are 0, and with them clear PACIA left a pointer unchanged under a test key (measured at EL2; the same SCTLR_EL2 governs EL0 in the EL2&0 regime). The fifth key, APGA, has no enable bit: PACGA computes with it (measured). So APGA must never hold anything that is useful alone.

**Bank 1: the EL1 bank, dead under VHE.** HCR_EL2 has TGE=1 and E2H=1: nothing runs at EL1 and no exception is taken there, so these registers are never written by hardware. Full width, holding through SMC and idle, read in ~3 cycles: TPIDR_EL1, SP_EL1, ELR_EL12, FAR_EL12, MAIR_EL12, TPIDR_EL2. The EL1 timer compares (CNTP_CVAL_EL02, CNTV_CVAL_EL02) hold too but read ~30× slower (95 cycles on the X3, 47 on the A715). Partial width: PAR_EL1, SPSR_EL12, ESR_EL12, CONTEXTIDR_EL12, VBAR_EL12. Hold nothing: AMAIR_EL12, AFSR0/1_EL12. `diag keyreg dbg` adds the breakpoint/watchpoint value registers; it has not been run, since EL3 may trap debug access.

**Held split.** The layout (built, below) keeps each secret as two shares, so that a leak of one class of register (a fault report, a dump, a context EL3 saves) reveals nothing:

| | share 0 (bank 0) | share 1 (bank 1) |
|---|---|---|
| ira | APIA, APIB | TPIDR_EL1, SP_EL1, ELR_EL12, FAR_EL12 |
| wairua | APDA, APDB | MAIR_EL12, APGA, TPIDR_EL2 |

A share is uniformly random on its own, so APGA's PACGA oracle can only ever compute over a random value. Using TPIDR_EL2 gives up the usual per-core pointer register; the slow timer compares are the alternative.

**Cost** of fetching all 64 bytes, in the core's own cycles (G#10000 fetches each):

| | RAM (L1-hot) | bank 0 alone | split |
|---|---|---|---|
| X3 | 10 | 8 | 24 |
| A715 | 13 | 8 | 16 |

24 cycles on the X3 is about 8 ns, against thousands of cycles for the BLAKE3 that consumes the key.

**What it does not cover.**
- Each core has its own registers, so each core that serves SYS_IRA or SYS_ENTROPY needs its own copy, and handing one over goes through memory (a cache line written, read and zeroed within microseconds).
- Power-down wipes the registers. Husky's c2 (PSCI power_state G#10000, 160–220 µs exit) and system deep sleep both cut the core's power domain. The ira needs no carrying: it is re-derived from the fuses and UFS. The wairua is random and must be carried through sleep unchanged (owner, 2026-10-28), so it rests sealed outside the core while the core is off.
- Every use must run in hand-checked assembly that never touches the stack (the workflow above), or the key spills while it is consumed.

**Storage that survives sleep but dies on reset: none found (2026-10-28).** Such a place would close the one gap a DRAM seal leaves: someone who resets the phone into their own code before DRAM fades and re-derives the seal key. The always-on PMU was the candidate (`diag keyreg ao`, read through the EL3 monitor, which refuses 44 of the 64 cells at G#1546_0800-08FC):
- G#860-880 are CPU_INFORM 0-8 (flexpmu_cal_cpu_zuma.h): each core's requested power mode for the firmware (1 c2, 2 CPD, 3 SICD, 4 sleep). Tags written to CPU4's and CPU5's cells (owner's go) read back whole, were gone after a plain reset, and survived a kernel reload except one bit of CPU4's, cleared when the reload cycled that core. Live firmware state, rewritten at boot and by every power transition, deep sleep included. Restored to zero.
- G#800 (INFORM0, Linux's c2 allow), G#810 (the reboot mode) and the other INFORM cells exist to carry information across a reset, and the allow-list is by address, not by caller: an attacker's code after a reset reads them just as ferros does. On Samsung's older parts INFORM3 (G#80C) carried the upload-mode cause, so it was not written.
- G#840-85C hold boot information (G#840 went 1 to 0 across a reset).

So on stock silicon the wairua crosses deep sleep sealed in DRAM under a key re-derived from the fuses at wake (microseconds, no disk), and the reset-and-dump gap stays open until PIPE holds the wairua on its own power. That is still strictly better than now, where the wairua sits in DRAM in the clear for the whole session.

## Built: the ira and wairua in registers (2026-10-28)

Neither secret has a RAM copy any more. KOHI and vault boot hand theirs over the moment they exist, and their copies are wiped.
- **The code that touches them** is keyreg/keys.rs: safe Rust except the system-register access, with BLAKE3's compression as NEON intrinsics. `tools/keyreg-lift.sh` compiles it with the kernel's own flags and freezes the output as keyreg/keys.s, which the kernel links (ferros_kernel/src/keys.rs). The lift refuses any output that names `sp`, uses v8–v15, or calls out; `--check` regenerates and diffs, for CI. Getting there took three refusals:
  - helpers that were not inlined passed state through the stack;
  - a strict-align target bounced vector rows off the stack (fixed by 16-byte-aligned outputs);
  - at -O3 the seven unrolled rounds ran out of registers.

  At -O2 the round loop stays rolled: 459 instructions, no stack.
- **Calls:** each runs with interrupts masked and ends in `keyreg_scrub`, which zeroes the caller-saved registers before they unmask.
- **What uses them:** `SYS_IRA`, `SYS_ENTROPY` (the same stream as before, now computed where the wairua is held), the capability key, the vault nonce seed and the wairua's fingerprint. All derive through `keyreg_derive`/`keyreg_stream` and get back only the derived bytes.
- **Self-test:** a known-answer check against the blake3 crate runs before the first secret is set; a failure means no secret is held at all.
- **Cores:** CPU0 seals both secrets for the X3 and the workers at the hop and clears its own registers. Each core takes the sealed blob in its loop before running a slice, and the blob is wiped when the last one has it (`diag keys`: X3 and all four workers at the current generation, blob waiting for none).
- **Power-down:** `keys::leave` seals and clears; `serve`/`ensure` unseal on return. `diag keys sleep` rehearses it on the X3: the wairua's fingerprint is unchanged. Seal and clear take 130 µs, the unseal 128 µs. Nearly all of that is reading the fuses for the seal key, since unsealing takes its masks from the seal and re-splits from the wairua's stream instead of calling the firmware TRNG, which costs about 1.3 ms a call. ferros does not power cores down yet; the c2 and deep-sleep paths call these when they land.
- **Verified from EL0** (payloads/keytest): two entropy draws differ, the ira key for a domain is the same twice, and another domain gives another key.

**Still in DRAM:**
- keys *derived* from the ira and wairua and kept by their users: the vault's AnchorKey and the oracle's system secret (from the ira at vault boot), and the capability key and nonce seed (each derived per use);
- residue inside blake3 and KOHI's own frames at the moment of derivation;
- the sealed blob, during a handoff or a sleep.

**The check (2026-10-28), §2 rebuilt across cores.** The inverted pair needs a third register a word, and there are none left. A parity word in a spare register would be a linear function of the secret, so it would leak part of the key if that register leaked, and it misses some two-bit flips. So the check is a one-way tag instead:
- **The tag:** per secret, 64 bits of `derive_key("ferros keyreg check v0", secret)`, kept in RAM. Like the wairua's fingerprint, it reveals nothing about the secret.
- **When it runs:** each core checks its copy about every quarter second in its loop, and before every derive (only the secret being used). One check costs a compression, about 420 cycles on the X3.
- **On a disagreement:** the cores that checked within the last second decide. If they agree with this core, the tag is what flipped and is replaced. Otherwise this core's copy is cleared and it takes a sealed copy from a core that passes; a holder checks itself before sealing.
- **Why repair is trustworthy here:** §2 dropped quorum repair because one core's registers can fail together. Five separate register files with different masks don't flip alike.
- **Fail closed:** if no core passes, the secret gives nothing, never a wrong key.
- **Odds:** a random flip escapes the check with odds 2⁻⁶⁴.
- **Measured:** `diag keys flip` flips one bit of the X3's wairua share. Every time, it was caught and repaired from a worker in 270–280 µs, with the wairua unchanged.
- **Cost:** the context-key cache (hashing the context had been ~1,100 cycles of each derive) more than pays for it. A checked derive costs 1,245 cycles; an unchecked one cost 1,461–1,616 before.

`SYS_ENTROPY` blocks are not checked one by one; the quarter-second check covers them.

## The identity's keys in the kernel alone (design, 2026-11-17; not built)

The owner, 2026-11-17: "As long as all this is priveliged and in the kernel so once we lock it there's ZERO way someone can get into those keys (as far as we know, proofs are obviously ideal but we get what we get)".
"Those keys" are the identity's roots (the identity seed and the handle proof, which ahau derives from the typed handle) and two random keys: the fleet key (fgtw recovers it from the fleet's fan-out) and the person key (one per identity, kept in the identity space sealed under the fleet key, FACETS.md D0).

### Where they are now (after branch key-custody-lock)

- **ahau** derives the seed from the handle and the proof from the seed, unlocks (SYS_UNLOCK op 1), hands both to fgtw (op 4) and overwrites every copy it made; its modes and fields hold none of them any more.
  A tag unlock (op 6) hands the seed from the kernel to ahau.
- **fgtw** holds the seed, the proof, the fleet key and the person key in its own memory for as long as the person is present, and wipes them when the kernel locks (the identity bell, `BELL_IDENTITY`) or when other roots come.
  Its wipe is volatile writes and a compiler fence: the zeros may sit in a dirty cache line, and copies the code made by value (each `let (Some(hp), Some(seed)) = (self.hp, self.identity_seed)`, the fgtw crate's own temporaries, freed heap) stay in its arc until the process ends.
- **The kernel** (kvault) holds the seed (for `tag_keep`), the three keys of the identity's vault class and the person key (handed in by fgtw, op 10), and derives each kist's alias from the person key.
  At lock it overwrites them in place and cleans the lines to DRAM (`keys::wipe_to_dram`).
- **The vault process** sees `fgtw.identity_seed`, `fgtw.handle_proof` and `fgtw.fleet_key` in the clear while unlock at boot keeps them (they are identity-class vault entries it seals and caches).

### The proposal

fgtw asks the kernel to sign, seal, open and derive, and never holds the seed, the fleet key or the person key.
The kernel keeps them beside the identity's vault keys (`IdKeys`), wipes them all at lock, and answers only for the identity present.
The calls are new SYS_UNLOCK ops (or a syscall of their own) behind a right fgtw alone holds (open question 4).

**What fgtw does with them today, and the call that would replace each:**

| fgtw today (payloads/fgtw, the fgtw crate) | Key | Kernel call |
|---|---|---|
| `join::adopt_chain` (`genesis_identity_matches`), `bindreq_list`'s check of each request's co-signature, the v2 genesis in `ensure_member` (it carries the identity's public key, no identity signature) | seed | `identity_public` → Ed25519(seed)'s public key, 32 bytes |
| `join::bind_request` and `bindreq_put`: the binding request co-signed by Ed25519(seed) (the registry's anti-spam gate) | seed | `identity_sign_bindreq(device key, hp, t, tap commitment)`: the kernel builds `bind_signing_bytes` itself and signs them; it signs nothing else (no general signing oracle) |
| `pair::device_name_default(device, seed)`: every device's two-word name (the Fleet page, op 19, the green screen, `say_device`) | seed | `identity_name(device key)` → the 8 bytes the name is made from |
| `pair::masked_device_words(device, seed)`: the pairing words, shown and matched (`show_words`, op 1, the add machine) | seed | `identity_word_mask` → `word_mask(seed)`, 32 one-way bytes; the XOR with a device key and the words stay in EL0 |
| `fanout_open` (`join::open_fanout`, `client::recover_fleet_key`): this device's wrap opened with its X25519 secret and the seed | seed, device key | `fleet_open(hp, kfp, wraps)`: the kernel finds this device's wrap by its commitment, derives the wrap key, decrypts, checks the fingerprint and keeps the fleet key; it answers the fingerprint only |
| `mint_fleet_key`: a fresh fleet key, wrapped to each member | seed | `fleet_mint` (drawn from KOHI and kept) then `fleet_wrap(hp, members)` → the wraps |
| `fanout_grow_held` (add.rs `Grow`): the held key wrapped again to a grown member set | seed, fleet key | `fleet_wrap(hp, members)` |
| `fgtw::space::seal` / `open` under `space_key(fleet key)`: the identity space kept, pushed to siblings, put to and pulled from fgtw.org | fleet key | `fleet_seal(SPACE, aad, doc)` / `fleet_open_doc(SPACE, aad, sealed)`, or a per-document key (open question 1) |
| `person::find` / `settle` (space.rs `space_person`, `space_person_local`): the person fact opened, minted, sealed again under a new fleet key | fleet key, person key | `person_open(sealed fact)` → its id and mint stamp, the key kept by the kernel; `person_mint` → a sealed fact (the eggs: KOHI, the jitter egg EL0 hands in, the fleet key, all inside); `person_reseal(sealed fact)` under the current fleet key |
| SYS_UNLOCK op 10 (the person key handed in) | person key | gone: the kernel opened it itself |
| `person_eggs` (the fleet key's bytes as one egg of the person key's mint) | fleet key | inside `person_mint` |
| `persist_set` of the seed, the proof and the fleet key while unlock at boot is on | seed, fleet key | the kernel keeps the fleet key beside `ID_UNLOCK` (the kept seed) in its own vault, which the vault process never reads (`KERNEL_HELD`'s rule) |
| the add machine (fgtw::flow `Add`, `kept::read_add`) built with the seed for names and words | seed | built with the word mask and a name call instead (a change to the fgtw crate) |

**What stays in EL0:**
- the handle proof: public by design (beacons, the registry, `ahau.proof` in the identity space); fgtw passes it in to the calls that bind it;
- the chain, its fold and the members, the protocol machines, Photon Transport, fgtw.org's HTTP;
- the device keys (`DeviceKeys` from `ira_key("fgtw device")`: Ed25519, Falcon, SPHINCS+), which sign PT frames and fleet ops (but see open question 0);
- the one-way values the calls answer: the identity's public key, fleet key fingerprints, person key ids, device names, the word mask;
- the space's plaintext (the person's kists, places, answers), which fgtw syncs into this device's vault anyway;
- each kist's alias seed, which the kernel hands to that kist alone (SYS_IRA's alias bit, as now);
- ahau, which reads the handle as it is typed, so the seed exists in its memory until the unlock (op 1) and is overwritten there; with this design it would no longer hand the seed to fgtw at all, only the proof, and a tag unlock (op 6) would stop returning the seed (ahau takes the proof from `ahau.proof`).

**The cost in kernel code.**
The kernel already links what these calls compute with: BLAKE3 with its XOF, XChaCha20-Poly1305, Ed25519's key expansion, SHA-512 and its field and scalar arithmetic (curve25519-dalek, for the facets stamp and capsule checks), about 50 KB of the 1.37 MB of text in the current Pixel build.
What is new: Ed25519 signing (another SHA-512 pass and a scalar product, a few KB), the X25519 ladder if the kernel wraps with its own ephemeral keys (a few KB, the field arithmetic already there), and the handlers, a fleet key slot and the person fact's seal (about 4 to 8 KB).
So about 10 to 20 KB, around 1 % of the kernel's text, and 64 bytes of RAM.
The time: everything but the space's seal is microseconds.
The space document is up to 1 MiB, and the kernel's XChaCha20-Poly1305 (the portable backend: the kernel is built without `chacha20_force_neon`) opens about 309 MB/s on the X3: about 3 ms, too long under the kernel lock (a 300 µs hold already showed in `diag lock`, kvault `bound`), hence open question 1.

**What it protects, by the threat model (harm, not observation; `threat_model_harm_not_observation`).**
- The fleet key and the person key are the unforgeability: with the fleet key one writes the person's space (kists stowed on all their devices, a person key of one's own choosing, so every kist alias of theirs); with the person key one derives every alias and speaks to kists as the person.
  Kept in the kernel, no EL0 bug, leak or compromised process can carry them away: a disclosure of fgtw's memory, a console or crash report, a copy kept past the lock.
  What a compromised fgtw still has is the calls, on this device, only while the person is present; a lock ends it at once, and nothing of the keys is left in EL0 to wipe.
- After a lock nothing of the identity is anywhere a process can read, and the kernel's copies are zeros in DRAM.

**What it does not protect.**
- The seed's secrecy is the handle's: the seed is BLAKE3 of the handle's `x` encoding (ahau_core), milliseconds to compute, and the fgtw crate says the same of it (fleet.rs: "a PUBLIC function of the handle").
  Whoever knows the handle has the seed whatever the kernel does; the kernel only keeps it out of process memory.
  The bind request's co-signature is a knowledge gate, not ownership (photon docs/fleet-identity-remediation.md).
- The fan-out: a wrap's key is BLAKE3 over the X25519 shared secret of this device's key and the seed.
  fgtw keeps the device's Ed25519 key, whose scalar is that X25519 secret (`fanout_open`'s `to_scalar_bytes`).
  So a compromised fgtw that also knows the handle opens this device's wraps itself, and the fleet key is in EL0 again, unless the X25519 half moves into the kernel too (open question 0).
  The same holds for `fleet_wrap`: if EL0 made the ephemeral keys and handed the kernel the shared secrets, it could open the wraps it got back; the kernel makes them itself.
- The calls themselves: while the person is present, whatever fgtw may ask, a compromised fgtw may ask (open a space, sign a binding request, mint a person fact over a missing one).
  The kernel bounds when and where, not what.
- The kernel itself (EL2) is the trust; a kernel bug that reads arbitrary memory reads the keys.

**What a locked device still exposes.**
- Nothing of the keys in the kernel's vault keys or fgtw (wiped in place, cleaned to DRAM).
  Residue a lock does not reach: BLAKE3's and the ciphers' frames on kernel stacks at the moment of derivation (as for the ira, "Still in DRAM" above), and kernel heap buffers wiped with a plain `fill` (the IPC path's `deliver`, which carries the person's domain keys in a forwarded store call's stamp: the hot path, left as it was).
- EL0 frames: whatever a process copied stays in its arc until it ends; a frame given back is zeroed (frames.rs `give`), so a process that ends at lock leaves nothing (open question 7).
- Cold RAM while unlocked: the keys sit in the kernel's heap in the clear; a power cut while the person is present leaves them to decay in DRAM, and a watchdog reset cannot run `before_reset` (AUTH.md "Known gaps").
  Sealing them under the wairua, which lives in registers only, would leave DRAM holding ciphertext (open question 9).
- DMA: the S2MPU fences hold the modem (HSI1), the WiFi chip (HSI2) and BLK_MISC's masters (AUTH.md "Radios while locked").
  Masters a process programs are not fenced from the kernel's frames: the GPU's MMU (payloads/gpu), the camera front end with its SYSMMUs in bypass on contiguous frames (process.rs `cam_ask`), the UFS controller.
  A compromised driver of one of them reaches any DRAM, the kernel's keys included, while the person is present (open question 10).
- At rest: unlock at boot keeps the seed (`ID_UNLOCK`) on UFS, and today the vault process keeps fgtw's roots and fleet key too; that is the setting's own warning.

### Open questions

0. The fan-out's X25519: either the kernel holds the device's Ed25519 key as well (derived from the ira as `ira_key("fgtw device")` derives it), and PT frames and fleet ops are signed thru it, or each device gets a kernel-only X25519 key for its wrap, a change to the fan-out's format across Photon.
   Without one of these the fleet key is only as kernel-held as the handle is unknown.
1. The space's seal: a per-document key wrapped under the fleet key, EL0 doing the bulk (a change to the space's sealed format; a leaked document key opens one document), or the bulk on a kernel worker outside the lock.
2. The handle proof: listed among the roots, but public (beacons, the registry, `ahau.proof`). Keep it in EL0?
3. Is the seed worth the calls, given that it is a function of the handle? The alternative: the kernel keeps only the two random keys (fleet, person), and fgtw keeps the seed until the lock, as now.
4. The right: a new one for fgtw alone, apart from Unlock (which ahau and init also hold).
5. The scope of the calls: only kernel-built binding request bytes are signed, only named purposes are sealed and opened; is any general call needed (fstate, which Photon seals under the fleet key, is not on ferros yet)?
6. The fgtw crate takes the keys as bytes (`&[u8; 32]`) thruout; it needs trait objects for the identity's and the fleet's operations (Photon on other platforms implements them with the bytes, ferros with the calls). The crate is another session's: a branch there.
7. At lock, end fgtw (its frames are zeroed on the way back to the pool) as well as, or instead of, the in-process wipe.
8. Unlock at boot: the fleet key kept by the kernel beside `ID_UNLOCK`, the vault process never seeing it.
9. Holding the keys in DRAM sealed under the wairua (registers), unsealed per use: against cold RAM and DMA; a checked derive is about 1,245 cycles a use.
10. Fencing the kernel's key frames from the masters processes program (GPU, camera, UFS), or keeping the keys out of the frames those masters can reach.
11. The race between ahau's unlock and fgtw's op 4: fgtw takes the epoch at op 4, so a lock between the two would be missed; nothing locks in that window today, and with a lock op in use ahau should hand over the epoch it unlocked at.
12. The IPC path's message wipe (`deliver`, a plain `fill` on every message): a volatile wipe of the store stamps alone, or of every message.

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
