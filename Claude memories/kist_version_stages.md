---
name: kist-version-stages
description: "The owner's version scheme (2026-11-10, novel per owner): any number of parts, leading zero parts insignificant, more parts = later stage, each part shown in the person's base"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-11T01:28:58.019Z
---

Owner, 2026-11-10:
- "we don't limit the .'s like the major, minor, patch".
- "don't show leading .'s that are 0".
- "00000123=123".
- "If I change my clock from ver 12 to ver 1.0 because I think it's ready to go from alpha to beta and then from 45.0 to 1.0.0 when I think it's ready to go mainstream".
- "each place deliminated in base of choosing".
- The owner considers the scheme novel. I had pointed at Debian/RPM omitting a zero epoch as nearby prior art.

**The rule:**
- A version is a list of non-negative integers.
- Normalise by dropping leading zero parts (all zeros → 0).
- More parts is newer: the part count is the stage, e.g. 1 alpha, 2 beta, 3 mainstream. So 12 < 1.0 < 45.0 < 1.0.0, 9.9 < 1.0.0, and 0.1.0 == 1.0.
- With the same number of parts, compare left to right, numerically.
- Display the normalised parts, each in the person's chosen number base, separated by dots.
- Store the developer's own signed bytes; normalise only to compare and display.

**How to apply:** kist versions (ferros_kist `Version`, the installer's update offer, the Kists page). Never major.minor.patch for ferros's own schemes. Related: [[kist-catalog]].
