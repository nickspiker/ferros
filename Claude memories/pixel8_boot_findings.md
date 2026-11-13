---
name: pixel-8-boot-findings
description: "Lasting lessons from the first Pixel 8 Pro (husky) boots, 2026-03..08: ABL behaviour, boot image, watchdog, USB/PT bring-up, payload tooling. Session logs trimmed 2026-11-12; detail in docs/BOOT.md, USB.md, REPEATER.md"
metadata:
  type: project
---

First ferros boot on husky (Pixel 8 Pro, Tensor G3, not shiba) 2026-08-14; USB/PT, hot reload and payload RUN the next day. What still holds:

**ABL:**
- `fastboot boot` accepts an image and never jumps to it on Tensor G3: flash a slot. `fastboot oem dmesg` dumps ABL's log (one `oem` call per bootloader session; more can wedge fastboot).
- boot.img v4, kernel LZ4 legacy (magic G#0221_4C18). ABL loads the kernel at G#8008_0000 with the MMU and D-cache off, so entry needs no cache/MMU teardown (the old `_entry` teardown crashed, as on M1).
- Cluster watchdogs (`watchdog_cl0` G#1006_0000, `cl1` G#1007_0000) fire ~60 s in unless WTCON is written 0 or they are petted; unsigned images boot while unlocked (AVB logs the error).

**USB/PT:**
- The eUSB2 PHY must be a faithful port of `phy_exynos_eusb_initiate`, including clearing TESTSE.test_iddq (analog powered down otherwise); ABL leaves repeater, PMU isolation and clocks up.
- bulk_out_arm must ENDTRANSFER only when a transfer is in flight (else a spurious len=0 completion eats the first DATA packet). Responses: SPEC + DATA each padded to 512, blast mode, no FIN.
- The long "enumeration coin flip" was SET_ADDRESS's status ZLP started before XferNotReady(Status) (`StatusInWait`, fixed 2026-10-14); host `journalctl -k | grep -i usb` tells a silent PHY from an EP0 wedge.
- VENDOR_REQ_DBG (G#5A) EP0 counters work when the bulk path is wedged (`ferros-bridge dbg`).

**Tooling pattern:** payloads (no_std blob, `payload.ld` pins `.text.entry` at 0) run over `bridge run <blob> [hex]`: probe or poke any MMIO with no kernel rebuild. Reach for a payload before touching kernel init.

**PMU from EL2:** direct writes to the PMU are blocked (S2MPU); go thru EL3 SMC G#8200_0504 ([[husky_linux_handoff_oracle]]).

Hardware from ABL's log: Zuma B1, Samsung KLUDG4UHGC UFS 3.1 HS-G4, CS40L26 haptics, S2MPG14/15 PMICs. Related: [[husky_cold_slot_b]], [[gs_google_reference_tree]].
