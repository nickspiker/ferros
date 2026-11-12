# Pucks

Status: proposed 2026-11-11 for the owner's review; nothing on the display, the map or init is built. Built: the tide engine, stations as VSF and a device probe of the data path ("What is built"). The owner, 2026-11-11: "tides on a puck (is that what we should call kists you can't open but are live like the clock and tide? Android calls them widgets I think?)". The kist catalog's order (Claude memories/kist_catalog.md): the clock first, then tides, then a few other simple ones, then Photon.

## What a puck is

- **A puck is a kist that lives on the map as a live circle and is never opened.** Its circle is its whole UI: no window, no page, no surface of its own on the glass. The clock is the first; tides the second.
- **It is a kist like any other**: the person installs it, it is theirs and travels with them, its rights are asked once and default off (KISTS.md "Permissions"). Nothing about a puck is baked into the OS.
- **It is live**: what its circle shows follows the world (the time, the tide) with no tap. A kist circle that shows only an icon until tapped is not a puck.
- **Android's word is widget**, and the code says widget today (display `WIDGET_CLOCK`, the map's `widget()`). Widget also names fluor's controls (button, slider: COMPOSITOR.md "Controls"), so puck keeps the two apart. If the owner takes the word: GLOSSARY.md gets it, and `widget` in the map and the display is renamed where it means a puck.

## listed on the Deck (2026-11-12)

The owner, 2026-11-12: "make sure the settings/items page lists kists and pucks ... and I think pucks distinct from kists being that you can't open a puck and it's basically a widget." So the word is taken (question 0 below), and a puck is its own kind on the home screen beside kists, not a kind of app.

- **Settings → Deck** lists everything on the home screen as a circle by kind: the kists, then the pucks, each row its name with "puck" or "kist" under it (KISTS.md "The deck and the card"). Contacts are to come as another kind.
- **A puck's card** (its row tapped): its name, "puck", what it shows (the clock: the time), its permissions (each a switch, as a kist's: the clock asks for none), and **remove from the home screen** in two taps. Underneath that is an unstow: the installer takes it out of the person's list, its answers kept, and its circle goes. No state and no stop: a puck is never opened and, today, never a process.
- **How the map tells a puck** is still the clock's name (`widget`, "Clock"); the signed `puck` flag (question 5) replaces it when decided. The code's `widget` is not renamed yet: it and the display's `WIDGET_CLOCK` move to puck with that flag.

## How the clock fits today

- The clock kist's capsule (payloads/clock) has nothing to run. The display draws the face itself (`WIDGET_CLOCK`, scene icon G#80), from its own clock and the person's base, in whatever circle the map gives that icon.
- The map gives that icon to a kist **by its name** (`widget`: "Clock", any case), and never launches it (`launch` answers RUNNING), and its glow is always running.
- Its refresh rides frames the display already makes: the 10 Hz temporal-dither flips redraw the clock's rectangle into the phase the flip shows, and without dithering a once-a-second check redraws it when its digits change. No timer of the clock's own.
- So the clock is a puck in behaviour, drawn the first way below (the display knows its drawing). Two things do not carry over to other pucks: a name is not an identity (any developer's kist named Clock gets the display's clock face), and a puck that needs work done (the tide's engine) needs a process the clock never had.

## How a puck draws

Three ways, each costed. "Frame" is a display frame; "the circle" is the puck's circle at whatever size the map's zoom gives it, from a few pixels to the whole screen.

| | (a) the display draws it from data | (b) the kist renders pixels into a surface | (c) the kist sends a list of drawing ops |
|---|---|---|---|
| what crosses | a few typed values (the clock: none) | the circle's pixels | a short list of ops in the circle's own units |
| who knows the drawing | the display, per puck | the kist | the kist; the display knows only the ops |
| a new puck needs | an OS update (display code) | nothing | nothing (until it needs an op the display lacks) |
| sharp at every zoom | yes | only at the size rendered; a pinch needs a new render each frame or a scaled (blurred) copy | yes: the display draws the ops at the circle's size |
| memory per puck | none | a surface at the largest size shown: a full-screen circle is 1344² pixels, about 14 MB at the working 8 bytes a pixel | the list (bytes) and the display's cached sprite of it, as an icon has today |
| cost per frame, puck unchanged | the clock's: its region redrawn at the flips | the surface composed under, as any layer | the cached sprite, as an icon; only the time-anchored parts (below) redrawn at the flips |
| cost when the picture changes | the display redraws it | the kist renders, then publishes (a mailbox swap) | the kist sends a new list (a call), the display re-rasterises it once |
| what the display must trust | its own code | a buffer it composes, nothing parsed | a list it parses: bounded, every op checked, as the region ops are today |

### Recommended: (c), with time-anchored ops and typed values

- **Any kist can be a puck** without the display learning it, which (a) cannot give: the clock's way does not scale past the handful of pucks ferros itself ships, and "no baked apps" says those are kists too.
- **Sharp at every zoom for free.** The ops are in the circle's own units (the radius is 1, its up the circle's up), so the display draws them at whatever size the map flies the circle to, as it draws every other circle. (b) needs a fresh render from the kist for every zoom step, a process round trip per frame while a finger pinches, or a blurred copy, and a surface the size of the screen per puck.
- **No timers in the puck.** Two kinds of op let the display move a puck's picture on its own clock, at the frames it already makes for the clock:
  - **time-anchored ops**: a value that is a function of the display's now, given as its anchor and its rate (a sweep at `(now − t0) / period`, a marker at now along a curve that spans [t0, t1]);
  - **typed values the display formats**: a moment (Eagle time) shown as wall time in the person's base and zone, a number with its unit in the person's base. A change of base, zone, daylight time or language then redraws every puck with no puck waking: the display already formats the clock this way.
  With these the clock becomes a puck with a list of ops and no process: its rings are sweeps, its centre a moment.
- **Refresh by push, nothing polled.** A puck sends a new list only when its picture changes in a way the ops above do not carry. The tide's words (the next high and low) change only when a high or a low passes: that moment is its one deadline, a wait until a time the content itself sets (about four a day), not an interval. Its station changes only when the person has moved far (below, the GPS question).
- **Under compositing, unchanged.** The puck's picture is one more layer, composed front to back by `under` like an icon's sprite: cached while cold (COMPOSITOR.md rule 5: 64 frames unchanged), its time-anchored parts redrawn clipped at the flips the clock already rides. The ops themselves draw front to back as the region ops do.
- **Cheap to carry.** The tide's curve as 64 heights with their slopes joined by cubic Hermite pieces is C¹ and within 0.13 mm of the true tide between samples (ferros_tide `Face`, its test); with a few text and shape ops a tide face is well under a 4 KiB call.
- **(b) stays the door for what ops cannot say**: a camera preview, a map tile. It is COMPOSITOR.md's mailbox layer, built for apps already; a puck asking for a surface would be an app in a circle, decided when one needs it.

### The ops, sketched (for the owner to cut)

A call from the puck to the display: `[op][generation u32][count]` then each op `[kind][len u16][body]`; coordinates Q14 of the radius from the centre; colours as the region ops take them (VSF RGB gamma 2 codes) or a theme index, so a puck can follow the theme. At most 2^6 ops and one call (4 KiB); anything past a bound, an unknown kind or a body of the wrong length refuses the whole list (the last good one stays).

- shapes: `disc`, `ring` (centre, radii, colour), `capsule` (two points, radius: the slider's track), `arc` (centre, radius, width, from and to as turns);
- `curve`: n samples of value and slope over [x0, x1] with a value range, drawn as cubic Hermite pieces of a given width and colour, optionally filled to a baseline;
- `text`: UTF-8, size and place in radius units, turned with the circle (op 19's renderer);
- time-anchored: `sweep` (an arc whose end is at `(now − t0) / period` of a turn), `marker` (a dot on a `curve` at now, its x mapped from [t0, t1]);
- typed values: `moment` (Eagle oscillations, shown as wall time in the person's base and zone: hours and minutes, or the clock's dozenal places), `number` (a value, its unit and how many places, in the person's base).

## What a tap does (owner's call)

Choices, none built:
0. Nothing: a puck shows, it does not act.
1. Focus, as the clock has it: the map flies to the circle and the display gives it the panel's rate (op 25); the puck may send a second, fuller list for its focused state (the tide: the week's highs and lows; the clock: two more places, as now).
2. A cycle of views, each tap the next list the puck sent.
3. Open a page of its own: then it is an app, not a puck.

The map already treats a tap on the clock as 1 (it never launches it); 1 keeps a puck a puck.

## Rights

- **`puck`**: a send right to the display that draws only into its own circle. The right is the circle: init makes it at launch from the person's arrangement, so a puck can never name another circle, and holds no `display`, `touch` or `surface`. (How the display ties the right to the circle: an endpoint per puck, or the kernel telling the display the caller's capsule; question below.)
- **Its own vault domain**, as every kist has, for anything it keeps.
- **Per puck, what its data needs**: the tide, a place (below); the clock, nothing.
- **Declared in the signed capsule**: the `kist` section gains a flag `puck`, so the map knows a puck by its declaration, never by its name. The clock's capsule is then rebuilt with it, and the map's `widget("Clock")` goes.

## Lifecycle

- Pucks are live, so init launches every installed puck once a person has attested (not at a tap), each with its granted rights, as it launches a kist today. One shot dead at lock, like every process.
- A puck that ends abnormally glows stopped (KISTS.md "Kist states"), and its circle keeps its last list, said stale (dimmed) rather than wrong.
- Memory: a puck's arc is small (tidetest's masked image is 246 KB, its heap 1 MiB), but N pucks are N processes.

## The tide puck

- **Its face**: the height now, rising or falling (an arrow, or the curve's slope at the marker), the day's curve with now on it, and the next high and the next low as moments with their heights, in the person's base. Colours in VSF RGB, picked by wavelength as the glows are (ferros_glow): the water's line and its fill are the owner's pick (question below).
- **Its work**: once per deadline, `Face::at(tide, now)` (ferros_tide), then one list to the display. Nothing between.
- **Its station**: the nearest in its catalog to the person's last fix, by the largest dot product of unit vectors (no seam at the antimeridian or the poles), on the device only. The place is never printed, logged, stored by the puck or sent.
- **Its data rides with it, whole**: every station, inside the capsule (or one data file fetched whole), so no request ever names a station near the person. NOAA's 1374 harmonic stations at about 3.7 KB each are about 5 MB as station documents today; a compact set form (one document, constituents as a packed table) would be a fraction of that.

## What is built (2026-11-11)

- **ferros_tide** (no_std, libm): h(t) = Z0 + Σ f H cos(V + u − G) over NOAA CO-OPS's 37 constituents, in NOAA's order. V from the mean longitudes of the Moon, the Sun, the lunar and solar perigees and the node (Meeus) and the mean sun's hour angle, by each constituent's Doodson numbers; f and u from the node by Schureman (SP 98), every angle continuous. Eagle time at the boundary, converted inside to days from J2000. Highs and lows by sampling h′ every 2^8 s and bisecting each change of sign; `next`; the puck's `Face`.
- **Validated** against NOAA's published highs and lows for 2026-11-01 to 07 at San Francisco, The Battery, Seattle and Honolulu (semidiurnal to strongly mixed), every event found and none extra:
  - as printed tables take the constituents (V0 at the year's start, f and u at its middle; `Frame::printed`): worst 2.1 min and 1.3 cm, means 0.3 to 0.7 min and 0.2 to 0.8 cm;
  - as the puck takes them, live (`Frame::at`, smooth across any boundary): worst 3.3 min and 3.0 cm (Seattle, its large diurnal tide, where the year-middle f and u differ most from the moment's).
  - The multiples of 90° the sources disagree on (the diurnal constituents' sign, S1, M1) are pinned by a test: no quarter or half turn on any one constituent fits the four stations better.
- **Stations as VSF** (ferros_tide `station`, feature `vsf`): a station document (`station`: id, name, position as a Dymaxion `w`, datum, msl_um, source, licence; a `constituent` section each: name, amplitude_um, phase_q32) and a catalog (`catalog`, then `station`: id, name, position), every value read by name and type marker.
- **tools/tide-station** (host): NOAA's JSON to a station document, the catalog from NOAA's list or from documents, `show`, `predict`. fetch.sh fetches with curl; fixtures.sh writes ferros_tide/stations/ (the four stations and their catalog).
- **payloads/tidetest**: the data path on the device without drawing and without anyone's place: the bundled catalog and documents parsed, each station found as the nearest to its own place, its face computed, each step timed. Builds (payload-check passes) and passes the sandbox checker masked (48569 words, 0 refused). Not run on a device yet.

## Sources and licences

- **NOAA CO-OPS** (US): harmonic constants, datums and predictions at api.tidesandcurrents.noaa.gov. Works of the US Government, public domain in the US (17 USC 105): the fixtures here and any full set built from them can ship in a capsule.
- **LINZ** (Aotearoa): publishes tide predictions for its standard ports, as its data generally, under CC BY 4.0 (attribution; to be confirmed for the tide files). Whether its harmonic constants themselves are published for reuse is to be checked before relying on them; predictions alone do not drive this engine.
- **Others to check before use**: TICON (a global compilation of tide-gauge constituents; its licence to be read), national hydrographic offices (the UK's Admiralty data is licensed commercially, not free to ship). Each station document carries its own `source` and `licence`, so a set may mix sources and say so per station.

## Open questions (owner)

0. **The word.** Puck for a live, never-opened kist circle? And rename the code's `widget` where it means a puck. Answered 2026-11-12: puck, a kind of its own beside kists ("listed on the Deck" above); the code's rename rides with the `puck` flag (5).
1. **The drawing path.** (c) as recommended, with time-anchored ops and display-formatted moments and numbers? Then the clock moves to it too (its capsule a list of ops, no display code of its own).
2. **The op set.** Which of the sketched ops, and their bounds (2^6 ops, one 4 KiB call)?
3. **A tap.** Nothing, focus (as the clock), a cycle of views, or a page?
4. **Tying a puck to its circle.** A `puck` endpoint per puck made by init, or the kernel telling the display the calling capsule?
5. **Declaring a puck.** A signed `puck` flag in the capsule's `kist` section, replacing the clock's match by name?
6. **When pucks run.** Launched at attest and alive while a person is present, or only while their circle is on the glass?
7. **The tide's place.** Claiming GPS turns its radio on. Should a puck read the last fix the GPS driver keeps (`gps.last`) without powering anything, by a new right ("last place", coarse), and follow it only when something else ran the GPS? Or claim GPS once at start and release at the first fix? Or the zone's principal place when there is no fix? Proposed in LOCATION.md (2026-11-11): a passive `place` right read from a place service that keeps the last place from GPS fixes and the serving cell, refreshed at most once an hour with the screen on.
8. **The station set.** Ship all of NOAA's stations in the tide capsule (about 5 MB as documents, less once packed), or a region the person picks (a choice that names a region, never a place)? And which other sources (LINZ, TICON) once their licences are checked?
9. **The tide's colours.** Wavelengths for the water's line, its fill below the line, and the marker.
10. **Heights' datum.** Show heights above chart datum (MLLW, as NOAA prints them, where the lowest lows go below zero) or above mean sea level?
