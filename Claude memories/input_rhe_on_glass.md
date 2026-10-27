---
name: input_rhe_on_glass
description: "Owner's input direction (2026-10-27, proposed, not decided): rhe-style chord/gesture input on the touchscreen — four primary buttons (one per finger), each with a tap plus four swipes = five per four (20 single gestures); chords multiply it. Feeds the attest screen and ahau. rhe is the owner's phonetic chord engine at /mnt/Harbor/Code/rhe"
metadata:
  type: project
---

The owner is going over the text-input idea for the phone (the input device the attest screen and `ahau` wait on). The proposal: rhe on glass — four primary buttons, one per finger; each takes a tap or a swipe up/down/left/right, so five per finger, twenty single gestures; chords across fingers multiply that.

rhe (/mnt/Harbor/Code/rhe): a phonetic chord engine — 9 keys, left hand chords vowels, right hand chords consonants with the thumb as the voicing bit; you type sounds and a 135k-word pronunciation dictionary spells the word; ordered briefs split homophones by which finger lands first; gestures effort-ranked on real hands and assigned by frequency; number and symbol modes.

How it maps onto the touch server (payloads/touch: per-contact id, phase down/move/up, pressure, x, y; CHANGES returns events oldest-first):
- commit on lift — the gesture is first-down to last-up (rhe's "keys never all hit zero until the roll completes");
- tap vs swipe by distance travelled, never by time — keeps the no-time-windowed-gestures rule ([[feedback_no_animations]]);
- swipe direction can replace rhe's thumb voicing bit; first-down order gives ordered briefs for free;
- effort: flex/extend (up/down) is cheap, sideways is costly, ring finger worst — measure and rank before assigning;
- 4 buttons × {tap, up, down} = 12 = a dozenal digit set in single gestures.

Catch for attest: rhe's phonetic engine spells via a dictionary, but the handle must be exact bytes (a handle may not be a word; one wrong byte derives a different identity) — attest needs a literal mode, not the phoneme engine. Gestures without visible letters resist shoulder-surfing, which suits a secret.

**Owner: prioritise single-finger interaction** (2026-10-27; grip unknown). That makes grip irrelevant (thumb one-handed or index two-handed) but drops chords, so a stroke's meaning is (start zone) × (direction). 4 zones × (tap + 4 swipes) = 20 is too few for letters + digits; recommended: add diagonals, tap + 8 directions = 9 per zone, 4 zones = 36 = exactly 0-9 + a-z, ferros's base36 alphabet. Effort-rank all 36 on real hands (diagonals cost more), cheapest to most frequent. Chords become an optional speed layer later, never required. Handles need **spaces and capitals**: the handle guard already handles multi-word, mixed-case handles and treats a case change as a different identity — so 0-9 a-z alone is not enough for attest.

Spaces and punctuation (owner liked the 36; asked for spaces, periods etc.): space is the most frequent character in text, so it gets the cheapest stroke. Proposed: a **fifth zone = the space bar**, bottom centre — tap = space; its 8 directions take backspace (left: where delete is expected), period, comma, enter (attest: submit), shift, apostrophe, hyphen, question mark, effort-ranked. **Shift is a stroke before the letter, not a held key**: one shift = next letter capital, two in a row = caps lock (a sequence, not a timed hold). **A symbol page** for the long tail: one stroke opens it, the next stroke on the same four zones picks one of 36 symbols, then it drops back (rhe's symbol mode). Totals: 45 single-stroke characters (36 alphanumerics, space, 8 edits/punctuation), everything else two strokes.

**How to apply:** single-finger first; design the 36-stroke set with the owner before building; give attest a literal exact-character mode; chords only as an optional layer.
