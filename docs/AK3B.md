# AK3B: akita's panel, knobs and plan

akita (Pixel 8a) has the google-ak3b panel; husky's is google-hk3 (DISPLAY-HK3.md).
This is the inventory of what AK3B offers, against HK3, and the plan for 120 Hz and calibration, for the owner to decide on before any code (owner rule: design first).
Source: Google's driver `panel-google-ak3b.c`, kernel/devices/google/akita branch android-gs-akita-android16, cloned to `/mnt/Harbor/ferros-ref/kdev-akita` (2026-11-06); akita's factory device trees in `/mnt/Harbor/tmp/akita-factory/dt/`.
All DCS bytes below are as the driver sends them.

## What the panel is

| | AK3B (akita) | HK3 (husky) |
|---|---|---|
| Raster | 1080 × 2400, 70 × 149 mm | 1344 × 2992 (and FHD by modeset) |
| Mode | command mode, TE-triggered | command mode, TE-triggered |
| Link | DSI 4 lanes, DSC 1.1, 2 slices of 540 × 48, 8 bpc, 8 bpp | DSI, DSC |
| TE pin | gpp0-3 (the SoC's tree) | gpp0-3 |
| Rails, in order | VDDI, VCI, VDDD (+10 ms); reset 1 / 1 / 20 ms | its own (DISPLAY-HK3.md) |
| Panel revision | read from module IDs; several sequences differ before EVT1 (this unit is MP) | by revision too |

## Knobs

0. **Refresh rate**: three operating points and an AOD rate, no free VRR:
   - HS120: `60 00 00`, TE select `B9 31`;
   - HS60 (60 Hz on the 120 Hz timing): `60 08 00`, `B9 30`;
   - NS60 (60 Hz "normal speed", the low-power 60): `60 18 00`, `B9 30`;
   - each wrapped in the test key `F0 5A 5A` … `F7 0F` (frequency update) … `F0 A5 A5`.
   - TE per mode: 221 µs at 120, 8.5 ms at 60. HK3 has a wider set (1-120 Hz with idle floors, measured from ferros); AK3B's driver offers only 60 / 120 / 30 (AOD).
1. **Brightness**: DBV through `51` (12 bits, big-endian):
   - normal: 209-3175 = 2-800 nits;
   - HBM: 3176-4095 = 800-1400 nits;
   - default 1023.
2. **Display control** `53` (WRCTRLD): BCTRL G#20 always; dimming G#08; HBM G#C0; local HBM G#10.
3. **Pulse settings** (`B0`-addressed writes to `F2` and `BD`, per revision and mode), sent with every WRCTRLD change. Likely the emission/PWM control. HK3's emission duty turned out to be the pixel circuit's own above DBV ~1490 (memory: HK3 emission duty), and this is the knob to measure on AK3B for the same question.
4. **AOD**:
   - 30 Hz LP mode;
   - `53 25` (10 nit) or `53 24` (50 nit);
   - exit `53 20` and back to HS60 first;
   - "AOD without blink" `B0 00 03 BB` / `BB 01 0C` at every enable.
5. **TE2** (a second sync output, `F2 03 14` on, per-mode edges via `CB`): for the fingerprint sensor's timing. Not needed by ferros.
6. **Local HBM** (the fingerprint circle: a box at x G#21C, y G#6B9, size G#63, gamma read from the panel, brightness overdrive groups): biometrics are never on ferros (memory: biometrics move with identity), so none of it.
7. **GPO_DC** at init (`B0 00 1F CB`, `CB FF`, then 110 ms): undocumented, as Google sends it on EVT1+.
8. **HDR metadata**: 1400 nits peak, 1200 average, 0.0005 minimum. Information only: ferros's pipeline is its own (VSF gamma2tanh).

What AK3B does not offer that HK3 does: free VRR and idle floors below 30 Hz, and the FHD/WQHD modeset.

## Found on the device (2026-11-06)

- ABL leaves AK3B at 60 Hz with its splash at G#FAC0_0000 (outside the DPU fence; ferros draws into its own framebuffer on akita).
- The panel does not hold one sent frame: at the gate, one triggered frame showed for about 18 of 60 frames, and DECON triggering on TE for the whole gate fixed it (ready.rs). That is unexpected for a command-mode panel with GRAM, and could be the pulse/GPO settings ABL leaves or the TE mode. It should be measured before the display server relies on single-frame presents on akita.

## On akita, 2026-11-08: the display server at ABL's mode

Plan step 0's part that needs no AK3B command is built (payloads/display; docs/BOARDS.md "The shell on akita"):
- ABL's link, raster, DSC and 60 Hz are kept, and nothing is sent to the panel: no HK3 command (refresh, idle floor, TE mode, brightness, WRCTRLD, TE2, panel off). The brightness is ABL's.
- The modeset takes DECON to the 10 bpc blend (the DQE's dither onto the same 8 bpc link, as on husky), with akita's raster (`ferros_dsc::AKITA`, from Google's `pps_config`), only after checking every raster word it writes against ABL's running pipe; any mismatch leaves the pipe untouched. The first run found one: each of the two DSC encoders holds the PPS of its own half (pic_width 540, initial_dec_delay 436), not the panel's (1080, 526), so on akita the encoders keep ABL's PPS words. Every other word matched.
- DECON stays triggered on every TE (the single-frame question above is still open): 60 frame starts a second while the shell is up, a frame every refresh. The kernel's clock policy counts that as animating, so akita stays in the active profile while the display runs: a cost until the question is answered.
- A DPU or panel left off, or a jump from Android, is refused: AK3B's power-up (rails VDDI, VCI, VDDD, reset, its init set) is not written.
- The glass radius (112.5) and the cutout (bottom row 102.43) are akita's (BOARDS.md).

- **A display restart on akita breaks the picture** (2026-11-08, unmeasured): after `diag "restart display"` (three in a row, each re-running the modeset on its own running pipe and re-sending DBV 3175) the frame was right in a capture, but the glass showed it under a gradient, bright at the top and black at the bottom (the space key unreadable), with a thin-duty 60 Hz flicker; a reboot cleared it. Until the restart path is measured (the panel's power mode and DBV read back, the TE trigger, the pulse settings), deploy a new display to akita by `store put` and a reboot, never a restart.

Still the owner's: the three questions below, and a look at the glass (the picture under the 10 bpc blend has been seen only by the logs).

## Refresh rate and power, measured (2026-11-08)

Display op 7 on akita (`[7, fps, rate byte]`) sends Google's ak3b_change_frequency sequence with any rate byte and sets DSIM's TE timers for fps. HS120 (`60 00 00`, `B9 31`) runs: the server logs 120 frame starts a second (DECON still triggered on every TE). HS60 (`60 08 00`) and NS60 (`60 18 00`) both come back to 60. A reboot returns to ABL's 60.

Whole-device draw from the fuel gauge (`diag power`, 16 s windows, charger cut, the bind screen at rest, mostly black, DBV 3175, clocks as the policy left them); the PMIC rail meters do not work on akita (every rail resets under the read):

| rate | windows | mean |
|---|---|---|
| HS60 | 1600, 1641, 1586, 1694 mW | 1630 mW |
| HS120 | 1842, 1789, 1814, 1771, 1837 mW | 1811 mW |
| NS60 | 1608 mW | |
| ABL's 60 at boot | 1640, 1790 mW | |

- **120 Hz at rest costs about 180 mW over 60** (the paired windows 143-185 mW apart; about 11 % of the device at rest). Window-to-window noise is about ±50 mW, so NS60 against HS60 is not resolved.
- Part of it is the link: akita's panel does not hold a sent frame, so DECON sends the whole frame every TE, 120 frames a second over DSI at 120 even when nothing changes. If the panel can be made to refresh from its own memory (the single-frame question above), 120 at rest would cost the panel's scanning only.
- Measured on a dark screen. A bright one may move the gap (the emission's duty and pulses per frame per mode are unmeasured).

## Profiled with the i1 Pro (2026-11-08 night)

`tools/akita-profile.sh run`, unattended (the owner calibrated the i1 on its tile and seated it on the 1 cm disc): 234 spectral readings, raw in `oracle/display/akita-ak3b/profile-0/` (readings.csv, spotread.log with every spectrum). Patches are the display server's op 52 mode 8 (raw 10-bit codes past the present table, 8 bits on the link), centred, radius 84 px (1 cm at 430 ppi).

**Emission duty:** full down to about DBV 1280 by the owner's photodiode (BPW34 on line in), degrading below. On the measured curve DBV 1280 is **99.7 cd/m²**: the knee is 100 nits of white. husky's (DBV 1219-1246) is about 108-114 on its coarser table. Both Samsung driver chips appear to switch at a luminance, not a register value.

**Luminance per DBV, full white in the disc:**

| DBV | cd/m² | | DBV | cd/m² |
|---|---|---|---|---|
| 209 | 1.86 | | 1500 | 141.8 |
| 400 | 8.12 | | 2000 | 273.7 |
| 853 | 42.9 | | 2600 | 501.0 |
| 1000 | 60.9 | | 3175 | 802.1 (normal mode's top) |
| 1200 | 89.2 | | 3176 HBM | 797.7 |
| 1280 | 99.7 | | 3600 HBM | 1072 |
| 1353 | 112.2 | | 4095 HBM | 1417 |

- Normal mode is a power law, exponent 2.23 end to end (Google's 2 at 209 and 800 at 3175 both hold); the middle runs a few percent under the line (2000: 274 against 290).
- High brightness mode is linear: 0.674 cd/m² a DBV step from 798 at 3176 to 1417 at 4095 (Google: 1400).

**Tone curve at DBV 3175** (code 32 to 224, 8-bit codes): grey 2.220-2.236, flat; green 2.21-2.25; red 2.19 falling to 2.13 at the top; blue 2.20 to 2.16. The present stage now uses 2.23 on akita (`odt::panel_gamma`), 2.19 on husky.

**Near black** (HBM, 1417 white): grey 1 0.030 cd/m², 2 0.05, 3 0.10, 4 0.16, 8 0.706 (the power law says 0.65), 16 3.13. Black (grey 0 at 3175) reads 0.003, the instrument's floor.

**Colours** (xy, the 1931 observer as spotread reports; the spectra are kept for VSF's): red (0.6803, 0.3194), green (0.2401, 0.7128), blue (0.1396, 0.0448); white (0.3073, 0.3257) at 3175, (0.3089, 0.3258) in HBM; grey 128 (0.3071, 0.3228). R + G + B is 8-9 % more light than white at both brightnesses (877 against 807; 1533 against 1421): white is limited. E's white solved from these spectra under Stockman-Sharpe 10° (tools/panel-white, DBV 3175): gains R 1, G 0.7532, B 0.7965 (husky 0.7394, 0.8172), Q15 code multipliers 32768, 28858, 29589 at γ 2.23, E keeping 81.1 % of native white's light; the ODT [[1.3065, −0.3222, 0.0157], [−0.0987, 0.8829, −0.0309], [0.0018, −0.0024, 0.7971]]. The display uses them on akita (odt::odt, e_code_q15, e_light).

**Window against field:** at 3175 the disc 806, a 2 cm disc 808, 4 cm 809, the whole panel 795 (−1.4 %); in HBM 1418, 1423, 1424, the whole panel 1374 (−3.1 %), the whole field's white also bluer-green shifted (y 0.320 against 0.326).

**Lost:** the 8a dropped off USB right after the full-field white in HBM (the most current the panel draws) and kept showing it; every later section (APL, sustain, the closing drift) read that full field, not its patch, and is void. USB did not come back with a host port reset. Unknown yet whether it is the charger path, the USB switch or the kernel; to look at with the device in hand (the kill chord or a long power press clears the screen).

## Proposed plan

0. **The display server on akita**:
   - Today akita runs no init and no display server: they assume HK3's geometry, rails and touch.
   - Board-parameterise the server: raster, cutout and glass radius (ferros_sys still has husky's constants), and the panel ops (refresh, brightness, AOD), as one table per panel, AK3B's taken from the driver above.
   - Keep ABL's link (lanes, PLL, DSC) as on husky, where ABL's DSIM matched Android's.
1. **120 Hz**:
   - the HS120 sequence above from the server's refresh op;
   - measured on gpp0-3 with `display terate`, as HK3's W120 was;
   - the server keeps triggering every TE on akita until the single-frame question above is answered.
2. **Brightness policy**:
   - Measure first, as on HK3: emission duty and PWM against DBV (the `F2`/`BD` pulse settings), with the photodiode/i1 Pro.
   - If AK3B, like HK3, has a DBV above which the duty is the panel's own, ferros keeps DBV there and dims in pixels.
3. **Calibration**:
   - The pinned profile (VSF gamma2tanh in root space) needs AK3B's measured response: the same DISPLAY-CALIB.md sweep, per panel unit, stored on the device.
   - akita's sweep is the first time two different panels share the pipeline, so the per-panel correction becomes data, not code.

## For the owner

- 120 Hz always while anything moves and NS60 at rest (as husky runs active/rest), or something else?
- The brightness policy waits on the measurement in step 2: agree to measure before choosing?
- Order: display server on akita first (needed for everything else), then 120 Hz, then the measurements?
