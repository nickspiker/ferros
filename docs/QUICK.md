# The quick row

Owner, 2026-11-11: "gesture ideas, would be nice to have the torch, brightness and maybe a few other things." On the proposal (pull down from the top edge where the status circles live, circles that grow with the drag, settle by physics, a second row of circles, long press for a radio's page): "Works". Then: "I'd also think NFC is rarely used so let's move it back to the bottom right like we do on the 8a so we have the same open spot where the camera is. any tap in that area or drag down should bring up said second row of consoleness, however we want to do that, some circle fitting goodness I suspect."

Built 2026-11-11 (branch `quick-settings`), not yet run on a device.

## The doorway

- NFC is down the right side on both boards now (display `corner_layout`), so the top row is clock, battery, GPS on the left and cellular, WiFi, Bluetooth on the right, and the spot by the camera is open on both.
- **The doorway** is the disc about the camera hole (`ferros_sys::cutout_hole`) out to the nearest top console circle, outside the app's circle (display quick.rs `doorway`; its radius is logged once per size as `display: quick row … doorway N`). Nothing in it is a fixed pixel: the hole is measured glass, the corners are solved from the glass.
- A still tap in the doorway opens the row, or shuts it.
- A drag down from the doorway, from the top's other ground (the clock, battery and USB circles and the air between the corners) or from a radio circle pulls it.

## Where the row sits: circle fitting inside out

The corners' circles are each tangent to the glass, the app's circle (from outside) and their neighbour. The quick row is the same fit turned inside out: six circles in the lens between the top of the app's circle and the console's last row (the first row where the app circle's chord is the whole width), each tangent to the app's circle from inside, to that row from above, and to its neighbour (display quick.rs `layout`). The middle pair touch each other on the centre line, under the camera; the lens thins toward the sides, so the circles shrink outward.

Left to right: radios off, sound, brightness | torch, do not disturb, screencap. The two rings (brightness, torch) are the middle, largest pair.

Solved on the host from the same equations (the device logs its own):

| | Middle pair | Next | Outer |
|---|---|---|---|
| husky (1344 × 2992) | r 81 at x 672 ± 81, y 203 | r 72 at ± 234, y 212 | r 58 at ± 363, y 226 |
| akita (1080 × 2400) | r 65 at x 540 ± 65, y 170 | r 58 at ± 188, y 177 | r 46 at ± 292, y 189 |

The row is drawn by the console's worker in the console's own buffer (it lies within the console's rows), with the console's region machinery: the quick circles are console circles that move, and a redraw empties where each was as well as where it is. No new buffer, no new layer; the bars are already composed in front of the home screen, every app and the flight.

## The pull and its physics

- **The pull `p`**: 0 shut, 1 open. While a finger drags it, `p` is where it was when the finger took hold plus the finger's travel over an eighth of the screen's harmonic span (2wh/(w + h): 232 px on husky, 186 on akita), unbounded.
- **The row is drawn at `ease(p)`**: `p` itself thru the middle (the circles follow the finger one to one), exactly 0 below and exactly 1 above, C∞ everywhere. `ease` is the ramp `p` with each corner smoothed by a C∞ bump step of half-width an eighth; no clamp anywhere.
- **At level `o`** every circle has `o` of its radius and `o` of its angle about the app circle's centre, still tangent to the app circle from inside: the row grows out of the point under the camera and fans out along the arc. Neighbours never overlap on the way.
- **The lift**: from shut, the row settles open if it was pulled past a third of the way, else shut; from open, it settles shut if pushed back past a third, else open. It settles on a critically damped spring (ω a quarter radian a frame: from rest it lands within a thousand-and-twenty-fourth in about 37 frames, a third of a second at 120 Hz) that starts at the finger's own speed, so position and speed are both continuous at the lift and it never overshoots from rest.
- **Frame timer on shown frames**: the spring steps once for each frame the panel showed (display op 24's frame-start count), never on a clock, so a missed render does not slow it. It lands exactly on 0 or 1 once within a thousand-and-twenty-fourth and nearly still.
- **Shutting**: push it back up (from the doorway, the top's ground or any quick circle but the rings), a still tap in the doorway, or a touch anywhere behind the row (that touch goes no further).

## The circles

- **Brightness** (a ring): a finger on it turns it, a whole turn the whole range of preference 3 (32nds of a stop, 0 the brightest, 255 the dimmest), eased at both ends as the pull is, so the value slows into each end. Applied live while the finger turns (`set_pref_live`) and kept once at the lift (`set_pref`), as the Appearance slider does. The ring's lit share is how bright it is; while a finger turns it, the display draws the exact share the map sent, so the lit end moves smoothly between the preference's steps.
- **Torch** (a ring): a tap lights it at the last level it was lit at (the whole dimmer at first) or puts it out; a turn sets the level, 1-255 the torch's own perceptual dimmer (docs/TORCH.md). The map calls the `torch` endpoint waiting (`call`, not `call_live`: the kernel starts the torch process on demand and a live call would be refused while it starts), from a thread of its own (map quick.rs `worker`), so the main loop never waits on it. The torch's level is asked once each time the row starts to show until it has answered. A torch that does not answer (no capsule stored, an older kernel with no grant) draws dimmed and a tap does nothing.
- **Radios off**: every radio off at once, the set that was off before kept (preference 41, `radios_before`); a tap from all off puts that set back (all on when it was every radio).
- **Sound**: a tap goes round sound, vibrate, silent (preference 39, a device setting). Its word is the mode now. Nothing plays a sound or a buzz by it yet.
- **Do not disturb**: the person's, not the device's (SETTINGS.md). A tap asks the identity service (fgtw op 24) from the map's worker thread; the service puts it in the person's space, hands it to their fleet and fgtw.org, and writes it to the kernel (preference 40), which the circle shows. Before the identity service's first pull it is refused and the circle does not change. Nothing is silenced by it yet.
- **Screencap**: the row shuts first; when it has landed the map sends op 56 with the capture flag, and the display takes the capture (as op 30 does) the first pass the bars on the screen hold none of the row, so the row is not in the picture. There is no clipboard yet (owner, 2026-11-07: "when we get the clipboard running that's where the screencap will live"), so the frame stays in the free surface op 30 wrote it to and its address goes to the console (`display: quick row screencap (no clipboard yet): G#…`).
- Rotation lock: none (no rotation today).

Each circle's word is ferros_lang's, in the person's language, a word a line. A circle is lit while what it switches is on; a finger on it lifts it to the live ink's light.

## Long press on a radio circle

A finger that rests on a radio circle (within the slop) for 64 shown frames (about half a second at 120 Hz) opens that radio's page in settings; meanwhile a ring fills around the circle, by the same frames. Its lift then switches nothing. A tap still switches the radio; a drag down from it pulls the row. This replaces the drag toward the centre (2026-11-07), which on the top corners now pulls the row.

This is a hold measured in shown frames, the one place a gesture waits on frames rather than being told by movement: the owner chose it on the proposal ("Long press on a radio circle opens its Settings page"). `LONG_PRESS` in map quick.rs is the one knob.

## Who does what

- **Display** (payloads/display quick.rs): the geometry (`layout`, `at`, `doorway`, `ease`), op 56 (the map's state of the row, stored in two words and the console's worker nudged), op 41's answers (32 + k with the circle's centre and radius, 10 the doorway, 11 the top's other ground, 12 behind the row), op 27 (a quick circle is never an app's point), the drawing (in `console_draw`), and the screencap.
- **Map** (payloads/map quick.rs): the finger, the pull, the spring, the rings' turning, the long press, every action, op 56 sent whenever anything in it moved. Touches reach the map first for the system's places, with an app in front too, as the corners' always have.
- **Kernel**: preferences 39 (sound), 40 (do not disturb, the identity's, taken from the process named fgtw alone) and 41 (radios before), kept in the device document; and the torch's send right handed to whoever it hands Settings (the map's slot 9), since init's grants page is full.
- **fgtw** (payloads/fgtw space.rs): op 24, the person's do not disturb into the space and to the kernel; ferros_space kind `DND`.

## Op 56

`quick [pull i32 Q16][flags u8][torch level u8][held u8][press radio u8][brightness ring u16 Q16][torch ring u16 Q16][press u16 Q16]`, 15 bytes:
- flags bit 0: the torch answers (its level is the next byte); bit 1: a screencap now that the row is shut.
- held: the quick circle a finger is on (G#FF none). Press radio: the radio circle a finger presses (G#FF none), with how far the press has come.
- A ring's share G#FFFF: none sent (the display takes brightness from preference 3 and the torch from its level).
