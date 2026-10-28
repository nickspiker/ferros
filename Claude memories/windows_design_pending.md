---
name: windows_design_pending
description: WINDOWS.md (2026-10-27) is the window manager proposal awaiting the owner's picks; the present app path's glitches all trace to one APP slot, one shared app-surface, no caller identity, no death notices, unrouted touches
metadata:
  type: project
---

The owner asked to sort the window manager (2026-10-27: "a glitchy hot mess"). WINDOWS.md diagnoses it and proposes a four-layer stack (map, front app, keyboard, attest) with kernel badges, death notices and per-app surface regions, touch routed through the display, and software span composing. It lists seven knobs with recommended picks.

**Why:** the display server's code shape depends on those picks, and display work is design-first ([[display_driver_design_first]]).

**How to apply:** don't build steps 2 onward of WINDOWS.md's order of work until the owner has picked the knobs; step 1 (map frames off the glass under an app, text at op 26) is done. Related: [[input_rhe_on_glass]] (the keyboard becomes layer 2).
