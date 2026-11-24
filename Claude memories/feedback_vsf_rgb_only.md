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

**VSF RGB is gamma-2 encoded, never linear (owner, 2026-11-24):** "VSF RGB is NOT linear, you know bloody well how to do colourspace conversions." A conversion "to VSF RGB" means linear light, VSF primaries, then encode with VSF RGB's gamma of two (code = sqrt(linear)); g2t is then assigned to those codes. Skipping the gamma-2 encode squared the icons' light twice (133dcd13, fixed after).

**Kist icons (owner, 2026-11-24, approved on glass: "that's what it's supposed to look like"):** an icon without `primaries: vsf` is sRGB: decurve to linear x, extend highlights L = tan(x·π/2) (f64, so white → finite huge, never negative), SRGB2VSF_RGB, then g2t encode (tanh(√L)); white lands neutral at the top code, nothing clipped, no white balance (forge 52af0b98, KISTS.md "Icons"). Kūaha and Aemulus are tagged `primaries: vsf` and kept as authored.
