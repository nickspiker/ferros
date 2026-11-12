# Location

Status: proposed 2026-11-11 for the owner's review.
Built: the modem's serving cell on a `cell` endpoint with a doorbell, the cell-table lookup (`ferros_cell`, host-tested with synthetic cells) and its host converter (tools/cell-table); none of it run on a device yet ("What is built").
Not built: the place service, the passive right, the refresh policy, any cell table on any device.

The owner, 2026-11-11: "Location permissions request, not sure if it looks the same to the user but tides, weather, etc. can just use last cached location, unless something is actively using it. Still a permission but passive? and maybe we at least lock once an hour? and can we use cellular for rough position without doing a full 2 minute bringup and almanac construction?"

## The short of it

- **Two rights.** `place`, passive: the last place the device knew, with its age, its accuracy and where it came from; reading it never turns a radio on. `gps`, active: today's right, a claim that runs the GPS (GPS.md "Location is the person's").
- **One keeper of the last place**, a place service: it takes every GPS fix anyone's claim produced, and the serving cell's place from an offline table, and keeps the best recent one in the person's space.
- **The refresh**: at most one GPS fix an hour, only while the screen is on, only while a granted passive reader needs better than the cell gives, and never when the cell already answers well enough.
- **Cellular first**: the modem knows its serving cell for free while it is registered; an offline table on the device turns it into a place good to its cell's range (hundreds of metres in a city, kilometres outside), with no GPS, no almanac and no request that names a cell.
- **GPS warm starts do not exist today**: every session starts the chip from nothing (its RAM firmware, its orbits); what makes the hourly fix cheap is a fresh LTO file in the vault, not a kept ephemeris ("GPS: what a fix costs").

## Two rights

| | `place` (passive) | `gps` (active) |
|---|---|---|
| what the kist gets | the last place: latitude, longitude, error radius, age, source (GPS, cell, zone) | the GPS driver's live fixes while it claims |
| radio cost of a read | none: a read is a lookup in the place service | the GPS on for as long as the claim (39 mA tracking, GPS.md) |
| while locked | nothing (the place is identity class; the service runs only while a person is present) | nothing (GPS.md: no GPS while nobody has attested) |
| how fresh | as fresh as the device's last fix or cell change; the age says | now |
| precision | proposed: rounded to what passive readers need (below, open question 2) | the fix |

- **Declared in the capsule like every want** (KISTS.md "Permissions"): `place` with the developer's reason, a new right code beside `gps` in `ferros_kist::RIGHTS`, its person's words "where you were last" (and `gps` keeps "where you are").
- **Asked separately, never one for the other.** The first-launch screen shows each unanswered want as its own switch with its reason under it, both default off. A kist that declares both (a map app) gets two switches; a tide or weather kist declares `place` alone and never touches the radio. Granting `gps` does not grant `place`, and the reverse: a passive reader that wants a fresh fix has to ask for the radio in words the person sees.
- **Proposed wording** (ferros_lang, for the owner to cut): `place` "where you were last (no radio turns on for it)"; `gps` "where you are now (turns GPS on while it looks)".
- **Grants are the rights** (KISTS.md rule 0): `place` is a send right to the place service's `place` endpoint, made by the kernel at launch; a kist without it has an empty slot and no way to ask.

### The `place` endpoint (sketch)

- `0` READ → `valid u8`, `lat i32`, `lon i32` (2^-23 degrees, the GPS fix's unit), `err u32` (m), `age s u32`, `source u8` (0 GPS, 1 cell, 2 the zone's principal place). Nothing at all when there is no place yet.
- `1` WATCH `count u32` → the same, held until the place moves by more than its own error radius or its source improves (a cell place replaced by a GPS one). The tide puck holds one and asks for its station again only when it rings: no timer in the puck (PUCKS.md "Refresh by push").

### How it looks on the console and the pages

- **An active claim** is what it is today: the console's GPS circle shows satellites while the driver runs, the GPS page and the permissions page say who claims it ("using now", SYS_CLAIM op 2).
- **A passive read shows nothing on the console**: no radio is on and no power is spent, so the circle has nothing true to draw. The permissions page lists, per kist with `place`, when it last read the place (written by the service at each read, an event, never a clock), so the person can see who looks.
- **The service's own refresh is a claim** like any other, listed as the system's (zeros, as op 2 lists system processes), so the satellites show while it runs and the person sees the hourly fix happen.

## The place service

A system process (proposed `payloads/place`), started by the kernel once a person has attested and killed at lock, with its own grant list as the GPS driver and the kists stower have (init's grants page is full at 108). Its grants:

- serve `place`;
- use `cell` (the modem's serving cell, below);
- use `gps` (its claims and the driver's WATCH and FIX: SYS_CLAIM op 0 takes the `gps` send right as proof);
- the claims' bell (SYS_CLAIM op 4, today the Settings holder's only: it learns when anyone's claim starts the driver, and then holds the driver's WATCH to take every fix without a claim of its own);
- its vault scope `place.`: the last place (identity class, as `gps.last`), and the cell tables (class: open question 6).

What it keeps, `place.last`, one VSF document: section `place` with `at` (`w`, the place as a Dymaxion coordinate), `err_m` (u), `source` (u), `when` (e, Eagle time of the fix or the cell reading). Every change hits disk.
It never sends a place anywhere and logs none (its console lines name sources and ages only, never a position, a cell or an area).

Which place wins: the newest whose error is no worse than the place it replaces, or any place once the kept one is older than its usefulness (an hour for a GPS fix, as long as the cell is unchanged for a cell place).

## The refresh: at most once an hour, screen on

The owner's "maybe we at least lock once an hour". Proposed:

0. **Events first, no clock.** The place moves when something happens: the serving cell changes (the modem's doorbell), or a fix arrives from anyone's claim (the driver's doorbell). With a table that holds the serving cell, most moves are caught this way with no radio of the service's own.
1. **A need, or nothing.** Each granted passive reader declares the accuracy it needs in its capsule (a tide station within 2^14 m, about 16 km; weather within 2^12 m). The service's need is the tightest among stowed kists holding `place`. No passive reader stowed: no refresh at all, ever.
2. **The cheapest source that meets the need.** If the serving cell's place meets it (its range under the need), that is the refresh: no GPS. Only when the cell's place is coarser than the need, or the serving cell is not in the table, does the service claim GPS.
3. **The deadline.** The service claims GPS when, and only when: the screen is on, a need exists that the cell does not meet, and the kept place is older than `1 << 12` s (68 minutes; the owner's "an hour" as a power of two). It releases the claim at the first fix whose error meets the need, or after `1 << 7` s (128 s) without one.
4. **Screen off: no deadline at all.** A refresh overdue while the screen was off happens at the next screen on, which is an event (a bell on the screen state: the kernel has the preference, not yet a bell for it; one is needed). So the hourly fix runs only for a screen that stays on longer than an hour, and otherwise at a screen-on.

**Why a timer is allowed here** (feedback_no_polling: none unless proven necessary): this is a deadline the content sets, not an interval. The place's age is a fact about the place; "older than an hour and someone needs better than the cell" is a moment computed once from the place's own time and re-armed only when a new place is kept, exactly as the zone's next DST change (TIME.md) and the tide's next high (PUCKS.md) are. No reader, no need, or a good cell place: no deadline exists. The 128 s cap is the other side: indoors the chip has gone 300 s with no fix (GPS.md), and a refresh nobody asked for must not hold the radio for that.

## Sources, cheapest first

| source | cost to the device | accuracy | what it needs |
|---|---|---|---|
| cellular serving cell | none while the modem is registered (it already knows its cell; no request goes out for it) | the cell's range: hundreds of m to tens of km | an offline cell table for the region |
| WiFi (later) | a passive scan the WiFi driver may already be doing | tens of m | a table of access points by address: none open to ship (below), and the most person-revealing of all tables |
| GPS with a fresh LTO | the chip for about 11 s after the 8 s replay (GPS.md: 11.2 s with the capture-night LTO) | metres | the LTO file in the vault (not done: "GPS") |
| GPS with ferros's last fix only | about 92 s (GPS.md, measured, no LTO) | metres | the last fix (`gps.last`) |
| GPS from the zone's place | minutes (166 s once, nothing in 300 s once, without the LTO) | metres | the zone in effect |
| assistance from a server (SUPL, Google's) | a request that names the serving cell or a rough place | | never: a request that says where the person is |

The owner's "2 minute bringup and almanac construction" is the GPS rows without an LTO: the chip decodes the satellites' own orbits off the air (a satellite's ephemeris takes at least 18 s of clean signal to receive, about 30 s in practice), which is why the first fix takes minutes, not seconds.

## Cellular

### What the modem knows

Read from the vendor's own reader (vendor.radio.protocol.sit.stream.so, `ProtocolNetDataRegStateAdapter`, disassembled 2026-11-11 from the factory vendor image on Harbor) and set against what the driver already sees:

| what | where | in the driver |
|---|---|---|
| registration state, reject cause, technology | SIT_GET_PS_REG_STATE (G#0701) reply, parameters 0, 1, 3 | read since 2026-10-29 (`reg_last`, the cellular tracks) |
| LAC (u16), cell id (u32), PSC | parameters 4, 8, 12 | **now** (`cell`) |
| TAC (u32), PCI (u32), ECI (u32), CSG id (u32) | parameters 13, 17, 21, 25 | **now** |
| timing advance (u32, all ones unknown) | parameter 29 | **now** |
| EARFCN (u32) | parameter 35, replies of 39 bytes or more | **now** |
| EN-DC available, DCNR restricted, NR available | parameters 39, 40, 41 (42 bytes or more) | not read |
| NR cell NCI (u64) | parameter 42 (62 bytes or more) | **now** |
| NR PCI, TAC, NR-ARFCN | parameters 50, 54, 58, when byte 3 is 21 | **now** |
| MCC, MNC | SIT_GET_OPERATOR (G#0702) reply: six ASCII digits, `#` in the sixth for a two-digit MNC; also NITZ's network field and IND_MCC | **now** (operator reply, asked as the data stack registers); the MCC alone already went to the zone (TIME.md) |
| RSRP, RSRQ, SNR of the serving cell | IND_SIGNAL_STRENGTH (G#0906) and the G#0900 reply | measures since 2026-11-03 |
| timing advance (second copy), CQI | the same signal words (LteSignalStrength's order) | inferred, not checked |
| neighbours (PCI, EARFCN, RSRP each) | IND_CELL_INFO_LIST (G#070F), the vendor's `CellInfoListParserV1-V5` | arrives, not decoded |
| carrier aggregation cells | IND_PHYSICAL_CHANNEL_CONFIG (G#0742) | arrives, not decoded |

**What changed in the driver** (payloads/modem, 2026-11-11):
- The data stack's registration reply is read whole (`Serving::parse`), each group of fields only when the reply is long enough for it, as the vendor's reader tests.
- SIT_GET_OPERATOR is asked once as the data stack becomes registered; its MCC and MNC kept.
- IND_NTW_STATE_CHANGED on the data stack asks the registration again at once (Android's RIL does the same), so a handover is seen when the CP says so, not at the 64 s backstop poll.
- All of it out of line (`cell_frame`): inline, the service loop outgrew the masked build's branch reach (`tbnz` fixups out of range).
- **Never logged**: the console still shows only the registration's first four bytes.
- **Served on `cell`** (grant slot 25, served on its own thread when the driver was spawned with `--serve cell`; tools/husky-modem.sh passes it last): `0` GET → 62 bytes (valid, change count, age, state, technology, MCC, MNC and its digits, LAC, cell id, TAC, ECI, PCI, EARFCN, timing advance, NCI, NR TAC, NR PCI, NR-ARFCN; payloads/modem main.rs header). `1` WATCH `count` → the same, held until the cell, the network or the service changes (the service's end answers with no cell). Four watches held at most.
- **init cannot grant it yet**: its page is full, and the modem is init's. Either the modem gets its own list from the kernel (as GPS, the vault, the stower have) or a slot is freed (open question 4).

**Not checked on a device**: the offsets are the vendor's reader's, not yet matched against a live reply (the driver prints none, by design). The check that keeps the rule: one session that prints, on the device's console only, which fields are present and whether the cell is found in a table, never the values; or, on rooted Android, the same moment's `dumpsys telephony.registry` cell identity against the driver's reply, both kept on Harbor.

**Open in the decode:** the registration's technology byte read `14` (G#14 = 20) while on NR at 15 s and `0E` (14) on LTE (MODEM-BOOT-RECON.md), but the vendor's reader takes its NR fields only at 21 (`toRadioTech` is in a library the capture lacks). On Verizon's EN-DC the anchor is LTE, so the LTE fields (TAC, ECI, EARFCN) are the ones a table matches anyway; whether 20 is NR or something else (the measures show it as TECH_NR today) waits on a reading.

### From a cell to a place: the table

`ferros_cell` (no_std, libm; feature `vsf` for the document):
- **The table** is columns, one row a cell: network (`mcc << 16 | mnc`), cell (ECI, NCI, or cell id), area (TAC or LAC), radio, latitude and longitude in 2^-23 degrees, range in metres. Sorted by (network, cell, area, radio), no key twice, checked once as it is read (`Cells::check`); a cell is found by binary search.
- **Found** by network and identity, of the same radio, in the area when the area is known. An LTE ECI or NR NCI is unique in its network, so it needs no area; a GSM or UMTS id repeats across areas, and without the area two rows answer: no place rather than a guess.
- **The error** is the row's range (or the table's default for a cell with none). A table of sites (antenna positions) also takes LTE's timing advance: (ta + 1) × 78.071 m (N_TA = 16 T_s a step, TS 36.213) is how far the device can be from the antenna, and the error is the smaller of the two discs. OpenCelliD's positions are centroids of where measurements were made, not antennas, so the timing advance is not used with them.
- **Neighbours later**: once G#070F is decoded, each neighbour's PCI and EARFCN can be found among cells near the serving one, and a weighted centroid narrows the place. Not built: the list is not decoded.
- **As VSF**: one complete VSF file, section `cells`: `region`, `source`, `licence` (a), `kind`, `default_range_m`, `count` (u), and the seven columns as typed tensors (`plmn` t_u5, `cell` t_u6, `area` t_u5, `radio` t_u3, `lat` and `lon` t_i5, `range_m` t_u5). Read by name and type marker, the provenance hash verified, refused whole on any mismatch.
- **Size**: 29 bytes a cell, measured (a synthetic 65,536-cell table is 1,901,077 bytes). OpenCelliD counted 35.5 million cells worldwide in 2017 (Wikipedia; no newer count found), so all of it is about 1 GB; a country is a fraction, a state a fraction of that. The real numbers come from the converter (it prints count and bytes per region) once an export is fetched; LTE and NR only (`--radio LTE,NR`) and `--min-samples 2` cut it further.
- **On the device**: a region's table read into the place service once a session and searched in memory (a state's few MB thru the vault's 4 KiB transfers: under a second at the rate Mahere's 47 MB capsule was read, 3.8 s, KISTS.md).
  A table split by tracking area (one small document per network and TAC, so a lookup reads tens of KB) is the next step if whole regions prove too big.

### The tables' sources and their licences

- **OpenCelliD** (Unwired Labs, opencellid.org): the largest open cell database. Its data is licensed **CC BY-SA 4.0**; its downloads page asks that "products or services using data derived from OpenCelliD need to visibly credit OpenCelliD", and downloads need an account's API token.
  - A subset may be redistributed: a table cut from it is adapted material, shared under CC BY-SA 4.0 with attribution and the licence named. The converter writes that into the table's `licence` field.
  - What it binds: the table itself stays BY-SA wherever it goes; the code that reads it does not become BY-SA (a separate work beside it, not an adaptation), but a capsule that bundles the table carries the table under BY-SA. ferros's repository is Apache-2.0: no OpenCelliD data goes into the repository.
  - The credit: wherever a place from it is shown, "cell data: OpenCelliD (CC BY-SA 4.0)". The place service can pass the source with every place for that.
  - The account terms (the token) are the website's, separate from the data licence; not read here. Owner's question 0.
- **Mozilla Location Service** shut down in 2024; its cell exports were public domain while it ran. Archived copies age quickly (cells change), and none was checked here.
- **beaconDB** (the community successor to MLS) plans public-domain dumps; none published as of this writing.
- **Regulators' site registers** (antenna sites, not cell identities): they would make a `site` table only paired with a cell-to-site mapping, which only crowd data has. Not pursued.
- **The device's own learning**: every GPS fix pairs with the serving cell it was taken under, so the device can build its own table where its person goes, with no outside data. That table is a record of where the person has been: identity class, never synced in the clear, and opt-in. Owner's question 5.

### Privacy

- Every lookup is on the device, in a table it already holds; no request ever names a cell, an area or a place (a server-side geolocation call does exactly that, which is why none is used).
- Fetching a table names only the region the person chose (a state, a country, or all of it); the fetch can be made by the person's desktop, which converts it and hands the device VSF.
- The serving cell's identity is as precise as a position: the modem serves it only on `cell`, to the place service alone, and never logs it. A table's region hints where its person lives (open question 6).

## GPS: what a fix costs, and keeping orbits

**Nothing survives a session today.**
- The GPS driver runs only while something claims GPS (GPS.md "On demand"); when the claims go to none the kernel kills it and its power switch turns off with it: the chip's three rails off, nSTANDBY low (its only power and reset line).
- The chip's firmware is a RAM patch, downloaded at every power-up (the replay's first 554 records, 3.3 s), so nothing the chip learned is in it at the next start.
- What the driver keeps is the last fix's place (`gps.last`, identity class, 8 bytes: latitude and longitude, no time, no error). The time comes from the RTC. No ephemeris, almanac or LTO of the device's own is kept anywhere.
- The replay (`gps.replay`) still carries the capture night's LTO, from 2026-10-25: past its 7 days, it now helps nothing, and the session starts as `gps.replay.sky` would.

**So a fix an hour after the last one costs the same as one a week after**: about 92 s from the last fix's place (measured without LTO), minutes from the zone's place. The four-hour life of a broadcast ephemeris is never used, because the chip forgets it at power-off.

**What keeping orbits would take**, cheapest first:
0. **A fresh LTO in the vault** (GPS.md "Aiding: Broadcom's LTO"): seven days of every satellite's predicted orbit, 188 KB, fetched whole from the same open URL the Pixel uses (a fetch that names no place), stored as device class (it holds no place), uploaded at every session start. Measured with a fresh one: first fix 11.2 s. What remains to do: one capture on Android of gpsd's `1e83`/`1e82` upload with the file it sent, to learn the message layout. This makes the hourly fix about 11 s (plus the 8 s replay) at any age within the file's week: better than a 4-hour warm start, with nothing kept from the person's sessions. Its form in the vault is Broadcom's binary: carried inside a VSF document (`lto` section: the file as bytes, its fetch time `e`, its source `a`), owner's question 7.
1. **The chip's own ephemerides**, read back at the end of a session and uploaded at the next: in on-chip mode the chip reports none (only the fix and two companions, GPS.md "What the chip sends"); gpsd uploads ephemerides only in host mode ("GenerateAiding() skip because it's onchip ME"). Needs host-mode captures to learn both directions. Kept as VSF in the vault, device class (orbits hold no place, but their freshness says when the device last had a fix: arguably identity class).
2. **Public broadcast ephemerides** (the IGS daily navigation files), converted host-side to VSF: the same unknown upload message as 1, plus a source that needs no account. So one company's feed is never the only path (GPS.md already says so).
3. **Keeping the chip powered in standby**: not proposed. It changes GPS's on-demand meaning (the switch would stay on), its standby draw is unmeasured, and the RAM patch's survival is unknown.

## What is built (2026-11-11)

- **payloads/modem**: the serving cell parsed from the data stack's registration reply, the network from SIT_GET_OPERATOR, the registration asked again at IND_NTW_STATE_CHANGED, and the `cell` endpoint (GET, WATCH held until a change). Builds native; payload-check passes; the masked build passes the sandbox checker (99810 words, 0 refused). Not run on a device.
- **ferros_cell**: the table, the search, the error, the timing advance radius, distances on the sphere; the table as VSF (feature `vsf`). Eight host tests on synthetic cells (network 001-01, the test network; places on round degrees): found by network and identity, a repeated GSM id needing its area, the error from the range and from the timing advance at a site, the default range, a damaged table refused (order, lengths, radio, a place off the Earth, a byte changed after hashing), distances (a degree, across the antimeridian, antipodes). Builds for the bare target.
- **tools/cell-table** (host, its own workspace): `make` cuts OpenCelliD's CSV to a region's MCCs (and optionally radios, a box, a least number of samples), keeps the better of a cell listed twice, and writes the table; `show` reads it back as the device would and lists rows by network and radio, never a position. Run on synthetic CSVs only.
- No cell data of any kind is in the repository.

## Open questions (owner)

0. **OpenCelliD.** Ship tables made from it (BY-SA 4.0, the credit shown wherever its place is), and if so how: built into a capsule, fetched by the person's desktop with their own token and converted there, or not at all until a public-domain source (beaconDB) publishes? And are its website terms acceptable for an OS's users?
1. **Two rights, asked separately**, as proposed (`place` passive, `gps` active, neither implying the other)? And the words for each.
2. **The passive place's precision.** Always rounded to what passive readers need (a tide kist never learns the street), or the best the device has? Proposed: rounded to the tightest declared need, never finer than 2^8 m.
3. **The refresh** as proposed: events first, a GPS claim only when the cell is coarser than a declared need, at most once per `1 << 12` s, only with the screen on, released at the first good fix or after `1 << 7` s? And the accuracy a kist declares: a field of the `place` want in the capsule?
4. **Where `cell` comes from.** init's page is full: the modem started by the kernel with its own list (as GPS, the vault, the stower), or a slot freed in init's list? Same for the place service's own list.
5. **Learning cells from the person's own fixes**: wanted (identity class, opt-in, never synced in the clear), or never?
6. **The cell table's class.** Device class (readable before attest; its region hints where the device's person lives) or identity class (read only after attest, travels with the person)?
7. **The LTO in the vault**: Broadcom's binary carried inside a VSF document as bytes, fetched whole from the open URL, device class: acceptable under Rule -1, or does the owner want it decoded into typed VSF first (its layout is only partly known)?
8. **NR's technology code.** The registration showed 20 on NR; the vendor's reader expects 21 for its NR fields. Settle with one reading on a device (fields present, never values)?
9. **A screen bell.** The refresh waits on screen on; the kernel has the screen state as a preference but no bell for it. A new `SYS_BELL` source, or the place service told by the display?
