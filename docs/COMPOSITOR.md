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

## Build order

0. **Measure the interactive path** (touch → map → display) to know where the 72 frames/s go.
1. **The console, the helm and the clock as producers** on a worker, into mailbox layers. The render thread only publishes their changed tiles.
2. **Compose once, dither four times**, in tiles.
3. **Coverage maps per layer** and the tile walk with its early-outs.
4. **The stack descriptor and groups**: cold merging in the background, the pointer swap.
5. **Kists' surfaces as mailbox layers** under the same walk.
