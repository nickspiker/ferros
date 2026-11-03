---
name: display_global_mask_then_layers
description: "Owner direction 2026-11-02 — the display server keeps one global compressed mask (per-row spans + sparse AA pixels, built once per screen size) for the OS's shape (edge, circle, corners), then moves to compositing layers; per-pixel geometry on the publish path is out"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-03T02:43:21.376Z
---

Owner (2026-11-02, after the screen edge moved to fluor's crossings walk): "Seems like we need to maintain a global compressed mask since most will be opaque for the OS in general. then start thinking about compositing layers."

**State when said:** the screen edge is a per-row plan (mask runs + sparse antialiased pixels from `fluor::paint::edge::walk_window_edges`, payloads/display `plan_edge`); the circle still computes a distance per pixel on the chrome rows. A per-pixel signed-distance edge cost 142 ms per publish (every keypress) that afternoon; the plan costs 20 ms for a full publish.

**Why:** most of the glass is opaque for the OS, so shape belongs in one compressed structure resolved once, not in per-pixel math on every publish.

**How to apply:** fold the circle (and anything else with a shape) into one plan per screen size: per row, spans (chrome / glass / mask) plus sparse AA pixels (x, coverage, hairline alpha, which hairline); colours resolve from the theme at compose. Then compositing layers: windows as layers composed by per-row span lists rebuilt when the window set changes, masked last. Never put geometry back on the per-pixel publish path. Related: [[feedback_hot_paths_branchless_bits]], [[windows_design_pending]], [[feedback_no_animations]].
