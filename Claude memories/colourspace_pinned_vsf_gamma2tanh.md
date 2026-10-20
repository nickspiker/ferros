---
name: colourspace-pinned-vsf-gamma2tanh
description: The owner pinned the colour pipeline across projects (2026-10-17): VSF RGB, roll-off tanh in root (gamma-2) space as chosen on the husky panel; Mahere's imagery adopts the same profile once ferros settles it
metadata:
  type: project
---

The display pipeline decided on the glass (ferros DISPLAY-DESIGN.md §6, 2026-10-17): decode L = atanh(v)², 3×3 VSF RGB → panel primaries (E → panel white), exposure 1/H, roll-off Y/peak = tanh²(√(L/H)) (tanh applied in root space, white at tanh²(1/√H)), panel code = Y^(1/2.185), DQE dither. The owner (2026-10-17): "make sure we pin the colourspace. once we get it sorted in ferros we'll be applying the same profile to all the Mahere imagery."

**Why:** one curve for everything the owner's projects display or store; Mahere today stores imagery as √(tanh(x)) (tanh in linear light, then gamma 2) and displays through Opsin's cubic rail — a sibling, not the same curve.

**How to apply:** treat ferros's panel pipeline as the reference profile; when it is final (LUT in the DQE, DISPLAY-CALIB.md measurements done), port the same roll-off to mahere_tiles::tone / mahere-engine colour.rs and the VSF gamma2tanh definition, and don't let the two drift meanwhile. Related: [[display_driver_design_first]].
