# The quick row

Owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things." On the proposal (pull down from the top edge where the status circles live, circles that grow with the drag, settle by physics, a second row of circles, long press for a radio's page): "Works". Then: "I'd also think NFC is rarely used so let's move it back to the bottom right like we do on the 8a so we have the same open spot where the camera is. any tap in that area or drag down should bring up said second row of consoleness, however we want to do that, some circle fitting goodness I suspect."

Built 2026-11-11 (branch `quick-settings`), not yet run on a device.

## The doorway

- NFC is down the right side on both boards now (display `corner_layout`), so the top row is clock, battery, GPS on the left and cellular, WiFi, Bluetooth on the right, and the spot by the camera is open on both.
- **The doorway** is the disc about the camera hole (`ferros_sys::cutout_hole`) out to the nearest top console circle, outside the app's circle (display quick.rs `doorway`; its radius is logged once per size as `display: quick row … doorway N`). Nothing in it is a fixed pixel: the hole is measured glass, the corners are solved from the glass.
- A still tap in the doorway opens the row, or shuts it.
- A drag down that starts in the doorway pulls it; nothing else does (owner, 2026-11-11: "the second console row should only show if drug down from the area in the invisible camera circle"). Until then a drag down from anywhere on the top, a radio circle included, pulled it.

## Where the row sits: circle fitting inside out

The corners' circles are each tangent to the glass, the app's circle (from outside) and their neighbour. The quick row is the same fit turned inside out: six circles inside the app's circle, each tangent to it from inside, to a horizontal floor from above and to its neighbour, and the outer pair tangent to the glass's left and right edges (display quick.rs `layout`).

Owner, 2026-11-11, after trying the first cut on the Pro: "Good idea on the swipe down console! Let's make the circles always touch the left/right sides so move the bottom horizontal guide down until they scale up and touch." Until then the floor was the console's last row (the first row where the app circle's chord is the whole width) and the row stopped well short of the sides (husky r 81, 72, 58).

- **The solve.** Two unknowns: the floor, and the split between the middle pair (the line both touch). For a floor, the split moves until both outer circles are as near their walls; then the floor moves down until that nearness is nought. Both by bisection, the circles' centres on the closed-form line where the arc's room equals the floor's. Once per screen size, logged with its time (`display: quick row (x,y,r) … floor F, walls L/R hundredths, solved in N us`).
- **The walls are the glass, not the screen's edge**: measured as `corner_layout` measures them (`GlassOutline`, hairline and rim off). The glass's corners are not alike (the big one top left), so the split is not the centre: a pixel right of it on husky, six on akita, whose fat corner is rounder.
- **Collisions.** The console's corner circles (clock top left, cellular, WiFi, Bluetooth and NFC top right) are outside the app's circle and these inside it, both tangent to it, so they cannot overlap; their drawn rims (two and a half pixels) can meet, and the console's region redraw composes both. The camera hole is above the app's circle; the middle pair clear it by 54 to 56 pixels on husky, 41 to 49 on akita.

Left to right: radios off, sound, brightness | torch, do not disturb, screencap. The two rings (brightness, torch) are the middle, largest pair.

Solved on the host from the same equations, the glass from fluor's corner parameters (the device logs its own):

| | Floor | Left outer | Left next | Left middle | Right middle | Right next | Right outer |
|---|---|---|---|---|---|---|---|
| husky (1344 × 2992) | 413 | (82, 334) r 79 | (273, 297) r 116 | (530, 270) r 143 | (816, 270) r 143 | (1072, 297) r 116 | (1263, 334) r 79 |
| akita (1080 × 2400) | 337 | (71, 273) r 64 | (225, 244) r 93 | (432, 223) r 114 | (660, 224) r 114 | (864, 246) r 92 | (1016, 275) r 62 |

## Drawn once, then slid (2026-11-11)

The first cut grew the row out of the point under the camera, every circle drawn again at a new size each frame. On the Pro the owner found it "stupidly slow and jerky"; a console draw had been measured at 9.5 ms ("display: console: 32 draws, 9530 us each"), over a 120 Hz frame's 8.3, and the growing row drew six more circles (vessels, rings, words at a new size) every frame. Then: "Simply appearing and disappearing is fine too. no need for a scale in/out animation", and, refining it, "track the finger down from top tho and slide it in so it doesn't need a re-render, just a move and composite, then do the iterate/2 each frame, like we do for the settings page until it snaps in or out of place". The C∞ grow-and-fan is gone by the owner's word.

- **Drawn once.** Each circle is drawn into its own sprite at its size, only when its look changes (a tap, a ring turned, the sound mode, the language; display quick.rs `update`, `render`): its ring, its word, then its vessel under them, front to back, then decoded to working values in place. A sprite holds only the circle's disc out to its reach, row by row (spans): about 264 thousand pixels, 2.1 MiB, on husky; 1.4 MiB on akita.
- **Then slid.** The row's offset, whole pixels: 0 hidden, the slide's length open (husky 299 px: from the row's lowest row to the top of the app's circle, so hidden every pixel of it is above that circle). Each pass the console's worker composes the sprites at the offset under the console's own circles into the bars' decoded rows (`compose`, from `bar_decode`), clipped to the app's circle with exact coverage on the two edge columns, so the row comes out from behind the top of that circle and the console stays in front. Only the band the row covered and covers is composed and diffed; the render thread publishes what changed.
- **Memory.** The console's circles are drawn over the chrome rows alone again (`BAR`, husky 288 rows); the decoded rows (`BAR_W`, its back buffer, its spans) reach the row's floor (`BAR_TOP`, quick.rs `rows`: 417 on husky, 341 on akita). Against the wide row's first cut (the drawing buffer grown too): 1.4 MiB less there, 2.1 MiB of sprites: about 0.7 MiB more on husky, about 30.4 of the display's 32 MiB heap, which is why the heap grows on demand since that night (COMPOSITOR.md "Memory": the boot line `display: heap at start …` says what it is).
- **Measured.** Each pass that composes the row adds to `display: slide: 32 passes in F frames; us mean/worst: circles …, sprites …, compose …, diff …, pass …; rows …; publish …` (the console's circles drawn, the row's sprites drawn, the rows decoded and composed, the diff, the whole pass, the rows, and the bars' publish on the render thread). F against 32 says whether the slide keeps the panel's pace. A host stand-in (x86, husky's geometry; not the device's numbers) puts a frame of the old grow at 1.4 ms against 0.3 ms for a frame of the slide; the device's 9.5 ms console draws scale to under 2 ms a slide frame by that ratio, before the glyphs the old path rasterised at every size, which the stand-in leaves out.

## The slide and its stepping

- **The offset** follows the finger one to one from where the drag took hold (the doorway, the top's ground, a radio circle dragged down, or a quick circle other than the rings dragged up or down), between hidden and open.
- **The lift**: from hidden, the row goes open if it was pulled past a third of the way, else hidden; from open, hidden if pushed back past a third, else open. Each frame the panel showed, what is left to go halves (a right shift, the half pixel dropped), so it lands in a few frames and holds still (map quick.rs `step`). A still tap in the doorway sends it open or hidden the same way.
- **Frame timer on shown frames**: one step for each frame the panel showed (display op 24's frame-start count), never on a clock, so a missed render steps twice rather than slowing.
- **Shutting**: push it back up, a still tap in the doorway, or a touch anywhere behind the row (that touch goes no further).
- **Taps on the row** count only once it is all the way open; while it slides, a finger on it is the slide's.

## The circles

- **Brightness** (a ring): a finger on it turns it, a whole turn the whole range of preference 3 (32nds of a stop, 0 the brightest, 255 the dimmest), eased at both ends (a C∞ ramp), so the value slows into each end. Applied live while the finger turns (`set_pref_live`) and kept once at the lift (`set_pref`), as the Appearance slider does. The ring's lit share is how bright it is; while a finger turns it, the display draws the exact share the map sent into that circle's sprite again (the one circle that changed), so the lit end moves smoothly between the preference's steps.
- **Torch** (a ring): a tap lights it at the last level it was lit at (the whole dimmer at first) or puts it out; a turn sets the level, 1-255 the torch's own perceptual dimmer (docs/TORCH.md). The map calls the `torch` endpoint waiting (`call`, not `call_live`: the kernel starts the torch process on demand and a live call would be refused while it starts), from a thread of its own (map quick.rs `worker`), so the main loop never waits on it. The torch's level is asked once each time the row starts to show until it has answered. A torch that does not answer (no capsule stored, an older kernel with no grant) draws dimmed and a tap does nothing.
- **Radios off**: every radio off at once, the set that was off before kept (preference 41, `radios_before`); a tap from all off puts that set back (all on when it was every radio).
- **Sound**: a tap goes round sound, vibrate, silent (preference 39, a device setting). Its word is the mode now. Nothing plays a sound or a buzz by it yet.
- **Do not disturb**: the person's, not the device's (SETTINGS.md). A tap asks the identity service (fgtw op 24) from the map's worker thread; the service puts it in the person's space, hands it to their fleet and fgtw.org, and writes it to the kernel (preference 40), which the circle shows. Before the identity service's first pull it is refused and the circle does not change. Nothing is silenced by it yet.
- **Screencap**: the row shuts first; when it has landed the map sends op 56 with the capture flag, and the display takes the capture (as op 30 does) the first pass the bars on the screen hold none of the row, so the row is not in the picture. There is no clipboard yet (owner, 2026-11-07: "when we get the clipboard running that's where the screencap will live"), so the frame stays in the free surface op 30 wrote it to and its address goes to the console (`display: quick row screencap (no clipboard yet): G#…`).
- Rotation lock: none (no rotation today).

Each circle's word is ferros_lang's, in the person's language, a word a line. A circle is lit while what it switches is on; a finger on it lifts it to the live ink's light.

## The dash's zones, and each circle's page (2026-11-11)

Owner, on the Pro: "the top bar, dragging a console circle to center no longer works. the second console row should only show if drug down from the area in the invisible camera circle. Also, ideally we make each zone in the console always map to a circle, USB, clock, battery, etc. (might have to build a battery page) and then closest to circle wins if it's in the dash area."

- **Every pixel of the dash is a circle's.** The top's chrome rows outside the app's circle are partitioned by the nearest circle's rim: a pixel belongs to the circle with the least |p − c| − r (display main.rs `dash_row`, built once per screen size, kept as runs per row; the hit map takes them, hit.rs `bars_row`). By the rim, not the centre: "closest to circle" is the distance to what the eye sees, a big circle (the clock) keeps air round it in proportion to its size, and inside any circle its own distance is negative, so a touch on a circle is always that circle's. The doorway's disc is claimed first. Ids (op 41): 0-4 the radios, 5 USB, 13 the clock, 14 the battery.
- **Drag toward the centre** (past a twelfth of the screen's harmonic span, as the helm's): that circle's page: the radios' and USB's link pages, GPS its page, the clock its page, the battery its page (new: charge, whether it charges, voltage, current, power, temperature, each in the person's base, from the gauge's facts; the gauge's health is not read by the kernel yet). Restored as it was before the quick row took that drag.
- **Tap**: a radio switches on or off (preference 15), as before; USB, the clock and the battery open their page.
- **No long press.** It went with drag-to-centre back (it was a hold measured in frames, the one gesture that waited on time; the press ring with it).
- **The quick row** opens only from the doorway; a drag down anywhere else on the top does nothing to it.

## Who does what

- **Display** (payloads/display quick.rs): the geometry (`layout`, `doorway_disc`), the sprites (`init`, `render`, `update`) and their composite (`compose`, from `bar_decode`), op 56 (the map's state of the row: the offset in a word, the looks in two more, the console's worker nudged; the reply the slide's length), the ids op 41 reads from the hit map (written at the bars' compose step, hit.rs `bars_row`: 32 + k while the row stands open, its centre and radius from `layout`; 10 the doorway, `doorway_disc`; 11 the top's other ground; 12 behind the row, everything inside the app's circle while any of it shows, so op 27 gives an app no point there), and the screencap.
- **Map** (payloads/map quick.rs): the finger, the offset, its halving, the rings' turning every action, op 56 sent whenever anything in it moved. Touches reach the map first for the system's places, with an app in front too, as the corners' always have.
- **Kernel**: preferences 39 (sound), 40 (do not disturb, the identity's, taken from the process named fgtw alone) and 41 (radios before), kept in the device document; and the torch's send right handed to whoever it hands Settings (the map's slot 9), since init's grants page is full.
- **fgtw** (payloads/fgtw space.rs): op 24, the person's do not disturb into the space and to the kernel; ferros_space kind `DND`.

## Op 56

`quick [offset i32][flags u8][torch level u8][held u8][press radio u8][brightness ring u16 Q16][torch ring u16 Q16][unused u16]`, 15 bytes; the reply `[0][the slide's length u16]`:
- flags bit 0: the torch answers (its level is the next byte); bit 1: a screencap now that the row is hidden.
- held: the quick circle a finger is on (G#FF none). Press radio: the radio circle a finger presses (G#FF none).
- A ring's share G#FFFF: none sent (the display takes brightness from preference 3 and the torch from its level).
