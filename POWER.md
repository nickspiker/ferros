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

## Plan, biggest first

1. **Make rest cheap.** The between-frames redraws touch only what changed: the clock's digits once a second and its sweeps' wedges; the scope's plot only (no cleared black), and nothing at all for a widget off screen or off. Then the X3 can rest at a low operating point.
2. **A clock policy from what the kernel knows.** Rest: MIF 1014, INT 178, the clusters at their floors, the X3 at the lowest point that keeps the rest work under half its time. Active (a finger down, a PT transfer moving, a frame being drawn): the X3 at 2.0 GHz and MIF 2028, back to rest a second after. 2.9 GHz only if 2.0 misses frames.
3. **Sleep.** No event stream: the kernel's core waits in `wfi` for the next deadline (its timer) or an interrupt (USB, keys, touch, the AoC's mailbox).
4. **Power down idle cores** (PSCI CPU_SUSPEND to the power-down state), and the clusters when all their cores are down; the kernel's housekeeping on a small core.
5. **USB off on battery**: no VBUS, no DWC3 and PHY.
