# Pucks

Status: built 2026-11-18 and running on the 8a since that morning (the clock and basecalc drawing; the six place-bound pucks stowed, waiting on their first tap's permissions); not yet on the Pro. The framework and six pucks were built 2026-11-17 on the owner's go ("plus a tide puck"; "GPS, compass, shit that's useful! Pucks! I'm DOWN!") with the display drawing a list of ops; on 2026-11-18 the owner chose squares instead ("What's wrong with supplying a diameter and having the kist return a square vector in sixteen bit that ferros will trim and place?"), and the touch rule, the clock as a real puck and basecalc as one ("Go for it. tides, proper signed clock, same way we do kists, and basecalc calculator! Well, a nice GUI puck version anyway."). Proposed 2026-11-11 ("tides on a puck (is that what we should call kists you can't open but are live like the clock and tide? Android calls them widgets I think?)"). The kist catalog's order (Claude memories/kist_catalog.md): the clock first, then tides, then a few other simple ones, then Photon.

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
- **Drawn in the arc, copied thru the handle**: a masked puck reaches its slot only thru its handle (SANDBOX.md "Handles"; the first clock on the 8a wrote its own grants page thru a plain pointer to the slot), so it draws each square in scratch at the top of its own arc (grown on demand to 2^26, given back when eight times too big) and copies it in with `Handle::write_words`. basecalc, native, writes its slot directly.
- **Words in the system's face**: a puck draws text with ferros_glyphs, the display's own engine and Oxanium +glyphs (the dozenal digits included), so a puck's words look like the system's. basecalc keeps fluor's text, as its window does.

### Scaling and turning between squares

- **A square costs its own circle.** At each flip, every puck whose newest square the phase about to show does not hold yet is drawn again there, its circle's box alone (`draw::CLIP` and `CLIP_PHASE`); the deck is drawn whole only for a binding, a puck gone, or on a display without the temporal flips. Measured on the 8a: 13.4 ms a box (a 656-pixel circle), where a whole redraw was 35 to 46 ms of scene and 34 to 41 ms of publish.
- **A square is sampled once, by every core** (2026-11-20). A new square is drawn into the canvas once; a later phase that lacks it while the canvas holds it whole only publishes the box from the canvas (puck.rs `canvas_holds`: a whole frame, or a redraw of the puck's own box, records the square it drew; a part of a box drawn from a newer square forgets it). The sampling is an item of the scene's list (`Item::Puck`), drawn by bands of rows across the publish workers and the render thread, each with its own rows. Measured on the Pro (a 188k-pixel box): before, 16 to 18 ms a box, all but 1 ms of it the sampling on the render thread alone, redone for each of the four phases; the display's console says each 64 boxes how many were redrawn and how many only published, and what a pass with boxes took.
- **Gravity turns the faces alone**: past half a pixel at the largest puck's rim, each puck hears its new turn and its box is owed to every phase, never a whole redraw.

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
- A tap (down and up without moving past the slop) on a puck goes to the puck: the map sends it to the display (op 74, scene index and screen pixel), which turns it into the face's own frame, Q14 of the radius from the centre, and answers the puck's held WATCH with it. The tap also centres the puck (owner, 2026-11-28: "a tap to center and rotate in frame so it's tangent left/right, unlike the full zoom in on a kist"): the camera flies so its circle sits at the panel's centre, upright, tangent to the left and right edges, and its face draws larger; a second tap is the face's alone. The helm's Deck flies back to the view before the first puck was centred (a pan or pinch of the plane forgets it). Nothing takes focus: a puck is still never opened.
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
| Tide | the owner's tide widget: the sky and water over a day centred on now, tapering to half a year, the sun's and moon's curves, the hours, now's time, the highs and lows, sunrise and sunset | ferros_tide (NOAA's 37 constituents; NOAA tables within 2.1 min and 1.3 cm printed, 3.3 min and 3.0 cm live), ferros_sky | now moving half a pixel at the centre, or now's time turning over | puck, gps | the ferros project key |
| Sun | the sun's levels, noon, the day's length | ferros_sky, Meeus ch. 25 and 15 (USNO within 0.46 min) | any crossing of the sun's levels, noon, the day's end | puck, gps | the ferros project key |
| Moon | the phase, rise and set, the next quarter | ferros_sky, Meeus ch. 47, 48, 49 (USNO's 2026 quarters within 0.78 min, rise and set within 0.48 min) | rise, set, the next quarter, the day's end, the shown percent turning over | puck, gps | the ferros project key |
| Compass | true north | ferros_compass, WMM2025 (all 100 NOAA test values, D and I within 0.005°) | the magnetometers' own 2^6 ms, only while shown | puck, gps | the ferros project key |
| GPS | the satellites and how well they place the device | the driver's FIX, SATS and WATCH thru the `gps` right and claim | the driver's doorbell, only while large (the radio on only then) | puck, gps | the ferros project key |
| Weather | the forecast where the person is, to 10 km | MET Norway thru fgtw.org (branch `pucks-weather`, not deployed) | the forecast's next hour or update, only while shown | puck, gps, udp | the ferros project key |
| Battery | the charge round the rim (0 to 100 %, clockwise) and large in the middle, the current (mA, negative draining) and the battery's temperature under it; the arc green (`USED`) while charging | the kernel's gauge sample thru the facts (`battery_pct`, `battery_ua`, `battery_temp_cc`) | the facts' bell (the share or the charging side changing), or 2^5 s after it last woke for the current and temperature the bell does not ring for, a shown number moving; only while shown | puck | the ferros project key (option blake3 of "ferros puck battery", 2026-11-28) |

- **Faces** (ferros_faces, the six of 2026-11-17): each a list of ferros_puck's drawing ops in the circle's own units, drawn by the puck itself into its square (ferros_puck::square), the fuller face when large; no human words but a station's name. The tide's since 2026-11-24 is painted per pixel instead (the widget's chart averages each column over its span, which no op list says).
- **Capsules**: tools/clock-kist.sh, tools/basecalc-kist.sh (the puck by default, `--window` the pad in a window instead: one option, one vault domain), tools/pucks-kist.sh (the six, masked, the ferros project key, options minted 2026-11-17).
- **basecalc's build** needs two sibling branches not yet on their repos' main: basecalc `ferros-engine` (the engine as a library) and ferros-libc `basecalc-file` (FILE and abort for the C), and builds GMP, MPFR and MPC for ferros itself (tools/basecalc-build.sh).

### The rotary face

ferros_faces::rotary (crates/faces/src/rotary.rs; owner, 2026-11-28: "rotary goodness around the temp readout, magnetic strength, stuff like that") is one face for every gauge-like puck: a dim track over three quarters of the circle near the rim, open at the bottom; the fill from the track's start to the value, clockwise or counterclockwise, with a short needle across the track; an optional second value as a thin arc inside it on the same scale; the value large in the middle and its notes smaller under it; the range's quarters as ticks in focus.
The angle is linear over the range and runs on past either end along a tanh into half the gap, C² at the joins and bounded by its own form (never a clamp), so a value outside the range is still seen moving and the fill is never more than 7/8 of a turn.
The battery is its first reader; the timer, the thermometer's dial and the compass's field-strength ring are to share it.

### The location step

The place-bound pucks (tide, sun, moon, compass, weather) share one: the ~10 km cell of the last GPS fix (ferros_weather's grid: rows 2^-12 of a turn of latitude, each row cut into 1 + floor(4096 cos(lat)) columns, a place always under 7 km from its cell's centre), kept in the puck's own vault domain as a VSF document (`place`: row, column, when), so a start has its place without the radio; a fresh fix only when the person granted the puck GPS, its circle is shown and the kept cell is older than 2^12 s, the claim released at the first fix or after 2^7 s (LOCATION.md's policy). The finer fix never leaves the process's memory. Without GPS granted there is no place yet: LOCATION.md's passive `place` right (not built) is where a picked or cell-derived place would come from.

### The tide puck

- **Its face is the owner's tide widget** (owner, 2026-11-24: "the tide one, the inksurf/tide android apk/widget is what the tides should look like on ferros"): inksurf's tide-widget/src/render.rs ported to the circle (ferros_faces::tide `paint`), drawn per pixel into the square as the clock draws its own; ferros_tide stays the engine (the nearest station by place) and ferros_sky gives the sun and the moon.
  - **The chart**: the sky and the water over time with now at the centre, a day across the middle half of the width tapering to half a year either side at the rim (the widget's axis), each column the average of its span in linear light, so months out the tide becomes a density and the curves a glow. The sun up lights the sky's red and green, the moon up its blue: yellow day, white day with the moon up, blue moonlit night, black night; the seam between day and night is hard, as the widget's is, at the sun's standard altitude (−0.833°, so it falls at the published sunrise and sunset; the widget's crossing of 0° falls about six minutes inside them). The water is cyan under a black waterline, on the station's fixed scale (mean sea level plus and less the sum of its amplitudes). The sun's and the moon's altitudes are curves (yellow and blue, black-edged), the horizon on the centreline, each at its year's highest at the top; the centreline is every channel flipped.
  - **The marks**: the now bar (white, black edges) with now's time straddling it; each high and low within half a day a bar flipping green and blue, its time straddling it, a high's in the lower third and a low's in the upper; sunrise and sunset within half a day, their times turned on the seam from the circle's top (sunrise reads up, sunset down). Each time's glyphs flip channels of the sky and water beneath them, so they read on any fill.
  - **The hours** (owner, 2026-11-24: "since it's round, I'd put the hour ticks as vertical lines using whatever channel and opacity make sense rather than single ticks"): full-height lines where the widget ticks the top and bottom edges, which a circle curves away. Each is a layer of its own under the bars: a neutral grey at the middle of the code range (gamma-2 code 0.5) in all three channels, the one colour that reads on the black night, the white sky and every primary between (darker on a lit sky, lighter on the night), at α 2^-2, so it lies behind the curves and the bars without competing; local midnight at α 2^-1 and twice as wide, the calendar's boundary standing out as the widget's longer tick does. Every line is a fixed span of time (a hair at the centre), narrowing toward the rim so the ink per hour stays even, antialiased by ferros's C¹ edge per column.
  - **Times** in the person's base and zone: dozenal as the widget's dozenal edition (the day's twelfth, the bar, its twelfth; a trailing zil dropped, the twelfth then centred on the bar); decimal as its hourly edition, HH:MM on the 24-hour clock with the colon on the bar (the clock's h:mm am or pm made two lows half a day apart collide on a 600-pixel circle); hex the seconds since midnight to G#100 s, the bar before the last place. The zone is the one in force now across the whole chart.
  - **Sizes**: lines a hair (2^-9 of the diameter), words 2^-4 of it; nothing in pixels.
  - **At rest and in focus**: the same chart at any size, as the widget draws one chart at any size; in focus (large) the station's name and its distance besides, below now's time, the now bar opening around them.
  - **Compositing**: front to back with `under`: the words, the now bar, the high and low bars, the hours, then the sky, water and curves, opaque.
  - **Host render**: tools/tide-face draws the face for a station document, a place and a UTC moment to a VSF visual (the circle trimmed over black, as the deck trims it); tools/vsf2web makes it a web image.
- **Its work**: when its circle is shown, the face drawn whole, the year of samples behind its far columns kept between squares (2^15 instants on an absolute 2^10 s grid, made again only when now has moved 2^19 s or the station or place changed). Measured on the host (x86-64): the year and a 600-pixel face 150 ms, a 600-pixel face again 47 ms, 1200 pixels 132 ms.
- **On the Pro** (2026-11-24, forge 4842f4b2 flashed): Tide version 3 (capsule blake3 c52433e8…, G#126FAE bytes, the ferros project key) stowed over USB (`ferros send`, developer mode), its entry appended to kists.fgtw.org and the capsule beside it, version 2 kept in keys/husky/rollbacks-2026-11-24/puck_tide.before-widget.capsule.vsf. On the deck its circle is about 650 pixels across (under half the panel: the face at rest); a pinch injected with `ferros-bridge input pinch` inside it left its size as it was, so the large face (with the station) is seen on the host only so far.
- **When it draws**: the chart is centred on now, so the face is drawn again when now has moved half a pixel at the centre or now's time turns over in the person's base (a dozenal tenth of an hour, a minute, G#100 s), whichever is first, a moment its content sets, never an interval; at a 600-pixel circle half a pixel is 144 s.
- **Its station**: the nearest in its catalog to the person's last fix, by the largest dot product of unit vectors (no seam at the antimeridian or the poles), on the device only. The place is never printed, logged, stored by the puck or sent.
- **Its data rides with it, whole**: every station, inside the capsule (or one data file fetched whole), so no request ever names a station near the person. NOAA's 1374 harmonic stations at about 3.7 KB each are about 5 MB as station documents today; a compact set form (one document, constituents as a packed table) would be a fraction of that.

- **The tide's station**: the nearest in the bundled catalog by the largest dot product of unit vectors, one station, never a blend (a tide's phase changes across a coast; an average of two stations is no tide anywhere; NOAA's own practice is one reference station per place), its name and distance shown when large. The owner's inksurf picks its station by a hard-coded id (Bremerton) and has no location code; its engine is the same method on that one station, so the puck uses ferros_tide (validated at four stations, datum handling, stations as VSF) and takes inksurf's look (above).

## What is built: the tide engine (2026-11-11)

- **ferros_tide** (no_std, libm): h(t) = Z0 + Σ f H cos(V + u − G) over NOAA CO-OPS's 37 constituents, in NOAA's order. V from the mean longitudes of the Moon, the Sun, the lunar and solar perigees and the node (Meeus) and the mean sun's hour angle, by each constituent's Doodson numbers; f and u from the node by Schureman (SP 98), every angle continuous. Eagle time at the boundary, converted inside to days from J2000. Highs and lows by sampling h′ every 2^8 s and bisecting each change of sign; `next`; the puck's `Face`.
- **Validated** against NOAA's published highs and lows for 2026-11-01 to 07 at San Francisco, The Battery, Seattle and Honolulu (semidiurnal to strongly mixed), every event found and none extra:
  - as printed tables take the constituents (V0 at the year's start, f and u at its middle; `Frame::printed`): worst 2.1 min and 1.3 cm, means 0.3 to 0.7 min and 0.2 to 0.8 cm;
  - as the puck takes them, live (`Frame::at`, smooth across any boundary): worst 3.3 min and 3.0 cm (Seattle, its large diurnal tide, where the year-middle f and u differ most from the moment's).
  - The multiples of 90° the sources disagree on (the diurnal constituents' sign, S1, M1) are pinned by a test: no quarter or half turn on any one constituent fits the four stations better.
- **Stations as VSF** (ferros_tide `station`, feature `vsf`): a station document (`station`: id, name, position as a Dymaxion `w`, datum, msl_um, source, licence; a `constituent` section each: name, amplitude_um, phase_q32) and a catalog (`catalog`, then `station`: id, name, position), every value read by name and type marker.
- **tools/tide-station** (host): NOAA's JSON to a station document, the catalog from NOAA's list or from documents, `show`, `predict`. fetch.sh fetches with curl; fixtures.sh writes crates/tide/stations/ (the four stations and their catalog).
- **payloads/tidetest**: the data path on the device without drawing and without anyone's place: the bundled catalog and documents parsed, each station found as the nearest to its own place, its face computed, each step timed. Builds (payload-check passes) and passes the sandbox checker masked (48569 words, 0 refused). Not run on a device yet.

## When the display starts again

A display that ends (a restart, a fault) takes its held calls with it: the kernel answers each one it had taken ERR. Nothing is killed by hand to recover (2026-11-20; until then a restart left every puck showing its name until it was killed and the map restarted).
- **The puck** (ferros_puck::rt `watcher`): its WATCH answered ERR once, it calls again; that call waits for the display that comes after (a call on an endpoint with no server queues until one receives), which answers at once, its count starting again, with the circle hidden. Two failures in a row are no display behind the grant, and the watch stops as before.
- **The map** hears the restart on the facts' bell (the display's frame count went down) and hands the new display everything again: the scene, the icons, the pucks' bindings (op 73). Each puck then hears its circle shown and draws its square there.
- A SHOW in flight at the end is answered ERR and taken as refused; the next square goes to the new display.
- Measured on the Pro (b1068): the seven pucks drew again on their own after `diag "restart display"`.

## What changes on the device

Stored together with a flash of the matching kernel (Claude memories/stored_capsules_need_flashed_kernel.md): the kernel (the `puck` right and its stamp, init's slot 9 the display's `puck` endpoint, the pucks' window, SYS_DMA x1 = 6, BELL_PUCKS, facts 88-89 for the compass), and the capsules display, map, init, kists (ferros_kist's `puck` right) and sensors (the magnetometers). Then the clock, basecalc and the six pucks are stowed like any kist; the clock's version 1 replaces the clock kist of 2026-11-10 (the same option).

## The display draws no circles of its own

Owner, 2026-11-18: "Delete the old clock, yes. none of that business. ferros settings, kists and pucks, that's it." The display's own clock face (icon G#80) and light scope (icon G#82) are gone, with the focus op (25) they took, the gravity turn they followed and payloads/focustest; a circle on the deck is the settings circle, a kist, or a puck. The console's corner clock writes its time with `time_text`. The light scope's data path (the sensors' 16 kHz stream by preference 7, SYS_LIGHT, the Developer page's channel) stays for a scope puck.

## Not built

- A puck mid-hand-over when the display restarts waits for an answer that never comes, and needs a restart itself (the development loop only: kill it, restart the map).
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
2. **The tide's colours.** Answered 2026-11-24 by the widget: its solid primaries (water cyan, the sun's sky yellow, the moon's blue), as VSF RGB primaries. Still open: whether to tune them (the widget calls them "solid primaries for now, to be tuned").
3. **Heights' datum.** Above chart datum (MLLW, as NOAA prints them) or above mean sea level?
4. **basecalc, puck or window.** Built as a puck by default with the window still buildable (`--window`); both are basecalc's one identity, so only one is on the deck at a time.
