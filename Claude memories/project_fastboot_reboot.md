---
name: Fastboot reboot investigation
description: All approaches tried for reboot-to-fastboot on FP5 QCM6490, none working yet
type: project
---

## Goal
Reboot from ferros directly into fastboot mode so the user doesn't have to manually hold volume-down+power.

## Approaches tried (all failed as of 2026-03-14)

1. **SPMI PON_SOFT_RB_SPARE** — PM7325 SID=0 PID=0x08 offset 0x88. Blocked by SPMI arbiter (PON not in APID map).

2. **PSCI SYSTEM_RESET2 (SMC32 0x84000012)** — vendor reset_type=0x10001, cookie=0x2 (from Qualcomm upstream patches for QCM6490). Device reboots normally, doesn't enter fastboot. Stock FP5 TF-A probably doesn't implement vendor SYSTEM_RESET2 extensions.

3. **PSCI SYSTEM_RESET2 (SMC64 0xC4000012)** — same parameters, same result.

4. **IMEM restart_reason** — IMEM base 0x146AA000, offset 0x65C, magic 0x77665500. IMEM IS accessible (reads back data), but writing 0x77665500 to 0x146AA65C + SYSTEM_RESET does not trigger fastboot. Pre-existing value at 0x65C was 0x6076D077 (not a standard Qualcomm magic), suggesting ABL may not check this offset on QCM6490.

5. **SDAM (PMIC Shared Device Auxiliary Memory)** — blocked by SPMI arbiter EE ownership.

6. **IMEM DLOAD mode** — not tried yet, but this enters EDL (9008 mode), not fastboot.

## Root cause (confirmed 2026-03-14)
ABL reads reboot reason from **PMK8350 PMIC** (always-on PMIC at SPMI SID 0), NOT from IMEM. The relevant registers are:
- **PON HLOS** (PID 0x13, reg 0x8F) — designed for HLOS access
- **SDAM_2** (PID 0x71, reg 0x48) — nvmem restart reason cell
- **PON PBS** (PID 0x08, reg 0x8F) — legacy PON scratch
- Value for fastboot: 0x04 (FASTBOOT_MODE=0x02 shifted left 1 bit into bits [7:1])

**ALL THREE are NOT_FOUND in the SPMI APID map** — the arbiter completely blocks EL1 access to all PMK8350 reboot-reason peripherals. The stock kernel writes these via SCM calls through TrustZone, which we don't have.

## SDAM_2 discovery (2026-03-15)
- SDAM_2 found at PPID 0x0871 (**SID 8**, PID 0x71), APID 0x122, ownership EE=0
- Observer read stride was wrong (0x1000 → fixed to 0x80), reads now work
- reg 0x48 reads as 0x00 (correct for "no reboot reason")
- **Direct SPMI write: STATUS=DONE(0x01) but value doesn't persist** — readback shows 0
- **SCM IO write to arbiter channel: also doesn't persist**
- Theory: SID 8 might be a read-only alias. The DTS says PMK8350 is SID 0 but the APID map shows SID 8. The writable SDAM might be on a different arbiter or SID.
- Writing to SDAM data registers (0x50-0x52) during boot kills USB — PBS-triggered side effects
- Arbiter is v5 (VERSION=0x50020000), not v7. Single bus (FEAT1=0).

## What might work (untried)
- **Boot Android (slot B), trace SPMI writes** during `adb reboot bootloader` using debugfs/ftrace — find the exact SID/arbiter path Linux uses
- **SCM call with different function ID** — there may be vendor-specific Qualcomm SCM calls for reboot reason not in upstream
- **RPMh TCS** — check if SDAM is available as a cmd-db resource type (unlikely, SDAM isn't a VRM)
- **Custom TF-A** that implements SYSTEM_RESET2 vendor extensions

## Boot timing
- Nag screen (unlocked bootloader warning): ~10 seconds
- Pre-boot: ~2 seconds
- USB enumeration after ferros starts: ~5 seconds
- Total from fastboot reboot to ferros-bridge usable: ~17 seconds

**Why:** User currently has to manually hold buttons to enter fastboot for each flash cycle.
**How to apply:** Don't re-try the failed approaches above. Focus on reverse-engineering ABL's actual check, or finding an alternative mechanism.
