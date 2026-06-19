# SECURITY_CHAIN — ferros Boot Trust Model
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Every link verifies the next. The owner holds the root.

---

## What This Document Defines

The complete chain of trust from power-on to userspace. Every step
verifies the next before handing off control. No step trusts blindly.
No step can be silently bypassed.

```
Power on
  → ABL (Qualcomm bootloader, not ours, runs first)
  → Seed (our trust anchor, rarely flashed)
  → Kernel (signed, verified by seed before jump)
  → Bootloader stage (kernel first code — scans vault root, restores state)
  → Userspace (cap-gated, no raw access to anything)
```

---

## The Chain

### Link 0: ABL (Android Bootloader)

Not ours. Qualcomm's UEFI-based bootloader. Runs before we exist.

```
ABL responsibilities:
  Load PE/COFF image from boot partition
  Check Android Verified Boot state (orange = unlocked)
  Display nag screen (10 seconds, unavoidable until we sign properly)
  Jump to our seed

ABL trust model:
  We do not trust ABL
  ABL does not trust us (orange state)
  Mutual distrust is correct — ABL is vendor code

Future: properly signed bootchain eliminates nag screen
  ABL checks vbmeta signature → if valid, no nag
  Requires signing with a key ABL recognizes
  Or: custom TF-A replaces ABL entirely (long-term goal)
```

### Link 1: Seed (Trust Anchor)

The first code we control. Minimal. Rarely updated. The root of our
trust chain.

```
Seed is:
  A small binary flashed to the boot partition
  Contains the developer's Ed25519 public key (baked in at flash time)
  Contains its own Ed25519 signature at a known offset

Seed does exactly three things:
  1. Verify own integrity
     Compute BLAKE3(seed_code) at runtime
     Ed25519_verify(seed_signature, computed_hash, embedded_pubkey)
     Same mechanism as kernel check — signature proves origin
     If fail → try copy B (see Redundancy below)
     If both copies fail → display "SEED INTEGRITY COMPROMISED", halt

  2. Verify kernel binary
     Located at known DRAM address (loaded by ABL as part of boot image)
     Compute BLAKE3(kernel_binary) at runtime
     Ed25519_verify(kernel_signature, computed_hash, embedded_pubkey)
     If fail → try copy B
     If both copies fail → display "KERNEL INTEGRITY COMPROMISED"
     User can override (it's their device) or halt

  3. Jump to kernel
     Same mechanism as hot-reload: branch to entry point
     Seed's job is done

Seed does NOT:
  Scan any ring or storage
  Load any state
  Manage any capabilities
  Touch any peripheral beyond framebuffer (for warnings)
  Know what the kernel will do after jump

Redundancy (4x on UFS, 2x on SD):
  Each critical binary has two copies per device, spaced >= 1MB apart.
  Physical separation protects against localized flash block failure.

  UFS layout:
    Seed copy A    @ LBA offset 0MB
    Seed copy B    @ LBA offset 2MB    (>= 1MB separation)
    Kernel copy A  @ LBA offset 4MB
    Kernel copy B  @ LBA offset 8MB    (>= 1MB separation)

  SD layout (mirror):
    Seed copy A    @ LBA offset 0MB
    Seed copy B    @ LBA offset 2MB
    Kernel copy A  @ LBA offset 4MB
    Kernel copy B  @ LBA offset 8MB

  Verification order:
    Try UFS copy A → fail → try UFS copy B
    Both UFS fail → try SD copy A → fail → try SD copy B
    All four fail → halt with integrity error

  Total: 4 copies on UFS + SD combined, 2 copies per device
  Any single flash block failure: survivable
  Any single device failure: survivable
  Cost: ~4MB total (negligible on 232GB UFS + 953GB SD)
```

### Link 2: Kernel / Bootloader First Stage

The kernel binary that seed verified and jumped to. This is
`ferros_kernel` — the same binary we hot-reload during development.

```
Kernel first stage does:
  1. Hardware init (same as current boot sequence)
     FB console, UART, DTB parse, pstore, SPMI, GCC, RPMh

  2. Storage init
     UFS controller probe (UFSHCI, already enabled by ABL)
     SD card probe (SDHCI, GCC clocks, RPMh power)

  3. Spine scan
     Read spine (vault root ring) from UFS (binary search for highest gen)
     Mirror check: compare UFS and SD spine generations
     Higher valid generation wins

  4. State restoration
     Load HAMT root from spine entry (hash + lba)
     Restore capability table via HAMT lookup
     Restore process snapshots via HAMT lookup

  5. Userspace handoff
     Start Ledger server (receives pre-boot log buffer)
     Start other servers
     Each receives appropriate caps
     Kernel becomes just another client
```

### Link 3: Userspace

Everything after handoff. Cap-gated. No raw hardware access.

```
Every userspace process:
  Holds only the capabilities it was granted
  Cannot access vault objects outside its namespace
  Cannot forge capabilities (BLAKE3 preimage resistance)
  Cannot access hardware directly (kernel mediates all I/O)

Critical partition protection:
  Seed partition: NO cap issued for read or write
  Spine: kernel-only, no userspace cap exists
  UFS raw blocks: kernel mediates all I/O, no direct access
  Even kernel privilege denies raw read/write to seed/bootloader regions
  Only the flash tool (ferros-mkimg via fastboot) can write the seed

Ledger server:
  Cap<Write, Ledger::*> categories
  Append-only event chain
  Cannot read vault boot snapshots

App processes:
  Cap<Write, Vault::App::<hash>>
  Cannot touch other apps or system objects

Cryptographic access control (survives physical disk access):
  Per-namespace content key, wrapped per-reader via X25519 + ChaCha20-Poly1305
  Physical disk read without private key → ciphertext only
  See VAULT.md Access Control for full model
```

---

## Developer Key

The Ed25519 keypair used to sign seed and kernel binaries.

```
Key generation:
  Developer generates Ed25519 keypair offline
  Private key: never leaves developer's machine
  Public key: embedded in seed binary at build time
  Public key hash: BLAKE3(pubkey) — the identity anchor

Signing flow (both seed and kernel use the same mechanism):
  ferros-mkimg builds the binary (seed or kernel)
  Computes: BLAKE3(binary)
  Signs: Ed25519_sign(private_key, BLAKE3(binary))
  Embeds signature at known offset in the binary

  Seed binary contains:
    seed_code
    VsfType::ke(developer_pubkey)        ← Ed25519 pubkey, baked in
    VsfType::ge(seed_signature)          ← Ed25519 sig of BLAKE3(seed_code)

  Kernel binary contains:
    kernel_code
    VsfType::ge(kernel_signature)        ← Ed25519 sig of BLAKE3(kernel_code)
    VsfType::hp(developer_pubkey_hash)   ← BLAKE3 of pubkey, cross-reference

Verification flow (seed):
  Read kernel_hash, kernel_signature from known offsets
  Compute BLAKE3(kernel_binary), compare to kernel_hash
  Ed25519_verify(kernel_signature, kernel_hash, embedded_pubkey)
  Both must pass
```

---

## Owner Sovereignty

The device belongs to the owner, not the developer. The owner can
replace the developer's key with their own.

```
Default state (as shipped):
  Seed has developer pubkey
  Kernel signed by developer key
  Chain: developer → seed → kernel

Owner takes control:
  Owner generates their own Ed25519 keypair
  Owner reflashes seed with their pubkey
  Owner signs kernel with their private key
  Chain: owner → seed → kernel
  Developer key no longer involved

This means:
  Owner can build and sign their own kernel
  Owner can audit the kernel source and verify the binary
  Owner is not dependent on developer for updates
  Owner cannot be locked out by developer revoking a key

This also means:
  Owner is responsible for what they sign
  A compromised owner key = compromised device
  No remote revocation (by design — your device, your risk)
```

---

## Failure Modes

Every link has explicit failure behavior. No silent degradation.

```
Link 0 failure (ABL):
  ABL is vendor code, can fail in vendor-specific ways
  Worst case: ABL refuses to boot (bricked by vendor update)
  Mitigation: dual-slot A/B, fastboot recovery
  Our code never touches ABL's state

Link 1 failure (Seed self-check):
  Compute BLAKE3(seed), verify Ed25519 signature
  Copy A fails → try copy B (same device)
  Both copies on UFS fail → try SD copies (A then B)
  All four fail:
    Cause: flash corruption, incomplete flash, bit rot, tampering
    Action: display "SEED INTEGRITY COMPROMISED" on FB, halt
    Recovery: reflash seed via fastboot
    Cannot proceed — trust anchor is broken
  One copy fails, others pass:
    Boot succeeds from good copy
    Log degraded copy location for repair on next write

Link 1 failure (Kernel verification):
  Compute BLAKE3(kernel), verify Ed25519 signature
  Copy A fails → try copy B (same device)
  Both copies on UFS fail → try SD copies (A then B)
  All four fail:
    Cause: kernel corrupted, modified, signed by wrong key, or unsigned
    Action: display "KERNEL INTEGRITY COMPROMISED"
    User choice: override and boot anyway, or halt
    Override is informed consent, not silent degradation
    Owner always has the final say
  One copy fails, others pass:
    Boot succeeds from good copy
    Repair bad copy from good copy on next write opportunity

Link 2 failure (Spine scan):
  No valid entries found:
    Cause: first boot, or all entries corrupt
    Action: genesis — create first spine entry
    System boots into fresh state

  Corrupt entry (BLAKE3 mismatch):
    Action: skip, try previous generation
    Binary search eliminates corrupt entries efficiently

  UFS and SD disagree (different generations):
    Action: use higher valid generation
    Resync lower device from higher after boot

  Both UFS and SD corrupt:
    Action: genesis — fresh state
    All persistent state lost
    User data on SD card: still readable if filesystem intact

Link 3 failure (Userspace):
  Process crash:
    Microkernel: process restarts, kernel unaffected
    Cap state: restored from vault snapshot

  Capability forge attempt:
    BLAKE3 preimage: 2^-256 probability
    Effectively impossible

  Ledger server crash:
    Restart, chain valid thru last committed entry
    Pre-boot buffer preserved in kernel memory
```

---

## Spoofing Prevention

```
Attack: flash modified kernel to extract data
  Prevention: seed rejects — signature doesn't match
  Unless: attacker has the developer/owner private key

Attack: replace seed to disable signature check
  Prevention: seed has its own BLAKE3 self-check
  But: attacker with fastboot access CAN reflash seed
  This is physical access — if they have your device unlocked,
  they can do anything. That's physics, not software.

Attack: MITM the flash process
  Prevention: ferros-mkimg signs locally on developer machine
  Signature travels with the image, verified on device
  Tampered image → signature mismatch → rejected

Attack: physical disk read to extract user data
  Prevention: per-namespace content key, X25519-wrapped per reader
  Disk read without matching private key → ciphertext
  Content key never on disk in plaintext — only wrapped copies
  Device key in CSR (PAC registers) — not readable from disk

Attack: supply chain (malicious developer key)
  Prevention: owner can audit source, build, sign with own key
  Open source: anyone can verify what the kernel does
  Owner sovereignty: replace developer key if trust is lost
  Reproducible builds: Rust compilation is deterministic —
    build from source on your platform, compare BLAKE3 hash
    to the developer-signed binary. If they match, the binary
    IS the source code compiled. No trust in the developer
    required — math proves it.
```

---

## Comparison to Android Verified Boot

```
Android Verified Boot (AVB):
  Google holds root key
  OEM holds intermediate key
  Bootloader locked: only Google/OEM-signed images boot
  Unlocked: permanent "orange state" warning, some features disabled
  User cannot sign their own images without permanent penalty
  Chain: Google → OEM → bootloader → kernel
  Owner is NOT in the chain

ferros:
  Developer holds signing key (open source, auditable)
  Owner CAN replace developer key with their own
  No "locked" vs "unlocked" distinction
  No features disabled for using your own key
  Warning for unsigned kernel (informational, not punitive)
  Chain: developer/owner → seed → kernel
  Owner IS the root of trust (if they choose)

Key difference:
  AVB protects Google's interests (DRM, SafetyNet, carrier lock)
  ferros protects the owner's interests (integrity, sovereignty)
  AVB: "you may use this device as Google permits"
  ferros: "this is your device, here's the state of its software"
```

---

## Nag Screen Elimination

```
Current state:
  ABL shows 10-second warning ("orange state — unlocked bootloader")
  Cannot be bypassed without signing our bootchain

Path to elimination:
  Option A: Sign with a key ABL recognizes
    Requires: understanding ABL's vbmeta verification
    Risk: ties us to Qualcomm's key infrastructure

  Option B: Custom TF-A (Trusted Firmware-A)
    Replace ABL entirely with our own first-stage bootloader
    Full control of boot process
    Eliminates all vendor boot restrictions
    Significant effort — TF-A for QCM6490 is not trivial

  Option C: Accept the nag (pragmatic)
    10 seconds on cold boot only
    Hot-reload bypasses it entirely
    Users who care can build custom TF-A later

Current recommendation: Option C for now, Option B long-term
```

---

## Formal Properties

```
Theorem SecurityChain_Integrity:
  ∀ boot sequence:
    seed verified ∧ kernel verified →
      kernel binary == developer-signed binary
    Probability of undetected modification:
      BLAKE3 collision: 2^-256
      Ed25519 forgery: 2^-128
      Combined: negligible

Theorem SecurityChain_OwnerSovereignty:
  ∀ owner actions:
    owner can replace developer_pubkey in seed
    owner can sign kernel with own key
    no remote revocation possible
    no feature degradation for using own key
    owner trust = developer trust (structurally identical)

Theorem SecurityChain_FailureExplicitness:
  ∀ failure f in chain:
    f produces visible indication (FB console message)
    f has defined recovery path
    f does not silently degrade to insecure state
    f does not silently skip verification

Theorem SecurityChain_PhysicalAccessBound:
  physical_access(device) → can_reflash(anything)
  This is a hardware fact, not a software failure
  ferros does not pretend to resist physical access
  ferros detects tampering, does not prevent it
```

---