---
name: power-profile-traps
description: "husky power work — measure with `diag power start` (not `display power`, which busy-waits); DVFS only within zuma-cpu.dtsi floors (CPUCL0 300 MHz hard-reset the phone); the 2.9 GHz X3 step is the 0.66 W cliff"
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-25T19:48:22.079Z
---

Power measurement on husky (2026-10-25): use `bridge diag "power start <s>"` then `bridge diag power` (service.rs power_start/power_poll, non-blocking). The old `bridge display power` busy-waits on the kernel's X3 and stalls the display, inflating and scattering every reading; readings seconds after a display restart are high too.

DVFS (`diag dvfs set <domain> <kHz>`): ACPM answers Ok even below a cluster's floor; CPUCL0 at 300 MHz (floor 324) hard-reset the phone and the next boot saw ACPM Err(2). Stay in zuma-cpu.dtsi: CPUCL0 324–1704, CPUCL1 402–2367, X3 500–2914 MHz; MIF 421–3744; INT 178–1066.

Findings: X3 2.9→2.0 GHz saves 0.66 W at rest; MIF 2028→1014 another ~0.36 W; below that nothing. The rest-state display redraws (clock + scope, whole bounding boxes every 10 Hz flip) are what hold the X3 high. Plan in POWER.md.

**Why:** the owner wants a lean power profile designed from first principles, not copied from Android.
**How to apply:** measure each lever with the clean window before and after; see [[sched-trace-and-past-deadline-spin]] for the loop's sleeping history.

**Later 2026-10-25:** per-rail power via the PMICs' meters: `diag odpm start` then `diag odpm` (odpm.rs). Trust its *changes*, not its absolute sum (it exceeds the battery's draw; BB_HLDO's scale is suspect); a clock switch resets the accumulators mid-batch (handled by re-measuring). The big wins were hardware left on, not CPUs: UFS link never hibernating (0.65 W, ferros_hal ufs h8), and ABL leaving every SoC power domain on (GPU, camera ISP chain, MFC, AUR, BW, EH, TPU: 0.35 W, pd.rs gates them at boot through EL3 only). Bringing a domain back with `diag pd on` (no saved-CMU replay) is untrusted: a TPU on/off preceded a reset. The camera/TPU diags refuse while gated. Fresh boot now: screen on 1.21-1.26 W (Android 1.16), off 0.61 W cable in. The panel's VSYS feed is the shunt the DT calls WLAN_BT on husky.
