# The global hit map

The owner, 2026-11-27: "what if we have a global hitmap too and then we can manage it and call the app or deck or whatever"; then "I think it would still be plentyfine to use a u16 and just do our own bookkeeping. it's almost a nice place for a teency hash map with collision ness, anyway, I think we build it! It sounds really delightful. Accesability and test hits, yes. and we just treat it like the alpha or the RGB".

One map of the glass, a u16 a pixel, that says what every pixel is: the system's place, the deck's circle, and inside an app's window the app's own widget.
The display keeps it and answers from it: the debug view sees it, a touch is routed by it, accessibility and the test harness address widgets by it.

## Where it starts (2026-11-27)

- **The display's map** (hit.rs, COMPOSITOR.md "The hit map"): a u16 a pixel, written by the compose front to back when the layout changes, holding the system's places (`SYS` + code), the deck (`PLANE`, `REGION`, `CIRCLE` + index) and an app's window as one id (`WINDOW` + surface).
- **The app's map** (fluor's `HitId`, u16, `claim_hit` front to back, the owner's "if it's showing on screen, it NEEDS to be hitting the hitmap, if it's already opaque and won't show? SKIP!"): one a pixel of the app's frame, inside the app's process; the display never sees it.
- **Touches** do not pass thru the display: every client reads the touch server and asks op 27 whether a finger is its own; fluor's host makes the coordinates frame-local and indexes its own map.
- WINDOWS.md already names the two levels: "the display's map picks the window, the app's own fluor map picks the widget".

## The design

### The id is a channel

An app's id plane is laid out as its pixels are: the panel's size, the same index (a surface's coordinates are the screen's), the same scroll ring.
The compose treats it as it treats alpha and colour: front to back, `under`; where a layer owns a pixel (the same test the window map uses: G2t16's α not 0, bit 63 clear for the others) its id is the pixel's id, and nothing behind is read.
A pixel see-through in the app's picture is see-through in its ids too, so what is seen and what is touched never disagree, and no window can claim a pixel it does not show.

### Where the plane lives

The display serves it (built 2026-11-27): an app asks with op 88 naming its surface, and the display gives it a run of the display's own pool read-write (SYS_SHARE `give`, as ip gives its socket rings), at k runs into the pool for surface k, cleared first.
A view the other way, the app's memory given to the display, was the first design; the kernel gives a run only from a server to the caller it answers, and apps are the display's callers, never the reverse.
The run: a head of 64 bytes (its first u32 the plane that goes with the frame shown, 0 or 1), then two planes of u16 ids in screen coordinates; the app writes the next frame's ids into the one not shown, then names it in the head, so the ids read belong to the picture on the glass.
Its size is the panel's (proof: it can never bind anything the panel shows); the pool grows to hold the runs of the surfaces that asked, and a surface slot keeps its pixels alone (32 MiB on husky, full already).
A plane counts only while the surface's holder is the one it was given to (the windows' bell's generation), so a dead app's ids never show for the next.

### The bookkeeping

Global ids are u16, handed out by the display; a small table maps each to (surface, the app's own id) and back (the owner: "a teency hash map with collision ness"): open addressing, linear probing, doubled when half full (hit.rs `Table`).
The display's own ids (`SYS`, the deck's) keep their numbers, all under G#2000; an app's are given out from G#2000 up as first seen, and an entry goes when its surface changes hands (`sweep`, at each list).
The debug view needs no table: it colours each pixel by its key (the display's id, or the surface and the app's id), so the painting threads share nothing.
The global id is what the debug view colours, what the accessibility and test ops name, and what the display says when a touch lands.

### What reads it

- **The debug view** (op 58, the fourth phase): every app's widgets in their own colours beside the system's places, instead of one colour a window.
- **Touch routing:** a finger down is answered with (window, the app's own id, frame-local x and y); the id says what was touched, the coordinates how (a drag, a pinch, a scroll still need them). An app without a plane keeps today's path: its window as one id, and coordinates.
- **Accessibility and tests:** an op lists the ids on the glass (global id, window, the app's id, its bounding box), and one taps by id: the test harness says "tap Mahere's search field", never "tap at 412, 1880".
- **The killswitch:** the system knows what every touch landed on whatever an app does; the bars and the deck never ask an app.

### The cost

- The plane is u16 a pixel: 8 MB a plane at husky's 1344 x 2992, two a window.
- The window map stays written only when the layout changes; an app's ids are read where it is needed: at a finger down (one pixel), in the debug phase (the screen once a showing), by the list op (on demand).
- The GPU compose writes no ids (GPU-COMPOSE.md decision 10); nothing here changes that.

## Stages

0. **The design** (this file), for the owner to mark up.
1. **The plane** (built 2026-11-27, not yet run on a device): display op 88 gives a surface's holder its plane; fluor's ferros host asks at start and copies its `HitId` map into the plane not shown at every present (fluor src/host/ferros/mod.rs `HitPlane`). Apps with a map: basecalc, Photon (its chrome), Kūaha. Mahere's ferros build has none yet (its touches are its own two-finger solve).
2. **The table and the debug view** (built 2026-11-27, not yet run): the fourth phase coloured by each pixel's key, so every app widget shows in a colour of its own; the table hands out ids for the list.
3. **The list and tap** (built 2026-11-27, not yet run): display op 89 `hits` (the system's, or anyone's on a development build): every id on the glass with its surface, the app's own id, its box and a pixel inside it; `ferros-bridge input hits` prints it, `ferros-bridge input tap #<id>` taps that pixel (payloads/capture prints long replies 256 bytes a line, then `end`).
4. **Touch routing by id:** the display answers a finger down with (window, id, local x, y); fluor's host takes the id from it.
5. **Pucks:** a puck's square gains an id plane beside its pixels, sampled with them into the circle, so a face's parts are addressable too.

## Open

- A plane in the slot (once slots grow on demand) or a view: the view first, since it is built.
- Whether the deck's circles take the app path too (the map writing its own plane) or keep the display's `CIRCLE` ids.
