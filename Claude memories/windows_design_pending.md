---
name: windows_design_pending
description: WINDOWS.md — owner picked full-screen windows, a full-screen buffer per app drawn frontmost first, apps inside a circle tangent to the screen's top and bottom (corners = system); built 2026-10-27 (4 surfaces, generation bytes as death notices, op 22/23/26/27 with the surface base)
metadata:
  type: project
---

The owner asked to sort the window manager (2026-10-27: "a glitchy hot mess"). WINDOWS.md diagnoses it and proposes a four-layer stack (map, front app, keyboard, attest) with kernel badges, death notices and per-app surface regions, touch routed through the display, and software span composing. It lists seven knobs with recommended picks.

**Why:** the display server's code shape depends on those picks, and display work is design-first ([[display_driver_design_first]]).

**Built 2026-10-27:** see WINDOWS.md "Built". Owner's picks: full screen for ownership and layering, an app gets a full-screen buffer to do as it wishes and the display draws them in order frontmost first; an app in front is a full-screen circle tangent to the top and bottom, the four corners for clock, signal, navigation. Open: how widgets fit; touch routing through the display; navigation in the corners; badges once memory isolation exists.

**How to apply:** new apps spawn with `--dma surface` and use ferros_sys::window (show/done/text/mine); keep content inside the circle (honeycomb::corner_rows); check WINDOWS.md before changing the display's ops. Related: [[input_rhe_on_glass]] (the keyboard becomes layer 2).
