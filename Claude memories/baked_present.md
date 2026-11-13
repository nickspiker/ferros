---
name: baked-present
description: Since 2026-11-12 the display's canvas and BAKED (format 2) surfaces hold the panel's own codes (roll-off, ODT, 1/2.19 applied by producers); the present is dither + pack only; blue-noise 16x16 dither with low-bit XOR per phase
metadata:
  type: project
---

The owner (2026-11-12): "if the GPU touches it last and we precompute the tanh highlight colours for all elements then it's a straight thru render and a sqrt to get the gamma close". Built on ferros branch `sqrt-present` (payloads/display bake.rs, neon.rs `present16`; COMPOSITOR.md "Baked"), fluor branch `baked` (host/panel.rs `Bake`, host/ferros op 65 + EV_BAKE), Mahere branch `ferros-baked` (GPU kernel ends with the bake).

**Why:** the per-pixel present (tanh, ODT, pow, dither, pack) was ~9.5 cycles/px and capped a full-screen GPU app near 75 fps on the Pro.

**How to apply:**
- A baked channel = panel code p^(1/2.19) × 65472 (Q16; `b >> 8` the 8-bit code), p = ODT · tanh²(k·v); compositing stays front-to-back `under` in these codes (root space after the roll-off). The owner's sqrt is folded into the bake: the canvas always held roots, so there is no root in the present (linear-light glows looked worse, 2026-10-24). If the owner asks for the root step back, the bench measures `present_sqrt16` (fsqrt / rsqrte+Newton).
- Colours bake once per draw op (`draw::opaque`, cover per pixel with `draw::cover`); images once (icons from the bundle; the wallpaper baked in place, no copy of its codes: re-baked at the change by unbake with `bake::before()` (root table read linearly) then bake, and from its source in the vault once the brightness settles, `wall_refresh`, exact; the old unbake banded it on the 8a, 2026-11-12); brightness change = `rebake` + `EV_BAKE` to windows. Measured on the 8a: present 9.45 -> 1.17 cycles/px (X3), rebake 6.7 ms. The display's first thread has a 64 KiB stack: keep big tables off it. Never write working values into the canvas.
- Formats: 0 working, 1 G2T16 (display bakes per frame), 2 BAKED (presented as is). Op 65 answers the bake (ferros_sys `window::bake`).
- Dither: neon.rs `BLUE` (void-and-cluster ranks) XOR `TURN` [0,2,3,1] on the low 2 bits per phase; host-measured shimmer 2.5x lower than the old Bayer+rank. The ramps (op 21) keep the old dither.
- Capture op 30 unbakes into working values; `[30, 2]` gives raw codes for look-checks.
Related: [[fluor-pixel-depth]], [[colourspace-pinned-vsf-gamma2tanh]], [[hk3-emission-duty-is-panel]].
