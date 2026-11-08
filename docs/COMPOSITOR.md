# The compositor

Status: decided with the owner, 2026-11-07 ("BUILD IT!"). Built in the order at the end; each step measured on the Pro before the next.

## Why

Measured on the Pro, 2026-11-07:
- The console redrew on the render thread every few seconds (each battery reading, each link measure). It took 33 ms to draw and about 16 ms to publish into the four temporal-dither phases. The render thread stalled for that long, a stutter each time.
- The radial table brought the draw to 9 ms. The stall is still there, because the drawing is still the render thread's.
- While a finger moved the map, frames took about 4.5 ms to draw, yet the screen ran at 72 frames/s.

Anything that draws on the render thread can stall the screen. The cure is structural.

## The rules

0. **Producers never draw on the compositing thread.** Every layer has its own buffer and its own producer: a kist, or a system worker on the A715s (the console, the helm, the clock, the wallpaper). The render thread on the X3 only composes and presents.
1. **A layer is a mailbox.**
   - Each layer has two buffers. The producer fills the back one and publishes it with one atomic swap.
   - The compositor takes the newest complete buffer each frame and never waits.
   - A slow producer drops its own frames, never the screen's.
2. **A layer carries its coverage.**
   - With each publish comes a map of 64 × 64 tiles, a byte each: empty, opaque or partial, and whether it changed since the last publish.
   - The compositor skips empty tiles, stops at the first opaque one (nothing behind it is read), and copies a tile whose frontmost non-empty layer is opaque.
3. **Front to back, `under` only, α + darkness** (AGENT.md "Compositing").
   - `under` is associative, so any contiguous run of layers composed together is itself one layer, with partial α where it has it. Merging a run is exact.
4. **The running stack is groups, not layers.**
   - Contiguous layers that have not changed lately are merged into one cold group, its composite cached.
   - A layer that changes is hot and stands alone.
   - A frame composes a handful of groups, however many kists there are, and only in tiles that changed.
5. **Temperature is counted in frames, not time** (NO TIMERS).
   - A layer's change makes it hot. After 64 frames unchanged it turns cold and joins its neighbours.
6. **Nothing that reorganises the stack runs on the frame path.**
   - The stack is an immutable descriptor (order, groups, buffers). It changes by building a new one and swapping a pointer at a frame boundary, so a frame reads one consistent stack.
   - A merged group is built by a worker, in bands, over as many frames as it takes. The old grouping stays live until it is done.
   - A kist entering splits the group it lands in, whose members stay valid on their own. A kist leaving lets its neighbours merge again.
   - An opaque full-screen layer drops everything behind it from the walk.
7. **Compose once, dither four times.**
   - The composed working buffer (gamma 2, white at Q14) is made once a frame.
   - Only the present (the table, the gains, the dither, the 10-bit pack) runs per phase, and only in tiles that changed.
8. **One kernel.**
   - `under` in NEON on 16-bit channels, two pixels a 128-bit register, branchless within a span.
   - The early-outs are per tile and per span, never per pixel.
   - Full-screen work is split by rows across the workers.

## Budget

- The Pro's panel is 1344 × 2992, 4.0 M pixels. At 8 bytes a pixel a full layer is 32 MB, about 3 ms of one core's memory traffic.
- A typical frame (a finger on the map, everything else cold) touches about one layer's worth of tiles through three or four groups. That is inside 120 Hz's 8.3 ms with room for the present.

## Not used

- The DPU's hardware windows. They blend Porter-Duff over, so they serve only pure selection: an opaque full-screen app in a plane, nothing blended against it.
- The GPU for `under`: the owner's decision, 2026-11-05. The GPU makes compute-dense layers; `under` and the present stay on NEON.

## The open circle is an app's region (owner, 2026-11-07)

The owner: "that's basically the internal region for the app, the rest should stay fixed aside from clock and signal updates on occasion with an atomic. Could also maintain a clean window and always just throw the text right on top. since that circle is ours we can do whatever we want with the inside."

Measured that day:
- With settings open, every frame redrew the whole scene. The settings circle fills the screen, and its mark was drawn again each frame (14 ms; too big for the sprite cache).
- The whole screen was published: about 105,000 of 108,000 blocks had something drawn, so compose-once found nothing to skip.
- That is 39 ms a frame, 25 frames a second, for a page where only text moved.

The design:
- **The clean window.** The open circle's ground and the ferros mark, made once for the circle's size and turn and kept, never redrawn while the page scrolls or turns.
- **The text in front.** The page's text is its own layer, drawn first (nearest the viewer) and composed under onto the clean window, only in the rows that changed. No Porter-Duff over anywhere.
- **The chrome stays fixed.** Outside the circle nothing moves but the console's own updates, which its worker publishes when they change (step 1).
- **Titles turn in.** A page's title spins about its own centre into place as the page changes, instead of sliding and being clipped at the circle's edge. The glyph engine draws each glyph at any angle, so a title is a few glyphs a frame.
- **Settings is the first app.** Its region uses the same machinery as an app window: surfaces, span chains, `publish_app`. What it learns, apps get.

### As built (2026-11-07)

Found while reading the path: the open circle's mark was drawn every frame under the page's own ground (the map's opaque `fill` disc), so none of it ever showed; the 14 ms bought nothing a frame. The clean window is the ground alone (owner, 2026-11-07: the mark still drew behind it; "can we not draw any other circles other than the nav and console?"): no circle of the map, not the open one's own mark, no glow; only the bars, their own layer, and the wallpaper outside it.

The shape, top down:
- **Two layers, front to back.** The region's text is the front layer, a full screen of working values in the sprite arena (no circle on the map is sprite-sized while a circle is open, so the arena is free; sprites are dropped when the region opens). The clean window is the canvas: the ground disc alone, made once per (circle, ground, camera, theme, scene) and kept. The wallpaper stays under both.
- **The protocol.** Landed, the map's frame is `region` (op 18: the circle and its ground) instead of `begin`, then the page's ops, then the camera. The page's ops go to the front layer; the camera draws no scene while the clean window is current and only publishes. A camera op without a region op (the flight, the plane) ends the region, and the old path draws everything as before.
- **Only what changed.** The display keeps last frame's front ops. An op sent again byte for byte is not drawn again; the rectangles of the ops that went and came are merged until disjoint, cleared, and every op that touches them is drawn again clipped to them. Those rectangles are the frame's damage: what is published, and what the other phases owe.
- **Compose once still holds.** `publish_band` reads the front layer for its block skip while a region is up (its clean bit then means "this phase holds the clean window here"), and composes a made block front to back: the text, the canvas under it, the wallpaper under that. Any change to the clean window resets the bits and owes every phase the whole screen, once.
- **Titles ride the circle.** The owner: "rotate around with the circle", not slid left and right and clipped, not spun about their own centres, and no vertical skew. Each page's title stands on the arc about the circle's centre through the title's place, its baseline the arc's tangent there (op 19: text centred, turned, with its own α). Neighbouring pages' titles are the angle apart whose arc is the screen's width, so at the top a title moves with the finger. Turning a page, the outgoing title rides the arc away and the incoming one rides in from the other side, at one size, each at α = cos²(π · its offset in pages), gone at half a page with α's slope 0 there (C¹). The strip's offset is the finger's or its critically damped spring's, counted in display frames: the motion has no clock of its own.
- **Measured** on the Pro (a synthetic swipe across the settings, `FERROS_MAP_START=swipe`, every page op changing every frame): 118-120 frames/s on the panel, against 25. A frame: the front layer 0.7 ms, the publish 3.2 ms (2.3 M pixels of damage, most of it skipped blocks), the worst 5.2 ms. The clean window is made once on landing (26 ms) and the region's end republishes once; the flight between still draws everything a frame (30-40 ms, the mark growing with the circle).
- **The flight, prerendered** (owner, 2026-11-07: "crossfade and pre-render the full size of what it will look like when zoomed in and do a simple nearest since nobody will notice at 120hz"). Opening and closing the circle drew the whole scene a frame (30-40 ms, the mark growing with the circle). Now two images are made once at the flight's start and every flight frame only samples them:
  - **The page image** is the landed page exactly as landing shows it: the clean window in the canvas and the text in the front layer, made by the region path itself (op 18 and the page's ops), at the landed camera. Opening, it is prerendered; closing, it is what is on the screen.
  - **The plane image** is the plane at its camera, a full screen of working values in a free app surface (the last of the four that no window shows and no process holds, the kernel's generation byte 0; the screen capture takes the first). Opening, it is the canvas as the plane's last frame left it, copied; closing, the scene is drawn into it at the camera the flight lands on. No surface free, or an app in front: the map flies the old way.
  - **The protocol.** The map sends op 45 (`flight`, answered when done) when a flight to or from the open circle starts: which way, the plane image's camera and the page image's, then the ops that make whichever image is missing (opening: the region op, the page laid out landed and the landed camera; closing: the landing camera). A flight turned round mid-way finds both images made and makes nothing. Each frame of the flight is op 46: the camera now and the flight's progress α.
  - **A flight frame draws no scene.** Each screen pixel goes through the camera now to each image's pixel (one 16.16 step along a row, the row's span inside the image worked out once, nearest, no divide), the page image composed over the wallpaper (or black) and so opaque, at α, in front, under-first; the plane image over the wallpaper under it. Both being opaque, that is the crossfade α·page + (1 − α)·plane with `under` alone, the same whichever way the flight goes, and at α = 1 it is exactly the landed page: landing (the region's own frame, its clean window already current) shows no switch. It runs inside the publish (`publish_band`), stripe by stripe on the workers, so no image of the frame is ever written but the phase.
  - **Half resolution, made smooth.** A whole screen through the present's light, ODT and code every frame is what the old path paid for most (about 16 ms of a flight frame on its own), so a flight frame samples and presents one pixel in four: each second column of each second row is sampled and coded (`neon::codes8`), and the codes are spread linearly to the rest (an odd column the mean of its neighbours, an odd row the mean of the rows above and below, `neon::spread16`), so it shows no blocks (the first cut doubled each sample into a 2 × 2 block: the owner saw "a weird blocky jitter while zooming out"). Each row is still dithered with its own thresholds. The clean window being the ground alone, the page image's ground is worked out per sample from its circle, not read, and its text is read only on the rows the front layer's ops touch. A block where neither image has anything shows only the wallpaper at full resolution, skipped where the phase already holds it. A block of the bars' rows that the bars leave clear is the scene's alone (`bars_in`), not composed per pixel.
  - **Measured** on the Pro (`FERROS_MAP_START=open`: eight flights tapped in and out, no one at the panel): a flight frame 10-15 ms (zooming out 10-11, in 14-15; up to 20 while other work holds the workers), against 30-40 drawing the scene. The first frame pays for the prerender: opening, 19 ms (the canvas copied, the clean window and the text made), closing, 9 ms (the plane drawn into the surface). Not yet the 8 ms aimed at: the sampling and the codes are most of what is left, on the A715s at about 1 GHz.
  - **α is the flight's own spring.** The map carries a fifth part on the camera's critically damped spring, the fade from 0 (the plane) to 1 (the page), stepped in display frames with the rest; from rest it never overshoots, and a flight turned round keeps its speed, so the fade turns with it. The camera lands only once the fade is within a thousandth of its end too.
  - **The ends.** Any frame that is not a flight frame ends the flight: every phase owes the whole screen once. Closing lands on the plane's own path.
- **What it does not do yet.** The other phases' debt is still one rectangle a phase (`PHASE_DIRTY`); a scroll moves every row, so its damage is the page's band either way. The front layer is not a `Win`/surface: the window table holds the kernel's per-app surfaces and the region needs none; when apps draw into a region of their own circle, the region becomes a window of the map's.

## Controls (owner, 2026-11-08)

The owner: "would be nice to add in the fluor elements like sliders (for brightness) and buttons (for 'close' 'remove this device' etc.)"; for sliders, "use the room tune or chirp gui" (chirp's tune.rs, whose sliders are fluor's); then "Radio buttons and checkboxes. I'm very against the switch design iProducts and Google use". No sliding toggle anywhere: an on/off setting is a checkbox, one of several a radio group.

Four region elements, page ops like the text, so they go to the front layer and are drawn only where they changed. Each is one op the display draws whole (a control's parts must agree with each other), front to back by `under`:
- **47 `button`** `[cx i16][cy i16][w u16][h u16][px u16][fill][edge top-left][edge bottom-right][ink][utf8 label]`: fluor's button (widgets/button.rs). The label centred in front, then the pill under it: a squircle rounded rect at `SHAPE_POWER` with the house's asymmetric corners (top-left and bottom-right at half the height, the others at a quarter), filled inside a two-tone edge a 32nd of the label's size wide plus a pixel. The pill is drawn by fluor's own kernels (`paint::squircle`) into a scratch the pill's size in fluor's α + darkness, and each pixel brought over as premultiplied light (α − darkness), so the shape is fluor's to the pixel. `w` is the page's one width for its buttons; a longer label grows its own pill.
- **48 `slider`** `[x0 i16][x1 i16][cy i16][track r u16 Q8][knob r u16 Q8][at u16 Q16][left][right][knob]`: fluor's slider (widgets/slider.rs, chirp's look): the knob, a disc at `at` of the way along (sub-pixel), in front; the track under it, a capsule (`draw::capsule`: the points within r of a segment, C¹ where its ends meet its sides) in `left` up to the knob and `right` past it.
- **49 `radio`**, **50 `check`** `[cx i16][cy i16][px u16][mark u16][stroke u16 Q8][state u8][stroke][fill][ink][tick][utf8 label]`: the mark and its label, centred together on (cx, cy), or with the mark's left edge at cx for a group's column (state bit 2). The radio is a ring (`draw::ring`: the band's coverage by first-order distance, no root) with a disc of `fill` half its width inside when chosen (bit 0). The checkbox is fluor's checkbox's squircle box (its corners, `SHAPE_POWER`) by fluor's kernel, its edge the band between the outer squircle and one inset a whole number of pixels (outer coverage less inner, exact for nested shapes), filled when checked, and with a tick of two capsules over the fill when bit 1 is set too.

The map (payloads/map) lays them out in a row's place and pitch, scaled by the page's unit (a 48th of the width at the circle's arrived size, times the UI scale), each row tappable across the page as a text row is:
- **Buttons**: the Fleet page's actions (add a device, type its words, finish, close or done, approve, remove this device), a link's actions (re-register now), the clock's zone (a quarter hour east, west), the first-launch question's start. The colours are fluor's button on grey (BUTTON_FILL, BUTTON_HELD, the textbox edges, shadow on the top-left so it stands raised) and the same in each theme's hue (`BUTTON_LOOK`). A finger down on a button shows it held until it lifts (the tap then acts) or moves (a drag, no tap); remove this device's first tap leaves it in the held fill, armed, with its label asking for the second.
- **Sliders**: brightness (its nine stops, dimmest on the left) and the UI scale (half to twice). The track is three quarters of the screen's width at the circle's arrived size and does not grow with the UI scale, so the scale's slider stays under the finger while the page grows round it. A drag that starts across a slider's row is the slider's (the strip and the scroll keep every other drag); a tap puts the knob and the value there at once. Brightness is written through the Settings capability each time the stop under the finger changes. The scale follows the finger live and is written once when the finger lifts, as the pinch's is (every process lays itself out again on a scale change). Let go between two values, the knob goes to its value's place on the strip's critically damped spring, stepped in shown frames.
- **Checkboxes**: names under circles, unlock at boot (two taps to turn on, the label asking for the second), proximity wake, a radio's own switch, the home screen across the fleet, every kist's wants and the rights' defaults, the first-launch question's wants, the temporal dither. **Radio groups**: the number base (a radio beside each sample), the keyboard's layout, WiFi's power save and band, the light scope's channel. A group is centred on the page by its widest label, which the display measures (op 28).
- **The look of radios and checkboxes is the owner's to pick** (`MARK_LOOK`, 2026-11-08): 0 the app circle's hairline (a pixel wide, its colour: the ring's light halved on release, orange on development builds) for the stroke and the fill; 1 that hairline round a fill of the theme's text colour, the box ticked in the ground's colour; 2 fluor's stroke weight in the secondary text colour, filled with the theme's lit colour and ticked.

Nothing moves on its own: a state changes in the frame its tap lands. The knob's spring after a lift is the only motion, counted in shown frames.

## Pans from the last frame (owner, 2026-11-07: "keeping pre-renders for pans")

A pan moves every circle by the same amount, so the frame before already holds this one, shifted.
- **The flag.** The map's camera op carries bit 1 of its flag byte while something moves: a finger is down, or a flight, fling or spring goes on. A frame without it is drawn whole: at rest everything is drawn once more for real, antialiased.
- **The shift.** A plain plane frame (a `begin`, nothing drawn over the scene, no region, no flight, no widget on the map) with the flag, at the zoom, turn, theme and scene the canvas was last drawn whole at (the anchor), is the canvas moved in place by whole pixels; only the strips the move uncovers (a band of rows, then the columns beside it, never a corner twice) are drawn, clipped to them. The `begin`'s clear waits for the camera op to know which kind of frame it is.
- **Exact, not approximate.** Shifted, the scene is drawn at the anchor's camera with every centre moved by the whole shift, so a circle's whole-pixel centre in a strip is where the moved part of it already stands: no seam. The real camera is under half a pixel from it, and the frame at rest puts everything exactly where the camera says.
- **Past half the screen** the frame is drawn whole and becomes the new anchor.
- **No antialiasing in motion** (owner, 2026-11-07: "ditch all anti-aliasing while fingers are down or motion is going on"). With the flag, the scene draws the ferros mark by rings and segments (`draw::logo_hard`; owner: "a simple distance squared check to determine the ring and then a segment based on angle"): each row is cut where it crosses the pieces' arcs (a root an arc a row) and sides (lines through the centre, a division a side a row, no angle), and each run between two cuts is one piece's colour, found once at its middle, filled straight. Discs are filled the same way (`draw::disc_hard`, a run a row). Sprites (circles cached at rest) keep their antialiasing; glows and labels are as at rest. The frame without the flag draws everything antialiased once.
- **Measured** on the Pro (`FERROS_MAP_START=zoom`: 128 frames of a pinch with a twist, the settings circle growing to three times the screen's width): the scene 2.4-2.7 ms a frame against 7.2 with the antialiased mark (spans, each edge pixel worked out by bump steps), 129 frames/s; the publish 2.2-2.4 ms.
- **Measured** on the Pro (`FERROS_MAP_START=crowd`, 200 circles, the synthetic drag): the shift and its strips 2.6 ms a frame against about 14 ms drawing the scene; 78 frames/s on the panel against 66. The publish is now most of a pan frame.

## The scene by rows (owner, 2026-11-07: "split across cores")

The scene loop stays on the render thread for what must be one thread's: each circle's place on the screen, its sprite made when it is cached, the widgets and labels (the glyph cache and the bump allocator are the render thread's). What it would draw it lists instead (`Item`: a sprite's blit, the mark, a disc, an icon, a glow), in its order. The list is then drawn in stripes of 32 rows taken by the publish workers and the render thread, as a publish is: each thread draws every item that reaches its stripe, inside its own clip (`draw::BAND_CLIP`, told by the stack it runs on, `draw::thread_slot`), so each pixel sees the same layers in the same order as on one thread. A pan's strips go the same way, a strip at a time.

Measured on the Pro (`FERROS_MAP_START=zoomcrowd`: the crowd's 200 circles and the settings circle, a pinch with a twist, every frame drawn whole): the scene 3.2 ms a frame against 14.7 on the render thread alone; the zoom of the settings circle alone 2.1 against 2.4-2.7.

## Build order

0. **Measure the interactive path** (touch → map → display) to know where the 72 frames/s go.
1. **The console, the helm and the clock as producers** on a worker, into mailbox layers. The render thread only publishes their changed tiles.
2. **Compose once, dither four times**, in tiles.
3. **Coverage maps per layer** and the tile walk with its early-outs.
4. **The stack descriptor and groups**: cold merging in the background, the pointer swap.
5. **Kists' surfaces as mailbox layers** under the same walk.
