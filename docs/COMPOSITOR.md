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

Found while reading the path: the open circle's mark was drawn every frame under the page's own ground (the map's opaque `fill` disc), so none of it ever showed; the 14 ms bought nothing a frame. The ground's glow outside the circle does show, so the clean window keeps the scene, made once.

The shape, top down:
- **Two layers, front to back.** The region's text is the front layer, a full screen of working values in the sprite arena (no circle on the map is sprite-sized while a circle is open, so the arena is free; sprites are dropped when the region opens). The clean window is the canvas: the ground disc, then the scene through the camera under it, glows and all, made once per (circle, ground, camera, theme, scene) and kept. The wallpaper stays under both.
- **The protocol.** Landed, the map's frame is `region` (op 18: the circle and its ground) instead of `begin`, then the page's ops, then the camera. The page's ops go to the front layer; the camera draws no scene while the clean window is current and only publishes. A camera op without a region op (the flight, the plane) ends the region, and the old path draws everything as before.
- **Only what changed.** The display keeps last frame's front ops. An op sent again byte for byte is not drawn again; the rectangles of the ops that went and came are merged until disjoint, cleared, and every op that touches them is drawn again clipped to them. Those rectangles are the frame's damage: what is published, and what the other phases owe.
- **Compose once still holds.** `publish_band` reads the front layer for its block skip while a region is up (its clean bit then means "this phase holds the clean window here"), and composes a made block front to back: the text, the canvas under it, the wallpaper under that. Any change to the clean window resets the bits and owes every phase the whole screen, once.
- **Titles ride the circle.** The owner: "rotate around with the circle", not slid left and right and clipped, not spun about their own centres, and no vertical skew. Each page's title stands on the arc about the circle's centre through the title's place, its baseline the arc's tangent there (op 19: text centred, turned, with its own α). Neighbouring pages' titles are the angle apart whose arc is the screen's width, so at the top a title moves with the finger. Turning a page, the outgoing title rides the arc away and the incoming one rides in from the other side, at one size, each at α = cos²(π · its offset in pages), gone at half a page with α's slope 0 there (C¹). The strip's offset is the finger's or its critically damped spring's, counted in display frames: the motion has no clock of its own.
- **Measured** on the Pro (a synthetic swipe across the settings, `FERROS_MAP_START=swipe`, every page op changing every frame): 118-120 frames/s on the panel, against 25. A frame: the front layer 0.7 ms, the publish 3.2 ms (2.3 M pixels of damage, most of it skipped blocks), the worst 5.2 ms. The clean window is made once on landing (26 ms) and the region's end republishes once; the flight between still draws everything a frame (30-40 ms, the mark growing with the circle).
- **What it does not do yet.** The other phases' debt is still one rectangle a phase (`PHASE_DIRTY`); a scroll moves every row, so its damage is the page's band either way. The front layer is not a `Win`/surface: the window table holds the kernel's per-app surfaces and the region needs none; when apps draw into a region of their own circle, the region becomes a window of the map's.

## Build order

0. **Measure the interactive path** (touch → map → display) to know where the 72 frames/s go.
1. **The console, the helm and the clock as producers** on a worker, into mailbox layers. The render thread only publishes their changed tiles.
2. **Compose once, dither four times**, in tiles.
3. **Coverage maps per layer** and the tile walk with its early-outs.
4. **The stack descriptor and groups**: cold merging in the background, the pointer swap.
5. **Kists' surfaces as mailbox layers** under the same walk.
