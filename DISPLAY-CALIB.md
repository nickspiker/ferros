# DISPLAY-CALIB — the husky panel, measured

The google-hk3 panel measured with the owner's i1 Pro (calibrated spectrometer) through ArgyllCMS `spotread`, patches driven through ferros (`tools/display-calib.sh`; the readings are `oracle/display/husky-hk3/calib-20261017-1018.csv`). Raw 8-bit codes on the 8 bpc DSC link, DECON at 8 bpc with no DQE: this is the **panel's native response**, nothing of Android's colour management in front of it. Dark room, instrument face-down on the glass, centre of the panel, 2026-10-17.

## Luminance per DBV, full white

| DBV | measured | the driver's table |
|---|---|---|
| 196 | 1.93 nits | 2 |
| 400 | 9.24 | |
| 853 | 49.6 | (the AOD DBV) |
| 1353 | **133.0** | 140 (−5 %) |
| 2000 | 316 | |
| 3307 | 986 | 1000 (−1.4 %) |
| 4095 HBM | 1488 | 1600 (−7 %) |

The driver's nits table holds to within ~5 %; the top end reads 7 % under its nominal, which is the panel (temperature, age) or the instrument's absolute scale, not something ferros does. White drifts slightly cooler with brightness (y 0.329 → 0.323).

## The EOTF at 140 nits (DBV 1353)

A 17-step grey ramp, code 0 → 255. Black is 0.000 (OLED). Per-code gamma exponent (Y/W = (c/255)^γ): 2.17 at code 16, 2.19 at 48–80, 2.14 at 128–160, 2.08 at 240; least squares over codes 16–240: **γ = 2.185**. Against the sRGB EOTF the panel is **−53 % at code 16, −25 % at 32, −13 % at 48**, within ±6 % from code 80 up: the panel is a plain power law with **no linear toe** — deeper shadows than sRGB, the look Samsung tunes for. It is not gamma 2.0 either (VSF's), so a VSF → panel transfer is needed per channel: L = v² (VSF) → c = L^(1/2.185) (panel), one 256-entry (or 1024 at 10 bpc) table, folded into the gamma2tanh render.

## Primaries and white

| | x | y | Y at full code (nits) |
|---|---|---|---|
| red | 0.6800 | 0.3199 | 32.9 |
| green | 0.2510 | 0.7066 | 95.3 |
| blue | 0.1401 | 0.0444 | 7.87 |
| white | 0.3092 | 0.3239 | 133.0 |

Wider than DCI-P3 (P3: G 0.265/0.690, B 0.150/0.060 — this green and blue sit further out), red on P3's. White is **~6750 K**, a little below the D65 locus (0.3127, 0.3290) toward green-blue; it is not Illuminant E (⅓, ⅓), VSF RGB's white. Additivity: R + G + B = 136.0 nits against white 133.0, **+2.3 %** — a small panel non-additivity (current limiting under full white), fine for a profile.

RGB → XYZ at full code, cd/m² (1931 observer, from the xy and Y above):

```
X   69.92   33.84   24.83
Y   32.89   95.27    7.87
Z    0.01    5.71  144.53
```

## What it means for the pipeline (DISPLAY-DESIGN.md §6)

- **Transfer:** render in gamma2tanh (gamma 2 + roll-off), then the per-channel VSF→panel table above. Dither at 10 bpc keeps the shadows clean where 2.185 compresses them.
- **White:** two defensible choices. (a) Display white = panel white: VSF's equal-energy white renders as the panel's native 6750 K, full luminance, the usual "media white = display white" practice. (b) Adapt to E: scale channels so equal VSF RGB measures (⅓, ⅓) on the glass — costs ~15 % luminance and the panel's white never looks "white" next to anything else. Recommended: (a), stated in the profile so the maths knows.
- **Gamut:** a 3×3 from VSF RGB's monochromatic primaries (703 / 523 / 462 nm) to the panel's, evaluated under the observer VSF uses (Stockman–Sharpe 10°); the i1 Pro can save spectra (`spotread -O file.sp`) so the panel primaries can be placed under the same observer rather than through 1931 xy. Out-of-gamut handling is gamma2tanh's per-channel roll-off.
- **Brightness:** DBV 1353 is 133 nits on this unit; the shell's brightness policy works in nits via the measured table, not in DBV.

## The full profile (2026-10-22)

Measured with the same i1 Pro through `tools/display-profile.sh`: 227 readings, every one spectral (380–730 nm, 10 nm), the raw spotread output kept beside the CSV (`oracle/display/husky-hk3/profile-20261022/`: `readings.csv`, `spotread.log`, `report.md` from `tools/display-profile-report.py`). The shell was off for the session (vault `home.off`), so only the kernel's measurement patch reached the panel: a disc 1 cm across (193 px at 489 ppi, checked with a ruler) at the centre, the instrument on it throughout. Same unit, same 8 bpc path as above, ABL's panel state.

**No limiter.** A full white screen gets what a 1 cm spot gets, at every brightness:

| DBV | 1 cm | 2 cm | 4 cm | full | full ÷ 1 cm |
|---|---|---|---|---|---|
| 196 | 1.9 | 1.9 | 1.9 | 1.9 | 1.003 |
| 400 | 9.2 | 9.2 | 9.1 | 9.2 | 0.997 |
| 853 | 49.9 | 49.8 | 49.6 | 49.3 | 0.986 |
| 1353 | 136.2 | 136.1 | 135.8 | 132.5 | 0.973 |
| 2000 | 324.6 | 324.9 | 325.0 | 318.2 | 0.980 |
| 2600 | 588.5 | 590.4 | 592.8 | 580.8 | 0.987 |
| 3307 | 984.2 | 989.1 | 1002.0 | 991.2 | 1.007 |
| HBM | 1471.9 | 1479.3 | 1500.2 | 1517.1 | 1.031 |

- **Full field:** at most 3 % lower than the window, at mid levels. This is the small sag the 2026-10-17 additivity reading saw (R+G+B 2.3 % over white, measured full field). In the window, R+G+B matches white to +0.1 % at DBV 1353 and −0.3 % at 3307. At HBM the larger areas read higher, which follows the order of the readings (the panel warming), not the area.
- **Held white:** 30 s of full white loses 0.2 % at DBV 3307 (989 → 987) and 0.9 % at HBM (1511 → 1498). That is a slow thermal drift, not a step.
- **Average picture level:** the 1 cm white disc on a surround from black to full white varies within ±1 % at DBV 3307 and within 3 % at HBM.

Neither the panel nor anything left on from ABL limits brightness by area, level or time in this mode. Any HDR roll-off is ours to design.

**Gamma does not move with brightness.**

| DBV | channel | Y at 255 | γ (16–240) | worst step off the fit |
|---|---|---|---|---|
| 1353 | grey | 137.75 | 2.189 | 8.6 % at 16 |
| 1353 | red | 33.46 | 2.206 | 3.4 % at 64 |
| 1353 | green | 96.20 | 2.205 | 2.5 % at 64 |
| 1353 | blue | 7.92 | 2.247 | 9.6 % at 32 |
| 3307 | grey | 988.27 | 2.191 | 2.9 % at 176 |
| 3307 | red | 233.56 | 2.175 | 4.6 % at 176 |
| 3307 | green | 697.57 | 2.196 | 2.5 % at 208 |
| 3307 | blue | 54.17 | 2.166 | 7.0 % at 144 |

Grey fits 2.19 at both 133 and 1000 nits, confirming 2.185. The channels agree within about 0.03, except blue at 1353: it peaks at 8 nits there, where the instrument's noise dominates.

**DBV is a pure gain.** Brightness scales the whole curve without bending it, so one transfer table serves every brightness. The panel's code range is then the only budget the headroom spends: with DBV raised so that the peak is HBM's ~1500 nits, a 133-nit white sits at code (133/1500)^(1/2.19) × 255 ≈ 84 of 255 at 8 bpc, or ≈ 337 of 1023 at 10 bpc. HDR needs the 10 bpc path (and its dither) before it needs anything else.

**Near black (DBV 1353):** codes 1–3 read a flat ~0.013 cd/m², which is the instrument's floor, not the panel. Codes 4–12 read 6–15 % above the 2.185 line.

**Colours:** the primaries match 2026-10-17 to the fourth decimal and barely move with brightness (red x 0.6800 → 0.6792 from DBV 1353 to 3307). White at 1353 reads x 0.3101, y 0.3262 (window) and turns slightly greener at 3307 (y 0.3344), while grey 128 at 3307 stays at y 0.3259. Secondaries, the 50 % grey and every patch's spectrum are in the run, for building the panel's primaries under VSF's observer (Stockman–Sharpe 10°) rather than through 1931 xy.

**Drift:** white at 1353 read 136.02 at the start and 137.31 after 37 minutes (+0.95 %).

## Method notes

- The panel runs at 120 Hz; the i1 Pro's integration spans many cycles: white at 1353 read 132.82 (default) vs 132.77 (`-Y R:120`, refresh-locked) vs 132.84 (again); grey 128 30.16 vs 30.23. No beat problem.
- Patches need a static surface: the display server with a one-shot client (`shelltest 01`), then the kernel's `display fill`.
- Repeat at a second brightness (2000 or 3307) before trusting the EOTF across the range: Samsung panels shift gamma with DBV (the AOR tables), and the AOD/LP mapping is a different curve altogether.
