---
name: ir-thermometer
description: Owner 2026-11-20 - the Pro's MLX90632 IR thermometer goes into a `thermo` mailbox + thermometer puck, open to every kist with no permission; the owner has blackbodies at known temperatures for calibration later
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-21T03:06:55.331Z
---

The Pro's non-contact IR thermometer (Melexis MLX90632, AoC i2c0 G#3B) is driven by Google's AoC firmware; ferros reads it over USF (types 56 normal, 67 extended), first streamed by hand 2026-10-24 (docs/SENSORS.md).

2026-11-20 the owner said "Sure" to: the sensors process publishing it in a `thermo` mailbox (as `motion`), and a thermometer puck. Staged as branch `thermo`.

**Owner has blackbodies with known temperatures** for calibration: "we can get to that". When calibration comes up, ask him to run them; don't invent a calibration from the factory numbers alone.

**Open, no permission (owner 2026-11-20):** "open, no permission necessary". The view opens with no proof; it is in the implicit set ([[rights_implicit_set]]).

Related: [[sensor_mailboxes]], [[rights_implicit_set]], [[aoc_start_needs_trusty]].
