# Pucks

Status: built 2026-11-18, not yet run on a device. The framework and six pucks were built 2026-11-17 on the owner's go ("plus a tide puck"; "GPS, compass, shit that's useful! Pucks! I'm DOWN!") with the display drawing a list of ops; on 2026-11-18 the owner chose squares instead ("What's wrong with supplying a diameter and having the kist return a square vector in sixteen bit that ferros will trim and place?"), and the touch rule, the clock as a real puck and basecalc as one ("Go for it. tides, proper signed clock, same way we do kists, and basecalc calculator! Well, a nice GUI puck version anyway."). Proposed 2026-11-11 ("tides on a puck (is that what we should call kists you can't open but are live like the clock and tide? Android calls them widgets I think?)"). The kist catalog's order (Claude memories/kist_catalog.md): the clock first, then tides, then a few other simple ones, then Photon.

## What a puck is

- **A puck is a kist that lives on the deck as a live circle and is never opened.** Its circle is its whole UI: no window, no page. The clock, the tide, basecalc's pad, the sun, the moon, a compass, GPS, the weather.
- **It is a kist like any other**: the person stows it, it is theirs and travels with them, its rights are asked once and default off (KISTS.md "Permissions"). Nothing about a puck is baked into the OS.
- **It is live**: what its circle shows follows the world (the time, the tide) with no tap.
- **Puck, never widget** (owner, 2026-11-12: "pucks distinct from kists being that you can't open a puck and it's basically a widget"): widget names fluor's controls (button, slider); a puck is its own kind beside kists on the deck and in Settings → Deck.

## Listed on the Deck

- **Settings → Deck** lists everything on the deck as a circle by kind: the kists, then the pucks, each row its name with "puck" or "kist" under it (KISTS.md "The deck and the card"). Contacts are to come as another kind.
- **A puck's card** (its row tapped): its name, "puck", its permissions (each a switch, as a kist's), and **remove from the deck** in two taps: an unstow, the stower taking it out of the person's list, its answers kept, its circle gone. No state and no stop.
- **The deck knows a puck by its signed declaration** (`puck`, below), never by its name: until 2026-11-18 a kist named Clock got the display's own clock face.

## How a puck draws: squares (owner, 2026-11-18)

The display asks; the puck answers with pixels; nothing is parsed.

- **The ask** (WATCH's answer): whether its circle is on the glass, whether it is large (at least half the panel's width across: room for a fuller face), its diameter in pixels (the side of the square to draw) and its turn (cos, sin, Q14: gravity's once the AoC reports it, else the camera's and the circle's own; owner: "Pucks turn, yes. the whole gravity thing"), and the taps that passed thru to it since.
- **The answer** (SHOW): a square of 16-bit VSF RGB g2t pixels (fluor's `G2t16`: α in bits 48-63, then each channel's premultiplied darkness of a g2t code, R in bits 32-47), `d` a side, drawn at the turn it names, lying in the puck's own slot. The g2t code is 2^16 · tanh(v / 2^14) of the working value v, so white sits at about 0.76 of the code and the codes above it are headroom to the working format's 16× white: a puck can show bright HDR (a gallery, a video) with nothing added.
- **Trim and place** (display puck.rs): the display takes a handed square at the top of its next render pass and answers then, so it never reads a square its puck is drawing; it samples the square into the circle's sprite (bilinear, the turn between the square's and the face's undone, scaled to the circle), trims it by the circle's C¹ edge, bakes it (bake.rs `g2t_row`) and composes it `under` with the deck as any icon sprite. What a puck drew outside its circle never reaches the glass: the circle is the puck's whole right to the screen.
- **Two squares, never overlapping** (ferros_puck::rt): a puck draws its next square anywhere in its slot clear of the one the display holds, at the slot's start when that is clear, else just past it.
- **Words in the system's face**: a puck draws text with ferros_glyphs, the display's own engine and Oxanium +glyphs (the dozenal digits included), so a puck's words look like the system's. basecalc keeps fluor's text, as its window does.

### Scaling and turning between squares

- While a pinch or a turn moves a circle, every scene tells the puck its new view when it has changed by a pixel of diameter or half a pixel of turn at the rim; meanwhile the display draws the newest square scaled and turned from the size and turn it was drawn at. A puck that keeps up is sharp thru a pinch; a slow one is soft until its next square lands. No rate is set and nothing is limited.
- A circle shown larger than 2^11 − 1 pixels across is drawn from the largest square the slot holds, scaled: two squares that size fill the slot's 64 MiB.

### When a puck draws

A puck draws only when its picture would change, and only while its circle is on the glass:
- **the ask**: a new size, turn, or large or not; the deck covered by an app or the screen off is the circle off the glass, so a puck under an app draws nothing;
- **its own deadline**: a moment its content sets (the tide's next high or low), or the next moment its time-anchored ops move half a pixel (ferros_puck::draw `next_change`): a wait on that moment, never an interval;
- **shown frames** (WATCH's `FRAMES`): for a puck whose picture moves continuously, every flip the deck shows while its circle is on the glass, 10 Hz at idle. A frame timer on shown frames, as post-lift motion is. The clock asks for these.
- **a tap**, for a puck that acts on one (basecalc).

### The deck stays under a screen

The circles on the deck never cover more than the screen (owner, 2026-11-18: "the total area of the entire deck is always less than the screen size so you'll never have a MASSIVE puck problem because it will be the only one drawing"). A puck zoomed to fill the glass is the only one on it; every other is off the glass and drawing nothing. Each slot gives frames back when it holds more than four times what two squares of its circle's size need.

## Touch (owner, 2026-11-18)

"Tap/drag with one finger moves it, with two scales/rotates and a simple tap passes thru so we can make our dozenal calculator work too."
- One finger dragging a puck moves its circle, two fingers in it move, size and turn it: the deck's own gestures on every circle, kept in `map.place.*`.
- A tap (down and up without moving past the slop) on a puck goes to the puck: the map sends it to the display (op 74, scene index and screen pixel), which turns it into the face's own frame, Q14 of the radius from the centre, and answers the puck's held WATCH with it. Nothing flies, nothing takes focus: a puck is never opened.
- A puck whose wants are not yet answered asks them on its first tap, as a kist does.

## Declaring a puck, its slot, its right

- **The signed want `puck`** (right 12 in `ferros_kist::RIGHTS`, never asked) in the capsule's declaration makes a kist a puck. The deck binds each puck's identity to its circle (display op 73, the system's alone); every call on the `puck` endpoint carries the caller's identity and square in the kernel's stamp (process.rs `puck_stamp`, 72 bytes), so a puck can name neither another circle nor another's pixels.
- **Its square's slot** (kernel, 2026-11-18): in its surface's slot (grant 3) a slot of the pucks' window (cachemap `PUCK_VA`, the GiB above the surfaces', 16 slots of 64 MiB of addresses), with no frames until it asks; SYS_DMA x1 = 6 (ferros_sys `puck_size`) resizes it to hold a number of bytes, larger or smaller, in whole frames of the pool, its base never moving; the frames go back at the puck's end. Every block of a slot held maps a frame of the puck's or the blank frame, so the display's read of a square whose puck ended or shrank lands on nothing anyone owns. The display hears the slots' owners on a bell (SYS_BELL 14, `BELL_PUCKS`, four bits a slot) and reads a square only under the generation its puck's call named. A puck holds no surface slot of the sixteen, none of a surface's 32 MiB, and no GPU block.
- **Its other rights** are its data's: a place (`gps`, asked), the network (`udp`, the weather's), and its own vault domain, as every kist has.

## Lifecycle

- The deck launches every answered puck once a person has attested (the map runs only then), each with its granted rights; one shot dead at lock, like every process.
- A puck off the glass sleeps on its watch: no asks, no frames, no work.
- A puck that ends abnormally glows stopped (KISTS.md "Kist states"); its circle shows the deck's plane until it is up again.

## The pucks

| puck | what it shows | engine | draws when | wants | signed by |
|---|---|---|---|---|---|
| Clock (payloads/clock) | rings that sweep from 12:00, ticks black once passed, the time in the person's base; six dozenal places when large | the display's face of 2026-10-23 to 26, drawn per pixel into the square | every shown frame | puck | the owner's key, the clock's option of 2026-11-10, version 1 |
| basecalc (std/basecalc `--features puck`) | basecalc's pad: the display, the history, the keys whose digit block follows the base | basecalc's own engine (rug over GMP, MPFR, MPC), its settings and history in its vault domain | a tap that presses a key, a new size | puck | the owner's key, basecalc's option of 2026-11-17, version 0.2.0 |
| Tide | the height now, the day's curve with now on it, the next high and low | ferros_tide (NOAA's 37 constituents; NOAA tables within 2.1 min and 1.3 cm printed, 3.3 min and 3.0 cm live) | the next high or low, or now moving half a pixel along the curve | puck, gps | the ferros project key |
| Sun | the sun's levels, noon, the day's length | ferros_sky, Meeus ch. 25 and 15 (USNO within 0.46 min) | any crossing of the sun's levels, noon, the day's end | puck, gps | the ferros project key |
| Moon | the phase, rise and set, the next quarter | ferros_sky, Meeus ch. 47, 48, 49 (USNO's 2026 quarters within 0.78 min, rise and set within 0.48 min) | rise, set, the next quarter, the day's end, the shown percent turning over | puck, gps | the ferros project key |
| Compass | true north | ferros_compass, WMM2025 (all 100 NOAA test values, D and I within 0.005°) | the magnetometers' own 2^6 ms, only while shown | puck, gps | the ferros project key |
| GPS | the satellites and how well they place the device | the driver's FIX, SATS and WATCH thru the `gps` right and claim | the driver's doorbell, only while large (the radio on only then) | puck, gps | the ferros project key |
| Weather | the forecast where the person is, to 10 km | MET Norway thru fgtw.org (branch `pucks-weather`, not deployed) | the forecast's next hour or update, only while shown | puck, gps, udp | the ferros project key |

- **Faces** (ferros_faces, the six of 2026-11-17): each a list of ferros_puck's drawing ops in the circle's own units, drawn by the puck itself into its square (ferros_puck::square), the fuller face when large; no human words but a station's name.
- **Capsules**: tools/clock-kist.sh, tools/basecalc-kist.sh (the puck by default, `--window` the pad in a window instead: one option, one vault domain), tools/pucks-kist.sh (the six, masked, the ferros project key, options minted 2026-11-17).
- **basecalc's build** needs two sibling branches not yet on their repos' main: basecalc `ferros-engine` (the engine as a library) and ferros-libc `basecalc-file` (FILE and abort for the C), and builds GMP, MPFR and MPC for ferros itself (tools/basecalc-build.sh).

### The location step

The place-bound pucks (tide, sun, moon, compass, weather) share one: the ~10 km cell of the last GPS fix (ferros_weather's grid: rows 2^-12 of a turn of latitude, each row cut into 1 + floor(4096 cos(lat)) columns, a place always under 7 km from its cell's centre), kept in the puck's own vault domain as a VSF document (`place`: row, column, when), so a start has its place without the radio; a fresh fix only when the person granted the puck GPS, its circle is shown and the kept cell is older than 2^12 s, the claim released at the first fix or after 2^7 s (LOCATION.md's policy). The finer fix never leaves the process's memory. Without GPS granted there is no place yet: LOCATION.md's passive `place` right (not built) is where a picked or cell-derived place would come from.

### The tide puck

- **Its face**: the height now, rising or falling (an arrow, or the curve's slope at the marker), the day's curve with now on it, and the next high and the next low as moments with their heights, in the person's base. Colours in VSF RGB, picked by wavelength as the glows are (ferros_glow): the water's line and its fill are the owner's pick (question below).
- **Its work**: once per deadline, `Face::at(tide, now)` (ferros_tide), drawn into its square. Nothing between but a new view, or now moving half a pixel along the curve.
- **Its station**: the nearest in its catalog to the person's last fix, by the largest dot product of unit vectors (no seam at the antimeridian or the poles), on the device only. The place is never printed, logged, stored by the puck or sent.
- **Its data rides with it, whole**: every station, inside the capsule (or one data file fetched whole), so no request ever names a station near the person. NOAA's 1374 harmonic stations at about 3.7 KB each are about 5 MB as station documents today; a compact set form (one document, constituents as a packed table) would be a fraction of that.

- **The tide's station**: the nearest in the bundled catalog by the largest dot product of unit vectors, one station, never a blend (a tide's phase changes across a coast; an average of two stations is no tide anywhere; NOAA's own practice is one reference station per place), its name and distance shown when large. The owner's inksurf picks its station by a hard-coded id (Bremerton) and has no location code; its engine is the same method on that one station, so the puck uses ferros_tide (validated at four stations, datum handling, stations as VSF) and takes inksurf's look (ferros_faces::tide): the fixed LAT..HAT scale (here mean sea level plus and less the sum of the amplitudes), water filled under the curve, the high's words below the water and the low's above it, nights behind it.

## What is built: the tide engine (2026-11-11)

- **ferros_tide** (no_std, libm): h(t) = Z0 + Σ f H cos(V + u − G) over NOAA CO-OPS's 37 constituents, in NOAA's order. V from the mean longitudes of the Moon, the Sun, the lunar and solar perigees and the node (Meeus) and the mean sun's hour angle, by each constituent's Doodson numbers; f and u from the node by Schureman (SP 98), every angle continuous. Eagle time at the boundary, converted inside to days from J2000. Highs and lows by sampling h′ every 2^8 s and bisecting each change of sign; `next`; the puck's `Face`.
- **Validated** against NOAA's published highs and lows for 2026-11-01 to 07 at San Francisco, The Battery, Seattle and Honolulu (semidiurnal to strongly mixed), every event found and none extra:
  - as printed tables take the constituents (V0 at the year's start, f and u at its middle; `Frame::printed`): worst 2.1 min and 1.3 cm, means 0.3 to 0.7 min and 0.2 to 0.8 cm;
  - as the puck takes them, live (`Frame::at`, smooth across any boundary): worst 3.3 min and 3.0 cm (Seattle, its large diurnal tide, where the year-middle f and u differ most from the moment's).
  - The multiples of 90° the sources disagree on (the diurnal constituents' sign, S1, M1) are pinned by a test: no quarter or half turn on any one constituent fits the four stations better.
- **Stations as VSF** (ferros_tide `station`, feature `vsf`): a station document (`station`: id, name, position as a Dymaxion `w`, datum, msl_um, source, licence; a `constituent` section each: name, amplitude_um, phase_q32) and a catalog (`catalog`, then `station`: id, name, position), every value read by name and type marker.
- **tools/tide-station** (host): NOAA's JSON to a station document, the catalog from NOAA's list or from documents, `show`, `predict`. fetch.sh fetches with curl; fixtures.sh writes ferros_tide/stations/ (the four stations and their catalog).
- **payloads/tidetest**: the data path on the device without drawing and without anyone's place: the bundled catalog and documents parsed, each station found as the nearest to its own place, its face computed, each step timed. Builds (payload-check passes) and passes the sandbox checker masked (48569 words, 0 refused). Not run on a device yet.

## What changes on the device

Stored together with a flash of the matching kernel (Claude memories/stored_capsules_need_flashed_kernel.md): the kernel (the `puck` right and its stamp, init's slot 9 the display's `puck` endpoint, the pucks' window, SYS_DMA x1 = 6, BELL_PUCKS, facts 88-89 for the compass), and the capsules display, map, init, kists (ferros_kist's `puck` right) and sensors (the magnetometers). Then the clock, basecalc and the six pucks are stowed like any kist; the clock's version 1 replaces the clock kist of 2026-11-10 (the same option).

## Not built

- The display's own clock is not yet deleted: nothing names it any more (the deck no longer gives icon G#80 to a kist named Clock), and it goes once the clock puck is seen on the glass (owner: "make another clock proper, then delete the weird wired in one"). The console's corner clock keeps `clock_state`.
- Frames for a moving puck come only with the 10 Hz temporal flips; with temporal dithering off, a `FRAMES` puck is drawn only when its view changes.
- A puck's square sampled bilinear only: a circle much smaller than its square while a pinch outruns the puck aliases until the next square.
- A doorbell on a sensor sample (the compass waits the sample period while shown).
- A picked place, and LOCATION.md's passive `place` right.
- The weather answer signed by fgtw.org's key.
- The full NOAA station set: the tide capsule carries the four fixtures.

## Sources and licences

- **NOAA CO-OPS** (US): harmonic constants, datums and predictions at api.tidesandcurrents.noaa.gov. Works of the US Government, public domain in the US (17 USC 105): the fixtures here and any full set built from them can ship in a capsule.
- **LINZ** (Aotearoa): publishes tide predictions for its standard ports, as its data generally, under CC BY 4.0 (attribution; to be confirmed for the tide files). Whether its harmonic constants themselves are published for reuse is to be checked before relying on them; predictions alone do not drive this engine.
- **Others to check before use**: TICON (a global compilation of tide-gauge constituents; its licence to be read), national hydrographic offices (the UK's Admiralty data is licensed commercially, not free to ship). Each station document carries its own `source` and `licence`, so a set may mix sources and say so per station.

## Open questions (owner)

Answered: the word (puck, 2026-11-12); the drawing path (squares, 2026-11-18); a tap (passes thru, 2026-11-18); tying a puck to its circle (the kernel's stamp and the deck's binding); declaring a puck (the signed `puck` want); when pucks run (from attest, drawing only while on the glass); the typeface ("whatever's cleanest": the display's, ferros_glyphs).

Still open:
0. **The tide's place.** A passive `place` right read from a place service that keeps the last place from GPS fixes and the serving cell (LOCATION.md, proposed 2026-11-11), or claiming GPS while shown (as built), or the zone's principal place?
1. **The station set.** All of NOAA's stations in the tide capsule (about 5 MB as documents, less once packed), or a region the person picks? And which other sources (LINZ, TICON) once their licences are checked?
2. **The tide's colours.** Wavelengths for the water's line, its fill and the marker.
3. **Heights' datum.** Above chart datum (MLLW, as NOAA prints them) or above mean sea level?
4. **basecalc, puck or window.** Built as a puck by default with the window still buildable (`--window`); both are basecalc's one identity, so only one is on the deck at a time.
