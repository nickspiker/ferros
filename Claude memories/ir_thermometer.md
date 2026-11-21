---
name: ir-thermometer
description: "Owner 2026-11-20 - the Pro's MLX90632 IR thermometer goes into a `thermo` mailbox + thermometer puck; the owner has blackbodies at known temperatures for calibration later; asked-vs-implicit right still undecided"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-21T03:06:55.331Z
---

The Pro's non-contact IR thermometer (Melexis MLX90632, AoC i2c0 G#3B) is driven by Google's AoC firmware; ferros reads it over USF (types 56 normal, 67 extended), first streamed by hand 2026-10-24 (docs/SENSORS.md).

2026-11-20 the owner said "Sure" to: the sensors process publishing it in a `thermo` mailbox (as `motion`), and a thermometer puck. Staged as branch `thermo`.

**Owner has blackbodies with known temperatures** for calibration: "we can get to that". When calibration comes up, ask him to run them; don't invent a calibration from the factory numbers alone.

**Not decided:** whether temperature is a right asked once or implicit. "Sure" answered only "want the mailbox and puck?" ([[owner_decisions_2026_11_19]]: a yes answers only its question). Built public like motion until he decides.

Related: [[sensor_mailboxes]], [[rights_implicit_set]], [[aoc_start_needs_trusty]].
