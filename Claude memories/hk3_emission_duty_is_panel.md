---
name: hk3-emission-duty-is-panel
description: the HK3 holds ~95 % emission duty at DBV ≥ ~1490 (pixel-circuit compensation window, no host knob); below ~1400 the DDIC lengthens the off window (85 % at 1236, <50 % at 983); ferros keeps DBV 3307 and dims in pixels (owner 2026-11-03)
metadata:
  type: project
---

The owner (who sees flicker to well above 1 kHz) sees the HK3 at ~94-97 % emission duty at every brightness, Android too. Tested live 2026-10-23 with `ferros-bridge display dcs`: HBM mode (G#53 E0, DBV G#FFF), dimming bit (G#53 28), 60 Hz NS (G#F2 01 / G#60 18: the off-window grew, so it is per frame), and the HBM-style G#BD early-exit + frame-count table in normal mode — identical notch every time. The driver never names AOR/duty; the DDIC's own mode tables set emission timing.

**Why:** the gap is the pixel circuit's per-frame scan/compensation window, not PWM dimming; there is no host-visible register for it. Full write-up in DISPLAY-CALIB.md "Emission duty".

**How to apply:** treat 100 % duty as unavailable on husky's panel; dimming stays in the present stage (table + dither) as designed. If the question comes up again, point at the write-up rather than re-testing. A G#BD table experiment is safe to send whole (the panel took it) and `bd 21 01 83 03 03 … bd 00 00 00 02 00 06 00 16 00 2e 00 ee / f7 0f` restores the normal one.

**2026-11-03:** the "every brightness" claim was wrong below the knee. Sweep in Android (owner by eye, backlight sysfs): 3307/2000/1490 ~95 %, 1236 ~85 %, 983 <50 %; panel idle made no difference. ferros now floors DBV at 3307 and dims every step in pixels (owner: "keep them all max"; payloads/display brightness()). DISPLAY-CALIB.md "Emission duty".
