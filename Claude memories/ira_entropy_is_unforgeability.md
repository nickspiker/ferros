---
name: ira_entropy_is_unforgeability
description: "ira entropy matters for UNFORGEABILITY, and that includes keeping the full source set from any one party (Google included); a forgeable ira registered+unlocked = an unevictable fleet member; best-ira = physical PUF core Android never saw + vendor breadth"
metadata: 
  node_type: memory
  type: project
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

Settled 2026-08-20. The ira is public (see [[ira_is_brand_not_credential]]), so its entropy is not about hiding it — it is about whether an attacker can FORGE a device (real or emulated) that derives an ira the fleet already trusts.

**Why:** if a forgeable ira is registered AND unlocked in the fleet, the attacker becomes an unevictable member — every SHRINK re-wraps the fleet key to that ira, and you cannot lock it without locking your own device because you cannot distinguish the brands.
So a serial-only ira is unsafe: a structured serial is settable in emulation and readable with brief possession.
The ira secret must root in material that is high-entropy AND physically bound AND not attacker-settable.

**How to apply:** build the ira in two layers — an unforgeable physical PUF core (fuzzy-extracted: secure sketch + ECC, public helper data) plus vendor-issued identifiers for breadth/collision-resistance (breadth only).
Tier sources so a drifting bit never bricks the vault: Tier 0 deterministic serials key now; Tier 1 chipid trims graduate via a two-boot + temp-cycle probe; Tier 2 PUFs (UFS bad-block, camera hot-pixel, SRAM, MCT clock-ratio) ride fuzzy extraction.
Grounded husky source catalog + register addresses: [[ira_entropy_sources_husky]] and ferros/IRA-ENTROPY-SOURCES.md. Measure before keying (see [[feedback_measure_before_fixing]]).

**Refined 2026-10-19 (owner):** "technically it is both — we don't want Google to have our entire fingerprint if possible." The brand is public, but the *source material* must not be fully held by any one party, because whoever holds every input can derive the ira and so forge it. Google is such a party: stock Android with Google services ran on this phone from 2023, so whatever Android could read (serials, IMEI, MACs, the camera API's per-unit calibration, motion-sensor calibration error, sensor noise in any photos backed up to Google) should be treated as already known to Google. Vendor identifiers therefore add breadth but zero forgery resistance *against Google*; that has to come from sources Android never saw or never could read (measured by ferros at cold boot or under conditions the Android stack never uses). Google's boot chain still runs first on every boot and could in principle read power-on state, but it has no network; the exfiltration path was Android, which ferros removes.

**Settled for husky, same day (owner):** "we assume that Android can see our identity but basically nothing else. That's fine and acceptable. Google made the hardware and it's not PIPE certified anyway so we get what we get." So on husky, Google-visible identity inputs are accepted; sources Android never saw are a best-effort strengthening, not a requirement, and nothing should be over-engineered to hide the identity from the hardware's maker. The hard line is everything *except* identity: vault contents, fleet keys, wairua and photon traffic must never be visible to Android. Full unforgeability against the hardware maker is a PIPE-certified-hardware property, not a husky one. See [[ira_is_brand_not_credential]].
