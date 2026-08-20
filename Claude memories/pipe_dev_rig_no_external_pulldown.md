---
name: pipe-dev-rig-no-external-pulldown
description: Current PIPE breadboard dev rig has NO external 5kΩ pull-down — just the 400Ω series resistor between bridge and FPGA pad
metadata: 
  node_type: memory
  type: project
  originSessionId: fc72d6be-36d5-40e1-9d05-5d113cb35b2c
---

PROTOCOL.md Layer 0 shows the canonical topology with a 5kΩ pull-down to GND between the two 400Ω series resistors. The current dev rig (FPGA + RP2040 bridge) does NOT have that external 5kΩ — only the 400Ω series resistor between the bridge GPIO and the FPGA pipe_pad.

**Why:** The user has been building/iterating on this rig without the production pull-down. The FPGA's internal weak pull-down (50-100 kΩ via `PULLMODE=DOWN` in the LPF) is the only DC return path when the wire is released.

**How to apply:** When reasoning about wire physics (RC time constants, drive characteristics, idle behavior) on this rig, the effective pull-down is ~50-100 kΩ, NOT 5 kΩ. That makes the RC discharge time constant much larger than the production-spec analysis in PROTOCOL.md predicts. Don't assume the PROTOCOL.md topology when debugging the current breadboard.
