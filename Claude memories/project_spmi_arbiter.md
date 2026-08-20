---
name: SPMI arbiter EE lockdown
description: SPMI arbiter blocks access to most PMIC peripherals (LDOs, SDAM, PON) — only flash LED and a few GPIO/MPP blocks are APPS-accessible
type: project
---

SPMI arbiter on FP5 (QCM6490) is configured by TF-A at EL3 before ferros boots. Only a subset of PMIC peripherals are accessible to APPS EE.

**Accessible (confirmed writes work):**
- PM8350C flash LED (SID=2, PID=0xEE)
- Various SID=0 GPIO/MPP peripherals (PIDs 0x16-0x20, 0x50-0x60)

**Blocked (EE ownership denied):**
- PM8350C LDOs (SID=2, PID=0xAA/0xB3) — find_apid succeeds, read/write FAIL
- PMK8350 SDAM_2 (SID=8, PID=0x71) — find_apid succeeds, read/write FAIL
- PM7325 PON (SID=0, PID=0x08) — NOT in APID map at all

**Impact:**
- Cannot enable SD card power rails (LDO6 vqmmc, LDO9 vmmc) via direct SPMI
- Cannot set reboot reason for fastboot (SDAM, PON_SOFT_RB_SPARE both blocked)
- SD card power and reboot reason require either RPMh mailbox or custom TF-A

**Reboot-to-fastboot attempts (all failed):**
- IMEM 0x146AA65C: upper IMEM not accessible (data abort)
- SDAM SID=8 PID=0x71 offset 0x48: EE blocked
- PON SID=0 PID=0x08 offset 0x88: not in APID map
- PSCI SYSTEM_RESET2 (0x84000012, type=0x10001, cookie=0x2): reboots but doesn't enter fastboot

**Arbiter config is XPU-protected:**
- Ownership table is at CORE+0x900 (not cnfg base 0x0C4C0000 which reads as zeros)
- Format: each 4-byte entry has bits [31:24] = owner EE (0=APPS, 1=blocked), bits [19:8] = PPID
- Writing to these registers triggers XPU security violation → SoC shutdown after ~28s
- Confirmed: CANNOT reconfigure ownership from EL1

**Charging:**
- ferros does not configure PMIC charging registers (also blocked by arbiter)
- Running ferros all day will drain the battery — USB trickle charge may not keep up
- Must boot Android periodically to charge, or configure charging via RPMh

**Why:** TF-A configures SPMI arbiter ownership before EL1 boot. Power management peripherals are owned by RPMh/AOP EE. XPU hardware enforces this — not just software.

**How to apply:** Don't write to SPMI arbiter config registers (XPU violation = SoC death). For SD card power and charging, investigate RPMh mailbox protocol. For fastboot reboot, may need to dump ABL binary to find what it checks.
