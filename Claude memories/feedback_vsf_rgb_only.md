---
name: feedback-vsf-rgb-only
description: "No sRGB, no OKLab, no Rec.709 anywhere in ferros — colours are picked in VSF RGB and brightness-matched by photopic luminance through LMS (owner, 2026-11-01)"
metadata:
  node_type: memory
  type: feedback
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-01T18:40:54.592Z
---

Every colour ferros draws is authored in **VSF RGB** (monochromatic primaries 703/523/462 nm, Illuminant E white, linear light; surfaces carry it as gamma 2). Pick any point in that triangle; match brightness by photopic luminance through LMS: `vsf::colour::vsf_rgb_to_photopic_f32` (VSF_RGB2LMS then LMS2PHOTOPIC, CIE 170-2 10°; weights R 0.2584, G 0.6346, B 0.1070, white 1). That is the whole method.

Never sRGB (owner: "200 flavors", not spectrally defined), never OKLab, never Rec.709 luma weights (0.2126/0.7152/0.0722), never XYZ/xyz or any CIE 1931 space, never a borrowed perceptual space. Photon too: a foreign space only where a platform forces it, at its output edge (owner, 2026-11-01).

**Why:** the display's ODT reads every surface as VSF RGB (payloads/display ODT, panel primaries measured). Colour authored in sRGB numbers came out oversaturated and hue-shifted — the attest mark and the keyboard rainbow were "completely fucked" (owner, 2026-11-01). VSF RGB was chosen so its R, G, B are perceptually right already.

**How to apply:** any new colour code: VSF RGB + VSF's photopic function, from the vsf crate (no hand-copied matrices). Grep for OKLab/sRGB/709 when touching colour and replace. Related: [[colourspace-pinned-vsf-gamma2tanh]], [[feedback-no-web-encodings]].
