---
name: pipe-dozenal-keypad-ground-issue
description: Dozenal keypad has 9 GPIO in spirix_keypad.v scanner but at least one of those physical wires is suspected to be hardwired to GND on the Colorlight 5A-75B board. Scanner will misbehave until identified and fixed.
metadata: 
  node_type: memory
  type: project
  originSessionId: 6adb97ac-ad4e-416a-9091-e8b2900d78dc
---

**The issue**: `/mnt/Harbor/Code/fpga/rtl/spirix_keypad.v` is a 4-col × 5-row matrix scanner that treats all 9 wires (`col[3:0]` and `row[4:0]`) as dynamic GPIO. The LPF (`/mnt/Harbor/Code/fpga/constraints/colorlight_5a75b_v8.lpf`) assigns them to:

```
col[0]=G16  col[1]=H14  col[2]=G15  col[3]=F15
row[0]=D16  row[1]=E15  row[2]=C16  row[3]=B16  row[4]=C15
```

**Original concern** (2026-05-13 morning): user suspected at least one of the 9 wires was hardwired to ground in the physical hardware, based on debugging frustration with the spirix_keypad scanner not working.

**RESOLVED (2026-05-13 afternoon)**: pin_id scanner at 8× speed revealed the actual hardware topology has **two unusual features**:

1. **Multiply key is wired single-ended to GND** — its two terminals are pin 62 (a normal scan GPIO) and a hardwired ground tab on the PCB. All other 17 keys are two-GPIO switches. Pin 62 itself is NOT permanently grounded; it floats with pull-up except when Multiply (or any "+62" key) is held.

2. **Row 4 keys are wired ad-hoc** — Divide, Decimal, Negate each have their own dedicated pin pair that doesn't fit the matrix rectangle.

Resulting topology: 4-row × 4-col matrix for rows 0–3 (rows {27,28,44,51} × cols {49,63,62,56}, 15 keys), plus 1 special Multiply-to-GND key, plus 3 ad-hoc row-4 keys. Total 11 GPIO pins + 1 GND tab = 12 wires, 18 keys.

Full mapping and three-phase scanner design in [/mnt/Harbor/Code/pipe/KEYPAD.md](pipe/KEYPAD.md).

The original "row or column is ground" comment was the user's intuition about the Multiply key's GND terminal — not that a scan GPIO was hardwired LOW. spirix_keypad.v fails because it assumed a clean 4×5 matrix and didn't account for either anomaly.

**Diagnostic to identify which pin**: build a tiny test bitstream that tristates all 9 keypad pins (no drives), enables pull-ups, reads them, and displays the 9-bit state on the OLED. Pull-ups make every floating input read HIGH; a grounded pin reads LOW even with no buttons pressed. That's the broken wire.

Suggested module name: `top_keypad_probe.v` in `pipe/rtl/measure/`. ~30 lines.

**Fix paths once identified**:
- **A (rework cable)** — desolder/reroute the GND trace, replace with a real GPIO data pair from another unused HUB75 pin. Restores full 9-wire matrix. Best long-term.
- **B (matrix shrink)** — accept the loss, redefine the scanner as 3×5=15 or 4×4=16. Doesn't fit 18 keys (Stelor through Negate) — would need to drop 2-3 keys' physical positions.
- **C (per-key remap)** — keep the existing scanner, manually remap key→o_key codes to avoid the broken pin's row/col, lose some keys' shift-alternates.

**Why this matters for IRA bringup**: we want the keypad for OLED mode-switching and STEP-mode advance in the [[pipe-ira-and-trng]] (KOHI → TATARI → TAHU) ceremony. A broken matrix means we can either fall back to the single dev-board `btn` (Option A from the immediate-flash discussion) or fix the hardware first.

**Where this knowledge came from**: user verbally indicated mid-debug ("either a row or column is ground"). NOT verified by reading the PCB or running the diagnostic — it's an in-flight discovery, not confirmed silicon state.

**How to apply**: when the user returns to the keypad work, FIRST suggest running the float-and-read diagnostic to identify which pin reads LOW unbidden, BEFORE wiring the scanner into any IRA top module. Saves a flash + frustration round-trip.
