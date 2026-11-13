---
name: input_rhe_on_glass
description: "Text input on glass (owner, 2026-10-27): a visible honeycomb keyboard (flush 9-10-9 QWERTY, Q moved; Dvorak fits) with shape writing to come, a scrolling extent (digits, symbols, emoji), two-finger scroll; built as the honeycomb library (Hive, ahau). Trimmed 2026-11-12"
metadata:
  type: project
---

Trimmed 2026-11-12: the superseded proposals (rhe chords on four finger buttons, a 36-stroke zone scheme, a centre scroll strip, 3x11 and 10-10-10 grids) are dropped; only the decisions and their reasons stay.

**Direction (owner, 2026-10-27):** design for looking, not blind. Physical keyboards are built so you don't look because you can feel the keys; glass has nothing to feel, so you look anyway. So: a visible **honeycomb** keyboard (organic, not tied to a two-hand finger layout; all six neighbours equidistant) plus a way past spelling every letter: **shape writing** (trace one path thru a word's letters, commit on lift, a decoder matches the shape to a dictionary word), then an English optimisation pass (rhe's frequency x effort method and its 135k-word dictionary at /mnt/Harbor/Code/rhe). **Prioritise single-finger interaction** (grip unknown).

**Layout (owner, 2026-10-27): a flush 9-10-9 honeycomb, 28 cells**, QWERTY with Q moved to the bottom row:
```
 W E R T Y U I O P
A S D F G H J K L .
 Z X C V B N M Q ,
```
Dvorak fits the same grid, only ' and ; moving to the extent: `, . P Y F G C R L` / `A O E U I D H T N S` / `Z Q J K X B M W V`. Owner: "careful. z and ; get swapped" (Z takes ;'s place at the left of the bottom row). A layout is data, so QWERTY, Dvorak (the owner's; `device.layout` 0 is the default) and an optimised layout are all options. The trace decoder inserts the apostrophe in contractions.

**Extent (owner: "close enough to build"):** stacked upward from the letters: digits row (0 first; base 10 fills the 10 cells, dozenal's two extra take the first two cells above), then symbol rows with the commonest at the row ends (easiest to reach), then obscure symbols and emoji. A fixed bar underneath never scrolls (shift, space, backspace, enter). Shift is a stroke before the letter, two in a row = caps lock (a sequence, not a timed hold). A page key on the symbol row does the two-finger swipe's job one press at a time (the one-handed answer).

**Gestures:** one finger traces or taps; **two-finger swipe scrolls** the extent (owner decided; a trace begun by one finger is dropped when a second lands, nothing commits before lift). Tap vs swipe by distance, never by time ([[feedback_no_animations]]). Scrolling tracks the finger 1:1; any post-lift motion is frame-counted physics per that memory.

**Attest:** taps on the same layout give exact bytes (a handle may not be a dictionary word; handles have spaces and capitals, and a case change is a different identity). The handle shows as typed ([[feedback_handle_not_password]]).

**Built 2026-10-27 (4df9644):** `payloads/hive` (working name) and the no_std `honeycomb` library shared by Hive and ahau: geometry, column-by-column drawing, nearest-centre hit test, `Touches` per-finger tracker (rollover = two taps), the extent with digits, symbol rows and the page key, two-finger scroll in whole rows. Not yet: emoji (no typeface for them), dozenal digits by `device.base`, shape writing (dictionary + decoder), a page key reachable from the letters view.

Related: [[windows_design_pending]] (the keyboard is layer 2), [[settings_identity_to_device_only]].
