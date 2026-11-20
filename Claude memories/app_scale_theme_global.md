---
name: app-scale-theme-global
description: Owner 2026-11-20 - every kist follows the person's global app scale (RU) and the device theme by default, an implicit right; the deck keeps its own scale and orientation; fluor's ferros host applies both
metadata:
  type: project
---

Owner, 2026-11-20, after Kūaha shipped ignoring the scale and copying the theme table: "deck has its own scale and orientation, but yes, settings pages should modify that, same with apps. I'd say it's a global permission we need to add? Device theme and etc. I'd presume we may add a way to unlink those later but I'm prety sure if the user sets the RU high, they want all their kists to render that way."

**The rule:**
- **One global app scale and one device theme,** which every kist follows by default: an implicit right, never asked ([[rights_implicit_set]]).
- **The scale is ferros_sys PREF_SCALE (14),** in 64ths of an octave, so the factor is 2^(s/64). **The theme is PREF_THEME (5).**
- **The deck keeps its own scale and orientation,** separate from the app scale.
- **Settings pages and a pinch inside a kist change the global app scale.**
- **A per-kist unlink** may come later; it isn't built.

**How to apply:**
- fluor's ferros host reads both and applies them to every app: the scale as the Viewport's RU, the theme as fluor's theme, following changes with a C1 relayout. No app copies a theme table or ignores RU.
- Kists are built on the OS fluor stack (Viewport/RU, layout, widgets, theme, text): never hand-drawn on raw canvas calls where fluor has the piece. Kūaha's first build did, and the owner caught it.
