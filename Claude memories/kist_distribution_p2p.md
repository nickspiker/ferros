---
name: kist-distribution-p2p
description: Owner 2026-11-24 - kists spread device to device like BitTorrent; fgtw.org holds the bootstrap list; fleet, contacts, anyone ("support the network" per device, on by default, WiFi + plugged in); devs append to the list at will; desktops serve only what they run
metadata:
  type: project
---

Owner, 2026-11-24: "where do we host the list? fgtw.org bootstrap I suppose? and then with clients? This is part of the bittorrent like network for distributing. I'd presume if my desktop has a copy and a friend wants it, I can serve it directly rather than going to brobdingnagian."

**Decided:**
- The list: fgtw.org (kists.fgtw.org) is the bootstrap copy; devices pass developer-signed entries to each other after that; the newest version under the same developer key wins.
- Capsules are content-addressed (blake3) and signed, so any source is as good as any other; pieces verify alone (blake3 tree), several sources at once.
- Fetch order: own fleet, contacts, local network peers, fgtw.org / R2 last (devices first, [[feedback-fgtw-devices-first]]). Ask more than one source now and then so a peer can't hold back an update.
- Who may fetch from you: the fleet always; contacts on by default; anyone only with a separate "support the network" opt-in (may later cover relaying and bootstrap). Kept separate from custodians (recovery is a trust choice about named people; serving is a contribution to strangers).
- Cellular: "Whatever. WiFi preferred", granular settings later.
- Desktops serve only kists they actually run ("Only if the desktop actually uses them so probably not. I'm a fan of supporting the network but not bloat.").

**How to apply:** design doc first (docs/KIST-DISTRIBUTION.md), then build on the fleet capsule fetch (fgtw ops 38-40, 2b53c2bb). Related: [[kist-catalog]], [[identity-restores-the-device]].

**Hosting and format (owner, 2026-11-24):** "One entry format, yes" (one developer-signed entry form, Photon included); "revoked key, also yes" (stowed kists keep running, no updates, a note on the card). "I'd put Lumis and Mahere on holdmyoscilloscope and make fgtw.org point there since those are mine. Photon and possibly some other system apps will be hosted at kists.fgtw.org since Photon is not mine." So the web rung is per developer: owner-key capsules on the holdmyoscilloscope R2, commons-key ones (Photon, some system apps) on the kists bucket; fgtw.org's entries point to each. Photon's deploy must move its ferros capsule from holdmyoscilloscope to kists.fgtw.org (Photon session's change).

**Support the network and listing (owner, 2026-11-24):** "support network is per device, yes, and it's actually enabled. Limit to WiFi and when plugged in by default but without clients supporting... think bittorrent." So: per device, ON by default, serving anyone by default only on WiFi and while plugged in. "fgtw.org is just the bootstrap/phonebook and host for a few things. devs should be able to append to that list at will." So developers append their own signed entries to fgtw.org's list themselves, no gatekeeper.
