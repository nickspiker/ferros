---
name: owner-decisions-2026-11-19
description: Owner's decisions of 2026-11-19 - RAW video lossless VSF clips (single-copy vault class), GPU fence and Mesa for its codec, the gnss crate and fgtw /gnss_eph, the camera cap, sensors, A510s; RAW video is the marketing priority
metadata:
  type: project
---

**RAW video (the marketing priority, finish it first):**
- Lossless only, VSF clips with new VSF types for the codec. Clips are written single-copy thru a new opt-in manifestus/vault class (no mirror).
- In Lumis: a linear FPS slider of whole frames 1-60 with 1 fps arrows, and RECORD/STOP. Each frame's interval is max(1/fps, shutter).
- Heat: any method is fine.
  - The A715s are capped at 1572 MHz during camera streams.
  - The encode is spread over all workers.
  - Zero copy.
  - Then the GPU: the camera ring passes the GPU's S2MPU fence READ-ONLY, only while a recording holds it. GPU kernels are compiled by Mesa's Valhall compiler on the HOST only; no Mesa on the device, and the binaries ship as VSF data.

**GPS:**
- The orbit-fitting code moves to a new shared sibling crate `gnss` (no_std, for ferros and the fgtw worker).
- fgtw.org gets a signed /gnss_eph endpoint, fetching BKG/ESA BRDC every 15 minutes and caching it (the owner approved the scheduled fetch and the cache). The device stays fleet-first.

**Also yes:**
- the busy lift stays on (1572 MHz);
- the A510s stay up as workers;
- the ultrawide's least gain is code 7 (1.007x; proof: codes 0-1 halve its slit rate);
- motion data is an implicit right;
- the accelerometer runs only while a reader holds it.

**Coming:** test eSIM profiles, "a little ways out".

Related: [[sensor_mailboxes]], [[gpu_role_layers]], [[feedback_keep_devices_busy]].
