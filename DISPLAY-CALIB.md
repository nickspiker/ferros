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

## 10 bits (2026-10-22)

The panel keeps 8 bits of a 10 bpc stream (DISPLAY.md). In the mode ferros runs (10 bpc blend + DQE dither on the 8 bpc link), with a correctly configured 10-bit layer, a grey ramp steps 0.4 % per 10-bit code (219.4, 220.5, 221.4 nits at codes 512, 513, 514; 223.1 at 516). The tone curve in 10-bit codes (`display-profile.sh run ramps10`, 193 readings):

| DBV | channel | Y at 1023 | γ (codes 64–960) |
|---|---|---|---|
| 1353 | grey | 136.18 | 2.209 |
| 1353 | red | 33.26 | 2.262 |
| 1353 | green | 95.55 | 2.226 |
| 1353 | blue | 7.96 | 2.265 |
| 3307 | grey | 983.33 | 2.191 |

The grey agrees with the 8-bit run (2.189 / 2.191). The channels' spread is the instrument's noise on dim patches: red's worst step is 16 % at code 64, where red emits 0.08 cd/m².

**Near black, measured where there is light to measure.** The i1 Pro's floor is about 0.01–0.02 cd/m², so at DBV 1353 the 10-bit codes under ~24 read noise. DBV is a pure gain, so the bottom codes were measured at DBV 3307 (white 983 cd/m²) and at HBM (white 1467) and compared with white × (c/1023)^2.19:

| 10-bit code | DBV 3307 | against γ 2.19 | HBM | against γ 2.19 |
|---|---|---|---|---|
| 8 | 0.0298 | +24 % | 0.0371 | +5 % |
| 12 | 0.0618 | +7 % | 0.0945 | +9 % |
| 16 | 0.1017 | −7 % | 0.1765 | +9 % |
| 24 | 0.2701 | +2 % | 0.4236 | +7 % |
| 32 | 0.4843 | −3 % | 0.7425 | 0 % |
| 48 | 1.1917 | −2 % | 1.7493 | −3 % |
| 64 | 2.2764 | 0 % | 3.4281 | +1 % |

A plain power law to at least 10-bit code 8 (1/1250 of white), within the instrument's noise there: no toe, no floor. The present stage's table is one power law over the whole range. Codes 2–6 stay below the instrument even at HBM.

**Decided (owner, 2026-10-22):**
- **The present stage.** 16-bit gamma-2 working values (white = Q14) go through one 1D table, shared by the three channels: square, tanh roll-off into the peak, 1/2.19. That gives 10-bit codes with fraction bits, dithered spatially and temporally (phase buffers flipped at 10 Hz, re-pointed rather than copied) to a 12-bit effective target, then packed BGRA1010102 for the hardware's 10 → 8 dither.
- **White is equal-energy E.** ~~Out-of-gamut colours move along the line to E at constant luminance until the lowest channel reaches 0.~~ Superseded 2026-10-24 (below): each channel clamps on its own, bottoms to 0 and tops to the panel's peak, done by the output tables rather than an instruction. No curve.
- **Headroom.** Max white is one stop under the panel's max (about 750 of about 1500 cd/m²). Below it the headroom grows with the panel's room, up to the working format's 16× (4 stops).
- **Blending** happens in gamma 2.

## Method notes

- The panel runs at 120 Hz; the i1 Pro's integration spans many cycles: white at 1353 read 132.82 (default) vs 132.77 (`-Y R:120`, refresh-locked) vs 132.84 (again); grey 128 30.16 vs 30.23. No beat problem.
- Patches need a static surface: the display server with a one-shot client (`shelltest 01`), then the kernel's `display fill`.
- Repeat at a second brightness (2000 or 3307) before trusting the EOTF across the range: Samsung panels shift gamma with DBV (the AOR tables), and the AOD/LP mapping is a different curve altogether.


## Emission duty: the ~95 % notch is the panel's, not ours (2026-10-23)

The owner sees the HK3's emission at roughly 94-97 % duty at every brightness, Android included, never 100 %. Tested live with the owner as the detector (`ferros-bridge display dcs`), each variant sent whole with the level-1 keys: HBM mode at DBV G#FFF (G#53 = G#E0) — identical; normal mode with the dimming bit (G#53 = G#28) — identical; 60 Hz NS op mode (G#F2 01, G#60 18) — the off-window grew, so the notch is per frame; the HBM-style G#BD early-exit and frame-count table in normal mode (`BD 21 00 83 03 01`, the halved counts at G#BD offset 1E) — the panel took it, picture fine, notch unchanged. The driver (panel-google-hk3.c) sends nothing that names AOR, duty or PWM; the DDIC derives the emission timing from its own tables per mode. Conclusion: the gap is the pixel circuit's per-frame off-window (the scan), under the DDIC alone; no host-visible register moves it, and 100 % is not on this panel. Brightness is already set by the present stage (the table, the dither) wherever the panel's own DBV is not needed; the notch is not a dimming mechanism we could take over.


## The ODT: VSF RGB → the panel (2026-10-24)

Until today the present stage had no gamut matrix: VSF RGB went straight to the panel's primaries, with only per-channel gains putting the white at E. Greys were right (the gains are the white), colour was close but off on the red-green axis (greens leaned yellow, reds orange); blue was already right.

**The matrix** (`tools/panel-white-e.py`, the DBV 3307 spectra integrated against VSF RGB's own colour-matching functions — Stockman & Sharpe 10° through vsf's LMS2VSF_RGB, no other space in between): rows panel R, G, B; columns VSF R, G, B; absolute (E in is E on the glass), so its row sums are E's gains.

```
+1.3596  −0.3725  +0.0129   sum 1.0000
−0.0984  +0.8640  −0.0261   sum 0.7394
+0.0018  −0.0011  +0.8165   sum 0.8172
```

Over the gains it is identity on blue and a 2×2 on red-green (panel red takes −37 % of VSF green, panel green −13 % of VSF red): that block is the whole colour correction. Every sRGB colour lands inside the panel (smallest channel 0.007, none over 1), so only colour beyond sRGB is ever out of gamut. Model tier, from one unit.

**Decided (owner, 2026-10-24):**
- Out of gamut: each channel on its own, bottoms to 0, tops to the panel's peak. No line to E (the earlier rule), no division.
- One path for every pixel, branchless: no grey test, no scalar fallback. The matrix is written as gains + K, `p = g∘l + K_R·(l_R − l_G) + K_B·(l_B − l_G)` with K = ODT − diag(g) (rows sum to zero), so a grey's differences are exactly 0 and greys come out as g∘l with no special case.
- f16 for the per-pixel maths (8 lanes), the panel's 1/2.19 from the f16's own bits: the exponent picks the code at its octave's foot, the top six mantissa bits a nudge across the octave (interpolated on the low four). The clamp is the tables: a negative light's sign bit indexes past the 32-entry exponent table, which TBL answers with 0; exponents past the peak saturate the dither.

**The pipe** (payloads/display/src/neon.rs `Odt`), per 16 pixels:
1. light: the 8-bit canvas as u = c²/65536 (an exact u8 multiply and one fixed-point convert) into a degree-4 polynomial fitted to tanh²(k·v) at each brightness; an app's 16-bit working values (to 16× white) as tanh(k·v) in f32 by Lambert's continued fraction to 7th order, squared, narrowed to f16;
2. the ODT as gains + K (f16, 11 instructions per 8 pixels);
3. the output code from the f16's bits (TBL, one widening multiply, a rounding narrow);
4. the dither (saturating add, saturating narrowing shift) and the pack: the 10-10-10-2 word's four bytes as four planes, one `st4`.

Checked against f64 on the phone: canvas greys within 6 Q4 (a tenth of an 8-bit step), canvas colour within 21 Q4 (only channels the matrix cancels to near zero, a negligible amount of light next to the pixel's others); the app path within 1 8-bit code (truncation).

**Measured on husky with the PMU** (ferros_kernel/src/pmu.rs, opened to EL0 the same day): canvas 86 cycles and 199 instructions per 16 pixels on the X3 (1.9 ns a pixel at 2.85 GHz), 126 cycles on an A715 (7.4 ns at its 1.07 GHz); the app path 157 cycles on the X3. A full-frame publish, split in bands weighted by those rates (X3 57 %, each A715 14 %): 5.2 ms (the grey-table path alone was 9 ms for a mostly black map, which it skipped). Along the way: the built-in target's strict-align had LLVM turn every NEON load and store into scalar pieces through the stack; processes now build with `cargo el0` (.cargo/config.toml), without it.

Timed and set aside: the scalar f32 ODT (9 ns a pixel on the X3), the scalar integer one (Q30 light, Q28 matrix, `clz` output; 9–10 ns, and within 1 Q4 of f32 only with Q28 — Q16 lost up to 5 codes where channels cancel), plain f16 (fast but up to 18 codes off before gains + K).
