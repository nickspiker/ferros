# BOOT — ferros Kernel Boot Sequence
**Version:** Zil (0)
**Author:** Nick Spiker
**Principle:** Scan, verify, restore. Deterministic from any valid state.

---

## Overview

The kernel boot sequence is the second stage after the seed verifies and jumps. It transforms raw hardware into a running system by scanning the vault root ring, restoring state, and launching userspace. The boot sequence is deterministic: given the same vault root state, it produces the same running system.

---

## Boot Stages

```
Seed verifies kernel, jumps to entry point
  ↓
Stage 0: Hardware Init (EL1, no allocator)
  - Configure exception vectors
  - Enable I-cache, D-cache, MMU (identity-mapped)
  - Parse DTB for memory layout, reserved regions
  - Initialize bump allocator from available DRAM
  - Initialize pstore/ramoops for crash logging
  - Initialize framebuffer console (splash FB at G#E1000000)
  - Log: "ferros v{version}"
  ↓
Stage 1: Storage Init
  - GCC clock enable for SDC2 (SD card)
  - RPMh LDO enable for SD card power
  - SDHCI init, SD card probe (CMD0→CMD3→CMD7)
  - UFS: verify HCE=1, HCS link up (ABL left it running)
  - UFS: set up UTRD list, send NOP OUT ping
  - Both storage backends online
  ↓
Stage 2: Spine Scan
  - Read spine (vault root ring) from UFS (primary)
  - Binary search for highest valid generation (BLAKE3 verified)
  - If UFS spine corrupt → fall back to SD mirror
  - If both corrupt → genesis boot (first boot ever)
  - Extract: HAMT root (hash, lba), plow position, ledger head
  ↓
Stage 3: PAC Key Init
  - Generate per-boot session key via TRNG (ARM RNDR instruction)
  - Store in PAC key registers (APIAKey_EL1, etc.)
  - These keys never touch RAM — hardware registers only
  - Used for: runtime encryption, capability token derivation
  ↓
Stage 4: GIC + Interrupt Init
  - Configure ARM GIC (Generic Interrupt Controller)
  - Route USB DWC3 interrupt
  - Route timer interrupt (for scheduling)
  - Enable WFI (Wait For Interrupt) in idle path
  - CPU power draw drops dramatically
  ↓
Stage 5: USB Init
  - DWC3 device mode init (reuse ABL's PHY config)
  - Bulk endpoints for Photon Transport
  - Device enumerates as "ferros" (VID G#1209, PID G#4665)
  ↓
Stage 6: Capability Restore
  - Deserialize capability table from vault root snapshot
  - Rebuild kernel CSpace (capability space)
  - Verify capability chain integrity (BLAKE3 provenance)
  - Issue initial caps to kernel subsystems
  ↓
Stage 7: Process Restore (future)
  - Read HAMT for process snapshots
  - Allocate rings for each process
  - Restore register state, memory contents
  - Resume scheduling
  ↓
Stage 8: Userspace Launch
  - Start ledger server (receives pre-boot log buffer)
  - Start display compositor (priority: get pixels on screen)
  - Start USB/PT bridge server
  - Start remaining services
  - Kernel enters idle loop (WFI + interrupt-driven scheduling)
```

---

## Genesis Boot (First Boot Ever)

```
No spine exists on either medium:
  1. Format spine on UFS (write ring header + generation 0)
  2. Mirror to SD card
  3. Generate root capability (BLAKE3 of TRNG seed)
  4. Write genesis spine entry:
     - generation: 0
     - HAMT root: nil
     - plow: G#C0000 (tract start)
     - prev_hash: hp(BLAKE3, [0; 32])
  5. Derive initial kernel caps from root cap
  6. Continue to Stage 5 (USB init)
  7. System is bootable, state will accumulate
```

---

## Pre-Boot Log Buffer

```
Before the ledger server starts, the kernel logs to a fixed ring buffer:
  - Allocated in BSS (no heap needed)
  - Raw VSF documents, unchained (no prev_hash)
  - No capability validation (server not running)
  - Overwrites oldest entries on wrap

When the ledger server starts (Stage 8):
  - Receives pre-boot buffer contents via IPC
  - Establishes genesis ledger entry
  - Chains pre-boot entries as entries 0..N
  - Pre-boot buffer released
```

---

## Hot Reload vs Cold Boot

```
Cold boot (power cycle):
  Seed runs → kernel loads fresh → full Stage 0-8

Hot reload (ferros-bridge reload):
  Kernel receives new binary over USB PT
  Copies to staging DRAM
  Jumps to new kernel entry point
  Stage 0-1 re-run (hardware re-init)
  Stage 2 skipped (spine not needed, state was live)
  Stage 3-8 re-run

  Key difference: hot reload does NOT scan vault root
  The previous kernel's state was live in memory
  New kernel re-initializes hardware but inherits DRAM contents
  This is a development convenience, not a production path
```

---

## Timing Budget

```
Stage 0: Hardware Init      <50ms    (cache, MMU, DTB parse)
Stage 1: Storage Init       <200ms   (SD probe, UFS ping)
Stage 2: Vault Root Scan    <100ms   (ring scan, BLAKE3 verify)
Stage 3: PAC Key Init       <1ms     (TRNG + register write)
Stage 4: GIC + Interrupts   <10ms    (register config)
Stage 5: USB Init           <100ms   (PHY + enumeration)
Stage 6: Capability Restore <50ms    (deserialize + verify)
Stage 7: Process Restore    <100ms   (depends on snapshot size)
Stage 8: Userspace Launch   <100ms   (server spawning)

Total target: <500ms from seed jump to userspace running
              <1s including ABL + seed
              (excludes 10s nag screen — eliminated when locked)
```

---

## Failure Recovery

```
Spine corrupt on UFS:
  → Fall back to SD mirror
  → If SD valid, restore from SD, repair UFS spine
  → If SD also corrupt, genesis boot

Spine generation mismatch (UFS ≠ SD):
  → Use higher generation (more recent)
  → Repair the stale copy from the fresh one

BLAKE3 chain break in ring:
  → Scan backwards from newest entry
  → Find last entry where chain is valid
  → Restore from that point (lose entries after break)
  → Log the corruption event

Capability table corrupt:
  → Fall back to previous generation's cap table
  → If no valid generation, genesis boot with root cap only

Process snapshot corrupt:
  → Skip that process, log error
  → Other processes restore normally
  → Corrupt process must be restarted manually
```

---

## Dependencies on Other Specs

```
SEED.md:        Seed verifies kernel, jumps to Stage 0
RING.md:        Ring format, generation numbering, spine/stem entries
VAULT.md:       Persistent object store: tract, plow, HAMT, spine
LEDGER.md:      Pre-boot buffer format, ledger server bootstrap
KERNEL.md:      Running kernel responsibilities (post-boot)
SECURITY_CHAIN.md: Trust model, key management, signature verification
```

---