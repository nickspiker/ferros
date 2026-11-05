---
name: feedback-photon-is-the-full-app
description: "Photon is ONE full self-supporting app on every platform, ferros included (std/photon); no photon_core / stripped / ferros-native copy; FGTW = attest, identity, basic OS sync (owner 2026-11-05)"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-05T21:54:45.435Z
---

The owner, 2026-11-05: "there is no Photon core, there is FGTW that handles the attest and identity and some basic OS sync stuff then there's Photon which is a full self supporting application on every platform. None of this 'Photon stripped copy' bullshit."

**Why:**
- Sessions on 2026-10-26 built `photon_core` (no_std): a reimplementation of Photon's wire, PT, join, chain and beam code, plus a payloads/beamtest beam stack. They put "finish photon_core" on STATUS.md's roadmap.
- A day later (2026-10-27) the real Photon ran on the device as `std/photon`: the photon-messenger crate on the ferros std target, drawing through fluor. Later sessions, me included, kept repeating the stale roadmap item.
- Two copies of a protocol drift apart. The real app is the only one that counts.

**How to apply:**
- Never propose finishing, extending or porting `photon_core`, or a "ferros-native Photon client".
- Photon work on ferros means making the real app run better there: the std target, fluor's ferros host, kete's vault backend, tokio over the `ip` server, input.
- Identity, attest and OS sync are FGTW's.
- Read "photon_core" in old docs as superseded. Retiring it (payloads/fgtw, beamtest and tools/photon-interop depend on it) is the owner's call.

Related: [[std-target-status]], [[photon-on-ferros-no-bootstrap]], [[photon-terminology]].
