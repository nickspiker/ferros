---
name: pipe-dozenal-keypad-topology
description: "PIPE dozenal keypad: RESOLVED 2026-05-13, no scan wire is grounded; Multiply is single-ended to a GND tab and row 4 keys are ad hoc pin pairs. Mapping + three-phase scanner in /mnt/Harbor/Code/PIPE/KEYPAD.md"
metadata:
  type: project
---

**Original suspicion (2026-05-13 morning):** one of the 9 keypad wires in `spirix_keypad.v` (a clean 4-col x 5-row scanner, LPF `colorlight_5a75b_v8.lpf`) was hardwired to GND on the Colorlight 5A-75B rig.

**Resolved the same afternoon** with a pin_id scanner at 8x speed. The real topology has two oddities:
- **Multiply is wired single-ended to GND:** its terminals are pin 62 (a normal scan GPIO, floating with pull-up) and a ground tab on the PCB. The owner's "a row or column is ground" was this key, not a grounded scan wire.
- **Row 4 is ad hoc:** Divide, Decimal and Negate each have their own pin pair outside the matrix.

Result: a 4x4 matrix for rows 0-3 (rows {27,28,44,51} x cols {49,63,62,56}, 15 keys) + the Multiply-to-GND key + 3 ad hoc keys = 11 GPIO + 1 GND tab, 18 keys. `spirix_keypad.v` fails because it assumed a clean 4x5 matrix.

**How to apply:** use the mapping and the three-phase scanner design in /mnt/Harbor/Code/PIPE/KEYPAD.md; don't re-run a grounded-wire hunt. The keypad drives OLED mode switching and STEP advance in the KOHI → TAHU ceremony.
