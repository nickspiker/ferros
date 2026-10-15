---
name: display_driver_design_first
description: When the husky display driver work starts, first deliver a knobs inventory + ideal design for the owner to review, before any code
metadata:
  type: feedback
---

Owner (2026-10-15): "when we get to the display driver I'd like to see what knobs we have and the ideal design."

**Why:** the display is the owner's UI surface (Ready? gate, checklist, later fluor/lumis/Photon UI, the prox-gated wake policy); they want to shape it before it's built, the way they shaped the boot UI.

**How to apply:** first artifact = everything DECON v7.6 / DPP / DSIM + D-PHY / DSC / the Samsung S6E3HC3 panel (DCS commands, brightness, refresh modes, partial update, TE, HBM, power states) lets us set, from the cal_9865 + panel sources, then a proposed design (display as a process with grants + a DMA framebuffer; kernel keeps only the pre-process painter). Status today in DISPLAY.md "Status". Related: [[ferros_wake_policy_prox_gate]].
