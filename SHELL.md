# SHELL — what the owner sees when ferros wakes

## The spec (owner, 2026-10-21)

**Your phone is a map of your life.**

Every person, message, photo and app is a circle on one plane. Zoom out and you're home: a quiet map where the only things lit are the things that changed since you last looked. Zoom in and you're reading the thread, in place. Nothing ever moves, so after a week you navigate by memory, not by search. There are no notifications, no feeds, no badges. Apps run in sealed circles and lose the right to speak unless a human is behind the message. An agent on the device, not in the cloud, is the only interface you talk to, and it keeps a log of everything it did for you. No lock screen, because the phone already knows it's you. Boots in under a second to the same map every time.

One line: **a phone where nothing moves, nothing shouts, and you always know where you are.**

This replaces the 2026-10-16 design, in which home was a status line over a list of people and apps were a second screen of rows. What carries over unchanged:

- **Design the wake, not the boot.** A wake happens a hundred times a day and is deliberate (power pressed, proximity far). The screen shows state, not a menu.
- **The OS's own record is the only launcher.** App circles come from the vault's `sealed` list; nothing else may appear as an app.
- **No "secure" badge, ever.** The map states facts; it does not reassure.
- **First boot** pairs with a device you trust before anything else. On the map that is the one circle there is.

What the spec retires: the unread *count* on the resting face (a count is a badge — the resting face shows the map with what is lit, and nothing numeric), and the open question of whether a lit screen gates before showing names (there is no lock screen: identity is the device's, through the ira and the proximity-gated wake).

## How it maps onto ferros

**The plane.** One world-space plane; every circle has a position in it. The view is a camera over the plane — pan and zoom — and the camera is the only thing that moves.

**Nothing ever moves** is a storage invariant, not a rendering choice. A circle's position is written to the vault once, when the circle first appears, and never rewritten. That is what makes navigation by memory possible, and it is enforced where positions live rather than hoped for where they are drawn.

**A circle** is: an identity, a kind (person, thread, photo, app), a fixed position, a size, a label, and two times — when it last changed and when it was last seen.

**Size is time** (owner, 2026-10-21). The whole map is one day. Each circle's area is the projected time a day spent on that thing, so the circles together cover the time spent on the phone and **the air between them is everything else** — sleep, a walk, the people in the room. The owner's layout rules, *more air is better* and *keep it simple*, stop being a preference and become literal: a life less taken by the phone draws a quieter, emptier map. It also shows the attention economy for what it is without a word of commentary — an app that takes three hours a day is a large circle, and you watch it shrink when you take the time back.

- Radius goes as the square root of time, not as time: the eye reads area, and a linear radius would draw something used twice as much four times as big.
- Size and position are separate. Position comes from what a circle relates to and is written once; size follows time and breathes. *Nothing moves* means a thing is always where you left it — its centre stays put while it grows or shrinks, as a tree does — so each circle is placed with air to grow into.
- Projected time is slow: a rolling average over weeks, measured on the device from what the owner actually zooms into and how long they stay. The map changes like a season, not a feed. A new circle starts small and earns its size.
- The time measurements never leave the device. They are the most personal data on it.

**Importance is the ring** (owner, 2026-10-21). Time measures attention, not love, so importance gets its own channel: the circle's outline weight, in a few discrete steps — none, thin, heavy — since nobody reads seven stroke widths apart. Someone called once a month becomes a small circle with a heavy ring: little time, much meaning, both shown and neither overwriting the other. Brightness was rejected for it: the map's one signal is that lit means changed, and permanently bright important circles would be a standing shout — and, on an OLED, a standing drain.

Each channel carries one meaning:

| channel | meaning | source |
|---|---|---|
| position | what it relates to | placed once, never moves |
| area | time spent | measured by the device |
| ring | importance | declared by the owner |
| colour | changed since last seen | events |

The device measures what you do; you declare what matters. When a circle lights, the colour goes on its fill, so the ring stays as its steady weight underneath.

**Apps to start:** Photon (people and threads, beam for video and wave for voice), Mahere (maps), Lumis (the camera), Opsin (recording).

**An app is its icon** (owner, 2026-10-21). An app's circle shows the app's icon and no name: "icons identify just fine." Names under circles are an option in settings, off by default. This reverses the 2026-10-16 design's "No icons", which belonged to a list of rows, where a name was the whole row. Icons come from each app's own assets — Photon's and Mahere's as PNG, Opsin's already as VSF — and fluor's `Icon` reads VSF and already renders an icon as a circle (`to_rgba_circular`). Lumis has no icon yet.

**Lit** means changed since last seen. Seeing a circle — zooming in until it can be read — updates its last-seen time. That is the only signal on the map: no counts, no badges, no motion.

**Apps lose the right to speak.** The only path that may update a circle's last-changed time is a message a human originated, arriving through Photon. An app has no channel to light its own circle; that is a property of the shell, not a permission an app could request.

**Zoom is semantic.** Far out, circles are points and only the lit ones show. Closer, they carry names. Closest, a circle opens into its content where it stands — the thread is read in place, not on another screen.

**Touch.** One finger pans; two fingers pinch to zoom. The touch server already serves held contacts with position and pressure, so a pinch is two held contacts and the distance between them.

**The agent** is its own process: the one interface the owner talks to, on the device, with an append-only record of everything it did. `ferros_ledger` (a categorised event chain, VSF from entry zero) is the natural home for that record.

**Under a second, the same map.** The map's state — positions, last-seen times, the camera — lives in the vault, and boot opens it and draws the last view. That is the existing one-second boot budget, spent on the map.

## Who draws what

The display server paints; the shell owns the map; Photon serves people and threads; the vault serves app circles and holds every position; the touch server serves contacts; the agent is separate.

The 2026-10-16 split — the shell sends draw operations one per call, the display server paints them — is right for rows and wrong for this. A pinch-zoom over hundreds of circles at 120 Hz would be hundreds of calls a frame. Since nothing moves, the scene is almost static and only the camera changes, so the proposed split is a **retained scene**: the shell hands the display server the circles once and again only when they change, and per frame sends only the camera — three numbers. Frame pacing then stays in the process that already owns the panel's tearing-effect signal.

## First slice

1. The display server learns a retained scene: an op to upload circles in world coordinates, an op to set the camera, and a circle primitive in its fluor renderer.
2. The shell reads the touch endpoint: one-finger pan, two-finger pinch.
3. A plane of real circles — the vault's `sealed` apps — at positions written to the vault once and never moved.

That is the whole idea in miniature: a plane, nothing moves, fingers move the view, the data is real. Lit circles, semantic zoom, Photon's people and the agent follow.

## Open

- **Growth against neighbours.** A circle that grows a great deal over months can outgrow the air it was placed with. Whether that is resolved by placing generously enough that it never happens, by a rare settling the owner asks for, or by letting circles overlap is open.
- **Placement.** Who decides where a new circle lands, given it can never move afterwards: the owner, placing it once by hand, or the system, deterministically — say, beside what it relates to, a new thread next to its person?
- **Icons and the one colour.** If lit is the only colour on the map, full-colour icons put colour everywhere and the signal is gone. One answer: icons drawn in grey at rest, identifying by shape, and in full colour only while lit — so colour still means exactly one thing.
- **What lit looks like** at the home scale: brightness, a glow, or the only colour on an otherwise grey map. It renders through the pinned gamma2tanh profile either way.
- **The clock** (eagle time, dozenal) and whether the map shows it at all, or the agent answers when asked.
- **Capture.** The 2026-10-16 design put it on a hardware chord, never behind a list. Nothing in the spec contradicts that; Lumis would open from it.

The 2026-10-16 mock is `tools/shell-mock` (fluor, headless, the panel's real geometry; `cargo run --release -- <out dir>` writes PPM frames), and the 2026-10-16 home screen still runs on the phone as `payloads/shelltest` through the display server's `display` endpoint (DISPLAY.md), in Oxanium.
