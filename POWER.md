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

## Plan, biggest first

1. **Make rest cheap.** The between-frames redraws touch only what changed: the clock's digits once a second and its sweeps' wedges; the scope's plot only (no cleared black), and nothing at all for a widget off screen or off. Then the X3 can rest at a low operating point.
2. **A clock policy from what the kernel knows.** Rest: MIF 1014, INT 178, the clusters at their floors, the X3 at the lowest point that keeps the rest work under half its time. Active (a finger down, a PT transfer moving, a frame being drawn): the X3 at 2.0 GHz and MIF 2028, back to rest a second after. 2.9 GHz only if 2.0 misses frames.
3. **Sleep.** No event stream: the kernel's core waits in `wfi` for the next deadline (its timer) or an interrupt (USB, keys, touch, the AoC's mailbox).
4. **Power down idle cores** (PSCI CPU_SUSPEND to the power-down state), and the clusters when all their cores are down; the kernel's housekeeping on a small core.
5. **USB off on battery**: no VBUS, no DWC3 and PHY.
