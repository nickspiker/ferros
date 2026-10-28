# Settings

What a person sets on a ferros device, where it is kept, and which way it is allowed to flow.

## Device and identity

A setting lives in one of two places:

- **The device** — the device's vault, in its settings document (below). It is what this device does for whoever holds it.
- **The identity** — the person's preference, carried with their identity across their fleet (Photon's fleet-wide `display.base` is one).

The two meet in one direction only. **When an identity attests on a device, the identity's preference is written to the device.** Nothing ever goes the other way: a device setting is never read and written into an identity's preference, whatever its value. A device can be set by anyone holding it; an identity's preferences are that person's own, and a device that someone else configured must not be able to change them.

A device setting sticks until it is set again — by the person holding the device, or by the next identity that attests.

## How the device keeps them

One spec VSF document in the vault under the key `device` (ferros_kernel `settings.rs`, since 2026-10-27; owner: "All device settings should be proper spec VSF for aid in vault storage"): a section `device` with a typed field per setting, stamped with its Eagle time and a provenance hash. It replaced the raw one-byte `device.*` keys, which nothing reads any more.

| Field | Type | Values | Absent |
|---|---|---|---|
| `base` | unsigned | tatau's code: 10 arabic, 12 DMS, 16 hex, G#8C DLS | DMS |
| `labels` | unsigned | 0 off, 1 on | 0 |
| `brightness` | unsigned | 0-8 stops below the brightest white | 2 |
| `dither` | unsigned | 0 spatial and temporal, 1 spatial only | 0 |
| `theme` | unsigned | an index into ferros_sys::theme::THEMES | 0 |
| `prox_wake` | unsigned | 0 off, 1 on | 0 |
| `scope` | unsigned | 0 off, 1-6 the light scope's channel | 0 |
| `layout` | unsigned | 0 Dvorak, 1 QWERTY with Q on the bottom row (honeycomb) | 0 |
| `unlock_at_boot` | unsigned | 0 off (the handle is typed each boot, nothing of the identity on disk), 1 on (the seed is stored so the phone unlocks without typing) | 0 |
| `tz` | signed | seconds east of UTC | 0 |

- **Loaded** by the kernel right after the vault opens: the defaults, then every valid field, into every process's preferences block.
- **Written whole** on every change: a process holding Settings (`SYS_SET_PREF`), `ferros-bridge diag "pref <name> <value>"`, or a `device` document stored from the bridge (applied at once).
- **The language** joins as its own VSF type (ISO 639-3 packed, with optional ISO 15924 script and ISO 3166-1 region, all numbers), asked of the VSF session on 2026-10-27. Like every field here it follows the last identity to attest. Until then the device is English.

## The number base (`device.base`)

Every number a person reads is drawn in one of four bases (`Code/tatau`): arabic (decimal, with unit words), hexadecimal (linear, for coders), DMS (dozenal magnitude: doublings) and DLS (dozenal linear: the value with its sign arrow and SI unit, `↑Zil·Lun Zil Zil s`). Time of day and dates read the same in DMS and DLS.

- **Kept** as the settings document's `base`: tatau's code, 10 arabic, 12 DMS, 16 hex, G#8C DLS. Absent means DMS, the house base.
- **Loaded** by the kernel right after the vault opens, into tatau's process-wide base, so everything drawn from then on reads in it (`ferros_kernel/src/main.rs`).
- **Set** by the person on the device — the back of the device's own circle on the map (SHELL.md), where each base is a row showing two seconds and a mebibyte written in it, and a tap chooses — or `ferros-bridge diag "pref base <code>"`, or by an attesting identity's preference.
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
