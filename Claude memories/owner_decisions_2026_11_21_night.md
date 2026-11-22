---
name: owner-decisions-2026-11-21-night
description: Owner's night-of-2026-11-21 calls - bring up the GPU and all hardware after a jump; build the GPU compose path now (profile later); screen off = no UI GPU work but Lumis RAW encode continues; layers AND whole-frame handoff; promo-ready Photon (Opsin built in) and Lumis (RAW, 4 cams, focus/ISO)
metadata:
  type: project
---

Owner, 2026-11-21 evening, answering the GPU-migration questions and setting the night's work ("Pro is all yours!"):
- **After a jump:** "if we can bring up GPU and all hardware after a jump, that would be excellent." Reset and start the GPU (and other hardware) after a jump instead of leaving it off; this closes the old "GPU after a jump" open decision.
- **Build the GPU compose path now:** "build the GPU path, whilst we are we can tweak and adjust. Once it's built we can profile it and look at hotspots and think about tricks." Don't wait on optimization (120 Hz tricks come after profiling).
- **Screen off:** the GPU does no UI work, but "Lumis shoots RAW video and uses the GPU for encode so if the screen is off, that should not affect video recording."
- **Handoff:** "support a full window and a frame. some cases we want a full window anyway." Both the layer handoff and a whole flattened frame.
- **Noise:** "we'll likely have to redo how we do the noise since I think it's stochastic which is not GPU friendly" (the parallel lowbias32 + scan form exists, be536647).
- **Thermals:** "No clue on thermals. we can look later."
- **Goal:** migrate Photon and Lumis (Lumis's intro camera picker and main interface) to the GPU path, and have both ready for marketing promo videos showing the smoothness. "Function over form tho."
  - Lumis: shoots RAW video, no dropped frames on all four cameras, focus adjustable on all four, ISO, "full meal deal".
  - Photon: "probably needs Opsin built in so it shows images and shit too".

Related: [[gpu_role_layers]], [[hv_jump_to_ferros]], [[owner_decisions_2026_11_19]], [[husky_camera_bringup]].
