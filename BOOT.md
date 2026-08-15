# BOOT — ferros Kernel Boot Sequence
**Version:** Zila (1)
**Author:** Nick Spiker
**Principle:** Scan, verify, restore. Deterministic from any valid state.

---

## Overview

The kernel boot sequence is the second stage after the seed verifies and jumps.
It transforms raw hardware into a running system by scanning the vault root ring, restoring state, and launching userspace.
The boot sequence is deterministic: given the same vault root state, it produces the same running system.

Targets: **M1 MacBook Air** (active, boots via m1n1 proxy) and **Pixel 8 / Tensor G3** (primary, bring-up in progress).
The Fairphone 5 (QCM6490) is dropped; FP5-specific steps below are marked legacy and are not built.

---

## Boot Entry Paths (per target)

How control reaches `_entry` differs per target; everything after Stage 0 converges.

```
M1 (active):
  m1n1 (patched, ESP) → proxy mode → m1n1-boot.py uploads flat binary + minimal DTB
  → p.reload() → _m1_entry (skips teardown; m1n1 already shut down MMU/caches)

Pixel 8 / Tensor G3 (primary):
  ABL (LK-based, Google-signed, immovable) → reads boot_a/boot_b slot
  → boot.img v4 (header_size=1584, header_version=4)
  → kernel payload must be LZ4-compressed (legacy frame, magic G#02214C18)
  → ABL decompresses, jumps to ARM64 Image header entry
  → _entry (MMU-state-checked teardown; see entry findings below)
```

### Tensor G3 ABL facts (measured 2026-03-31, ABL build ripcurrent-16.4-14540574)

- **`fastboot boot` DOES NOT WORK** — ABL accepts the image ("Booting OKAY") but never jumps.
  There is no RAM-boot dev loop on this device; the dev loop is flash-to-slot.
- `fastboot oem pkvm disable` works → kernel entered at EL2.
- `fastboot oem dmesg` dumps the full ABL log — primary bring-up diagnostic.
- Kernel must be LZ4-compressed (legacy format, magic G#02214C18); ABL decompresses then jumps.
- Boot image header v4 required (header_size=1584, header_version=4).
- **Small kernels fail silently** — a 12KB image decompressed but never ran; code injected into the 37MB GrapheneOS kernel executed.
  ABL likely validates the ARM64 Image header image_size or enforces a minimum. Threshold unknown — bisect experiment pending.
- Hardware watchdog (Exynos CLUSTER0_NONCPU WDT) fires at ~60 seconds if the kernel doesn't pet it — confirmed 2026-08-14: reboot reason G#CBEA "APC Watchdog Early", RST_STAT G#1.
- On watchdog reset ABL silently retries the active slot (decrementing its retry counter) — the visible symptom is a frozen Google splash cycling every ~70s until retries exhaust and it falls back to the other slot.
- AVB verification of an unsigned image logs `avb_ret=ERROR_VERIFICATION, avb_error_parts=boot` and **boots it anyway** while the bootloader is unlocked. Signing (avb_custom_key) is only needed for the locked endgame.
- A/B rollback observed working: a failing slot is marked and ABL falls back to the other slot or fastboot.

### Entry teardown hazard (the 2026-03 crash, root-caused)

The original `_entry` cache-clean + SCTLR MMU/cache-disable sequence **faults on Tensor G3** (crash ~2s), same class of failure as the M1.
Current `_entry` checks the MMU bit first and skips the teardown entirely when the MMU is already off; `_m1_entry` skips unconditionally.
**Hardware-validated 2026-08-14**: first husky boot ran from ABL jump to the ~60s watchdog with no fault — the guard works.
Full first-boot ABL log archived locally (gitignored — device identifiers) at tools/pixel8/abl_dmesg_first_ferros_boot_2026-08-14.txt.

---

## Boot Stages

```
Seed verifies kernel, jumps to entry point
  ↓
Stage 0: Hardware Init (EL2/EL1, no allocator)
  - Configure exception vectors
  - Enable I-cache, D-cache, MMU (identity-mapped)
  - Parse DTB for memory layout, reserved regions
  - Initialize bump allocator from available DRAM
  - Initialize pstore/ramoops for crash logging
  - Initialize framebuffer console
    (M1: simplefb from boot_args, 30bpp; Pixel 8: none at bring-up — DECON sits behind
     a Samsung SYSMMU, framebuffer has no fixed physical address; USB is first output)
  - Log: "ferros v{version}"
  ↓
Stage 1: Storage Init (per target)
  - Pixel 8: UFS — verify HCE=1, HCS link up (ABL left it running), set up UTRD list, NOP OUT ping
  - M1: NVMe via ANS2 (future — bring-up uses USB only, no persistent storage yet)
  - [FP5 legacy, dropped: SDC2 clocks, RPMh LDO, SDHCI/SD probe]
  ↓
Stage 2: Spine Scan
  - Read spine (vault root ring) from primary storage
  - Binary search for highest valid generation (BLAKE3 verified)
  - If primary spine corrupt → fall back to mirror medium (see Mirror Media below)
  - If all copies corrupt → genesis boot (first boot ever)
  - Extract: HAMT root (hash, lba), plow position, ledger head
  ↓
Stage 3: PAC Key Init
  - Generate per-boot session key via TRNG (ARM RNDR instruction)
  - Store in PAC key registers (APIAKey_EL1, etc.)
  - These keys never touch RAM — hardware registers only
  - Used for: runtime encryption, capability token derivation
  ↓
Stage 4: GIC + Interrupt Init
  - Configure interrupt controller (Pixel 8: GIC; M1: AIC — future, polling today)
  - Route USB DWC3 interrupt
  - Route timer interrupt (for scheduling)
  - Enable WFI (Wait For Interrupt) in idle path
  - CPU power draw drops dramatically
  ↓
Stage 5: USB Init
  - DWC3 device mode init
    (M1: DART IOMMU + full DWC3 bring-up, code complete;
     Pixel 8: DWC3 at G#11210000 is clocked, but ABL shuts down the eUSB PHY at G#11100000
     before jumping — Samsung eUSB PHY re-init required, sequence from AOSP
     drivers/phy/samsung/phy-exynos-usbdrd.c)
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

## Mirror Media (per target)

The spine/vault mirror protocol (see VAULT.md, manifestus) needs two verified copies.
What the second medium is depends on the hardware:

```
FP5 (dropped):  UFS primary + SD card mirror        [the original design; SD code is legacy]
Pixel 8:        UFS only — no SD slot.               Mirror = second copy on UFS (placement-diverse)
                + generation ring depth + off-device (custodian Shamir / NFC anchor, future)
M1:             ferros partition (disk0s9) only.     Same single-device reality as Pixel.
```

On single-device targets the two on-device copies do not protect against whole-device loss; that tier is the off-device story (TOKEN custodian shards), not the boot path's job.

---

## Dual Boot (Pixel 8): mirrored ferros slots + Android chainload

Decision (2026-08-14): **both boot slots are ferros. Android is a payload, not a peer.**
`boot_a` = `boot_b` = identical ferros seed+kernel image.
A=Android/B=ferros is rejected — it spends the slot mirror on the wrong thing (one corrupt ferros slot would "fall back" to Android, i.e. ferros redundancy zero).

```
ABL (Google's, immovable)
  → boot_a / boot_b: ferros seed — verify ring, jump    [identical, mirrored]
    → ferros kernel — always boots, <1s
      → [key held?]  chainload staged Android payload
      → [default]    ferros proper
```

### Slot mirroring

- Identical seed+kernel image flashed to both slots.
- ABL's A/B rollback (observed working) = hardware-enforced fallback between our two copies — write-verify-then-mirror implemented by the bootloader itself.
- Slot updates follow the vault protocol literally: write slot B, verify, only then slot A. Never both dirty at once.

### Android as managed guest

- Graphene/Android is **left alone** — its boot images are not modified, they are *relocated*: `boot.img`/`vendor_boot` contents live as staged payloads in the ferros partition (vault objects, BLAKE3-verified like everything else).
- **Seed stays dumb.** Seed never parses Android images — its job remains verify-and-jump, nothing else grows the trust anchor.
- **The kernel does the chainload**, as a capability sibling to RELOAD: load staged kernel Image + DTB + ramdisks, tear down to Linux arm64 boot protocol state (MMU off, caches clean, x0 = dtb, jump — EL2 entry is fine), go.
- **ABL impersonation is the real work item:** Android init expects androidboot.* bootconfig (slot suffix, serial, hardware rev, verifiedbootstate…) that ABL normally synthesizes. ferros must reproduce these or Android userspace misbehaves in non-obvious ways.
- **OTA caveat (stated plainly):** chainloaded Android cannot self-update its kernel — its OTA wants the boot slots, which are ours. Kernel/vendor updates get re-staged through ferros (future: ferros-bridge stage-android <boot.img>); system-partition OTA still works normally.

### Boot selection

- Default path is **always ferros** — silent boot never lands in Android.
- Android boots only on explicit physical input: a volume key held through early boot (GPIO read at EL2, small driver).
- Future: NFID/sticker-gated — Android as a summoned domain, invisible unless its token is present.

### Locked-bootloader endgame (much later)

- Pixel supports one `avb_custom_key` — a re-locked dual boot requires ferros seed and the Android build signed by the **same** key.
  Stock GrapheneOS (their key) cannot share a locked device with ferros (our key); a self-signed Graphene build can.
- Dev stays unlocked (orange state). **Never lock until the signed chain is proven** — and macOS/stock recovery escape hatches stay per the M1 rules.

---

## Genesis Boot (First Boot Ever)

```
No spine exists on any medium:
  1. Format spine on primary storage (write ring header + generation 0)
  2. Write second copy per Mirror Media table
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

Hot reload (ferros-bridge reload — works on any target with live USB):
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

Pixel 8 note: hot reload is the ONLY fast dev loop on Tensor — fastboot boot is broken
(see ABL facts), so the cold path is always flash-to-slot + reboot. Getting USB up
is therefore the gating milestone for iteration speed.
```

---

## Timing Budget

```
Stage 0: Hardware Init      <50ms    (cache, MMU, DTB parse)
Stage 1: Storage Init       <200ms   (UFS ping; was SD probe on FP5)
Stage 2: Vault Root Scan    <100ms   (ring scan, BLAKE3 verify)
Stage 3: PAC Key Init       <1ms     (TRNG + register write)
Stage 4: GIC + Interrupts   <10ms    (register config)
Stage 5: USB Init           <100ms   (PHY + enumeration)
Stage 6: Capability Restore <50ms    (deserialize + verify)
Stage 7: Process Restore    <100ms   (depends on snapshot size)
Stage 8: Userspace Launch   <100ms   (server spawning)

Total target: <500ms from seed jump to userspace running
              <1s including ABL + seed
              (excludes 10s unlocked-bootloader nag — eliminated when locked with our key)

Watchdog: Tensor G3 hardware watchdog fires at ~60s — kernel must pet it before then.
```

---

## Failure Recovery

```
Spine corrupt on primary:
  → Fall back to mirror copy (per Mirror Media table)
  → If mirror valid, restore from it, repair primary
  → If all copies corrupt, genesis boot

Spine generation mismatch between copies:
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

Boot slot corrupt (Pixel 8):
  → ABL marks slot failed, falls back to the other (identical ferros) slot
  → Recovered slot re-flashed from the good one at next ferros boot (future)
```

---

## Open Items (Pixel 8 bring-up)

First boot achieved 2026-08-14 (husky, slot a, `--pad 16`): image accepted, kernel ran ~60s to watchdog, no entry fault.
Watchdog defeated same day — kernel now runs indefinitely (survived 5+ min, no reset).
Validated and closed: mkimg LZ4, pad/image_size, entry guard, cluster watchdog disable.

1. ~~**USB enumeration**~~ — **DONE 2026-08-14: ferros enumerates as `1209:4665` on the host.** The bug was the inline eUSB2 PHY init: right register *offsets* but wrong *bits* (`rptr_mode` b1 not b10, `pll_fb_div` [11:0] not [19:8], `pll_ref_div` [3:0] not [11:8]) and — the killer — it never cleared `TESTSE.test_iddq` (b6), leaving the PHY **analog powered down** so nothing reached the wire. Fixed by a faithful port of `phy_exynos_eusb_initiate` (source pulled from Google `gs-google` kernel into gitignored [tools/pixel8/phy-ref/](tools/pixel8/phy-ref/)): correct bits + `test_iddq→0` + real timing (10us / 10us / 1000us / 28us / 2500us). ABL leaves the eUSB repeater (I2C) + PMU isolation + PHY clocks up across the jump (they persist); only the analog IDDQ/enable needed re-establishing. Earlier "Qualcomm PHY" and "trust ABL" theories were both wrong/ruled-out — the active path never ran QCOM code, and warm-init-only stayed silent 91s.
   - Host side DONE: udev rule (`/etc/udev/rules.d/70-ferros.rules`, MODE 0666) added; `ferros-bridge status` connects (`VID=1209 PID=4665 EP_OUT=G#01 EP_IN=G#81`). EP0 control transfers fully work (that's what enumeration + status exercise).

2. **Device-side PT dispatch** (the new wall, and it's pure software): the pixel8 `kernel_main` USB loop only handles `Reset`/`ConnectDone`/`Ep0Setup` — it arms bulk OUT once and never reads it or replies. So `ferros-bridge diag` hangs: the host sends a PT SPEC on bulk EP2 and waits for an ACK the device never sends. The M1/FP5 paths have the full dispatch (read bulk OUT → decode PT command → DIAG/reload/etc → reply on bulk IN); it needs wiring into the pixel8 loop. This is protocol software with the bridge as a live harness — no more hardware archaeology. Once done: PT diag + `ferros-bridge reload` hot-reload loop.
2. **androidboot.* synthesis**: enumerate exactly what Graphene init requires for the chainload path.
3. **AVB footer** (locked endgame only): unsigned images boot with a logged ERROR_VERIFICATION while unlocked; the avb_custom_key signing path picks this up in Phase 5.

**Watchdog disable** (done): Exynos cluster watchdogs (`watchdog_cl0@G#10060000`, `watchdog_cl1@G#10070000`, Samsung s3c2410-style) are armed by ABL. Writing 0 to WTCON (base+0) clears the enable bit (halts the counter) and the reset-enable bit — done first thing in kernel_main. No PMU write needed; stopping the counter stops the reset request.

---

## Dependencies on Other Specs

```
SEED.md:        Seed verifies kernel, jumps to Stage 0
RING.md:        Ring format, generation numbering, spine/stem entries
VAULT.md:       Persistent object store: tract, plow, HAMT, spine (manifestus engine)
LEDGER.md:      Pre-boot buffer format, ledger server bootstrap
KERNEL.md:      Running kernel responsibilities (post-boot)
SECURITY_CHAIN.md: Trust model, key management, signature verification
```

---
