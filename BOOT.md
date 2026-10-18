# BOOT — ferros Kernel Boot Sequence
**Version:** Zila (1)
**Author:** Nick Spiker
**Principle:** Scan, verify, restore. Deterministic from any valid state.

---

## Overview

The kernel boot sequence is the second stage after the seed verifies and jumps.
It transforms raw hardware into a running system by scanning the vault root ring, restoring state, and launching the first process.
On husky it is split by the "Ready?" gate below: plumbing before the owner's press, the boot proper after it.
The boot sequence is deterministic: given the same vault root state, it produces the same running system.

Targets: **M1 MacBook Air** (active, boots via m1n1 proxy) and **Pixel 8 / Tensor G3** (primary, bring-up in progress).
The Fairphone 5 (QCM6490) is dropped; FP5-specific steps below are marked legacy and are not built.

---

## The "Ready?" Gate (decided 2026-10-14)

Everything before the gate is vendor plumbing we tolerate; ferros's own boot starts when a human presses the power button.

```
power on → BootROM → … → ABL (unlocked-bootloader nag) → ferros plumbing → "Ready?"
                                                                       │
                                            power key PRESSED (not released)
                                                                       ▼
                                                       boot, officially: the <1 s budget starts here
```

### The screen
- Black, a green triangle on the right edge pointing at the power button, and white "Ready?" in the top third, thin (Oxanium ExtraLight).
- The text is rasterised at build time on the host into an alpha bitmap; the kernel only blits it (no font engine in the kernel). The triangle's height is calibrated by eye against the physical button.
- The press is an interrupt, like the killswitch: the power key is gpa4-0 on the always-on bank (SPI 28, INTID 60), armed on its falling edge (the key is active low); the release is ignored.

### Before the press: only what it takes to ask
1. Integrity of the code about to draw the screen (see Boot integrity below; on husky that is ABL's job).
2. The killswitch (armed first, as always).
3. The static cache map (KERNEL.md, Execution Model: identity, written once).
4. The panel and the "Ready?" screen.
5. The buttons, with the power key armed.

Nothing that talks to the outside world or holds the self runs before the owner says go.
The USB controller is held halted and its PHY in reset, so nothing ABL left behind can answer either.

### After the press: the boot
UFS power-on and link, the vault and the *ira*, slot marking, USB data, radios, services, the first process.
The disk and the identity are never touched without a human present.

### What is not affected
- **Charging.** Power is not data: the charger and USB-C controller chips negotiate and charge on their own, without the main CPU, so a phone at "Ready?" charges normally. Only the AP's USB data path stays dark.
- **The killswitch.** Vol Up + Vol Down works before, during and after the gate.

### The gate's contract (owner, 2026-10-17)

Dev builds never default to Android: the one-second auto-press goes to ferros, and Android takes an explicit volume-down during the second (or volume-up to hold, then down). Release builds do not auto-press; they stop on Ready? and wait for the power key (ferros) or volume down (Android). ferros_kernel/src/ready.rs is this today. Slot-a hypervisor tests bypass the gate by making slot a active, which is the owner-approved override for those tests only.

**Planned: a USB handshake at the gate.** The host should be able to choose ferros or Android from the Ready? screen (a PT command while the gate waits), for the dev loop and for recovery without touching the phone. The gate currently stops the USB controller before the press on purpose ("nothing answers on USB before the press"); the handshake means bringing DWC3 and a minimal PT endpoint up while the screen waits, so it costs the gate its "only what it takes to ask" economy on release builds only when the cable is present.

### Dev builds: auto-press
Dev builds show "Ready?" and proceed as if the key were pressed after one second; production builds compile that out and wait for the key.
In a dev build, a volume key (up or down) during that second cancels the timer: the gate then waits for the power key indefinitely, as production does (owner hardware test 2026-10-15: PASS).
Every line after the gate is identical in both, and nothing new listens on any interface: flashing a dev build is itself the developer's consent.
There is deliberately no remote "press" (no USB bypass): USB data does not exist before the press.

### Hot reload skips the gate
A RELOAD swaps code on a device that has already booted; it is not a boot.

### Boot integrity: no runtime hashing by ferros, ever
- **Data at rest is not the reason to check.** The data is bound to PIPE: separate or damage PIPE and it is gone, whatever code boots.
- **Corruption is one reason:** a flipped bit in code that writes the vault could damage data quietly.
- **The evil maid is the other:** with the bootloader unlocked, anyone holding the phone for a minute can flash a modified ferros. It draws a perfect "Ready?", the owner presses and unlocks, and from then on it has full use of PIPE — PIPE answers whatever host asks and cannot see which code is asking. The check protects **who gets to ask PIPE**, not the data.
- **ferros cannot check itself**: a replaced ferros simply skips the check. The anchor must be something the attacker cannot reflash. On husky that is Google's chain: relock the bootloader with **the owner's own key** (`avb_custom_key`, see Locked-bootloader endgame). ABL then verifies our image on every boot, in the vendor phase before "Ready?", so it costs nothing in our budget; it catches corruption for free, and flashing anything else requires an unlock, which wipes the device.
- **The first lock is trust-on-first-use**, the same bootstrap problem as HTTPS: whoever locks first picks the key. The backstop is the human: ABL's warning screen shows the ID of the key it verified on every boot, and the owner compares it with what they recorded when *they* generated the key. Like SSH and better than HTTPS, there is no CA to compromise: the owner is the CA. Each owner generates their own key; there is no single ferros-project key.
- **On Glyph silicon** the bootstrap becomes PIPE's genesis ceremony (*whakaira*): the trust anchor is born in the owner's hands instead of inherited from a vendor's lock. The first-trust moment cannot be removed, only made a ceremony the owner performs.
- **Not now:** we stay unlocked while we are flashing constantly. Until we relock, an evil maid with a minute and a cable wins, PIPE or not.

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

### Current husky layout (2026-10-14): slot b boots ferros, slot a is Android

- **Slot b = ferros, cold.** `tools/husky-cold.sh` builds the cold image (`ferros-mkimg boot --pad 16`), flashes `boot_b`, runs `--set-active=b`, and reboots. ABL loads the kernel at G#8008_0000, the cold link base (`ferros_kernel/aarch64-pixel8.ld`; m1 keeps `aarch64.ld`). The image is statically linked, so it must run where it is linked: DIAG reports `BASE` and `RUNNING_AT`.
  - Slot b's other partitions are stock, and `product_b` is empty; ABL doesn't care.
  - Measured first boot: EL2, USB enumerated, `EXC=0`, hot RELOAD to GEN=1/2, RUN payloads, and text drawn on ABL's splash surface.
- **Slot a = Android + the handoff kernel.** It is the escape hatch, and the handoff is kept **only as a register oracle and guide, not as a boot option** (decided 2026-10-14): when bringing up new hardware (WiFi/PCIe, the modem) we boot Linux, let it set the hardware up, hand off with `tools/husky-hotload.sh` and dump/diff what it left. ferros itself runs from the cold boot on slot b; new features are not ported to or tested on the handoff path. Delete it once the oracle has served (after WiFi bring-up).
- **Back to Android:** `tools/husky-cold.sh --android` (fastboot, `--set-active=a`, reboot, about 40 s, no buttons), or from ferros itself: `bridge diag android`, or volume-down at the Ready? gate (ready.rs). Both call `slot::set_active(0)` and reset.
- **How the bootloader really picks the slot (measured 2026-10-17 from its own log, `bl_log` at G#FD80_0000, 512 KiB, `bl_old_log` at G#FD88_0000 for the boot before):** the ROM boots the bootloader chain from the UFS Boot LU named by the device attribute `bBootLunEn` (1 = Boot LU A = LU1, 2 = Boot LU B = LU2), and ABL takes the Android slot of the same letter *before* it reads devinfo: `ufshc: bBootLunEn 0x2` → `[RIPCURRENT] boot slot: b` → `[A/B] slot data a/b` → `[PXL] AB Decision: active slot boot ok` (or `decrement active slot boot retry`). When devinfo disagrees, ABL re-syncs it to the boot LU's slot: three devinfo-only writes of "a active" (including one byte-identical to what `fastboot --set-active=a` leaves) came back with a's active bit cleared and b's set, a never tried. `set_active` now writes `bBootLunEn` as well (WRITE ATTRIBUTE, idn 0) and reads it back; the first hand-off with it booted Android in 36 s. GPT entry attributes carry no slot state here (only bit 49 on persist/efs/efs_backup). The Pixel kernel exposes the same attribute to root as `/sys/bus/platform/devices/13200000.ufs/pixel/boot_lun_enabled` (ufs-pixel.c, `PIXEL_ATTRIBUTE_RW`), which is how Android's own slot switch works and how `tools/husky-to-ferros.sh` comes back from Android without fastboot.
- **What DRAM keeps across a reset (measured 2026-10-17, markers written from ferros, read back after a PSCI reset and after the fastboot round trip):** nothing in the high bank (G#8_8000_0000 and up) and nothing in the low bank outside the bootloader's preserved debug regions; markers at G#A000_0000, G#B610_0000, G#C800_0000, G#F000_0000, G#FD90_0000 and the whole high bank all came back as either decayed cells (FF with scattered zero bits, the same pattern every time) or bootloader-written data. What does survive: the pstore ramoops console zone (G#FD3F_F000, 2 MiB, header "DBGC" + start + size) — which ABL also appends its own boot log to and re-initialises when it finds it invalid — and the debug-snapshot header (G#D810_0000, 64 KiB), which the bootloader rewrites. ferros's result block at G#FD60_0000 is inside ramoops's *pmsg* zone: it survives ferros's own resets because nothing else writes there until Android's logd does. Nothing written by a hypervisor or kernel outside those regions can be read after the next reset; HV.md's diagnostics went to the glass for that reason.
- **devinfo flag bit 3 is `fastboot_ok`** (`fastboot getvar all`: `slot-fastboot-ok:a:no`, `b:yes`, matching the words): ABL sets it on a slot it has booted; `--set-active` clears it on the new slot. Not a boot gate.
- **Exits from cold ferros:**
  - A plain reset just boots ferros again (slot b is marked successful), so every exit except a plain host `bridge reboot` goes to fastboot, where the host decides what boots next: idle 512 s, a 64 s enumeration timeout, a 16 s PT stall, or `bridge reboot fastboot`. Both volume keys are the killswitch (KILLSWITCH.md): power off, no exit path, no report.
  - The report block at G#FD60_0000 survives warm resets and the trip through fastboot. The next cold boot shows it as `PREV_*` in DIAG, and Android's handoff kernel prints it as `ferros: r[..]`.
- **Fastboot from ferros:** stamp PMU SYSIP_DAT0 = G#FC through the EL3 monitor, then fire the cluster watchdogs (count = reload = 1).
  - **The reset is not immediate:** these watchdogs reset the SoC about 15-20 s after they start expiring, so ferros waits up to 64 s by the clock.
  - A PSCI SYSTEM_RESET from ferros loses the reboot mode (ABL sees a PIN reset).
- **Slot b marks itself successful** (`ferros_kernel/src/slot.rs`, since 2026-10-14), so ABL stops counting down its retries and a power cycle lands in ferros again. This is what Android's boot-control HAL does once boot completes.
  - The record is `devinfo`: UFS LUN 3 (`sdd1`), 4 KiB LBA 6. It holds magic "DEVI", u16 major = 3, then at G#30 one word per slot (a, b): byte 0 = retries left, byte 1 = flags (b0 unbootable, b1 successful, b2 active, b3 fastboot-ok). Measured on hardware: `--set-active` sets retries to 3, each ABL boot of an unmarked slot takes one, and a marked slot keeps its count. Nothing else in the partition changes (no checksum, no mirror).
  - **The rest of devinfo is factory identity data:** serials, WiFi/BT MACs, both IMEIs. ferros rewrites the block byte-identical apart from the one flag bit and verifies it by read-back. `bridge diag devinfo` returns only the 128-byte header plus a BLAKE3 fingerprint of the rest. A pre-experiment backup of the whole 8 KiB partition is in `keys/husky-devinfo-2026-10-14.bin` (sha256 c0735a11…; rest-of-partition BLAKE3 starts 7034e633).
  - The booted slot must agree in two sources: the last `androidboot.slot_suffix` in ABL's bootconfig (end of the initrd; it carries `_a` and then the real one) and devinfo's active bit. Marking happens only on a real ABL boot with the vault verified, so a build that dies earlier still falls back. `--set-active=b` clears the mark; the next ferros boot sets it again. DIAG: `SLOT`, `SLOT_WORD_BEFORE/AFTER`, `SLOT_MARK` (0 marked now, 1 already marked, G#1x/2x/3x refusals, see slot.rs).

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
- Why lock at all, and why with the owner's own key: see Boot integrity under the "Ready?" gate (evil maid, trust-on-first-use, the on-screen key ID).

---

### Can Android be kept from writing the boot chain? Measured 2026-10-17

The question behind dual boot: once both slots are signed with the owner's key and the bootloader locked, Android (its OTA engine as a matter of routine, or anything with root) can still *write* the inactive slot and `devinfo` — the lock only governs what ABL will boot. On a locked bootloader that turns "re-flash ferros" into "unlock and wipe". The hardware answer would be UFS power-on write protection: a per-LU provisioning type, armed by one device flag (`fPowerOnWPEn`) that only a reset clears. ferros read the flash's own descriptors (`bridge diag ufsdesc`):

```
DEVICE_DESC len 89 ocs 0: numLU 1 bootEnable 0 descrAccessEn 4 features G#B300
bConfigDescrLock 0 bBootLunEn 2 bCurrentPowerMode 17 fPermanentWPEn 0 fPowerOnWPEn 0 fDeviceInit 0
CONFIG_DESC len 230 ocs 0: bootEnable 1 descrAccessEn 1 initPowerMode 1 highPriorityLUN 127
LU0: enable 1 bootLunId 0 writeProtect none memType 0 blockSize 4096 blocks 62436352 = 243892 MiB
    GPT: persist(131072K) klog(16384K) misc(1024K) frp(512K) efs(65536K) efs_backup(65536K) modem_userdata(65536K) trusty_persist(65536K) trusty_userdata(131072K) metadata(65536K) fips(2048K) ufs_internal(4096K) boot_a(65536K) init_boot_a(8192K) vendor_boot_a(65536K) vendor_kernel_boot_a(65536K) dtbo_a(16384K) vbmeta_a(64K) vbmeta_system_a(64K) vbmeta_vendor_a(64K) pvmfw_a(1024K) modem_a(204800K) boot_b(65536K) init_boot_b(8192K) vendor_boot_b(65536K) vendor_kernel_boot_b(65536K) dtbo_b(16384K) vbmeta_b(64K) vbmeta_system_b(64K) vbmeta_vendor_b(64K) pvmfw_b(1024K) modem_b(204800K) super(8331264K) userdata(105728104K) ferros(134217708K)
LU1: enable 1 bootLunId 1 writeProtect POWER-ON memType 3 blockSize 4096 blocks 8192 = 32 MiB
    GPT: bl1_a(16K) pbl_a(80K) bl2_a(540K) dram_train_a(100K) abl_a(4096K) bl31_a(256K) tzsw_a(20480K) gsa_a(1024K) gsa_bl1_a(96K) ldfw_a(4096K) dpm_a(36K) gcf_a(352K)
LU2: enable 1 bootLunId 2 writeProtect POWER-ON memType 3 blockSize 4096 blocks 8192 = 32 MiB
    GPT: bl1_b(16K) pbl_b(80K) bl2_b(540K) dram_train_b(100K) abl_b(4096K) bl31_b(256K) tzsw_b(20480K) gsa_b(1024K) gsa_bl1_b(96K) ldfw_b(4096K) dpm_b(36K) gcf_b(352K)
LU3: enable 1 bootLunId 0 writeProtect none memType 0 blockSize 4096 blocks 1024 = 4 MiB
    GPT: devinfo(8K) pinfo(8K) blenv(8K) mfg_data(3280K)
LU4: enable 0 bootLunId 0 writeProtect none memType 0 blockSize 1 blocks 0 = 0 MiB
LU5: enable 0 bootLunId 0 writeProtect none memType 0 blockSize 1 blocks 0 = 0 MiB
LU6: enable 0 bootLunId 0 writeProtect none memType 0 blockSize 1 blocks 0 = 0 MiB
LU7: enable 0 bootLunId 0 writeProtect none memType 0 blockSize 1 blocks 0 = 0 MiB
```

- **LU1 and LU2 (the bootloader's two 32 MiB boot LUs) are the only power-on-protectable LUs.** Google provisioned the protection for their chain. Ours — `boot_b`, `vbmeta_b`, `dtbo_b`, the `ferros` partition — sits on LU0 beside `userdata`, and `devinfo` on LU3 with no protection type; protecting either LU would freeze Android's own data. **No hardware barrier for our slot on this phone.**
- **`bConfigDescrLock = 0`**: the LU layout is *not* locked, so the flash would accept a re-provisioning (a new power-on-protected LU for our chain) — but writing the Configuration Descriptor erases every LU, the bootloader's two included, and with ABL gone there is no fastboot and no public ROM recovery on Tensor. That is a brick, not a factory reset. Not a path.
- **`fPowerOnWPEn` is not a barrier against Android either (2026-10-17).** ferros arms it at every cold boot (main.rs, `UFS_POWER_ON_WP` in DIAG), and the flag was clear again after a hot reload whose UFS bring-up resets the device: JEDEC clears it on a hardware reset, not only a power cycle, and the Exynos driver Android runs has a `device_reset` hook (ufs-exynos.c `exynos_ufs_dev_hw_reset`) that fires on probe. So Android's kernel comes up with the bootloader LUs writable whatever ferros did before the hand-off. Arming it still costs nothing and covers the window up to that reset.
- **Using Android for real, today (no hardware sandbox exists on this phone):** the one routine writer of slot b is the OTA engine (GMS SystemUpdate + `update_engine` stream the update straight into the inactive slot, then switch the Boot LU). Before any real use: Developer options → Automatic system updates off, and with root disable the updater (`pm disable-user --user 0 com.google.android.gms/com.google.android.gms.update.SystemUpdateService`, `settings put global ota_disable_automatic_update 1`). Nothing in Android touches the `ferros` partition or LU3 except the boot-control HAL's own-slot mark. If slot b is ever overwritten, `tools/husky-cold.sh` puts ferros back in minutes; the vault on the `ferros` partition is untouched by an OTA. The hypervisor oracle (later) is the real answer: a UFS command firewall under Android.
- **`fPowerOnWPEn = 0` under ferros**: ABL does not arm the protection before jumping to us; even LU1/LU2 are writable from this kernel. Whether ABL arms it when booting Android: to read from Android's sysfs. ferros should arm it at boot regardless (one SET FLAG; the next reset clears it, and fastboot runs before anything sets it) — pending the owner's yes.

What remains, in order of cost: (1) **signed and unlocked** — images signed with the owner's key so ABL *reports* tampering (orange/yellow), ferros re-flashed from fastboot when Android stomps on it; integrity by detection and recovery, never by prevention; the posture today minus the signing. (2) **A UFS command firewall at EL2** — ferros stays hypervisor, Android runs at EL1 with the hardware passed through, but the UFS doorbell is trapped and the 32-slot command list shadowed: every SCSI WRITE's LUN/LBA is checked against the protected set before the real doorbell rings. No virtual disk, one trap per command batch, the S2MPU fence already keeps the device's DMA out of ferros's memory. The only sandbox that holds against root inside Android; a bounded project, not pKVM. (3) **No Android.** Locking the bootloader before (2) exists is the one thing not to do.

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
Stage 8: Process Launch     <100ms   (server spawning)

Total target: <500ms from seed jump to the first process running
              (husky: measured from the "Ready?" press; the plumbing stages before it are not counted)
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

2. ~~**Device-side PT dispatch**~~ — **DONE 2026-08-15: full bidirectional PT command round trip on hardware.** `ferros-bridge diag` returns live diagnostics: SPEC → ACK → DATA → COMPLETE → dispatch → PT-framed response (SPEC + DATA, root hash verified). The DATA→COMPLETE hang was the DWC3 driver, not the protocol (`ferros_pt/tests/wire_replay.rs` proved the protocol sound on host first): `bulk_out_arm`'s unconditional ENDTRANSFER errored a DEPCMD on every re-arm and produced a spurious len=0 ep2 completion that ate the first DATA packet. Fix: ENDTRANSFER only when a transfer is actually in flight. Found in one boot via the EP0 vendor debug readout (`VENDOR_REQ_DBG` G#5A / `ferros-bridge dbg`, raw event ring on G#5B / `dbg2`) — EP0 keeps working when the bulk path wedges, so the counters are always reachable. Device responses are PT-framed with each packet pre-padded to 512 so the idle-pump chunks align with packet boundaries; blast mode, no FIN. **Enumeration watchdog** (same commit series): 4s of USB silence without reaching configured → full PHY+DWC3 re-init → fresh attach; disarms only on SET_CONFIGURATION. Validated three boots in a row: self-heals to enumeration in 35-80s, zero button intervention.
3. ~~**RELOAD hot-reload**~~ — **DONE 2026-08-15: kernel-over-USB proven through three consecutive generations.** `ferros-bridge reload <bin>` sends the flat image over PT (95 chunks, per-chunk BLAKE3), the kernel stages it, write-verifies the copy, flushes the COMPLETE, clears Run/Stop for a graceful detach, invalidates the icache, and jumps. Generations **ping-pong between the two proven slots** — the ABL load base (`G#8008_0000` on husky) and base+32MiB — with the vacated slot handed to the successor via `x0 | 1` (ABL's DTB pointer is 8-aligned, so bit 0 tags a reload handoff; DIAG reports `BASE=`/`STAGE=`). Marching further up is fatal: base+64MiB staged and hash-verified fine but never came back from the jump — stay inside the proven pair. The entry path being fully PC-relative (adrp/adr throughout) is what makes the two-slot scheme work. Iteration is now build → `bridge reload` → 30-80s → new kernel answering diag; fastboot is only needed if a hot kernel bricks itself.
4. **Reboot-to-fastboot from ferros**: PSCI SYSTEM_RESET boots back into slot a; getting to fastboot still needs the button dance. Port the PMU INFORM magic from `exynos-reboot.c` (gs-google) so `ferros-bridge reboot fastboot` works.
5. **androidboot.* synthesis**: enumerate exactly what Graphene init requires for the chainload path.
6. **AVB footer** (locked endgame only): unsigned images boot with a logged ERROR_VERIFICATION while unlocked; the avb_custom_key signing path picks this up in Phase 5.

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
