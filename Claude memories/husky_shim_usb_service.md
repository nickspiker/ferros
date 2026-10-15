---
name: husky_shim_usb_service
description: "The husky dev loop since 2026-10-13 — runtime handoff into the shim USB service (PT diag/reload/run/mem/poke with UFS live), button-free fastboot via EL3 + watchdog, and the hard-won hardware facts behind it"
metadata:
  node_type: memory
  type: project
  originSessionId: 75576678-89a2-4635-8c20-1eba33babf4a
  modified: 2026-10-14T05:39:48.285Z
---

**Also:** `bridge store put|get|ls|rm` (vault by logical key, persists), `bridge display [--hold] "text"` + automatic boot status screen after a `--live` handoff (DECON adoption, see ferros DISPLAY.md; text on the panel visually confirmed by the owner 2026-10-14), `--live` = /sys/kernel/ferros/go from running Android (screen on, DECON hibernation off).

**Loop:** `tools/husky-hotload.sh` (push + /sys/kernel/ferros/stage + `adb reboot ferros`) → ferros on USB as 1209:4665 in ~35 s → `ferros-bridge diag | run <payload.bin> [hex] | reload <flat.bin> | read <addr> [len] [file] | poke <addr> <val> | reboot [fastboot]`. Disk (UFS) stays live, vault opens from the ira. Built by `shim_usb.rs` (default shim entry; `FERROS_SHIM_ENTRY=vault` = old one-shot probe). See [[husky_ferros_probe_boot]], [[ufs_linux_handoff]].

**Never needs hands:** every exit lands in Android with a ramoops report (crumb byte: G#B0 reboot, B1 no enumeration in 64 s, B2 512 s idle, B3 fastboot, (B5 both volume keys — retired 2026-10-14: the keys are now the killswitch, power off, no report), B6 16 s mid-transfer PT stall); faults → fault sink; hangs → cluster watchdog. Nothing armed at boot. Kernel prints the whole result block as `ferros: r[00..31]` (layout documented on `leave()` in shim_usb.rs).

**Facts measured on hardware (don't re-derive):**
- Enumeration is first-try in the handoff (cold boot needed a retry watchdog) — Linux already applied the eUSB repeater tune.
- Linux leaves USB DMA translating through the USB SYSMMU (G#1104_0000, VM0 ctrl G#25); ferros must clear bit 0 of every VM ctrl (G#8000 + vm*G#1000), not the old offset-0 write.
- HAL bug fixed (50fc550): re-arming bulk OUT on every XferNotReady ENDTRANSFERs the live TRB → STARTTRANSFER "no resource" storm → event buffer overflow. Arm on NotReady only when not armed (Linux dwc3 rule).
- Reload: image is statically linked at G#9240_0000 with absolute addresses in data, so RELOAD stages at G#92A0_0000 and a copy stub (run from G#92E8_0000) moves it to the link base. A 258 KB reload takes ~3 s since the I-cache is on (was ~55 s with it off).
- Fastboot: stamp PMU SYSIP_DAT0 (G#1546_0810) = G#FC via EL3 SMC G#8200_0504 (x1 addr, x2 1=write, x3 val), then fire the cluster watchdog. A ferros PSCI SYSTEM_RESET loses the mode (bootloader: PIN_RESET, reason G#CECA, mode 0); a watchdog reset keeps it.
- **EL3 poison rule (bisected 2026-10-13):** a direct EL2 read of the PMU (G#1546_xxxx) or of the DPU while its power domain is off "works" (returns a value / 0) but makes the NEXT EL3 SMC_CMD_PRIV_REG call hang forever; consecutive EL3 calls are fine. So read PMU state only via `priv:` (EL3), and check DPU power via EL3 first (word 0 of pd-dpub G#1546_1D00 / pd-dpuf0 G#1546_1D80 / pd-dpuf1 G#1546_1E00 = 1 when on; other offsets return -EINVAL). An earlier note claiming "direct PMU reads hang" was wrong (a grep hid the bit-63 address lines).
- Bootloader log of the current boot: reserved G#FD80_0000, 1 MiB (`bridge read FD800000 10000 file` ×16); DSS header G#D810_0000 (reason @+G#300 reads G#CACA while Linux runs, G#CAFE = clean reboot).
- `ferros-bridge` calibrates its (unused) nunc session clock on a background task and exits explicitly, so commands take milliseconds even when time sources don't answer.
