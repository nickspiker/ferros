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

**Both slots identical** (owner): flash with `FERROS_HV_CORE=1 FERROS_HV_UFS=1 tools/husky-cold.sh` (the booting slot first, then the other; FERROS_ONE_SLOT=1 only for a quick test; never a slot switch since 2026-11-18), which signs the image with avb-footer and keeps it in keys/husky/images. Hand-flashed unsigned images, or a set-active from a warm fastboot, got slot b refused by the bootloader.

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

**DPU S2MPU fenced (cf2acbd):** VID 0 may read display-fb and display-phases only. It is fenced at service start when the DPU is on, and by the `dpu` switch after power-up. One fault at boot is expected: DECON is still reading ABL's splash at G#FAC0_0000.

**Test trap:** `diag restart display` alone leaves the screen black once someone has attested, because the home screen only hands its scene to a display server when the home screen starts. Restart the map too, or use tools/husky-shell.sh display.

**2026-11-07 morning: the jump lands again, static on the panel.** The jump had regressed: ferros died between crumbs C0 and C1 (the eUSB repeater's I2C on hsi2c_11, still in Android's state) and the cluster watchdog reset it; ferros now skips that snapshot/tune after a jump (Android tuned it). With that the jump lands (USB, vault), but the panel shows black over static: the layer DMA (DMA0, G#1990_0000) is stuck busy (RDMA_ENABLE G#4000_0004, OP_STATUS) from Android's in-flight read when the freeze pulled stage 2, so DPP0's shadow words (+G#100: scaler on at 1008x2244, Android's ratios) never latch ferros's active ones (+G#80: off, 1344x2992). IDMA_SFR_UPDATE_FORCE does nothing; IDMA_SRESET / DPP SRESET never complete; `diag dpu off` then cannot power DPUF0 down (status stays 1) and DPUF1 will not come up: only a reset recovers. Fix direction: quiesce the DPU's DMA before the freeze (stop DECON's trigger, wait for the DMA idle) or reset the DPU block before the S2MPU fence.

**2026-11-13: "works a minute or two, then reboots" was ferros's own 60 s escape** (dev builds reset a jumped session with no USB command in 60 s). Tell a reset's kind from ABL's log (`bridge read FD800000 80000`): `[PRE] RST_STAT` is the reset that ended the session (G#80000 PIN_RESET + reason G#CECA = a ferros PSCI reset; CLUSTER0_NONCPU_WDTRESET + G#CBEA = the watchdog), `Reboot Info` is the bootloader's own. After a jump an unclean reset goes thru ABL's dump stage and a power cycle (PIN|PO), so DRAM, the report page and pstore decay: only the flash log survives. The escape now logs once and runs on (HV.md). USB after some jumps still never configures (TCPC power G#C, not sinking): open.

2026-11-14: the b871 non-configuring jump (resets 0, connects 0, DSTS link in suspend) points at the path in front of the DWC3, which Android takes down when its USB goes idle (repeater disabled mode G#B2 bit 6, PMU isolation G#1546_3EB0, LDO8M/9M/19M, HS ReWA, PHY suspend) and the jump never undid (the boot skips the repeater after a jump). Built: `pixel8_usb_path_restore` (main.rs) on every `usb` power-on, read-then-fix to the measured cold state, one `usb path` flash-log line naming what it found. Untested on a jump: after the next one read `diag log` for that line (HV.md "USB that never configures after a jump").
2026-11-15 (b878): that line read "changed: u2phy suspend off, u3pipe suspend off" and USB configured.

**2026-11-15: the radios after a jump (b878, the Pro).** Android leaves PCIe ch0 and ch1 suspended: PERST low, the PHY powered down, the PMU isolation RELEASED (G#4C1): the ELBI/DBI stall state. With the PHY isolated the controller reads at reset (nothing of Android's to undo). Fix: pcie1 and modem begin every bring-up with their own `off` (PHY isolated, chip down, no ELBI/DBI), then the cold path; init starts WiFi, BT, NFC and the modem after a jump (sensors and GPU still out); the kernel runs radio_down and fence_modem after a jump too, and brings the NFC bus up fresh. Measured by hand on b878: WiFi joined and leased, BT and NFC up, Android's live CP taken down. The capsules are stored and sealed on the Pro (old ones in /mnt/Harbor/tmp/jumpradios/back); the kernel half waits for a flash. Modem registration after a jump not yet measured. HV.md "The radios after a jump".

**2026-11-15: the high bank after a jump (HV.md "The high bank after a jump").** Tripwire (`diag frames trip arm|check|off`) on b879 (a reload into b878's session, high bank held): 9240 MiB armed, 0 words changed in 12.4 min. Measured live: no trace (TMCs off, ETR no buffer), GPU powered off, the AoC's SysMMU on with its table at G#8FBC_0000 inside ferros's display phases and its S2MPU open. Built: a reload reads the hv counters (G#FD60_0D00) and stays jumped; after a jump the kernel stops capturing TMCs, denies the GPU when on, fences the AoC to its carve-out + ferros's AOC_L1, keeps HSI0 denied until the `usb` switch resets the DWC3; the high bank is in the pool and idle workers wipe it after attest. Trap: reading every core's ETE frame (G#2Bn3_0000) or ETR_MIU at the start of a jumped session stalled the bus (b880, APC watchdog, the session lost): touch the TMCs only. cpucl0 still open after a jump until its deny is tried alone. Next jump verifies all of it (flashed build).

**2026-11-15: the g2d hang after a jump (b883; HV.md "The g2d hang after a jump").** The jump froze in pd's off of g2d. g2d was on for the first time after a jump: G2D's driver holds its domain, its clock and EL3's protection for each job and gives them back only on completion, so a g2d found on is mid-job. The EL3 poison, read from BL31 and measured (`diag pd poison`): an access into a down or erroring block leaves an SError waiting, EL3 bounces the next SMC back to it A#100 times and then panics. `platform::take_serror` takes it at EL2, and that cures it. **A power-down thru the PMU alone (CMU bit 24 not cleared) never completes and fires the APM watchdog (b885): never cut a domain without its CMU write.** The fix: Linux's order with each CMU access checked; a CMU answering with an error leaves the domain on and denies its S2MPUs; after a jump every step gets its own flash-log line. Untested on a real jump. Trap: a reload into a cold session that followed a jumped one runs as jumped (stale hv counters at G#FD60_0D00; fixed 2026-11-16: a cold first kernel sets bit 63 of word 19).
2026-11-16, two jumps on the flashed fix: b893 clean. On b894 (camera open in Android) AUR's power-down never finished: the APM's log ends in its AUR handler (`PMUAURHD`, pend_grp G#8000). EH, ISPFE and TPU queued behind it, the APM wedged, and the APC watchdog fired. The APM runs power-downs one at a time. Fix: STATUS waited for (1 << 13 us); a power-down that does not finish stops the pass and is fenced. After a jump AUR (the GXP) and the TPU are fenced only, never powered down.

**2026-11-20: black screen after the jump = Android's layers (HV.md "The black screen after a jump").** The display's first frame after a jump latched DMA0 as Android left it (AFBC, its IOVA G#F900_0000); with translation off the fetch was denied and DMA0 stayed busy for good. Tells: `diag console kept` shows "frame starts 1" then flips timing out; `bridge read 19900000 50` shows G#4000_0004 and +G#440 an Android IOVA. Fixed both ways (hv `crowbar_layers` at the jump, the display's `own_layer` after it); flashed on the Pro as keys/husky/images/boot_hold_ddfd6437_jumplayers_vaultcarried.img (hv "98814 ufs core"), the display from the pucks-build source with the fix stored (3068993 B, under 3 MiB). IDMA_SRESET never completes on this DPU; SFR_UPDATE_FORCE does. After the next jump read pstore for "layers (bit k = Lk)" and the console for "layer 0 owned after the jump".

**2026-11-28, a jump that reset (the owner: "Failed android jump to ferros"):** b1196 came up after the jump (vault open, identity unlocked, "HSI0's S2MPU denied all after the jump"), then core 8 (the kernel's) went silent 1.3 s after its last pet with interrupts masked and the watchdog reset the device into cold boot b1197. Its last crumb was G#CB, `smp::launch_all` (the worker cores brought up, right after the USB path's mark G#36); core 3's was G#E6 (the settings step). So the jump lands and the stall is in starting the worker cores Android left parked under pKVM. Not chased yet; a `diag` right after a jump would read `stall` and the crumbs.

