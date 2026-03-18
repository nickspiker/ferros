# SEED — ferros Trust Anchor
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Verify everything. Trust nothing. Be tiny.

---

## Purpose

The seed is the first code that runs after ABL hands off control. Its sole job is to establish the trust chain: verify its own integrity, verify the kernel's integrity, and jump. Nothing else.

The seed is the smallest possible trust anchor. It contains no drivers beyond what ABL left running, no allocator, no display beyond error output, no USB, no power management. It runs for under one second and never returns.

---

## Execution Flow

```
ABL loads seed as PE/COFF image (same as current kernel)
  ↓
1. Compute BLAKE3 of own binary
   - Hash from byte 0 to signature offset (known constant)
   - Signature lives at a fixed offset near or at end of binary
   ↓
2. Verify Ed25519 signature against embedded public key
   - Public key: 32 bytes, baked into .rodata
   - Signature: 64 bytes at known offset
   - If mismatch → display "SEED INTEGRITY FAILURE", halt
   ↓
3. Read kernel binary from UFS
   - Kernel stored at known LBA (primary copy)
   - Use ABL's existing UFS controller state (HCE=1, link up)
   - Read via SCSI READ(10) through UTRD ring
   - Load into staging DRAM address
   ↓
4. Compute BLAKE3 of kernel binary
   - Hash entire kernel image (known size, stored alongside kernel)
   ↓
5. Verify Ed25519 signature of kernel hash
   - Kernel signature stored adjacent to kernel on UFS
   - Same public key as seed self-verification
   - If mismatch → try backup copy (different LBA, >1MB apart)
   - If backup also fails → display "KERNEL INTEGRITY FAILURE", halt
   ↓
6. Jump to kernel entry point
   - Same mechanism as hot-reload: branch to staging address
   - Kernel takes over, never returns to seed
```

---

## Storage Layout

```
UFS LUN0:
  LBA range A:  Seed copy A        (seed binary + signature)
  LBA range B:  Seed copy B        (>1MB from A)
  LBA range C:  Kernel copy A      (kernel binary + signature + size)
  LBA range D:  Kernel copy B      (>1MB from C)

SD card (mirror):
  LBA range E:  Kernel copy C      (same format as UFS)
  LBA range F:  Kernel copy D      (>1MB from E)
```

Four kernel copies total: UFS-A, UFS-B, SD-C, SD-D.
Two seed copies: UFS-A, UFS-B (seed not on SD — it's loaded by ABL from boot partition).

Each copy is a contiguous block:
```
[kernel binary: N bytes, 4KB-aligned]
[VSF signature document: see below]
[padding to 4KB alignment]
```

---

## Signature Format

The signature block is a minimal VSF document. All lengths are EWE-encoded. Type tags are standard VSF single-byte codes:

```
VSF document (flat, no sections):
  VsfType::hp(binary_hash)        ← 'h' 'p' [EWE len] [32 bytes BLAKE3]
  VsfType::ge(signature)          ← 'g' 'e' [EWE len] [64 bytes Ed25519 sig of hp]
  VsfType::ke(public_key)         ← 'k' 'e' [EWE len] [32 bytes Ed25519 pubkey]
  VsfType::u(binary_size)         ← 'u' [EWE encoded size]
```

The seed parses this with minimal logic — scan for 2-byte type tag, read EWE length, extract fixed-size payload. No VSF section parser, no dictionary keys, no nesting. The type tags and EWE encoding ensure forward compatibility (new fields can be appended without breaking old seeds) while keeping the parser under 50 lines.

The seed's .rodata also contains the public key (for self-verification). The ke field in the signature document is for auditability — anyone can inspect the on-disk signature and see which key signed it.

---

## Public Key Management

```
Developer key:
  Developer holds Ed25519 private key
  Public key embedded in seed .rodata at build time
  ferros-mkimg signs kernel binary during build
  ferros-mkimg signs seed binary during build

Owner override:
  Owner audits source, builds from source
  Reproducible build: BLAKE3 of binary matches developer-signed version
  Owner can generate own Ed25519 keypair
  Re-flash seed with owner's pubkey embedded
  Re-sign kernel with owner's private key
  Chain: owner → seed → kernel (full sovereignty)

Key rotation:
  New key → new seed flash (rare)
  New seed signs new kernel
  Old seed copies become invalid (signature check fails)
  Old kernel copies become invalid (wrong pubkey)
  Dual-copy layout means rotation is atomic:
    1. Write new seed to copy B
    2. Write new kernel to copy B
    3. Verify both B copies
    4. Switch boot pointer to B
    5. Update copy A with new versions
    6. Verify copy A
```

---

## Failure Modes

```
Seed copy A corrupt:
  ABL tries copy B (via A/B slot mechanism)
  If B also corrupt → fastboot recovery

Kernel copy A corrupt:
  Seed loads copy B from UFS
  If B corrupt → try SD copies C, D
  If all four corrupt → display error, halt
  User can re-flash via fastboot

Signature mismatch (tampering):
  Display "KERNEL NOT SIGNED BY EXPECTED KEY"
  Display pubkey fingerprint for user verification
  Halt — do not boot unsigned code
  User can override by re-flashing seed with new pubkey

Power loss during key rotation:
  Dual-copy: at least one valid copy always exists
  Worst case: one copy old key, one copy new key
  Seed tries both, boots whichever verifies
```

---

## Reproducible Build Verification

```
Rust compiler is deterministic for a given:
  - Source tree hash
  - Compiler version
  - Target triple
  - Cargo.lock

Anyone can verify:
  1. Clone ferros repo at tagged commit
  2. Build with documented toolchain
  3. Compute BLAKE3 of output binary
  4. Compare with BLAKE3 in developer's signature
  5. Match → binary IS the source, provably
  6. Mismatch → binary was modified after build

This eliminates supply chain trust:
  You don't trust the developer
  You trust the source you audited
  The hash proves the binary matches that source
  The signature proves the developer built from that source
```

---

## What the Seed Does NOT Do

```
- No memory allocation (static buffers only)
- No USB (no host communication needed)
- No display beyond error messages (uses ABL's splash FB)
- No power management (sub-second runtime)
- No interrupt handling (linear execution)
- No SPMI/PMIC access (ABL configured everything)
- No SD card init (only UFS, already initialized by ABL)
- No capability system (that's the kernel's job)
- No ring memory (that's the kernel's job)
- No VSF parsing (reads fixed offsets for speed)
- No clock configuration (ABL set clocks)
- No DTB parsing (kernel does this)
```

The seed is NOT a bootloader. It is a signature verification stub that happens to load a binary. The kernel is the bootloader — it scans the vault root, loads state, and starts the system.

---

## Code Budget

```
Target: <512 lines of Rust (no_std, no_alloc)
Binary: <16KB (fits in one UFS allocation unit)

Dependencies:
  - BLAKE3 (no_std, ~3KB code)
  - Ed25519 (no_std, ~8KB code with curve25519-dalek)
  - UFS read (reuse ABL's controller state, ~200 lines)
  - Framebuffer write (error messages only, ~50 lines)
```

---

## Android Integration

```
ABL (Android Bootloader):
  Loads boot.img from boot_a or boot_b partition
  boot.img contains: seed (as the "kernel" payload)
  ABL verifies boot.img via dm-verity / vbmeta
  ABL displays orange "unlocked" nag screen (10s)
  ABL jumps to seed entry point

Seed:
  Runs in EL1 (same as normal kernel)
  ABL left UFS controller enabled (HCE=1, link up)
  ABL left DRAM configured, caches off
  ABL left splash framebuffer at G#E1000000

Future (locked bootloader):
  Sign seed + vbmeta with owner's key
  ABL verifies vbmeta signature → green "locked" boot
  No nag screen, no 10s delay
  Requires understanding ABL's vbmeta key enrollment
```

---