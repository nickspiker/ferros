# Settings

What a person sets on a ferros device, where it is kept, and which way it is allowed to flow.

## Device and identity

A setting lives in one of two places:

- **The device** — the device's vault, under a `device.*` key. It is what this device does for whoever holds it.
- **The identity** — the person's preference, carried with their identity across their fleet (Photon's fleet-wide `display.base` is one).

The two meet in one direction only. **When an identity attests on a device, the identity's preference is written to the device.** Nothing ever goes the other way: a device setting is never read and written into an identity's preference, whatever its value. A device can be set by anyone holding it; an identity's preferences are that person's own, and a device that someone else configured must not be able to change them.

A device setting sticks until it is set again — by the person holding the device, or by the next identity that attests.

## The number base (`device.base`)

Every number a person reads is drawn in one of four bases (`Code/tatau`): arabic (decimal, with unit words), hexadecimal (linear, for coders), DMS (dozenal magnitude: doublings) and DLS (dozenal linear: the value with its sign arrow and SI unit, `↑Zil·Lun Zil Zil s`). Time of day and dates read the same in DMS and DLS.

- **Kept** in the device vault as `device.base`, one byte: tatau's code, 10 arabic, 12 DMS, 16 hex, G#8C DLS. Absent means DMS, the house base.
- **Loaded** by the kernel right after the vault opens, into tatau's process-wide base, so everything drawn from then on reads in it (`ferros_kernel/src/main.rs`).
- **Set** by the person on the device (the settings screen, to come; for now `bridge store put device.base @<one-byte file>`), or by an attesting identity's preference.
- **Drawn** as glyphs only where the font is ours (fluor with the Oxanium `+glyphs` face: the shell and the apps) and as words everywhere else. Terminals, bash and anything else without font control are not a supported surface for glyphs.

Developer output keeps its own convention (`G#` hexadecimal, `A#` decimal, CLAUDE.md): it is read by developers in terminals and logs, not by the person holding the device.
