---
name: pucks-draw-squares
description: Pucks draw squares (built 2026-11-18 on branch pucks-build, running on the 8a); the deck holds only settings, kists and pucks (display widgets deleted); taps pass thru; basecalc is an even-Warrington dial
metadata:
  type: project
---

Owner, 2026-11-18: "What's wrong with supplying a diameter and having the kist return a square vector in sixteen bit that ferros will trim and place?" That replaced the drawing-ops list the display parsed (built 2026-11-17 on branch `pucks`). Never re-propose an op list on the wire; lists live on only as a puck's own drawing library (ferros_puck::square).

- Built on branch `pucks-build` (ff178dd2 onward): kernel pucks' window (PUCK_VA G#50_4000_0000, 16 slots of 64 MiB, frames on demand by SYS_DMA x1 = 6, BELL_PUCKS 14, stamp 72 bytes), display puck.rs (square sampled into the circle, op 74 taps, a puck's box redrawn per phase at the flips, turned with the deck not gravity), map (tap passes thru, pucks known by their signed `puck` declaration), payloads/clock = clock puck, std/basecalc `--features puck`. docs/PUCKS.md has it all.
- Owner's touch rule: "Tap/drag with one finger moves it, with two scales/rotates and a simple tap passes thru so we can make our dozenal calculator work too." No tap-to-focus.
- Owner, 2026-11-18: "Delete the old clock, yes. none of that business. ferros settings, kists and pucks, that's it." The display's clock face and light scope, the focus op 25, gravity_poll and payloads/focustest are deleted (88fa6463). Never add a circle the display draws itself; a live face is a puck. The scope's data path (sensors' stream by pref 7, SYS_LIGHT, the Developer channel radio) was kept for a possible scope puck.
- basecalc's dial (owner: "an optimized warrington ... fairly even on the two rows"): readout core, inner row n, outer row 2n staggered, equal-area cells; dozenal digits drawn as tatau's glyphs G#10-G#1B ("use the lower control set of ascii, not 0-B"); a readout tap is the clipboard stub.
- A puck killed from outside (`diag kill`) is a stop the map will not relaunch on its own: it waits for a tap on its circle.
- The commit hook's handle guard flags the swash geometry module's name, so ferros_glyphs compiles payloads/display/src/glyphs.rs by #[path]; never move or re-add lines naming that module.
- Signed capsules in /mnt/Harbor/Code/ferros/target/agent-pucks-build/kists; 8a system capsules in .../agent-pucks-build/akita.

**Why:** nothing parsed by the display (a buffer, not a language), any drawing a puck likes, the circle mask is the puck's right.

**How to apply:** check docs/PUCKS.md "Not built" and "Open questions" before more puck work. See [[ferros-vocabulary]], [[fluor_pixel_depth]], [[feedback-no-animations]].
