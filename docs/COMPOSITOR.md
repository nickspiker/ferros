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
   - The composed buffer is made once a frame, baked (since 2026-11-12, "Baked" below: the panel's own codes).
   - Only the present (the dither and the 10-bit pack) runs per phase, and only in tiles that changed.
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
- **49 `radio`**, **50 `check`** `[cx i16][cy i16][px u16][mark u16][stroke u16 Q8][state u8][stroke][fill][ink][tick][utf8 label]`: the mark and its label, centred together on (cx, cy), or with the mark's left edge at cx for a group's column (state bit 2). The radio is a ring (`draw::ring`: the band's coverage by first-order distance, no root) with a disc of `fill` half its width inside when chosen (bit 0). The checkbox is fluor's checkbox's squircle box (its corners, `SHAPE_POWER`) by fluor's kernel, its edge the band between the outer squircle and one inset a whole number of pixels (outer coverage less inner, exact for nested shapes), filled when checked, and with a tick of two capsules over the fill when bit 1 is set too. Inside both, behind the dot and wherever no fill is, is black (owner, 2026-11-08: "background for radio and checkbox should be black, not dk grey").

The map (payloads/map) lays them out in a row's place and pitch, scaled by the page's unit (a 48th of the width at the circle's arrived size, times the UI scale), each row tappable across the page as a text row is:
- **Buttons**: the Fleet page's actions (add a device, type its words, finish, close or done, approve, remove this device), a link's actions (re-register now), the clock's zone (a quarter hour east, west), the first-launch question's start. The colours are fluor's button on grey (BUTTON_FILL, BUTTON_HELD, the textbox edges, shadow on the top-left so it stands raised) and the same in each theme's hue (`BUTTON_LOOK`). A finger down on a button shows it held until it lifts (the tap then acts) or moves (a drag, no tap); remove this device's first tap leaves it in the held fill, armed, with its label asking for the second.
- **Sliders**: the UI scale (half to twice), and the typed settings' (QUICK.md "Quick pages": the torch's level, strobe, lit share and timeout, and since 2026-11-12 brightness on Appearance, its 32nds of a stop dimmest on the left; each with its name and its value in words above it, applied live and kept at the lift). The track is three quarters of the screen's width at the circle's arrived size and does not grow with the UI scale, so the scale's slider stays under the finger while the page grows round it. A drag that starts across a slider's row is the slider's (the strip and the scroll keep every other drag); a tap puts the knob and the value there at once. A typed setting (brightness among them) is written live thru the Settings capability each time the value under the finger changes, and kept once at the lift. The scale follows the finger live and is written once when the finger lifts, as the pinch's is (every process lays itself out again on a scale change). Let go between two values, the knob goes to its value's place on the strip's critically damped spring, stepped in shown frames.
- **Checkboxes**: names under circles, unlock at boot (two taps to turn on, the label asking for the second), proximity wake, a radio's own switch, the deck across the fleet, every kist's wants and the rights' defaults, the first-launch question's wants, the temporal dither. **Radio groups**: the number base (a radio beside each sample), the keyboard's layout, WiFi's power save and band, the light scope's channel. A group is centred on the page by its widest label, which the display measures (op 28).
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

## Memory (2026-11-11)

The display panicked at start on the Pro that night, "memory allocation of 2097152 bytes failed": its heap was a fixed 32 MiB region the kernel granted (cachemap `DISPLAY_HEAP`, `display-heap`), a bump allocator that frees nothing, and the quick row's band (QUICK.md "Memory") with the night's other additions had taken the start to about 30.4 MiB of it, with the wallpaper loader's 2 MiB still to come.

Since then the heap is the display's own arc (docs/ARCS.md), grown on demand:
- **The header.** `ferros_sys::entry!(display_main, grows HEAP_CEILING)`: the image's heap grows (bit 0 of the pad word), its ceiling 128 MiB (offset 48). The kernel maps 4 MiB of it right after .bss at spawn and sizes the arc's window for the whole ceiling; frames are mapped only as the display asks.
- **The arenas.** The console worker's 1 MiB and the wallpaper loader's 2 MiB live in those first 4 MiB (`heap_init`), fixed as before.
- **The render thread's heap** (`ALLOC`) starts empty at the arc's top, above the first stack, and asks for more (`ferros_sys::arc::grow`, SYS_ARC 40) when it runs out: at least 8 MiB, or a quarter of what it holds, or only what is short when that much would pass the ceiling. Each grow is a console line from the kernel (`display: arc grew G#… to G#…`). The new frames lie right above the old top, so the bump's run goes on unbroken.
- **Nothing in the heap is DMA.** The back buffer is the canvas region (`display-canvas`), the framebuffers, phases, sprites and app surfaces are their own granted regions, so the heap may lie in any frames of the pool, and move (ARCS.md step 3). What a device scans stays granted.
- **A kernel from before ARCS** (no SYS_ARC) maps the whole declared heap at spawn; the display then uses it as a fixed heap. On such a kernel the arc needs 128 MiB more than before: a display with this header wants the ARCS kernel.
- **Slot 9**, the old `display-heap` grant, is still passed by init and no longer read, so every later slot keeps its number.

The budget on husky (1344 × 2992), what lives for the process (before the heap's mark):

| What | Size | Where it is made |
|---|---|---|
| The typeface's glyph cache (2 Mi samples of u16) and its table | 4.1 MiB | `glyphs::Glyphs::new` |
| The present table, the g2t table | 0.25 MiB | `server` |
| The icons' mip chains | about 4.4 MiB | `draw::Mips::build` |
| The bars: `BAR` (the chrome rows), `BAR_W` and its back buffer (to the quick row's floor, 8 bytes a pixel) | about 15 MiB | `server`, QUICK.md "Memory" |
| The quick row's six sprites | 2.1 MiB | `quick::init` |
| The hit map (a u16 a pixel) and the bars' ids (two) | 7.7 + about 2.8 MiB | `hit::alloc`, "The hit map" below |
| The compose-once bitmap, the flight's scratch, the glass's plan (the wallpaper's refining mask, 0.5 MiB, went with its CPU job, 2026-11-11: wallgen draws it on the GPU) | about 0.5 MiB | `server`, `plan_glass` |
| Outside the bump: the console's arena and the loader's | 3 MiB | the 4 MiB after .bss |

About 27.4 MiB kept in the bump before the hit map (the 30.4 of QUICK.md less the two arenas now outside it), about 38 MiB with it, then each frame's scratch on top (glyph bitmaps, a draw's tables), reset every frame. The measured figures are the display's own lines:
- `display: heap at start N KiB kept (typeface and glyph cache …, tables and icons …, bars and quick row …, hit map …, the rest …), N KiB mapped for it; console arena …, loader …; arc top G#… at the start, ceiling G#…`, once, at the mark;
- `display: heap high water N KiB (kept …, mapped … of 131072), console arena N of 1024, loader N of 2048 KiB`, from the render loop's pass top each time one of the three passes a new MiB: the frame's scratch at its fullest, the console's arena as the console and the quick row come up, the loader's as a wallpaper is read.

## The hit map (2026-11-11)

The owner, on touch routing (op 27 walked the windows front to back reading their pixels' clear bit; op 41 recomputed the system's geometry): "I mean, I already built it in fluor so sure?" (fluor: `HitId = u16`, one per pixel, written by each widget as it paints). Then: "the hitmap is never even touched if the pixel is opaque. early out!"

- **What it is.** A u16 a pixel, the glass's size (husky 1344 × 2992: 7.7 MiB), the owner of what the glass shows there (payloads/display hit.rs). Ids: 0 nobody (black inside the circle with no window there), 1 the map's plane, 2 the open circle's page, G#100 + op 41's code the system's places, G#200 + surface an app's window, G#1000 + scene index a circle of the map's plane (the crowd's included; a glow is light, not a shape, and owns nothing).
- **Written by the compose, front to back, once a pixel.** Each publish writes the rows it composes, with the first phase of a publish that carries several (`publish_band`, `publish_app`). A row starts as one free run of columns; each layer, frontmost first, claims the runs it owns, writes its id over what is still free and takes that out (`Free`), so nothing behind is looked at where a layer in front owns the pixel; the ground takes what is left. No pixel is written twice (checked on the host against a brute force over 20000 random rows).
- **The layers, front to back.**
  - The system's places, written by the console's worker at its own compose step (`bars_row`, from `bar_decode`) into the bars' ids, a u16 a pixel of the bars' rows, front and back, swapped with the bars' decoded rows at a pass's top: outside the app's circle every pixel (the radios' console circles 0-4, the doorway by the camera 10, the top's other ground 11, back 9 left of the middle and deck 8 right of it at the bottom), and inside it the quick row's circles while it stands open as composed (32 + k). In full screen (owner, 2026-11-11: "helm should be functional but not visible in full screen, whilist console should be gone gone") `row` reads them on the helm's rows alone, one test of `FULL` a row: the console's rows fall to the windows, and the helm's wedges stay the system's, undrawn. The bars' ids are written as ever; every phase is published whole when `FULL` changes, and every row of the map with it.
  - Behind the quick row (12): everything inside the app's circle while any of the row shows on the glass (never in full screen, where the row does not show). When that changes the whole map is written again without composing anything (`refresh`, by stripes across the publish workers), since everything inside the circle changed hands.
  - With an app in front, the windows by the layer stack's spans: a pixel is the first window's in its chain that is not see-through there (a G2T16 pixel with any α, a working pixel without bit 63), as fluor tags a widget by its shape, not its α; a translucent window's pixel is still its own. Nobody's where no window is.
  - On the deck: the open circle's ground while a region is up (2), then the map's circles, front first, each its disc as the camera op draws it this frame (the same transform, a shifted pan's circles at the anchor's camera moved by the shift), then the plane (1). A flight's frame is the plane throughout.
- **Read by one lookup.** Op 27 is `map(x, y) == G#200 + the caller's surface`; op 41 is `map(x, y) − G#100` when that is one of the system's, else G#FF, with a quick circle's centre and radius from the row's layout (where it draws). Gone with them: `owner_at` (the windows' rectangles and clear bits walked per call), the console's circles and the doorway tested per call, `quick::hit`, `quick::showing`. What changed: while any of the quick row shows, a point inside the app's circle is the row's "behind" (12) and no longer also the app's (op 27 used to say yes there, so a tap that shut the row also reached the app).
- **Memory:** the map 7.7 MiB and the bars' ids 2 × (`BAR_TOP` + `CHROME_BOT`) rows × 2 bytes (about 2.8 MiB on husky), in the heap before its mark; `display: hit map N KiB …, the bars' ids N KiB` at start.
- **Cost a frame:** the rows each publish already composes, a few runs a row and one u16 write a pixel; a host stand-in (x86, one core, husky's size, the deck's layers) writes the whole screen in 0.3 ms with 16 circles, 0.9 ms with 64 and 1.8 ms with 200, before the four publishing threads share it. The stats line every ten seconds says what it cost on the device: `…; hit map N us in N rows` (summed over the publishing threads).
- **The development view** (owner: "a debug/devel checkbox for toggling on/off the hitmap on the fourth frame so we get three frames normal and fourth is always hitmap so we can see both. Quick shortcut in dev only is swipe up from both back and home to toggle on/off"; "on that fourth frame, any redraw to the map should trigger a new colour palette"): the fourth temporal phase shows the map, a flat colour per owner (one channel full, every code halved: the owner's "roughly 18% grey"), instead of the image, so three frames of four are the image and the fourth the map, at whatever rate frames come.
  - **Whole, every rotation** (owner, on the Pro, 2026-11-11: "Mostly just blockey areas around updates, like the time and there was a big rectangle in the bottom left at one point", then "make sure that fourth frame gets updated completely so I can see the whole map proper with fresh colours per rotate"). The first cut painted only the rectangle each publish wrote, in a palette new to that publish, so one owner (the plane, the console's ground, the back half) showed in a different colour inside every rectangle a publish had touched since: the console's clock and measures redrawn (blocks around the time), and the helm's back wedge lit by a finger on back (its whole bottom-left rectangle republished: the big rectangle). The map itself was one id there; its view was not one palette. Now the phase is painted whole from the map, a colour a run of one owner, as it comes round to the layer from another phase (`hit::on_layer`, from `layer_fb`, split across the publish workers), in a palette new to that showing; the publishes never paint into it. The stats line says `its view painted N times, N us each`.
  - **Owners that change with no pixel.** The console's worker publishes the rectangles whose pixels changed; it now adds the columns whose bars' ids changed (`hit::bars_changed`), so the map is written wherever an owner changed, the quick row's circles counting as open only on the slide's last step among them. Op 58 `[0 off, 1 on, 2 toggle, 3 ask]` → `[0][on]`, answered by the receive thread; the map's Developer page has its checkbox ("hit map"), and two fingers landing on back and deck together and swiped up past a twelfth of the screen's span toggle it (the corner's own swipe does nothing while they are a pair). Development builds only: a release build answers status 1 and the map shows neither.

## The console's bodies (2026-11-11)

The owner: "the console circle, I feel like those get re-rasterized every go. like once you attest, they get drawn once into a buffer and then that circle RGBA just gets blit px for px right to the screen."

They were: every redraw of a circle (a battery reading every two seconds, a radio's measure, the minute) worked its vessel out again (its table over the radius, then a root, a lookup and two folds a pixel) and, for a radio switched off, its field (a sine and a tanh a pixel). Now:
- **The body** (the vessel, and the art a radio shows switched off or GPS's pin) is drawn into its own layer (`BODY`, fluor's α + darkness over the chrome rows at the top) once, and again only when its own key moves: its fill (a radio's state, the battery's colour by its charge), its art, its size. The quick row's sprites (QUICK.md) did the same for the row.
- **What the circle shows** (the words and numbers, the clock's calendar rings, GPS's satellite rings) is drawn into the console as before, from empty over the circle's region, only when it changes.
- **Then the bodies blit under it**, pixel for pixel over the region (one fold a pixel, front to back).
- **Measured**: the 32-draw line (`display: console: 32 draws, X us each; … vessels A + B us a draw; …; bodies drawn N, their blit C us a draw`) says how many bodies were drawn and what the blit cost; before this, every draw paid the vessels (the owner's Pro, before: 9530 us a draw). A host stand-in (x86, a circle of radius 110): the vessel worked out again 0.26-0.28 ms, the blit 0.07 ms.
- **Heap**: husky 288 × 1344 × 8 bytes, 3.0 MiB; akita 239 × 1080, 2.0 MiB (`display: console bodies N KiB` at start).
- **Not yet**: the rows a redraw changed are still decoded to working values across the whole width (`bar_decode`, table lookups, no drawing), and their publish follows the diff's columns.

## The panel's channels and the release look (2026-11-11)

The owner: "Need a channel picker in debug that enables/disables red/green/blue of the panel output so I can look at channels. Prolly same dev spot as the hitmap and I think that's a good spot for the release look, we can have it always be off and show orange, upon tap in UI it goes black and stays until a reboot."

- **The channels.** Op 63 `[what u8: bits 0-2 red, green, blue on; G#80 asks]` → `[0][mask]`, answered by the receive thread; the map's Developer page has a checkbox each under "panel channels". An off channel is code 0 in the panel's words on every pixel of every phase: the console, the windows, the map, a flight, the hit map's view, the test patterns. All on at every start; never stored.
  - **Where it is applied: the ODT's matrix** (`odt_masked`). The panel's light for a channel is that channel's row of E's gains and K (`neon::Odt` c0 lanes 0, 3, 4 red; 1, 5, 6 green; 2, 7 and c1's lane 0 blue); with the row zeroed its light is 0, the output table's code for 0 is 0 (exponent 0 scales by 0; a −0's exponent index is past the table, which answers 0), and the dither's threshold, at most 63 of the 64 a step needs, never lifts it. So the panel gets code 0 exactly, at **no cost a pixel**: the table is rebuilt once when the mask changes, as for a brightness, and every phase published again (`DEV_GEN`, at the end of the render loop's pass). The words written without the ODT take the mask as one AND a word: `pack10` (the hit map's view, the patterns, op 52's raw drawings), the dev dot, and the 8 bpc `publish`. A screencap (op 30) is working values before the panel, so it has every channel.
  - **The DQE's matrix, not used.** The DQE has a 3×3 gamma matrix in the path (`DQE_GAMMA_MATRIX_CON` G#0C00, its coefficients G#0C04-G#0C14 and offsets G#0C18-G#0C1C in Google's cal_9865 regs-dqe.h; `dqe_quiet` writes its control 0), which could zero a channel in hardware for free. Not used: the coefficients' unity scale, cal's version offset for the matrix block on zuma, and whether an identity matrix passes every 10-bit code thru the display dither's shift mode unchanged are all unmeasured, and the software mask costs nothing a pixel anyway.
- **The release look.** The development look (the orange outside the glass's edge, on its hairline and the camera hole's knockout, `EDGE_ORANGE`; the app circle's hairline, `hairline()`, until 2026-11-12, when the display stopped cutting windows to the circle and its last use went, WINDOWS.md "Drawn under the controls") is a runtime switch on development builds (`LOOK`, from `DEV_LOOK`: FERROS_RELEASE_LOOK at build time still starts a build in the release look). Op 64 `[what u8: 0 ask, 1 release]` → `[0][release]` turns it off until the display starts again: no way back but a reboot, nothing stored. The map's Developer page has its row ("release look", the box and the label in the orange while the orange shows, "until a reboot" under it), and the map's own hairline follows it (`LOOK` there too, asked of the display when the map starts).
- **Release builds** answer both ops with status 1 (every channel on, the release look) and the map shows neither row.

## The frame counter (2026-11-13)

The owner: "can we get a fps counter?"

- **Where.** A console circle, in the USB circle's place while it is on (display `console_draw`, `Corner::Frames`). The top has no free place: the corners hold four circles each, and the spot by the camera is the quick row's doorway (QUICK.md), which a circle there would close. USB is the development link, so the development measure stands where someone who switched it on already looks for one. The layout, the dash's zone and a tap there stay USB's (its page). Not the quick row: it shows only while pulled, and pulling it changes what is measured.
- **The switch.** Developer's "frame counter" checkbox (map), display op 72 `[0 off, 1 on, 2 toggle, 3 ask]` → `[0][on]`, answered by the receive thread. Off at every start and never stored, as the hit map's view and the panel's channels are: a development view, not a setting. On every build that has the Developer page.
- **What it says**, in the person's base as every console number (`frame_lines`): the frames a second as the circle's name ("fps" alone until a second has been measured), then "draw" and the time a frame took when it is known, then "deck" on the deck.
  - An app in front (`front_app`): the display's own count of that window's shows (op 22, counted per surface as each is published), and the time its host says each frame took.
  - On the deck: the map's camera frames published (the stats' `SEC_US` 4) and the display's scene time for each (`SEC_US` 0): the deck's frame is drawn here, from the map's camera.
- **The host's time per frame** comes in op 22's 40-byte form: the 36-byte form and then `[frame us u32]`, the host's draw and present for this frame. A buffer of 0 in bytes 28-35 is the window's own surface (no mailbox: answered at once, as the shorter forms). fluor's ferros host sends it on every frame (fluor branch `ferros-frame-time`); an app on an older fluor shows frames a second alone. A display from before it reads the first 36 bytes as ever.
- **No timer.** Worked out on the display's once-a-second stats tick, over the time the tick actually took, into one word (`FRAME_WORD`) the console's worker reads at its next look (it is nudged every pass).
- **Cheap.** The console keys on the words the counter makes, not on its raw count, so the circle is drawn again only when what it says changed: at most once a second, and not at all at rest (0 fps on the deck stays as it is). A redraw is the circle's text over its region (its body is drawn once), as a battery reading is.
- **State (2026-11-13).** Built; the display and the map stored on the 8a and booted (the deck, settings and the console as before with the counter off). Not yet switched on there: injected swipes stopped turning the settings ring short of Developer within the device slot, so the circle itself has not been seen on glass. Deploy: the display and map capsules (store, reboot, attest); fluor's apps rebuilt on `ferros-frame-time` for the time line.

## Baked (2026-11-12)

Until then every frame's pixels went thru the present: the roll-off (tanh in root space), the ODT to the panel's primaries, the panel's 1/2.19, the four-phase dither and the 10-bit pack, about 9.5 cycles a pixel on NEON (8 the codes, 1.5 the dither and the store). At 4 Mpx and 120 frames/s that is 4.6 G cycles a second, which held a full-screen GPU app near 75 frames/s on the Pro.

The owner: "it's just a block of memory so if the GPU touches it last and we precompute the tanh highlight colours for all elements then it's a straight thru render and a sqrt to get the gamma close (we'd bake the diff in the precomputed colours but that makes the last step a sqrt instead)". On blends after the roll-off and brightness inside the curve: "meh. doubt anyone will notice. also meh. I don't particularly plan on putting quick brightness changing in the UI." And: "just has to look closeish, pixel exact will not be a thing."

### The baked format

- **A pixel** is as before, 64 bits: R, G, B in bits 0-15, 16-31, 32-47, premultiplied, and the fourth lane the canvas's α (or a surface's flags: bit 63 see-through, bits 49-62 the transparency in Q14, bit 48 no dither).
- **A channel** is now the panel's own code for that channel, in Q16 of its top: a working value v (VSF RGB, gamma 2, white 1) goes to l = tanh²(k·v) (k = atanh(√(white/peak)), the brightness's), the panel's light p = ODT · l (E's gains in its rows), and the code p^(1/2.19) × 65472 (the old present's Q4 top, 16368, times four), held at the field's ends. Code 255 of the 8-bit link is 65280; `b >> 8` is the code, and the low byte is the fraction the dither spends.
- **Compositing is unchanged**: front to back, `under`, premultiplied, in these codes. The owner's root is baked in with the rest. The canvas has always held roots (gamma 2 working values; fluor composes in g2t codes, tanh in root space), so a canvas of baked codes composes where it always did, only after the roll-off instead of before it; a canvas of light with a root as the last step would have moved every edge and glow into linear light, which the glows tried on 2026-10-24 and looked worse. The present's last step is then no root at all: each code plus its threshold, the high byte, packed (payloads/display neon.rs `present16`, `present_app16`, `present_pack`): about one instruction a pixel.
- **What it costs**: once per colour, once per image, or per frame only for a window that is not baked (bake.rs):
  - the palette (the theme's colours, the inks, the glows, every page op's colours): `draw::opaque` bakes a colour once a draw op (a 256-entry light table for 8-bit codes, the ODT, a code table over the light's root), and a draw covers the baked colour per pixel (`draw::cover`); the ferros mark's pieces the same, a few a draw;
  - the icons: their mip chains baked as they are built, from the bundle's straight colours (each texel baked, then premultiplied, then averaged down). Not at build time in tools/app-icons: the bake depends on the brightness, and the bundle stays VSF RGB;
  - the wallpaper: the loader puts each code in its pixel's lane as it reads the plane and bakes the buffer in place once the plane is in. It keeps no copy of the codes: a copy in the display's heap (15 MiB on akita, 23 on husky) was tried first and dropped, the arc pool being tight already (on the 8a that day wallgen was refused for room with the old display and the new alike). wallgen is unchanged: what it keeps in the vault is VSF RGB gamma2tanh, the house format at rest, never the panel's codes;
  - the console, the helm and the quick row: drawn in fluor's α + darkness as before, baked as their rows decode (`Bake::g2t_row`, by NEON 16 a step);
  - the button's pill: fluor's squircle kernels draw its three colours as three channels (fill red, edge tones green and blue: fluor's blends are linear, so each pixel's channels are each colour's share), and the pixel is that mix of the three colours baked;
  - a window: format 2 (`BAKED`, ferros_sys `window::BAKED`) is composed and presented as it is; formats 0 (working values) and 1 (fluor's G2T16) are baked by the display every frame, row by row (`layer_row`), what the old present did for them.
- **Blend modes** other than Normal (a window's mode byte) run against the bake's own white (`Bake::white`, white's code a channel), so Multiply by white is still no change. Normal is the code, no division.

### Brightness and the apps

- **A brightness change bakes everything again** (`rebake`, the render thread between passes): the icons in place from the bundle; the wallpaper by stripes on the publish workers, each pixel unbaked with the bake before (its slot stays as it was until the next change) and baked with the new one; the sprites, the region's front layer and clean window made again when next asked; the console, the helm and the quick row by their worker, which sees the bake's generation move and bakes both of its buffers; the map's last frame drawn again; every phase published again.
- **The wallpaper, from its source** (owner, 2026-11-12, on the 8a: "The moment you adjust the brightness on the 8a it bands terribly bad"). The re-bake from its codes is a round trip thru a bake that lost what it lost, and each change started from the last one's result. Measured on the 8a before the fix: brightness 0 to 128 and back moved 97 % of a smooth stretch of the wallpaper (440 × 190 px between the fractal's arms), 46 g2t codes darker on average in every channel, and cut its distinct values from 1141 to 641 (red; green 1099 to 584, blue 1204 to 599). The unbake's root table (4096 steps of √l up to the peak) was read at the step below: a dim bake keeps white at a quarter of the root, so about a thousand steps covered the whole picture, and every step down was a bias toward black. Two changes:
  - the unbake reads its root table linearly between steps and rounds (bake.rs `unbake_with`), so the re-bake at the change is close: it holds the dim bake's own steps (Q4 of a 10-bit code) instead of the table's;
  - once the brightness has held for a render pass after a change, the loader reads the wallpaper's plane from the vault again and bakes it with the bake in use (`wall_refresh`): band by band (three planes' rows in its two stretch buffers, 323 rows on akita, 260 on husky), each row baked in a row of scratch and copied in under a lock the render thread holds from `bake::set` to the end of its re-bake, and only if the bake it was made with is still the one in use. A further change, or a load, supersedes it (the rows done stay; the next settling starts it again), so a drag reads at most one plane's band per step. A whole pass makes the wallpaper exactly what a load at that brightness makes, and every phase is published again. No copy of the source is held: the vault is the source (wallgen's, or the person's own), and the arc pool has no 15 to 23 MiB to spare.
- **An app learns the bake** by display op 65 (`[0][generation u32][k f32][m 9 × f32][g f32]`; ferros_sys `window::bake`), answered by the receive thread at once, and **a change of it** as its window's event `EV_BAKE` (op 62, a doorbell). fluor's ferros host asks at start and at every `EV_BAKE` (redrawing the frame whole), bakes its scratch on the worker cores as its present (fluor `host/panel.rs` `Bake`, the display's NEON kernels carried there, since fluor does not depend on ferros), and shows the window `BAKED`. Mahere's GPU kernel ends with the bake (tanh² on the special function unit, the matrix, 2^(g · log₂ p)), the bake's numbers params of its recording so a brightness change compiles nothing (owner: Mahere's kernel "ends with the matrix + tanh").
- A display without op 65 gets formats 0 and 1 as before from the new fluor, and the new display takes formats 0 and 1 from an old fluor: either can land first. Only format 2 needs both.

### The capture

Op 30 composes the frame as before, baked, and unbakes it into working values (`Bake::unbake`: each code to panel light by a table, the ODT inverted, the root unrolled by a table), the format tools/screencap reads; approximate only where the bake lost something for good (a channel past the panel's gamut, the roll-off's last stops). `[30, 2]` keeps the codes, for comparing looks; `[30, 1]` (the glass's mask) is composed with white in Q14 and not unbaked, its compose being linear. With no free surface there is no capture (the canvas is not unbaked in place: it is the screen's).

### The dither

The owner: "if I remember right, there's some tricks to get a nice dither for our four panes." The phases flip every 100 ms at rest and every frame under a finger, so each phase is seen on its own at rest, and whatever differs from phase to phase is a 2.5 Hz shimmer there.

Measured on the host (a scratch evaluator: every fraction of a code in 256ths, each phase's error and the change from one phase to the next, thru a Gaussian blur of σ px standing in for the eye at arm's length, 1 px about a hundredth of a degree at 489 ppi; root mean square over a 64 × 64 torus, then the mean over the fractions):

| Thresholds | rest, σ 1 | shimmer, σ 1 | rest, σ 1.5 | shimmer, σ 1.5 | rest, σ 0.75 | shimmer, σ 0.75 |
|---|---|---|---|---|---|---|
| until 2026-11-12: 4×4 Bayer + a temporal rank by 4×4 cell (64 levels) | 0.0366 | 0.0416 | 0.0166 | 0.0220 | 0.0682 | 0.0573 |
| 16×16 Bayer, static | 0.0367 | 0 | 0.0161 | 0 | | |
| 16×16 Bayer, its finest 2×2 turned thru the phases (frame-rate control) | 0.0367 | 0.0335 | 0.0161 | 0.0103 | | |
| 16×16 Bayer, its coarsest level turned | 0.0367 | 0.0214 | 0.0161 | 0.0142 | 0.0684 | 0.0287 |
| interleaved gradient noise, offset each phase | 0.0421 | 0.0553 | 0.0212 | 0.0242 | | |
| blue noise, the tile shifted half a tile each phase | 0.0375 | 0.0520 | 0.0156 | 0.0221 | | |
| **16×16 blue noise, its two low bits XOR [0, 2, 3, 1] by phase** | 0.0380 | **0.0169** | 0.0163 | **0.0111** | 0.0796 | **0.0228** |

Chosen: the last (neon.rs `BLUE`, `TURN`). Each phase alone is a full 256-level dither (eight fraction bits, where the old was six), a pixel's four thresholds lie within 3/256 of a code of each other, and its shimmer is 2.5 times smaller than the old one's at σ 1 and the smallest of every candidate at σ 0.75 and 1. At σ 1.5 the 2 × 2 turned thru the phases shimmers a hair less (0.0103 against 0.0111) and twice as much at σ 1, a flicker at the panel's finest period, where the pentile's half-resolution red and blue would alias it. At rest the blue noise is as quiet as the old Bayer at σ 1 and 1.5 and a little busier at σ 0.75 (very close viewing), with no regular cross-hatch. A fresh noise each phase (the tile shifted, or offset by a quarter of a code) shimmers three to four times as much. The root's fractional bits feed the dither directly: the low byte of each code is its fraction, and the threshold is added to it. The ramps (op 21) keep the old dither, being its test screen.

### The GPU, touching last

The owner allows it ("if the GPU touches it last"). Kept for later: with the canvas baked, a GPU present is the dither and the pack alone, a 64-bit pixel in and a 32-bit word out with no arithmetic between but an add; the GPU driver's present kernel (GPU.md "The present stage on the GPU") did the whole ODT bit-exactly in 2026-11-05, so this one is a strict subset of it. It pays only where the CPU's present is the bottleneck, which at about one cycle a pixel it no longer is.

### Measured

On the 8a, 2026-11-12 (Tensor G3, the Pro's chip: its cycles a pixel are the Pro's), the display's start-up bench on the X3 at 2920 MHz (64 rows of the screen warm in the caches, the fastest of eight passes, `display: present bench …`):

| What a pixel costs | Cycles |
|---|---|
| the present until 2026-11-12 (light, ODT, code, dither, pack) | 9.45 |
| **the present now** (dither, pack) | **1.17** |
| with the owner's root first, `fsqrt` (f16) | 2.36 |
| with the root as rsqrte and one Newton step (f16) | 3.09 |
| the bake (an image once, or a format 0 or 1 window every frame) | 10.56 |

- **On the Pro** (4.02 Mpx, 120 frames/s, 482 Mpx/s): the present was 4.6 G cycles a second, about one and a half X3s; now 0.56 G, a fifth of one. A full-screen baked app (Mahere's GPU frame, `BAKED`) costs the display that much and its glass; an app still showing format 0 or 1 costs what it did (the bake per frame, 10.6 cycles a pixel, against 10.3 measured live for Mahere's format 0 frame on the 8a the same morning: 13.8 M cycles of the X3 for its 1246 rows of 1080).
- **A brightness change** (`diag "pref brightness 100"` and back): baked again in 6.7 ms (no wallpaper up), the full publish after it 4.7 ms; the console, the canvas and the edge all at the new brightness in the next capture.
- **The look** (the deck with the settings circle open and its 703 nm glow, the console, no wallpaper: wallgen was refused for room on the 8a that day with either display): the old present's maths on the old display's capture against the new display's baked codes, the largest channel's difference a pixel, in codes of the 8-bit link: 94.7 % of pixels within half a code, 95.0 % within one; 2.7 % between 4 and 8 codes, all in the glow's falloff (now composed after the roll-off: the owner's "meh") and in two live wedges and the console's clock, which moved between the captures; the mean 0.21 of a code. Side by side (each side's panel codes back to VSF RGB the same way) they look the same, the glow's falloff included at four times the gain. Captures: `/mnt/Harbor/tmp/sqrt-present-lookcheck/` (`deck-before-after.vsf`, before on the left).
- **The memory**: the display's arc as before (26 frames of 2 MiB on the 8a); the tables add about 0.5 MiB.
- **On the host**: Mahere's GPU bake against the f64 bake, every pixel of a town's view, within 8.7 of 65472 (valhall's emulator); fluor's bake against its formula.
- **Not yet on the glass**: Mahere and fluor's apps showing `BAKED` (built against fluor's `baked`, host-tested; not stowed on the 8a, which was another agent's kist bench that day). A wallpaper re-baked at a brightness change: on the glass since 2026-11-12, below.
- **The wallpaper at a brightness change**, on the 8a (2026-11-12, the display on forge with the surfaces branch, wallgen's wallpaper): a capture at brightness 255, then 0 and back to 255, then nine changes in a row (200, 150, 100, 50, 0, 60, 120, 180, 255), each compared with the first in three stretches of wallpaper alone (728,000 pixels, the fractal's detail and two smooth stretches): not one value changed, against 97 % moved by one change and back before. The refresh took 213 ms after the last change ("wallpaper baked again from its source"); the re-bake at each change, the wallpaper's unbake included, 29 to 38 ms.

### Deploy order

Everything that bakes lands with the display or after it; nothing breaks either way round:
0. The display (ferros branch `sqrt-present`: payloads/display, ferros_sys). Alone, it bakes its own pixels and every window's (formats 0 and 1) as before, and answers op 65.
1. fluor (branch `baked`) for its apps (Photon, fluordemo) and Mahere (branch `ferros-baked`, needs fluor's): rebuilt against it, they find op 65 and show `BAKED`; against an older display they show formats 0 and 1 as before.

## The row's tail (2026-11-13)

The owner on the 8a: "sometimes after crashes the right twelve pixels or so contain fragments of the crashed app."
- **The cause.** The deck's publish (`publish_band`, and the flight inside it) goes by whole 16-pixel blocks and stopped at the width rounded down to 16. Akita is 1080 wide, 67 blocks and 8 columns, so columns 1072-1079 were written only by an app's publish (`publish_app_rows`, which pads its last block). Once a window went, every phase kept the app's last frame there: the deck's own glass mask and wallpaper never reached them.
- **Measured** on the 8a (the phases read over the bridge, 2026-11-13): after Mahere stopped, columns 1064-1071 held the deck's ground and 1072-1078 Mahere's (8-bit codes G#161414 against G#16161A), in all four phases and every row between the corners; column 1079 is the glass's masked column.
- **The fix.** A row's tail is composed and presented by every publish that reaches it (`tail_front`): the glass's edge, the bars, then the scene (the canvas, a region's text in front, the wallpaper under), or a flight frame's samples (`flight_tail`). The tail is always the glass's (its block reaches past the width, where the mask is), so the block skip has no byte for it. The capture's flight path and the spatial-only `publish10` take the tail the same way.
- **After**, on the 8a: the deck after boot, and after a kist was killed, shows the deck in columns 1072-1078 and the mask in 1079 and the corners, in every phase.
- **husky** (1344 = 84 blocks) has no tail and no change. The left edge starts at block 0, and rows are whole, so the top and bottom have nothing like it.

## Build order

0. **Measure the interactive path** (touch → map → display) to know where the 72 frames/s go.
1. **The console, the helm and the clock as producers** on a worker, into mailbox layers. The render thread only publishes their changed tiles.
2. **Compose once, dither four times**, in tiles.
3. **Coverage maps per layer** and the tile walk with its early-outs.
4. **The stack descriptor and groups**: cold merging in the background, the pointer swap.
5. **Kists' surfaces as mailbox layers** under the same walk.
