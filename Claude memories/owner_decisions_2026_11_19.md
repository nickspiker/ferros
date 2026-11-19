---
name: owner-decisions-2026-11-19
description: Owner's ACTUAL decisions of 2026-11-19 (RAW video, heat by any method, GPU fence + host Mesa) and the coordinator's recommendations he has NOT approved; RAW video is the marketing priority
metadata:
  type: project
---

**Approved by the owner (2026-11-19):**
- **RAW video** is the marketing priority, finished first.
  - Lossless only, VSF clips with new VSF types for the codec.
  - Single-copy writes thru a new opt-in manifestus/vault class (no mirror).
  - In Lumis: a linear FPS slider of whole frames 1-60 with 1 fps arrows, and RECORD/STOP. Each frame's interval is max(1/fps, shutter).
- **Heat for RAW video: "anything else really, doesn't matter to me."** Any method that cuts heat.
- **The GPU stage of the RAW codec ("yes and yes"):**
  - the camera ring passes the GPU's S2MPU fence READ-ONLY, only while a recording holds it;
  - GPU kernels are compiled by Mesa's Valhall compiler on the HOST only, with no Mesa on the device, and binaries shipped as VSF data.

**NOT approved (the coordinator's recommendations, still open):** the coordinator once recorded these as approved by mistake. Never act on them as decisions:
- a shared `gnss` crate, and fgtw.org fetching BRDC ephemerides every 15 minutes with a cache. The owner asked why GPS needs fgtw.org at all. The options are his fleet's devices fetching the ephemerides over HTTPS and sharing them (devices-first), TLS in ferros, or fgtw.org as a fallback;
- capping the A715s at 1572 MHz during every camera stream (beyond the RAW-video heat work);
- keeping the A510s always up, and the busy lift on by default;
- the ultrawide's least gain at code 7 (a floor, which needs his approval under the no-clamp rule);
- motion data as an implicit right, and the accelerometer only while read.

**Decided later the same day:** no gnss mirror on fgtw.org. ferros gets HTTPS (a TLS client): "I think we need https as annoying as it is. So much shit uses it. Think a Google Chrome APK that will likely have to run on ferros." The GPS ephemerides then come straight from the public HTTPS sources (BKG, ESA), fleet-first. Staged after RAW video.

**How to apply:** a "yes" answers only the question it was given to. When an answer is ambiguous, ask which question it answers before recording anything.
**Coming:** test eSIM profiles, "a little ways out".

Related: [[sensor_mailboxes]], [[gpu_role_layers]], [[feedback_keep_devices_busy]].

**RAW video, later the same day:**
- **Clip sealing at rest is optional, the coordinator's call.** Chosen: sealed by default, with a per-clip off switch. It lands after the first shootable build; today's first clips are unsealed.
- **Grading export approved:** "VSF -> cinema DNG or TIFF or png sequence or video or whatever man, I really don't care. plugin maybe for a format ideally but you tell me."
  - Chosen: a HOST-ONLY tool, `tools/clip-export`, writing a CinemaDNG sequence that keeps the mosaic lossless.
  - It's the one approved foreign encoder, for grading only; nothing on a device writes DNG.
  - A Resolve decode plugin is being researched as the ideal.
