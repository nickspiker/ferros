---
name: deck-loading-pie
description: Owner 2026-11-24 - a kist or puck loading on the deck shows a pie (pacman) fill per stage: blue = download, green = optimisation and verification, red = the final steps; multiplied by the kist's icon, a puck's black/grey
metadata:
  type: project
---

Owner, 2026-11-24: "Would also be nice to have a pacman/pie fill for kists and pucks whilst they are loading. I think blue channel for download, green for the optimization and verification step and the red channel for any final stuffs, which should be multiplied by the kist icon or in the case of a puck be black/grey."

**The design:** each loading circle on the deck carries three pie sweeps, one per colour channel: blue fills with the download, green with the stow's optimisation and verification (compile, hash, signature checks), red with the final steps (seal, store, register). Each sweep multiplies the circle's icon in its own channel, so the icon comes up channel by channel as its stages finish; a puck (no icon) uses black/grey under the sweeps. Composited front to back with `under` (darkness space), C¹ (the pie's edge antialiased analytically, no kinks).

**Built 2026-11-24 (forge 9fa97f9c):** ferros_glow pie.rs (C¹ edge, exact at 0 and whole), display op 87 (system-only), the map reads the stower trailer (op 0, per job [hi][ht][blue][green][red] Q15) or derives shares from today's stages; puck or missing icon = grey G#808080. Not yet watched on a real stow.

**Why:** after a factory reset the whole deck refetches ([[identity-restores-the-device]], [[kist-distribution-p2p]]); each circle should show where it is.

Related: [[feedback-darkness-space]], [[pucks-draw-squares]].
