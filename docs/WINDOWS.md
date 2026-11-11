# Windows on ferros

The design for putting more than one thing on the glass: the map, an app in front, the keyboard over it, and the attest screen over everything. Written 2026-10-27 after the owner called the present path "a glitchy hot mess". The owner decided on 2026-10-27: every window is full screen for ownership and layering; each app gets its own full-screen buffer to draw as it wishes, and the display draws them in order, frontmost first; an app in front lives in a full-screen circle tangent to the screen's top and bottom, the four corners left for the system (clock, signal, navigation). How widgets fit is still open. "Built" below says what runs.

## What goes wrong today, and why

The present path is one slot. The display server has one `APP` (a rectangle), the kernel has one 32 MiB `app-surface` region granted to every app, and every process that wants touches polls the touch server. Each glitch traces to that:

1. **The map's frames reached the glass under an app.** The map kept drawing (its dashboard redraws every second, a bridge-spawned app never stopped it), and each of its frames went into one of the four dither phases while the app held the other three. The 10 Hz flips then showed both. Fixed 2026-10-27: `publish_next` publishes nothing while an app is in front, and op 23 already repaints the map in full.
2. **Every app shares one surface.** Photon, Ramps and Hive all get the same `app-surface` block. Two apps alive at once draw into the same memory, and a keyboard over an app has nowhere to draw.
3. **The display cannot tell its clients apart.** `recv` hands a server a reply token and nothing else (KERNEL.md: no PID, no ambient authority). Any client's op 22 takes the screen and any client's op 23 ends whatever app is in front.
4. **A dead app keeps the glass.** Nothing tells the display that a process ended, so its last frame stays up and the map waits for an op 23 that never comes. Restarting the display drops a live app's frame instead.
5. **Touches go to everyone.** Each process polls the touch server and sees every finger, so the map and the app both act on one tap. The map now stands aside while the display says an app is in front, but that is a convention, not a rule. A HELD poll also takes about 15 ms (measured 2026-10-27), so polling clients see touches at about 66 Hz.
6. **The text op assumes a full-screen surface.** Op 26 indexes with the panel's width, while op 22 lays a frame out at its own width. Fluor's centred half-size frame and text cannot be used together.
7. **Op numbers collided.** The frame wait took op 24 on 2026-10-24, which was already text. Every text call since drew nothing. Text moved to op 26 on 2026-10-27.

## The model

On the phone, apps are places (SHELL.md), not overlapping windows. So the window manager is a short stack of layers, not a desktop:

| layer | what | how many |
|---|---|---|
| 0 home | the map | one, always there |
| 1 front | the app in front, full screen | at most one shown; others alive behind it |
| 2 input | the keyboard, docked at the bottom | one, while the front app wants text |
| 3 system | attest (ahau), the power-key states | one, above everything, takes all input |

A pixel comes from exactly one layer, because the layers do not overlap where they are opaque: the keyboard's band is the keyboard's, the rest is the front app's. So composing is a list of row spans per layer, not blending. That keeps the one-table, dither-at-full-precision publish the display already has, run once per span.

## What the kernel has to add

Three primitives, each small and each useful beyond windows:

- **Badges.** A capability to an endpoint can carry a 64-bit badge, set by whoever grants it (the spawner) and returned by `recv` beside the message. The map spawns an app with a display capability badged for that app, so the display keys windows by badge. A badge is not a PID: it names a capability, and only the granter chooses it. This is the seL4 way, and it fits KERNEL.md's "no ambient authority".
- **Death notices.** When a process ends, each endpoint it held a badged capability to receives one kernel message: the badge, and that it is gone. The display drops the window, the input routing drops its fingers, and the map learns that the app ended. This replaces polling op 5 for an app that has left.
- **Shared regions.** KERNEL.md already plans this: "Shared memory regions: explicit cap grant, both parties hold caps." At spawn, the spawner asks for a fresh surface region from a surface pool, granted to the child, with the same region granted to the display under the child's badge. No app can see another's pixels, and the display reads only the regions it holds.

## Knobs (the owner's picks)

1. **Caller identity:** badges set by the granter (recommended), or the kernel stamping each message with the sender's spawn epoch. Badges keep the kernel out of naming. The epoch is less code but is a PID by another name.
2. **Surface memory:** regions from a pool at spawn (recommended), or the display server owns one big pool and hands apps offsets. The second needs no kernel work, but every app can read and write every other app's pixels, so attest could be spoofed by an app.
3. **Pixel format in surfaces:** 64-bit working values (today: R, G, B as gamma-2 Q14 plus a no-dither bit) or 32-bit. A full-screen 64-bit surface is 32 MiB, and the pool has to hold one per live app plus the keyboard's band. 32-bit (10:10:10 gamma-2 plus 2 flag bits) halves that, at 10 bits a channel before the dither instead of 14.
4. **Who routes touches:** the display server (recommended) or the touch server with a region table. The display knows where every layer is. With routing there, the touch server has exactly one client and apps block on a "next input" op instead of polling, which also removes the 15 ms HELD poll from every app.
5. **Composing:** software spans through the existing publish (recommended first), or DECON's hardware windows, one layer per plane. Hardware planes cost nothing per frame, but the temporal dither and E's gains are software, so each plane would still need its own publish. Measure the software path first.
6. **The keyboard's place:** it overlays the bottom of the front app and the app is told its visible height (recommended), or the app's frame shrinks and it redraws. The first never resizes an app under the user. The second is what Android does, and it reflows text.
7. **Background apps:** an app not in front keeps running and keeps its surface (recommended, so returning is instant), or it is suspended and its surface released. The pool size decides how many can stay.

## The protocol, once those exist

Per window, keyed by the caller's badge:

- **show** `[x][y][w][h]`: this window's frame, from its own surface, laid out at its own width. Text into the window's surface uses window coordinates.
- **done**: this window leaves. Only the window's own badge can end it.
- **want text** `[on/off]`: the front app asks for the keyboard. The display raises layer 2 and tells the app its visible height.
- **next input**: blocks until a contact or a key for this window arrives, in window coordinates. Keys from the keyboard arrive here as bytes, so an app never talks to the keyboard directly.
- **frame wait** keeps op 24.

The map is a window like any other, on layer 0, with the camera ops it has today.

## Built (2026-10-27)

- **Surfaces.** Four full-screen surfaces of 32 MiB at G#CA00_0000 (cachemap SURFACES), between the display's canvas and the debug snapshot. The display holds the `surfaces` region; an app gets one surface of its own at spawn: the map passes the region (SYS_SPAWN turns it into one fresh surface) and `bridge spawn --dma surface` asks for one. The old shared `app-surface` (over the modem image at G#C000_0000) is gone.
- **Death notices, without messages.** Bytes 8-11 of every process's preferences block hold each surface's generation (0 free). The kernel frees a surface when its process ends or is killed (`diag kill <name>`), and the display drops a window whose byte changed on its next pass. Badges are not built: the window is named by its surface's base, which the display checks. There is no per-process memory isolation on the daily processes yet (they run native; SANDBOX.md's masking and handles exist and they migrate one by one), so nothing stronger would hold until they do.
- **The windows' owners' bell (2026-11-11).** Mahere, a kist, was ended from outside (`diag "kill Mahere"`) on the Pro; its window stayed in the display's table, op 5 kept saying an app was in front, and the map ignored every gesture but the corners until the display was restarted. Now the kernel pushes the owners: SYS_BELL source 9 (`BELL_SURFACES`, for the holder of the `surfaces` region alone) writes into a word of the display's the four surfaces' live generations (byte k surface k's while the process it was given to is alive and not ending, 0 otherwise, from the kernel's own table) and a change count above bit 32, as a surface is given (once its process is in the table) or freed (every end: exit, fault, panic, `diag kill`, a relaunch), waking the display's `reaper` thread asleep there. The reaper wakes the render thread, whose next pass drops every window shown under a generation the bell and the preference byte no longer both hold. Op 5's byte 12 is no longer `APP` but "a window up whose owner lives", from a copy of the windows' generations and the bell, answered by the receive thread, so the map hears of the end at its next op 5 whatever the render thread is doing. Op 22 reads the generation once (read twice, a process ending between the reads left a window kept under generation 0 that nothing ever dropped) and refuses a surface whose owner is gone. The kernel also gives back a surface taken for a spawn that is then refused, and `kill` passes over a process already ending. No timer anywhere: a process's end is the only thing that rings.
- **Composing.** Each row frontmost first (level, then the last shown): a pixel comes from the first window whose rectangle covers it and whose bit 63 (see-through) is clear, branchless; black behind them all. Then the glass's shape, from one plan per screen size (`plan_glass`, owner 2026-11-02: "maintain a global compressed mask since most will be opaque for the OS in general"): per row the chrome columns from each side (outside the circle, which is tangent to the bottom of the camera cutout and the bottom of the screen since 2026-11-02), the masked columns from each side (outside the screen's own edge, over the chrome) and the row's antialiased pixels, the circle's then the edge's, each with a coverage and a hairline alpha. The publish path does no geometry: it copies the runs and blends the listed pixels. The circle's hairline is one pixel wide on the radius, its profile (1 - t²)² across it (E grey one stop below the theme's ring on release builds, bright orange on development builds), planned from one crossing per row and the band's pixels. The edge is Photon's window on Android glass: a superellipse corner inside the glass's arc (fluor's glass corner rule at the panel's radius, 80 px on husky), the top-left and bottom-right corners twice the top-right and bottom-left, from fluor's crossings walk (`paint::edge`, shared with Photon's chrome): one antialiased pixel per axis on the curve, the hairline on the outer pixel and its complement on the inner one. Outside the curve the mask: black on release builds, where the edge hairline is black too (Photon's release rule on Android), the hairline's orange one stop darker on development builds. Then the one table and the dither, as before.
- **Ops.** 22 show `[x][y][w][h][base u64][level]` (then optionally `[format]`, `[changed x y w h]`, `[mode]`), 23 done `[base]`, 26 text `[base][x][baseline][size][value][utf8]` (synchronous), 27 whose point `[base][x][y]` (answered by the receive thread: one read of the hit map since 2026-11-11, COMPOSITOR.md "The hit map"). ferros_sys::window wraps them; fluor's ferros host speaks them.
- **The corners (2026-11-03).** Three circles in each corner (owner: "tangent on all sides, three for each"), solved from the glass exactly as `plan_glass` drew it (its fat and small corners, glass mode's diagonal pair) and the app's circle: the middle one maximises the clearance to the nearer of the two, so it touches both; each side one sits where the two clearances are equal and also equals its distance to the middle circle's edge, so it touches the glass, the app's circle and the middle one. Clearance to the glass is signed (outside the glass on a point's own row is none). Solved once per screen size and cached; a moving or resizing app circle needs the cache keyed on the circle and a Newton step from the last solution instead of the grid. Indicators up top, navigation at the bottom (owner, 2026-11-03): top left the clock (the time in the device's base), battery along the top, USB down the edge and GPS chained past the battery (tangent to the glass, the app's circle and the battery's); top right cellular, WiFi and Bluetooth the same way along the top and NFC down the edge (2026-11-11, both boards: the spot by the camera stays open as the quick row's doorway, QUICK.md); bottom right home, the rest of the bottom blank for navigation. IP is no radio: its latency and loss are the links' measures.
- **Link measures (2026-11-03).** No signal bars, and no one number for a link (owner: "frequency, bandwidth, speed, latency, flavor, packet loss ... RSSI, VSWR ... definitely not quantized to 5 bars"). Each driver publishes up to eight tagged measures (kernel SYS_STATUS 27, SYS_FACTS 33-80, eight words a link: modem, pcie1, bt, nfc, gps, usb; `ferros_sys::measure`): a word is the kind in bits 48-63 and its value, signed, in its own unit below (milli-dBm, milli-dB, kHz, bit/s, ns, ppm, thousandths for VSWR), all ones an empty slot, and a new measure is a new kind. The kernel keeps the words and reads none. A circle shows its name and each measure as a line in its unit, each line as large as its own chord allows. WiFi publishes RSSI, SNR from the noise floor, the centre frequency and width from the chanspec, the PHY rate and the transmit loss over two seconds (WLC_GET_RSSI, WLC_GET_PHY_NOISE, chanspec, WLC_GET_RATE, WLC_GET_PKTCNTS); USB its signalling rate; the modem its state and technology (its name shows LTE or NR), its signal waiting on IND_SIGNAL_STRENGTH (G#0906) decoded against Android's readings of the same moment, never guessed. VSWR has a kind; no driver measures it yet. Everything else a link knows, its names and addresses, and its knobs are on its page in settings (docs/RADIO-CONTROLS.md: tracks, the detail document behind Settings, the control blocks).
- **Flavours.** Each radio its colour, picked in VSF RGB and scaled to the grey's light by VSF's photopic weights: cellular amber, WiFi sea teal, Bluetooth periwinkle, NFC lime, GPS dusk magenta. The ring full when connected, a stop darker when up without a link, grey when the driver does not run; the name in the flavour, the measures in plain ink. Taps are not built.
- **Touches** still go to every client; each asks op 27 on a finger down and drops fingers that are not its own. Op 27 reads the display's hit map (2026-11-11, COMPOSITOR.md "The hit map"): a window owns the pixels where it is frontmost and not see-through, inside the circle, and never one the system holds (the corners, the quick row and what lies behind it while it shows); routing through the display can read the same map. Two levels: the display's map picks the window, the app's own fluor map (`HitId`) picks the widget.
- **Measure** is op 28 (2026-11-01): `[size u16][utf8…]` → `[0][width u32, quarter pixels]`, synchronous. It was op 27 for a day, which is "whose point"; the same collision as op 24.
- **Centred text** is op 29 (2026-11-01): op 26's layout with x the centre of the text's ink, left to right; the keyboards' labels (`window::text_centred`).

- **The system level.** The attest screen (payloads/ahau) is the first window at level 2: init starts it before the map, so nothing else shows until the person attests. Started by init through SYS_SPAWN, it got its own surface from init's `surfaces` grant, which exercised that path.

Verified on the phone by reading the dither phases back over USB: Hive alone; Ramps in front of Hive, cut to the circle; Ramps killed and Hive back; fluor's half-size frame over Hive with Hive around it.

## The layer stack (2026-11-02)

The owner's direction: "we will need to support full compositing. How do we maintain a layer stack and update slices?" and, on blend modes, "do it with blend modes". Built the same day in the display server (`layout_build`, `fold`, the span walk in `publish_app`); the geometry moves to fluor next so Photon's own stacking can share it.

- **A layer** is a window: its surface, its rectangle, its level and the order it was shown (the stack order, as before), its pixel format, and a mode byte on the show op (`ferros_sys::window::show_full`): bits 0-2 the blend mode in fluor's order (Normal, Multiply, Screen, Add, Subtract, Overlay, Darken, Lighten), bit 3 `OPAQUE`, the window's promise that none of its pixels is see-through.
- **Bands and spans.** The windows' top and bottom edges cut the screen into bands of rows; within a band every row is laid out alike, so the left and right edges cut it into spans. A span's chain is the windows covering it, frontmost first, cut after the first opaque one: nothing behind an opaque window is read. At most four windows, so at most nine bands of nine spans. The layout is rebuilt when a window is shown at a new place, level or mode, when one is done, and when one dies: never per frame.
- **Slices.** A show carries what changed (`show_changed`); each dither phase keeps its own dirty region, the union of what it owes. Publishing a row walks its band's spans clipped to the dirty columns: an empty chain is black; one opaque working window is a copy; working windows in Normal are the branchless select (see-through is the bit alone); anything else is the fold.
- **The fold** is fluor's `Blend::under` front to back on the working values: the composite so far carries a transparency (Q14 in bits 49-62 of the working pixel, 0 opaque, the see-through bit 16384); each layer behind adds the mode's value of (composite, layer) by what its alpha fills of what was still transparent, and the transparency multiplies. The mode applied when a layer is folded is the mode of the layer above it: a window's mode says how it sits on what is beneath. A fluor surface (`G2T16`) keeps its alpha through the decode, so a half-covered pixel of a fluor window really shows the window behind; a working surface declares only the bit. What is still transparent at the end is over black.
- **Cost.** Nothing where no layer uses a mode or an alpha: the copy and the select are what the server did before. A moded or alpha slice is a few integer operations per channel, on that slice alone. A moded window can never become a hardware plane later (DECON blends with alpha only); the rest of the model does not care.
- **The glass's shape** (the circle and the edge, `plan_glass`) applies after the spans, as the last layer of all.

## The corners: controls and gestures (proposed 2026-11-02, the owner sleeping on it, nothing built)

The owner's question: "controls. could be zero, could be a hundred. back, recents, home, search... should we? gestures?" The proposal is close to zero, with the four corners doing what is left, because the map already is most of it.

**What the map already is**

- **Home** is the map. An app is one of its circles opened in front; leaving the app is closing the circle, and the map is there with no transition.
- **Recents** is the map too: the circles that are open look open. A task switcher would be a second, worse map.
- **Search** is typing on the map: the keyboard is shape writing, so writing a name on the map filters its circles. No button, no search bar.
- **Back** belongs to the app. Android's system back exists because apps could not be trusted to draw their own; a fluor app draws its own way back. The exception is APKs (decision 0).

**Why the corners, and only the corners**

The corners outside the circle are the system's by construction: no app surface ever covers them, so a touch there can never be stolen by a fullscreen game or a misbehaving app. That makes them the escape hatch every OS needs, without a button. Each corner does the one thing it shows:

- Top left, the clock: time, alarms, the zone.
- Top right, the radios: connections, WiFi join, airplane.
- Bottom left, the battery: power, brightness.
- Bottom right: home. A tap from anywhere and the map is there. The only navigation control, and enough, because the map is home, recents and search at once.

Four corners, four things, nothing to learn. Hardware stays what it is: power wakes under the proximity gate, the volume pair is the killswitch chord and volume.

**Gestures**

Geometric ones only and nothing timed (the standing rule: no long press, no double tap, no swipe that depends on speed):

- Pinch an app down to close its circle: home by gesture, the same operation as the corner.
- Two fingers turn a circle over for its back and settings, as the map does now.
- Drag to pan the map.
- A drag that begins outside the circle is the system's by the same ownership rule: room later for an edge slider (volume, brightness) no app can collide with.
- No edge swipes from the glass edge: they collide with the screen's own mask and the apps' content, and they are the gesture most often triggered by accident.

**Decisions**

0. **APK back.** Android apps need a back signal. Either bottom right means back while an APK is in front and home from an APK's top screen (what Android's gesture ends up doing), or the APK runtime draws a small back control inside the APK's window and the corner stays home everywhere. Proposed: the second, one meaning per corner.
1. **Home as a tap, a pinch, or both.** Proposed: both; they are the same operation and neither is timed.

**Build order, once decided:** the bottom-right tap (one corner hit test and a done op), then the pinch, then the other three corners as their pages exist (ATTEST-UI.md's indicators give the top right and bottom left their faces first).

## The helm: home and back (2026-11-11)

Owner: "Home backgrounds the kist so depending on how it was designed, like Photon, it doesn't die but it stops rendering everything and stops playing anything except for notifications. and yes, both [taps and swipes]. back should signal escape to the app and go up one page. I'd say tapping on back bottoms out on the kist main page, so Photon that would be the contacts screen. if the user wants to go home, they can tap on the bottom right to go home." Home's swipe: "let's rotate it to find the most zoom we can too so it spins into place".

| | tap (still) | swipe inward (past a twelfth of the screen's harmonic span) |
|---|---|---|
| back (bottom left, hit id 9) | Escape to the app in front: up one page; at its main page nothing more (it never leaves the kist). With the map on the glass, a level back (`back_step`). | Flip to the kist before (this one behind). With the map on the glass, the last kist in front. |
| home (bottom right, hit id 8) | Home: the app in front behind, the map as it was left. With the map on the glass, an open circle closes. | Home, then the plane flies out to every circle at the turn that fits them largest, spinning into place. |

Full screen, which home's swipe switched until then, is the quick row's middle circle (QUICK.md). A running kist's circle tapped brings its window to the front instead of starting another copy.

- **Behind.** A window behind stays in the display's table with its surface and its process, but is in no stack: nothing of it is composed, it owns no pixel of the hit map, op 5 does not count it (the map has the glass), and its shows keep their place and format and publish nothing. Op 59 brings it back: to the front of its level, every phase owed the whole screen, the next published at once as a show's.
- **Ops** (all synchronous, answered by the render thread but 62):
  - **59 `front [surface u8][generation u8]`** → [0], or [3] for no such window: the window on that surface under that generation to the front, out from behind (`ferros_sys::window::front`).
  - **60 `behind`** → [0][surface][generation], or [3] with no app window in front: the app window in front (level 0, the last shown not behind) behind (`window::behind`).
  - **61 `key [usage u8]`** → [0][surface][generation], or [3]: a key by its USB HID usage to the app window in front (`window::key`; back's tap sends Escape, G#29).
  - **62 `events [base u64][seen u32]`** → [0][count u32][state u8: bit 0 behind][k u8][k events u16]: the caller's window's events after `seen`, held by the receive thread until there is one (a doorbell, never a poll; display events.rs). An event is its kind in the high byte (1 behind, 2 front, 3 key) and a key's usage in the low one; the last sixteen are kept, and the state byte says where the window stands whatever was missed. A surface's events belong to the generation it was given under: a new owner starts from nothing.
  - **43 `full screen`** now switches the app window in front's own flag (`Win::full`, kept while it is behind), [3] with none; `FULL` follows the app in front, so the home screen always has its bars. Full screen keeps the console's rows hidden only while the bars on the glass hold none of the quick row: pulled from the doorway, the console and the row show over the app, so its circle can switch full screen off again.
- **Which kist is which window.** The kernel answers a kist launch for a kist already running with `RUNNING_ON` | surface << 8 | generation (process.rs `kist_request`); init passes it on as RUNNING with the two bytes (ferros_kist::launch::RUNNING); the map raises that window (op 59). The map keeps the kists it brought to the front, most recent first, by capsule, for back's flip; a kist that ended since is launched again by the flip.
- **The app's side** (fluor's ferros host): a thread holds op 62; behind, the frame loop draws nothing and reads no touches, asleep until the display or the app's own threads ring, taking in its user events and its tick as they come; the app hears `Focused(false)` and `on_focus_changed(false)`. In front again: focus back, `on_window_shown`, the touches queued meanwhile dropped (they were made on other windows), and the next frame drawn whole. Escape arrives as a key press and release, `NamedKey::Escape`.
- **Home's turn** (map `home_turned`): the fit's zoom as a function of the turn has period a half turn; sampled 64 times across the half turn centred on the camera's own, then golden-section steps between the best sample's neighbours, and the flight's spring turns the short way. Where the zoom limit caps every turn alike, the camera keeps its turn.

## Order of work

1. Done: map frames kept off the glass under an app; text at op 26; the map stands aside for any app in front.
2. Kernel: badges and death notices, with `bridge spawn --badge` for development.
3. Display: a window table keyed by badge. Show and done per window. The front app is the last one shown that is still alive. A death notice removes its window.
4. Kernel and display: surface regions per app from a pool. The map's spawn and `bridge spawn` both ask for one.
5. Display: touch routing and the blocking next-input op. The touch server serves only the display.
6. Hive moves to layer 2 and sends keys through the display. Photon gets "want text".
7. Attest on layer 3, once ahau exists.

Each step leaves the phone working. Steps 2 and 3 alone fix glitches 3 and 4, and step 4 fixes 2.
