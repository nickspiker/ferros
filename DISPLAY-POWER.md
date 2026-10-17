# DISPLAY-POWER — the husky display's power, measured under ferros

How: the phone on USB to the dev box, running ferros from slot b. For each row the charger's buck is switched off (MAX77759 CHG_CNFG_00 MODE = 0, PMIC.md), so the battery carries the whole system and the fuel gauge's current is the system's draw; half a second for the gauge to see it, then 8 samples a quarter second apart, then the charger back on. `tools/display-power-sweep.sh` is the sweep; `bridge display power 8 4 battery` is one row. The scene is a solid fill over the whole panel (`bridge display fill`), brightness by DCS `51` (DBV 196 = 2 nits, 1353 = 140 nits, 3307 = 1000 nits, HBM `53 E0` + 4095 = 1600 nits). The TE column is the panel's TE pin over half a second, the proof of the state.

The number is the **whole phone**: SoC with one core spinning in the EL2 service loop, DRAM, the USB link, the panel. Differences between rows are the display; the absolute is not a display figure. Run-to-run spread on the same state is about ±50 mW (the 120 Hz black row came out at 3.21–3.26 W across six visits; one earlier visit read 2.83 W, before the sweep set the brightness explicitly — see the brightness finding).

## 2026-10-16, battery 98 %, 37 °C

| state | TE | mean draw |
|---|---|---|
| WQHD 120, 10 bpc, black, 140 nits | 120 Hz, 272 µs | 756 mA at 4271 mV = **3.23 W** |
| WQHD 120, 10 bpc, grey 50 % (code 128), 140 nits | 120 Hz, 272 µs | 1183 mA = **5.02 W** |
| WQHD 120, 10 bpc, white, 140 nits | 120 Hz, 272 µs | 1341 mA = **5.67 W** |
| WQHD 120, 10 bpc, white, 2 nits | 120 Hz | 649 mA = **2.77 W** |
| WQHD 120, 10 bpc, white, 1000 nits | 120 Hz | 1989 mA = **8.33 W** |
| WQHD 120, 10 bpc, white, HBM 1600 nits | 120 Hz | 3127 mA at 4114 mV = **12.86 W** |
| WQHD 120, 8 bpc, white, 140 nits | 120 Hz | 1349 mA = 5.70 W |
| WQHD 120, 8 bpc, black, 140 nits | 120 Hz | 754 mA = 3.21 W |
| WQHD 120, 8 bpc, black, 1368 Mbps link | 120 Hz | 758 mA = 3.23 W |
| WQHD 60, black, 140 nits | 60 Hz, 8639 µs | 720 mA = 3.07 W (second visit 745 mA = 3.18 W) |
| WQHD 60, white, 140 nits | 60 Hz, 8640 µs | 1305 mA = 5.52 W |
| WQHD, idle floor 30 Hz, black, 140 nits | pin 120 Hz (see below) | 729 mA = 3.11 W (3 s settle: 3.08 W) |
| WQHD, idle floor 30 Hz, white | | 1308 mA = 5.53 W |
| WQHD, idle floor 10 Hz, black | | 712 mA = 3.04 W (3 s settle: 3.01 W) |
| WQHD, idle floor 10 Hz, white | | 1301 mA = 5.50 W |
| WQHD, idle floor 1 Hz, black | | 715 mA = 3.05 W (3 s settle: 3.04 W) |
| WQHD, idle floor 1 Hz, white | | 1289 mA = 5.45 W |
| FHD 120, 10 bpc, black, 140 nits | 120 Hz, 277 µs | 764 mA = 3.26 W |
| FHD 120, 10 bpc, white, 140 nits | 120 Hz | 1350 mA = 5.71 W |
| WQHD 120, black, 2 nits | 120 Hz | 566 mA = **2.42 W** |
| WQHD 120, black, 1000 nits | 120 Hz | 791 mA = **3.37 W** |

## What it says

- **Content is the display's cost.** At 140 nits a white screen costs **2.44 W more than a black one** (5.67 vs 3.23 W); 50 % grey (code 128, ~22 % of white's luminance in a γ2.2 world) costs 1.8 W more than black — 73 % of white's increment, so the panel's current is nowhere near linear in luminance at this brightness. Dark UI is not a style choice on this panel.
- **Brightness moves the panel's floor even on black.** Black at 2 nits: 2.42 W; black at 140 nits: 3.23 W; black at 1000 nits: 3.37 W. The driver IC's own supply follows the DBV (0.8 W between 2 and 140 nits with nothing lit); above 140 nits the floor barely moves and the pixels do the spending. The "2.83 W black" from the first measurement of the day was ABL's brightness, between those two.
- **White at 1000 nits is 8.3 W, HBM 1600 nits is 12.9 W** (3.1 A from the cell, 4.11 V under load). HBM is for sunlight, briefly; the battery says so.
- **10 bpc blend + DQE dither costs nothing measurable** (5.70 vs 5.67 W white, 3.21 vs 3.23 W black). The 1346 vs 1368 Mbps link: nothing measurable either. FHD vs WQHD: nothing measurable (the panel scales; the link carries the same DSC stream rate).
- **Refresh rate is worth 100–200 mW.** 60 Hz saves 100–160 mW on black, 150 mW on white; the auto-rate floors save 120–190 mW (black) and 140–220 mW (white) against 120 Hz, with 10 Hz and 1 Hz no better than each other at this resolution of measurement. Small next to content and brightness, but it is free when nothing moves.
- **The TE pin at the idle floors read 120 Hz in this sweep because the TE was fixed**: `display idle … changeable` switches the panel to changeable TE only for its own measurement (it saw 28 / 8 Hz at the 30 / 10 Hz floors) and puts fixed TE back, as Android runs; the panel keeps idling underneath, which is what the power shows. Not a hardware question.

## Still to measure

- AOD (30 Hz LP, DBV 853) — the state a phone spends most of its life in.
- The SoC's own floor: the same rows with the core in WFI instead of spinning, once the service loop sleeps between PT packets; then the absolute numbers mean something as battery life.
- Android's numbers for the same scenes, for the comparison the design asked for (DISPLAY-DESIGN.md §5): a fixed full-screen pattern, hibernation off, the gauge read from `sysfs`.
- Longer averages (20 s) for the small differences — the 2 s averages here resolve ~50 mW.
