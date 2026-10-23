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
- **Set** by the person on the device — the back of the device's own circle on the map (SHELL.md), where each base is a row showing two seconds and a mebibyte written in it, and a tap chooses — or `bridge store put device.base @<one-byte file>`, or by an attesting identity's preference.
- **Drawn** as glyphs only where the font is ours (fluor with the Oxanium `+glyphs` face: the shell and the apps) and as words everywhere else. Terminals, bash and anything else without font control are not a supported surface for glyphs.

## Names under circles (`device.labels`)

Whether the map writes a circle's name under it: one byte, 0 off (the default: icons identify) or 1 on, preference 1 in every process's block. Set from the same back of the device's circle, kept and applied exactly as the base is.

## Brightness (`device.brightness`)

Brightness is a whole number of stops below the brightest white, 0 to 8, one byte, preference 3 (absent = 2). It is set on the device's back as a row of nine dots, dimmest on the left.
- **Step 0** puts white at half the panel's peak: the owner's "max white one stop under max". On this unit's E-white peak of 1178 cd/m², that is 589 cd/m².
- **Each further step halves white:** 294, 147, 73, 37, 18, 9, 4.6 and 2.3 cd/m².
- **The panel's own brightness** stays at its high-brightness peak while that peak is within 16× of white, the headroom the working format holds. Below that, it comes down so its peak is exactly 16× white, set from the measured DBV table.
- **The display server** rebuilds its one table (tanh into the peak, white exactly on target) and republishes the frame when the preference changes. On the X3 the rebuild takes about a millisecond.

Measured on the phone, 2026-10-22:

| Step | White | E peak | Panel |
|---|---|---|---|
| 0 | 589 | 1178 | HBM |
| 2 | 147 | 1178 | HBM |
| 5 | 18 | 294 | DBV 2126 |
| 8 | 2.3 | 36 | DBV 831 |

## How a process sees and changes them

- Every process reads the current values from its own grants page (`ferros_sys::device_base`, `ferros_sys::pref`): the kernel rewrites that block in every live process the moment a value changes.
- Changing one takes the Settings capability (`ferros_sys::set_pref`): init holds it and passes it to the map, whose device circle is where settings live.

Developer output keeps its own convention (`G#` hexadecimal, `A#` decimal, CLAUDE.md): it is read by developers in terminals and logs, not by the person holding the device.
