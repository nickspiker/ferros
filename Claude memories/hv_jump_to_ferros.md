---
name: hv-jump-to-ferros
description: "Power+vol up under Android = jump: hv freezes Linux (stage-2 pull), parks cores on a mailbox, boots ferros fresh on the hv core, no bootloader. LANDS since 2026-11-02 (jumps 35-37): attest screen, USB, touch, display right, from Android at full or scaled resolution; radios still gated after a jump"
metadata:
  type: project
---

Owner direction (2026-11-01): "jump to ferros", not reboot. Halt Android, fresh ferros boot, no bootloader nag, no RAM wipe (owner: skip it). The reverse (the circle back to Android) is wanted too, with no reboot.

**Built (`FERROS_HV_CORE`, HV.md "The jump to ferros"); lands since 2026-11-02 (jump 35):**
- The freeze pulls stage 2; Android's cores park powered on a mailbox (G#A_FFFE_0000), ferros starts them from it (platform::start_core); no PSCI CPU_ON of an Android core (hung in EL3 one time in two).
- ferros boots on the hv core (core 3) by a branch, not CPU_ON (core 0's hung).
- The 35-jump hunt's real bugs, in order: drivers meeting Android's hardware (gated off after a jump), the TRNG/keys lock, the X3 never launched (x3::launch took a non-zero boot MPIDR for a reload; display and touch pin to the X3 and starved), the layer scaler, the DQE and DECON's configuration left at Android's (the display server runs its full modeset after a jump; works from the scaled resolution too, jump 37).
- Diagnosis tools that found them: the pstore ring mirror (loses its position word on watchdog deaths: use the crumb page), the mark trail (G#FD60_0240, previous at +8), per-worker steps (G#FD60_0B00), the hand-off trace, the 60 s escape's thread dump.

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

2026-11-03: after the deep-sleep fix, jumps are intermittent (5 tries: 3 froze at the first firmware TRNG_RND64 from ferros, 1 lost the X3 + two workers, 1 landed). Suspect Android cores frozen right after a PRIV_REG (G#8200_0504) PMU write, before CPU_SUSPEND, reaching ferros armed to power down at WFI; the jump logs last SMC + x1/x2 per core (HV.md).

2026-11-03 evening, FOUND: parked cores kept Android's PSTATE.PAN; with E2H/TGE it applies at EL2 and the first load from the EL0-accessible hand-over block after MMU-on faulted silently (hv vectors unexecutable). Fix: entries eret to themselves (SPSR G#3C9) first. Also active clocks at once after a jump (Android left the X3 at 377 MHz). Jump 12 landed clean. The PMU-write and DMA theories were tested and ruled out.

2026-11-03: jumps 12-14 clean after PAN + clocks. Camera jumps still fail: UFS doorbell busy (fixed: adopt only when UTRLDBR==0) and DECON reprogrammed under ferros (static) + later watchdog. Android DMA masters keep running after the freeze; proposal: hv fences all non-ferros S2MPUs (deny, fault mode) at the jump.

**2026-11-04: jumps from a screen-off Android.** Android's screen state at the jump decides which path ferros takes: DPU on or off, panel rail on or off, panel awake or asleep. Each combination failed for its own reason; all are fixed and committed:
- USB: the TCPC was left as a source (role G#11/G#B, VBUS detection off). The kernel sets the sink role G#0A and EnableVbusDetect.
- The TE pin: gpp0-3 must be function **2** (not G#F); Android leaves it 0 or G#F.
- The modeset after a jump uses the cold-boot tables (ColdPipe), not Android's live pipe; the live PHY gave PLL 0, so TE_CTRL1 was 0.
- The panel: asked its power mode (0x0A) after any jump and reset and powered up if not G#9C.
- VDDD: `ferros_sys::spawn` silently kept only 24 slots and the display has 25. Raised to 64, with a panic past that. The display also pins to the X3 for its power switches, because SYS_POWER is X3-only.
- Layer alpha: RDMA_IN_CTRL_0 alpha is set to G#FF and DPP ALPHA_SEL to 1, written explicitly.
- After a DPU reset: the DPUF S2MPUs (G#1987_0000, G#19C7_0000) come back with protection on and every DMA read faults; the `dpu` switch clears it. DECON window-block words at +G#8D00 + k·G#1000 blank the output at reset; they are written to ABL's values.

**How to hunt display bugs:** register-diff a black state against a cold boot's picture (scratch capture scripts). `diag dpu off` + display op 31 (panel off) + `diag restart display` reproduces the DPU-from-reset jump on a cold boot, with no Android. `tecount` (`bridge spawn tecount.bin --hw gpio-gpp0`) counts TE edges.

**Jump keys:** hold volume up first, then power. Pressing power first is forwarded to Android, which wakes the screen.

**Open:** the DPU S2MPU is left open, as ABL leaves it. Fence it to the display's DMA regions, like the HSI1 S2MPU.
