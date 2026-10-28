# Windows on ferros

The design for putting more than one thing on the glass: the map, an app in front, the keyboard over it, and the attest screen over everything. Written 2026-10-27 after the owner called the present path "a glitchy hot mess". It is a proposal: the knobs below are the owner's to pick before the display server's code changes shape (Claude memories/display_driver_design_first.md).

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

## Order of work

1. Done: map frames kept off the glass under an app; text at op 26; the map stands aside for any app in front.
2. Kernel: badges and death notices, with `bridge spawn --badge` for development.
3. Display: a window table keyed by badge. Show and done per window. The front app is the last one shown that is still alive. A death notice removes its window.
4. Kernel and display: surface regions per app from a pool. The map's spawn and `bridge spawn` both ask for one.
5. Display: touch routing and the blocking next-input op. The touch server serves only the display.
6. Hive moves to layer 2 and sends keys through the display. Photon gets "want text".
7. Attest on layer 3, once ahau exists.

Each step leaves the phone working. Steps 2 and 3 alone fix glitches 3 and 4, and step 4 fixes 2.
