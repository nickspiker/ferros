# Time and zones

Status: built 2026-11-10 (host-tested; on the device once the owner deploys the kernel, the modem, gps, fgtw and map capsules and stores the `zones` document).

Owner, 2026-11-10: "arabic needs to match wall clock with DST legal, dozenal stays thus making the date possibly differ between settings, your call on hex. Proper zone too, cellular, then GPS with regions ideally or whatever the standard practice is there. and it's per identity, you can uncheck and set per device tho."
Then: "no reason not to knock out the fleet preference, auto/manual zone, picker, all that."

## The clock

- **Where the time comes from:** the PMIC's real-time clock (s2mpg14), counting UTC, read at boot and every hour after (ferros_kernel `rtc.rs`).
- Every process reads wall time from its own counter and the kernel's base in its preferences block, no syscall (ferros_sys `local_time`).
- Network time (NITZ's own clock fields) and GPS time are not used yet; nothing sets the PMIC's clock.

## Two offsets

A zone has a standard offset and a legal one (its standard offset, or its daylight one while daylight time is in effect).
The kernel hands both to every process in the one wall slot (preferences bytes 24-31: standard in the low 32 bits, legal in the high 32; `rtc::packed`, ferros_sys `zone_offsets`).

| Base | Offset | Why |
|---|---|---|
| Arabic | legal | it matches the wall clocks, daylight time and all |
| Dozenal (DMS, DLS) | standard, all year | the date rolls with the clock, not the law (owner: "dozenal stays") |
| Hex | standard, all year | a linear count never jumps (the owner left hex to Claude) |

So in daylight season an arabic date and a dozenal one can differ for an hour a day; that is intended.
The rule is tzdata's own, the POSIX TZ string at the end of each zone file, read by `ferros_tz` (no_std, integer, no heap), which also says when the legal offset next changes: the kernel hands the offsets out again at that instant, a known deadline, never a poll.

## The zones document

`tools/tzvsf` writes one VSF document, stored on each device as the vault key `zones` (`ferros-bridge store put zones @zones.vsf`):

- a section `zone` for each of the 312 zones of `zone1970.tab`: `id` (the IANA name), `rule` (the POSIX string; every one is parsed by ferros_tz before it is written), `country` (ISO 3166 alpha-2, one value each, the country of the zone's principal place first) and `at` (that place, latitude and longitude in arc-seconds);
- a section `country` for each country with a mobile network: `id` and its mobile country codes (`mcc`), from mobile-broadband-provider-info's `serviceproviders.xml` (public domain, CC-PDDC): 152 countries, 156 codes.

The kernel reads it once into a table (`zone.rs`), and again when the bridge stores a new one.
Without it a zone by id has no rule (the fixed offset stands) and nothing can be found.

## Which zone

Two choices, each **automatic** (the zone found on the device; its zone by hand stands in until one is found) or **by hand** (an IANA id, or none: a fixed offset, a quarter hour at a time):

- **The identity's**, the person's: a fact in their identity space (ferros_space kind `ZONE`), the same on all their devices.
- **This device's own.**

The device follows the identity's choice while "follow my identity" is on (the default) and the identity has made one; otherwise its own.
Turned off, the device keeps a zone of its own, as the home screen's "this device's own" does (IDENTITY-SPACE.md).

Settings flow identity to device only: the identity service (payloads/fgtw) writes the person's choice to the kernel at every join where it changed (SYS_ZONE op 4); nothing on the device is ever read back into the person's space.
A choice made on the clock page while following goes to the identity service (fgtw op 22), into the space and on to the fleet and fgtw.org, then to this device's kernel; refused before the first pull, as the home screen's switch is.
Releasing the identity from the device forgets its zone.

## Found on the device

As Android does: the network first, then a position.

- **The cellular network.** The modem decodes every NITZ (SIT `IND_NITZ_TIME_RECEIVED`, G#0905) as the vendor's `ProtocolNitzData` reads it: the parameters after the 8-byte indication header are 0 the daylight flag valid, 1-6 the time (UTC), 7 the zone in signed quarter hours (the whole legal offset, daylight time in it), 8 the daylight adjustment in hours, 9 the weekday, 10 MM info, 11-16 the network as ASCII (MCC then MNC). It hands the MCC, the offset and the daylight flag to the kernel (SYS_ZONE op 0). `IND_MCC` (G#072D) hands in the country alone, once per change.
- **GPS.** The driver hands each new position to the kernel (op 1) as it keeps `gps.last`; the kernel takes it as a new clue only once the device has moved 1 << 10 arc-seconds (some 30 km). The position never goes into a measure. GPS runs only after attest.

Both are narrow rights of their own, by the caller's name as `SYS_STATUS` is, never the settings right.
The service loop's next pass finds the zone (`ferros_tz::find`):

- the zones of the network's country whose rule gives the network's offset now, and its daylight flag too when it gave one (failing that the offset alone, failing that the country alone);
- with a position, the one whose principal place is nearest (integer great-circle ordering, `ferros_tz::nearest`);
- without one, the first whose country is listed first (zone1970.tab puts the most populous first), but only when the network gave an offset or the country keeps one rule: a country of several zones and no offset names none, and an offset alone names nothing.

Nothing named keeps the zone found before, unless the network now says a country it is not in.
The zone found and what found it are kept in the device settings, so a boot has its zone before any network.

**The position is a fallback until zone regions exist.**
The nearest principal place is not the zone a place is in: near a border, a place can be nearer another zone's principal place than its own.
Region boundaries (timezone-boundary-builder, from OpenStreetMap) are ODbL; whether ferros ships them is the owner's call.

## Kept

In the device settings document (SETTINGS.md): `tz` (the fixed offset), `zone` (this device's zone by hand), `zone_auto` (1, the default: automatic), `zone_follow` (1, the default), `zone_identity_auto` (present once the identity's choice was written) and `zone_identity`, `zone_found` and `zone_found_by` (1 the network, 2 a position).

## The clock page (payloads/map)

The zone in effect by name, its standard and legal offsets, where it came from; "follow my identity"; automatic / by hand; the zone by hand and "choose a zone"; the quarter-hour buttons only for a fixed offset.
The picker lists the regions (the IANA id's first part), then a region's places, alphabetical, from the kernel's list (SYS_ZONE op 6).
There is no typing: the map has no keyboard of its own (the attest screen hosts the one there is).

## SYS_ZONE (35)

- Op 0, the modem: x1 the MCC (0 none), x2 NITZ's legal offset in seconds east, x3 flags (bit 0 the offset given, bit 1 the daylight flag given, bit 2 daylight time on).
- Op 1, the GPS driver: x1 latitude, x2 longitude, in 2^-23 degrees.
- Op 2, Settings: this device's choice, x2 1 automatic, x3/x4 its id by hand (length 0: none).
- Op 3, Settings: x2 1 follow the identity's choice.
- Op 4, the identity service (fgtw): x1 0 by hand, 1 automatic, 2 forget; x2/x3 its id.
- Op 5: the state into x1/x2 (ferros_sys `zone::State`). No capability: the zone is what the clock shows whoever holds the device.
- Op 6: every zone's id into x1/x2, each its length then its bytes; returns the list's whole length.

Ops 2-4 keep the settings document and put the choice back when the vault refuses.

## Bench

- `ferros-bridge diag zone`: the zone in effect, both choices, what was found and what was heard (the position only as known or not).
- `diag "zone net 311 -28800 0"`: the network's word as the modem hands it (MCC, offset in seconds east, daylight flag).
- `diag "zone at 122580 -425640"`: a position in arc-seconds (Los Angeles, 34° 3' N 118° 14' W), as the GPS driver hands it.
- `diag "pref zone America/Los_Angeles"`, `pref zone auto`, `pref zone none`, `pref zone_follow 0`.
