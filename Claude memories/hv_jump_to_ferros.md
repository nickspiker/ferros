---
name: hv-jump-to-ferros
description: "Power+vol up under Android = jump: hv freezes Linux (stage-2 pull), boots ferros fresh, no bootloader. Mechanics solid; drivers meeting Android's hardware state stall the bus; panel stays on Android's frame"
metadata:
  type: project
---

Owner direction (2026-11-01): "jump to ferros", not reboot. Halt Android, fresh ferros boot, no bootloader nag, no RAM wipe (owner: skip it). The reverse (the circle back to Android) is wanted too, with no reboot.

**Built (`FERROS_HV_CORE`, HV.md "The jump to ferros"):**
- The freeze stops all 8 Android cores in 1-3 ms.
- ferros starts on core 0 with GATE_ANSWER 6; SYS_FACTS 31 = 6 tells processes.

**The real work left:** every ferros driver assumes the bootloader's hardware state. After a jump they meet Android's state and stall the bus, and a watchdog resets the device (APM or APC). Seen so far:
- the modem's register read;
- the AoC start;
- WiFi's BAR write;
- the kernel service loop on the X3 (jump 11, not named).

These are gated off after a jump for now. The panel still shows Android's last frame although DECON sends frames: diff the display blocks, cold vs jumped.

**Diagnose with:**
- `bridge read FD6001C8 48`: the previous boot's last register access per core.
- `PREV_NOTE_*` in DIAG: the lock holder, plus 0x90/0x70 + syscall n per core.
- the bootloader log at G#FD80_0000: the reset reason.

**Both slots identical** (owner): flash with `FERROS_HV_CORE=1 FERROS_HV_UFS=1 tools/husky-cold.sh` (slot b first, then a; FERROS_SLOT_B_ONLY=1 only for a quick test), which signs the image and writes vbmeta_b. Hand-flashed unsigned images, or a set-active from a warm fastboot, got slot b refused by the bootloader.

**Android OTA warning:** Android thinks it is slot a, so an update writes slot b (ferros's). Keep updates postponed until the UFS firewall refuses writes (not built yet; it only measures).

Related: [[hv-chords-decided]], [[ufs-firewall-decided]], [[android-behind-attestation]], [[feedback-measure-before-fixing]].
