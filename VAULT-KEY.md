# VAULT-KEY — anchor-key sourcing: seed integration on husky

**Status:** design/map. Grounds the FP5-flavored trust-domain design in `vault/src/anchor.rs` on husky (Pixel 8 Pro, Tensor G3, Trusty + Titan M2). Complements [VAULT-INDEX.md](VAULT-INDEX.md) (what the key protects) — this is *where the key comes from*. Captured 2026-08-19.

---

## Two different keys, don't conflate them

- **Authenticity key (seed, Ed25519, asymmetric, public).** Baked into `ferros_seed` at build time (`.rodata.pubkey`). Answers "is this the real ferros kernel?" — signature verification of code. Already built and working.
- **Confidentiality key (the AnchorKey, symmetric, secret).** 32 bytes. Answers "can I find and decrypt the vault?" — HMAC of the anchor ring, and (per VAULT-INDEX) the root of object/index encryption + keyed addressing. Currently a hardcoded `[0x5A; 32]` test constant.

"Proper vault + seed" = give the AnchorKey a real *source*. The seed is the natural place because it is the one component that is itself verified (self-verify) before it runs, so a secret it derives has a trustworthy origin. But sourcing a secret needs *inputs* the seed doesn't have yet — which is why this couples to display + input (below).

## The existing design (anchor.rs), and where husky differs

`anchor.rs` already specifies **two trust domains**: Domain A (key store — survives power loss, holds the AnchorKey) and Domain B (the storage device — anchors at key-derived secret offsets; without the key it's noise). The documented Domain-A paths and phases were written for FP5/Qualcomm (RPMB behind QTEE, a modifiable EDK2 ABL). Husky changes the specifics:

- **Secure world is Trusty, not QTEE** (we see `trusty_persist` / `trusty_userdata`). Same shape: RPMB auth key is fuse-derived, provisioned at first boot, held in Trusty; normal-world ferros can't reach RPMB directly.
- **Husky has a Titan M2 (the GSC — Google Security Chip).** This is the real hardware root: it runs **Weaver** (lockscreen secret combined with a chip-held secret, *with hardware throttling* — the thing that makes offline brute-force impossible) and StrongBox keymaster. It is the correct home for an owner-secret-bound device key — and it is proprietary and Google-controlled.
- **The Pixel ABL is closed** (Google publishes no EDK2 source), so FP5's "Phase 2: modify ABL to read RPMB → DTB → kernel" is *not* available the same way.

## The derivation we want

```
anchor_key = BLAKE3-derive_key("ferros.vault.anchor.v0",  owner_secret || device_root)
```

- **owner_secret** — a passphrase/PIN the owner supplies. Not on disk. Owner-bound. (Endgame: the PIPE/TOKEN hardware; see the PIPE memories — a boot secret is meaningless without TOKEN binding.)
- **device_root** — a per-device secret only *this* hardware produces. Not on disk. Device-bound, and ideally *throttled* (so a stolen flash image can't be brute-forced offline).

You want both: the vault opens only on *this device* with *this owner's secret*. Either alone is weaker (owner-only = a flash image is offline-guessable; device-only = a stolen powered device is openable).

## The sovereignty tension (name it honestly)

The only husky source of a *throttled* device_root is the GSC/Weaver — i.e. trusting Google's chip. That is a sovereignty compromise. The passphrase-only path is *more* sovereign (no Google dependency) but loses hardware throttling, so it must lean on a **memory-hard KDF (Argon2id)** over the passphrase to make offline brute-force expensive. The endgame resolves the tension: a device_root from hardware *ferros owns* (the PIPE enclave / a Glyph secure element), with a killswitch that zeroes it and cryptographically erases the ledger. Until then it's a deliberate choice per build: Google-throttled vs. Argon2-hardened-sovereign.

## Phased map (husky-specific)

**Phase 0 — now (chainload, no seed).** Linux jumps straight to the kernel shim; the seed isn't in the loop. AnchorKey is the `[0x5A; 32]` constant. Fine for the persistence proofs; not a real key.

**Phase 1 — passphrase-derived, no TEE.** `anchor_key = Argon2id(passphrase, salt)`, salt in a small `ferros_anchor` region (we already have the repartition tooling that carved the ferros partition — carve or reserve a slot). Owner-bound, device-independent, offline-brute-force-resisted by Argon2 cost. Feasible with zero secure-world access. The passphrase arrives via the bridge for now; via on-device entry once display + input exist (below).

**Phase 2 — chainload-Linux as key fetcher (the husky opportunity).** While we still boot via the Linux crutch, Linux *has* the access ferros@EL2 lacks: it can pull a device-bound, Weaver-throttled secret through Android keystore / Trusty and pass it into the handoff (the same one-way "read secret → stash in handoff → kernel reads → wipe" shape as FP5's ABL→DTB, but Linux is the trusted fetcher). Mixing that into the derivation gives device-binding + throttling without a custom bootloader. Cost: depends on Linux + Google's TEE (transitional, not sovereign).

**Phase 3 — ferros owns the root (endgame).** Cold boot ABL→seed→kernel; device_root comes from hardware ferros controls (PIPE enclave / Glyph CSR / a ferros-owned secure element), killswitch zeroes it. The seed is the verified gate that derives and hands off the key. This is the `anchor.rs` "Phase 3 (Glyph): own the stack."

## The seed's expanded role (cold-boot, Phase 3)

Extend `ferros_seed` from verify-only to **verified key-derivation gate**: after self-verify and kernel verify, it (1) collects the owner_secret via display + keypad/touch, (2) fetches device_root from the hardware root, (3) derives `anchor_key`, (4) hands it to the kernel in a register / reserved-memory slot, (5) wipes its own copy before jumping. Because the seed is itself verified before it runs, the derivation has a trustworthy origin. Note this adds a *symmetric KDF* (BLAKE3-derive / Argon2) to the seed, which today only does *asymmetric* Ed25519 verification.

## Dependency: this is why display comes next

Collecting an owner_secret *on device* needs a framebuffer + input (PIN pad / passphrase entry). So the fully-proper owner-bound vault is gated on display — exactly the next priority. Staging around it:

- Before display: passphrase via bridge (dev), or a build-time secret — better than `[0x5A; 32]`, not yet owner-entered.
- After display: real on-device owner-secret entry → Phase 1 becomes genuinely owner-bound; the seed's collect-secret step has a UI.

## Near-term concrete step

Replace the `[0x5A; 32]` hardcode with `anchor_key = Argon2id(passphrase, salt)`, salt persisted in a reserved `ferros_anchor` slot, passphrase supplied via the bridge. Small, host-testable, no secure-world or display dependency — and it turns the vault from "opens for anyone with the disk" into "opens only for someone with the passphrase," which is the first real security property. Device-binding (Phase 2 chainload-fetch or Phase 3 hardware root) layers on after.
