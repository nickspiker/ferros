---
name: feedback_no_web_encodings
description: "Owner 2026-10-27 ('WTF is base64url doing anywhere near ferros'): no web encodings (base64, JSON, URL forms) in anything ferros-side; keys and numbers in ferros's own forms (hex G#, base36, EWE/VSF on disk). Change a limit rather than squeeze bytes through a foreign encoding"
metadata:
  type: feedback
---

No base64/base64url, JSON or other web-world encodings in ferros code, protocols or stored keys. The one JSON file is rustc's target spec (`targets/aarch64-unknown-ferros.json`): rustc reads nothing else, so that one is rustc's format, not ours.

**Why:** ferros has its own forms — hex `G#`, decimal `A#`, base36 per the number convention, EWE/VSF on disk — and a foreign encoding showing up means a limit was being worked around instead of fixed (kete's vault keys had been squeezed into a 64-byte key limit I had set myself).

**How to apply:** when bytes do not fit a ferros-side format, widen the format or the limit, or carry the bytes as bytes (kete's 32-byte addresses go to the kernel store as 32 bytes, SYS_STORE bit 5 — hex was the second wrong answer); never reach for base64/JSON or a text form of binary. Ask if a foreign format seems unavoidable.
