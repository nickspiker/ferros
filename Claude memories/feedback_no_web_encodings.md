---
name: feedback_no_web_encodings
description: "Owner 2026-10-27: no web encodings (base64, JSON, URL forms) ferros-side; ferros forms only (G#, base36, EWE/VSF). One legacy-edge exception (2026-11-09): the eSIM SGP.22 client, one process, strict envelope, nothing stored"
metadata:
  type: feedback
---

No base64/base64url, JSON or other web-world encodings in ferros code, protocols or stored keys. The one JSON file is rustc's target spec (`targets/aarch64-unknown-ferros.json`): rustc reads nothing else, so that one is rustc's format, not ours.

**Why:** ferros has its own forms — hex `G#`, decimal `A#`, base36 per the number convention, EWE/VSF on disk — and a foreign encoding showing up means a limit was being worked around instead of fixed (kete's vault keys had been squeezed into a 64-byte key limit I had set myself).

**How to apply:** when bytes do not fit a ferros-side format, widen the format or the limit, or carry the bytes as bytes (kete's 32-byte addresses go to the kernel store as 32 bytes, SYS_STORE bit 5 — hex was the second wrong answer); never reach for base64/JSON or a text form of binary. Ask if a foreign format seems unavoidable.

**The one exception, legacy at the edge (owner, 2026-11-09: "keep that in legacy with the CIE 1931 sRGB and other oddities"):** eSIM provisioning (GSMA SGP.22 ES9+) only speaks HTTPS + JSON with base64 DER, so the download client may use them — in one process only, the legacy edge, like a foreign colour space at a platform's output: a fixed-schema envelope codec for the spec's few message shapes (refuse any unknown key, nesting or type), base64 decoded to bytes on the spot, never stored, never passed out of that process. What is kept (profiles, EID, activation history) is VSF in the vault. TLS via rustls in a std process, trusting only the GSMA CI root. The eUICC verifies the SM-DP+ end to end, so the client is a courier. Relaying ES9+ through FGTW is a later option, never the only path (the relay sees the activation code; single egg). Related: [[feedback-vsf-rgb-only]].
