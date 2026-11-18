# DISPLAY-SCROLL: a scroll frame on the display side, and how to stop moving pixels

Status: a design for the owner to decide from (the rule "display driver: design first").
Written 2026-11-17, ferros forge at 8a993b59, fluor 6fc6ea5, Photon f209cf09.
Built the same day (owner: "your call on the B and B+, I like both"; B first, as B+'s first step): option B (section 2) with knobs 0 and 1 as proposed, and recommendation step 1's split of op 22's call from the publish.
- The display: op 22's 50-byte form (`Ring`, `ring_of`), the row map (`src_row`, `ring_cuts`, `layer_row`, the hit map's rows), op 5's byte 14 (`FEATURES`, bit 0: rings; byte 13 became the Escapes count on forge, op 76, so the features moved to 14 when this merged on 2026-11-20 and fluor's ferros-scroll-ring host must read 14), text ops refused into a ringed window; the stats line's app shows split into the call on the receive thread, the wait in the ring, the publish and the armed frame's wait, and a count of shows that came while the window's last show was still being read (knob 2, measured, not closed).
- fluor (branch ferros-scroll-ring): `paint::ring` and the ferros host's ring, used only when op 5 says the display reads one.
- Knob 14 is not built: on husky's HK3 (`PANEL_HOLDS`) a frame starts only when one is armed, so after a show's own frame no frame starts are counted until something arms again; counting N of them needs either armed frames of the phase on the glass, which delay an on-time app's next frame by a refresh, or a TE count without frames (the TE pin, gpp0-3, as an interrupt). The eighth of a second stays until the owner picks one.

Photon's scroll frames on the Pro take about 17 ms; the target is 8.3 ms (120 a second).
Two of the costs left are the display's side of the frame:
- **the show**, about 4.0 to 4.2 ms a scroll frame in the coordinator's runs: a scroll changes the whole pane, so the display publishes the whole pane into a dither phase again;
- **the surface move**, about 2.5 to 2.7 ms: fluor's ferros host moves Photon's surface (30 MB at 16 bits) by the scroll's rows every scroll frame, so only the damage is converted (the scratch's move is already skipped for Photon, `scratch_unread_outside_damage`).

Section 0 is how the path works today, sections 1 to 3 the options, section 4 the combinations, the recommendation and the knobs.
Section 5 is B+, the owner's direction after reading B: the window as layers (chrome, content, background), each a ring, merged in one pass, with the content in a strip rendered ahead.

## 0. The publish path today, for an app's surface

### The buffers

| Buffer | Size on the Pro (1344 × 2992) | Memory | Who writes, who reads |
|---|---|---|---|
| fluor's scratch | 8 B a pixel at G2t16, 30.7 MiB | the app's heap, cacheable | the app renders, the host converts from it |
| the app's surface (`surface-k`) | 8 B a pixel, 30.7 MiB, always the whole screen | cacheable (taken on demand from the pool, WINDOWS.md "Surfaces on demand") | the host writes baked codes (format 2, `BAKED`), the display reads |
| the four dither phases (`display-phases`) | 4 B a pixel (BGRA1010102), 15.3 MiB each, 64 MiB in all at G#8C00_0000 | **Normal non-cacheable** (cachemap `DISPLAY_PHASES`) | the display writes, the DPU's layer scans the one on the glass |

The DPU never reads a surface: the display composes every window on the CPU into a phase (WINDOWS.md "The GPU's fence follows the frames").

### One scroll frame, step by step

0. Photon's render draws the exposed band and the overlays into the scratch (`damage_rects`, up to four rects since fluor 6fc6ea5) and names the pane's rigid move (`scroll_hint`: the pane's rect and `dy`).
1. fluor's host (`FerrosShell::present_rows`) moves the pane in the surface by `dy` rows (`paint::scroll_copy_rect`, column stripes on the worker cores), then bakes each damage rect from the scratch into the surface (`PanelPixel::to_baked`).
2. The host calls op 22 in its 28-byte form: the window's rect, the surface, level 0, format 2 (`BAKED`), and `changed` = what the present wrote: the moved pane and the damage's box (`FerrosShell::shown`).
   The receive thread answers the 28-byte form at once (`op_is_sync` does not name 22), before anything is published.
3. The render thread (payloads/display main.rs, `Some(22)`) adds `changed` to every phase's owed rectangle (`PHASE_DIRTY`, one rectangle a phase) and publishes **one** phase: k = the phase after the one on the glass, over everything k owes (`publish_app`, rows striped over the render thread on the X3 and the three publish workers on the A715s, `PUB_STRIPE` 32 rows).
4. Per row (`publish_app_rows`):
   - the layer stack's spans give the chain of windows;
   - Photon's chain is Photon alone, baked, not opaque (Photon does not override `FluorApp::opaque`), so its row is copied into a scratch row and every see-through pixel cleared (the `plain` select);
   - the glass, the bars and the camera hole are composed in front on the columns they touch (`glass_cols`);
   - then the present (`neon::present_app16`: each code plus its threshold, the high byte, packed, about 1.17 cycles a pixel) into a row, and the row copied into the phase.
5. The phase is armed for the panel's next frame start (`pipe.settle`, `pipe.arm`), and becomes the phase on the glass.

Under a finger the phases flip with every show, so each scroll frame writes a different phase, and the phase it writes owes the pane of the last four shows (all of it, since each show changed the whole pane).
The other three are brought up only an eighth of a second after the last op 22 (the idle pass, one phase a pass): after a scroll stops, three more publishes of the pane.

### One phase a frame while moving, the rest only at rest

The owner (2026-11-17): "we only need to render one phase as we go since it's likely that whilst scrolling we'll have a different next phase."
This holds for every option in this document:
- **While the content moves**, each frame composes only the phase it shows next. The other three are never caught up meanwhile: each would be stale by the time it came round, since the next frame moves the content again, and its own turn composes it fresh.
- **At rest** the three that are behind are filled, one an idle pass, and only then do the 10 Hz flips resume (the flip already waits for the next phase to be current, `flip_due` reads its owed rect).
- **Every cost estimate here is one phase a frame**: no catch-up pass is counted during motion, and the three catch-ups at rest are counted apart (three pane publishes once, after the last frame).

**What "at rest" means.** Today it is a timer: no op 22 for an eighth of a second (`last_interaction`, the idle pass).
Proposed instead, counted, not timed: rest begins once the panel has started N frames (TE, the frame starts the display already counts, `FRAME_STARTS`) with no new show from the window in front.
- N = 2^1 is the proven floor: a show that comes one frame late (the app missed a refresh) must not start a catch-up that the next show throws away, and a 120 Hz app that missed one refresh is still moving.
- Each catch-up is due before its phase is next on the glass, which is the real deadline: the flips do not resume until the next phase is current, so a slow catch-up delays the first 10 Hz flip and never shows a stale phase.
- A show arriving during the catch-up wins: the catch-up of a phase it owes again is simply dropped (that phase's owed rect grows by the show's change, as now), so nothing at rest ever delays a moving frame by more than the one phase pass in progress.
- N is a knob (knob 14); the count is in the panel's own frames, so it means the same at 120 Hz, at 60 and at the idle floors.

### The dither

- A 16 × 16 blue-noise field (`neon::BLUE`, every rank 0 to 255 once), its two low bits XOR `TURN[k]` = [0, 2, 3, 1] in phase k (COMPOSITOR.md "The dither").
- The threshold of pixel (x, y) in phase k is `BLUE[y & 15][x & 15] ^ TURN[k]`: anchored to the screen, period 16 rows and 16 columns, four phases.
- Over the four phases a pixel visits four thresholds within 3/256 of a code of each other; that pairing is what makes its shimmer the smallest measured (0.0169 at σ 1 against 0.0520 for a field shifted each phase).
- Under a finger the flips run at the frame rate; at rest every 100 ms.

### Measured

From the Pro, Photon build D on Ready, scrolled by injected drags, the lift on and the X3 shared (`/mnt/Harbor/tmp/schedwake/D-on-x3s9-scroll-*`, b922):
- fluor, A#80 scroll frames: move the scratch 5257 us (skipped since), clear 307, render 9408, **move the surface 2720**, convert 2404, show 64 us a frame; drawn and converted 32 % of the frame (before the four damage rects).
- The display's ten-second line: 65 to 106 app shows, **publish 4 to 5 ms a show, waited for the armed frame 0 ms**.
- The display's parts line (512 shows, scroll and whole frames mixed, the threads' time summed a show): compose 10.3 ms, glass 5.2, codes 0.47, store 8.5; rows a show 847, 864, 868 on the workers and 1028 on the render thread; publish 5.1 ms of wall time a show.
- THERMAL.md decision 3's table: a scroll frame with the lift on about 28 ms then (draw 19.6, present 6.1, show 1.9).

So the display spends about 24 ms of thread time on a show whose floor is far lower:
- the pane read once (8 B a pixel) and written once (4 B a pixel): about 42 MB for a pane of 3.5 Mpx (most of the screen; Ready's pane size is still to read off the device), 1.1 to 1.4 ms at the 30 to 40 GB/s four threads reach (THERMAL.md, `std/membench`);
- the present's arithmetic, 1.17 cycles a pixel: about 4 M cycles, under half a millisecond over four cores.

Where the rest goes, by the parts line: compose (the copy of every pixel into the scratch row and the see-through pass, because Photon is not opaque), the glass and bars rows, and the store (the dither and pack, then the row's copy into non-cacheable memory, with compiler-builtins' word loop: no_std payloads do not have ferros_rt's `memcpy` yet, STD-TARGET.md).

What the 4.0 to 4.2 ms show is: the 28-byte op 22 is answered before the publish, so the call's own time is the receive thread getting a core and a ring slot, while the 4 to 5 ms publish runs on the display's threads, three of them on the A715s that Photon's rayon workers also need (KERNEL.md Order item 8: the A715s are saturated in a scroll).
Either way the publish is time taken from the cores Photon's frame runs on.
Splitting the two (the display's stats line and `diag sched` over one scroll) is the first measurement any option here should start from.

## 1. Option A: the display shifts its own phases instead of publishing the pane

### What it does

The display keeps, per phase, the pane's shift it has not yet applied (`Δk`, the sum of the `dy` of the shows since phase k was last written) and what it owes besides (rects).
When phase k comes round: move the pane's rows in phase k by `Δk`, then publish only what it owes, which is the damage of the shows since, each moved by the shifts after it.

### Why that is exact for the content

Take phase k last written at show a, and the shows a+1 to now, each a rigid move `d_f` of the pane and then damage `D_f` (fluor's `scroll_hint` contract: the exposed band is inside the damage).
Trace a pane pixel back from now: at each show either it lies in `D_f` (new at that show) or it came from `d_f` rows away inside the pane; by the contract a trace that would leave the pane hits the band first.
So a pixel no damage touched is the pixel `Δk` rows away in phase k's old frame, and phase k shifted by `Δk` holds it, word for word.
What phase k must publish is therefore the union of each `D_f` moved forward by the shifts after show f (`d_{f+1} + … + d_now`).
For Photon that is the exposed band (the rows that entered, `|Δk|` of them when the scroll keeps its direction) and its fixed overlays (the orb, a toast, the standing bands) where they sit now and where phase k's old copy of them lands, `Δk` rows away.
Photon already names its overlays this way for one frame; the display applies the same rule over four frames' shifts.

### Where it must not shift

The phase holds the composite, not the window, so a shifted row also moves everything composed in front of or behind the window there:
- **The system's front layers.**
  - The glass's edge and corners, the camera hole, the console's circles and the helm's wedges are composed into the same words (`glass_front`, `bars_front`).
  - The rows the glass plan touches (`GLASS_ROWS`: a mask column, an edge pixel, a bar or the hole) are never shifted; inside the pane they are published as today.
  - On husky those are the chrome rows at the top and the bottom; a pane that reaches into them republishes those rows only.
- **Other windows.**
  - A row of the pane is shiftable only where the scrolling window's span chain is the window alone (or the window is opaque): a keyboard in front, or a window behind showing thru a see-through pixel, would be moved with it.
  - The layer stack's bands (`layout_build`) say this per row once, when the windows change.
- **The hit map's development view**: its phase is painted whole from the hit map (`hit::on_layer`) and is never shifted.

### The dither: what is exact and what is not

Moving a phase's words by `Δ` rows carries each word's threshold with it: after the move, row y holds the threshold of row y − Δ.
- **`Δ` a multiple of 16** (the field's period): the moved words carry exactly the thresholds a fresh publish would give them, bit-exact.
- **Any other `Δ`**: each phase alone is still a whole blue-noise field (a translate on the 16 × 16 torus keeps every rank once and the spectrum), but:
  - the four phases are shifted by different amounts (each by the shows since it was last written), so a pixel's four thresholds are no longer the designed quartet: the shimmer becomes that of a field shifted each phase, about three times the chosen one's, until each phase is published again;
  - where a moved region meets a freshly published band (screen-anchored), two unrelated noise fields abut: one row of seam;
  - every row of the pane is then owed once more at rest (as the pans' "drawn once more for real at rest", COMPOSITOR.md): four pane publishes after the scroll stops, one an idle pass.
- **Content-anchored dither in the pane** (the exact choice):
  - the pane's thresholds are taken at the content's row, `BLUE[(y − D) & 15]`, where D is the pane's total scroll, mod 16;
  - a moved word then keeps the threshold its content row has, and the band published now uses the same D: every shift is bit-exact, any `Δ`, with nothing owed at rest;
  - the cost is nothing a pixel (`neon::row_thresholds_at` takes the row; one subtraction a row);
  - on the glass, the noise in the pane rides with the content while the chrome's stays put, and the two fields meet at the pane's edge, which is a content edge already;
  - at rest nothing differs from today;
  - this is the owner's look to judge.

### Cost

The shift is a move of 4 B a pixel inside the phase: for a 3.5 Mpx pane, 14 MB read and 14 MB written.
- **In today's phases (non-cacheable)**:
  - reads are the slow direction: about 300 ns a read (SHELL.md), 14.4 ms to copy one binned camera frame out with 64-byte loads (CAMERA.md);
  - even at 2 GB/s over four threads the move is 7 ms, **slower than the publish it replaces**;
  - Option A is not worth building on non-cacheable phases.
- **Phases made write-back cacheable**, the written lines cleaned to the point of coherency before the phase is armed:
  - the move is about 28 MB of traffic (about 1 ms over four threads) plus the clean (`DC CVAC` over the moved lines, bound by the write-back the move needs anyway);
  - EL0 may not clean today: SCTLR_EL2.UCI is 0 on purpose (cachemap.rs), so this needs UCI = 1 or a kernel call that cleans a granted range;
  - against a publish brought near its floor (section 0, about 1.1 to 1.4 ms of traffic for the same pane), the saving is a few tenths of a millisecond a frame.
- **A hardware mover** (zuma's G2D blitter) would take the move off the CPU, but G2D has never run under ferros, has its own S2MPU and EL3 protection call, and is the block whose off hung two jumps (HV.md "The g2d hang"): a project of its own.
- The damage and the band: as today, a publish of those rects alone.

### What op 22 must carry

- The scroll: the pane's rect and `dy` (signed rows), each show, so the display never infers a move from pixels.
- The damage as rects, not as the moved pane's box: today `changed` is the pane ∪ the damage, which would force the whole pane again.
- A form after the 40-byte one: `[pane x u16][y u16][w u16][h u16][dy i16][n u8]` then n rects of `[x u16][y u16][w u16][h u16]` (fluor names up to four).
- The display falls back to today's whole-pane publish for any show it cannot shift (a different pane, a `dy` past the pane, a layout change).

### Memory

Per phase a shift and a short list of owed rects (the union of up to four rects for each of four shows, merged): a few hundred bytes.
Cacheable phases change nothing in size (still 64 MiB), only their mapping.

## 2. Option B: zero-copy, the surface's pane becomes a ring

### What it does

Nothing moves on a scroll: the pane's rows in the surface become a ring, and a scroll moves the ring's origin.
For the ring's rect (x0..x1, y0..y1, H = y1 − y0 rows) and origin o, screen row y inside the rect is read from surface row `y0 + ((y − y0 + o) mod H)`, at the rect's columns only; every other pixel of the surface is where it always was.
A scroll by `dy` (positive: the content moved down) is `o ← (o − dy) mod H`, which is exactly what a memmove by `dy` makes the reader see, with one difference: the rows a memmove would leave stale (the band) hold the rows that left at the other edge instead.
Both are inside the damage by `scroll_hint`'s contract (the band is always repainted), so the app sees no difference.
`(y − y0 + o)` is under 2H, so the mod is one compare and one subtraction, a row.

### fluor's host (src/host/ferros/mod.rs)

- `FerrosShell` keeps `ring: Option<(PixelRect, usize)>`, the rect and its origin.
- A hinted frame (`self.shift`) whose rect is the ring's (or with no ring yet: a ring starts at origin 0) moves the origin instead of calling `scroll_copy_rect` on the surface.
- `present_rows` writes each damage rect's row y into surface row `map(y)`; a rect that crosses the wrap is two runs of rows; the bake (`to_baked`) is unchanged, row by row as now.
- The ring is let go (back to origin 0) when the next frame is not a hint on the same rect:
  - a whole frame (the first, back in front, a new bake, the app's own whole frame) converts everything anyway, so origin 0 is free there;
  - otherwise (a new pane rect, a hint dropped while o ≠ 0) the rect is rotated in place once, which costs one of today's moves, once.
- An own frame (`render_own`, the GPU's layer, the mailbox's 36-byte form) is always whole: no ring.
- The scratch is untouched by this: apps that read it outside the damage still have it moved (fluor's own `scroll_copy_rect`), Photon does not.

### The scroll contract and the other hosts

`FluorApp::scroll_hint`, `damage_rect`, `damage_rects` and `scratch_unread_outside_damage` keep their meaning and their code; the ring is the ferros host's own way of honouring a hint.
Android and the desktop host keep the memmove: their present buffers are the platform's (a NativeWindow buffer, softbuffer's), read linearly by something that takes no origin.
Photon changes nothing.
Its own memmoves inside the render (its bg layer and its hit map, driver.rs "SCROLL AS A MEMMOVE") are Photon's and stay; the same ring idea could reach them later, in Photon.

### op 22

The show names the ring with every frame, so the display never holds ring state the app did not just state: a form after the 40-byte one, `[ring x u16][y u16][w u16][h u16][origin u16]` (a height of 0: no ring).
The display reads the ring with the show it came with; a show without it reads the surface as it is.
The `changed` rect stays what the present wrote: the damage, and on a ring frame the ring's rect (its rows all moved for the reader).

### The display's reader

- `publish_app_rows` takes each row's source row from the ring: one compare and one subtraction a row, no division, nothing per pixel.
- A ring as wide as the surface (x0 = 0, x1 = width) is one source pointer a row, and the `direct` and `plain` paths read it as they read a row today.
- A narrower ring splits a row into at most three runs from two source rows; the 16-pixel blocks a run edge falls inside are taken thru the scratch row, as the glass's touched blocks are now (`touched`).
- The capture (op 30) goes thru the same rows and needs nothing more.
- The text ops into a surface (26, 29) write at screen rows: they map their rows thru the ring too, or the display refuses a text op into a ringed window (fluor's host never uses them).
- The display's hit map is by window, not by pixel content: unchanged by a scroll (Photon's own fluor hit map is Photon's).

### Edge cases

- **A pane that is not the whole surface**: the ring is the pane's rect; the rows and columns outside it are read in place.
- **Chrome above and below the pane** (Photon's header, its compose bar): outside the rect, never rotated.
- **Fixed overlays over the pane** (the orb, a toast): they are pixels of ring rows like everything in the rect, so after an origin change they appear moved, as after a memmove; Photon already repaints each where it sits and where its moved copy lands.
- **The display's own front layers** (glass, bars, hole): composed in front at publish as today, from screen coordinates, so a ring changes nothing for them.
- **A window behind Photon** thru a see-through pixel: composed per pixel at publish as today; unaffected.
- **The answer before the publish**:
  - the 28-byte op 22 is answered before the display reads the surface;
  - today the next frame's move of the whole pane can land while the display is still reading the last one (a tear by the scroll's rows inside one phase, if the publish runs late);
  - with the ring only the next frame's band rows can land early, and under the old origin those are the rows leaving at the far edge: at worst a sliver there;
  - the ring does not create the race and narrows it; closing it is answering op 22 once the publish has read the window's rows (a knob, below).

### Cost

- fluor: the surface move goes, about 2.5 to 2.7 ms of the four A715s a scroll frame; a rare rotation when a pane changes under a live ring.
- The display: a row's index computed per row; the publish of the pane itself is unchanged: one phase a show while the content moves, the other three filled only at rest (section 0, "One phase a frame while moving"), each read thru the ring's origin as it then stands.
- Memory: none (the ring lives in the surface's own rows).
- Exactness: total, since the display reads the same pixels the memmove would have put there, and the dither stays screen-anchored as now.

## 3. Option C, for completeness: the panel reads the phases at an offset

Option A's move is its cost.
The DPU could do it for free by reading the pane's rows of a phase at an offset: Option B's ring, one stage later.
- The phase is cut into full-width row bands, each a DECON window fed by its own DPP channel: the rows above the pane, the pane as a ring (two windows, one from the origin to the ring's end and one from its start), the rows below.
- The windows are opaque and do not overlap, so DECON's blend only selects (COMPOSITOR.md "Not used": hardware windows serve only pure selection); no Porter-Duff over is introduced.
- A scroll is then the band's publish into the ring plus base-address writes for the two pane windows, per phase, at the shadow update: no pixel of the pane is read or written.
- The rows with the system's front layers (the glass's corners, the console, the helm, the hole) belong to the fixed bands, published as today.
- The dither must be content-anchored in the pane (section 1), since the pane's words never move relative to the content.
- Instead of two windows a phase's ring can be laid out twice (every row written at r and r + H), so one window reads H rows from any origin: one window fewer, the band written twice, and the phases about doubling (beyond the 64 MiB below the firmware carveout at G#9000_0000: the region would move).
- What it needs: DECON0's further windows and DPP channels brought up (ferros runs one layer, window 5 and DPP0, today); their SRAM, QoS and IRQ words from the oracle's dumps; every register read checked against the poison rule (DISPLAY-DESIGN.md §8).
- A project, not a change; only worth it if, after B and a trimmed publish, the display's publish is still the wall.

## 4. Combinations, recommendation, knobs

### What each removes from a scroll frame

| | fluor's surface move (2.5 to 2.7 ms) | the display's pane publish (4 to 5 ms, about 24 ms of thread time) | exact | size of the change |
|---|---|---|---|---|
| B | gone | stays | yes | fluor's host and the display's row reader, one op 22 form |
| A, non-cacheable phases | stays | replaced by a slower move | content yes; dither only at Δ ≡ 0 mod 16 or content-anchored | per-phase shift bookkeeping, op 22 form |
| A, cacheable phases | stays | replaced by a move about 0.3 to 0.5 ms cheaper than a trimmed publish | as above | as above, plus UCI or a clean call, plus the phases' mapping |
| a trimmed publish (below) | stays | toward its floor, about 1.1 to 1.4 ms | yes | the display alone |
| B + a trimmed publish | gone | toward its floor | yes | both |
| B + C | gone | the band only | yes, with content-anchored dither | the DPU's windows |

A trimmed publish is neither A nor B, but the measured parts show it is where most of the 4 to 5 ms is; each item is a measurement first (the owner's rule: measure before fixing):
- **Photon opaque**: if Photon's frame never has a see-through pixel on ferros (the glass edge is the display's), `FluorApp::opaque` true sends the display's mode bit 3, and every row takes the `direct` path: no copy of the row, no see-through pass (the "compose" part, about 40 % of the thread time measured).
- **The present straight into the phase**: `present_app16` stores into a row that is then copied into the non-cacheable phase; storing into the phase itself drops a copy.
- **ferros_rt's `memcpy` for no_std payloads**: the display's copies run compiler-builtins' word loop today (STD-TARGET.md).
- **The workers' share**: the stripes are taken by three A715s and the X3; in a scroll the A715s are Photon's, so the render thread's X3 taking more of the publish is a knob.

### Recommendation

0. **Build B.**
   - It removes the whole surface move (about 2.5 to 2.7 ms of the four A715s a frame) with no change to the picture, to the scroll contract, to Photon, or to Android and the desktop; the display's side is a row map.
   - Its one cost is a rotation when a pane changes under a live ring.
1. **Measure, then trim the publish.**
   - Split op 22's call time from the publish (the stats line and `diag sched` over one scroll), then take the trims above in that order.
   - The 4 to 5 ms publish is about three times its own floor, and every millisecond of it is A715 time Photon is short of.
2. **Do not build Option A.**
   - On non-cacheable phases the move is slower than the publish.
   - Made cacheable it saves a few tenths of a millisecond over a trimmed publish, while adding EL0 cache cleaning (UCI is 0 on purpose), per-phase shift bookkeeping, the exclusion of the system's rows and the dither question.
3. **Keep C for later**, if after 0 and 1 the publish is still what stands between Photon and 8.3 ms: it is the only path on which a scroll costs the display the band alone.

With B, the frame loses its 2.5 to 2.7 ms move; a trimmed publish then gives back most of the publish's A715 time.
What remains is Photon's own render and the band's convert (9.4 and 2.4 ms in the run above, before the four damage rects), which the display side cannot reach: 8.3 ms needs Photon's side too.
Section 5 (B+, the owner's layers and strip, added the same day) is the design that takes that render off the scroll frame; its recommendation is there.

### Knobs (the owner chooses)

0. **B or not**; and when a pane changes under a live ring: rotate the rect once in place (one move, then origin 0), or convert the rect whole from the scratch (only for apps that keep their scratch whole).
1. **op 22's form for the ring**: appended to the 40-byte form (stated with every show, nothing held between shows: proposed) or a separate op that sets it.
2. **When op 22 is answered**: at once as now (the app may write the surface while the display reads it), or once the publish has read the window's rows (no race; the app then waits the publish, unless it draws into a second buffer as the mailbox form does).
3. **Photon opaque**: a question for Photon's frame on ferros (does any pixel stay see-through), then one line in Photon.
4. **The publish trims**, in the order measured.
5. **The dither in a moving pane** (Option A or C only): screen-anchored as today (exact only for shifts of a multiple of 16 rows; the pane owed again at rest) or content-anchored (exact for every shift; the noise rides with the content inside the pane), a look to judge on the glass.
6. **The phases' memory** (Option A only): non-cacheable as now, or write-back with a clean before arming (SCTLR_EL2.UCI = 1 for EL0, or a kernel call that cleans a granted range).
7. **The DPU's windows** (Option C): two windows for the ring's wrap (no memory) or one window over a doubled ring (the phases' region moved past the carveout).
8. **G2D as a blitter**: not proposed; noted only as the hardware way to move pixels, with its hang after jumps unexplained.
9. **B+ (section 5)**: the window as layers (chrome, content, background), each a ring, merged in one pass; with or without the content strip.
10. **The strip's height** H_s: the pane plus how many rows rendered ahead each way (memory against how far a fling runs before a band is rendered on the frame).
11. **Surfaces for an app**: B+ wants four or five slots of the sixteen for Photon (noise, chrome, the strip's two); how an app asks for more than its spawn's one.
12. **Which layers**: the three the owner named; whether Photon's fixed overlays drawn into the target (the wave, the chord hint, the toast) move into the chrome layer later.
13. **Text over the noise composed after the roll-off** (B+ composes baked layers): accept, as on the deck, or keep the noise flattened under the content in fluor (which gives back the strip's cleanness).
14. **At rest**: N panel frame starts with no new show before the three stale phases are filled (proposed N = 2, counted in the panel's frames), in place of today's eighth of a second.

## 5. B+: the window as layers, each a ring, merged in one pass

The owner, 2026-11-17, on B: "render a big strip and just chunk our piece we want out", and "I'd assume we layer it so we can wrap the background and keep the content separate so it's one merge pass?"

### The idea

The app hands the display its window as a few layers instead of one finished surface, each with its own source and its own ring origin:
- **the chrome** (fixed): fluor's chrome layer (`DefaultChrome`'s orb, title strip and buttons), never moved;
- **the content** (the scrolled pane): the list, drawn with no background under it, in a **strip** taller than the pane, a ring whose origin is the scroll;
- **the background**: Photon's noise wallpaper (fluor's `NoiseField`), generated once for the frame's size, baked once, and only rolled vertically with the scroll, never scaled.

The display composes them for each output row, front to back, straight into the phase with the dither, in one pass: the glass, bars and hole first (the system's, as today), then the chrome, then the content, then the background; black behind is never reached, since the noise is opaque.
Each layer is read at its own source row: one row index per layer per output row, nothing per pixel.

"Render a big strip and chunk our piece out" is the strip: the app renders the content's rows ahead of the viewport, and a scroll that stays inside what is rendered is a new origin and nothing else.
That is where B+ goes past B.
B removes the moves; B+ with a strip removes the **render** from a scroll frame (Photon's 9.4 ms and the 2.4 ms convert in the run of section 0), which is what stands between Photon and 8.3 ms.

### The rings

- A layer is a source (a surface, or adjacent slots for a tall strip), its format (`BAKED`), the screen rect it covers, the source's height in rows and the origin.
- Screen row y of the rect reads source row `(y − y0 + origin) mod H`: one compare and one subtraction a row, as in B.
- The noise is two layers side by side, one for each half of its mirror (`NoiseField`'s right and left strips, split at its axis), each half with its own origin.
  Settings' split (the rail and the content, each half of the noise with its own pane) is then two rings and no special case.
  Its height is the frame's: `NoiseField` already wraps at h rows, so its ring is exactly the field.
- The content strip is H_s rows: at least the pane's height plus the rows rendered ahead in each direction.
  A second pane scrolling on its own (Settings' rail) is a second content layer.
- The chrome has no ring.
- Each layer carries its coverage as COMPOSITOR.md rule 2 already decides for layers: 64 × 64 tiles in its own coordinates, empty, opaque or partial.
  The display maps a tile thru the ring like a row, skips the layer in an empty tile and ends the walk at an opaque one.
  The early-outs are per tile and per span, never per pixel (rule 8).

### Front to back and darkness space (AGENT.md "Compositing")

- The walk is `under` only, frontmost first into an empty accumulator, as `fold` and `under_pm` do today: the chrome, then the content under it, then the noise under that.
- No Porter-Duff over, no `1 − α` between layers.
- Each layer is premultiplied (fluor's α + darkness, baked to panel codes with its transparency in bits 49-62, as a `BAKED` surface is), so the content's see-through pixels let the noise thru by their α and nothing else.
- The noise is opaque: where it is reached, the walk ends.
- One consequence: fluor today composes the content over the noise in its g2t codes and the result is baked; in B+ each layer is baked first and composed in panel codes, after the roll-off, as the deck already is (COMPOSITOR.md "Baked", the owner's "meh, doubt anyone will notice"). The antialiased edges of text over the noise move by a code or so.
- The dither stays screen-anchored and exact: every scroll frame composes the whole pane into the phase, as today.
- One phase a frame (section 0, "One phase a frame while moving"): while the content moves, each frame's one pass composes only the phase shown next; the other three are filled only at rest, from the layers as they then stand (three more passes, once). The costs below are that one pass.

### What it removes, per scroll frame

For a pane of about 3.5 Mpx, the passes over pane-sized data in one scroll frame and their memory traffic per pane pixel:

| | today (fluor 6fc6ea5, Photon f209cf09) | B | B+ without a strip | B+ with a strip |
|---|---|---|---|---|
| Photon: its hit map moved (`scroll_hit_map`, u16) | 4 B | 4 B | 0 (a ring too, below) | 0 |
| Photon: the band's content drawn, the noise laid under it | the band | the band | the band, no noise | nothing on the frame (rows rendered ahead) |
| fluor: the surface moved | 16 B | 0 | 0 | 0 |
| fluor: the band converted (baked) | the band | the band | the band | nothing on the frame |
| the display: read, compose, present into the phase | 8 B read, 4 B written | 8 + 4 | content 8 (not in empty tiles), noise 8 (not under opaque tiles), chrome in its tiles only, 4 written | the same |
| pane-sized passes | 3 | 2 | 1 | 1 |
| traffic a pane pixel | about 32 B | about 16 B | about 20 B, less where the content's tiles are empty | about 20 B |
| at 30 to 40 GB/s, four threads | 2.8 to 3.7 ms | 1.4 to 1.9 ms | 1.75 to 2.3 ms | 1.75 to 2.3 ms |

What this says:
- B+ reads the noise every frame wherever the content is not opaque, which is most of Photon's list (rows of text on the noise), so its one pass carries more bytes than B's: the layer split **alone** does not pay for a pure scroll.
- What it removes is the noise's flatten under the band (small), the orb's smeared copy in the damage (the chrome is no longer in the pane's pixels), the hit map's move, and, with the strip, the app's whole render and convert on a scroll frame.
- The compose has arithmetic today's select does not: `under` of up to three layers with partial α, in NEON on the baked codes. An estimate to bench on the display's start-up bench: 2 to 3 cycles a pixel, 7 to 10 M cycles a frame, under a millisecond spread over the X3 and the three workers, overlapping the memory traffic.
- So a pure scroll frame inside the strip costs the display about 2 to 2.5 ms of its four threads and Photon no render: inside 8.3 ms with room, where today's frame is about 17.
- A whole frame (a pinch, a screen change) renders as today, but the noise is not in it, so it converts the content only. The noise is baked once per brightness (32 MB at 10.6 cycles a pixel, about 15 ms over four cores, once per `EV_BAKE`).

### The window protocol

A new op (its number taken when built) beside op 22, which stays as it is for every app that hands one surface.
Its message: `[x u16][y u16][w u16][h u16]` the window's rect, `[level u8]`, `[mode u8]`, `[frame us u32]`, `[n u8]`, then n layer records, frontmost first:
- `[source u64]`: the surface (or the first of adjacent slots) the layer's pixels are in; the display's "Whose surface" check applies to each;
- `[format u8]`: 2, `BAKED`, for every layer fluor hands;
- `[flags u8]`: bit 0 opaque (the walk ends here: the noise), bit 1 fixed (no ring: the chrome);
- `[x u16][y u16][w u16][h u16]`: the screen rect the layer covers;
- `[src y u16][src h u16][origin u16]`: the ring's first row and height in the source, and its origin;
- `[tiles u64]`: where the layer's coverage map is (a byte a 64 × 64 tile, in the source's coordinates), 0 for none (every tile partial).

Then `[n u8]` and up to four changed rects in screen coordinates, as fluor names its damage: what changed in any layer apart from an origin.
An origin's change is a change of that layer's whole rect, which the display knows from the records.
A few hundred bytes, well inside a message slot (4096).
Every show states every layer, so the display holds no layer state the app did not just state; it keeps the last show's records only to see what moved.

The display's side:
- the window table holds the layer records instead of one surface;
- `layout_build` is unchanged (the window's rect, level and mode);
- `publish_app_rows` walks a window's layers inside its span: per row each layer's source row from its ring, per tile its coverage, then the `under` walk and the present;
- the display's hit map is unchanged: it is by window, and the window owns its rect as it does today (the noise is opaque, so no pixel of a B+ window shows a window behind it);
- the race of section 2 closes: with a strip the app writes only rows outside the viewport of every show still being published (it keeps the last shows' origins as its guard), so the display never reads a row being written.

### fluor's side

fluor already holds the three apart until the end of a frame:
- `DefaultChrome::group` (its bg, chrome and hover layers), flattened into the scratch by the app (`flatten_chrome_into`, `flatten_bg_into`);
- `NoiseField`, laid under the scratch by the app at the end of its render (`under_into`), generated once per size and base and kept at 16 bits;
- the scratch, which the app draws its content into.

The change is where they meet:
- **The host takes the layers.** `Context` calls (say `ctx.lay_noise(&mut field, origin, left_scroll, right_scroll)` and `ctx.lay_chrome(&chrome)`) that a host either does as today (Android and the desktop: `under_into` and `flatten_chrome_into` into the frame, the same pixels) or takes as layers (ferros: the noise baked once into a surface of its own, the chrome baked into another when it changes, nothing flattened into the scratch).
- **The content strip.** The scratch becomes the strip for the pane: H_s rows rendered by rows in strip coordinates; the host bakes the rows it renders into the content surface's ring rows (the convert, off the frame when the rows are rendered ahead).
- **The hit map as a ring.** The chrome's hit map stays fixed; the content's hit ids live in a u16 strip with the content's origin, and a hit test reads the chrome's first, then the content's thru the ring: front to back, like the pixels. `scroll_hit_map` goes.
- **The scroll contract.** `scroll_hint` keeps its meaning (the pane's rect and `dy`) and becomes the content layer's origin change; a host that cannot ring moves as today.

### Photon's side

Not none, but small, in two steps:
- **The layers**: two calls changed, the noise's `under_into` and the chrome's flatten, to the `Context` ones.
  fluor's host cannot take them over as written: Photon builds its own `Canvas` over the target for `under_into`, so the host never sees the call.
  The wave, the chord hint, the toast and the other fixed overlays Photon draws into the target stay in the content layer under today's rule (repainted where they sit and where their moved copy lands); they can become chrome later.
- **The strip**: Photon's pane renderer draws a range of the list's rows in strip coordinates instead of the viewport's, and renders the rows ahead of the scroll (in the fling's direction) on idle frames.
  Ready already builds only the rows near its damage (Photon 64422e80), which is most of the way.
  A scroll past the rendered rows renders its band on the frame, as today.

### Android and the desktop

Unchanged at first: their hosts do the `Context` calls as today's flatten and `under_into`, and keep the memmove.
They can gain the strip later: a host that composes the three layers itself into its window's buffer does one `under` pass a frame (as many as its memmove today) and no render on a scroll.

### Memory on the Pro

| Layer | Size |
|---|---|
| the noise, baked, the frame's size | one surface slot (32 MiB), beside fluor's 16-bit strips (30.7 MiB, kept today already) |
| the chrome, baked | one slot (mostly empty; its tiles say so) |
| the content strip, baked | ceil(H_s × 1344 × 8 B / 32 MiB) adjacent slots: 2 for two pane heights |
| the content strip in fluor's α + darkness (the scratch) | the same size again, in the app's heap |

Four or five of the sixteen slots for Photon, against one today; the kernel gives one surface at spawn, so an app needs a way to ask for more (knob 11).

### Cost estimate per scroll frame (the Pro, a pane of about 3.5 Mpx, inside the strip)

- Photon: input and layout, no render.
- fluor's host: the new origins in the show; no move, no convert.
- The display: one pass, about 2 to 2.5 ms of its four threads.
- Rendering ahead: the rows the scroll will need, in idle time on the A715s: the rows the band costs today, off the frame and without the noise under them.

### Recommendation for B+

0. **Build B first** (section 2): small, exact, removes the move now, and its ring and row reader are B+'s.
1. **Then B+ with the strip, not the layer split alone.**
   - The split by itself trades the move for a read of the noise every frame, about even.
   - It pays once the strip takes the render off the scroll frame, the only path here to 120 a second in Photon's scroll.
   - The split is what makes the strip clean: the noise is never rendered into the strip, it rolls under the fixed rows too, and Settings' two panes are two rings.
2. **Bench before building**: the `under` walk of two and three layers in NEON on the start-up bench (cycles a pixel), and Photon's pane coverage (how much of the list is empty, opaque or partial in 64 × 64 tiles), which sets the real bytes a pixel.
