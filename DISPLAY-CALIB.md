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

## Method notes

- The panel runs at 120 Hz; the i1 Pro's integration spans many cycles: white at 1353 read 132.82 (default) vs 132.77 (`-Y R:120`, refresh-locked) vs 132.84 (again); grey 128 30.16 vs 30.23. No beat problem.
- Patches need a static surface: the display server with a one-shot client (`shelltest 01`), then the kernel's `display fill`.
- Repeat at a second brightness (2000 or 3307) before trusting the EOTF across the range: Samsung panels shift gamma with DBV (the AOR tables), and the AOD/LP mapping is a different curve altogether.
