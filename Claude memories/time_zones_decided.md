---
name: time-zones-decided
description: Arabic shows legal wall-clock time (zone rules, DST); dozenal (and hex) show the zone's standard offset, never DST, so their date can differ; zone auto from cellular then GPS regions; the setting is the identity's with a per-device override
metadata:
  type: project
---

Owner, 2026-11-10: "arabic needs to match wall clock with DST legal, dozenal stays thus making the date possibly differ between settings, your call on hex. Proper zone too, cellular, then GPS with regions ideally or whatever the standard practice is there. and it's per identity, you can uncheck and set per device tho." Earlier the same day: "No DST in dozenal ... we do the local offset at least. makes the date roll with the clock, not 'legal'".

- **Arabic:** legal local time: the zone's rules including DST (IANA tz rules, carried as VSF).
- **Dozenal (DMS, DLS):** the zone's STANDARD offset all year, never DST; the date rolls at that local midnight, so it can differ from arabic's for an hour a day in DST season. Intended.
- **Hex (my call):** like dozenal (standard offset, no DST): hex is a linear count, and a DST jump breaks linearity.
- **Which zone:** a proper zone (IANA id), detected: the cellular network's time and zone first (NITZ with the country from the MCC, as Android does), then GPS position against zone regions; manual as the fallback.
- **Whose:** the identity's setting (travels to all the person's devices), with a per-device override (uncheck "follow my identity" and set the zone on that device), like home-screen sync.

**How to apply:** never apply DST to dozenal or hex; never show standard time in arabic. Zone data in VSF (Rule -1), built from tzdata by a Rust tool. GPS region data (timezone-boundary-builder is ODbL, from OpenStreetMap): its licence is the owner's call before it ships. Related: [[settings-identity-to-device-only]], [[feedback-no-polling]] (zone changes are pushed, never polled).
