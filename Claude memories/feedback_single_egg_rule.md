---
name: feedback_single_egg_rule
description: Owner's "single egg rule" — never rely on one basket; critical inputs (entropy above all) come from several independent sources so no single firmware/vendor/physical effect decides the outcome
metadata:
  type: feedback
---

"Four eggs is good. You know my single egg rule." (2026-10-15, on KOHI entropy sources)

**Why:** any single source can be compromised or broken (EL3 firmware mediating the TRNG, a vendor block, one physical effect); independence makes one bad source irrelevant — the same thinking as PIPE's 8-of-16 byzantine ring threshold.

**How to apply:** for KOHI aim for four independent sources (SoC TRNG, CPU jitter, cross-oscillator jitter, WiFi radio noise; camera/mic/IMU join as their hardware comes up — "we'll be bringing all this up anyways"). Apply the same instinct elsewhere: don't design anything security-relevant around a single trusted component when an independent second is cheap. Table in KOHI-SOURCES.md. Related: [[pipe_byzantine_ring_threshold]].
