# POWER — the husky's power profile under ferros

## How to measure

`bridge diag "power start <s> [gap in 1/16 s] [input]"`, wait, then `bridge diag power`. The charger's buck is cut for the window (and CHGIN deselected with `input`), the service loop samples the gauge on its own passes from half a second after the cut, and puts the charger back at the deadline whatever the host does (service.rs `power_start` / `power_poll`). Do not talk to the phone during the window.

The old `bridge display power` busy-waits between samples on the kernel's core: every reading carries the X3 spinning flat out and the display server stalled behind it (its passes stretched to 16 s). Its numbers in DISPLAY-POWER.md are whole-phone figures with that load in them; compare them only with each other. Readings taken seconds after a display restart are high too (all four phases refilled).

## 2026-10-25: the clocks at rest

The map zoomed on the light scope (the clock's face half on screen), DBV 1353, the scope on, the flicker stream on; 20 s windows, settled. Each row adds to the one above.

| state | draw |
|---|---|
| boot defaults: MIF 2028 MHz, INT 356, CPUCL0 1425, CPUCL1 1065, X3 2910, DSU 1197 | 3.59 W, spikes to 1.44 A |
| X3 to 1000 MHz | 2.83 W, spikes gone |
| MIF to 1014 MHz | 2.47 W |
| MIF to 421 MHz | 2.50 W (no further gain) |
| INT to 178 MHz | 2.46 W |

The X3 alone, with MIF 1014, INT 178, CPUCL0 324 and CPUCL1 402 (the clusters' floors):

| X3 | draw |
|---|---|
| 2914 MHz | 3.37 W, spikes to 1.5 A |
| 2000 MHz | 2.71 W |
| 1400 MHz | 2.57 W |
| 1000 MHz | 2.47 W |
| 500 MHz | 2.36 W |

- **The X3's top step is a voltage cliff: 2.9 → 2.0 GHz is 0.66 W** with the same work done.
- **The rest-state work is what holds the X3 up.** At rest the display server redraws the clock and the scope between frames, every 10 Hz flip: at 2.9 GHz 17 ms (clock, 1.1 Mpx region) + 13 ms (scope, 0.84 Mpx) per 100 ms; at 1 GHz 45 + 32 ms, and passes stretch to 200 ms. Both clear and republish their whole bounding boxes, mostly black, when a sliver changed.
- **The kernel never sleeps for long.** Its service loop runs on the X3 and idles in `wfe` with the event stream at 2^13 counter ticks: ~3,000 wakeups a second, a full loop pass each.
- **Nothing powers down.** Idle cores wait in `wfe`/`wfi` at their cluster's voltage; no PSCI power-down state is used, no cluster ever goes off.
- **Floors are real.** CPUCL0 at 300 kHz below its 324 MHz minimum (zuma-cpu.dtsi) was answered `Ok` by ACPM and hard-reset the phone a few seconds later; the next boot found ACPM refusing (`Err(2)`) and reset again. Only the device tree's operating points (CPUCL0 324–1704, CPUCL1 402–2367, X3 500–2914 MHz; MIF 421–3744; INT 178–1066).
- Earlier: the AOD floor (panel in its low-power mode) was 2.12 W whole-phone with the old measurement, so roughly 1.9 W of today's floor is SoC, DRAM and USB, not panel.

## 2026-10-25: the clock policy

`clock_policy` (service.rs): rest — X3 2.0 GHz, MIF 1014, INT 178, CPUCL1 402, CPUCL0 324 — and active — the boot's clocks — on a touch taken by the touch driver or a PT transfer, held a second. On by default from 16 s after the service loop starts; `diag dvfs policy on|off`, `diag dvfs profile rest|active`.

| state | draw |
|---|---|
| boot clocks, same scene | 3.50–3.59 W |
| rest (policy), fresh boot | **2.21–2.22 W** |

The small clusters at their floors alone took 3.5 to 2.80 W; MIF and INT then 2.54 W; the X3 at 2.0 GHz 2.23 W.

**The hang it first had:** the clusters', MIF's and INT's rate requests take no reply (as Linux sends them); five went out back to back and the loop went into `wfe` straight after. With the charger cut (the phone on battery) the X3 never woke from that `wfe`: the watchdog reset it, the last crumb G#B3 (before the idle `wfe`), its timer control still G#C7 (event stream on) — so not a lost event stream. The same switches by hand (followed by a PT reply, not a sleep) never hung. Fixed by reading each domain's rate after setting it: the read takes a reply and the firmware answers in order, so the loop sleeps only once every request is done. The longest such request measured 2.8 ms.

Then the DSU (the clusters' shared L3 and snoop, 1197 MHz) to its 324 MHz floor: 2.24 → 2.16 W, now in the rest profile. BCI (840 → 315 MHz): nothing measurable. The camera, TNR, MFC and BW domains refuse a rate (`Err(1)`/`Err(2)`): powered off, as they should be — never send them requests.

A third hang came after those refused requests, with the DSU and BCI at their floors and the charger cut: the same signature (the X3 in the idle `wfe`, event stream on). The idle `wfe` now has a backstop: the EL2 timer armed 2 ms out, so a GIC interrupt ends the wait whatever the event stream does, and DIAG `BACKSTOP_WAKES` counts sleeps it ended. Ten 30 s windows at rest since: no hang, no backstop wake — so the backstop has not been needed, and the two triggers taken away (the no-reply burst, requests to powered-off domains) are the likelier explanation. Kept as a net.

## 2026-10-25: Android for comparison, and the UFS link

Android on the same phone (slot a, production build, no root): the cable out, adb over WiFi, `current_now` once a second with the screen on (home screen, brightness 158/255) and the battery's charge counter over 15 minutes with it off (WiFi and the adb connection up, so a light idle, not its deepest).

| | Android | ferros |
|---|---|---|
| screen on, idle | 1.16 W | 3.50 W at the start of the day → 2.21 W (clock policy) → **1.58 W** (UFS hibernate) |
| screen off | 0.31 W | 1.70 W → **1.02 W** |

Screen off at rest with the clocks at their floors still drew 1.63 W, and the CPUs were not it: the four workers switched off saved 60 mW, the X3 at its floor 60 mW. The UFS link was: it idled at full speed for as long as ferros ran. Linux on zuma has no auto-hibernate (no `samsung,support-ah8`) and enters by hand from its clock-gating path, with the vendor PHY steps around it (`exynos_ufs_hibern8_notify`: post_h8_enter after DME_HIBERN8_ENTER, then the internal clocks stopped; clocks on and pre_h8_exit before DME_HIBERN8_EXIT). ferros now does the same (ferros_hal ufs `h8_enter_if_idle`, `h8_wake` before every command): after 16 ms idle the link hibernates. **0.65 W** with the screen off (1.69 → 1.04), 0.6 W with it on. Every command brings it back; vault writes and reads come through intact, no failure and no lost command over hundreds of entries. `diag ufs h8 [on|off]` and its counts; a reload wakes it first.

The vault's key count (fact 7) was read every 2 s and woke the link each time; now every 64 s.

Touch: the controller kept scanning with the screen off. The touch process now sends the vendor driver's screen-down mode (`A0 00 00`, sensing off) when the screen goes off and `A0 00 FF` when it comes back.

## 2026-10-25: the cable out, and the dark profile

Screen off, the dark clock profile (X3 500 MHz, MIF 421, the display clock 143 MHz; the policy's third state), a 60 s window started with `after 60` and the cable pulled before it began: **0.85 W** — against ~0.98 W with the cable in, so the live USB link (PHY, repeater, controller, the host's polling) is ~0.13 W, and it includes the USB re-init the loop runs every 2 s while no host is there. Android, cable out, screen off: 0.31 W.

Pulling the cable used to lose the link for good: the MAX77759's USB data switch (TCPC vendor register G#93) opens on a detach and nothing closed it again. Fixed (the re-init closes it when CC is attached and VBUS present); see the USB commit of the same day.

## 2026-10-25: rail by rail (the PMICs' meters)

`diag odpm start`, then `diag odpm` (odpm.rs): the S2MPG14's and S2MPG15's on-device power meters, every named rail (zuma-pmic.dtsi) in batches of 12 per PMIC, a second each, sampled from the service loop. A clock switch resets the accumulators under a batch (the first sweep read 14–43 W on some rails: wrapped differences), so a batch that goes backwards is measured again and the sweep starts two seconds after its command. The outputs are each rail's own: the LLDO bucks feed the LDOs, so their sum counts some power twice; the panel's VSYS feed and the radios are outside.

Screen off, dark profile, the cable in (rail outputs, mW): BB_HLDO 336 · S3S_LLDO1 168 · L2S_PLL_MIPI_UFS 127 · SA 113 · S5M_VDD_INT 91 · S11S_G3D_GLB 66 · S6M_LLDO1 56 · S4M_VDD_CPUCL0 54 · L1M_ALIVE 54 · S8M_LLDO2 51 · S2M_VDD_CPUCL2 38 · S6S_LLDO2 37 · S3M_VDD_CPUCL1 36 · S1S_VDD_CAM 36 · S1M_VDD_MIF 32 · L24S_HSI 31 · S7S_MLDO 26 · S10S_VDD_SLC_M 23 · L6M_HSI 22 · S4S_VDD2H_MEM 20 · the eUSB LDOs 31 together · the PCIe LDOs ~15 · S12S_VDD_AUR 12. Screen on at rest adds the X3 (CPUCL2 412 mW: the between-frames redraws and the loop), the display's supplies (L22M_DISP_VDDD 87, L14M_DISP_VCI 22) and more on INT, CPUCL0 and MIF.

- The CPUs are no longer the floor (the X3 38 mW dark).
- **Blocks ferros never uses draw from their rails because their SoC power domains are on**: the GPU (S11S_G3D_GLB 66 mW and its others), the camera (S1S_VDD_CAM 36), AUR (12), and the PCIe PHYs (~15). Linux keeps these rails always-on too and saves by gating the domains in the PMU (exynos-pd); ferros gates none. That is the next real fix — through the PMU, carefully (EL3 refuses even reads of those registers; the EL3 poison rule).
- BB_HLDO (the 3.0–3.6 V buck-boost under the high-voltage LDOs) and SA (the 1.8 V I/O buck: UWB, display I/O, audio codec, …) are always-on in Linux as well; their loads are what to find, not the rails.
- L2S_PLL_MIPI_UFS stays at 127 mW with the screen off and UFS hibernating: PLLs and PHYs left running.

## 2026-10-25: the unused power domains off

Every SoC power domain read on through EL3 — ABL leaves them so: the GPU and its embedded part, G2D, the camera's ISP chain (GDC, GSE, MCSC, RGBP, TNR, YUVP, ISPFE), MFC, AUR, BW, EH, the TPU. ferros had never turned one off. pd.rs runs Linux's off sequence for each (flexpmu_cal_local_zuma.h: the secure world's save, SMC G#8200_0410; the domain CMU's controller option bit 24; PMU CONFIGURATION bit 0 — every PMU access through EL3, a fixed millisecond in place of a STATUS poll, for the EL3 poison rule), once, 16 s into the service loop.

| | before | after |
|---|---|---|
| rail outputs, screen off (odpm) | 1558 mW | 1185 mW (PLL rail 127 → 25, INT 85 → 23, GPU 66 → 15, camera 35 → 14, S3S_LLDO1 168 → 49) |
| battery, screen off, cable in | 0.98 W | **0.63 W** |
| battery, screen on | 1.58 W | **1.24 W** |

Bringing a domain back (`diag pd on`, CONFIGURATION through EL3 and the secure restore, without Linux's saved CMU registers) is not yet trusted: a TPU on/off by hand preceded a reset during the next power window. The camera and TPU diags refuse while their domains are gated; the GPU's way back (its embedded part's PLL and atomic-alias sequence) is not written.

## 2026-10-25: the external channels

The meters' external shunt channels (VSYS feeds, zuma-pmic.dtsi `external_rail`: 10 mΩ display and WLAN_BT, 5 mΩ the rest) are in the sweep too. With the domains gated:

| | screen off | screen on |
|---|---|---|
| "VSYS_PWR_WLAN_BT" (sub VSEN_C1) | 0.7 mW | **318 mW** |
| VSYS_PWR_MODEM | 17 mW | 26 mW |
| VSYS_PWR_RFFE | 15 mW | 18 mW |
| VSYS_PWR_IFPMIC (the modem's PMIC) | 8 mW | 17 mW |
| VSYS_PWR_DISPLAY, VSYS_PWR_MMWAVE | under 1 mW | under 1 mW |

- The channel the device tree names WLAN_BT follows the screen and WiFi is not running at boot: on husky it is the panel's feed (the names look like another board's). The panel, this dark scene at DBV 1353: ~0.32 W.
- The modem, never booted by ferros, draws ~40–60 mW across its feed, its RF front end and its PMIC: powering it down (pmucal_cp, the IF PMIC) is a candidate.
- BB_HLDO stays ~324 mW screen on or off and its LDOs read a few mW each: its load is still unknown — or its resolution (BB_POWER, 0.010989 mW per count, the one constant not cross-checked) is off.

## 2026-10-25: the kernel's sleep, and focus

- The kernel core's event stream every 2.7 ms (counter bit 15) instead of 333 us: 15–25 mW less with the screen off (four alternating windows). The idle `wfe`'s EL2 timer is armed at the next sleeping thread's deadline (capped at 2 ms), so deadlines land on time whatever the stream. `diag evnt <8-15>` sets the stream live.
- The clock policy counts display frame starts: more than 24 a second (the rest flips are 10) is active, as a finger is. The focused scope ran 61 frames a second at the rest clocks and runs 90 at the active ones — **at 3.96 W**: full rate in focus is that expensive.
- A display server that outlived a map restarted in focus kept flipping at the panel's rate, and the policy held the active clocks (2.9 W at rest): the map now always sends its focus state on its first frame.

Fresh boot, all of today's changes: screen on at rest **1.21–1.26 W**, screen off **0.61 W** (cable in).

## Where it stands, and what is left (2026-10-25 evening)

| | start of the day | now | Android (same phone) |
|---|---|---|---|
| screen on, at rest | 3.50–3.59 W | **1.21–1.26 W** | 1.16 W |
| screen off, cable in | (1.70 W with the first clock policy) | **0.61 W** | — |
| screen off, cable out | — | (0.85 W before the domains were gated) | 0.31 W |
| the scope in focus (90 frames/s) | — | 3.96 W | — |

Left, roughly by size:

1. **The cable-out screen-off number again**, with everything since: the live USB link was ~0.13 W of the cable-in figure.
2. **The USB re-init loop while no cable is in**: every 2 s a full PHY and controller init. Re-init only when the TCPC shows VBUS (the replug path closes the data switch then anyway). Needs a hand on the cable to test.
3. **BB_HLDO** (~324 mW by the meter, screen on or off, its LDOs a few mW each): either a load hanging on the 3.3 V buck-boost or its meter resolution is off — the meter's absolute total exceeds what the battery delivers, so treat its absolute numbers with care; its changes match the gauge.
4. **The modem's supplies** (~40 mW: its VSYS feed, RF front end, its PMIC) with the CP already held off (CP_PWR low) — likely what Android pays too.
5. **Bringing gated domains back properly** (Linux's saved CMU registers replayed; the GPU's embedded part), so the camera, TPU and GPU work can run without a reboot.
6. **Screen on: the rest redraws** — the gravity-following clock repaints its whole face on every 10 Hz flip and the scope its plot box; cheaper redraws would let the X3 rest below 2 GHz. A design choice for the owner (the clock's no-dead-zone rule is part of it).
7. **Focus at 90 frames a second costs ~2.7 W over rest**: whether focus should run at full clocks is the owner's call.
8. Brightness: the panel's floor alone moves ~0.8 W between 2 and 140 nits; automatic brightness from the front light sensor is the owner's call.
