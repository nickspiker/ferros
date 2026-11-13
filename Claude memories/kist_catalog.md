---
name: kist-catalog
description: "Public searchable kist catalog on ferros.org and on the device (Settings → kists installs from it); no account, no gatekeeper, no fees, no ads; metrics later, many-dimensional, never a star rating"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-11T03:15:48.562Z
---

Owner, 2026-11-10:
- "for the home page, I say we have a passless kist list and we can list ferros, photon, lumis, tidal clock, etc. in there. probably make it searchable as I'm sure many devs will contribute kists. We get the Photon kist up so we can wire it up for realzies in the settings page".
- Answers: the list lives on both ferros.org and the device; "passless" means both no account to install and no approval to list.
- "do note we will eventually have metrics attached to each one, kinda like a marketplace but no fees and no adds and not a single dimensional star system."

**Design so far:**
- One VSF catalog file, one section per kist: name, developer key, option, version, capsule location, blake3, size, description, icon.
- The list vouches for nothing. The device checks the capsule hash against the list and the developer signature on install. The list itself is signed by the ferros project key against tampering.
- The site renders the list to HTML at build time, with a small script for search. No JSON.
- On the device, Settings → kists installs and updates from the list (fetch over plain HTTP, verify, store, seal). This doubles as the OTA updater.

**Host (2026-11-10, owner: "Go!"):** R2 bucket `kists` on the owner's Cloudflare account, custom domain kists.fgtw.org, plain HTTP 200 with no redirect (fgtw.org's zone has none), verified byte for byte with a probe. Wrangler on the dev box is logged in and can put objects (`npx wrangler r2 object put kists/<key> --file <f> --remote`). ferros.org renders its list from the same directory file.

**Order (owner, 2026-11-10):** "first kist is clock and we'll get that sorted. then we'll add tides, then a few other simple ones, then Photon etc." The Clock is published at kists.fgtw.org under the owner's developer key; its install shows version 0.0.0 on the device, cause still open. Photon comes later, signed with Photon's release key ([[kist-keys-follow-ownership]]).

**How to apply:**
- Leave room in each catalog entry for a metrics section with many independent dimensions.
- Never add a single score or stars, fees, or ads.

Related: [[ferros_vocabulary]], [[feedback-fgtw-devices-first]].
