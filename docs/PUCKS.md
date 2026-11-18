# Pucks

Status: proposed 2026-11-11, drawing path decided 2026-11-18 (a square of pixels per circle, "How a puck draws"); nothing of it is built on the display, the map or init. Built: the tide engine, stations as VSF and a device probe of the data path ("What is built"). The owner, 2026-11-11: "tides on a puck (is that what we should call kists you can't open but are live like the clock and tide? Android calls them widgets I think?)". The kist catalog's order (Claude memories/kist_catalog.md): the clock first, then tides, then a few other simple ones, then Photon.

## What a puck is

- **A puck is a kist that lives on the map as a live circle and is never opened.** Its circle is its whole UI: no window, no page, no surface of its own on the glass. The clock is the first; tides the second.
- **It is a kist like any other**: the person stows it, it is theirs and travels with them, its rights are asked once and default off (KISTS.md "Permissions"). Nothing about a puck is baked into the OS.
- **It is live**: what its circle shows follows the world (the time, the tide) with no tap. A kist circle that shows only an icon until tapped is not a puck.
- **Android's word is widget**, and the code says widget today (display `WIDGET_CLOCK`, the map's `widget()`). Widget also names fluor's controls (button, slider: COMPOSITOR.md "Controls"), so puck keeps the two apart. If the owner takes the word: GLOSSARY.md gets it, and `widget` in the map and the display is renamed where it means a puck.

## listed on the Deck (2026-11-12)

The owner, 2026-11-12: "make sure the settings/items page lists kists and pucks ... and I think pucks distinct from kists being that you can't open a puck and it's basically a widget." So the word is taken (question 0 below), and a puck is its own kind on the deck beside kists, not a kind of app.

- **Settings → Deck** lists everything on the deck as a circle by kind: the kists, then the pucks, each row its name with "puck" or "kist" under it (KISTS.md "The deck and the card"). Contacts are to come as another kind.
- **A puck's card** (its row tapped): its name, "puck", what it shows (the clock: the time), its permissions (each a switch, as a kist's: the clock asks for none), and **remove from the deck** in two taps. Underneath that is an unstow: the stower takes it out of the person's list, its answers kept, and its circle goes. No state and no stop: a puck is never opened and, today, never a process.
- **How the map tells a puck** is still the clock's name (`widget`, "Clock"); the signed `puck` flag (question 5) replaces it when decided. The code's `widget` is not renamed yet: it and the display's `WIDGET_CLOCK` move to puck with that flag.

## How the clock fits today

- The clock kist's capsule (payloads/clock) has nothing to run. The display draws the face itself (`WIDGET_CLOCK`, scene icon G#80), from its own clock and the person's base, in whatever circle the map gives that icon.
- The map gives that icon to a kist **by its name** (`widget`: "Clock", any case), and never launches it (`launch` answers RUNNING), and its glow is always running.
- Its refresh rides frames the display already makes: the 10 Hz temporal-dither flips redraw the clock's rectangle into the phase the flip shows, and without dithering a once-a-second check redraws it when its digits change. No timer of the clock's own.
- So the clock is a puck in behaviour, drawn the first way below (the display knows its drawing). Two things do not carry over to other pucks: a name is not an identity (any developer's kist named Clock gets the display's clock face), and a puck that needs work done (the tide's engine) needs a process the clock never had.

## How a puck draws (owner, 2026-11-18)

The owner, 2026-11-18, on the drawing-ops list: "What's wrong with supplying a diameter and having the kist return a square vector in sixteen bit that ferros will trim and place?" Nothing that outweighs what it gives, so that is the path. The display asks; the puck answers with pixels; nothing is parsed.

- **The ask**: the display tells a puck the circle it will be shown in: its diameter in pixels, its turn (the camera's turn and the circle's own, the gravity turn every circle has), the centre's sub-pixel offset, the theme, and whether it is focused. The puck renders a square of that diameter, already turned and at that phase, so the display places it without resampling when the two agree.
- **The answer**: a square of 16-bit VSF RGB g2t pixels (fluor's `G2t16`, op 22's format 1: α and premultiplied darkness, the code 2^16 · tanh(v / 2^14) of the working value v, so white sits at about 0.76 of the code and the codes above it are headroom, to the working format's 16× white in light). A puck can show bright HDR (a gallery puck, a video) with nothing added. It draws with fluor, the one compositor, as any kist does: the house's squircles, glows and text, no second drawing vocabulary.
- **Trim and place**: the display masks the square by the circle (its own C¹ edge), places it, and composes it `under` with everything else, a layer like any other (COMPOSITOR.md rules 0 to 5). What the puck drew outside its circle never reaches the glass, so the circle is the puck's whole right to the screen.
- **A mailbox, so the screen never waits** (COMPOSITOR.md rule 1): the puck fills its back buffer and publishes it with one swap; the display takes the newest each frame. A puck slower than the frames drops its own frames, never the screen's.
- **Nothing to parse**: the display reads a buffer it composes, never a list a puck wrote. The worst a puck can do is show an ugly picture inside its own circle.

### Scaling and turning between renders

- While a pinch or a turn moves a circle, the display asks again with the new diameter and turn every shown frame, the newest ask replacing any the puck has not taken yet. Meanwhile it draws the puck's newest square scaled and turned from the size and turn it was rendered at to the circle's now: continuous in the gesture (a C¹ resample), softer by how far the puck lags. When the puck catches up the factor is 1 and it is sharp again.
- That is the whole balance: no rate the display sets, no limit. A puck as fast as the frames is sharp thru a pinch; a slow one is a little soft until the finger stops and its next square lands.

### When a puck renders

A puck renders only when its picture would change. Three things can say so; a puck takes the ones it needs:
- **the ask** above (size, turn, theme, focus, the person's base, zone or language changed): pushed by the display, every puck;
- **its own deadline**, a moment its content sets (the tide: the next high or low; a minute's digits: the next minute): a wait on that one moment, as a doorbell, never an interval;
- **shown frames**, for a puck whose picture moves continuously (the clock's sweeping rings): it asks for the display's frame bell and renders once per shown frame while it is on the glass, 10 Hz at idle (the flips the clock rides today), the panel's rate while focused (op 25). A frame timer on shown frames, as post-lift motion is, so a skipped frame advances it rather than slowing it.

A puck off the glass gets no asks and no frame bell: it renders nothing and spends no CPU, and its buffer can go.

### The whole deck stays under a screen

The circles on the deck never cover more than the screen (owner, 2026-11-18: "the total area of the entire deck is always less than the screen size so you'll never have a MASSIVE puck problem because it will be the only one drawing"). One puck zoomed to fill the glass is the only one on it; every other is off the glass and drawing nothing. So the pucks' pixels and their work together are bounded by about one screen's worth however many are stowed: the squares are 4/π of their circles and a circle cut by the glass's edge still has its whole square, so the bound is near two screens of pixels (1344 × 2992 at 8 bytes a pixel, about 31 MiB, per buffer), to be measured.

## What a tap does (owner's call)

Choices, none built:
0. Nothing: a puck shows, it does not act.
1. Focus, as the clock has it: the map flies to the circle and the display gives it the panel's rate (op 25); the puck, told it is focused, may show more (the tide: the week's highs and lows; the clock: two more places, as now).
2. A cycle of views, each tap telling the puck to show its next.
3. Open a page of its own: then it is an app, not a puck.

The map already treats a tap on the clock as 1 (it never launches it); 1 keeps a puck a puck.

## Rights

- **`puck`**: the right to fill one mailbox the display shows in one circle. Init makes it at launch from the person's arrangement, so a puck can never name another circle, and holds no `display`, `touch` or `surface`. How the kernel hands the mailbox over and how it is tied to the circle is open (question 4).
- **Its own vault domain**, as every kist has, for anything it keeps.
- **Per puck, what its data needs**: the tide, a place (below); the clock, nothing.
- **Declared in the signed capsule**: the `kist` section gains a flag `puck`, so the map knows a puck by its declaration, never by its name (question 5).

## Lifecycle

- Pucks are live, so init launches every stowed puck once a person has attested (not at a tap), each with its granted rights, as it launches a kist today. One shot dead at lock, like every process.
- A puck off the glass sleeps on its doorbell: no asks, no frames, no work (question 6).
- A puck that ends abnormally glows stopped (KISTS.md "Kist states"), and its circle keeps its last square, dimmed as stale rather than wrong.
- Memory: a puck's arc is small (tidetest's masked image is 246 KB, its heap 1 MiB) plus its two buffers at the size it is shown; N pucks are N processes.

## The clock, built fresh

The clock today is the display's own code (`WIDGET_CLOCK`, `clock_state`, the scope beside it), matched by name in the map, built to try out the deck's circles. It goes. A clock puck is built fresh on this path (owner, 2026-11-18: "ideally we just build this fresh and make another clock proper, then delete the weird wired in one"):
- payloads/clock becomes a real process, flagged `puck` in its signed capsule, rendering its face with fluor into its mailbox: the rings on shown frames, the digits in the person's base;
- once it shows the same face on the glass, the display's clock (`WIDGET_CLOCK`, `clock_state`, `clock_dirty`, `CLOCK_RECT` and its replay of the last camera op each second) and the map's `widget("Clock")` are deleted, and `widget` becomes puck where it means a puck;
- the light scope (`WIDGET_SCOPE`) is the same kind of display-drawn circle; it moves to a puck or goes with the clock (question 11).

## The tide puck

- **Its face**: the height now, rising or falling (an arrow, or the curve's slope at the marker), the day's curve with now on it, and the next high and the next low as moments with their heights, in the person's base. Colours in VSF RGB, picked by wavelength as the glows are (ferros_glow): the water's line and its fill are the owner's pick (question below).
- **Its work**: once per deadline, `Face::at(tide, now)` (ferros_tide), then one square into its mailbox. Nothing between, unless an ask (a zoom, a turn) or the marker's next pixel calls for a new square.
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

Answered:
- **0. The word.** Puck, a kind of its own beside kists (2026-11-12, "listed on the Deck" above); the code's `widget` is renamed with the `puck` flag.
- **1. The drawing path.** The display asks with a diameter and the puck answers with a square of 16-bit VSF RGB g2t pixels, trimmed and placed by the display (2026-11-18, "How a puck draws").
- **2. The op set.** None: there is no op list (2026-11-18).

Still open:
3. **A tap.** Nothing, focus (the map flies to it and the display gives it the panel's rate, as the clock has it; the puck told it is focused and free to show more), a cycle of views, or a page (then it is an app, not a puck)? Recommended: focus.
4. **Tying a puck to its mailbox.** The kernel holds 16 surface slots today (`SURFACE_SLOTS`), shared with app windows. Pucks from those slots, or a pool of their own sized by the deck's bound (near two screens of pixels)? And the ask and the frame bell: one endpoint per puck made by init, or the kernel telling the display the calling capsule?
5. **Declaring a puck.** A signed `puck` flag in the capsule's `kist` section, replacing the clock's match by name? Recommended: yes, and the fresh clock is the first capsule with it.
6. **When pucks run.** Launched at attest and asleep while off the glass (a first square ready the moment a circle comes on), or launched only while their circle is on the glass (no process at all off it, a blank circle until its first square)? Recommended: at attest, asleep off the glass.
7. **The tide's place.** Claiming GPS turns its radio on. A passive `place` right read from a place service that keeps the last place from GPS fixes and the serving cell (LOCATION.md, proposed 2026-11-11), claim GPS once at start, or the zone's principal place?
8. **The station set.** All of NOAA's stations in the tide capsule (about 5 MB as documents, less once packed), or a region the person picks (a choice that names a region, never a place)? And which other sources (LINZ, TICON) once their licences are checked?
9. **The tide's colours.** Wavelengths for the water's line, its fill below the line, and the marker.
10. **Heights' datum.** Above chart datum (MLLW, as NOAA prints them, where the lowest lows go below zero) or above mean sea level?
11. **The light scope.** A puck of its own on this path, or gone with the display's clock?
12. **The typeface.** fluor bundles Open Sans; the display's text is its own one typeface (SHELL.md). A puck's digits drawn with fluor's face, or the display's face shipped to kists?
